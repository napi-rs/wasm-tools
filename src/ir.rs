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
//! Only process-aborting hazards are guarded: an out-of-range local/global/func
//! index, an out-of-range branch label, or a bad multi-value block-type index
//! are rejected with a catchable error BEFORE a panicking walrus lookup can be
//! reached. Nothing here validates wasm well-formedness — an ill-typed body is
//! emitted as-is and left for `WebAssembly.validate` to reject.

use napi::bindgen_prelude::{BigInt, Result};
use napi::Error;
use napi_derive::napi;
use walrus::ir as wir;
use walrus::ir::{BinaryOp, TernaryOp, UnaryOp};
use walrus::{FunctionBuilder, FunctionId, GlobalId, LocalFunction, LocalId, Module};

use crate::convert::{resolve_type_id, val_type_to_walrus_in};
use crate::valtype::ValType;

/// The maximum control-flow nesting depth the three instruction walks
/// (`validate_body`, `emit_desc`, `read_instr_seq`) will descend before
/// refusing with a catchable error.
///
/// ## Why a cap at all
/// walrus is fully ITERATIVE over nesting — it parses with an explicit
/// `ControlStack = Vec<ControlFrame>` and emits via `dfs_in_order`, an explicit
/// `stack: Vec<(InstrSeqId, usize)>` — so it can parse/emit arbitrarily-deep
/// `block block … end end` without a stack overflow. Our three walks, however,
/// RECURSE once per nesting level. Under `panic = abort` a Rust stack overflow
/// is a `SIGABRT` that `catch_unwind` does NOT catch, so it would tear down the
/// whole Node process across the FFI boundary — an uncatchable abort reachable
/// from either a JS-supplied deep `body` (build) or a legitimately deep parsed
/// module (`.instructions()` read). Capping converts that abort into a catchable
/// `napi::Error` BEFORE the unsafe frame is ever reached.
///
/// ## Why this exact value (and not higher)
/// The nested `Vec<InstrDesc>` tree ALSO recurses in napi's generated Rust↔JS
/// marshalling (JS→Rust arg decode, Rust→JS return encode) and in Rust `Drop` —
/// recursions we do not own and cannot guard. Those impose their own ceiling on
/// usable nesting, and on the smaller `wasm32-wasi` stack that ceiling is what
/// binds. `MAX_NESTING_DEPTH` is chosen to sit comfortably below the empirically
/// measured wasi marshalling ceiling (with margin for the `+ K` over-cap tests,
/// which must marshal `MAX_NESTING_DEPTH + K` descriptors just to reach the
/// guard), so the WHOLE round trip is safe end-to-end. Real-world wasm nesting is
/// tiny (compilers rarely exceed ~50 deep), so this ceiling is invisible in
/// practice. All three walks share the SAME cap so build and read stay symmetric:
/// a body you can build+emit is a body you can read back, and vice-versa.
///
/// ### Empirically why 256
/// Measured on this repo (debug builds, per-depth isolated processes):
/// * `wasm32-wasi` never hard-aborts on over-deep nesting — the emnapi JS glue
///   raises a CATCHABLE `RangeError` ("Maximum call stack size exceeded") on both
///   decode (build ≳ 550 deep) and encode (read ≳ 700 deep). So the wasi target
///   is safe at any N; it only ever throws.
/// * Native (Node 24) DOES hard-abort: the JS→Rust decode SIGSEGVs at ≈740 deep,
///   and — tighter — the Rust→JS ENCODE of a read-back tree hits V8's
///   `Check failed: isolate_->IsOnCentralStack()` fatal at ≈525 deep (a GC fired
///   while deep in the native encode recursion; GC-timing-dependent, so the
///   threshold is a fuzzy, non-monotonic window, not a hard line).
///
/// 256 sits ~2x below that native encode fatal (read-back at depth 256/300/350
/// was 25/25 clean across runs) and ~2.9x below the decode SIGSEGV, leaving
/// comfortable margin for the `+ K` over-cap tests to reach the guard and throw
/// catchably rather than trip an abort. Anything materially higher risks the
/// timing-dependent native encode fatal under real heap pressure.
pub(crate) const MAX_NESTING_DEPTH: usize = 256;

/// The catchable error returned when a walk would descend past
/// [`MAX_NESTING_DEPTH`].
fn nesting_too_deep() -> Error {
  Error::from_reason(format!(
    "instruction nesting too deep (max {MAX_NESTING_DEPTH}); refusing to recurse to avoid a \
     stack overflow"
  ))
}

