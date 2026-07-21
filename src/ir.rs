//! The instruction-body layer (Tier C foundation): a JS-facing description of a
//! function's instructions (`InstrDesc`), the emit path that turns a descriptor
//! array into a walrus function body (`build_function` -> `emit_desc`), and the
//! read-walk that turns a parsed `LocalFunction` back into descriptors
//! (`WasmFunction::instructions` -> `read_instr_seq`).
//!
//! The two directions are exact mirrors of each other so a body round-trips
//! (parse -> read -> build -> emit -> parse -> read is structurally stable).
//!
//! ## Label stack (the one genuinely tricky part)
//! wasm branch targets are RELATIVE label depths (`0` = the innermost enclosing
//! block/loop/if), but walrus stores them as ABSOLUTE `InstrSeqId`s. Both the
//! emit and the read walk maintain a parallel `Vec<InstrSeqId>` label stack:
//! entering a control construct pushes its sequence id, leaving pops it, and the
//! function's entry sequence is the outermost frame (matching walrus, whose emit
//! pushes the `FunctionEntry` block first — see walrus `emit.rs`).
//!
//! * emit (depth -> id): `label_stack[len - 1 - depth]` (out-of-range => Err).
//! * read (id  -> depth): `rev().position(|b| b == target)` — the exact inverse,
//!   identical to walrus' own `branch_target` (`emit.rs:1171`).
//!
//! An `if`'s two arms are two separate walrus sequences, each its OWN label
//! frame: a `br 0` in the `then` arm targets the consequent sequence, a `br 0`
//! in the `else` arm targets the alternative sequence. This mirrors walrus'
//! parser (which pops the `If` control frame and pushes an `Else` frame at the
//! `else`) and its emit (which pushes the consequent then, separately, the
//! alternative). So recursing into an arm adds exactly ONE frame bound to that
//! arm's own sequence id — the same shape as `block`/`loop`.
//!
//! ## MIRROR-WALRUS
//! Only process-aborting hazards are guarded: an out-of-range local/global/func/
//! memory/data/table/element index, an out-of-range branch label, a bad
//! multi-value or `call_indirect`/`return_call_indirect` type index, a
//! `ref.null` heap type naming a foreign/deleted/entry concrete type, or a
//! `MemArg` offset that is not a lossless `u64` are rejected with a catchable
//! error BEFORE a panicking walrus lookup can be reached. Nothing here validates
//! wasm well-formedness — an ill-typed body (a non-power-of-two alignment, an
//! out-of-bounds access, an atomic op on a non-shared memory, a `call_indirect`
//! whose type does not match the callee, a `ref.func` to an undeclared function,
//! a tail call whose signature does not match, …) is emitted as-is and left for
//! `WebAssembly.validate` to reject.

use napi::bindgen_prelude::{BigInt, Result, Uint8Array};
use napi::Error;
use napi_derive::napi;
use walrus::ir as wir;
use walrus::ir::{BinaryOp, TernaryOp, UnaryOp};
use walrus::{
  DataId, ElementId, FunctionBuilder, FunctionId, GlobalId, LocalFunction, LocalId, MemoryId,
  Module, TableId, TagId,
};

use crate::convert::{
  checked_index, heap_type_to_walrus_in, resolve_type_id, val_type_to_walrus_in,
};
use crate::handle::bigint_to_u64;
use crate::valtype::{HeapType, ValType};

/// The maximum control-flow nesting depth the instruction walks — the three
/// in-module walks (`validate_body`, `emit_desc`, `read_instr_seq`) and the
/// iterative FFI decode (`crate::ir_marshal::InstrBody`) — will descend before
/// refusing with a catchable error.
///
/// ## Why a cap at all
/// walrus is fully ITERATIVE over nesting — it parses with an explicit
/// `ControlStack = Vec<ControlFrame>` and emits via `dfs_in_order`, an explicit
/// `stack: Vec<(InstrSeqId, usize)>` — so it can parse/emit arbitrarily-deep
/// `block block … end end` without a stack overflow. Our three in-module walks,
/// however, RECURSE once per nesting level, and Rust `Drop` of a nested
/// descriptor tree recurses too. Under `panic = abort` a Rust stack overflow
/// is a `SIGABRT` that `catch_unwind` does NOT catch, so it would tear down the
/// whole Node process across the FFI boundary — an uncatchable abort reachable
/// from either a JS-supplied deep `body` (build) or a legitimately deep parsed
/// module (`.instructions()` read). Capping converts that abort into a catchable
/// `napi::Error` BEFORE the unsafe frame is ever reached. The cap exists solely
/// to bound OUR recursive walkers (and `Drop`); the FFI marshalling itself no
/// longer recurses (see below). Real-world wasm nesting is tiny (compilers
/// rarely exceed ~50 deep), so the ceiling is invisible in practice. All walks
/// share the SAME cap so build and read stay symmetric: a body you can
/// build+emit is a body you can read back, and vice-versa.
///
/// ## Where the guard fires
/// * build: the `crate::ir_marshal::InstrBody` ITERATIVE decode enforces the
///   cap WHILE decoding, so an over-deep JS body throws `nesting_too_deep()`
///   deterministically before anything past the cap is even materialized — on
///   every target and harness. `validate_body`/`emit_desc` re-check it as
///   defense in depth (they are `pub(crate)` and reachable without the decode).
/// * read: `read_instr_seq`'s guard is the primary for parsed-module reads
///   (walrus parses arbitrarily-deep modules iteratively; our read walk
///   refuses past the cap). The `crate::ir_marshal::InstrList` encode is
///   iterative and needs no guard — it only ever sees read-capped trees.
///
/// ## Historical note (why 250, was 256 through C6b)
/// Through C7b the nested `Vec<InstrDesc>` tree ALSO recursed in napi's DERIVED
/// Rust↔JS marshalling — a per-level call-stack recursion we did not own. Its
/// empirically-measured ceilings (native decode SIGSEGV ≈740, native encode V8
/// fatal ≈525, AVA-wasi harness RangeError just past ~255) are what forced the
/// 256 → 250 retune when C7a widened `InstrDesc`, and left the guard
/// unreachable from the over-cap side under the AVA-wasi harness (C7c measured
/// that ceiling at EXACTLY the at-cap canary's depth — zero margin). The CH
/// hardening replaced the derived marshalling with the iterative drivers in
/// `crate::ir_marshal`, which use O(1) call stack at ANY depth, so those
/// marshalling ceilings are GONE and no longer constrain this value. 250 is
/// kept: it is amply beyond real-world nesting, the in-module walks and `Drop`
/// still recurse (the cap is their bound), and the depth is pinned by the
/// at-cap round-trip canary in `__test__/ir.spec.ts`.
pub(crate) const MAX_NESTING_DEPTH: usize = 250;

/// The catchable error returned when a walk would descend past
/// [`MAX_NESTING_DEPTH`]. Shared with the iterative decode driver
/// (`crate::ir_marshal`), so the guard message is identical wherever it fires.
pub(crate) fn nesting_too_deep() -> Error {
  Error::from_reason(format!(
    "instruction nesting too deep (max {MAX_NESTING_DEPTH}); refusing to recurse to avoid a \
     stack overflow"
  ))
}

/// A constant value carried by a `*.const` instruction, mirroring
/// `walrus::ir::Value`.
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'I32', value: number } | { type: 'I64', value: bigint }`
/// `| { type: 'F32', value: number } | { type: 'F64', value: number }`
/// `| { type: 'V128', value: Uint8Array }`.
///
/// `I64` crosses the boundary as a JS `bigint` for exactness; an `f32` has no
/// dedicated JS type, so `F32` uses a `number` (`f64`) that is narrowed to
/// `f32` on emit. `V128` crosses as the raw 16 bytes of the vector register in
/// LITTLE-ENDIAN order (byte 0 is the least-significant, matching walrus'
/// `u128` decoding): emit requires EXACTLY 16 bytes (any other length is a
/// catchable representation error, NOT a wasm semantic check) and folds them via
/// `u128::from_le_bytes`; read produces them via `u128::to_le_bytes`.
///
/// Named `ConstValue`, not `Value`: napi-derive reserves the bare type name
/// `Value` and maps it to TS `any` (it assumes a `serde_json::Value`-style
/// dynamic value), which would erase the union from `InstrDesc.value`. The
/// rename keeps the field precisely typed.
#[napi]
pub enum ConstValue {
  /// A 32-bit integer constant.
  I32 {
    /// The constant value.
    value: i32,
  },
  /// A 64-bit integer constant (a JS `bigint`, for exactness).
  I64 {
    /// The constant value.
    value: BigInt,
  },
  /// A 32-bit float constant (a JS `number`, narrowed to `f32` on emit).
  F32 {
    /// The constant value.
    value: f64,
  },
  /// A 64-bit float constant.
  F64 {
    /// The constant value.
    value: f64,
  },
  /// A 128-bit vector constant: exactly 16 little-endian bytes.
  V128 {
    /// The raw 16 bytes of the vector, least-significant first.
    value: Uint8Array,
  },
}

/// The type of a control-flow block (`block`/`loop`/`if`), mirroring
/// `walrus::ir::InstrSeqType`.
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'Empty' } | { type: 'Value', value: ValType }`
/// `| { type: 'MultiValue', typeIndex: number }`.
///
/// `Empty` is a block with no parameters and no result; `Value` is a block that
/// leaves a single result and takes no parameters; `MultiValue` references a
/// function type (by its stable index) for arbitrary parameters/results.
#[napi]
pub enum BlockType {
  /// No parameters, no result (`InstrSeqType::Simple(None)`).
  Empty,
  /// A single result value, no parameters (`InstrSeqType::Simple(Some(_))`).
  Value {
    /// The single result value type.
    value: ValType,
  },
  /// Arbitrary parameters/results via a referenced function type
  /// (`InstrSeqType::MultiValue`).
  MultiValue {
    /// The stable index of the function type describing this block's signature.
    type_index: f64,
  },
}

/// The alignment and offset immediate of a `Load`/`Store`, mirroring
/// `walrus::ir::MemArg` (`ir/mod.rs:1655`).
///
/// Generated as a `#[napi(object)]`: `{ align: number, offset: bigint }`.
///
/// `align` is the RAW alignment in bytes (a power of two per wasm, e.g. `4` for
/// a natural `i32.load`), NOT the log2 exponent the binary format encodes —
/// walrus converts between the two on parse (`1 << exp`) and emit (counting the
/// trailing shift), so this stores the same power-of-two value both directions.
/// It crosses the boundary as a JS `number` and is validated LOSSLESSLY
/// (`checked_index`) before emit, so an out-of-domain value (NaN/Infinity/
/// negative/fraction/`> u32::MAX`) is a catchable error, NOT a silent ToUint32
/// alias into a different valid alignment.
/// `offset` is the constant byte offset from the dynamic address; it is a wasm
/// `u64` (memory64 uses the full range), so it crosses the boundary as a JS
/// `bigint` for exactness and is rejected on build if negative or not lossless.
///
/// MIRROR-WALRUS: neither field is validated for wasm well-formedness — `align`
/// is NOT checked to be a power of two and `offset` is NOT range-checked against
/// the memory; both are stored verbatim (only silent value corruption is
/// guarded, not wasm semantic validity).
#[napi(object)]
pub struct MemArg {
  /// The raw alignment in bytes (a power of two per wasm). Validated LOSSLESSLY
  /// (`checked_index`) on build — an out-of-domain JS number throws — but NOT
  /// checked to be a power of two (mirror walrus).
  pub align: f64,
  /// The constant byte offset of the access (a JS `bigint`, for exact `u64`
  /// range).
  pub offset: BigInt,
}

/// The sign/zero-extension behavior of a narrowing load, mirroring
/// `walrus::ir::ExtendedLoad` (`ir/mod.rs:1562`). Carried in the `kind` field of
/// the narrow [`LoadKind`] variants (`I32_8`/`I32_16`/`I64_8`/`I64_16`/`I64_32`).
///
/// Generated as a string enum: `'SignExtend' | 'ZeroExtend' | 'ZeroExtendAtomic'`.
///
/// `SignExtend`/`ZeroExtend` are the ordinary narrow loads (`i32.load8_s` /
/// `i32.load8_u`); `ZeroExtendAtomic` is the atomic narrow load
/// (`i32.atomic.load8_u`, which is always zero-extending).
#[napi(string_enum)]
pub enum ExtendedLoad {
  /// Sign-extend the narrow value to the full result width.
  SignExtend,
  /// Zero-extend the narrow value to the full result width.
  ZeroExtend,
  /// Zero-extend an atomic narrow load to the full result width.
  ZeroExtendAtomic,
}

/// The kind of a `Load` instruction, mirroring `walrus::ir::LoadKind`
/// (`ir/mod.rs:1514`).
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'I32', atomic: boolean } | { type: 'I64', atomic: boolean }`
/// `| { type: 'F32' } | { type: 'F64' } | { type: 'V128' }`
/// `| { type: 'I32_8', kind: ExtendedLoad } | { type: 'I32_16', kind: ExtendedLoad }`
/// `| { type: 'I64_8', kind: ExtendedLoad } | { type: 'I64_16', kind: ExtendedLoad }`
/// `| { type: 'I64_32', kind: ExtendedLoad }`.
///
/// The full-width `I32`/`I64` carry an `atomic` flag (`i32.atomic.load` is a
/// plain `Load` with `atomic: true`); the narrow variants carry an
/// [`ExtendedLoad`] describing sign/zero/atomic extension (this is the ASYMMETRY
/// with [`StoreKind`], whose narrow variants carry `atomic: bool` instead).
/// `V128` is the plain `v128.load` (the lane/splat SIMD loads are the separate,
/// deferred `LoadSimd` instruction).
#[napi]
pub enum LoadKind {
  /// A full-width `i32` load (`atomic` => `i32.atomic.load`).
  I32 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// A full-width `i64` load (`atomic` => `i64.atomic.load`).
  I64 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// An `f32` load.
  F32,
  /// An `f64` load.
  F64,
  /// A `v128` load (plain `v128.load`).
  V128,
  /// An 8-bit load extended to `i32`.
  I32_8 {
    /// The extension behavior.
    kind: ExtendedLoad,
  },
  /// A 16-bit load extended to `i32`.
  I32_16 {
    /// The extension behavior.
    kind: ExtendedLoad,
  },
  /// An 8-bit load extended to `i64`.
  I64_8 {
    /// The extension behavior.
    kind: ExtendedLoad,
  },
  /// A 16-bit load extended to `i64`.
  I64_16 {
    /// The extension behavior.
    kind: ExtendedLoad,
  },
  /// A 32-bit load extended to `i64`.
  I64_32 {
    /// The extension behavior.
    kind: ExtendedLoad,
  },
}

/// The kind of a `Store` instruction, mirroring `walrus::ir::StoreKind`
/// (`ir/mod.rs:1609`).
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'I32', atomic: boolean } | { type: 'I64', atomic: boolean }`
/// `| { type: 'F32' } | { type: 'F64' } | { type: 'V128' }`
/// `| { type: 'I32_8', atomic: boolean } | { type: 'I32_16', atomic: boolean }`
/// `| { type: 'I64_8', atomic: boolean } | { type: 'I64_16', atomic: boolean }`
/// `| { type: 'I64_32', atomic: boolean }`.
///
/// Note the ASYMMETRY with [`LoadKind`]: EVERY integer store variant (full-width
/// AND narrow) carries an `atomic: bool`, because a store has no sign/zero
/// extension to describe — so there is no `ExtendedLoad` here. `V128` is the
/// plain `v128.store` (the lane SIMD stores are the separate, deferred
/// `LoadSimd` instruction).
#[napi]
pub enum StoreKind {
  /// A full-width `i32` store (`atomic` => `i32.atomic.store`).
  I32 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// A full-width `i64` store (`atomic` => `i64.atomic.store`).
  I64 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// An `f32` store.
  F32,
  /// An `f64` store.
  F64,
  /// A `v128` store (plain `v128.store`).
  V128,
  /// An 8-bit store of an `i32` (`atomic` => `i32.atomic.store8`).
  I32_8 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// A 16-bit store of an `i32` (`atomic` => `i32.atomic.store16`).
  I32_16 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// An 8-bit store of an `i64` (`atomic` => `i64.atomic.store8`).
  I64_8 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// A 16-bit store of an `i64` (`atomic` => `i64.atomic.store16`).
  I64_16 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
  /// A 32-bit store of an `i64` (`atomic` => `i64.atomic.store32`).
  I64_32 {
    /// Whether this is the atomic form.
    atomic: bool,
  },
}

/// The kind of a `LoadSimd` instruction, mirroring `walrus::ir::LoadSimdKind`
/// (`ir/mod.rs:1534`). This is the SIMD vector load/store-lane family — the
/// counterpart of [`LoadKind`]/[`StoreKind`] for the `v128.load*`/`v128.store*`
/// memory ops that a plain `Load`/`Store`'s `V128` does NOT cover.
///
/// Generated as a TypeScript discriminated union keyed on `type`: 12 FIELDLESS
/// variants (`{ type: 'Splat8' }` … `{ type: 'V128Load64Zero' }`) and 8
/// LANE-CARRYING variants each with a `lane: number`
/// (`{ type: 'V128Load8Lane', lane }` … `{ type: 'V128Store64Lane', lane }`).
///
/// The fieldless variants are the whole-vector loads: the four `*.splat`
/// broadcasts (`Splat8`/`Splat16`/`Splat32`/`Splat64` = `v128.load8_splat` …
/// `v128.load64_splat`), the six sign/zero-extending widening loads
/// (`V128Load8x8S`/`U`, `V128Load16x4S`/`U`, `V128Load32x2S`/`U`), and the two
/// zero-filling loads (`V128Load32Zero`/`V128Load64Zero`). The lane-carrying
/// variants are the eight load-lane / store-lane ops (`v128.load8_lane` …
/// `v128.store64_lane`), each carrying the vector `lane` it reads into / writes
/// from — walrus stores this immediate as a tuple `(u8)`; here it is surfaced as
/// a named `lane` field.
///
/// MIRROR-WALRUS: the `lane` index is stored verbatim — it is NOT range-checked
/// against the vector's lane count (that is a wasm semantic check, not a
/// representation constraint).
#[napi]
pub enum LoadSimdKind {
  /// Broadcast one 8-bit element to all 16 lanes (`v128.load8_splat`).
  Splat8,
  /// Broadcast one 16-bit element to all 8 lanes (`v128.load16_splat`).
  Splat16,
  /// Broadcast one 32-bit element to all 4 lanes (`v128.load32_splat`).
  Splat32,
  /// Broadcast one 64-bit element to both lanes (`v128.load64_splat`).
  Splat64,
  /// Load eight 8-bit ints, sign-extending each to 16 bits (`v128.load8x8_s`).
  V128Load8x8S,
  /// Load eight 8-bit ints, zero-extending each to 16 bits (`v128.load8x8_u`).
  V128Load8x8U,
  /// Load four 16-bit ints, sign-extending each to 32 bits (`v128.load16x4_s`).
  V128Load16x4S,
  /// Load four 16-bit ints, zero-extending each to 32 bits (`v128.load16x4_u`).
  V128Load16x4U,
  /// Load two 32-bit ints, sign-extending each to 64 bits (`v128.load32x2_s`).
  V128Load32x2S,
  /// Load two 32-bit ints, zero-extending each to 64 bits (`v128.load32x2_u`).
  V128Load32x2U,
  /// Load a 32-bit value into the low lane, zeroing the rest (`v128.load32_zero`).
  V128Load32Zero,
  /// Load a 64-bit value into the low lane, zeroing the rest (`v128.load64_zero`).
  V128Load64Zero,
  /// Load an 8-bit value into a single lane (`v128.load8_lane`).
  V128Load8Lane {
    /// The lane index the value is loaded into.
    lane: f64,
  },
  /// Load a 16-bit value into a single lane (`v128.load16_lane`).
  V128Load16Lane {
    /// The lane index the value is loaded into.
    lane: f64,
  },
  /// Load a 32-bit value into a single lane (`v128.load32_lane`).
  V128Load32Lane {
    /// The lane index the value is loaded into.
    lane: f64,
  },
  /// Load a 64-bit value into a single lane (`v128.load64_lane`).
  V128Load64Lane {
    /// The lane index the value is loaded into.
    lane: f64,
  },
  /// Store the 8-bit value of a single lane (`v128.store8_lane`).
  V128Store8Lane {
    /// The lane index whose value is stored.
    lane: f64,
  },
  /// Store the 16-bit value of a single lane (`v128.store16_lane`).
  V128Store16Lane {
    /// The lane index whose value is stored.
    lane: f64,
  },
  /// Store the 32-bit value of a single lane (`v128.store32_lane`).
  V128Store32Lane {
    /// The lane index whose value is stored.
    lane: f64,
  },
  /// Store the 64-bit value of a single lane (`v128.store64_lane`).
  V128Store64Lane {
    /// The lane index whose value is stored.
    lane: f64,
  },
}

/// The read/modify/write operation of an `AtomicRmw`, mirroring
/// `walrus::ir::AtomicOp` (`ir/mod.rs:1665`, fieldless).
///
/// Generated as a string enum: `'Add' | 'Sub' | 'And' | 'Or' | 'Xor' | 'Xchg'`.
///
/// These are the six atomic rmw ops (`i32.atomic.rmw.add`, `…​.sub`, `…​.and`,
/// `…​.or`, `…​.xor`, `…​.xchg`); the compare-exchange rmw is the separate
/// [`InstrDesc`] `Cmpxchg` instruction, not an `AtomicOp`.
#[napi(string_enum)]
pub enum AtomicOp {
  /// Atomic add (`*.atomic.rmw.add`).
  Add,
  /// Atomic subtract (`*.atomic.rmw.sub`).
  Sub,
  /// Atomic bitwise and (`*.atomic.rmw.and`).
  And,
  /// Atomic bitwise or (`*.atomic.rmw.or`).
  Or,
  /// Atomic bitwise xor (`*.atomic.rmw.xor`).
  Xor,
  /// Atomic exchange (`*.atomic.rmw.xchg`).
  Xchg,
}

/// The access width of an atomic memory operation, mirroring
/// `walrus::ir::AtomicWidth` (`ir/mod.rs:1677`, fieldless). Carried by
/// `AtomicRmw` (with an [`AtomicOp`]) and `Cmpxchg`.
///
/// Generated as a string enum:
/// `'I32' | 'I32_8' | 'I32_16' | 'I64' | 'I64_8' | 'I64_16' | 'I64_32'`.
///
/// The bare `I32`/`I64` are the full-width ops; the `_8`/`_16`/`_32` suffixes are
/// the sub-word ops that operate on a narrow slice of the value (`I32_8` =
/// `i32.atomic.rmw8`, `I64_32` = `i64.atomic.rmw32`, …). MIRROR-WALRUS: whether
/// the width is legal for a given op is NOT checked here.
#[napi(string_enum)]
pub enum AtomicWidth {
  /// A full-width 32-bit atomic op.
  I32,
  /// An 8-bit-wide atomic op on an `i32` value.
  I32_8,
  /// A 16-bit-wide atomic op on an `i32` value.
  I32_16,
  /// A full-width 64-bit atomic op.
  I64,
  /// An 8-bit-wide atomic op on an `i64` value.
  I64_8,
  /// A 16-bit-wide atomic op on an `i64` value.
  I64_16,
  /// A 32-bit-wide atomic op on an `i64` value.
  I64_32,
}

/// The reference type carried by a `RefNull` instruction, mirroring
/// `walrus::RefType` (`ty.rs:874`): a `nullable` flag plus the [`HeapType`] the
/// null belongs to (`(ref null $t)`).
///
/// Generated as a `#[napi(object)]`: `{ nullable: boolean, heap: HeapType }`.
///
/// This exists because `RefNull` is the one instruction whose payload is a whole
/// `RefType` rather than a bare id. It REUSES the existing [`HeapType`] napi enum
/// (no separate abstract/concrete plumbing): a concrete/exact `heap` carries a
/// `type_index` that emit resolves against the live arena via the module-aware
/// [`crate::convert::heap_type_to_walrus_in`] — a foreign/deleted/entry index is
/// rejected catchably there rather than aborting the process at emit. `walrus`
/// inlines `RefType` into `ValType::Ref` (see [`ValType`]); this is the same two
/// fields, surfaced as a named object only for `InstrDesc.refType`.
#[napi(object)]
pub struct RefType {
  /// Whether the reference is nullable (mirrors `RefType::nullable`).
  pub nullable: bool,
  /// The heap type the reference points to (mirrors `RefType::heap_type`).
  pub heap: HeapType,
}

/// A single catch clause of a `TryTable` (modern, phase-4) OR a legacy `Try`
/// (phase-1) instruction, as a wide tagged record shared by both the read and
/// build directions — the exception-handling analogue of [`InstrDesc`].
///
/// `kind` is the discriminant. For a MODERN `TryTable` it is the walrus
/// `TryTableCatch` variant name verbatim: `"Catch"` (tag + label), `"CatchRef"`
/// (tag + label), `"CatchAll"` (label only), `"CatchAllRef"` (label only). For a
/// LEGACY `Try` it is a distinct, collision-free name (walrus' legacy variant
/// names `Catch`/`CatchAll` overlap with the modern ones): `"LegacyCatch"` (tag +
/// `seq` handler body + `blockType`), `"LegacyCatchAll"` (`seq` handler body +
/// `blockType`), `"LegacyDelegate"` (`relativeDepth`). An unknown `kind` or a
/// missing required field is a catchable error in BOTH build and preflight.
/// Which kinds are legal is decided by the PARENT instruction `type` (`"TryTable"`
/// routes clauses through `try_table_catches_to_walrus`; `"Try"` through
/// `legacy_catches_to_walrus`).
///
/// LOAD-BEARING SCOPING (modern only): a modern clause `label` is a relative
/// branch depth resolved against the label stack WITHOUT the `try_table`'s own
/// body sequence — depth `0` names the innermost block ENCLOSING the `try_table`
/// instruction, NOT the try body. walrus resolves catch labels before pushing the
/// try_table control frame (parse) and computes their branch targets before
/// pushing the try_table block (emit), so our emit/validate/read convert/validate/
/// invert the clauses against the OUTER stack, then descend into the body (which
/// walks one frame deeper). Legacy handlers carry NO clause label: each is its own
/// child `InstrSeq` (`seq`), a SIBLING of the try body at the SAME label depth
/// (like an `IfElse` arm); `relativeDepth` (`LegacyDelegate`) is a RAW pass-through
/// `u32` walrus never resolves. See the `TryTable`/`Try` arms.
//
// MAINTENANCE (plain comment so the generated .d.ts is unchanged): `catches`
// crosses the FFI through the ITERATIVE marshalling driver in
// `src/ir_marshal.rs` (a LEAF `Vec`, decoded by a non-preallocating loop), NOT
// napi's derived `Vec` decode. The LEGACY handler body `seq` is the one
// self-referential field on a clause (it can hold `InstrDesc`s): it is a NESTED
// edge two levels down (`InstrDesc.catches[i].seq`), so it is driven by the
// `ParentSlot::CatchSeq` frame bookkeeping + `decode_catch_clause_shallow` there
// (it is NOT in `EDGE_NAMES`/`EDGE_CSTRS`, which are direct `InstrDesc` edges).
#[napi(object)]
pub struct CatchClause {
  /// The clause variant. Modern (`TryTable`): the walrus `TryTableCatch` name —
  /// `"Catch"`, `"CatchRef"`, `"CatchAll"`, `"CatchAllRef"`. Legacy (`Try`):
  /// `"LegacyCatch"`, `"LegacyCatchAll"`, `"LegacyDelegate"`.
  pub kind: String,
  /// The caught exception tag's stable index. Required for the modern
  /// `"Catch"`/`"CatchRef"` and the legacy `"LegacyCatch"`; absent otherwise.
  pub tag: Option<f64>,
  /// The relative label depth of the block a MODERN clause branches to on a catch
  /// (`0` = the innermost block ENCLOSING the `try_table` — clause labels resolve
  /// against the OUTER scope, NOT the try body). Required for every modern kind;
  /// absent for every legacy kind (legacy handlers are child sequences, not
  /// branch targets).
  pub label: Option<f64>,
  /// LEGACY `"LegacyCatch"`/`"LegacyCatchAll"`: the handler body instructions (a
  /// child `InstrSeq`, a SIBLING of the try body). Required for those two kinds,
  /// absent for every other kind. This is the nested edge driven by the iterative
  /// marshalling in `src/ir_marshal.rs`.
  pub seq: Option<Vec<InstrDesc>>,
  /// LEGACY `"LegacyDelegate"`: the relative depth this clause delegates to — a
  /// RAW pass-through `u32` walrus never resolves (NOT a resolved `label`).
  /// Required for `"LegacyDelegate"`, absent otherwise.
  pub relative_depth: Option<f64>,
  /// LEGACY `"LegacyCatch"`/`"LegacyCatchAll"`: the handler `InstrSeq`'s own type
  /// signature (each legacy handler carries its OWN `InstrSeqType`, distinct from
  /// the try's). Captured for a faithful in-memory round-trip; absent for every
  /// other kind. Defaults to empty when omitted on build.
  pub block_type: Option<BlockType>,
}

/// A single wasm instruction, as a wide tagged record shared by both the read
/// (`WasmFunction::instructions`) and build (`WasmModule::buildFunction`)
/// directions.
///
/// `type` is the discriminant (the walrus variant name, e.g. `"Const"`,
/// `"LocalGet"`, `"Block"`, `"Br"`); every other field is optional and only the
/// ones relevant to that `type` are set. Control-flow bodies nest as
/// `Array<InstrDesc>` (`seq` for `block`/`loop`, `consequent`/`alternative` for
/// `if`/`else`), making the interface self-referential.
///
/// This is the C1a/C1b/C2/C3/C4/C5 subset: leaf ops (`Unreachable`/`Return`/
/// `Drop`), `Const`, local/global get/set/tee, `Call`, `Select`, the control
/// constructs (`Block`/`Loop`/`IfElse`), the branches (`Br`/`BrIf`/`BrTable`),
/// the numeric/comparison/conversion operators (`Binop`/`Unop`/`TernOp`, keyed by
/// `op`), the memory + general load/store instructions (`MemorySize`/
/// `MemoryGrow`/`MemoryInit`/`DataDrop`/`MemoryCopy`/`MemoryFill`/`Load`/`Store`),
/// the atomic (threads) instructions (`AtomicRmw`/`Cmpxchg`/`AtomicNotify`/
/// `AtomicWait`/`AtomicFence`), the table instructions + `call_indirect`
/// (`TableGet`/`TableSet`/`TableGrow`/`TableSize`/`TableFill`/`TableInit`/
/// `TableCopy`/`ElemDrop`/`CallIndirect`), the core reference + tail-call
/// instructions (`RefNull`/`RefIsNull`/`RefFunc`/`ReturnCall`/
/// `ReturnCallIndirect`), and the C6a SIMD subset: the lane-carrying
/// `Binop`/`Unop` ops (`op` + `lane`), the v128 `Const`, and the fixed-shape
/// `V128Bitselect`/`I8x16Swizzle`/`I8x16Shuffle` instructions, and the C6b SIMD
/// memory op (`LoadSimd`, the vector load / load-lane / store-lane family), and
/// the C7a wasm-GC struct/array subset (`StructNew`/`StructNewDefault`/
/// `StructGet`/`StructGetS`/`StructGetU`/`StructSet` and `ArrayNew`/
/// `ArrayNewDefault`/`ArrayNewFixed`/`ArrayNewData`/`ArrayNewElem`/`ArrayGet`/
/// `ArrayGetS`/`ArrayGetU`/`ArraySet`/`ArrayLen`/`ArrayFill`/`ArrayCopy`/
/// `ArrayInitData`/`ArrayInitElem`), and the C7b GC reference subset — the
/// label-free ops (`RefAsNonNull`/`CallRef`/`ReturnCallRef`/`RefI31`/`I31GetS`/
/// `I31GetU`/`RefTest`/`RefCast`/`AnyConvertExtern`/`ExternConvertAny`/`RefEq`),
/// and the C7c GC branch subset — the label-carrying ops (`BrOnNull`/
/// `BrOnNonNull`/`BrOnCast`/`BrOnCastFail`), and the C8a modern
/// exception-handling subset — the `TryTable` control construct (a `Block` twin
/// carrying a `catches` clause list, [`CatchClause`]), `Throw` (`tag`), and
/// `ThrowRef`, and the C8b LEGACY exception-handling subset — the `Try` control
/// construct (a `Block` twin whose `catches` clauses carry full child handler
/// bodies in `CatchClause.seq`) and `Rethrow` (`relativeDepth`), and the C9
/// wide-arithmetic subset — the four fieldless leaves (`I64Add128`/`I64Sub128`/
/// `I64MulWideS`/`I64MulWideU`). With C9 every walrus `Instr` variant is covered;
/// any unknown discriminant is still rejected catchably by both directions.
//
// MAINTENANCE (plain comment so the generated .d.ts is unchanged): the three
// self-referential edge fields (`seq`/`consequent`/`alternative`) do NOT cross
// the FFI through this struct's DERIVED marshalling — `src/ir_marshal.rs`
// drives them iteratively (decode: the copy-except-edges skip list + edge walk
// in `InstrBody::from_napi_value`; encode: the take/attach list in
// `InstrList::to_napi_value`). The C8b legacy handler bodies (`catches[].seq`, a
// nested edge two levels down) are driven by the SEPARATE `ParentSlot::CatchSeq`
// frame bookkeeping there. ANY future field that can contain `InstrDesc`s MUST be
// routed through those drivers, or the derived per-element call will recurse on
// the call stack again and reopen the uncatchable stack-overflow abort this
// layering removed.
#[napi(object)]
pub struct InstrDesc {
  /// The instruction discriminant — the walrus variant name.
  pub r#type: String,
  /// `Const`: the constant value.
  pub value: Option<ConstValue>,
  /// `LocalGet`/`LocalSet`/`LocalTee`: the referenced local's stable index.
  pub local: Option<f64>,
  /// `GlobalGet`/`GlobalSet`: the referenced global's stable index.
  pub global: Option<f64>,
  /// The referenced function's stable index, for `Call`, `RefFunc` (`ref.func`),
  /// and `ReturnCall` (`return_call`).
  pub func: Option<f64>,
  /// `Select`: the optional result type of a typed `select` (absent => a plain,
  /// untyped `select`).
  pub select_type: Option<ValType>,
  /// `Block`/`Loop`/`IfElse`: the block's type signature.
  pub block_type: Option<BlockType>,
  /// `Block`/`Loop`: the body instructions.
  pub seq: Option<Vec<InstrDesc>>,
  /// `IfElse`: the `then`-arm instructions.
  pub consequent: Option<Vec<InstrDesc>>,
  /// `IfElse`: the `else`-arm instructions.
  pub alternative: Option<Vec<InstrDesc>>,
  /// `Br`/`BrIf`/`BrOnNull`/`BrOnNonNull`/`BrOnCast`/`BrOnCastFail`: the
  /// relative label depth of the branch target
  /// (`0` = the innermost enclosing block/loop/if).
  pub label: Option<f64>,
  /// `BrTable`: the relative label depths of the table's targets, in order.
  pub labels: Option<Vec<f64>>,
  /// `BrTable`: the relative label depth of the default (fallthrough) target.
  pub default_label: Option<f64>,
  /// `Binop`/`Unop`/`TernOp`: the operator name (the walrus
  /// `BinaryOp`/`UnaryOp`/`TernaryOp` variant name, e.g. `"I32Add"`, `"I32Eqz"`,
  /// `"F32x4RelaxedMadd"`). The `type` discriminant selects which of the three
  /// operator enums decodes it, so one shared field is unambiguous.
  pub op: Option<String>,
  /// `Binop`/`Unop`: the lane index of a lane-carrying SIMD operator (the walrus
  /// `idx: u8` immediate of a `*ReplaceLane` / `*ExtractLane*` variant). Present
  /// exactly for the 14 lane ops, paired with `op`; absent for every fieldless
  /// operator. A lane op missing this field is rejected catchably (a lane index
  /// is part of its representation, not a wasm semantic check).
  pub lane: Option<f64>,
  /// The referenced memory's stable index, for
  /// `MemorySize`/`MemoryGrow`/`MemoryInit`/`MemoryFill`/`Load`/`Store`/
  /// `LoadSimd`, and the DESTINATION memory of `MemoryCopy`.
  pub memory: Option<f64>,
  /// `MemoryCopy`: the SOURCE memory's stable index (the destination uses
  /// `memory`).
  pub src_memory: Option<f64>,
  /// `MemoryInit`/`DataDrop`: the referenced data segment's stable index.
  pub data: Option<f64>,
  /// `Load`/`Store`/`LoadSimd`: the alignment and offset immediate.
  pub mem_arg: Option<MemArg>,
  /// `Load`: the kind of load (width, atomicity, extension).
  pub load_kind: Option<LoadKind>,
  /// `Store`: the kind of store (width, atomicity).
  pub store_kind: Option<StoreKind>,
  /// `LoadSimd`: the kind of SIMD memory op (vector load / load-lane /
  /// store-lane; the lane variants carry a `lane` index).
  pub load_simd_kind: Option<LoadSimdKind>,
  /// The referenced table's stable index, for
  /// `TableGet`/`TableSet`/`TableGrow`/`TableSize`/`TableFill`/`TableInit`, the
  /// DESTINATION table of `TableCopy`, and the table of `CallIndirect` /
  /// `ReturnCallIndirect`.
  pub table: Option<f64>,
  /// `TableCopy`: the SOURCE table's stable index (the destination uses
  /// `table`).
  pub src_table: Option<f64>,
  /// `TableInit`/`ElemDrop`: the referenced element segment's stable index.
  pub elem: Option<f64>,
  /// The stable index of the function type being called through the table, for
  /// `CallIndirect` and `ReturnCallIndirect` (named `typeIndex`, matching
  /// `BlockType::MultiValue`).
  pub type_index: Option<f64>,
  /// The reference-type payload: for `RefNull` the type of the null being
  /// produced (`(ref null $t)`); for `RefTest`/`RefCast` the tested/cast-to
  /// type; for `BrOnCast`/`BrOnCastFail` the SOURCE/input pair of the cast
  /// (walrus' `from_nullable` + `from_heap_type` — the target pair is
  /// `toRefType`).
  pub ref_type: Option<RefType>,
  /// `AtomicRmw`: the read/modify/write operation.
  pub atomic_op: Option<AtomicOp>,
  /// `AtomicRmw`/`Cmpxchg`: the access width of the atomic operation.
  pub atomic_width: Option<AtomicWidth>,
  /// `AtomicWait`: whether this is a 64-bit (`memory.atomic.wait64`) wait; `false`
  /// is the 32-bit (`memory.atomic.wait32`) form.
  pub sixty_four: Option<bool>,
  /// `I8x16Shuffle`: the 16 byte lane indices selecting the result vector (the
  /// walrus `[u8; 16]` immediate) as a `Uint8Array`. Emit requires EXACTLY 16
  /// bytes (any other length is a catchable representation error, NOT a wasm
  /// semantic check — a lane index >= 32 is emitted verbatim); read produces the
  /// 16 bytes as-is.
  pub shuffle_indices: Option<Uint8Array>,
  /// `StructGet`/`StructGetS`/`StructGetU`/`StructSet`: the field index within
  /// the GC struct. MIRROR-WALRUS: a plain immediate, stored verbatim and NOT
  /// range-checked against the struct's field count.
  pub field: Option<f64>,
  /// `ArrayNewFixed`: the element count for `array.new_fixed` (a statically known
  /// immediate). Validated LOSSLESSLY (`checked_index`) in BOTH the preflight and
  /// emit, so an out-of-domain JS number is a catchable error, NOT a silent u32
  /// alias — matching every other numeric immediate. (Guard-silent-corruption,
  /// still MIRROR-WALRUS: walrus takes a `u32`; we only reject numbers that don't
  /// LOSSLESSLY fit `u32`, we do NOT range-check against any wasm semantic bound.)
  pub len: Option<f64>,
  /// `ArrayCopy`: the SOURCE array type's stable index (the DESTINATION array
  /// type uses `type_index`), mirroring walrus' `ArrayCopy { dst_ty, src_ty }`.
  pub src_type_index: Option<f64>,
  /// `BrOnCast`/`BrOnCastFail`: the TARGET pair of the cast (walrus'
  /// `to_nullable` + `to_heap_type`); the source/input pair uses `refType`.
  pub to_ref_type: Option<RefType>,
  /// `Throw`: the thrown exception tag's stable index.
  pub tag: Option<f64>,
  /// `TryTable` (modern) / `Try` (legacy): the catch clauses of the try block, in
  /// order. Absent (or empty) is a legal catch-less try. Both reuse `block_type` +
  /// `seq` (the try body) — each is a `Block` twin — so those are NOT re-declared
  /// here. Legacy `Try` clauses carry their handler bodies in `CatchClause.seq`.
  pub catches: Option<Vec<CatchClause>>,
  /// `Rethrow` (legacy): the relative depth of the caught exception to rethrow — a
  /// RAW pass-through `u32` walrus never resolves (NOT a resolved branch `label`).
  pub relative_depth: Option<f64>,
}

impl InstrDesc {
  /// A descriptor with the given discriminant and every payload field empty.
  fn new(ty: &str) -> Self {
    InstrDesc {
      r#type: ty.to_string(),
      value: None,
      local: None,
      global: None,
      func: None,
      select_type: None,
      block_type: None,
      seq: None,
      consequent: None,
      alternative: None,
      label: None,
      labels: None,
      default_label: None,
      op: None,
      lane: None,
      memory: None,
      src_memory: None,
      data: None,
      mem_arg: None,
      load_kind: None,
      store_kind: None,
      load_simd_kind: None,
      table: None,
      src_table: None,
      elem: None,
      type_index: None,
      ref_type: None,
      atomic_op: None,
      atomic_width: None,
      sixty_four: None,
      shuffle_indices: None,
      field: None,
      len: None,
      src_type_index: None,
      to_ref_type: None,
      tag: None,
      catches: None,
      relative_depth: None,
    }
  }
}

// ---------------------------------------------------------------------------
// Index -> Id resolution (guards against a panicking walrus lookup on a bad
// index; MIRROR-WALRUS: these are the only checks — no wasm-validity checks).
// ---------------------------------------------------------------------------

/// Resolve a local's stable index to its live `LocalId`, or a catchable error.
pub(crate) fn local_id_at(module: &Module, index: u32) -> Result<LocalId> {
  module
    .locals
    .iter()
    .find(|l| l.id().index() as u32 == index)
    .map(|l| l.id())
    .ok_or_else(|| Error::from_reason(format!("no local at index {index} in this module")))
}

/// Resolve a global's stable index to its live `GlobalId`, or a catchable error.
pub(crate) fn global_id_at(module: &Module, index: u32) -> Result<GlobalId> {
  module
    .globals
    .iter()
    .find(|g| g.id().index() as u32 == index)
    .map(|g| g.id())
    .ok_or_else(|| Error::from_reason(format!("no global at index {index} in this module")))
}

/// Resolve a function's stable index to its live `FunctionId`, or a catchable
/// error.
///
/// Note the self-reference limitation: a function's own id does not exist until
/// its builder is finished, so a body cannot `Call` itself through
/// `buildFunction` (the index simply names no live function yet, and this errs).
pub(crate) fn function_id_at(module: &Module, index: u32) -> Result<FunctionId> {
  module
    .funcs
    .iter()
    .find(|f| f.id().index() as u32 == index)
    .map(|f| f.id())
    .ok_or_else(|| Error::from_reason(format!("no function at index {index} in this module")))
}

/// Resolve a memory's stable index to its live `MemoryId`, or a catchable error.
///
/// This is an ABORT GUARD: a foreign/deleted memory index reaching emit panics
/// walrus (`IdsToIndices::get_memory_index`), and a panic across the FFI boundary
/// is uncatchable under `panic = abort`. Rejecting the bad index here (and in the
/// preflight) turns that abort into an ordinary JS exception.
pub(crate) fn memory_id_at(module: &Module, index: u32) -> Result<MemoryId> {
  module
    .memories
    .iter()
    .find(|m| m.id().index() as u32 == index)
    .map(|m| m.id())
    .ok_or_else(|| Error::from_reason(format!("no memory at index {index} in this module")))
}

/// Resolve a data segment's stable index to its live `DataId`, or a catchable
/// error.
///
/// This is an ABORT GUARD, exactly like [`memory_id_at`]: a foreign/deleted data
/// index reaching emit panics walrus (`IdsToIndices::get_data_index`).
pub(crate) fn data_id_at(module: &Module, index: u32) -> Result<DataId> {
  module
    .data
    .iter()
    .find(|d| d.id().index() as u32 == index)
    .map(|d| d.id())
    .ok_or_else(|| Error::from_reason(format!("no data segment at index {index} in this module")))
}

/// Resolve a table's stable index to its live `TableId`, or a catchable error.
///
/// This is an ABORT GUARD, exactly like [`memory_id_at`]: a foreign/deleted table
/// index reaching emit panics walrus (`IdsToIndices::get_table_index`), and a
/// panic across the FFI boundary is uncatchable under `panic = abort`. Rejecting
/// the bad index here (and in the preflight) turns that abort into an ordinary JS
/// exception.
pub(crate) fn table_id_at(module: &Module, index: u32) -> Result<TableId> {
  module
    .tables
    .iter()
    .find(|t| t.id().index() as u32 == index)
    .map(|t| t.id())
    .ok_or_else(|| Error::from_reason(format!("no table at index {index} in this module")))
}

/// Resolve an element segment's stable index to its live `ElementId`, or a
/// catchable error.
///
/// This is an ABORT GUARD, exactly like [`memory_id_at`]: a foreign/deleted
/// element index reaching emit panics walrus (`IdsToIndices::get_element_index`).
pub(crate) fn element_id_at(module: &Module, index: u32) -> Result<ElementId> {
  module
    .elements
    .iter()
    .find(|e| e.id().index() as u32 == index)
    .map(|e| e.id())
    .ok_or_else(|| {
      Error::from_reason(format!(
        "no element segment at index {index} in this module"
      ))
    })
}

/// Resolve a tag's stable index to its live `TagId`, or a catchable error.
///
/// This is an ABORT GUARD, exactly like [`memory_id_at`]: a foreign/deleted tag
/// index reaching emit panics walrus (`IdsToIndices::get_tag_index`), and a panic
/// across the FFI boundary is uncatchable under `panic = abort`. `Throw` and the
/// tag-carrying catch clauses (`Catch`/`CatchRef`) resolve through here, turning
/// a bad index into an ordinary JS exception before emit (and in the preflight).
pub(crate) fn tag_id_at(module: &Module, index: u32) -> Result<TagId> {
  module
    .tags
    .iter()
    .find(|t| t.id().index() as u32 == index)
    .map(|t| t.id())
    .ok_or_else(|| Error::from_reason(format!("no tag at index {index} in this module")))
}

// ---------------------------------------------------------------------------
// Block-type <-> InstrSeqType.
// ---------------------------------------------------------------------------

/// Build a walrus `InstrSeqType` from a JS block type, resolving a multi-value
/// type index against the live arena (a bad/entry index => catchable error). An
/// absent block type defaults to empty.
fn to_instr_seq_type(module: &Module, bt: Option<BlockType>) -> Result<wir::InstrSeqType> {
  Ok(match bt {
    None | Some(BlockType::Empty) => wir::InstrSeqType::Simple(None),
    Some(BlockType::Value { value }) => {
      wir::InstrSeqType::Simple(Some(val_type_to_walrus_in(module, value)?))
    }
    Some(BlockType::MultiValue { type_index }) => wir::InstrSeqType::MultiValue(resolve_type_id(
      module,
      checked_index(type_index, "typeIndex")?,
    )?),
  })
}

/// Read a walrus `InstrSeqType` back into a JS block type. Fallible only because
/// a `Simple(Some(vt))` result value type may embed a `#[non_exhaustive]` walrus
/// heap type (see [`crate::convert`]).
fn from_instr_seq_type(ty: wir::InstrSeqType) -> Result<BlockType> {
  Ok(match ty {
    wir::InstrSeqType::Simple(None) => BlockType::Empty,
    wir::InstrSeqType::Simple(Some(vt)) => BlockType::Value {
      value: vt.try_into()?,
    },
    wir::InstrSeqType::MultiValue(id) => BlockType::MultiValue {
      type_index: id.index() as f64,
    },
  })
}

// ---------------------------------------------------------------------------
// MemArg / LoadKind / StoreKind / ExtendedLoad <-> walrus. All pure value
// conversions: the kind enums carry no arena ids (so no resolution), and the
// only fallible step is MemArg's `u64` offset losslessness on the write side.
// The kind matches are EXHAUSTIVE (no `_`) so a future walrus variant is a
// COMPILE error rather than a silent mismap.
// ---------------------------------------------------------------------------

/// Build a walrus `MemArg` from a JS `MemArg`, validating `align` LOSSLESSLY
/// (`checked_index`, so an out-of-domain JS number is a catchable error, not a
/// silent u32 alias) and rejecting an `offset` that is negative or does not fit
/// losslessly in a `u64` (mirror of the size-write path). MIRROR-WALRUS still
/// does NOT check `align` is a power of two. Shared by emit and its preflight so
/// the two never disagree.
fn mem_arg_to_walrus(arg: &MemArg) -> Result<wir::MemArg> {
  Ok(wir::MemArg {
    align: checked_index(arg.align, "MemArg align")?,
    offset: bigint_to_u64(arg.offset.clone(), "MemArg offset")?,
  })
}

/// Read a walrus `MemArg` back into a JS `MemArg` (`align` widens `u32` -> `f64`
/// losslessly; `offset` as an exact bigint).
fn mem_arg_from_walrus(arg: &wir::MemArg) -> MemArg {
  MemArg {
    align: arg.align as f64,
    offset: BigInt::from(arg.offset),
  }
}

/// `ExtendedLoad` -> walrus. Total 1:1 mapping (no fallibility, no `_`).
fn extended_load_to_walrus(kind: &ExtendedLoad) -> wir::ExtendedLoad {
  match kind {
    ExtendedLoad::SignExtend => wir::ExtendedLoad::SignExtend,
    ExtendedLoad::ZeroExtend => wir::ExtendedLoad::ZeroExtend,
    ExtendedLoad::ZeroExtendAtomic => wir::ExtendedLoad::ZeroExtendAtomic,
  }
}

/// walrus `ExtendedLoad` -> our enum. Total 1:1 mapping.
fn extended_load_from_walrus(kind: wir::ExtendedLoad) -> ExtendedLoad {
  match kind {
    wir::ExtendedLoad::SignExtend => ExtendedLoad::SignExtend,
    wir::ExtendedLoad::ZeroExtend => ExtendedLoad::ZeroExtend,
    wir::ExtendedLoad::ZeroExtendAtomic => ExtendedLoad::ZeroExtendAtomic,
  }
}

/// `LoadKind` -> walrus. Total 1:1 mapping (the narrow variants carry an
/// [`ExtendedLoad`]; the full-width integers carry `atomic`).
fn load_kind_to_walrus(kind: &LoadKind) -> wir::LoadKind {
  match kind {
    LoadKind::I32 { atomic } => wir::LoadKind::I32 { atomic: *atomic },
    LoadKind::I64 { atomic } => wir::LoadKind::I64 { atomic: *atomic },
    LoadKind::F32 => wir::LoadKind::F32,
    LoadKind::F64 => wir::LoadKind::F64,
    LoadKind::V128 => wir::LoadKind::V128,
    LoadKind::I32_8 { kind } => wir::LoadKind::I32_8 {
      kind: extended_load_to_walrus(kind),
    },
    LoadKind::I32_16 { kind } => wir::LoadKind::I32_16 {
      kind: extended_load_to_walrus(kind),
    },
    LoadKind::I64_8 { kind } => wir::LoadKind::I64_8 {
      kind: extended_load_to_walrus(kind),
    },
    LoadKind::I64_16 { kind } => wir::LoadKind::I64_16 {
      kind: extended_load_to_walrus(kind),
    },
    LoadKind::I64_32 { kind } => wir::LoadKind::I64_32 {
      kind: extended_load_to_walrus(kind),
    },
  }
}

/// walrus `LoadKind` -> our enum. Total 1:1 mapping.
fn load_kind_from_walrus(kind: wir::LoadKind) -> LoadKind {
  match kind {
    wir::LoadKind::I32 { atomic } => LoadKind::I32 { atomic },
    wir::LoadKind::I64 { atomic } => LoadKind::I64 { atomic },
    wir::LoadKind::F32 => LoadKind::F32,
    wir::LoadKind::F64 => LoadKind::F64,
    wir::LoadKind::V128 => LoadKind::V128,
    wir::LoadKind::I32_8 { kind } => LoadKind::I32_8 {
      kind: extended_load_from_walrus(kind),
    },
    wir::LoadKind::I32_16 { kind } => LoadKind::I32_16 {
      kind: extended_load_from_walrus(kind),
    },
    wir::LoadKind::I64_8 { kind } => LoadKind::I64_8 {
      kind: extended_load_from_walrus(kind),
    },
    wir::LoadKind::I64_16 { kind } => LoadKind::I64_16 {
      kind: extended_load_from_walrus(kind),
    },
    wir::LoadKind::I64_32 { kind } => LoadKind::I64_32 {
      kind: extended_load_from_walrus(kind),
    },
  }
}

/// `StoreKind` -> walrus. Total 1:1 mapping (EVERY integer variant carries
/// `atomic`; there is no `ExtendedLoad` on a store).
fn store_kind_to_walrus(kind: &StoreKind) -> wir::StoreKind {
  match kind {
    StoreKind::I32 { atomic } => wir::StoreKind::I32 { atomic: *atomic },
    StoreKind::I64 { atomic } => wir::StoreKind::I64 { atomic: *atomic },
    StoreKind::F32 => wir::StoreKind::F32,
    StoreKind::F64 => wir::StoreKind::F64,
    StoreKind::V128 => wir::StoreKind::V128,
    StoreKind::I32_8 { atomic } => wir::StoreKind::I32_8 { atomic: *atomic },
    StoreKind::I32_16 { atomic } => wir::StoreKind::I32_16 { atomic: *atomic },
    StoreKind::I64_8 { atomic } => wir::StoreKind::I64_8 { atomic: *atomic },
    StoreKind::I64_16 { atomic } => wir::StoreKind::I64_16 { atomic: *atomic },
    StoreKind::I64_32 { atomic } => wir::StoreKind::I64_32 { atomic: *atomic },
  }
}