/// A constant value carried by a `*.const` instruction, mirroring
/// `walrus::ir::Value` (V128 is intentionally excluded until the SIMD task).
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'I32', value: number } | { type: 'I64', value: bigint }`
/// `| { type: 'F32', value: number } | { type: 'F64', value: number }`.
///
/// `I64` crosses the boundary as a JS `bigint` for exactness; an `f32` has no
/// dedicated JS type, so `F32` uses a `number` (`f64`) that is narrowed to
/// `f32` on emit.
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
    type_index: u32,
  },
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
/// This is the C1a/C1b subset: leaf ops (`Unreachable`/`Return`/`Drop`),
/// `Const`, local/global get/set/tee, `Call`, `Select`, the control constructs
/// (`Block`/`Loop`/`IfElse`), the branches (`Br`/`BrIf`/`BrTable`), and the
/// numeric/comparison/conversion operators (`Binop`/`Unop`/`TernOp`, keyed by
/// `op`). Any other instruction is rejected catchably by both directions (later
/// tasks add memory, tables, refs, atomics, the lane-carrying SIMD ops, GC, and
/// EH).
#[napi(object)]
pub struct InstrDesc {
  /// The instruction discriminant — the walrus variant name.
  pub r#type: String,
  /// `Const`: the constant value.
  pub value: Option<ConstValue>,
  /// `LocalGet`/`LocalSet`/`LocalTee`: the referenced local's stable index.
  pub local: Option<u32>,
  /// `GlobalGet`/`GlobalSet`: the referenced global's stable index.
  pub global: Option<u32>,
  /// `Call`: the callee function's stable index.
  pub func: Option<u32>,
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
  /// `Br`/`BrIf`: the relative label depth of the branch target
  /// (`0` = the innermost enclosing block/loop/if).
  pub label: Option<u32>,
  /// `BrTable`: the relative label depths of the table's targets, in order.
  pub labels: Option<Vec<u32>>,
  /// `BrTable`: the relative label depth of the default (fallthrough) target.
  pub default_label: Option<u32>,
  /// `Binop`/`Unop`/`TernOp`: the operator name (the walrus
  /// `BinaryOp`/`UnaryOp`/`TernaryOp` variant name, e.g. `"I32Add"`, `"I32Eqz"`,
  /// `"F32x4RelaxedMadd"`). The `type` discriminant selects which of the three
  /// operator enums decodes it, so one shared field is unambiguous.
  pub op: Option<String>,
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
    Some(BlockType::MultiValue { type_index }) => {
      wir::InstrSeqType::MultiValue(resolve_type_id(module, type_index)?)
    }
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
      type_index: id.index() as u32,
    },
  })
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
// MIRROR-WALRUS: the only guarded hazards are string-decode failures (unknown op
// name, or a deferred lane-carrier), each surfaced as a catchable error. Nothing
// here type-checks operands.
//
// Scope is the 352 FIELDLESS variants (BinaryOp 214 + UnaryOp 129 + TernaryOp 9).
// The 14 lane-carrying SIMD variants (6 `*ReplaceLane`, 8 `*ExtractLane*`, each
// `{ idx: u8 }`) are DEFERRED to the SIMD task (C6): `to_str` returns a catchable
// error for them (the match stays EXHAUSTIVE — no `_` arm — so a future walrus
// variant is a COMPILE error, the safety net against a miscount), and `from_str`
// rejects their names (they are not buildable without a lane index).
// ---------------------------------------------------------------------------

/// The catchable error for an operator name that is not a fieldless variant of
/// the given enum (an unknown op, or a deferred lane-carrier name).
fn unknown_op(kind: &str, s: &str) -> Error {
  Error::from_reason(format!("unknown {kind} operator `{s}`"))
}

/// The catchable error for a deferred lane-carrying SIMD op encountered while
/// reading (`to_str`); building such an op is instead rejected by `from_str` as
/// an unknown operator.
fn deferred_lane_op(name: &str) -> Error {
  Error::from_reason(format!(
    "SIMD lane op `{name}` is deferred to the SIMD task (C6)"
  ))
}

/// Generate the two string-conversion functions (and a test-only slice of all
/// fieldless variants) for one walrus operator enum from a single variant list.
///
/// * `<to_str>(op) -> Ok("<VariantName>")` for a fieldless variant; a deferred
///   lane-carrier (`Variant { .. }`) returns a catchable error. The match is
///   EXHAUSTIVE (no wildcard) so a new walrus variant fails to compile.
/// * `<from_str>(s) -> Ok(Variant)` for a fieldless name; anything else
///   (including a deferred lane-carrier name) is a catchable error.
/// * `#[cfg(test)] const <ALL>: &[<Enum>]` — every fieldless variant, for the
///   exhaustive round-trip test.
macro_rules! str_enum {
  (
    kind: $kind:literal,
    ty: $Enum:ident,
    to_str: $to_str:ident,
    from_str: $from_str:ident,
    all_fieldless: $all:ident,
    fieldless: [ $($fl:ident),* $(,)? ],
    deferred: [ $($df:ident),* $(,)? ] $(,)?
  ) => {
    fn $to_str(op: &$Enum) -> Result<&'static str> {
      Ok(match op {
        $( $Enum::$fl => stringify!($fl), )*
        $( $Enum::$df { .. } => return Err(deferred_lane_op(stringify!($df))), )*
      })
    }

    fn $from_str(s: &str) -> Result<$Enum> {
      Ok(match s {
        $( stringify!($fl) => $Enum::$fl, )*
        other => return Err(unknown_op($kind, other)),
      })
    }

    #[cfg(test)]
    const $all: &[$Enum] = &[ $( $Enum::$fl, )* ];
  };
}