/// walrus `StoreKind` -> our enum. Total 1:1 mapping.
fn store_kind_from_walrus(kind: wir::StoreKind) -> StoreKind {
  match kind {
    wir::StoreKind::I32 { atomic } => StoreKind::I32 { atomic },
    wir::StoreKind::I64 { atomic } => StoreKind::I64 { atomic },
    wir::StoreKind::F32 => StoreKind::F32,
    wir::StoreKind::F64 => StoreKind::F64,
    wir::StoreKind::V128 => StoreKind::V128,
    wir::StoreKind::I32_8 { atomic } => StoreKind::I32_8 { atomic },
    wir::StoreKind::I32_16 { atomic } => StoreKind::I32_16 { atomic },
    wir::StoreKind::I64_8 { atomic } => StoreKind::I64_8 { atomic },
    wir::StoreKind::I64_16 { atomic } => StoreKind::I64_16 { atomic },
    wir::StoreKind::I64_32 { atomic } => StoreKind::I64_32 { atomic },
  }
}

// ---------------------------------------------------------------------------
// LoadSimdKind <-> walrus. Pure value conversions (the enum carries no arena
// ids, so no resolution and no fallibility); the 8 lane variants map walrus'
// tuple `(u8)` immediate to/from our named `lane` field. Both matches are
// EXHAUSTIVE (no `_`) so a future walrus variant is a COMPILE error rather than
// a silent mismap. Each is `#[inline(never)]` (like the operator `*_to_str`/
// `*_from_str` tables) so its 20-arm match lives in its OWN frame instead of
// inflating the recursive walkers (`emit_one`/`read_one`) — the stack-frame
// discipline the deep-nesting abort guard depends on (see `MAX_NESTING_DEPTH`).
// ---------------------------------------------------------------------------

/// `LoadSimdKind` -> walrus. Total 1:1 mapping (the 8 lane variants surface our
/// named `lane` field as walrus' tuple `(u8)` immediate).
#[inline(never)]
fn load_simd_kind_to_walrus(kind: &LoadSimdKind) -> Result<wir::LoadSimdKind> {
  // The 8 lane variants carry `lane` as `f64` (decoded losslessly, no ToUint32
  // coercion); `checked_lane` narrows it to the `u8` walrus stores, rejecting an
  // out-of-domain lane catchably. Fallible only for those; the 12 fieldless
  // arms never fail.
  Ok(match kind {
    LoadSimdKind::Splat8 => wir::LoadSimdKind::Splat8,
    LoadSimdKind::Splat16 => wir::LoadSimdKind::Splat16,
    LoadSimdKind::Splat32 => wir::LoadSimdKind::Splat32,
    LoadSimdKind::Splat64 => wir::LoadSimdKind::Splat64,
    LoadSimdKind::V128Load8x8S => wir::LoadSimdKind::V128Load8x8S,
    LoadSimdKind::V128Load8x8U => wir::LoadSimdKind::V128Load8x8U,
    LoadSimdKind::V128Load16x4S => wir::LoadSimdKind::V128Load16x4S,
    LoadSimdKind::V128Load16x4U => wir::LoadSimdKind::V128Load16x4U,
    LoadSimdKind::V128Load32x2S => wir::LoadSimdKind::V128Load32x2S,
    LoadSimdKind::V128Load32x2U => wir::LoadSimdKind::V128Load32x2U,
    LoadSimdKind::V128Load32Zero => wir::LoadSimdKind::V128Load32Zero,
    LoadSimdKind::V128Load64Zero => wir::LoadSimdKind::V128Load64Zero,
    LoadSimdKind::V128Load8Lane { lane } => wir::LoadSimdKind::V128Load8Lane(checked_lane(*lane)?),
    LoadSimdKind::V128Load16Lane { lane } => {
      wir::LoadSimdKind::V128Load16Lane(checked_lane(*lane)?)
    }
    LoadSimdKind::V128Load32Lane { lane } => {
      wir::LoadSimdKind::V128Load32Lane(checked_lane(*lane)?)
    }
    LoadSimdKind::V128Load64Lane { lane } => {
      wir::LoadSimdKind::V128Load64Lane(checked_lane(*lane)?)
    }
    LoadSimdKind::V128Store8Lane { lane } => {
      wir::LoadSimdKind::V128Store8Lane(checked_lane(*lane)?)
    }
    LoadSimdKind::V128Store16Lane { lane } => {
      wir::LoadSimdKind::V128Store16Lane(checked_lane(*lane)?)
    }
    LoadSimdKind::V128Store32Lane { lane } => {
      wir::LoadSimdKind::V128Store32Lane(checked_lane(*lane)?)
    }
    LoadSimdKind::V128Store64Lane { lane } => {
      wir::LoadSimdKind::V128Store64Lane(checked_lane(*lane)?)
    }
  })
}

/// walrus `LoadSimdKind` -> our enum. Total 1:1 mapping.
#[inline(never)]
fn load_simd_kind_from_walrus(kind: wir::LoadSimdKind) -> LoadSimdKind {
  match kind {
    wir::LoadSimdKind::Splat8 => LoadSimdKind::Splat8,
    wir::LoadSimdKind::Splat16 => LoadSimdKind::Splat16,
    wir::LoadSimdKind::Splat32 => LoadSimdKind::Splat32,
    wir::LoadSimdKind::Splat64 => LoadSimdKind::Splat64,
    wir::LoadSimdKind::V128Load8x8S => LoadSimdKind::V128Load8x8S,
    wir::LoadSimdKind::V128Load8x8U => LoadSimdKind::V128Load8x8U,
    wir::LoadSimdKind::V128Load16x4S => LoadSimdKind::V128Load16x4S,
    wir::LoadSimdKind::V128Load16x4U => LoadSimdKind::V128Load16x4U,
    wir::LoadSimdKind::V128Load32x2S => LoadSimdKind::V128Load32x2S,
    wir::LoadSimdKind::V128Load32x2U => LoadSimdKind::V128Load32x2U,
    wir::LoadSimdKind::V128Load32Zero => LoadSimdKind::V128Load32Zero,
    wir::LoadSimdKind::V128Load64Zero => LoadSimdKind::V128Load64Zero,
    wir::LoadSimdKind::V128Load8Lane(lane) => LoadSimdKind::V128Load8Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Load16Lane(lane) => LoadSimdKind::V128Load16Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Load32Lane(lane) => LoadSimdKind::V128Load32Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Load64Lane(lane) => LoadSimdKind::V128Load64Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Store8Lane(lane) => LoadSimdKind::V128Store8Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Store16Lane(lane) => LoadSimdKind::V128Store16Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Store32Lane(lane) => LoadSimdKind::V128Store32Lane { lane: lane as f64 },
    wir::LoadSimdKind::V128Store64Lane(lane) => LoadSimdKind::V128Store64Lane { lane: lane as f64 },
  }
}

// ---------------------------------------------------------------------------
// AtomicOp / AtomicWidth <-> walrus. Pure value conversions (fieldless enums,
// no arena ids, so no resolution and no fallibility). Both matches are
// EXHAUSTIVE (no `_`) so a future walrus variant is a COMPILE error rather than
// a silent mismap.
// ---------------------------------------------------------------------------

/// `AtomicOp` -> walrus. Total 1:1 mapping.
fn atomic_op_to_walrus(op: &AtomicOp) -> wir::AtomicOp {
  match op {
    AtomicOp::Add => wir::AtomicOp::Add,
    AtomicOp::Sub => wir::AtomicOp::Sub,
    AtomicOp::And => wir::AtomicOp::And,
    AtomicOp::Or => wir::AtomicOp::Or,
    AtomicOp::Xor => wir::AtomicOp::Xor,
    AtomicOp::Xchg => wir::AtomicOp::Xchg,
  }
}

/// walrus `AtomicOp` -> our enum. Total 1:1 mapping.
fn atomic_op_from_walrus(op: wir::AtomicOp) -> AtomicOp {
  match op {
    wir::AtomicOp::Add => AtomicOp::Add,
    wir::AtomicOp::Sub => AtomicOp::Sub,
    wir::AtomicOp::And => AtomicOp::And,
    wir::AtomicOp::Or => AtomicOp::Or,
    wir::AtomicOp::Xor => AtomicOp::Xor,
    wir::AtomicOp::Xchg => AtomicOp::Xchg,
  }
}

/// `AtomicWidth` -> walrus. Total 1:1 mapping.
fn atomic_width_to_walrus(width: &AtomicWidth) -> wir::AtomicWidth {
  match width {
    AtomicWidth::I32 => wir::AtomicWidth::I32,
    AtomicWidth::I32_8 => wir::AtomicWidth::I32_8,
    AtomicWidth::I32_16 => wir::AtomicWidth::I32_16,
    AtomicWidth::I64 => wir::AtomicWidth::I64,
    AtomicWidth::I64_8 => wir::AtomicWidth::I64_8,
    AtomicWidth::I64_16 => wir::AtomicWidth::I64_16,
    AtomicWidth::I64_32 => wir::AtomicWidth::I64_32,
  }
}

/// walrus `AtomicWidth` -> our enum. Total 1:1 mapping.
fn atomic_width_from_walrus(width: wir::AtomicWidth) -> AtomicWidth {
  match width {
    wir::AtomicWidth::I32 => AtomicWidth::I32,
    wir::AtomicWidth::I32_8 => AtomicWidth::I32_8,
    wir::AtomicWidth::I32_16 => AtomicWidth::I32_16,
    wir::AtomicWidth::I64 => AtomicWidth::I64,
    wir::AtomicWidth::I64_8 => AtomicWidth::I64_8,
    wir::AtomicWidth::I64_16 => AtomicWidth::I64_16,
    wir::AtomicWidth::I64_32 => AtomicWidth::I64_32,
  }
}

// ---------------------------------------------------------------------------
// Label stack: relative depth <-> absolute InstrSeqId.
// ---------------------------------------------------------------------------

/// Resolve a relative branch label depth to the enclosing sequence id it names.
/// `0` is the innermost frame (the top of the stack). An out-of-range depth is
/// rejected before it can produce an invalid `InstrSeqId` (which would abort the
/// process at emit).
fn label_target(depth: u32, label_stack: &[wir::InstrSeqId]) -> Result<wir::InstrSeqId> {
  let len = label_stack.len();
  if (depth as usize) >= len {
    return Err(Error::from_reason(format!(
      "branch label depth {depth} is out of range: only {len} enclosing block(s)"
    )));
  }
  Ok(label_stack[len - 1 - depth as usize])
}

/// Invert an absolute branch-target sequence id back to its relative label
/// depth. This is the exact inverse of [`label_target`] and matches walrus'
/// `branch_target` (`emit.rs:1171`): the innermost frame is depth `0`.
fn label_depth(target: wir::InstrSeqId, label_stack: &[wir::InstrSeqId]) -> Result<u32> {
  label_stack
    .iter()
    .rev()
    .position(|b| *b == target)
    .map(|p| p as u32)
    .ok_or_else(|| {
      Error::from_reason(
        "branch target is not an enclosing block (malformed function body?)".to_string(),
      )
    })
}

// ---------------------------------------------------------------------------
// Operator string tables: <Enum> <-> JS operator name (Binop/Unop/TernOp).
//
// The three walrus operator enums (`BinaryOp`/`UnaryOp`/`TernaryOp`) derive only
// `Copy, Clone, Debug` — no `Display`, `FromStr`, or `PartialEq` — so BOTH string
// directions are generated here from ONE explicit variant list per enum (the
// `str_enum!` macro), which is the single source of truth: `to_str` and
// `from_str` can never drift apart. The JS operator name is exactly the walrus
// variant name (e.g. `"I32Add"`).
//
// MIRROR-WALRUS: the only guarded hazards are string-decode failures (an unknown
// op name, or a lane op given without its `lane` index), each surfaced as a
// catchable error. Nothing here type-checks operands.
//
// Scope is the 352 FIELDLESS variants (BinaryOp 214 + UnaryOp 129 + TernaryOp 9)
// PLUS the 14 lane-carrying SIMD variants (6 `*ReplaceLane`, 8 `*ExtractLane*`,
// each `{ idx: u8 }`), added in C6a. A lane op crosses as its `op` NAME paired
// with a `lane` index (the `InstrDesc.lane` field), since one `op: String` cannot
// carry the immediate. The name<->variant mapping for BOTH the fieldless and the
// lane variants is generated from ONE list per enum (the `str_enum!` macro), the
// single source of truth so `to_str` and `from_str` can never drift. `to_str`
// yields `(name, None)` for a fieldless variant and `(name, Some(idx))` for a
// lane variant (EXHAUSTIVE — no `_` arm — so a new walrus variant is a COMPILE
// error, the safety net against a miscount). `from_str` maps a fieldless name to
// its variant (ignoring any spurious lane), a lane name + a `lane` to the lane
// variant, a lane name WITHOUT a lane to a catchable error, and any other name to
// a catchable error.
// ---------------------------------------------------------------------------

/// The catchable error for an operator name that names no variant of the given
/// enum (neither fieldless nor lane).
fn unknown_op(kind: &str, s: &str) -> Error {
  Error::from_reason(format!("unknown {kind} operator `{s}`"))
}

/// The catchable error for a lane-carrying SIMD op whose descriptor omits the
/// `lane` index (a lane op is not buildable without one).
fn missing_lane(name: &str) -> Error {
  Error::from_reason(format!("SIMD lane op `{name}` requires a `lane` index"))
}

/// Losslessly validate a SIMD `lane` immediate carried as `f64` and narrow it to
/// the `u8` walrus stores it as. Mirrors `checked_index` (rejects
/// NaN/fraction/negative/`> u32::MAX`), then rejects `256..=u32::MAX` on the `u8`
/// narrow — so an out-of-domain lane throws catchably instead of silently
/// aliasing via the old ToUint32 `u8` decode (`2**32 + k` -> `k`). A valid lane
/// `0..=255` passes through losslessly. Consumed at BOTH the emit and the
/// preflight sites of every lane-carrying op (operator `*_from_str` lane arm +
/// `load_simd_kind_to_walrus`), so a bad lane is rejected in the preflight with
/// the arena unchanged.
fn checked_lane(lane: f64) -> Result<u8> {
  let n = checked_index(lane, "lane")?;
  u8::try_from(n)
    .map_err(|_| Error::from_reason(format!("lane must be an integer in 0..=255, got {n}")))
}

/// Generate the two string-conversion functions (and test-only slices of the
/// fieldless and lane variants) for one walrus operator enum from a single
/// variant list — the single source of truth for both directions.
///
/// * `<to_str>(op) -> ("<VariantName>", None)` for a fieldless variant, or
///   `("<VariantName>", Some(idx))` for a lane-carrier (`Variant { idx }`). The
///   match is EXHAUSTIVE (no wildcard) so a new walrus variant fails to compile.
///   Infallible: every variant maps.
/// * `<from_str>(s, lane) -> Ok(Variant)` — a fieldless name ignores `lane`; a
///   lane name requires it (missing => catchable error); any other name is a
///   catchable error.
/// * `#[cfg(test)] const <ALL>: &[<Enum>]` — every fieldless variant, and
///   `#[cfg(test)] const <ALL_LANE>: &[<Enum>]` — every lane variant (with a
///   representative `idx`), for the exhaustive round-trip tests.
macro_rules! str_enum {
  (
    kind: $kind:literal,
    ty: $Enum:ident,
    to_str: $to_str:ident,
    from_str: $from_str:ident,
    all_fieldless: $all:ident,
    all_lane: $all_lane:ident,
    fieldless: [ $($fl:ident),* $(,)? ],
    lane: [ $($ln:ident),* $(,)? ] $(,)?
  ) => {
    // `#[inline(never)]`: these are matches over 200+ operator variants; keeping
    // them out of line guarantees an optimizing build can never inline the huge
    // match into the recursive walkers (`emit_one`/`read_one`), which would
    // explode their stack frame (see `MAX_NESTING_DEPTH`). One cold call per op
    // is negligible.
    #[inline(never)]
    fn $to_str(op: &$Enum) -> (&'static str, Option<u8>) {
      match op {
        $( $Enum::$fl => (stringify!($fl), None), )*
        $( $Enum::$ln { idx } => (stringify!($ln), Some(*idx)), )*
      }
    }

    // `lane` is carried as `Option<f64>` (decoded losslessly, no ToUint32
    // coercion). A fieldless name ignores it entirely (a spurious lane on a
    // fieldless op is dropped, unchanged); a lane name narrows it through
    // `checked_lane`, which rejects an out-of-domain lane catchably.
    #[inline(never)]
    #[allow(unused_variables)]
    fn $from_str(s: &str, lane: Option<f64>) -> Result<$Enum> {
      Ok(match s {
        $( stringify!($fl) => $Enum::$fl, )*
        $( stringify!($ln) => $Enum::$ln {
          idx: checked_lane(lane.ok_or_else(|| missing_lane(stringify!($ln)))?)?,
        }, )*
        other => return Err(unknown_op($kind, other)),
      })
    }

    #[cfg(test)]
    const $all: &[$Enum] = &[ $( $Enum::$fl, )* ];

    #[cfg(test)]
    const $all_lane: &[$Enum] = &[ $( $Enum::$ln { idx: 7 }, )* ];
  };
}

str_enum! {
  kind: "binary",
  ty: BinaryOp,
  to_str: binop_to_str,
  from_str: binop_from_str,
  all_fieldless: BINOP_ALL_FIELDLESS,
  all_lane: BINOP_ALL_LANE,
  fieldless: [
    I32Eq, I32Ne, I32LtS, I32LtU, I32GtS, I32GtU, I32LeS, I32LeU, I32GeS, I32GeU, I64Eq, I64Ne,
    I64LtS, I64LtU, I64GtS, I64GtU, I64LeS, I64LeU, I64GeS, I64GeU, F32Eq, F32Ne, F32Lt, F32Gt,
    F32Le, F32Ge, F64Eq, F64Ne, F64Lt, F64Gt, F64Le, F64Ge, I32Add, I32Sub, I32Mul, I32DivS,
    I32DivU, I32RemS, I32RemU, I32And, I32Or, I32Xor, I32Shl, I32ShrS, I32ShrU, I32Rotl,
    I32Rotr, I64Add, I64Sub, I64Mul, I64DivS, I64DivU, I64RemS, I64RemU, I64And, I64Or, I64Xor,
    I64Shl, I64ShrS, I64ShrU, I64Rotl, I64Rotr, F32Add, F32Sub, F32Mul, F32Div, F32Min, F32Max,
    F32Copysign, F64Add, F64Sub, F64Mul, F64Div, F64Min, F64Max, F64Copysign, I8x16Eq, I8x16Ne,
    I8x16LtS, I8x16LtU, I8x16GtS, I8x16GtU, I8x16LeS, I8x16LeU, I8x16GeS, I8x16GeU, I16x8Eq,
    I16x8Ne, I16x8LtS, I16x8LtU, I16x8GtS, I16x8GtU, I16x8LeS, I16x8LeU, I16x8GeS, I16x8GeU,
    I32x4Eq, I32x4Ne, I32x4LtS, I32x4LtU, I32x4GtS, I32x4GtU, I32x4LeS, I32x4LeU, I32x4GeS,
    I32x4GeU, I64x2Eq, I64x2Ne, I64x2LtS, I64x2GtS, I64x2LeS, I64x2GeS, F32x4Eq, F32x4Ne,
    F32x4Lt, F32x4Gt, F32x4Le, F32x4Ge, F64x2Eq, F64x2Ne, F64x2Lt, F64x2Gt, F64x2Le, F64x2Ge,
    V128And, V128Or, V128Xor, V128AndNot, I8x16Shl, I8x16ShrS, I8x16ShrU, I8x16Add,
    I8x16AddSatS, I8x16AddSatU, I8x16Sub, I8x16SubSatS, I8x16SubSatU, I16x8Shl, I16x8ShrS,
    I16x8ShrU, I16x8Add, I16x8AddSatS, I16x8AddSatU, I16x8Sub, I16x8SubSatS, I16x8SubSatU,
    I16x8Mul, I32x4Shl, I32x4ShrS, I32x4ShrU, I32x4Add, I32x4Sub, I32x4Mul, I64x2Shl,
    I64x2ShrS, I64x2ShrU, I64x2Add, I64x2Sub, I64x2Mul, F32x4Add, F32x4Sub, F32x4Mul, F32x4Div,
    F32x4Min, F32x4Max, F32x4PMin, F32x4PMax, F64x2Add, F64x2Sub, F64x2Mul, F64x2Div, F64x2Min,
    F64x2Max, F64x2PMin, F64x2PMax, I8x16NarrowI16x8S, I8x16NarrowI16x8U, I16x8NarrowI32x4S,
    I16x8NarrowI32x4U, I8x16AvgrU, I16x8AvgrU, I8x16MinS, I8x16MinU, I8x16MaxS, I8x16MaxU,
    I16x8MinS, I16x8MinU, I16x8MaxS, I16x8MaxU, I32x4MinS, I32x4MinU, I32x4MaxS, I32x4MaxU,
    I32x4DotI16x8S, I16x8Q15MulrSatS, I16x8ExtMulLowI8x16S, I16x8ExtMulHighI8x16S,
    I16x8ExtMulLowI8x16U, I16x8ExtMulHighI8x16U, I32x4ExtMulLowI16x8S, I32x4ExtMulHighI16x8S,
    I32x4ExtMulLowI16x8U, I32x4ExtMulHighI16x8U, I64x2ExtMulLowI32x4S, I64x2ExtMulHighI32x4S,
    I64x2ExtMulLowI32x4U, I64x2ExtMulHighI32x4U, I8x16RelaxedSwizzle, F32x4RelaxedMin,
    F32x4RelaxedMax, F64x2RelaxedMin, F64x2RelaxedMax, I16x8RelaxedQ15mulrS,
    I16x8RelaxedDotI8x16I7x16S,
  ],
  lane: [
    I8x16ReplaceLane, I16x8ReplaceLane, I32x4ReplaceLane, I64x2ReplaceLane, F32x4ReplaceLane,
    F64x2ReplaceLane,
  ],
}

str_enum! {
  kind: "unary",
  ty: UnaryOp,
  to_str: unop_to_str,
  from_str: unop_from_str,
  all_fieldless: UNOP_ALL_FIELDLESS,
  all_lane: UNOP_ALL_LANE,
  fieldless: [
    I32Eqz, I32Clz, I32Ctz, I32Popcnt, I64Eqz, I64Clz, I64Ctz, I64Popcnt, F32Abs, F32Neg,
    F32Ceil, F32Floor, F32Trunc, F32Nearest, F32Sqrt, F64Abs, F64Neg, F64Ceil, F64Floor,
    F64Trunc, F64Nearest, F64Sqrt, I32WrapI64, I32TruncSF32, I32TruncUF32, I32TruncSF64,
    I32TruncUF64, I64ExtendSI32, I64ExtendUI32, I64TruncSF32, I64TruncUF32, I64TruncSF64,
    I64TruncUF64, F32ConvertSI32, F32ConvertUI32, F32ConvertSI64, F32ConvertUI64, F32DemoteF64,
    F64ConvertSI32, F64ConvertUI32, F64ConvertSI64, F64ConvertUI64, F64PromoteF32,
    I32ReinterpretF32, I64ReinterpretF64, F32ReinterpretI32, F64ReinterpretI64, I32Extend8S,
    I32Extend16S, I64Extend8S, I64Extend16S, I64Extend32S, I8x16Splat, I16x8Splat, I32x4Splat,
    I64x2Splat, F32x4Splat, F64x2Splat, V128Not, V128AnyTrue, I8x16Abs, I8x16Popcnt, I8x16Neg,
    I8x16AllTrue, I8x16Bitmask, I16x8Abs, I16x8Neg, I16x8AllTrue, I16x8Bitmask, I32x4Abs,
    I32x4Neg, I32x4AllTrue, I32x4Bitmask, I64x2Abs, I64x2Neg, I64x2AllTrue, I64x2Bitmask,
    F32x4Abs, F32x4Neg, F32x4Sqrt, F32x4Ceil, F32x4Floor, F32x4Trunc, F32x4Nearest, F64x2Abs,
    F64x2Neg, F64x2Sqrt, F64x2Ceil, F64x2Floor, F64x2Trunc, F64x2Nearest,
    I16x8ExtAddPairwiseI8x16S, I16x8ExtAddPairwiseI8x16U, I32x4ExtAddPairwiseI16x8S,
    I32x4ExtAddPairwiseI16x8U, I64x2ExtendLowI32x4S, I64x2ExtendHighI32x4S,
    I64x2ExtendLowI32x4U, I64x2ExtendHighI32x4U, I32x4TruncSatF64x2SZero,
    I32x4TruncSatF64x2UZero, F64x2ConvertLowI32x4S, F64x2ConvertLowI32x4U,
    F32x4DemoteF64x2Zero, F64x2PromoteLowF32x4, I32x4TruncSatF32x4S, I32x4TruncSatF32x4U,
    F32x4ConvertI32x4S, F32x4ConvertI32x4U, I32TruncSSatF32, I32TruncUSatF32, I32TruncSSatF64,
    I32TruncUSatF64, I64TruncSSatF32, I64TruncUSatF32, I64TruncSSatF64, I64TruncUSatF64,
    I16x8WidenLowI8x16S, I16x8WidenLowI8x16U, I16x8WidenHighI8x16S, I16x8WidenHighI8x16U,
    I32x4WidenLowI16x8S, I32x4WidenLowI16x8U, I32x4WidenHighI16x8S, I32x4WidenHighI16x8U,
    I32x4RelaxedTruncF32x4S, I32x4RelaxedTruncF32x4U, I32x4RelaxedTruncF64x2SZero,
    I32x4RelaxedTruncF64x2UZero,
  ],
  lane: [
    I8x16ExtractLaneS, I8x16ExtractLaneU, I16x8ExtractLaneS, I16x8ExtractLaneU,
    I32x4ExtractLane, I64x2ExtractLane, F32x4ExtractLane, F64x2ExtractLane,
  ],
}

str_enum! {
  kind: "ternary",
  ty: TernaryOp,
  to_str: ternop_to_str,
  from_str: ternop_from_str,
  all_fieldless: TERNOP_ALL_FIELDLESS,
  all_lane: TERNOP_ALL_LANE,
  fieldless: [
    F32x4RelaxedMadd, F32x4RelaxedNmadd, F64x2RelaxedMadd, F64x2RelaxedNmadd,
    I8x16RelaxedLaneselect, I16x8RelaxedLaneselect, I32x4RelaxedLaneselect,
    I64x2RelaxedLaneselect, I32x4RelaxedDotI8x16I7x16AddS,
  ],
  lane: [
  ],
}

// ---------------------------------------------------------------------------
// SIMD byte-payload conversions (v128 const bytes, i8x16.shuffle indices).
//
// Both cross as a raw `Uint8Array` of EXACTLY 16 bytes; any other length is a
// catchable REPRESENTATION error (mirror of the `constexpr::ConstExpr::v128`
// factory), never a wasm semantic check. Each is `#[inline(never)]` so its
// `[u8; 16]`/`u128` temporaries live in their OWN frame rather than inflating the
// recursive walkers (`emit_one`/`validate_one`/`read_one`) — the same stack-frame
// discipline as `emit_atomic` (see `MAX_NESTING_DEPTH`).
// ---------------------------------------------------------------------------

/// Fold exactly 16 little-endian bytes into a `u128` (a v128 const), or a
/// catchable error on any other length. `#[inline(never)]`: keeps the `[u8; 16]`
/// out of the emit/validate walker frames.
#[inline(never)]
fn v128_bytes_to_u128(bytes: &[u8]) -> Result<u128> {
  let array: [u8; 16] = bytes.try_into().map_err(|_| {
    Error::from_reason(format!(
      "v128 const requires exactly 16 bytes, got {}",
      bytes.len()
    ))
  })?;
  Ok(u128::from_le_bytes(array))
}

/// Unfold a `u128` (a v128 const) into its 16 little-endian bytes as a
/// `Uint8Array`. `#[inline(never)]`: keeps the `[u8; 16]` out of `read_one`'s
/// frame.
#[inline(never)]
fn v128_u128_to_bytes(value: u128) -> Uint8Array {
  Uint8Array::from(value.to_le_bytes().to_vec())
}

/// Unfold the 16-byte `I8x16Shuffle` lane-index immediate into a `Uint8Array`.
/// `#[inline(never)]`: keeps the `[u8; 16]` out of `read_one`'s frame (same
/// stack-frame discipline as `v128_u128_to_bytes`).
#[inline(never)]
fn shuffle_indices_to_bytes(indices: &[u8; 16]) -> Uint8Array {
  Uint8Array::from(indices.to_vec())
}

/// Coerce a byte slice to the `[u8; 16]` shuffle immediate, or a catchable error
/// on any other length. The single source of the length check + message shared by
/// [`emit_shuffle`] and [`validate_shuffle`].
fn to_shuffle_indices(bytes: &[u8]) -> Result<[u8; 16]> {
  bytes.try_into().map_err(|_| {
    Error::from_reason(format!(
      "i8x16.shuffle requires exactly 16 lane indices, got {}",
      bytes.len()
    ))
  })
}

// ---------------------------------------------------------------------------
// Emit path: InstrDesc array -> walrus function body.
// ---------------------------------------------------------------------------

/// Emit a descriptor array into the sequence `seq_id`, maintaining the label
/// stack. `seq_id` is pushed for the duration of the body (so branches inside
/// target it at the correct depth) and popped on exit.
pub(crate) fn emit_desc(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  body: Vec<InstrDesc>,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<()> {
  // Cap nesting BEFORE descending (see `MAX_NESTING_DEPTH`). On entry
  // `label_stack.len()` is the parent depth (this level's frame is pushed just
  // below), so `>= MAX_NESTING_DEPTH` means this level would be `cap + 1`. Guards
  // `emit_desc` directly for defense in depth: `build_function` preflights with
  // `validate_body` first, but `emit_desc` is `pub(crate)` and a future
  // `replace_*_func` may reach it without that preflight.
  if label_stack.len() >= MAX_NESTING_DEPTH {
    return Err(nesting_too_deep());
  }
  label_stack.push(seq_id);
  for d in body {
    emit_one(fb, module, seq_id, d, label_stack)?;
  }
  label_stack.pop();
  Ok(())
}

/// Emit a single descriptor into `seq_id`.
fn emit_one(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  d: InstrDesc,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<()> {
  // Only the four recursive control constructs stay on the recursion stack; every
  // other (leaf / non-recursive) instruction is delegated to `emit_leaf`
  // (`#[inline(never)]`), so the summed frame of ~50 leaf arms AND the full
  // `InstrDesc` destructure live in `emit_leaf`'s frame — live one instruction at
  // a time, NEVER accumulating on the depth-`MAX_NESTING_DEPTH` recursion stack.
  // This is the exact `read_one`/`read_leaf` split applied to emit, and it is what
  // keeps the at-cap canary green as `InstrDesc` widens (each new field would
  // otherwise widen THIS recursive frame). The recursion (`emit_desc`) stays here.
  if !matches!(
    d.r#type.as_str(),
    "Block" | "Loop" | "IfElse" | "TryTable" | "Try"
  ) {
    return emit_leaf(fb, module, seq_id, d, label_stack);
  }
  // Destructure ONLY the control-construct fields (the rest are dropped via `..`),
  // so the recursion frame holds a handful of `Option`s, not all ~35 fields.
  let InstrDesc {
    r#type,
    block_type,
    seq,
    consequent,
    alternative,
    catches,
    ..
  } = d;

  match r#type.as_str() {
    "Block" => {
      let ty = to_instr_seq_type(module, block_type)?;
      let child = fb.dangling_instr_seq(ty).id();
      emit_desc(fb, module, child, seq.unwrap_or_default(), label_stack)?;
      fb.instr_seq(seq_id).instr(wir::Block { seq: child });
    }
    "Loop" => {
      let ty = to_instr_seq_type(module, block_type)?;
      let child = fb.dangling_instr_seq(ty).id();
      emit_desc(fb, module, child, seq.unwrap_or_default(), label_stack)?;
      fb.instr_seq(seq_id).instr(wir::Loop { seq: child });
    }
    "IfElse" => {
      // Both arms share the block type; each arm is its OWN label frame (see the
      // module-level note), so recurse into each with its own sequence id.
      let ty = to_instr_seq_type(module, block_type)?;
      let consequent_id = fb.dangling_instr_seq(ty).id();
      let alternative_id = fb.dangling_instr_seq(ty).id();
      emit_desc(
        fb,
        module,
        consequent_id,
        consequent.unwrap_or_default(),
        label_stack,
      )?;
      emit_desc(
        fb,
        module,
        alternative_id,
        alternative.unwrap_or_default(),
        label_stack,
      )?;
      fb.instr_seq(seq_id).instr(wir::IfElse {
        consequent: consequent_id,
        alternative: alternative_id,
      });
    }
    // Modern exception handling (C8a). `TryTable` is a `Block` twin — a control
    // construct with its OWN child `seq` (the try body) PLUS a catch-clause list.
    // LOAD-BEARING SCOPING: the clause labels resolve against the CURRENT (outer)
    // label stack — the try_table's own seq is NOT pushed yet — so we convert them
    // FIRST (walrus computes catch branch targets BEFORE pushing the try_table
    // block; see `emit.rs`), THEN descend into the body via `emit_desc` (which
    // pushes `child`, so inside the body `br 0` targets the try_table block). The
    // per-clause work lives in `try_table_catches_to_walrus` (`#[inline(never)]`),
    // keeping this recursive arm `Block`-slim. Missing `catches` = catch-less
    // (empty), a legal `try_table` — mirror `seq.unwrap_or_default()`.
    "TryTable" => {
      let catches =
        try_table_catches_to_walrus(module, catches.as_deref().unwrap_or(&[]), label_stack)?;
      let ty = to_instr_seq_type(module, block_type)?;
      let child = fb.dangling_instr_seq(ty).id();
      emit_desc(fb, module, child, seq.unwrap_or_default(), label_stack)?;
      fb.instr_seq(seq_id).instr(wir::TryTable {
        seq: child,
        catches,
      });
    }
    // Legacy exception handling (C8b). `Try` is the 5th recursive control
    // construct — a `Block` twin whose child `seq` is the try body PLUS a legacy
    // catch-clause list, EACH `LegacyCatch`/`LegacyCatchAll` handler being its OWN
    // child `InstrSeq` (a SIBLING of the try body at the same label depth, like an
    // `IfElse` arm), not a branch target. All of that work — building the body,
    // building + emitting each handler body, and resolving each clause — lives in
    // `emit_try` (`#[inline(never)]`) so this recursive `emit_one` frame does NOT
    // grow as the try machinery is added (the at-cap canary stays green).
    "Try" => {
      emit_try(fb, module, seq_id, block_type, seq, catches, label_stack)?;
    }
    // The `matches!` guard above admits only the five control types.
    _ => unreachable!("emit_one handles only control constructs"),
  }
  Ok(())
}

/// Emit one NON-CONTROL (leaf / non-recursive) descriptor. Split out of
/// [`emit_one`] and marked `#[inline(never)]` so this large match — ~50 arms plus
/// the full `InstrDesc` destructure — keeps its frame OFF the depth-
/// [`MAX_NESTING_DEPTH`] recursion stack (the mirror image of [`read_leaf`]). It
/// is called once per leaf and never recurses, so its frame is live for a single
/// instruction at a time. `Br`/`BrIf`/`BrTable` (and the GC branches) read
/// `label_stack` to resolve their targets; no leaf recurses into `emit_desc`.
#[inline(never)]
fn emit_leaf(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  d: InstrDesc,
  // Immutable slice: a leaf never recurses, so it only READS the label stack to
  // resolve `Br`/`BrIf`/`BrTable`/`BrOn*` targets — no push/pop.
  label_stack: &[wir::InstrSeqId],
) -> Result<()> {
  let InstrDesc {
    r#type,
    value,
    local,
    global,
    func,
    select_type,
    label,
    labels,
    default_label,
    op,
    lane,
    memory,
    src_memory,
    data,
    mem_arg,
    load_kind,
    store_kind,
    load_simd_kind,
    table,
    src_table,
    elem,
    type_index,
    ref_type,
    atomic_op,
    atomic_width,
    sixty_four,
    shuffle_indices,
    field,
    len,
    src_type_index,
    to_ref_type,
    tag,
    relative_depth,
    // `block_type`/`seq`/`consequent`/`alternative`/`catches` are control-only
    // (handled in `emit_one`, never reached here) — dropped via `..`.
    ..
  } = d;

  match r#type.as_str() {
    "Unreachable" => {
      fb.instr_seq(seq_id).instr(wir::Unreachable {});
    }
    "Return" => {
      fb.instr_seq(seq_id).instr(wir::Return {});
    }
    "Drop" => {
      fb.instr_seq(seq_id).instr(wir::Drop {});
    }
    "Const" => {
      let value = value.ok_or_else(|| missing("Const", "value"))?;
      let wv = match value {
        ConstValue::I32 { value } => wir::Value::I32(value),
        ConstValue::I64 { value } => {
          let (v, lossless) = value.get_i64();
          if !lossless {
            return Err(Error::from_reason(
              "i64 const value does not fit losslessly in a signed 64-bit integer",
            ));
          }
          wir::Value::I64(v)
        }
        ConstValue::F32 { value } => wir::Value::F32(value as f32),
        ConstValue::F64 { value } => wir::Value::F64(value),
        // The `[u8; 16]` -> `u128` fold lives in `v128_bytes_to_u128` (an
        // `#[inline(never)]` helper) so its array temporary stays out of this
        // recursive walker's frame; `wv` already sizes for `Value::V128(u128)`.
        ConstValue::V128 { value } => wir::Value::V128(v128_bytes_to_u128(&value)?),
      };
      fb.instr_seq(seq_id).instr(wir::Const { value: wv });
    }
    "LocalGet" => {
      let id = local_id_at(
        module,
        checked_index(local.ok_or_else(|| missing("LocalGet", "local"))?, "local")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::LocalGet { local: id });
    }
    "LocalSet" => {
      let id = local_id_at(
        module,
        checked_index(local.ok_or_else(|| missing("LocalSet", "local"))?, "local")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::LocalSet { local: id });
    }
    "LocalTee" => {
      let id = local_id_at(
        module,
        checked_index(local.ok_or_else(|| missing("LocalTee", "local"))?, "local")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::LocalTee { local: id });
    }
    "GlobalGet" => {
      let id = global_id_at(
        module,
        checked_index(
          global.ok_or_else(|| missing("GlobalGet", "global"))?,
          "global",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::GlobalGet { global: id });
    }
    "GlobalSet" => {
      let id = global_id_at(
        module,
        checked_index(
          global.ok_or_else(|| missing("GlobalSet", "global"))?,
          "global",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::GlobalSet { global: id });
    }
    "Call" => {
      let id = function_id_at(
        module,
        checked_index(func.ok_or_else(|| missing("Call", "func"))?, "func")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::Call { func: id });
    }
    "Select" => {
      let ty = match select_type {
        Some(vt) => Some(val_type_to_walrus_in(module, vt)?),
        None => None,
      };
      fb.instr_seq(seq_id).instr(wir::Select { ty });
    }
    // The four recursive control constructs (`Block`/`Loop`/`IfElse`/`TryTable`)
    // are handled inline in `emit_one` and never reach this leaf helper.
    // `Throw`/`ThrowRef` are leaf EH ops: delegated to `emit_eh` (`#[inline(never)]`)
    // for the SAME frame-size reason as `emit_gc_ref` (keep their locals out of
    // this recursive walker's frame). `Throw` resolves its `tag` via `tag_id_at`
    // (the abort guard); `ThrowRef` is fieldless.
    "Throw" | "ThrowRef" => {
      // Pass `tag` RAW: `emit_eh` narrows it INSIDE the `Throw` arm only, so the
      // fieldless `ThrowRef` never `checked_index`es an irrelevant `tag` (which
      // preflight's `validate_eh` also ignores) — keeps emit-checked ==
      // preflight-checked == the fields the opcode consumes.
      emit_eh(fb, module, seq_id, r#type.as_str(), tag)?;
    }
    // Legacy EH leaf op (C8b): `Rethrow` carries a RAW `relativeDepth` `u32` walrus
    // never resolves (§A.3) — emitted verbatim, NO `label_target`/`label_stack`. A
    // missing depth is a catchable representation error, not an abort surface.
    "Rethrow" => {
      let relative_depth = checked_index(
        relative_depth.ok_or_else(|| missing("Rethrow", "relativeDepth"))?,
        "relativeDepth",
      )?;
      fb.instr_seq(seq_id).instr(wir::Rethrow { relative_depth });
    }
    "Br" => {
      let target = label_target(
        checked_index(label.ok_or_else(|| missing("Br", "label"))?, "label")?,
        label_stack,
      )?;
      fb.instr_seq(seq_id).instr(wir::Br { block: target });
    }
    "BrIf" => {
      let target = label_target(
        checked_index(label.ok_or_else(|| missing("BrIf", "label"))?, "label")?,
        label_stack,
      )?;
      fb.instr_seq(seq_id).instr(wir::BrIf { block: target });
    }
    "BrTable" => {
      let labels = labels.ok_or_else(|| missing("BrTable", "labels"))?;
      let default = label_target(
        checked_index(
          default_label.ok_or_else(|| missing("BrTable", "defaultLabel"))?,
          "defaultLabel",
        )?,
        label_stack,
      )?;
      let blocks = labels
        .into_iter()
        .map(|d| label_target(checked_index(d, "labels")?, label_stack))
        .collect::<Result<Vec<_>>>()?;
      fb.instr_seq(seq_id).instr(wir::BrTable {
        blocks: blocks.into_boxed_slice(),
        default,
      });
    }
    // `Binop`/`Unop` decode BOTH fieldless ops and the 14 lane-carriers: the
    // `op` name selects the variant and, for a lane op, `lane` supplies its `idx`
    // (missing lane on a lane op => catchable error inside `from_str`). The op
    // enums are `Copy` (a `BinaryOp`/`UnaryOp` is tiny), so this arm adds no
    // meaningful frame; the big match lives in the separate `*_from_str` fn.
    "Binop" => {
      let op = binop_from_str(&op.ok_or_else(|| missing("Binop", "op"))?, lane)?;
      fb.instr_seq(seq_id).instr(wir::Binop { op });
    }
    "Unop" => {
      let op = unop_from_str(&op.ok_or_else(|| missing("Unop", "op"))?, lane)?;
      fb.instr_seq(seq_id).instr(wir::Unop { op });
    }
    "TernOp" => {
      // TernaryOp has no lane-carriers; `lane` is ignored.
      let op = ternop_from_str(&op.ok_or_else(|| missing("TernOp", "op"))?, lane)?;
      fb.instr_seq(seq_id).instr(wir::TernOp { op });
    }
    // Fixed-shape SIMD instructions. `V128Bitselect`/`I8x16Swizzle` are fieldless
    // (trivial arms). `I8x16Shuffle` carries a 16-byte immediate whose length
    // check + `[u8; 16]` build live in `emit_shuffle` (an `#[inline(never)]`
    // helper) so its array temporary stays out of this recursive walker's frame.
    "V128Bitselect" => {
      fb.instr_seq(seq_id).instr(wir::V128Bitselect {});
    }
    "I8x16Swizzle" => {
      fb.instr_seq(seq_id).instr(wir::I8x16Swizzle {});
    }
    "I8x16Shuffle" => {
      emit_shuffle(fb, seq_id, shuffle_indices)?;
    }
    "MemorySize" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("MemorySize", "memory"))?,
          "memory",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::MemorySize { memory });
    }
    "MemoryGrow" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("MemoryGrow", "memory"))?,
          "memory",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::MemoryGrow { memory });
    }
    "MemoryInit" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("MemoryInit", "memory"))?,
          "memory",
        )?,
      )?;
      let data = data_id_at(
        module,
        checked_index(data.ok_or_else(|| missing("MemoryInit", "data"))?, "data")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::MemoryInit { memory, data });
    }
    "DataDrop" => {
      let data = data_id_at(
        module,
        checked_index(data.ok_or_else(|| missing("DataDrop", "data"))?, "data")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::DataDrop { data });
    }
    "MemoryCopy" => {
      // `memory` is the DESTINATION, `srcMemory` the SOURCE (see `InstrDesc`).
      let dst = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("MemoryCopy", "memory"))?,
          "memory",
        )?,
      )?;
      let src = memory_id_at(
        module,
        checked_index(
          src_memory.ok_or_else(|| missing("MemoryCopy", "srcMemory"))?,
          "srcMemory",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::MemoryCopy { src, dst });
    }
    "MemoryFill" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("MemoryFill", "memory"))?,
          "memory",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::MemoryFill { memory });
    }
    "Load" => {
      let memory = memory_id_at(
        module,
        checked_index(memory.ok_or_else(|| missing("Load", "memory"))?, "memory")?,
      )?;
      let kind = load_kind_to_walrus(&load_kind.ok_or_else(|| missing("Load", "loadKind"))?);
      let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("Load", "memArg"))?)?;
      fb.instr_seq(seq_id).instr(wir::Load { memory, kind, arg });
    }
    "Store" => {
      let memory = memory_id_at(
        module,
        checked_index(memory.ok_or_else(|| missing("Store", "memory"))?, "memory")?,
      )?;
      let kind = store_kind_to_walrus(&store_kind.ok_or_else(|| missing("Store", "storeKind"))?);
      let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("Store", "memArg"))?)?;
      fb.instr_seq(seq_id).instr(wir::Store { memory, kind, arg });
    }
    // SIMD vector load / load-lane / store-lane. Delegated to `emit_load_simd`
    // (an `#[inline(never)]` helper) SO THAT the arm's locals (the `MemoryId`,
    // the `wir::LoadSimdKind`, the `wir::MemArg`) and its 20-arm kind conversion
    // do NOT inflate this recursive walker's frame — the same stack-frame
    // discipline as `emit_atomic`/`emit_shuffle` (see `MAX_NESTING_DEPTH`).
    "LoadSimd" => {
      // Pass `memory` RAW: `emit_load_simd` narrows it INSIDE (LoadSimd always
      // consumes it, matching preflight).
      emit_load_simd(fb, module, seq_id, memory, load_simd_kind, mem_arg)?;
    }
    // Atomic (threads) instructions. Delegated to `emit_atomic` (a separate,
    // non-inlined function) SO THAT the atomic arms' locals do NOT inflate this
    // recursive walker's stack frame: `emit_one` recurses once per control-flow
    // nesting level (via the `Block`/`Loop`/`IfElse` arms), and in a debug build a
    // single big `match` reserves frame space for EVERY arm's locals at every
    // level. Keeping the atomic locals in their own frame preserves the
    // `MAX_NESTING_DEPTH` headroom the deep-nesting abort guard depends on.
    "AtomicRmw" | "Cmpxchg" | "AtomicNotify" | "AtomicWait" | "AtomicFence" => {
      // Pass `memory` RAW: `emit_atomic` narrows it INSIDE the four
      // memory-bearing arms only, so the fieldless `AtomicFence` never
      // `checked_index`es an irrelevant `memory` (which preflight's
      // `validate_atomic` also ignores).
      emit_atomic(
        fb,
        module,
        seq_id,
        r#type.as_str(),
        memory,
        atomic_op,
        atomic_width,
        mem_arg,
        sixty_four,
      )?;
    }
    "TableGet" => {
      let table = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableGet", "table"))?, "table")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableGet { table });
    }
    "TableSet" => {
      let table = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableSet", "table"))?, "table")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableSet { table });
    }
    "TableGrow" => {
      let table = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableGrow", "table"))?, "table")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableGrow { table });
    }
    "TableSize" => {
      let table = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableSize", "table"))?, "table")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableSize { table });
    }
    "TableFill" => {
      let table = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableFill", "table"))?, "table")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableFill { table });
    }
    "TableInit" => {
      let table = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableInit", "table"))?, "table")?,
      )?;
      let elem = element_id_at(
        module,
        checked_index(elem.ok_or_else(|| missing("TableInit", "elem"))?, "elem")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableInit { table, elem });
    }
    "TableCopy" => {
      // `table` is the DESTINATION, `srcTable` the SOURCE (see `InstrDesc`).
      let dst = table_id_at(
        module,
        checked_index(table.ok_or_else(|| missing("TableCopy", "table"))?, "table")?,
      )?;
      let src = table_id_at(
        module,
        checked_index(
          src_table.ok_or_else(|| missing("TableCopy", "srcTable"))?,
          "srcTable",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::TableCopy { src, dst });
    }
    "ElemDrop" => {
      let elem = element_id_at(
        module,
        checked_index(elem.ok_or_else(|| missing("ElemDrop", "elem"))?, "elem")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ElemDrop { elem });
    }
    "CallIndirect" => {
      // Reuse `resolve_type_id` for the callee type: it rejects a nonexistent
      // index AND an internal function-entry type index (neither is a real user
      // type a `call_indirect` may name), turning either into a catchable error
      // rather than an emit-time abort.
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("CallIndirect", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let table = table_id_at(
        module,
        checked_index(
          table.ok_or_else(|| missing("CallIndirect", "table"))?,
          "table",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::CallIndirect { ty, table });
    }
    "RefNull" => {
      // The one ref instruction whose payload is a whole `RefType`. Route the
      // heap type through the MODULE-AWARE `heap_type_to_walrus_in`: a
      // concrete/exact heap carries a `TypeId` that a non-module-aware conversion
      // could not rebuild, and a foreign/deleted/entry index reaching emit panics
      // walrus (`get_type_index`) into an uncatchable abort — resolving it here
      // turns that into a catchable error. MIRROR-WALRUS: whether the null type is
      // legal is NOT checked.
      let rt = ref_type.ok_or_else(|| missing("RefNull", "refType"))?;
      let heap_type = heap_type_to_walrus_in(module, rt.heap)?;
      fb.instr_seq(seq_id).instr(wir::RefNull {
        ty: walrus::RefType {
          nullable: rt.nullable,
          heap_type,
        },
      });
    }
    "RefIsNull" => {
      fb.instr_seq(seq_id).instr(wir::RefIsNull {});
    }
    "RefFunc" => {
      // Reuse `function_id_at` (the abort guard). MIRROR-WALRUS: whether the func
      // is "declared" for `ref.func` is NOT our concern.
      let func = function_id_at(
        module,
        checked_index(func.ok_or_else(|| missing("RefFunc", "func"))?, "func")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::RefFunc { func });
    }
    "ReturnCall" => {
      let func = function_id_at(
        module,
        checked_index(func.ok_or_else(|| missing("ReturnCall", "func"))?, "func")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ReturnCall { func });
    }
    "ReturnCallIndirect" => {
      // Identical payload handling to `CallIndirect`: reuse `resolve_type_id` for
      // the callee type (rejects a nonexistent AND an internal function-entry type
      // index) plus `table_id_at`. MIRROR-WALRUS: the tail-call signature match is
      // NOT checked.
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ReturnCallIndirect", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let table = table_id_at(
        module,
        checked_index(
          table.ok_or_else(|| missing("ReturnCallIndirect", "table"))?,
          "table",
        )?,
      )?;
      fb.instr_seq(seq_id)
        .instr(wir::ReturnCallIndirect { ty, table });
    }
    // GC reference instructions (C7b) — the label-free subset (the `br_on_*`
    // label-carriers are a later task). Delegated to `emit_gc_ref` (an
    // `#[inline(never)]` helper) for the SAME frame-size reason as `emit_atomic`:
    // eleven inline arms — even individually `RefNull`-sized — were MEASURED to
    // grow this recursive walker's -O0 frame past the at-cap headroom (the at-cap
    // canary SIGSEGV'd), so their locals live in the helper's own frame instead.
    "RefAsNonNull" | "CallRef" | "ReturnCallRef" | "RefI31" | "I31GetS" | "I31GetU" | "RefTest"
    | "RefCast" | "AnyConvertExtern" | "ExternConvertAny" | "RefEq" => {
      // Pass `type_index` RAW: `emit_gc_ref` narrows it INSIDE the
      // `CallRef`/`ReturnCallRef` arms only, so the nine fieldless / ref-type
      // ops never `checked_index` an irrelevant `type_index` (which preflight's
      // `validate_gc_ref` also ignores).
      emit_gc_ref(fb, module, seq_id, r#type.as_str(), type_index, ref_type)?;
    }
    // GC branch instructions (C7c) — the label-carrying subset. Delegated to
    // `emit_br_on` (an `#[inline(never)]` helper) for the SAME frame-size reason
    // as `emit_gc_ref`: a `BrOnCast` arm is multi-statement (two required-field
    // checks, two heap conversions, a five-field struct), so its locals must NOT
    // inflate this recursive walker's frame (see `MAX_NESTING_DEPTH`). All four
    // resolve their branch `label` via `label_target` exactly like `Br`;
    // `BrOnCast`/`BrOnCastFail` also convert their `refType` (FROM) and
    // `toRefType` (TO) heaps via the module-aware `heap_type_to_walrus_in`.
    "BrOnNull" | "BrOnNonNull" | "BrOnCast" | "BrOnCastFail" => {
      // Pass `label` RAW: `emit_br_on` narrows it INSIDE (all four br_on ops
      // consume it, matching preflight's `validate_br_on`).
      emit_br_on(
        fb,
        module,
        seq_id,
        r#type.as_str(),
        label,
        ref_type,
        to_ref_type,
        label_stack,
      )?;
    }
    // GC struct instructions (C7a). Delegated to `emit_struct` (an
    // `#[inline(never)]` helper) SO THAT the six arms' locals do NOT inflate this
    // recursive walker's frame — the same stack-frame discipline as `emit_atomic`
    // (see `MAX_NESTING_DEPTH`). Each resolves its `TypeId` via `resolve_type_id`
    // (the abort guard) exactly like `CallIndirect`.
    "StructNew" | "StructNewDefault" | "StructGet" | "StructGetS" | "StructGetU" | "StructSet" => {
      // Pass `type_index`/`field` RAW: `emit_struct` narrows each INSIDE the arm
      // that consumes it, so `StructNew`/`StructNewDefault` never
      // `checked_index` an irrelevant `field` (which preflight's
      // `validate_struct` also ignores for those two).
      emit_struct(fb, module, seq_id, r#type.as_str(), type_index, field)?;
    }
    // GC array instructions (C7a). Delegated to `emit_array` (an
    // `#[inline(never)]` helper) for the SAME frame-size reason. Each resolves its
    // `TypeId`(s) via `resolve_type_id`; the data/elem ops also resolve a
    // `DataId`/`ElementId` via `data_id_at`/`element_id_at` (all abort guards).
    "ArrayNew" | "ArrayNewDefault" | "ArrayNewFixed" | "ArrayNewData" | "ArrayNewElem"
    | "ArrayGet" | "ArrayGetS" | "ArrayGetU" | "ArraySet" | "ArrayLen" | "ArrayFill"
    | "ArrayCopy" | "ArrayInitData" | "ArrayInitElem" => {
      // Pass every index field RAW: `emit_array` narrows each INSIDE the arm(s)
      // that consume it, so e.g. the fieldless `ArrayLen` never `checked_index`es
      // an irrelevant `type_index`/`data`/`elem`, and only `ArrayCopy` checks
      // `src_type_index` (matching preflight's `validate_array` arm-for-arm).
      emit_array(
        fb,
        module,
        seq_id,
        r#type.as_str(),
        type_index,
        src_type_index,
        len,
        data,
        elem,
      )?;
    }
    // Wide-arithmetic instructions (C9) — the four fieldless leaves of the
    // wide-arithmetic proposal. Delegated to `emit_wide_arith` (an
    // `#[inline(never)]` helper) for the SAME frame-size reason as `emit_gc_ref`:
    // even bare, fieldless arms added in bulk INLINE to this recursive walker were
    // MEASURED (C7b) to erode the at-cap headroom, so their (empty) locals live in
    // the helper's OWN frame. All four are fieldless — no payload, no resolver.
    "I64Add128" | "I64Sub128" | "I64MulWideS" | "I64MulWideU" => {
      emit_wide_arith(fb, seq_id, r#type.as_str())?;
    }
    other => {
      return Err(Error::from_reason(format!(
        "unknown or unsupported instruction type `{other}` (buildFunction handles only the \
         C1a/C1b/C2/C3/C4/C5 core, control-flow, numeric-operator, memory/load-store, \
         atomic, table, reference/tail-call, GC struct/array/reference/branch, and modern \
         exception-handling subset)"
      )));
    }
  }
  Ok(())
}

/// The error for a descriptor missing a payload field its `type` requires.
fn missing(ty: &str, field: &str) -> Error {
  Error::from_reason(format!("`{ty}` instruction is missing its `{field}` field"))
}

/// The catchable error for a catch clause `kind` that names no `TryTableCatch`
/// variant. Shared by emit ([`try_table_catches_to_walrus`]) and preflight
/// ([`validate_try_table_catches`]) so both reject an unknown kind identically.
fn unknown_catch_kind(kind: &str) -> Error {
  Error::from_reason(format!(
    "unknown catch clause kind `{kind}` (expected `Catch`, `CatchRef`, `CatchAll`, or \
     `CatchAllRef`)"
  ))
}

/// The catchable error for a LEGACY catch clause `kind` that names no
/// `LegacyCatch` variant. Shared by emit ([`legacy_catches_to_walrus`]) and
/// preflight ([`validate_legacy_catches`]) so both reject an unknown kind
/// identically. Distinct from [`unknown_catch_kind`]: the legacy `Try` clause
/// kinds are `LegacyCatch`/`LegacyCatchAll`/`LegacyDelegate` (collision-free with
/// the modern `TryTable` kinds).
fn unknown_legacy_catch_kind(kind: &str) -> Error {
  Error::from_reason(format!(
    "unknown legacy catch clause kind `{kind}` (expected `LegacyCatch`, `LegacyCatchAll`, or \
     `LegacyDelegate`)"
  ))
}

/// Convert JS catch clauses into walrus `TryTableCatch`es, resolving each tag via
/// [`tag_id_at`] and each `label` via [`label_target`] against the CURRENT (outer)
/// label stack — the try_table's own seq is deliberately NOT on the stack yet, so
/// depth `0` names the block ENCLOSING the try_table (walrus computes these branch
/// targets BEFORE pushing the try_table block; see `emit.rs`). Split out of
/// [`emit_one`]'s `TryTable` arm and marked `#[inline(never)]` so the per-clause
/// locals stay OFF the recursive walker's frame (see [`MAX_NESTING_DEPTH`]).
/// Mirrors [`validate_try_table_catches`]'s kind/required-field checks exactly.
#[inline(never)]
fn try_table_catches_to_walrus(
  module: &Module,
  catches: &[CatchClause],
  label_stack: &[wir::InstrSeqId],
) -> Result<Vec<wir::TryTableCatch>> {
  let mut out = Vec::with_capacity(catches.len());
  for c in catches {
    let clause = match c.kind.as_str() {
      "Catch" => wir::TryTableCatch::Catch {
        tag: tag_id_at(
          module,
          checked_index(c.tag.ok_or_else(|| missing("Catch", "tag"))?, "tag")?,
        )?,
        label: label_target(
          checked_index(c.label.ok_or_else(|| missing("Catch", "label"))?, "label")?,
          label_stack,
        )?,
      },
      "CatchRef" => wir::TryTableCatch::CatchRef {
        tag: tag_id_at(
          module,
          checked_index(c.tag.ok_or_else(|| missing("CatchRef", "tag"))?, "tag")?,
        )?,
        label: label_target(
          checked_index(
            c.label.ok_or_else(|| missing("CatchRef", "label"))?,
            "label",
          )?,
          label_stack,
        )?,
      },
      "CatchAll" => wir::TryTableCatch::CatchAll {
        label: label_target(
          checked_index(
            c.label.ok_or_else(|| missing("CatchAll", "label"))?,
            "label",
          )?,
          label_stack,
        )?,
      },
      "CatchAllRef" => wir::TryTableCatch::CatchAllRef {
        label: label_target(
          checked_index(
            c.label.ok_or_else(|| missing("CatchAllRef", "label"))?,
            "label",
          )?,
          label_stack,
        )?,
      },
      other => return Err(unknown_catch_kind(other)),
    };
    out.push(clause);
  }
  Ok(out)
}

/// Emit one leaf exception-handling instruction (`Throw`/`ThrowRef`). Split out of
/// [`emit_one`] and marked `#[inline(never)]` for the same frame-size reason as
/// [`emit_gc_ref`] (keep its locals out of the recursive walker's frame). `Throw`
/// resolves its `tag` via [`tag_id_at`] (the abort guard); `ThrowRef` is
/// fieldless. The caller only routes those two discriminants here.
#[inline(never)]
fn emit_eh(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  ty: &str,
  // RAW carrier: narrowed via `checked_index` ONLY in the `Throw` arm — the
  // fieldless `ThrowRef` never touches it, so an irrelevant poisoned `tag` is
  // ignored by BOTH emit and preflight (`validate_eh`).
  tag: Option<f64>,
) -> Result<()> {
  match ty {
    "Throw" => {
      let id = tag_id_at(
        module,
        checked_index(tag.ok_or_else(|| missing("Throw", "tag"))?, "tag")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::Throw { tag: id });
    }
    "ThrowRef" => {
      fb.instr_seq(seq_id).instr(wir::ThrowRef {});
    }
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not an EH leaf op"
      )))
    }
  }
  Ok(())
}