str_enum! {
  kind: "binary",
  ty: BinaryOp,
  to_str: binop_to_str,
  from_str: binop_from_str,
  all_fieldless: BINOP_ALL_FIELDLESS,
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
  deferred: [
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
  deferred: [
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
  fieldless: [
    F32x4RelaxedMadd, F32x4RelaxedNmadd, F64x2RelaxedMadd, F64x2RelaxedNmadd,
    I8x16RelaxedLaneselect, I16x8RelaxedLaneselect, I32x4RelaxedLaneselect,
    I64x2RelaxedLaneselect, I32x4RelaxedDotI8x16I7x16AddS,
  ],
  deferred: [
  ],
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
  let InstrDesc {
    r#type,
    value,
    local,
    global,
    func,
    select_type,
    block_type,
    seq,
    consequent,
    alternative,
    label,
    labels,
    default_label,
    op,
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
      };
      fb.instr_seq(seq_id).instr(wir::Const { value: wv });
    }
    "LocalGet" => {
      let id = local_id_at(module, local.ok_or_else(|| missing("LocalGet", "local"))?)?;
      fb.instr_seq(seq_id).instr(wir::LocalGet { local: id });
    }
    "LocalSet" => {
      let id = local_id_at(module, local.ok_or_else(|| missing("LocalSet", "local"))?)?;
      fb.instr_seq(seq_id).instr(wir::LocalSet { local: id });
    }
    "LocalTee" => {
      let id = local_id_at(module, local.ok_or_else(|| missing("LocalTee", "local"))?)?;
      fb.instr_seq(seq_id).instr(wir::LocalTee { local: id });
    }
    "GlobalGet" => {
      let id = global_id_at(
        module,
        global.ok_or_else(|| missing("GlobalGet", "global"))?,
      )?;
      fb.instr_seq(seq_id).instr(wir::GlobalGet { global: id });
    }
    "GlobalSet" => {
      let id = global_id_at(
        module,
        global.ok_or_else(|| missing("GlobalSet", "global"))?,
      )?;
      fb.instr_seq(seq_id).instr(wir::GlobalSet { global: id });
    }
    "Call" => {
      let id = function_id_at(module, func.ok_or_else(|| missing("Call", "func"))?)?;
      fb.instr_seq(seq_id).instr(wir::Call { func: id });
    }
    "Select" => {
      let ty = match select_type {
        Some(vt) => Some(val_type_to_walrus_in(module, vt)?),
        None => None,
      };
      fb.instr_seq(seq_id).instr(wir::Select { ty });
    }
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
    "Br" => {
      let target = label_target(label.ok_or_else(|| missing("Br", "label"))?, label_stack)?;
      fb.instr_seq(seq_id).instr(wir::Br { block: target });
    }
    "BrIf" => {
      let target = label_target(label.ok_or_else(|| missing("BrIf", "label"))?, label_stack)?;
      fb.instr_seq(seq_id).instr(wir::BrIf { block: target });
    }
    "BrTable" => {
      let labels = labels.ok_or_else(|| missing("BrTable", "labels"))?;
      let default = label_target(
        default_label.ok_or_else(|| missing("BrTable", "defaultLabel"))?,
        label_stack,
      )?;
      let blocks = labels
        .into_iter()
        .map(|d| label_target(d, label_stack))
        .collect::<Result<Vec<_>>>()?;
      fb.instr_seq(seq_id).instr(wir::BrTable {
        blocks: blocks.into_boxed_slice(),
        default,
      });
    }
    "Binop" => {
      let op = binop_from_str(&op.ok_or_else(|| missing("Binop", "op"))?)?;
      fb.instr_seq(seq_id).instr(wir::Binop { op });
    }
    "Unop" => {
      let op = unop_from_str(&op.ok_or_else(|| missing("Unop", "op"))?)?;
      fb.instr_seq(seq_id).instr(wir::Unop { op });
    }
    "TernOp" => {
      let op = ternop_from_str(&op.ok_or_else(|| missing("TernOp", "op"))?)?;
      fb.instr_seq(seq_id).instr(wir::TernOp { op });
    }
    other => {
      return Err(Error::from_reason(format!(
        "unknown or unsupported instruction type `{other}` (buildFunction handles only the \
         C1a/C1b core, control-flow, and numeric-operator subset)"
      )));
    }
  }
  Ok(())
}