/// Emit a legacy `Try` (C8b) into `seq_id`. Split out of [`emit_one`]'s `Try` arm
/// and marked `#[inline(never)]` so the try machinery's locals (the body seq, the
/// converted catch list) live in THIS frame, NOT the recursive `emit_one` frame —
/// the same stack-frame discipline that keeps the at-cap canary green as the
/// instruction set grows (see [`MAX_NESTING_DEPTH`]).
///
/// The try body is a child `InstrSeq` built via `dangling_instr_seq` + [`emit_desc`]
/// (which pushes it, so `br 0` inside the body targets the try block). Each
/// `LegacyCatch`/`LegacyCatchAll` handler is built the SAME way inside
/// [`legacy_catches_to_walrus`] — its own dangling seq at the SAME label depth as
/// the body (a SIBLING, like an `IfElse` arm; §A.2), NOT nested under it. There is
/// no outer-scope clause-label resolution (unlike `TryTable`): legacy handlers are
/// child sequences, and `Delegate`'s `relativeDepth` is a raw pass-through, so the
/// body/handler emit order is immaterial. Body first, then handlers, matches
/// walrus' own traversal + our read order.
#[inline(never)]
fn emit_try(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  block_type: Option<BlockType>,
  seq: Option<Vec<InstrDesc>>,
  catches: Option<Vec<CatchClause>>,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<()> {
  let ty = to_instr_seq_type(module, block_type)?;
  let child = fb.dangling_instr_seq(ty).id();
  emit_desc(fb, module, child, seq.unwrap_or_default(), label_stack)?;
  let catches = legacy_catches_to_walrus(fb, module, catches.unwrap_or_default(), label_stack)?;
  fb.instr_seq(seq_id).instr(wir::Try {
    seq: child,
    catches,
  });
  Ok(())
}

/// Convert JS legacy catch clauses into walrus `LegacyCatch`es, BUILDING + emitting
/// each handler body as its own child `InstrSeq` (a SIBLING of the try body — §A.2)
/// via `dangling_instr_seq(clause.blockType)` + [`emit_desc`]. Split out of
/// [`emit_try`] and marked `#[inline(never)]` so the per-clause locals stay OFF the
/// recursive walker's frame (see [`MAX_NESTING_DEPTH`]). Takes `catches` BY VALUE so
/// each clause's handler-body `seq` can be MOVED into `emit_desc`.
///
/// `LegacyCatch` resolves its `tag` via [`tag_id_at`] (the abort guard) and requires
/// its handler `seq`; `LegacyCatchAll` requires its handler `seq`; `LegacyDelegate`
/// passes its `relativeDepth` through VERBATIM (walrus never resolves it — no
/// `label_target`, no abort surface — §A.4). An unknown kind / missing required
/// field is a catchable error. Mirrors [`validate_legacy_catches`] arm-for-arm.
#[inline(never)]
fn legacy_catches_to_walrus(
  fb: &mut FunctionBuilder,
  module: &Module,
  catches: Vec<CatchClause>,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<Vec<wir::LegacyCatch>> {
  let mut out = Vec::with_capacity(catches.len());
  for c in catches {
    let clause = match c.kind.as_str() {
      "LegacyCatch" => {
        let tag = tag_id_at(
          module,
          checked_index(c.tag.ok_or_else(|| missing("LegacyCatch", "tag"))?, "tag")?,
        )?;
        let ty = to_instr_seq_type(module, c.block_type)?;
        let handler = fb.dangling_instr_seq(ty).id();
        emit_desc(
          fb,
          module,
          handler,
          c.seq.ok_or_else(|| missing("LegacyCatch", "seq"))?,
          label_stack,
        )?;
        wir::LegacyCatch::Catch { tag, handler }
      }
      "LegacyCatchAll" => {
        let ty = to_instr_seq_type(module, c.block_type)?;
        let handler = fb.dangling_instr_seq(ty).id();
        emit_desc(
          fb,
          module,
          handler,
          c.seq.ok_or_else(|| missing("LegacyCatchAll", "seq"))?,
          label_stack,
        )?;
        wir::LegacyCatch::CatchAll { handler }
      }
      "LegacyDelegate" => wir::LegacyCatch::Delegate {
        relative_depth: checked_index(
          c.relative_depth
            .ok_or_else(|| missing("LegacyDelegate", "relativeDepth"))?,
          "relativeDepth",
        )?,
      },
      other => return Err(unknown_legacy_catch_kind(other)),
    };
    out.push(clause);
  }
  Ok(out)
}

/// Emit one atomic (threads) instruction. Split out of [`emit_one`] and marked
/// `#[inline(never)]` so its locals live in their OWN frame rather than bloating
/// the recursive `emit_one` frame (see the call site) — this preserves the
/// deep-nesting stack headroom. `ty` is one of the five atomic discriminants; the
/// caller guarantees that, so the final arm is `unreachable!`. Each memory-bearing
/// atomic resolves its `memory` (the abort guard) and converts its `MemArg`
/// (offset losslessness), exactly like Load/Store; the `AtomicOp`/`AtomicWidth`/
/// `sixtyFour` immediates are plain values needing no resolution.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn emit_atomic(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  ty: &str,
  // RAW carrier: narrowed via `checked_index` ONLY in the four memory-bearing
  // arms — the fieldless `AtomicFence` never touches it, so an irrelevant
  // poisoned `memory` is ignored by BOTH emit and preflight (`validate_atomic`).
  memory: Option<f64>,
  atomic_op: Option<AtomicOp>,
  atomic_width: Option<AtomicWidth>,
  mem_arg: Option<MemArg>,
  sixty_four: Option<bool>,
) -> Result<()> {
  match ty {
    "AtomicRmw" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("AtomicRmw", "memory"))?,
          "memory",
        )?,
      )?;
      let op = atomic_op_to_walrus(&atomic_op.ok_or_else(|| missing("AtomicRmw", "atomicOp"))?);
      let width =
        atomic_width_to_walrus(&atomic_width.ok_or_else(|| missing("AtomicRmw", "atomicWidth"))?);
      let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("AtomicRmw", "memArg"))?)?;
      fb.instr_seq(seq_id).instr(wir::AtomicRmw {
        memory,
        op,
        width,
        arg,
      });
    }
    "Cmpxchg" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("Cmpxchg", "memory"))?,
          "memory",
        )?,
      )?;
      let width =
        atomic_width_to_walrus(&atomic_width.ok_or_else(|| missing("Cmpxchg", "atomicWidth"))?);
      let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("Cmpxchg", "memArg"))?)?;
      fb.instr_seq(seq_id)
        .instr(wir::Cmpxchg { memory, width, arg });
    }
    "AtomicNotify" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("AtomicNotify", "memory"))?,
          "memory",
        )?,
      )?;
      let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("AtomicNotify", "memArg"))?)?;
      fb.instr_seq(seq_id)
        .instr(wir::AtomicNotify { memory, arg });
    }
    "AtomicWait" => {
      let memory = memory_id_at(
        module,
        checked_index(
          memory.ok_or_else(|| missing("AtomicWait", "memory"))?,
          "memory",
        )?,
      )?;
      let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("AtomicWait", "memArg"))?)?;
      let sixty_four = sixty_four.ok_or_else(|| missing("AtomicWait", "sixtyFour"))?;
      fb.instr_seq(seq_id).instr(wir::AtomicWait {
        memory,
        arg,
        sixty_four,
      });
    }
    "AtomicFence" => {
      fb.instr_seq(seq_id).instr(wir::AtomicFence {});
    }
    // Unreachable: `emit_one` only routes the five atomic discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not an atomic instruction"
      )))
    }
  }
  Ok(())
}

/// Emit one `I8x16Shuffle`. Split out of [`emit_one`] and marked
/// `#[inline(never)]` for the same frame-size reason as [`emit_atomic`]: the
/// `[u8; 16]` immediate lives in this function's OWN frame, not the recursive
/// `emit_one` frame. Requires the `shuffleIndices` field present and EXACTLY 16
/// bytes (a representation constraint, not a wasm semantic check).
#[inline(never)]
fn emit_shuffle(
  fb: &mut FunctionBuilder,
  seq_id: wir::InstrSeqId,
  shuffle_indices: Option<Uint8Array>,
) -> Result<()> {
  let bytes = shuffle_indices.ok_or_else(|| missing("I8x16Shuffle", "shuffleIndices"))?;
  let indices = to_shuffle_indices(&bytes)?;
  fb.instr_seq(seq_id).instr(wir::I8x16Shuffle { indices });
  Ok(())
}

/// Emit one `LoadSimd`. Split out of [`emit_one`] and marked `#[inline(never)]`
/// for the same frame-size reason as [`emit_atomic`]: the resolved `MemoryId`,
/// the converted `wir::LoadSimdKind`, and the `wir::MemArg` live in this
/// function's OWN frame, not the recursive `emit_one` frame. Resolves `memory`
/// (the abort guard), converts the `MemArg` (offset losslessness) and the
/// `LoadSimdKind` (a plain value — no id to resolve; the lane variants' `lane`
/// is narrowed losslessly by `checked_lane`, so an out-of-domain lane throws
/// catchably). MIRROR-WALRUS: the (in-range) lane index and the alignment are
/// still emitted verbatim — no wasm semantic check against the vector's lane
/// count.
#[inline(never)]
fn emit_load_simd(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  // RAW carrier: narrowed via `checked_index` inline (LoadSimd always consumes
  // it, matching preflight).
  memory: Option<f64>,
  load_simd_kind: Option<LoadSimdKind>,
  mem_arg: Option<MemArg>,
) -> Result<()> {
  let memory = memory_id_at(
    module,
    checked_index(
      memory.ok_or_else(|| missing("LoadSimd", "memory"))?,
      "memory",
    )?,
  )?;
  let kind =
    load_simd_kind_to_walrus(&load_simd_kind.ok_or_else(|| missing("LoadSimd", "loadSimdKind"))?)?;
  let arg = mem_arg_to_walrus(&mem_arg.ok_or_else(|| missing("LoadSimd", "memArg"))?)?;
  fb.instr_seq(seq_id)
    .instr(wir::LoadSimd { memory, kind, arg });
  Ok(())
}

/// Emit one GC struct instruction (C7a). Split out of [`emit_one`] and marked
/// `#[inline(never)]` for the same frame-size reason as [`emit_atomic`]: the six
/// arms' locals live in this function's OWN frame, not the recursive `emit_one`
/// frame. Every arm resolves its `TypeId` through [`resolve_type_id`] (which
/// rejects a nonexistent AND an internal function-entry-type index — a
/// foreign/deleted/entry `TypeId` reaching emit panics walrus' `get_type_index`
/// into an uncatchable abort). The `field` immediate of the get/set ops is a
/// plain value (MIRROR-WALRUS: not range-checked). The caller only routes the six
/// struct discriminants here, so the final arm is unreachable.
#[inline(never)]
fn emit_struct(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  ty_str: &str,
  // RAW carriers: each narrowed via `checked_index` ONLY in the arm that
  // consumes it. `type_index` is consumed by all six; `field` ONLY by the
  // get/set ops — so `StructNew`/`StructNewDefault` never touch an irrelevant
  // `field` (which preflight's `validate_struct` also ignores for those two).
  type_index: Option<f64>,
  field: Option<f64>,
) -> Result<()> {
  match ty_str {
    "StructNew" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("StructNew", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::StructNew { ty });
    }
    "StructNewDefault" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("StructNewDefault", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::StructNewDefault { ty });
    }
    "StructGet" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("StructGet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let field = checked_index(field.ok_or_else(|| missing("StructGet", "field"))?, "field")?;
      fb.instr_seq(seq_id).instr(wir::StructGet { ty, field });
    }
    "StructGetS" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("StructGetS", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let field = checked_index(
        field.ok_or_else(|| missing("StructGetS", "field"))?,
        "field",
      )?;
      fb.instr_seq(seq_id).instr(wir::StructGetS { ty, field });
    }
    "StructGetU" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("StructGetU", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let field = checked_index(
        field.ok_or_else(|| missing("StructGetU", "field"))?,
        "field",
      )?;
      fb.instr_seq(seq_id).instr(wir::StructGetU { ty, field });
    }
    "StructSet" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("StructSet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let field = checked_index(field.ok_or_else(|| missing("StructSet", "field"))?, "field")?;
      fb.instr_seq(seq_id).instr(wir::StructSet { ty, field });
    }
    // Unreachable: `emit_one` only routes the six struct discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a struct instruction"
      )))
    }
  }
  Ok(())
}

/// Emit one GC array instruction (C7a). Split out of [`emit_one`] and marked
/// `#[inline(never)]` for the same frame-size reason as [`emit_atomic`]. Every
/// type-bearing arm resolves its `TypeId` through [`resolve_type_id`] (the abort
/// guard); `ArrayCopy` resolves BOTH `dst_ty` (`type_index`) and `src_ty`
/// (`src_type_index`); the data/elem ops additionally resolve a `DataId`/
/// `ElementId` via [`data_id_at`]/[`element_id_at`]. `ArrayNewFixed`'s `len` is a
/// plain immediate (never an arena index) but is still narrowed LOSSLESSLY via
/// [`checked_index`] so an out-of-domain JS number is a catchable error, not a
/// silent u32 alias; `ArrayLen` is fieldless. The caller only routes the fourteen
/// array discriminants here, so the final arm is unreachable.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn emit_array(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  ty_str: &str,
  // RAW carriers: each narrowed via `checked_index` ONLY in the arm(s) that
  // consume it — `type_index` in every type-bearing arm (NOT the fieldless
  // `ArrayLen`), `src_type_index` ONLY in `ArrayCopy`, `data` ONLY in the
  // data ops, `elem` ONLY in the elem ops — so an irrelevant poisoned index
  // (e.g. `ArrayLen.typeIndex`, `ArrayGet.data`) is ignored by BOTH emit and
  // preflight (`validate_array`). `len` is a plain immediate (never an arena
  // index) but is likewise carried as `f64` and narrowed via `checked_index` in
  // the `ArrayNewFixed` arm (lossless: reject NaN/fraction/negative/>u32::MAX).
  type_index: Option<f64>,
  src_type_index: Option<f64>,
  len: Option<f64>,
  data: Option<f64>,
  elem: Option<f64>,
) -> Result<()> {
  match ty_str {
    "ArrayNew" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayNew", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayNew { ty });
    }
    "ArrayNewDefault" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayNewDefault", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayNewDefault { ty });
    }
    "ArrayNewFixed" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayNewFixed", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let len = checked_index(len.ok_or_else(|| missing("ArrayNewFixed", "len"))?, "len")?;
      fb.instr_seq(seq_id).instr(wir::ArrayNewFixed { ty, len });
    }
    "ArrayNewData" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayNewData", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let data = data_id_at(
        module,
        checked_index(data.ok_or_else(|| missing("ArrayNewData", "data"))?, "data")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayNewData { ty, data });
    }
    "ArrayNewElem" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayNewElem", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let elem = element_id_at(
        module,
        checked_index(elem.ok_or_else(|| missing("ArrayNewElem", "elem"))?, "elem")?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayNewElem { ty, elem });
    }
    "ArrayGet" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayGet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayGet { ty });
    }
    "ArrayGetS" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayGetS", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayGetS { ty });
    }
    "ArrayGetU" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayGetU", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayGetU { ty });
    }
    "ArraySet" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArraySet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArraySet { ty });
    }
    "ArrayLen" => {
      fb.instr_seq(seq_id).instr(wir::ArrayLen {});
    }
    "ArrayFill" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayFill", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayFill { ty });
    }
    "ArrayCopy" => {
      // `type_index` is the DESTINATION array type, `src_type_index` the SOURCE
      // (see `InstrDesc`). Resolve dst first, then src, so a preflight error names
      // the same field emit would fail on.
      let dst_ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayCopy", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let src_ty = resolve_type_id(
        module,
        checked_index(
          src_type_index.ok_or_else(|| missing("ArrayCopy", "srcTypeIndex"))?,
          "srcTypeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id)
        .instr(wir::ArrayCopy { dst_ty, src_ty });
    }
    "ArrayInitData" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayInitData", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let data = data_id_at(
        module,
        checked_index(
          data.ok_or_else(|| missing("ArrayInitData", "data"))?,
          "data",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayInitData { ty, data });
    }
    "ArrayInitElem" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ArrayInitElem", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      let elem = element_id_at(
        module,
        checked_index(
          elem.ok_or_else(|| missing("ArrayInitElem", "elem"))?,
          "elem",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ArrayInitElem { ty, elem });
    }
    // Unreachable: `emit_one` only routes the fourteen array discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not an array instruction"
      )))
    }
  }
  Ok(())
}

/// Emit one label-free GC reference instruction (C7b). Split out of [`emit_one`]
/// and marked `#[inline(never)]` for the same frame-size reason as
/// [`emit_atomic`]: the eleven arms' locals and call temporaries live in this
/// function's OWN frame, not the recursive `emit_one` frame (inlining them was
/// MEASURED to push the -O0 at-cap round-trip into a SIGSEGV).
///
/// `CallRef`/`ReturnCallRef` resolve their callee type through
/// [`resolve_type_id`] (the abort guard; rejects a nonexistent AND an internal
/// function-entry-type index) exactly like `CallIndirect` — MIRROR-WALRUS: that
/// the type is actually a FUNCTION type is NOT checked. `RefTest`/`RefCast` are
/// EXACT payload twins of `RefNull`: they REUSE the `refType` field (walrus
/// stores the pair as two fields, `nullable` + `heap_type`, rather than a
/// bundled `RefType`) and route the heap through the MODULE-AWARE
/// [`heap_type_to_walrus_in`] — the abort guard for a concrete/exact heap naming
/// a foreign/deleted/entry `TypeId`; whether the tested/cast type is legal for
/// the operand is NOT checked. The seven remaining ops are fieldless. The caller
/// only routes the eleven GC reference discriminants here, so the final arm is
/// unreachable.
#[inline(never)]
fn emit_gc_ref(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  ty_str: &str,
  // RAW carrier: narrowed via `checked_index` ONLY in the `CallRef`/
  // `ReturnCallRef` arms — the nine fieldless / `refType`-only ops never touch
  // it, so an irrelevant poisoned `type_index` is ignored by BOTH emit and
  // preflight (`validate_gc_ref`).
  type_index: Option<f64>,
  ref_type: Option<RefType>,
) -> Result<()> {
  match ty_str {
    "RefAsNonNull" => {
      fb.instr_seq(seq_id).instr(wir::RefAsNonNull {});
    }
    "CallRef" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("CallRef", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::CallRef { ty });
    }
    "ReturnCallRef" => {
      let ty = resolve_type_id(
        module,
        checked_index(
          type_index.ok_or_else(|| missing("ReturnCallRef", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      fb.instr_seq(seq_id).instr(wir::ReturnCallRef { ty });
    }
    "RefI31" => {
      fb.instr_seq(seq_id).instr(wir::RefI31 {});
    }
    "I31GetS" => {
      fb.instr_seq(seq_id).instr(wir::I31GetS {});
    }
    "I31GetU" => {
      fb.instr_seq(seq_id).instr(wir::I31GetU {});
    }
    "RefTest" => {
      let rt = ref_type.ok_or_else(|| missing("RefTest", "refType"))?;
      let heap_type = heap_type_to_walrus_in(module, rt.heap)?;
      fb.instr_seq(seq_id).instr(wir::RefTest {
        nullable: rt.nullable,
        heap_type,
      });
    }
    "RefCast" => {
      let rt = ref_type.ok_or_else(|| missing("RefCast", "refType"))?;
      let heap_type = heap_type_to_walrus_in(module, rt.heap)?;
      fb.instr_seq(seq_id).instr(wir::RefCast {
        nullable: rt.nullable,
        heap_type,
      });
    }
    "AnyConvertExtern" => {
      fb.instr_seq(seq_id).instr(wir::AnyConvertExtern {});
    }
    "ExternConvertAny" => {
      fb.instr_seq(seq_id).instr(wir::ExternConvertAny {});
    }
    "RefEq" => {
      fb.instr_seq(seq_id).instr(wir::RefEq {});
    }
    // Unreachable: `emit_one` only routes the eleven GC reference discriminants
    // here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a GC reference instruction"
      )))
    }
  }
  Ok(())
}

/// Emit one wide-arithmetic instruction (C9): the four fieldless leaves
/// `i64.add128` / `i64.sub128` / `i64.mul_wide_s` / `i64.mul_wide_u`. Split out of
/// [`emit_leaf`] and marked `#[inline(never)]` for the same frame-size reason as
/// [`emit_gc_ref`] (keep even these empty-arm locals off the recursive walker's
/// frame — see [`MAX_NESTING_DEPTH`]). All four are fieldless (no payload, no
/// resolver, so no abort hazard). The caller only routes the four wide-arithmetic
/// discriminants here, so the final arm is unreachable.
#[inline(never)]
fn emit_wide_arith(fb: &mut FunctionBuilder, seq_id: wir::InstrSeqId, ty_str: &str) -> Result<()> {
  match ty_str {
    "I64Add128" => {
      fb.instr_seq(seq_id).instr(wir::I64Add128 {});
    }
    "I64Sub128" => {
      fb.instr_seq(seq_id).instr(wir::I64Sub128 {});
    }
    "I64MulWideS" => {
      fb.instr_seq(seq_id).instr(wir::I64MulWideS {});
    }
    "I64MulWideU" => {
      fb.instr_seq(seq_id).instr(wir::I64MulWideU {});
    }
    // Unreachable: `emit_leaf` only routes the four wide-arithmetic discriminants
    // here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a wide-arithmetic instruction"
      )))
    }
  }
  Ok(())
}

/// Emit one label-carrying GC branch instruction (C7c). Split out of
/// [`emit_one`] and marked `#[inline(never)]` for the same frame-size reason as
/// [`emit_gc_ref`]: a `BrOnCast` arm is multi-statement (two required-field
/// checks, two heap conversions, a five-field struct), and even
/// individually-thin arms added in bulk INLINE to the recursive walker were
/// MEASURED (C7b) to SIGSEGV the -O0 at-cap canary — so all four arms' locals
/// live in this function's OWN frame.
///
/// All four resolve their branch `label` (relative depth, innermost = 0)
/// through [`label_target`] — the abort guard for an out-of-range depth —
/// exactly like `Br`. `BrOnCast`/`BrOnCastFail` additionally carry walrus'
/// `from_*`/`to_*` cast pair: the FROM (source/input) pair reuses `refType` and
/// the TO (target) pair uses `toRefType`, each heap routed through the
/// MODULE-AWARE [`heap_type_to_walrus_in`] (the abort guard for a
/// concrete/exact heap naming a foreign/deleted/entry `TypeId`).
/// MIRROR-WALRUS: whether the cast is a legal subtype relation (or the target
/// block's type is compatible) is NOT checked. The caller only routes the four
/// br_on_* discriminants here, so the final arm is unreachable.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn emit_br_on(
  fb: &mut FunctionBuilder,
  module: &Module,
  seq_id: wir::InstrSeqId,
  ty_str: &str,
  // RAW carrier: narrowed via `checked_index` inline in each arm — all four
  // br_on ops consume `label`, matching preflight's `validate_br_on`.
  label: Option<f64>,
  ref_type: Option<RefType>,
  to_ref_type: Option<RefType>,
  label_stack: &[wir::InstrSeqId],
) -> Result<()> {
  match ty_str {
    "BrOnNull" => {
      let block = label_target(
        checked_index(label.ok_or_else(|| missing("BrOnNull", "label"))?, "label")?,
        label_stack,
      )?;
      fb.instr_seq(seq_id).instr(wir::BrOnNull { block });
    }
    "BrOnNonNull" => {
      let block = label_target(
        checked_index(
          label.ok_or_else(|| missing("BrOnNonNull", "label"))?,
          "label",
        )?,
        label_stack,
      )?;
      fb.instr_seq(seq_id).instr(wir::BrOnNonNull { block });
    }
    "BrOnCast" => {
      let block = label_target(
        checked_index(label.ok_or_else(|| missing("BrOnCast", "label"))?, "label")?,
        label_stack,
      )?;
      let from = ref_type.ok_or_else(|| missing("BrOnCast", "refType"))?;
      let to = to_ref_type.ok_or_else(|| missing("BrOnCast", "toRefType"))?;
      let from_heap_type = heap_type_to_walrus_in(module, from.heap)?;
      let to_heap_type = heap_type_to_walrus_in(module, to.heap)?;
      fb.instr_seq(seq_id).instr(wir::BrOnCast {
        block,
        from_nullable: from.nullable,
        from_heap_type,
        to_nullable: to.nullable,
        to_heap_type,
      });
    }
    "BrOnCastFail" => {
      let block = label_target(
        checked_index(
          label.ok_or_else(|| missing("BrOnCastFail", "label"))?,
          "label",
        )?,
        label_stack,
      )?;
      let from = ref_type.ok_or_else(|| missing("BrOnCastFail", "refType"))?;
      let to = to_ref_type.ok_or_else(|| missing("BrOnCastFail", "toRefType"))?;
      let from_heap_type = heap_type_to_walrus_in(module, from.heap)?;
      let to_heap_type = heap_type_to_walrus_in(module, to.heap)?;
      fb.instr_seq(seq_id).instr(wir::BrOnCastFail {
        block,
        from_nullable: from.nullable,
        from_heap_type,
        to_nullable: to.nullable,
        to_heap_type,
      });
    }
    // Unreachable: `emit_one` only routes the four GC branch discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a GC branch instruction"
      )))
    }
  }
  Ok(())
}

// ---------------------------------------------------------------------------
// Preflight: a read-only mirror of the emit walk, run BEFORE any arena mutation.
// ---------------------------------------------------------------------------

/// Validate a descriptor array against the PRE-CALL module, without mutating
/// anything. This is a structural mirror of [`emit_desc`]/[`emit_one`] that
/// REUSES the exact same leaf resolvers (`local_id_at`/`global_id_at`/
/// `function_id_at`/`resolve_type_id`/`val_type_to_walrus_in`), so preflight and
/// emit can never disagree about which bodies are accepted.
///
/// `build_function` runs this against `&self.inner` BEFORE `FunctionBuilder::new`
/// inserts the function's signature and entry types into the arena. Two things
/// follow: a body can never resolve its own not-yet-created signature/entry type
/// index (that index is simply out of range against the pre-call arena, so it is
/// rejected catchably here instead of aborting the process at emit under
/// `panic = abort`), and because preflight completes before the first mutation,
/// ANY error leaves the module completely unchanged (no orphan entry/sig type is
/// left behind on a late failure).
///
/// `label_len` models `label_stack.len()` at the current scope. The top-level
/// body is validated at `label_len == 1` (emit pushes the entry sequence first,
/// making the stack length 1 there); each nested `block`/`loop` body and each
/// `if`/`else` arm adds exactly one frame — identical to how emit grows the label
/// stack — so a branch depth resolves in preflight exactly as it would in emit.
pub(crate) fn validate_body(module: &Module, body: &[InstrDesc], label_len: usize) -> Result<()> {
  // Cap nesting BEFORE descending (see `MAX_NESTING_DEPTH`). `label_len` IS this
  // body's depth (top-level = 1, +1 per nested level), so it is the value emit
  // will reach as `label_stack.len()`; rejecting `> MAX_NESTING_DEPTH` here keeps
  // the preflight symmetric with `emit_desc`'s guard, so a body that passes
  // preflight is guaranteed to emit within the cap.
  if label_len > MAX_NESTING_DEPTH {
    return Err(nesting_too_deep());
  }
  for d in body {
    validate_one(module, d, label_len)?;
  }
  Ok(())
}

/// Validate a single descriptor. Mirrors [`emit_one`] arm-for-arm, including the
/// same missing-field and unknown-variant errors, so preflight rejects exactly
/// the input emit would.
fn validate_one(module: &Module, d: &InstrDesc, label_len: usize) -> Result<()> {
  match d.r#type.as_str() {
    // Fieldless leaf ops: nothing to resolve. `RefIsNull` joins them (its emit
    // arm takes no payload); so do the fieldless SIMD `V128Bitselect` /
    // `I8x16Swizzle`. (The seven fieldless C7b GC reference ops are routed to
    // `validate_gc_ref` below instead, mirroring the emit dispatch exactly.)
    "Unreachable" | "Return" | "Drop" | "RefIsNull" | "V128Bitselect" | "I8x16Swizzle" => {}
    "Const" => {
      // Mirror emit's fallible Const checks: an i64 that is not lossless, and a
      // v128 that is not exactly 16 bytes (via the shared `v128_bytes_to_u128`).
      let value = d.value.as_ref().ok_or_else(|| missing("Const", "value"))?;
      match value {
        ConstValue::I64 { value } => {
          if !value.get_i64().1 {
            return Err(Error::from_reason(
              "i64 const value does not fit losslessly in a signed 64-bit integer",
            ));
          }
        }
        ConstValue::V128 { value } => {
          v128_bytes_to_u128(value)?;
        }
        ConstValue::I32 { .. } | ConstValue::F32 { .. } | ConstValue::F64 { .. } => {}
      }
    }
    "LocalGet" => {
      local_id_at(
        module,
        checked_index(
          d.local.ok_or_else(|| missing("LocalGet", "local"))?,
          "local",
        )?,
      )?;
    }
    "LocalSet" => {
      local_id_at(
        module,
        checked_index(
          d.local.ok_or_else(|| missing("LocalSet", "local"))?,
          "local",
        )?,
      )?;
    }
    "LocalTee" => {
      local_id_at(
        module,
        checked_index(
          d.local.ok_or_else(|| missing("LocalTee", "local"))?,
          "local",
        )?,
      )?;
    }
    "GlobalGet" => {
      global_id_at(
        module,
        checked_index(
          d.global.ok_or_else(|| missing("GlobalGet", "global"))?,
          "global",
        )?,
      )?;
    }
    "GlobalSet" => {
      global_id_at(
        module,
        checked_index(
          d.global.ok_or_else(|| missing("GlobalSet", "global"))?,
          "global",
        )?,
      )?;
    }
    "Call" => {
      function_id_at(
        module,
        checked_index(d.func.ok_or_else(|| missing("Call", "func"))?, "func")?,
      )?;
    }
    "Select" => {
      if let Some(vt) = d.select_type.as_ref() {
        val_type_to_walrus_in(module, vt.clone())?;
      }
    }
    "Block" | "Loop" => {
      validate_block_type(module, &d.block_type)?;
      validate_body(module, d.seq.as_deref().unwrap_or(&[]), label_len + 1)?;
    }
    "IfElse" => {
      // Each arm is its own label frame (see the module-level note), exactly like
      // emit — so recurse into both at `label_len + 1`.
      validate_block_type(module, &d.block_type)?;
      validate_body(
        module,
        d.consequent.as_deref().unwrap_or(&[]),
        label_len + 1,
      )?;
      validate_body(
        module,
        d.alternative.as_deref().unwrap_or(&[]),
        label_len + 1,
      )?;
    }
    // Modern exception handling (C8a). `TryTable` mirrors emit: validate the block
    // type, then EVERY clause against the OUTER `label_len` (NO `+1` — the clause
    // labels resolve against the scope ENCLOSING the try_table, the load-bearing
    // rule), then the body at `label_len + 1` (one frame deeper, like `Block`).
    // The clause checks live in `validate_try_table_catches` (`#[inline(never)]`),
    // keeping this recursive arm slim.
    "TryTable" => {
      validate_block_type(module, &d.block_type)?;
      validate_try_table_catches(module, d.catches.as_deref().unwrap_or(&[]), label_len)?;
      validate_body(module, d.seq.as_deref().unwrap_or(&[]), label_len + 1)?;
    }
    // Legacy exception handling (C8b). `Try` mirrors [`emit_try`]: validate the try
    // body type + body at `label_len + 1` (one frame deeper, like `Block`), then the
    // clauses via `validate_legacy_catches` (`#[inline(never)]`) — which resolves
    // each `LegacyCatch` tag (the abort guard) + validates each handler body at
    // `label_len + 1` (handlers are SIBLINGS of the body at the SAME depth — §A.2).
    // `relativeDepth` (Delegate) needs NO preflight — it is a raw pass-through
    // walrus never resolves (cannot abort — §B).
    "Try" => {
      validate_block_type(module, &d.block_type)?;
      validate_body(module, d.seq.as_deref().unwrap_or(&[]), label_len + 1)?;
      validate_legacy_catches(module, d.catches.as_deref().unwrap_or(&[]), label_len)?;
    }
    // `Throw`/`ThrowRef` preflight via `validate_eh` (`#[inline(never)]`), mirroring
    // `emit_eh`: `Throw` resolves its `tag` via `tag_id_at` (a missing preflight
    // resolution re-opens the emit-abort / partial-mutation defect); `ThrowRef`
    // needs nothing. `Rethrow` still routes through the preflight (below) for
    // missing-field parity, though its raw `relativeDepth` cannot abort.
    "Throw" | "ThrowRef" | "Rethrow" => {
      validate_eh(module, d)?;
    }
    "Br" => {
      validate_label(
        checked_index(d.label.ok_or_else(|| missing("Br", "label"))?, "label")?,
        label_len,
      )?;
    }
    "BrIf" => {
      validate_label(
        checked_index(d.label.ok_or_else(|| missing("BrIf", "label"))?, "label")?,
        label_len,
      )?;
    }
    "BrTable" => {
      // Same field/ordering as emit: labels present, then default in range, then
      // every table entry in range.
      let labels = d
        .labels
        .as_ref()
        .ok_or_else(|| missing("BrTable", "labels"))?;
      validate_label(
        checked_index(
          d.default_label
            .ok_or_else(|| missing("BrTable", "defaultLabel"))?,
          "defaultLabel",
        )?,
        label_len,
      )?;
      for &l in labels {
        validate_label(checked_index(l, "labels")?, label_len)?;
      }
    }
    // The fallible steps for an operator are the op-string decode AND (for a lane
    // op) the presence of `lane`: `from_str` is passed `d.lane`, so a lane op
    // missing its lane is rejected here exactly as emit would, discarding the
    // decoded op. Ops carry no ids/labels, so nothing else needs resolving.
    "Binop" => {
      binop_from_str(
        d.op.as_deref().ok_or_else(|| missing("Binop", "op"))?,
        d.lane,
      )?;
    }
    "Unop" => {
      unop_from_str(
        d.op.as_deref().ok_or_else(|| missing("Unop", "op"))?,
        d.lane,
      )?;
    }
    "TernOp" => {
      ternop_from_str(
        d.op.as_deref().ok_or_else(|| missing("TernOp", "op"))?,
        d.lane,
      )?;
    }
    // `I8x16Shuffle` preflight mirrors `emit_shuffle`: `shuffleIndices` present
    // and exactly 16 bytes. Delegated to `validate_shuffle` (`#[inline(never)]`)
    // to keep its `[u8; 16]` out of this recursive walker's frame.
    "I8x16Shuffle" => {
      validate_shuffle(d)?;
    }
    // Memory + load/store. The fallible steps mirror emit exactly: required
    // fields present, each memory/data index resolves (the abort guards), and a
    // `Load`/`Store`'s MemArg offset is a lossless `u64`. The kind enums carry no
    // ids, so nothing else needs resolving.
    "MemorySize" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("MemorySize", "memory"))?,
          "memory",
        )?,
      )?;
    }
    "MemoryGrow" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("MemoryGrow", "memory"))?,
          "memory",
        )?,
      )?;
    }
    "MemoryInit" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("MemoryInit", "memory"))?,
          "memory",
        )?,
      )?;
      data_id_at(
        module,
        checked_index(d.data.ok_or_else(|| missing("MemoryInit", "data"))?, "data")?,
      )?;
    }
    "DataDrop" => {
      data_id_at(
        module,
        checked_index(d.data.ok_or_else(|| missing("DataDrop", "data"))?, "data")?,
      )?;
    }
    "MemoryCopy" => {
      // Same fields/ordering as emit: destination (`memory`) then source
      // (`srcMemory`).
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("MemoryCopy", "memory"))?,
          "memory",
        )?,
      )?;
      memory_id_at(
        module,
        checked_index(
          d.src_memory
            .ok_or_else(|| missing("MemoryCopy", "srcMemory"))?,
          "srcMemory",
        )?,
      )?;
    }
    "MemoryFill" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("MemoryFill", "memory"))?,
          "memory",
        )?,
      )?;
    }
    "Load" => {
      memory_id_at(
        module,
        checked_index(d.memory.ok_or_else(|| missing("Load", "memory"))?, "memory")?,
      )?;
      d.load_kind
        .as_ref()
        .ok_or_else(|| missing("Load", "loadKind"))?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("Load", "memArg"))?,
      )?;
    }
    "Store" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("Store", "memory"))?,
          "memory",
        )?,
      )?;
      d.store_kind
        .as_ref()
        .ok_or_else(|| missing("Store", "storeKind"))?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("Store", "memArg"))?,
      )?;
    }
    // SIMD vector load / load-lane / store-lane. Mirrors emit exactly (the abort
    // guard + the missing-field + offset-losslessness checks): `memory` resolves,
    // `loadSimdKind` is present, and the `MemArg` offset is a lossless `u64`. The
    // kind carries no ids, but a lane variant's `lane` is a VERBATIM immediate
    // walrus never range-checks — so `load_simd_kind_to_walrus` (which narrows it
    // through `checked_lane`) MUST run HERE in the preflight too, so an
    // out-of-domain lane is rejected before `FunctionBuilder::new` with the arena
    // unchanged (all-or-nothing), not only at emit. This arm's shape still matches
    // `Store`; the 20-arm kind conversion stays behind the helper.
    "LoadSimd" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("LoadSimd", "memory"))?,
          "memory",
        )?,
      )?;
      load_simd_kind_to_walrus(
        d.load_simd_kind
          .as_ref()
          .ok_or_else(|| missing("LoadSimd", "loadSimdKind"))?,
      )?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("LoadSimd", "memArg"))?,
      )?;
    }
    // Atomic (threads) instructions. Delegated to `validate_atomic` (a separate,
    // non-inlined function) for the SAME frame-size reason as `emit_atomic` (see
    // its call site): `validate_one` recurses per nesting level, so the atomic
    // arms' locals are kept out of its frame to preserve the deep-nesting headroom.
    "AtomicRmw" | "Cmpxchg" | "AtomicNotify" | "AtomicWait" | "AtomicFence" => {
      validate_atomic(module, d)?;
    }
    // Table instructions + call_indirect. The fallible steps mirror emit exactly:
    // required fields present and each table/element/type index resolves (the
    // abort guards). `TableCopy` resolves BOTH tables; `TableInit` the table AND
    // the elem; `CallIndirect` the type (via `resolve_type_id`) AND the table — a
    // missing preflight resolution re-opens the emit-abort / partial-mutation
    // defect. Nothing else needs resolving.
    "TableGet" => {
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableGet", "table"))?,
          "table",
        )?,
      )?;
    }
    "TableSet" => {
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableSet", "table"))?,
          "table",
        )?,
      )?;
    }
    "TableGrow" => {
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableGrow", "table"))?,
          "table",
        )?,
      )?;
    }
    "TableSize" => {
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableSize", "table"))?,
          "table",
        )?,
      )?;
    }
    "TableFill" => {
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableFill", "table"))?,
          "table",
        )?,
      )?;
    }
    "TableInit" => {
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableInit", "table"))?,
          "table",
        )?,
      )?;
      element_id_at(
        module,
        checked_index(d.elem.ok_or_else(|| missing("TableInit", "elem"))?, "elem")?,
      )?;
    }
    "TableCopy" => {
      // Same fields/ordering as emit: destination (`table`) then source
      // (`srcTable`).
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("TableCopy", "table"))?,
          "table",
        )?,
      )?;
      table_id_at(
        module,
        checked_index(
          d.src_table
            .ok_or_else(|| missing("TableCopy", "srcTable"))?,
          "srcTable",
        )?,
      )?;
    }
    "ElemDrop" => {
      element_id_at(
        module,
        checked_index(d.elem.ok_or_else(|| missing("ElemDrop", "elem"))?, "elem")?,
      )?;
    }
    "CallIndirect" => {
      // Same fields/ordering as emit: the callee type (via `resolve_type_id`)
      // then the table.
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("CallIndirect", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      table_id_at(
        module,
        checked_index(
          d.table.ok_or_else(|| missing("CallIndirect", "table"))?,
          "table",
        )?,
      )?;
    }
    // Reference + tail-call instructions. The fallible steps mirror emit exactly
    // (the abort guards): `RefNull`'s heap type resolves via the module-aware
    // `heap_type_to_walrus_in` (a concrete/exact `TypeId` naming a
    // foreign/deleted/entry type would otherwise abort at emit); `RefFunc` /
    // `ReturnCall` resolve their `func`; `ReturnCallIndirect` resolves its type
    // (via `resolve_type_id`) AND its table — a missing preflight resolution
    // re-opens the emit-abort / partial-mutation defect. `RefIsNull` is fieldless
    // (handled above). MIRROR-WALRUS: no ref.func-declared / null-legality /
    // tail-call-signature checks.
    "RefNull" => {
      let rt = d
        .ref_type
        .as_ref()
        .ok_or_else(|| missing("RefNull", "refType"))?;
      heap_type_to_walrus_in(module, rt.heap.clone())?;
    }
    "RefFunc" => {
      function_id_at(
        module,
        checked_index(d.func.ok_or_else(|| missing("RefFunc", "func"))?, "func")?,
      )?;
    }
    "ReturnCall" => {
      function_id_at(
        module,
        checked_index(d.func.ok_or_else(|| missing("ReturnCall", "func"))?, "func")?,
      )?;
    }
    "ReturnCallIndirect" => {
      // Same fields/ordering as emit: the callee type (via `resolve_type_id`)
      // then the table.
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ReturnCallIndirect", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      table_id_at(
        module,
        checked_index(
          d.table
            .ok_or_else(|| missing("ReturnCallIndirect", "table"))?,
          "table",
        )?,
      )?;
    }
    // GC reference instructions (C7b) — the label-free subset. Delegated to
    // `validate_gc_ref` (`#[inline(never)]`) for the SAME frame-size reason as
    // `validate_atomic` (keep its locals out of this recursive walker's frame; the
    // inline arms were measured to erode the at-cap headroom). Mirrors
    // `emit_gc_ref` arm-for-arm: `CallRef`/`ReturnCallRef` resolve their callee
    // type via `resolve_type_id` and `RefTest`/`RefCast` resolve their `refType`'s
    // heap via the module-aware `heap_type_to_walrus_in` (both abort guards) — a
    // missing preflight resolution re-opens the emit-abort / partial-mutation
    // defect. The seven fieldless ops need no check.
    "RefAsNonNull" | "CallRef" | "ReturnCallRef" | "RefI31" | "I31GetS" | "I31GetU" | "RefTest"
    | "RefCast" | "AnyConvertExtern" | "ExternConvertAny" | "RefEq" => {
      validate_gc_ref(module, d)?;
    }
    // GC branch instructions (C7c) — the label-carrying subset. Delegated to
    // `validate_br_on` (`#[inline(never)]`) for the SAME frame-size reason as
    // `validate_gc_ref` (keep its locals out of this recursive walker's frame).
    // Mirrors `emit_br_on` arm-for-arm: all four range-check their `label` via
    // `validate_label`, and `BrOnCast`/`BrOnCastFail` resolve BOTH their
    // `refType` (FROM) and `toRefType` (TO) heaps via the module-aware
    // `heap_type_to_walrus_in` (the abort guards) — a missing preflight
    // resolution re-opens the emit-abort / partial-mutation defect.
    "BrOnNull" | "BrOnNonNull" | "BrOnCast" | "BrOnCastFail" => {
      validate_br_on(module, d, label_len)?;
    }
    // GC struct/array instructions (C7a). Delegated to `validate_struct`/
    // `validate_array` (`#[inline(never)]`) for the SAME frame-size reason as
    // `validate_atomic` (keep their locals out of this recursive walker's frame).
    // Each mirrors its emit helper arm-for-arm: it resolves the SAME `TypeId`(s)/
    // `DataId`/`ElementId` (ArrayCopy resolves BOTH dst AND src; the data/elem ops
    // resolve `ty` AND `data`/`elem`) and requires the SAME fields — a missing
    // preflight resolution re-opens the emit-abort / partial-mutation defect.
    "StructNew" | "StructNewDefault" | "StructGet" | "StructGetS" | "StructGetU" | "StructSet" => {
      validate_struct(module, d)?;
    }
    "ArrayNew" | "ArrayNewDefault" | "ArrayNewFixed" | "ArrayNewData" | "ArrayNewElem"
    | "ArrayGet" | "ArrayGetS" | "ArrayGetU" | "ArraySet" | "ArrayLen" | "ArrayFill"
    | "ArrayCopy" | "ArrayInitData" | "ArrayInitElem" => {
      validate_array(module, d)?;
    }
    // Wide-arithmetic instructions (C9). Delegated to `validate_wide_arith`
    // (`#[inline(never)]`) for the SAME frame-size reason as `validate_gc_ref`
    // (keep its locals out of this recursive walker's frame). Mirrors
    // `emit_wide_arith`: all four are fieldless, so there is nothing to resolve —
    // but the arms must EXIST so preflight ACCEPTS the discriminants (an
    // unrouted `type` would be rejected here before emit ever runs).
    "I64Add128" | "I64Sub128" | "I64MulWideS" | "I64MulWideU" => {
      validate_wide_arith(d)?;
    }
    other => {
      return Err(Error::from_reason(format!(
        "unknown or unsupported instruction type `{other}` (buildFunction handles only the \
         C1a/C1b/C2/C3/C4/C5 core, control-flow, numeric-operator, memory/load-store, \
         atomic, table, reference/tail-call, GC struct/array/reference/branch, and modern \
         exception-handling subset)"
      )));
    }
  }
  Ok(())
}

/// Preflight the catch clauses of a `TryTable`. Split out of [`validate_one`] and
/// marked `#[inline(never)]` for the same frame-size reason as [`validate_atomic`]
/// (keep the per-clause locals out of the recursive walker's frame). Mirrors
/// [`try_table_catches_to_walrus`] arm-for-arm: the SAME kind/required-field
/// checks, `tag_id_at` for the tag-carrying kinds, and `validate_label` against
/// the OUTER `label_len` (NO `+1` — the load-bearing outer-scope rule) — a missing
/// preflight resolution re-opens the emit-abort / partial-mutation defect.
#[inline(never)]
fn validate_try_table_catches(
  module: &Module,
  catches: &[CatchClause],
  label_len: usize,
) -> Result<()> {
  for c in catches {
    match c.kind.as_str() {
      "Catch" => {
        tag_id_at(
          module,
          checked_index(c.tag.ok_or_else(|| missing("Catch", "tag"))?, "tag")?,
        )?;
        validate_label(
          checked_index(c.label.ok_or_else(|| missing("Catch", "label"))?, "label")?,
          label_len,
        )?;
      }
      "CatchRef" => {
        tag_id_at(
          module,
          checked_index(c.tag.ok_or_else(|| missing("CatchRef", "tag"))?, "tag")?,
        )?;
        validate_label(
          checked_index(
            c.label.ok_or_else(|| missing("CatchRef", "label"))?,
            "label",
          )?,
          label_len,
        )?;
      }
      "CatchAll" => {
        validate_label(
          checked_index(
            c.label.ok_or_else(|| missing("CatchAll", "label"))?,
            "label",
          )?,
          label_len,
        )?;
      }
      "CatchAllRef" => {
        validate_label(
          checked_index(
            c.label.ok_or_else(|| missing("CatchAllRef", "label"))?,
            "label",
          )?,
          label_len,
        )?;
      }
      other => return Err(unknown_catch_kind(other)),
    }
  }
  Ok(())
}