/// The error for a descriptor missing a payload field its `type` requires.
fn missing(ty: &str, field: &str) -> Error {
  Error::from_reason(format!("`{ty}` instruction is missing its `{field}` field"))
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
    "Unreachable" | "Return" | "Drop" => {}
    "Const" => {
      // Mirror emit's only fallible Const check: an i64 that is not lossless.
      let value = d.value.as_ref().ok_or_else(|| missing("Const", "value"))?;
      if let ConstValue::I64 { value } = value {
        if !value.get_i64().1 {
          return Err(Error::from_reason(
            "i64 const value does not fit losslessly in a signed 64-bit integer",
          ));
        }
      }
    }
    "LocalGet" => {
      local_id_at(module, d.local.ok_or_else(|| missing("LocalGet", "local"))?)?;
    }
    "LocalSet" => {
      local_id_at(module, d.local.ok_or_else(|| missing("LocalSet", "local"))?)?;
    }
    "LocalTee" => {
      local_id_at(module, d.local.ok_or_else(|| missing("LocalTee", "local"))?)?;
    }
    "GlobalGet" => {
      global_id_at(
        module,
        d.global.ok_or_else(|| missing("GlobalGet", "global"))?,
      )?;
    }
    "GlobalSet" => {
      global_id_at(
        module,
        d.global.ok_or_else(|| missing("GlobalSet", "global"))?,
      )?;
    }
    "Call" => {
      function_id_at(module, d.func.ok_or_else(|| missing("Call", "func"))?)?;
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
    "Br" => {
      validate_label(d.label.ok_or_else(|| missing("Br", "label"))?, label_len)?;
    }
    "BrIf" => {
      validate_label(d.label.ok_or_else(|| missing("BrIf", "label"))?, label_len)?;
    }
    "BrTable" => {
      // Same field/ordering as emit: labels present, then default in range, then
      // every table entry in range.
      let labels = d
        .labels
        .as_ref()
        .ok_or_else(|| missing("BrTable", "labels"))?;
      validate_label(
        d.default_label
          .ok_or_else(|| missing("BrTable", "defaultLabel"))?,
        label_len,
      )?;
      for &l in labels {
        validate_label(l, label_len)?;
      }
    }
    // The only fallible step for an operator is the op-string decode: check the
    // `op` field is present and names a buildable (fieldless) variant, discarding
    // the decoded op. Ops carry no ids/labels, so nothing else needs resolving.
    "Binop" => {
      binop_from_str(d.op.as_deref().ok_or_else(|| missing("Binop", "op"))?)?;
    }
    "Unop" => {
      unop_from_str(d.op.as_deref().ok_or_else(|| missing("Unop", "op"))?)?;
    }
    "TernOp" => {
      ternop_from_str(d.op.as_deref().ok_or_else(|| missing("TernOp", "op"))?)?;
    }
    other => {
      return Err(Error::from_reason(format!(
        "unknown or unsupported instruction type `{other}` (buildFunction handles only the \
         C1a/C1b core, control-flow, and numeric-operator subset)"
      )));
    }
  }
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
      resolve_type_id(module, *type_index)?;
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
fn read_one(
  lf: &LocalFunction,
  instr: &wir::Instr,
  label_stack: &mut Vec<wir::InstrSeqId>,
) -> Result<InstrDesc> {
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
        wir::Value::V128(_) => {
          return Err(Error::from_reason(
            "v128 const is not yet supported by instructions() (SIMD is a later task)",
          ))
        }
      });
      d
    }
    wir::Instr::LocalGet(e) => {
      let mut d = InstrDesc::new("LocalGet");
      d.local = Some(e.local.index() as u32);
      d
    }
    wir::Instr::LocalSet(e) => {
      let mut d = InstrDesc::new("LocalSet");
      d.local = Some(e.local.index() as u32);
      d
    }
    wir::Instr::LocalTee(e) => {
      let mut d = InstrDesc::new("LocalTee");
      d.local = Some(e.local.index() as u32);
      d
    }
    wir::Instr::GlobalGet(e) => {
      let mut d = InstrDesc::new("GlobalGet");
      d.global = Some(e.global.index() as u32);
      d
    }
    wir::Instr::GlobalSet(e) => {
      let mut d = InstrDesc::new("GlobalSet");
      d.global = Some(e.global.index() as u32);
      d
    }
    wir::Instr::Call(c) => {
      let mut d = InstrDesc::new("Call");
      d.func = Some(c.func.index() as u32);
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
    wir::Instr::Br(br) => {
      let mut d = InstrDesc::new("Br");
      d.label = Some(label_depth(br.block, label_stack)?);
      d
    }
    wir::Instr::BrIf(br) => {
      let mut d = InstrDesc::new("BrIf");
      d.label = Some(label_depth(br.block, label_stack)?);
      d
    }
    wir::Instr::BrTable(bt) => {
      let labels = bt
        .blocks
        .iter()
        .map(|b| label_depth(*b, label_stack))
        .collect::<Result<Vec<_>>>()?;
      let mut d = InstrDesc::new("BrTable");
      d.labels = Some(labels);
      d.default_label = Some(label_depth(bt.default, label_stack)?);
      d
    }
    wir::Instr::Binop(b) => {
      let mut d = InstrDesc::new("Binop");
      d.op = Some(binop_to_str(&b.op)?.to_string());
      d
    }
    wir::Instr::Unop(u) => {
      let mut d = InstrDesc::new("Unop");
      d.op = Some(unop_to_str(&u.op)?.to_string());
      d
    }
    wir::Instr::TernOp(t) => {
      let mut d = InstrDesc::new("TernOp");
      d.op = Some(ternop_to_str(&t.op)?.to_string());
      d
    }
    other => {
      // MIRROR-WALRUS: never panic on an out-of-subset variant — surface a
      // catchable error naming it. Later C-tasks replace these arms with real
      // handling.
      let dbg = format!("{other:?}");
      let name = dbg.split(['(', ' ', '{']).next().unwrap_or("unknown");
      return Err(Error::from_reason(format!(
        "instruction `{name}` is not yet supported by instructions() (only the C1a/C1b core, \
         control-flow, and numeric-operator subset is)"
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
  /// name, and `from_str(to_str(v)) == v` for all of them (compared through
  /// `{:?}` since the walrus enums have no `PartialEq`). This is the definitive
  /// proof the two directions are exact inverses over the whole enum.
  fn check_roundtrip<T: std::fmt::Debug>(
    all: &[T],
    to_str: fn(&T) -> Result<&'static str>,
    from_str: fn(&str) -> Result<T>,
  ) {
    let mut names = HashSet::new();
    for v in all {
      let name = to_str(v).expect("a fieldless variant must map to a name");
      assert!(names.insert(name), "duplicate operator name `{name}`");
      let back = from_str(name).expect("a fieldless name must decode back to a variant");
      assert_eq!(
        format!("{v:?}"),
        format!("{back:?}"),
        "round-trip mismatch for `{name}`"
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
  fn deferred_lane_carriers_are_rejected_both_directions() {
    // A representative deferred lane-carrier from each enum that has them.
    let binop_lane = wir::BinaryOp::I8x16ReplaceLane { idx: 0 };
    let unop_lane = wir::UnaryOp::I8x16ExtractLaneS { idx: 0 };

    // `to_str` refuses to read a lane-carrier (catchable, mentions the deferral).
    let e = binop_to_str(&binop_lane).unwrap_err();
    assert!(format!("{e}").contains("deferred"), "got: {e}");
    let e = unop_to_str(&unop_lane).unwrap_err();
    assert!(format!("{e}").contains("deferred"), "got: {e}");

    // `from_str` refuses to build a lane-carrier by name, and any bogus name too.
    assert!(binop_from_str("I8x16ReplaceLane").is_err());
    assert!(unop_from_str("I8x16ExtractLaneS").is_err());
    assert!(binop_from_str("NotARealOp").is_err());
    assert!(ternop_from_str("").is_err());
  }
}