/// Preflight one leaf exception-handling descriptor (`Throw`/`ThrowRef`). Split
/// out of [`validate_one`] and marked `#[inline(never)]` for the same frame-size
/// reason as [`validate_atomic`]. Mirrors [`emit_eh`]: `Throw` resolves its `tag`
/// via `tag_id_at` (the abort guard); `ThrowRef` needs nothing. The caller only
/// routes those two discriminants here.
#[inline(never)]
fn validate_eh(module: &Module, d: &InstrDesc) -> Result<()> {
  match d.r#type.as_str() {
    "Throw" => {
      tag_id_at(
        module,
        checked_index(d.tag.ok_or_else(|| missing("Throw", "tag"))?, "tag")?,
      )?;
    }
    "ThrowRef" => {}
    // Legacy `Rethrow` (C8b): its `relativeDepth` is a raw pass-through walrus never
    // resolves (cannot abort), but its PRESENCE is still required so preflight
    // rejects a missing field pre-mutation exactly as `emit_leaf` does.
    "Rethrow" => {
      checked_index(
        d.relative_depth
          .ok_or_else(|| missing("Rethrow", "relativeDepth"))?,
        "relativeDepth",
      )?;
    }
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not an EH leaf op"
      )))
    }
  }
  Ok(())
}

/// Preflight the legacy catch clauses of a `Try` (C8b). Split out of
/// [`validate_one`] and marked `#[inline(never)]` for the same frame-size reason as
/// [`validate_try_table_catches`]. Mirrors [`legacy_catches_to_walrus`] arm-for-arm:
/// the SAME kind/required-field checks, `tag_id_at` for `LegacyCatch` (the abort
/// guard), and each handler body validated at `label_len + 1` (SIBLING of the try
/// body, same depth — §A.2). `LegacyDelegate`'s `relativeDepth` needs only presence
/// (raw pass-through — no resolution, no abort — §B), and its ABSENCE is rejected
/// pre-mutation exactly as emit rejects it.
#[inline(never)]
fn validate_legacy_catches(
  module: &Module,
  catches: &[CatchClause],
  label_len: usize,
) -> Result<()> {
  // walrus's legacy emit treats `LegacyDelegate` as TERMINAL (it emits the
  // `Delegate` and RETURNS — there is no handler block, so nothing re-enters the
  // clause loop) and assumes `LegacyCatchAll` is the LAST handler (its block-end
  // just emits `End` without re-entering the loop). So any clause appearing AFTER
  // a `LegacyDelegate` or `LegacyCatchAll` would be SILENTLY DROPPED at emit,
  // yielding a mis-encoded module — silent value corruption, not a hard abort.
  // Reject those two orderings here in the read-only preflight (BEFORE
  // `FunctionBuilder::new` mutates the arena), so the whole call stays
  // all-or-nothing. Modern `try_table` catches are instruction OPERANDS with no
  // such terminal/last assumption (see `validate_try_table_catches`), so no
  // matching rule belongs there.
  let mut terminal_seen = false;
  for c in catches {
    if terminal_seen {
      return Err(Error::from_reason(
        "a legacy catch clause after a `LegacyCatchAll`/`LegacyDelegate` clause is unreachable and would be silently dropped at emit",
      ));
    }
    match c.kind.as_str() {
      "LegacyCatch" => {
        tag_id_at(
          module,
          checked_index(c.tag.ok_or_else(|| missing("LegacyCatch", "tag"))?, "tag")?,
        )?;
        validate_block_type(module, &c.block_type)?;
        validate_body(
          module,
          c.seq
            .as_deref()
            .ok_or_else(|| missing("LegacyCatch", "seq"))?,
          label_len + 1,
        )?;
      }
      "LegacyCatchAll" => {
        validate_block_type(module, &c.block_type)?;
        validate_body(
          module,
          c.seq
            .as_deref()
            .ok_or_else(|| missing("LegacyCatchAll", "seq"))?,
          label_len + 1,
        )?;
        // `CatchAll` is assumed LAST by emit — any later clause is dropped.
        terminal_seen = true;
      }
      "LegacyDelegate" => {
        checked_index(
          c.relative_depth
            .ok_or_else(|| missing("LegacyDelegate", "relativeDepth"))?,
          "relativeDepth",
        )?;
        // `Delegate` ends the try — any later clause is dropped at emit.
        terminal_seen = true;
      }
      other => return Err(unknown_legacy_catch_kind(other)),
    }
  }
  Ok(())
}

/// Preflight one atomic (threads) descriptor. Split out of [`validate_one`] and
/// marked `#[inline(never)]` for the same frame-size reason as [`emit_atomic`]
/// (keep the atomic locals out of the recursive walker's frame). Mirrors
/// `emit_atomic` arm-for-arm: required fields present, each `memory` resolves (the
/// abort guard), and the `MemArg` offset is a lossless `u64`. The `AtomicOp`/
/// `AtomicWidth`/`sixtyFour` immediates are plain values (no resolution), but
/// their PRESENCE is still required — emit reads them AFTER `FunctionBuilder::new`
/// has mutated the arena, so a missing one must be rejected pre-mutation.
/// `AtomicFence` is fieldless. The caller only routes the five atomic
/// discriminants here, so the final arm is unreachable.
#[inline(never)]
fn validate_atomic(module: &Module, d: &InstrDesc) -> Result<()> {
  match d.r#type.as_str() {
    "AtomicRmw" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("AtomicRmw", "memory"))?,
          "memory",
        )?,
      )?;
      d.atomic_op
        .as_ref()
        .ok_or_else(|| missing("AtomicRmw", "atomicOp"))?;
      d.atomic_width
        .as_ref()
        .ok_or_else(|| missing("AtomicRmw", "atomicWidth"))?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("AtomicRmw", "memArg"))?,
      )?;
    }
    "Cmpxchg" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("Cmpxchg", "memory"))?,
          "memory",
        )?,
      )?;
      d.atomic_width
        .as_ref()
        .ok_or_else(|| missing("Cmpxchg", "atomicWidth"))?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("Cmpxchg", "memArg"))?,
      )?;
    }
    "AtomicNotify" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("AtomicNotify", "memory"))?,
          "memory",
        )?,
      )?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("AtomicNotify", "memArg"))?,
      )?;
    }
    "AtomicWait" => {
      memory_id_at(
        module,
        checked_index(
          d.memory.ok_or_else(|| missing("AtomicWait", "memory"))?,
          "memory",
        )?,
      )?;
      mem_arg_to_walrus(
        d.mem_arg
          .as_ref()
          .ok_or_else(|| missing("AtomicWait", "memArg"))?,
      )?;
      d.sixty_four
        .ok_or_else(|| missing("AtomicWait", "sixtyFour"))?;
    }
    "AtomicFence" => {}
    // Unreachable: `validate_one` only routes the five atomic discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not an atomic instruction"
      )))
    }
  }
  Ok(())
}

/// Preflight one GC struct descriptor (C7a). Split out of [`validate_one`] and
/// marked `#[inline(never)]` for the same frame-size reason as [`validate_atomic`]
/// (keep its locals out of the recursive walker's frame). Mirrors [`emit_struct`]
/// arm-for-arm: every arm resolves its `TypeId` via [`resolve_type_id`] (the abort
/// guard), and the get/set ops additionally require `field` present — emit reads
/// `field` AFTER `FunctionBuilder::new` has mutated the arena, so a missing one
/// must be rejected pre-mutation. The caller only routes the six struct
/// discriminants here, so the final arm is unreachable.
#[inline(never)]
fn validate_struct(module: &Module, d: &InstrDesc) -> Result<()> {
  match d.r#type.as_str() {
    "StructNew" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("StructNew", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "StructNewDefault" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("StructNewDefault", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "StructGet" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("StructGet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      checked_index(
        d.field.ok_or_else(|| missing("StructGet", "field"))?,
        "field",
      )?;
    }
    "StructGetS" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("StructGetS", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      checked_index(
        d.field.ok_or_else(|| missing("StructGetS", "field"))?,
        "field",
      )?;
    }
    "StructGetU" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("StructGetU", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      checked_index(
        d.field.ok_or_else(|| missing("StructGetU", "field"))?,
        "field",
      )?;
    }
    "StructSet" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("StructSet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      checked_index(
        d.field.ok_or_else(|| missing("StructSet", "field"))?,
        "field",
      )?;
    }
    // Unreachable: `validate_one` only routes the six struct discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a struct instruction"
      )))
    }
  }
  Ok(())
}

/// Preflight one GC array descriptor (C7a). Split out of [`validate_one`] and
/// marked `#[inline(never)]` for the same frame-size reason as [`validate_atomic`].
/// Mirrors [`emit_array`] arm-for-arm and in the SAME order: every type-bearing
/// arm resolves its `TypeId` via [`resolve_type_id`]; `ArrayCopy` resolves the
/// destination (`type_index`) THEN the source (`src_type_index`); the data/elem
/// ops resolve `ty` THEN the `DataId`/`ElementId` (via [`data_id_at`]/
/// [`element_id_at`]) — a missing preflight resolution re-opens the emit-abort /
/// partial-mutation defect. `ArrayNewFixed` requires `len` present AND narrows it
/// LOSSLESSLY via [`checked_index`] (arm-for-arm with emit, so a bad `len` is
/// rejected before any `FunctionBuilder` mutation); `ArrayLen` is fieldless. The
/// caller only routes the fourteen array discriminants here, so the final arm is
/// unreachable.
#[inline(never)]
fn validate_array(module: &Module, d: &InstrDesc) -> Result<()> {
  match d.r#type.as_str() {
    "ArrayNew" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayNew", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArrayNewDefault" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayNewDefault", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArrayNewFixed" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayNewFixed", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      checked_index(d.len.ok_or_else(|| missing("ArrayNewFixed", "len"))?, "len")?;
    }
    "ArrayNewData" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayNewData", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      data_id_at(
        module,
        checked_index(
          d.data.ok_or_else(|| missing("ArrayNewData", "data"))?,
          "data",
        )?,
      )?;
    }
    "ArrayNewElem" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayNewElem", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      element_id_at(
        module,
        checked_index(
          d.elem.ok_or_else(|| missing("ArrayNewElem", "elem"))?,
          "elem",
        )?,
      )?;
    }
    "ArrayGet" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayGet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArrayGetS" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayGetS", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArrayGetU" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayGetU", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArraySet" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArraySet", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArrayLen" => {}
    "ArrayFill" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayFill", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ArrayCopy" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayCopy", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      resolve_type_id(
        module,
        checked_index(
          d.src_type_index
            .ok_or_else(|| missing("ArrayCopy", "srcTypeIndex"))?,
          "srcTypeIndex",
        )?,
      )?;
    }
    "ArrayInitData" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayInitData", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      data_id_at(
        module,
        checked_index(
          d.data.ok_or_else(|| missing("ArrayInitData", "data"))?,
          "data",
        )?,
      )?;
    }
    "ArrayInitElem" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ArrayInitElem", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
      element_id_at(
        module,
        checked_index(
          d.elem.ok_or_else(|| missing("ArrayInitElem", "elem"))?,
          "elem",
        )?,
      )?;
    }
    // Unreachable: `validate_one` only routes the fourteen array discriminants
    // here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not an array instruction"
      )))
    }
  }
  Ok(())
}

/// Preflight one label-free GC reference descriptor (C7b). Split out of
/// [`validate_one`] and marked `#[inline(never)]` for the same frame-size reason
/// as [`validate_atomic`] (keep its locals out of the recursive walker's frame).
/// Mirrors [`emit_gc_ref`] arm-for-arm: `CallRef`/`ReturnCallRef` resolve the
/// SAME `TypeId` via [`resolve_type_id`] and `RefTest`/`RefCast` resolve the SAME
/// heap type via the module-aware [`heap_type_to_walrus_in`] (the abort guards),
/// with the SAME missing-field errors; the seven fieldless ops have nothing to
/// resolve. The caller only routes the eleven GC reference discriminants here, so
/// the final arm is unreachable.
#[inline(never)]
fn validate_gc_ref(module: &Module, d: &InstrDesc) -> Result<()> {
  match d.r#type.as_str() {
    "RefAsNonNull" | "RefI31" | "I31GetS" | "I31GetU" | "AnyConvertExtern" | "ExternConvertAny"
    | "RefEq" => {}
    "CallRef" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("CallRef", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "ReturnCallRef" => {
      resolve_type_id(
        module,
        checked_index(
          d.type_index
            .ok_or_else(|| missing("ReturnCallRef", "typeIndex"))?,
          "typeIndex",
        )?,
      )?;
    }
    "RefTest" => {
      let rt = d
        .ref_type
        .as_ref()
        .ok_or_else(|| missing("RefTest", "refType"))?;
      heap_type_to_walrus_in(module, rt.heap.clone())?;
    }
    "RefCast" => {
      let rt = d
        .ref_type
        .as_ref()
        .ok_or_else(|| missing("RefCast", "refType"))?;
      heap_type_to_walrus_in(module, rt.heap.clone())?;
    }
    // Unreachable: `validate_one` only routes the eleven GC reference
    // discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a GC reference instruction"
      )))
    }
  }
  Ok(())
}

/// Preflight one wide-arithmetic descriptor (C9). Split out of [`validate_one`]
/// and marked `#[inline(never)]` for the same frame-size reason as
/// [`validate_gc_ref`] (keep its locals out of the recursive walker's frame).
/// Mirrors [`emit_wide_arith`]: all four ops are fieldless, so there is nothing to
/// resolve and no abort hazard — the arms exist ONLY so preflight accepts the
/// discriminants. The caller only routes the four wide-arithmetic discriminants
/// here, so the final arm is unreachable.
#[inline(never)]
fn validate_wide_arith(d: &InstrDesc) -> Result<()> {
  match d.r#type.as_str() {
    "I64Add128" | "I64Sub128" | "I64MulWideS" | "I64MulWideU" => {}
    // Unreachable: `validate_one` only routes the four wide-arithmetic
    // discriminants here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a wide-arithmetic instruction"
      )))
    }
  }
  Ok(())
}

/// Preflight one label-carrying GC branch descriptor (C7c). Split out of
/// [`validate_one`] and marked `#[inline(never)]` for the same frame-size
/// reason as [`validate_gc_ref`] (keep its locals out of the recursive walker's
/// frame). Mirrors [`emit_br_on`] arm-for-arm and in the SAME check order: all
/// four range-check their `label` via [`validate_label`] (the abort guard for
/// an out-of-range depth), and `BrOnCast`/`BrOnCastFail` require BOTH `refType`
/// (FROM) and `toRefType` (TO) and resolve BOTH heaps via the module-aware
/// [`heap_type_to_walrus_in`] — a concrete/exact heap naming a
/// foreign/deleted/entry `TypeId` reaching emit panics walrus into an
/// uncatchable abort, so BOTH heaps must be preflighted, with the SAME
/// missing-field errors as emit. The caller only routes the four br_on_*
/// discriminants here, so the final arm is unreachable.
#[inline(never)]
fn validate_br_on(module: &Module, d: &InstrDesc, label_len: usize) -> Result<()> {
  match d.r#type.as_str() {
    "BrOnNull" => {
      validate_label(
        checked_index(
          d.label.ok_or_else(|| missing("BrOnNull", "label"))?,
          "label",
        )?,
        label_len,
      )?;
    }
    "BrOnNonNull" => {
      validate_label(
        checked_index(
          d.label.ok_or_else(|| missing("BrOnNonNull", "label"))?,
          "label",
        )?,
        label_len,
      )?;
    }
    "BrOnCast" => {
      validate_label(
        checked_index(
          d.label.ok_or_else(|| missing("BrOnCast", "label"))?,
          "label",
        )?,
        label_len,
      )?;
      let from = d
        .ref_type
        .as_ref()
        .ok_or_else(|| missing("BrOnCast", "refType"))?;
      let to = d
        .to_ref_type
        .as_ref()
        .ok_or_else(|| missing("BrOnCast", "toRefType"))?;
      heap_type_to_walrus_in(module, from.heap.clone())?;
      heap_type_to_walrus_in(module, to.heap.clone())?;
    }
    "BrOnCastFail" => {
      validate_label(
        checked_index(
          d.label.ok_or_else(|| missing("BrOnCastFail", "label"))?,
          "label",
        )?,
        label_len,
      )?;
      let from = d
        .ref_type
        .as_ref()
        .ok_or_else(|| missing("BrOnCastFail", "refType"))?;
      let to = d
        .to_ref_type
        .as_ref()
        .ok_or_else(|| missing("BrOnCastFail", "toRefType"))?;
      heap_type_to_walrus_in(module, from.heap.clone())?;
      heap_type_to_walrus_in(module, to.heap.clone())?;
    }
    // Unreachable: `validate_one` only routes the four GC branch discriminants
    // here.
    other => {
      return Err(Error::from_reason(format!(
        "`{other}` is not a GC branch instruction"
      )))
    }
  }
  Ok(())
}

/// Preflight one `I8x16Shuffle` descriptor. Split out of [`validate_one`] and
/// marked `#[inline(never)]` for the same frame-size reason as [`emit_shuffle`]:
/// the `[u8; 16]` immediate stays out of the recursive walker's frame. Mirrors
/// `emit_shuffle`: `shuffleIndices` present and (via the shared
/// [`to_shuffle_indices`]) exactly 16 bytes.
#[inline(never)]
fn validate_shuffle(d: &InstrDesc) -> Result<()> {
  let bytes = d
    .shuffle_indices
    .as_ref()
    .ok_or_else(|| missing("I8x16Shuffle", "shuffleIndices"))?;
  to_shuffle_indices(bytes)?;
  Ok(())
}

/// Validate a block type against the pre-call arena, mirroring
/// [`to_instr_seq_type`] and reusing its resolvers. `MultiValue` is the primary
/// abort vector: an index naming the function's own future entry/sig type is out
/// of range here and rejected catchably.
fn validate_block_type(module: &Module, bt: &Option<BlockType>) -> Result<()> {
  match bt {
    None | Some(BlockType::Empty) => {}
    Some(BlockType::Value { value }) => {
      val_type_to_walrus_in(module, value.clone())?;
    }
    Some(BlockType::MultiValue { type_index }) => {
      resolve_type_id(module, checked_index(*type_index, "typeIndex")?)?;
    }
  }
  Ok(())
}

/// Validate a relative branch label depth against the enclosing-scope count,
/// mirroring [`label_target`]'s `depth >= len` guard and its error message. Only
/// the stack LENGTH matters for range validity, which emit and preflight compute
/// identically, so no real `InstrSeqId`s are needed here.
fn validate_label(depth: u32, label_len: usize) -> Result<()> {
  if (depth as usize) >= label_len {
    return Err(Error::from_reason(format!(
      "branch label depth {depth} is out of range: only {label_len} enclosing block(s)"
    )));
  }
  Ok(())
}

// ---------------------------------------------------------------------------
// Read path: walrus function body -> InstrDesc array (mirrors the emit path).
// ---------------------------------------------------------------------------

/// Walk the sequence `seq_id` of `lf` into a descriptor array, maintaining the
/// label stack exactly as the emit path does (push on entry, pop on exit) so
/// branch depths invert consistently.
pub(crate) fn read_instr_seq(
  lf: &LocalFunction,
  seq_id: wir::InstrSeqId,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<Vec<InstrDesc>> {
  // Cap nesting BEFORE descending (see `MAX_NESTING_DEPTH`). walrus parsed this
  // (arbitrarily deep) module iteratively, so `lf` may nest past the cap; the
  // same ceiling emit/validate use keeps read symmetric — anything we can build
  // we can read back, and a legitimately deeper module surfaces a catchable error
  // here instead of overflowing the stack. On entry `label_stack.len()` is the
  // parent depth, so `>= MAX_NESTING_DEPTH` means this level would be `cap + 1`.
  if label_stack.len() >= MAX_NESTING_DEPTH {
    return Err(nesting_too_deep());
  }
  label_stack.push(seq_id);
  let mut out = Vec::with_capacity(lf.block(seq_id).instrs.len());
  for (instr, _loc) in &lf.block(seq_id).instrs {
    out.push(read_one(lf, instr, label_stack)?);
  }
  label_stack.pop();
  Ok(out)
}

/// Read a single walrus instruction into a descriptor.
///
/// This is the recursion point of the read walk (`read_instr_seq` -> `read_one`
/// -> `read_instr_seq`), so its frame is what stacks up to `MAX_NESTING_DEPTH`
/// deep. It is kept DELIBERATELY THIN — only the three control constructs
/// (`Block`/`Loop`/`IfElse`), the arms that actually recurse via `read_instr_seq`,
/// live here; every non-recursive (leaf) instruction is delegated to
/// [`read_leaf`]. This split is load-bearing for the deep-nesting abort guard: at
/// the `-O0` (debug) opt level a match arm's `InstrDesc` temporary does NOT
/// reliably coalesce with its siblings, so a `read_one` carrying all ~50 leaf arms
/// grows a frame that — multiplied by 256 recursion levels — SIGSEGVs the at-cap
/// round-trip. Housing the leaf arms in a SEPARATE, non-recursive `#[inline(never)]`
/// function keeps their big frame off the recursion stack (it is live for one leaf
/// at a time, never stacked), leaving `read_one`'s recursive frame at ~three
/// control descriptors plus one `read_leaf` return slot. (C7a added 20 GC leaf arms;
/// this split is what preserves the canary as the leaf set grows.)
fn read_one(
  lf: &LocalFunction,
  instr: &wir::Instr,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<InstrDesc> {
  Ok(match instr {
    // The three recursive control constructs stay here (they need `lf` +
    // `label_stack` to walk their child sequences). Everything else is a leaf,
    // delegated to `read_leaf` so its frame never joins the recursion stack.
    wir::Instr::Block(b) => {
      let inner = read_instr_seq(lf, b.seq, label_stack)?;
      let mut d = InstrDesc::new("Block");
      d.block_type = Some(from_instr_seq_type(lf.block(b.seq).ty)?);
      d.seq = Some(inner);
      d
    }
    wir::Instr::Loop(l) => {
      let inner = read_instr_seq(lf, l.seq, label_stack)?;
      let mut d = InstrDesc::new("Loop");
      d.block_type = Some(from_instr_seq_type(lf.block(l.seq).ty)?);
      d.seq = Some(inner);
      d
    }
    wir::Instr::IfElse(ie) => {
      let consequent = read_instr_seq(lf, ie.consequent, label_stack)?;
      let alternative = read_instr_seq(lf, ie.alternative, label_stack)?;
      let mut d = InstrDesc::new("IfElse");
      d.block_type = Some(from_instr_seq_type(lf.block(ie.consequent).ty)?);
      d.consequent = Some(consequent);
      d.alternative = Some(alternative);
      d
    }
    // Modern exception handling (C8a). `TryTable` is the 4th recursive control
    // construct (a `Block` twin with a `catches` list). LOAD-BEARING SCOPING:
    // invert the clause labels via `read_try_table_catches` (`label_depth` against
    // the CURRENT/outer stack — the try_table's own seq is NOT pushed here) BEFORE
    // descending into the body via `read_instr_seq` (which pushes `t.seq`), exactly
    // mirroring walrus' emit order. The per-clause work lives in the
    // `#[inline(never)]` helper, keeping this arm `Block`-slim.
    wir::Instr::TryTable(t) => {
      let catches = read_try_table_catches(&t.catches, label_stack)?;
      let inner = read_instr_seq(lf, t.seq, label_stack)?;
      let mut d = InstrDesc::new("TryTable");
      d.block_type = Some(from_instr_seq_type(lf.block(t.seq).ty)?);
      d.seq = Some(inner);
      d.catches = Some(catches);
      d
    }
    // Legacy exception handling (C8b). `Try` is the 5th recursive control construct
    // (a `Block` twin whose clauses carry FULL child handler bodies). Each handler
    // is its OWN `InstrSeq`, a SIBLING of the try body at the SAME label depth —
    // read via its own `read_instr_seq` inside `read_legacy_catches` (which pushes
    // the handler, exactly mirroring emit). No outer-scope clause labels: the legacy
    // clauses carry child sequences (`seq`) + a raw `relativeDepth` (Delegate), not
    // branch targets, so body/handler read order is immaterial. The per-clause work
    // lives in the `#[inline(never)]` helper, keeping this recursive arm slim.
    wir::Instr::Try(t) => {
      let catches = read_legacy_catches(lf, &t.catches, label_stack)?;
      let inner = read_instr_seq(lf, t.seq, label_stack)?;
      let mut d = InstrDesc::new("Try");
      d.block_type = Some(from_instr_seq_type(lf.block(t.seq).ty)?);
      d.seq = Some(inner);
      d.catches = Some(catches);
      d
    }
    _ => read_leaf(instr, label_stack)?,
  })
}

/// Invert a `TryTable`'s walrus catch clauses back into [`CatchClause`]s. Split
/// out of [`read_one`]'s `TryTable` arm and marked `#[inline(never)]` so its
/// per-clause `String`/`InstrDesc`-free locals stay OFF the recursion stack (see
/// [`read_one`]). Each tag becomes its stable `.index()`; each `label` inverts via
/// [`label_depth`] against the CURRENT (outer) `label_stack` — the exact inverse
/// of [`try_table_catches_to_walrus`], and called BEFORE the body descent so the
/// try_table's own seq is not yet on the stack (the load-bearing outer-scope rule).
#[inline(never)]
fn read_try_table_catches(
  catches: &[wir::TryTableCatch],
  label_stack: &[wir::InstrSeqId],
) -> Result<Vec<CatchClause>> {
  let mut out = Vec::with_capacity(catches.len());
  for c in catches {
    let clause = match c {
      wir::TryTableCatch::Catch { tag, label } => CatchClause {
        kind: "Catch".to_string(),
        tag: Some(tag.index() as f64),
        label: Some(label_depth(*label, label_stack)? as f64),
        seq: None,
        relative_depth: None,
        block_type: None,
      },
      wir::TryTableCatch::CatchRef { tag, label } => CatchClause {
        kind: "CatchRef".to_string(),
        tag: Some(tag.index() as f64),
        label: Some(label_depth(*label, label_stack)? as f64),
        seq: None,
        relative_depth: None,
        block_type: None,
      },
      wir::TryTableCatch::CatchAll { label } => CatchClause {
        kind: "CatchAll".to_string(),
        tag: None,
        label: Some(label_depth(*label, label_stack)? as f64),
        seq: None,
        relative_depth: None,
        block_type: None,
      },
      wir::TryTableCatch::CatchAllRef { label } => CatchClause {
        kind: "CatchAllRef".to_string(),
        tag: None,
        label: Some(label_depth(*label, label_stack)? as f64),
        seq: None,
        relative_depth: None,
        block_type: None,
      },
    };
    out.push(clause);
  }
  Ok(out)
}

/// Invert a legacy `Try`'s walrus `LegacyCatch` clauses back into [`CatchClause`]s
/// (C8b). Split out of [`read_one`]'s `Try` arm and marked `#[inline(never)]` so its
/// per-clause locals stay OFF the recursion stack (see [`read_one`]). Each
/// handler-bearing kind reads its OWN handler `InstrSeq` via [`read_instr_seq`]
/// (which pushes the handler — the handler is a SIBLING of the try body at the SAME
/// label depth, §A.2) into `seq`, and captures the handler's own `InstrSeqType` into
/// `blockType` (distinct from the try's — needed for a faithful round-trip). This is
/// the exact inverse of [`legacy_catches_to_walrus`]; `Delegate` surfaces its raw
/// `relativeDepth` VERBATIM. Maps the walrus names to the collision-free legacy
/// kinds (`Catch`→`"LegacyCatch"`, `CatchAll`→`"LegacyCatchAll"`,
/// `Delegate`→`"LegacyDelegate"`).
#[inline(never)]
fn read_legacy_catches(
  lf: &LocalFunction,
  catches: &[wir::LegacyCatch],
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<Vec<CatchClause>> {
  let mut out = Vec::with_capacity(catches.len());
  for c in catches {
    let clause = match c {
      wir::LegacyCatch::Catch { tag, handler } => {
        let seq = read_instr_seq(lf, *handler, label_stack)?;
        CatchClause {
          kind: "LegacyCatch".to_string(),
          tag: Some(tag.index() as f64),
          label: None,
          seq: Some(seq),
          relative_depth: None,
          block_type: Some(from_instr_seq_type(lf.block(*handler).ty)?),
        }
      }
      wir::LegacyCatch::CatchAll { handler } => {
        let seq = read_instr_seq(lf, *handler, label_stack)?;
        CatchClause {
          kind: "LegacyCatchAll".to_string(),
          tag: None,
          label: None,
          seq: Some(seq),
          relative_depth: None,
          block_type: Some(from_instr_seq_type(lf.block(*handler).ty)?),
        }
      }
      wir::LegacyCatch::Delegate { relative_depth } => CatchClause {
        kind: "LegacyDelegate".to_string(),
        tag: None,
        label: None,
        seq: None,
        relative_depth: Some(*relative_depth as f64),
        block_type: None,
      },
    };
    out.push(clause);
  }
  Ok(out)
}

/// Read a single NON-CONTROL (leaf) walrus instruction into a descriptor. Split
/// out of [`read_one`] and marked `#[inline(never)]` so this ~50-arm match keeps
/// its large frame OFF the recursion stack (see [`read_one`] for why that matters
/// to the deep-nesting abort guard). It is called once per leaf and never
/// recurses, so its frame is live for a single instruction at a time. `Br`/`BrIf`/
/// `BrTable` read `label_stack` (immutably) to invert their absolute target back
/// to a relative depth; no leaf needs the mutable stack or `lf`.
#[inline(never)]
fn read_leaf(instr: &wir::Instr, label_stack: &[wir::InstrSeqId]) -> Result<InstrDesc> {
  Ok(match instr {
    wir::Instr::Unreachable(_) => InstrDesc::new("Unreachable"),
    wir::Instr::Return(_) => InstrDesc::new("Return"),
    wir::Instr::Drop(_) => InstrDesc::new("Drop"),
    wir::Instr::Const(c) => {
      let mut d = InstrDesc::new("Const");
      d.value = Some(match c.value {
        wir::Value::I32(v) => ConstValue::I32 { value: v },
        wir::Value::I64(v) => ConstValue::I64 {
          value: BigInt::from(v),
        },
        wir::Value::F32(v) => ConstValue::F32 { value: v as f64 },
        wir::Value::F64(v) => ConstValue::F64 { value: v },
        // The `u128` -> 16 little-endian bytes unfold lives in
        // `v128_u128_to_bytes` (an `#[inline(never)]` helper) so its `[u8; 16]`
        // stays out of this recursive walker's frame.
        wir::Value::V128(v) => ConstValue::V128 {
          value: v128_u128_to_bytes(v),
        },
      });
      d
    }
    wir::Instr::LocalGet(e) => {
      let mut d = InstrDesc::new("LocalGet");
      d.local = Some(e.local.index() as f64);
      d
    }
    wir::Instr::LocalSet(e) => {
      let mut d = InstrDesc::new("LocalSet");
      d.local = Some(e.local.index() as f64);
      d
    }
    wir::Instr::LocalTee(e) => {
      let mut d = InstrDesc::new("LocalTee");
      d.local = Some(e.local.index() as f64);
      d
    }
    wir::Instr::GlobalGet(e) => {
      let mut d = InstrDesc::new("GlobalGet");
      d.global = Some(e.global.index() as f64);
      d
    }
    wir::Instr::GlobalSet(e) => {
      let mut d = InstrDesc::new("GlobalSet");
      d.global = Some(e.global.index() as f64);
      d
    }
    wir::Instr::Call(c) => {
      let mut d = InstrDesc::new("Call");
      d.func = Some(c.func.index() as f64);
      d
    }
    wir::Instr::Select(s) => {
      let mut d = InstrDesc::new("Select");
      d.select_type = match s.ty {
        Some(vt) => Some(vt.try_into()?),
        None => None,
      };
      d
    }
    wir::Instr::Br(br) => {
      let mut d = InstrDesc::new("Br");
      d.label = Some(label_depth(br.block, label_stack)? as f64);
      d
    }
    wir::Instr::BrIf(br) => {
      let mut d = InstrDesc::new("BrIf");
      d.label = Some(label_depth(br.block, label_stack)? as f64);
      d
    }
    wir::Instr::BrTable(bt) => {
      let labels = bt
        .blocks
        .iter()
        .map(|b| label_depth(*b, label_stack).map(|l| l as f64))
        .collect::<Result<Vec<_>>>()?;
      let mut d = InstrDesc::new("BrTable");
      d.labels = Some(labels);
      d.default_label = Some(label_depth(bt.default, label_stack)? as f64);
      d
    }
    // `Binop`/`Unop` read BOTH fieldless and lane-carrying ops: `to_str` yields
    // the name plus, for a lane op, its `idx` (set as `lane`). `to_str` is
    // infallible (every walrus variant maps), so no `?`.
    wir::Instr::Binop(b) => {
      let mut d = InstrDesc::new("Binop");
      let (name, lane) = binop_to_str(&b.op);
      d.op = Some(name.to_string());
      d.lane = lane.map(|l| l as f64);
      d
    }
    wir::Instr::Unop(u) => {
      let mut d = InstrDesc::new("Unop");
      let (name, lane) = unop_to_str(&u.op);
      d.op = Some(name.to_string());
      d.lane = lane.map(|l| l as f64);
      d
    }
    wir::Instr::TernOp(t) => {
      let mut d = InstrDesc::new("TernOp");
      // TernaryOp has no lane-carriers, so `lane` is always `None` here.
      let (name, _lane) = ternop_to_str(&t.op);
      d.op = Some(name.to_string());
      d
    }
    // Fixed-shape SIMD instructions (C6a). The two fieldless ones are trivial;
    // `I8x16Shuffle`'s 16-byte immediate is unfolded into a `Uint8Array`.
    wir::Instr::V128Bitselect(_) => InstrDesc::new("V128Bitselect"),
    wir::Instr::I8x16Swizzle(_) => InstrDesc::new("I8x16Swizzle"),
    wir::Instr::I8x16Shuffle(e) => {
      let mut d = InstrDesc::new("I8x16Shuffle");
      // 16-byte immediate built in `shuffle_indices_to_bytes` (an
      // `#[inline(never)]` helper) so its `[u8; 16]` temporary stays out of
      // this recursive frame — mirrors the `Const` v128 arm.
      d.shuffle_indices = Some(shuffle_indices_to_bytes(&e.indices));
      d
    }
    wir::Instr::MemorySize(e) => {
      let mut d = InstrDesc::new("MemorySize");
      d.memory = Some(e.memory.index() as f64);
      d
    }
    wir::Instr::MemoryGrow(e) => {
      let mut d = InstrDesc::new("MemoryGrow");
      d.memory = Some(e.memory.index() as f64);
      d
    }
    wir::Instr::MemoryInit(e) => {
      let mut d = InstrDesc::new("MemoryInit");
      d.memory = Some(e.memory.index() as f64);
      d.data = Some(e.data.index() as f64);
      d
    }
    wir::Instr::DataDrop(e) => {
      let mut d = InstrDesc::new("DataDrop");
      d.data = Some(e.data.index() as f64);
      d
    }
    wir::Instr::MemoryCopy(e) => {
      // `memory` carries the DESTINATION, `srcMemory` the SOURCE (see `emit_one`).
      let mut d = InstrDesc::new("MemoryCopy");
      d.memory = Some(e.dst.index() as f64);
      d.src_memory = Some(e.src.index() as f64);
      d
    }
    wir::Instr::MemoryFill(e) => {
      let mut d = InstrDesc::new("MemoryFill");
      d.memory = Some(e.memory.index() as f64);
      d
    }
    wir::Instr::Load(e) => {
      let mut d = InstrDesc::new("Load");
      d.memory = Some(e.memory.index() as f64);
      d.load_kind = Some(load_kind_from_walrus(e.kind));
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d
    }
    wir::Instr::Store(e) => {
      let mut d = InstrDesc::new("Store");
      d.memory = Some(e.memory.index() as f64);
      d.store_kind = Some(store_kind_from_walrus(e.kind));
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d
    }
    wir::Instr::LoadSimd(e) => {
      // The 20-arm `LoadSimdKind` decode lives in `load_simd_kind_from_walrus`
      // (an `#[inline(never)]` helper) so its match stays out of this recursive
      // walker's frame — mirrors the `Binop`/`Unop` `*_to_str` split.
      let mut d = InstrDesc::new("LoadSimd");
      d.memory = Some(e.memory.index() as f64);
      d.load_simd_kind = Some(load_simd_kind_from_walrus(e.kind));
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d
    }
    wir::Instr::AtomicRmw(e) => {
      let mut d = InstrDesc::new("AtomicRmw");
      d.memory = Some(e.memory.index() as f64);
      d.atomic_op = Some(atomic_op_from_walrus(e.op));
      d.atomic_width = Some(atomic_width_from_walrus(e.width));
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d
    }
    wir::Instr::Cmpxchg(e) => {
      let mut d = InstrDesc::new("Cmpxchg");
      d.memory = Some(e.memory.index() as f64);
      d.atomic_width = Some(atomic_width_from_walrus(e.width));
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d
    }
    wir::Instr::AtomicNotify(e) => {
      let mut d = InstrDesc::new("AtomicNotify");
      d.memory = Some(e.memory.index() as f64);
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d
    }
    wir::Instr::AtomicWait(e) => {
      let mut d = InstrDesc::new("AtomicWait");
      d.memory = Some(e.memory.index() as f64);
      d.mem_arg = Some(mem_arg_from_walrus(&e.arg));
      d.sixty_four = Some(e.sixty_four);
      d
    }
    wir::Instr::AtomicFence(_) => InstrDesc::new("AtomicFence"),
    wir::Instr::TableGet(e) => {
      let mut d = InstrDesc::new("TableGet");
      d.table = Some(e.table.index() as f64);
      d
    }
    wir::Instr::TableSet(e) => {
      let mut d = InstrDesc::new("TableSet");
      d.table = Some(e.table.index() as f64);
      d
    }
    wir::Instr::TableGrow(e) => {
      let mut d = InstrDesc::new("TableGrow");
      d.table = Some(e.table.index() as f64);
      d
    }
    wir::Instr::TableSize(e) => {
      let mut d = InstrDesc::new("TableSize");
      d.table = Some(e.table.index() as f64);
      d
    }
    wir::Instr::TableFill(e) => {
      let mut d = InstrDesc::new("TableFill");
      d.table = Some(e.table.index() as f64);
      d
    }
    wir::Instr::TableInit(e) => {
      let mut d = InstrDesc::new("TableInit");
      d.table = Some(e.table.index() as f64);
      d.elem = Some(e.elem.index() as f64);
      d
    }
    wir::Instr::TableCopy(e) => {
      // `table` carries the DESTINATION, `srcTable` the SOURCE (see `emit_one`).
      let mut d = InstrDesc::new("TableCopy");
      d.table = Some(e.dst.index() as f64);
      d.src_table = Some(e.src.index() as f64);
      d
    }
    wir::Instr::ElemDrop(e) => {
      let mut d = InstrDesc::new("ElemDrop");
      d.elem = Some(e.elem.index() as f64);
      d
    }
    wir::Instr::CallIndirect(e) => {
      let mut d = InstrDesc::new("CallIndirect");
      d.type_index = Some(e.ty.index() as f64);
      d.table = Some(e.table.index() as f64);
      d
    }
    wir::Instr::RefNull(e) => {
      // `e.ty` is a walrus `RefType` (Copy). The walrus -> napi `HeapType`
      // conversion is fallible on a `#[non_exhaustive]` variant — propagate with
      // `?`, never panic.
      let mut d = InstrDesc::new("RefNull");
      d.ref_type = Some(RefType {
        nullable: e.ty.nullable,
        heap: e.ty.heap_type.try_into()?,
      });
      d
    }
    wir::Instr::RefIsNull(_) => InstrDesc::new("RefIsNull"),
    wir::Instr::RefFunc(e) => {
      let mut d = InstrDesc::new("RefFunc");
      d.func = Some(e.func.index() as f64);
      d
    }
    wir::Instr::ReturnCall(e) => {
      let mut d = InstrDesc::new("ReturnCall");
      d.func = Some(e.func.index() as f64);
      d
    }
    wir::Instr::ReturnCallIndirect(e) => {
      let mut d = InstrDesc::new("ReturnCallIndirect");
      d.type_index = Some(e.ty.index() as f64);
      d.table = Some(e.table.index() as f64);
      d
    }
    // GC reference instructions (C7b) — the label-free subset. Built INLINE as
    // tail expressions, exactly like every other leaf arm (see the C7a note just
    // below for why READ inlines where EMIT/VALIDATE delegate). `CallRef`/
    // `ReturnCallRef` surface their callee `TypeId` as `type_index` (like
    // `CallIndirect`); `RefTest`/`RefCast` rebuild the shared `refType` payload
    // through the SAME fallible walrus -> napi heap conversion as `RefNull`
    // (`try_into` errs catchably on a `#[non_exhaustive]` future heap variant).
    wir::Instr::RefAsNonNull(_) => InstrDesc::new("RefAsNonNull"),
    wir::Instr::CallRef(e) => {
      let mut d = InstrDesc::new("CallRef");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ReturnCallRef(e) => {
      let mut d = InstrDesc::new("ReturnCallRef");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::RefI31(_) => InstrDesc::new("RefI31"),
    wir::Instr::I31GetS(_) => InstrDesc::new("I31GetS"),
    wir::Instr::I31GetU(_) => InstrDesc::new("I31GetU"),
    wir::Instr::RefTest(e) => {
      let mut d = InstrDesc::new("RefTest");
      d.ref_type = Some(RefType {
        nullable: e.nullable,
        heap: e.heap_type.try_into()?,
      });
      d
    }
    wir::Instr::RefCast(e) => {
      let mut d = InstrDesc::new("RefCast");
      d.ref_type = Some(RefType {
        nullable: e.nullable,
        heap: e.heap_type.try_into()?,
      });
      d
    }
    wir::Instr::AnyConvertExtern(_) => InstrDesc::new("AnyConvertExtern"),
    wir::Instr::ExternConvertAny(_) => InstrDesc::new("ExternConvertAny"),
    wir::Instr::RefEq(_) => InstrDesc::new("RefEq"),
    // GC branch instructions (C7c) — the label-carrying subset. These are
    // LEAVES: each references an ENCLOSING block by label and carries NO child
    // sequence, so they belong here (off the recursion path), built INLINE as
    // tail expressions like every other leaf arm. Each inverts its absolute
    // `block` target back to a relative depth via `label_depth` exactly like
    // `Br`; `BrOnCast`/`BrOnCastFail` rebuild walrus' `from_*`/`to_*` cast pair
    // as `refType` (FROM) + `toRefType` (TO) through the SAME fallible
    // walrus -> napi heap conversion as `RefTest`/`RefCast`.
    wir::Instr::BrOnNull(e) => {
      let mut d = InstrDesc::new("BrOnNull");
      d.label = Some(label_depth(e.block, label_stack)? as f64);
      d
    }
    wir::Instr::BrOnNonNull(e) => {
      let mut d = InstrDesc::new("BrOnNonNull");
      d.label = Some(label_depth(e.block, label_stack)? as f64);
      d
    }
    wir::Instr::BrOnCast(e) => {
      let mut d = InstrDesc::new("BrOnCast");
      d.label = Some(label_depth(e.block, label_stack)? as f64);
      d.ref_type = Some(RefType {
        nullable: e.from_nullable,
        heap: e.from_heap_type.try_into()?,
      });
      d.to_ref_type = Some(RefType {
        nullable: e.to_nullable,
        heap: e.to_heap_type.try_into()?,
      });
      d
    }
    wir::Instr::BrOnCastFail(e) => {
      let mut d = InstrDesc::new("BrOnCastFail");
      d.label = Some(label_depth(e.block, label_stack)? as f64);
      d.ref_type = Some(RefType {
        nullable: e.from_nullable,
        heap: e.from_heap_type.try_into()?,
      });
      d.to_ref_type = Some(RefType {
        nullable: e.to_nullable,
        heap: e.to_heap_type.try_into()?,
      });
      d
    }
    // GC struct/array instructions (C7a). These arms build the descriptor INLINE
    // (each `let mut d = InstrDesc::new(..); ..; d` is the arm's tail expression),
    // exactly like every arm above — deliberately NOT via a by-value-returning
    // helper. At the `-O0` (debug) opt level the recursive `read_one`/
    // `read_instr_seq` frames are the deep-nesting constraint (see
    // `MAX_NESTING_DEPTH`), and a `fn read_gc(..) -> InstrDesc` forces the callee's
    // `InstrDesc` result into a DISTINCT, non-coalescing `sret` slot in
    // `read_one`'s frame at EACH call site — measured to erode the at-cap headroom
    // until the canary round-trip SIGSEGVs (whether the helper is `#[inline(never)]`
    // OR `#[inline(always)]`). Inline tail-expression arms, by contrast, all build
    // into the shared match-result place and cost ~nothing extra. So the frame-slim
    // choice for READ is to INLINE — the mirror image of why EMIT/VALIDATE DELEGATE:
    // `emit_struct`/`emit_array`/`validate_struct`/`validate_array` return
    // `Result<()>` and push into the builder, carrying no large value back, so a
    // helper there costs nothing. The build is infallible (a `TypeId`/`DataId`/
    // `ElementId` `.index()` and the `field`/`len` immediates are plain `u32`s).
    // `ArrayCopy` surfaces `dst_ty` as `type_index` and `src_ty` as
    // `src_type_index` (the inverse of `emit_array`).
    wir::Instr::StructNew(e) => {
      let mut d = InstrDesc::new("StructNew");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::StructNewDefault(e) => {
      let mut d = InstrDesc::new("StructNewDefault");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::StructGet(e) => {
      let mut d = InstrDesc::new("StructGet");
      d.type_index = Some(e.ty.index() as f64);
      d.field = Some(e.field as f64);
      d
    }
    wir::Instr::StructGetS(e) => {
      let mut d = InstrDesc::new("StructGetS");
      d.type_index = Some(e.ty.index() as f64);
      d.field = Some(e.field as f64);
      d
    }
    wir::Instr::StructGetU(e) => {
      let mut d = InstrDesc::new("StructGetU");
      d.type_index = Some(e.ty.index() as f64);
      d.field = Some(e.field as f64);
      d
    }
    wir::Instr::StructSet(e) => {
      let mut d = InstrDesc::new("StructSet");
      d.type_index = Some(e.ty.index() as f64);
      d.field = Some(e.field as f64);
      d
    }
    wir::Instr::ArrayNew(e) => {
      let mut d = InstrDesc::new("ArrayNew");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArrayNewDefault(e) => {
      let mut d = InstrDesc::new("ArrayNewDefault");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArrayNewFixed(e) => {
      let mut d = InstrDesc::new("ArrayNewFixed");
      d.type_index = Some(e.ty.index() as f64);
      // `wir::ArrayNewFixed.len` is a `u32`; widening u32 -> f64 is always lossless.
      d.len = Some(e.len as f64);
      d
    }
    wir::Instr::ArrayNewData(e) => {
      let mut d = InstrDesc::new("ArrayNewData");
      d.type_index = Some(e.ty.index() as f64);
      d.data = Some(e.data.index() as f64);
      d
    }
    wir::Instr::ArrayNewElem(e) => {
      let mut d = InstrDesc::new("ArrayNewElem");
      d.type_index = Some(e.ty.index() as f64);
      d.elem = Some(e.elem.index() as f64);
      d
    }
    wir::Instr::ArrayGet(e) => {
      let mut d = InstrDesc::new("ArrayGet");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArrayGetS(e) => {
      let mut d = InstrDesc::new("ArrayGetS");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArrayGetU(e) => {
      let mut d = InstrDesc::new("ArrayGetU");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArraySet(e) => {
      let mut d = InstrDesc::new("ArraySet");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArrayLen(_) => InstrDesc::new("ArrayLen"),
    wir::Instr::ArrayFill(e) => {
      let mut d = InstrDesc::new("ArrayFill");
      d.type_index = Some(e.ty.index() as f64);
      d
    }
    wir::Instr::ArrayCopy(e) => {
      let mut d = InstrDesc::new("ArrayCopy");
      d.type_index = Some(e.dst_ty.index() as f64);
      d.src_type_index = Some(e.src_ty.index() as f64);
      d
    }
    wir::Instr::ArrayInitData(e) => {
      let mut d = InstrDesc::new("ArrayInitData");
      d.type_index = Some(e.ty.index() as f64);
      d.data = Some(e.data.index() as f64);
      d
    }
    wir::Instr::ArrayInitElem(e) => {
      let mut d = InstrDesc::new("ArrayInitElem");
      d.type_index = Some(e.ty.index() as f64);
      d.elem = Some(e.elem.index() as f64);
      d
    }
    // Modern exception handling (C8a) leaf ops. `TryTable` is NOT here — it is a
    // recursive control construct handled in `read_one`. `Throw` surfaces its tag's
    // stable `.index()`; `ThrowRef` is fieldless.
    wir::Instr::Throw(t) => {
      let mut d = InstrDesc::new("Throw");
      d.tag = Some(t.tag.index() as f64);
      d
    }
    wir::Instr::ThrowRef(_) => InstrDesc::new("ThrowRef"),
    // Legacy EH leaf op (C8b). `Try` is NOT here — it is a recursive control
    // construct handled in `read_one`. `Rethrow` surfaces its raw `relativeDepth`
    // VERBATIM (walrus never resolved it — §A.3), NOT via the `label_stack`.
    wir::Instr::Rethrow(r) => {
      let mut d = InstrDesc::new("Rethrow");
      d.relative_depth = Some(r.relative_depth as f64);
      d
    }
    // Wide-arithmetic instructions (C9) — the four fieldless leaves. Built INLINE
    // as tail expressions like every other fieldless leaf (see the C7a note above
    // for why READ inlines where EMIT/VALIDATE delegate): each is a bare
    // `InstrDesc::new` with no payload.
    wir::Instr::I64Add128(_) => InstrDesc::new("I64Add128"),
    wir::Instr::I64Sub128(_) => InstrDesc::new("I64Sub128"),
    wir::Instr::I64MulWideS(_) => InstrDesc::new("I64MulWideS"),
    wir::Instr::I64MulWideU(_) => InstrDesc::new("I64MulWideU"),
    other => {
      // MIRROR-WALRUS: never panic on an out-of-subset variant — surface a
      // catchable error naming it. Later C-tasks replace these arms with real
      // handling.
      let dbg = format!("{other:?}");
      let name = dbg.split(['(', ' ', '{']).next().unwrap_or("unknown");
      return Err(Error::from_reason(format!(
        "instruction `{name}` is not yet supported by instructions() (only the C1a/C1b/C2/C3/C4/C5 \
         core, control-flow, numeric-operator, memory/load-store, atomic, table, \
         reference/tail-call, GC struct/array/reference/branch, and modern exception-handling \
         subset is)"
      )));
    }
  })
}

// ---------------------------------------------------------------------------
// Operator-table tests (exhaustive over all 352 fieldless variants).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashSet;

  /// For one generated table: `to_str` maps every fieldless variant to a UNIQUE
  /// name with NO lane, and `from_str(to_str(v), None) == v` for all of them
  /// (compared through `{:?}` since the walrus enums have no `PartialEq`). This is
  /// the definitive proof the two directions are exact inverses over the fieldless
  /// variants.
  fn check_roundtrip<T: std::fmt::Debug>(
    all: &[T],
    to_str: fn(&T) -> (&'static str, Option<u8>),
    from_str: fn(&str, Option<f64>) -> Result<T>,
  ) {
    let mut names = HashSet::new();
    for v in all {
      let (name, lane) = to_str(v);
      assert_eq!(lane, None, "a fieldless variant must not carry a lane");
      assert!(names.insert(name), "duplicate operator name `{name}`");
      let back = from_str(name, None).expect("a fieldless name must decode back to a variant");
      assert_eq!(
        format!("{v:?}"),
        format!("{back:?}"),
        "round-trip mismatch for `{name}`"
      );
    }
    assert_eq!(names.len(), all.len());
  }

  /// For one generated table's LANE variants: `to_str` surfaces the name plus the
  /// variant's `idx`; `from_str(name, None)` is a catchable error (a lane op needs
  /// its lane); and `from_str(name, Some(idx))` rebuilds the exact variant. This
  /// proves the lane name<->variant mapping is an exact inverse and that a missing
  /// lane is rejected.
  fn check_lane_roundtrip<T: std::fmt::Debug>(
    all: &[T],
    to_str: fn(&T) -> (&'static str, Option<u8>),
    from_str: fn(&str, Option<f64>) -> Result<T>,
  ) {
    let mut names = HashSet::new();
    for v in all {
      let (name, lane) = to_str(v);
      // The test consts instantiate every lane variant with `idx: 7`.
      assert_eq!(lane, Some(7), "a lane variant must surface its `idx`");
      assert!(names.insert(name), "duplicate lane operator name `{name}`");
      assert!(
        from_str(name, None).is_err(),
        "lane op `{name}` must require a lane"
      );
      let back = from_str(name, Some(7.0)).expect("a lane name + lane must build the variant");
      assert_eq!(
        format!("{v:?}"),
        format!("{back:?}"),
        "lane round-trip mismatch for `{name}`"
      );
    }
    assert_eq!(names.len(), all.len());
  }

  #[test]
  fn binop_table_is_exact_inverse() {
    assert_eq!(BINOP_ALL_FIELDLESS.len(), 214);
    check_roundtrip(BINOP_ALL_FIELDLESS, binop_to_str, binop_from_str);
  }

  #[test]
  fn unop_table_is_exact_inverse() {
    assert_eq!(UNOP_ALL_FIELDLESS.len(), 129);
    check_roundtrip(UNOP_ALL_FIELDLESS, unop_to_str, unop_from_str);
  }

  #[test]
  fn ternop_table_is_exact_inverse() {
    assert_eq!(TERNOP_ALL_FIELDLESS.len(), 9);
    check_roundtrip(TERNOP_ALL_FIELDLESS, ternop_to_str, ternop_from_str);
  }

  #[test]
  fn lane_carriers_round_trip_and_require_a_lane() {
    // The 14 lane-carriers (6 BinaryOp `*ReplaceLane`, 8 UnaryOp `*ExtractLane*`,
    // 0 TernaryOp) map name<->variant exactly, surface their `idx`, and reject a
    // missing lane.
    assert_eq!(BINOP_ALL_LANE.len(), 6);
    assert_eq!(UNOP_ALL_LANE.len(), 8);
    assert_eq!(TERNOP_ALL_LANE.len(), 0);
    check_lane_roundtrip(BINOP_ALL_LANE, binop_to_str, binop_from_str);
    check_lane_roundtrip(UNOP_ALL_LANE, unop_to_str, unop_from_str);
    check_lane_roundtrip(TERNOP_ALL_LANE, ternop_to_str, ternop_from_str);

    // An unknown name is still a catchable error in every table (C1b behavior),
    // and a spurious lane on a fieldless name is simply ignored.
    assert!(binop_from_str("NotARealOp", None).is_err());
    assert!(ternop_from_str("", None).is_err());
    assert!(binop_from_str("I32Add", Some(3.0)).is_ok());
  }
}
