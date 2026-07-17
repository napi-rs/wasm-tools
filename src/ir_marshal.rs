//! Iterative (heap work-stack) FFI marshalling for the recursive [`InstrDesc`]
//! descriptor tree — both directions of the Tier-C instruction layer crossing.
//!
//! ## Why this module exists (the CH hardening)
//! napi's DERIVED marshalling recurses once per nesting level on the CALL STACK
//! in both directions: the JS→Rust arg decode runs BEFORE the method body (so
//! `build_function`'s `validate_body` depth guard was unreachable for bodies
//! deep enough to exhaust the stack first — native Node SIGSEGVed uncatchably,
//! and the AVA-wasi harness sat at the guard's exact depth with ZERO margin),
//! and the Rust→JS return encode of `instructions()` recursed the same way
//! (native V8 fatal at a few hundred levels). Every field added to `InstrDesc`
//! widened those frames and lowered the ceilings further.
//!
//! The two wrapper types here replace that with EXPLICIT heap work-stacks, so
//! descriptor marshalling uses O(1) call stack at ANY depth, and the
//! [`MAX_NESTING_DEPTH`] guard is enforced DURING the decode — any over-deep
//! body yields the deterministic, catchable `nesting_too_deep()` error before
//! anything deeper than the cap is materialized, on every target and harness.
//!
//! ## How each element still uses the DERIVED (auto-lockstep) field code
//! * Decode ([`InstrBody`]): per element, a fresh empty JS object receives an
//!   own-`undefined` shadow of the three self-referential edges AND the `labels`
//!   leaf, plus an OWN-only shallow copy of every other enumerable property of
//!   the element, and the DERIVED `InstrDesc::from_napi_value` runs on that copy
//!   — it cannot recurse (the edges read as own `undefined`, so the derived read
//!   never re-enters via a polluted `Object.prototype.seq`) and cannot prealloc
//!   from an untrusted `labels.length` (the `labels` shadow makes the derived
//!   `Vec::<u32>` decode see `None`), and every plain field keeps its derived
//!   decoding and error behavior, automatically tracking future field additions.
//!   The real edges are then walked by the driver, reading OWN edges only so an
//!   inherited edge is ignored in lockstep with the derived read's shadow; the
//!   real OWN `labels` is decoded as a leaf `Vec<u32>` by a non-preallocating
//!   loop in `decode_element_shallow` (never pushed on the frame stack).
//! * Encode ([`InstrList`]): per (owned) element, the three edge `Option`s are
//!   `Option::take`n out, the now-edge-free descriptor is encoded with the
//!   DERIVED `InstrDesc::to_napi_value` (cannot recurse), and each taken
//!   `Some(vec)` becomes a child work-stack frame whose JS array is attached to
//!   the element's object under the edge's property name. A `None` edge sets no
//!   property — exactly the derived `#[napi(object)]` semantics for a `None`
//!   `Option` field. Taking the edges also dismantles the tree as it is
//!   consumed, so Rust `Drop` never sees deep nesting on this path.
//!
//! ## MAINTENANCE: adding a self-referential field
//! ANY future `InstrDesc` field that can contain `InstrDesc`s MUST be routed
//! through BOTH drivers or the derived per-element call will recurse again. A
//! DIRECT `InstrDesc` edge (own property of the descriptor):
//! * decode: extend [`EDGE_NAMES`]/[`EDGE_CSTRS`] (the copy-except skip list
//!   AND the edge walk in `InstrBody::from_napi_value`) and [`write_edge`];
//! * encode: extend the edge take/attach list in `InstrList::to_napi_value`.
//!
//! A field with a DIFFERENT shape (a `Vec<InstrDesc>` nested inside another `Vec`/
//! struct) additionally needs its OWN parent-slot variant + child-enqueue walk in
//! both drivers. One such field is HANDLED today: the LEGACY `Try` handler bodies
//! `catches[].seq` (C8b) — a `Vec<InstrDesc>` two levels down inside
//! `Vec<CatchClause>`. It is NOT in [`EDGE_NAMES`]/[`EDGE_CSTRS`] (adding `"seq"`
//! there would collide with `InstrDesc.seq`); instead it rides the
//! [`ParentSlot::CatchSeq`] frame bookkeeping: decode shadows each clause's `seq`
//! (`decode_catch_clause_shallow`) then a SECOND child-enqueue walk reads each
//! clause's OWN `seq`; encode takes each clause's `seq` out before
//! `InstrDesc::to_napi_value` then re-attaches it as a child frame.
//!
//! ## MAINTENANCE: adding a NON-edge `Vec`/`Option<Vec<T>>` field (e.g. C8a
//! `catches: Option<Vec<CatchClause>>`)
//! ANY `InstrDesc` field whose type is `Vec<T>`/`Option<Vec<T>>` — even a LEAF
//! (non-`InstrDesc` `T`, so no recursion) — inherits the untrusted-length abort:
//! the derived `Vec::<T>::from_napi_value` calls `Vec::with_capacity(len)` from
//! the JS `Array.length` BEFORE inspecting any element, and a sparse
//! `length ≈ 2**32` (own OR inherited via `Object.prototype`) aborts the process
//! (capacity overflow / `handle_alloc_error`) — uncatchable, especially under
//! WASI `panic=abort`. Such a field MUST be handled like `labels`:
//! * shadow it as an own `undefined` on the copy AND skip it in the copy loop of
//!   `decode_element_shallow` (so the derived decode never reaches its `Vec`
//!   prealloc, on any prototype chain), then
//! * decode the original's OWN value there yourself with a NON-preallocating
//!   loop (`Vec::new()` + push, decoding each element via its own
//!   `FromNapiValue`), setting the field on the returned `desc`.
//!
//! It is a LEAF (decoded inline), NOT a frame-stack edge — do not add it to the
//! edge lists. Two such fields exist today: `labels` (a `BrTable`'s `Vec<u32>`)
//! and `catches` (a `TryTable`'s `Vec<CatchClause>`, C8a) — each `CatchClause` is
//! itself FLAT (a `kind`/`tag`/`label` record, no inner `InstrDesc`), so it is
//! decoded element-for-element by the derived `CatchClause::from_napi_value`
//! inside the non-preallocating leaf loop.
//!
//! The generated `index.d.ts` is UNCHANGED: `build_function` keeps
//! `body: Array<InstrDesc>` via `#[napi(ts_arg_type = "Array<InstrDesc>")]` and
//! `instructions()` keeps `Array<InstrDesc>` via
//! `#[napi(ts_return_type = "Array<InstrDesc>")]` — napi-derive's typegen uses
//! those override strings verbatim (napi-derive-backend `typegen/fn.rs`), and
//! neither wrapper type is `#[napi]`, so no new type is emitted.

use std::ffi::CStr;

use napi::bindgen_prelude::{
  set_named_property_raw, FromNapiValue, ToNapiValue, TypeName, Uint8Array, ValidateNapiValue,
};
use napi::{check_status, sys, Error, Result, Status, ValueType};

use crate::ir::{nesting_too_deep, CatchClause, ConstValue, InstrDesc, MAX_NESTING_DEPTH};

/// The self-referential edge fields of [`InstrDesc`], in DECLARATION order (the
/// order the derived decode reads fields and the derived encode sets
/// properties, which both drivers preserve). Index positions are shared with
/// [`EDGE_CSTRS`] and [`write_edge`].
const EDGE_NAMES: [&str; 3] = ["seq", "consequent", "alternative"];

/// The edge property names as C strings, for the raw named-property calls
/// (`get_named_property_raw` / `set_named_property_raw` — the same calls the
/// derived impl makes for these fields).
const EDGE_CSTRS: [&CStr; 3] = [c"seq", c"consequent", c"alternative"];

/// Write a completed child sequence into its parent descriptor's edge slot.
/// `edge` indexes [`EDGE_NAMES`].
fn write_edge(parent: &mut InstrDesc, edge: usize, seq: Vec<InstrDesc>) {
  match edge {
    0 => parent.seq = Some(seq),
    1 => parent.consequent = Some(seq),
    _ => parent.alternative = Some(seq),
  }
}

// ---------------------------------------------------------------------------
// Small raw-sys helpers. Error messages deliberately match the ones napi's own
// `Array`/`Object` runtime produces for the same operations, so the observable
// failure behavior stays byte-compatible with the previous derived marshalling.
// ---------------------------------------------------------------------------

/// `napi_get_array_length` — errors (with the derived `Array::from_napi_value`
/// message) if `val` is not a JS array. This is the non-array/`null` edge and
/// non-array body rejection point, exactly as under the derived decode.
unsafe fn array_length(env: sys::napi_env, val: sys::napi_value) -> Result<u32> {
  let mut len = 0u32;
  check_status!(
    unsafe { sys::napi_get_array_length(env, val, &mut len) },
    "Failed to get Array length",
  )?;
  Ok(len)
}

/// `napi_get_element`, with the derived `Array::get` error message.
unsafe fn get_element(
  env: sys::napi_env,
  arr: sys::napi_value,
  index: u32,
) -> Result<sys::napi_value> {
  let mut ret = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_get_element(env, arr, index, &mut ret) },
    "Failed to get element with index `{}`",
    index,
  )?;
  Ok(ret)
}

/// `napi_set_element`, with the derived `Array::set` error message.
unsafe fn set_element(
  env: sys::napi_env,
  arr: sys::napi_value,
  index: u32,
  val: sys::napi_value,
) -> Result<()> {
  check_status!(
    unsafe { sys::napi_set_element(env, arr, index, val) },
    "Failed to set element with index `{}`",
    index,
  )
}

/// `napi_get_named_property` — a plain (prototype-traversing) read. Used ONLY on the
/// ENCODE side to fetch back the `catches` array we JUST wrote onto our own freshly
/// created object via the derived `InstrDesc::to_napi_value` (so it is a real own
/// property, no user JS / prototype pollution involved).
unsafe fn get_named_property(
  env: sys::napi_env,
  obj: sys::napi_value,
  name: &CStr,
) -> Result<sys::napi_value> {
  let mut ret = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_get_named_property(env, obj, name.as_ptr(), &mut ret) },
    "Failed to get property"
  )?;
  Ok(ret)
}

/// `napi_create_array_with_length`, with the derived `Array::new` message.
unsafe fn create_array(env: sys::napi_env, len: usize) -> Result<sys::napi_value> {
  let mut ptr = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_create_array_with_length(env, len, &mut ptr) },
    "Failed to create napi Array"
  )?;
  Ok(ptr)
}

/// A data-property descriptor named by a static C string (used for the edge
/// `undefined` shadows). `method`/`getter`/`setter` are `None`, so *defining* it
/// runs NO user JS — unlike a `napi_set_property`, which for an accessor or a
/// `__proto__` key would invoke a setter.
fn data_descriptor_cstr(name: &CStr, value: sys::napi_value) -> sys::napi_property_descriptor {
  sys::napi_property_descriptor {
    utf8name: name.as_ptr(),
    name: std::ptr::null_mut(),
    method: None,
    getter: None,
    setter: None,
    value,
    attributes: sys::PropertyAttributes::enumerable
      | sys::PropertyAttributes::writable
      | sys::PropertyAttributes::configurable,
    data: std::ptr::null_mut(),
  }
}

/// A data-property descriptor named by a JS string value (used to copy each of
/// the original's own non-edge properties onto the fresh object). Same
/// no-user-JS guarantee on *define* as [`data_descriptor_cstr`].
fn data_descriptor_named(
  name: sys::napi_value,
  value: sys::napi_value,
) -> sys::napi_property_descriptor {
  sys::napi_property_descriptor {
    utf8name: std::ptr::null(),
    name,
    method: None,
    getter: None,
    setter: None,
    value,
    attributes: sys::PropertyAttributes::enumerable
      | sys::PropertyAttributes::writable
      | sys::PropertyAttributes::configurable,
    data: std::ptr::null_mut(),
  }
}

/// Read an edge property that is an OWN property of `obj`. Unlike the derived
/// per-field read (`get_named_property_raw` → `napi_get_named_property`, a
/// prototype-traversing `[[Get]]`), this does NOT walk the prototype chain: an
/// inherited edge (e.g. `Object.prototype.seq` prototype pollution) is invisible
/// and treated as absent (`None`), so the driver never walks — and so never
/// re-drives the derived recursion through — an inherited edge. An own edge whose
/// value is `undefined` also yields `None`, matching the `Option` semantics the
/// prototype-traversing `get_named_property_raw` gave here before.
unsafe fn get_own_named_property(
  env: sys::napi_env,
  obj: sys::napi_value,
  name: &CStr,
) -> Result<Option<sys::napi_value>> {
  let bytes = name.to_bytes();
  let mut key = std::ptr::null_mut();
  check_status!(
    unsafe {
      sys::napi_create_string_utf8(env, bytes.as_ptr().cast(), bytes.len() as isize, &mut key)
    },
    "Failed to create property name"
  )?;
  let mut has_own = false;
  check_status!(
    unsafe { sys::napi_has_own_property(env, obj, key, &mut has_own) },
    "Failed to check own property"
  )?;
  if !has_own {
    return Ok(None);
  }
  // Own property present: read its value. Because it is OWN, the `[[Get]]` cannot
  // resolve to a prototype-chain value for this name.
  let mut ret = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_get_named_property(env, obj, name.as_ptr(), &mut ret) },
    "Failed to get property"
  )?;
  let mut ty = 0;
  check_status!(
    unsafe { sys::napi_typeof(env, ret, &mut ty) },
    "Failed to get type of property"
  )?;
  Ok(if ty == sys::ValueType::napi_undefined {
    None
  } else {
    Some(ret)
  })
}

/// Decorate `err` with the ` on InstrDesc.<field>` location suffix — the same
/// format `napi::decorate_field_error` gives derived per-field failures, so a
/// nested bad field reports the identical breadcrumb trail (e.g.
/// `... on InstrDesc.local on InstrDesc.seq`).
fn decorate(err: Error, field: &str) -> Error {
  napi::decorate_field_error(err, "InstrDesc", field)
}

/// A CATCHABLE error for an allocation that failed while growing a decode-side
/// `Vec` whose element count is bounded ONLY by an untrusted JS length (a plain
/// `Array.length`, or a `Proxy`'s `length`/`ownKeys` trap). The push-loops on
/// the decode path grow with a fallible `try_reserve` and map its
/// `TryReserveError` here, so a hostile huge-but-hole-free length exhausts memory
/// CATCHABLY (process survives) instead of an infallible `push` aborting via
/// capacity-overflow / `handle_alloc_error` — uncatchable under WASI
/// `panic=abort`, the exact abort class this module exists to remove.
fn too_large(what: &str) -> Error {
  Error::new(
    Status::GenericFailure,
    format!("{what} too large to decode"),
  )
}

/// `napi_typeof(val) == napi_undefined`. Used by [`snapshot_uint8array`] to detect
/// an out-of-range typed-array index read — a real `[[ArrayLength]]` shorter than
/// the 16 bytes we require (including a detached view, where EVERY integer index
/// reads `undefined`), and the presence of a 17th element (index 16 in range).
unsafe fn is_undefined(env: sys::napi_env, val: sys::napi_value) -> Result<bool> {
  let mut ty = 0;
  check_status!(
    unsafe { sys::napi_typeof(env, val, &mut ty) },
    "Failed to get type of value"
  )?;
  Ok(ty == sys::ValueType::napi_undefined)
}

/// Copy a decoded typed-array field into Rust-OWNED bytes by reading its 16 fixed
/// bytes THROUGH the typed array's integer-indexed `[[Get]]` (`napi_get_element`),
/// never through `napi_get_typedarray_info`'s reported `length`/`byteOffset` and
/// never by dereferencing a raw backing-store pointer.
///
/// Both call sites pass an exactly-16-byte field — `shuffle_indices` (16 lane
/// bytes, `I8x16Shuffle`) and the `V128` const's `value` (16 vector bytes) — so we
/// read EXACTLY 16 elements and reject any other length. The fixed 16 is the only
/// thing about size this function trusts; nothing here is sized from an untrusted,
/// JS-reported length.
///
/// ## Why the index read, not the reported length — spoof-proof on native AND WASI
/// napi's `Uint8Array::from_napi_value` / `napi_get_typedarray_info` read a typed
/// array's `length`/`byteOffset`. On the published emnapi (WASI) those come from
/// the ordinary, JS-SHADOWABLE `length`/`byteOffset` properties, so a caller can
/// wrap a SHORT buffer in a real `Uint8Array` whose own `length` is shadowed to
/// `>= 16`; trusting that reported length and copying via
/// `from_raw_parts(data, length)` reads PAST the real backing store — a bounded OOB
/// heap read (leaking adjacent heap into the emitted immediate, or faulting at a
/// page edge). `napi_get_element(i)` is instead the typed array's EXOTIC
/// integer-indexed access, bounded by the REAL `[[ArrayLength]]` internal slot and
/// impossible to spoof with an own `length`/`byteOffset` (native reads V8 internal
/// slots; emnapi/WASI dispatches to the host engine's genuine element `[[Get]]`):
/// * legit 16-byte input: the 16 real bytes are read in-bounds; index 16 is out of
///   range → `undefined` → accepted. Byte-identical to before for legit input.
/// * real length `< 16` (any shadowed `length`): an in-range index reads out of the
///   REAL bounds → `undefined` → catchable reject. NO OOB read.
/// * real length `> 16`: index 16 is present → catchable reject. NO silent
///   truncation of a longer array to its first 16 bytes.
///
/// ## Detach & UAF safety
/// A detached view returns `undefined` for EVERY integer index → index 0 is
/// `undefined` → catchable reject. No raw pointer is dereferenced at all, so the
/// retained-cached-pointer use-after-free the old `from_raw_parts` version guarded
/// against is eliminated at the source (strictly safer). No
/// `napi_get_typedarray_info`, no `from_raw_parts`, no napi7 `is_detached`.
///
/// Consumes `u`: `Uint8Array::to_napi_value` hands back the ORIGINAL typed array's
/// `napi_value` via its stored reference (no `[[Get]]`, no user JS runs), and taking
/// ownership here guarantees the pointer napi cached at decode time is NEVER
/// dereferenced (`u`'s `Drop` early-returns on the null ref `to_napi_value` leaves).
unsafe fn snapshot_uint8array(env: sys::napi_env, u: Uint8Array) -> Result<Uint8Array> {
  let napi_val = unsafe { Uint8Array::to_napi_value(env, u)? };
  let mut bytes: Vec<u8> = Vec::with_capacity(16); // trusted fixed size, not untrusted
  for i in 0..16u32 {
    let el = unsafe { get_element(env, napi_val, i)? };
    // An in-range index that reads `undefined` means the REAL `[[ArrayLength]]` is
    // `< 16` (incl. a detached view: every index undefined) → catchable reject,
    // NO OOB. A shadowed own `length` is irrelevant: the exotic `[[Get]]` is bounded
    // by the internal slot, so it never lets this read past the real backing store.
    if unsafe { is_undefined(env, el)? } {
      return Err(Error::from_reason("expected exactly 16 bytes"));
    }
    // A genuine typed array yields a number in `0..=255`; the status check and range
    // check are defense-in-depth (a non-number would be a catchable error, never UB).
    let mut v = 0u32;
    check_status!(
      unsafe { sys::napi_get_value_uint32(env, el, &mut v) },
      "expected a byte value"
    )?;
    if v > 0xff {
      return Err(Error::from_reason("expected a byte value in 0..=255"));
    }
    bytes.push(v as u8);
  }
  // Reject arrays LONGER than 16: index 16 must be OUT of range (`undefined`).
  // Prevents silently truncating a `> 16` typed array to its first 16 bytes.
  let el16 = unsafe { get_element(env, napi_val, 16)? };
  if !unsafe { is_undefined(env, el16)? } {
    return Err(Error::from_reason("expected exactly 16 bytes"));
  }
  Ok(Uint8Array::from(bytes)) // owned (`raw: None`), exactly 16 bytes
}

// ---------------------------------------------------------------------------
// Decode: JS `Array<InstrDesc>` -> `InstrBody` (iterative, depth-guarded).
// ---------------------------------------------------------------------------

/// `build_function`'s `body` argument: a `Vec<InstrDesc>` whose JS→Rust decode
/// is ITERATIVE (heap work-stack, O(1) call stack at any depth) with the
/// [`MAX_NESTING_DEPTH`] guard enforced during the decode itself, so an
/// over-deep body throws the deterministic catchable `nesting_too_deep()`
/// error before anything past the cap is materialized. In the generated
/// `.d.ts` it still reads `Array<InstrDesc>` (via `ts_arg_type`).
pub struct InstrBody(pub Vec<InstrDesc>);

/// Locates the parent slot a completed child sequence fills. Two shapes cross the
/// driver, but the work-stack NODES stay homogeneous (both are a `Vec<InstrDesc>`
/// decoded by the same [`DecodeFrame`]/driver/[`MAX_NESTING_DEPTH`] guard) — only
/// the WRITE-BACK target differs:
/// * [`ParentSlot::Edge`] — an `InstrDesc` self-referential edge
///   (`seq`/`consequent`/`alternative`), written via [`write_edge`].
/// * [`ParentSlot::CatchSeq`] — a LEGACY `Try` handler body,
///   `InstrDesc.catches[catch].seq` (C8b). This is the "different shape" nesting
///   two levels down (a `Vec<InstrDesc>` inside a `Vec<CatchClause>`), so it is NOT
///   an `InstrDesc` edge and is NOT in [`EDGE_NAMES`]/[`EDGE_CSTRS`] (adding `"seq"`
///   there would collide with `InstrDesc.seq`). The target slot already exists: the
///   clause was decoded with `seq == None` by [`decode_catch_clause_shallow`].
enum ParentSlot {
  Edge {
    frame: usize,
    elem: usize,
    edge: usize,
  },
  CatchSeq {
    frame: usize,
    elem: usize,
    catch: usize,
  },
}

/// One in-flight sequence being decoded. `parent` locates the slot this sequence
/// fills when complete (see [`ParentSlot`]). The root frame (the `body` argument
/// itself) has no parent and `depth == 1`, matching `validate_body(body, 1)`'s root
/// depth.
struct DecodeFrame {
  js_array: sys::napi_value,
  len: u32,
  next: u32,
  out: Vec<InstrDesc>,
  parent: Option<ParentSlot>,
  depth: usize,
}

/// Decode ONE array element via the shallow-copy-except-edges trick: onto a
/// fresh empty object, shadow the three edge names AND the leaf `Vec` fields
/// (`labels`, `catches`) as own `undefined` and copy every OWN enumerable
/// non-edge, non-leaf-`Vec` property of `elem`, then run the DERIVED
/// `InstrDesc::from_napi_value` on that copy and finally decode `labels` and
/// `catches` ourselves as leaves. The derived impl reads every field via a
/// prototype-traversing `[[Get]]`, so both steps are hardened against an
/// adversarial prototype chain:
///
/// * The edge shadows make the derived read of `seq`/`consequent`/`alternative`
///   find an own `undefined` FIRST — so a polluted `Object.prototype.seq` (etc.)
///   can never be read and recursed into (`Vec<InstrDesc>::from_napi_value`) on
///   the native call stack. That inherited-edge recursion is an UNCATCHABLE stack
///   overflow, exactly the abort class this module exists to remove; the shadow
///   closes it on ANY prototype chain, and the driver still walks the real edges.
/// * The `labels`/`catches` shadows make the derived read of those leaf `Vec`
///   fields yield `None` on any prototype chain, so the untrusted-length
///   `Vec::<_>::with_capacity` in the derived `Vec` decode is never reached (own
///   OR inherited sparse `labels`/`catches`). The real OWN `labels`/`catches` are
///   then decoded below by non-preallocating loops.
/// * Copying is OWN-only and via `napi_define_property` (a data descriptor), so
///   no inherited property is smuggled onto the copy, no accessor/setter runs,
///   and an own `"__proto__"` key can never retarget the copy's prototype.
///
/// The copy carries no edges of its own, so the derived impl cannot recurse;
/// every plain field keeps its derived decode + error behavior and automatically
/// tracks future field additions. The user's `elem` is never mutated.
///
/// Returns the decoded [`InstrDesc`] AND — captured in the SAME single pass over
/// `elem.catches` — the OWN `seq` handle of each legacy catch clause that has one,
/// as `(clause index, seq napi_value)`. The driver enqueues the legacy handler
/// bodies (`catches[ci].seq`) from THESE captured handles, so `elem.catches` (and
/// each clause) is read EXACTLY ONCE: a `Proxy`/own-getter returning a different
/// array or different clause objects on a second read can never splice one clause's
/// `kind`/`tag`/`blockType` with another clause's handler body (the snapshot-once
/// rule, the TOCTOU class CH-fix3 closed for typed arrays). The capture is OWN-only
/// (`get_own_named_property`), so an inherited `Object.prototype.seq` is ignored —
/// in lockstep with `decode_catch_clause_shallow`'s own-`undefined` shadow.
unsafe fn decode_element_shallow(
  env: sys::napi_env,
  elem: sys::napi_value,
) -> Result<(InstrDesc, Vec<(usize, sys::napi_value)>)> {
  let mut copy = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_create_object(env, &mut copy) },
    "Failed to create napi Object"
  )?;

  // Shadow each edge as an OWN `undefined` data property (built first so the
  // derived prototype-traversing read of that edge resolves to the own
  // `undefined` regardless of the input's prototype chain). All properties are
  // installed in ONE `napi_define_properties` call at the end.
  let mut undefined = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_get_undefined(env, &mut undefined) },
    "Failed to get undefined"
  )?;
  let mut descriptors: Vec<sys::napi_property_descriptor> = Vec::new();
  for cname in EDGE_CSTRS {
    descriptors.push(data_descriptor_cstr(cname, undefined));
  }
  // `labels` is a LEAF `Vec<u32>` (a `BrTable`'s target list) — NOT a recursive
  // edge, so it is NOT walked by the frame driver — but it has the SAME
  // untrusted-length prealloc abort as the edges: the derived
  // `Vec::<u32>::from_napi_value` calls `Vec::with_capacity(labels.length)`
  // BEFORE inspecting any element, so a sparse `labels.length ≈ 2**32` (own OR
  // inherited via a polluted `Object.prototype.labels`) requests billions of
  // slots → capacity-overflow panic / `handle_alloc_error` → uncatchable abort.
  // Shadow it as an own `undefined` (so the derived read yields `None` on ANY
  // prototype chain) and decode the original's OWN `labels` ourselves below with
  // a NON-preallocating loop.
  descriptors.push(data_descriptor_cstr(c"labels", undefined));
  // `catches` is the SAME shape of hazard as `labels` — a leaf `Vec<CatchClause>`
  // (a `TryTable`'s clause list), NOT a recursive edge (a `CatchClause` is flat:
  // no inner `InstrDesc`), but the derived `Vec::<CatchClause>::from_napi_value`
  // still calls `Vec::with_capacity(catches.length)` from the untrusted JS length
  // BEFORE inspecting any element, so a sparse `catches.length ≈ 2**32` (own OR
  // inherited via `Object.prototype.catches`) aborts. Shadow it as own `undefined`
  // and decode the original's OWN `catches` ourselves below (non-preallocating).
  descriptors.push(data_descriptor_cstr(c"catches", undefined));

  // Enumerate the ORIGINAL's OWN enumerable string property names only (never the
  // prototype chain, never symbols). On a `null`/`undefined` element this raises
  // the same pending TypeError the derived decode's first read raised ("Cannot
  // convert undefined or null to object").
  let mut names = std::ptr::null_mut();
  check_status!(
    unsafe {
      sys::napi_get_all_property_names(
        env,
        elem,
        sys::KeyCollectionMode::own_only,
        sys::KeyFilter::enumerable | sys::KeyFilter::skip_symbols,
        sys::KeyConversion::numbers_to_strings,
        &mut names,
      )
    },
    "Failed to get property names of given object"
  )?;
  let names_len = unsafe { array_length(env, names)? };

  // A property name only needs enough buffer to distinguish it from the edge
  // names (max 11 bytes, "alternative"), "labels" (6 bytes), "catches" (7 bytes),
  // and "__proto__" (9 bytes): a name that fills the buffer is longer than any of
  // those and is copied without further inspection.
  let mut buf = [0u8; 16];
  for i in 0..names_len {
    let name = unsafe { get_element(env, names, i)? };
    let mut written = 0usize;
    check_status!(
      unsafe {
        sys::napi_get_value_string_utf8(env, name, buf.as_mut_ptr().cast(), buf.len(), &mut written)
      },
      "Failed to read property name"
    )?;
    if written < buf.len() - 1 {
      let nm = &buf[..written];
      // Edges keep their own-`undefined` shadow (the driver walks the real ones);
      // `labels`/`catches` keep their own-`undefined` shadow too (decoded as
      // leaves below, never via the untrusted-length derived `Vec` prealloc);
      // `"__proto__"` is dropped so it can never retarget the copy's prototype.
      if EDGE_NAMES.iter().any(|e| e.as_bytes() == nm)
        || nm == b"labels"
        || nm == b"catches"
        || nm == b"__proto__"
      {
        continue;
      }
    }
    // Read the OWN value (own property => this `[[Get]]` cannot resolve to a
    // prototype-chain value for this name) and DEFINE it onto the copy as an own
    // data property.
    let mut val = std::ptr::null_mut();
    check_status!(
      unsafe { sys::napi_get_property(env, elem, name, &mut val) },
      "Failed to get property"
    )?;
    // FALLIBLE growth: `names_len` is `napi_get_all_property_names`' count, driven
    // by the element's own keys — a `Proxy` `ownKeys` trap can inflate it. Grow
    // `descriptors` with `try_reserve` so an exhausting key count is CATCHABLE,
    // not an infallible `push` abort.
    descriptors
      .try_reserve(1)
      .map_err(|_| too_large("object property set"))?;
    descriptors.push(data_descriptor_named(name, val));
  }

  check_status!(
    unsafe { sys::napi_define_properties(env, copy, descriptors.len(), descriptors.as_ptr()) },
    "Failed to define properties"
  )?;

  let mut desc = unsafe { InstrDesc::from_napi_value(env, copy)? };

  // SNAPSHOT every JS-backed typed-array field of `desc` into Rust-OWNED bytes.
  // `snapshot_uint8array` reads the 16 fixed bytes THROUGH the typed array's
  // integer-indexed `[[Get]]` (`napi_get_element`) — bounded by the REAL
  // `[[ArrayLength]]` internal slot, so it is spoof-proof against a shadowed own
  // `length`/`byteOffset` (the round-4 OOB) on native AND emnapi/WASI, and it
  // dereferences NO cached backing-store pointer, so the retained-pointer UAF a
  // later `labels`/edge/sibling getter or child `Proxy` trap could cause (by
  // detaching the buffer) is eliminated at the source — a detached view simply
  // reads `undefined` at index 0 and degrades to a catchable length error.
  //
  // These two are the ONLY pointer-retaining fields: a decoded `InstrDesc` has
  // exactly two `Uint8Array` fields (`shuffle_indices` and the `V128` const's
  // `value`); every other field is a scalar / `String` / `BigInt` / owned enum
  // decoded to owned Rust values. ANY future typed-array (or other pointer-
  // retaining) field MUST be snapshotted here too.
  if let Some(u) = desc.shuffle_indices.take() {
    desc.shuffle_indices =
      Some(unsafe { snapshot_uint8array(env, u) }.map_err(|e| decorate(e, "shuffleIndices"))?);
  }
  if let Some(ConstValue::V128 { value }) = &mut desc.value {
    let taken = std::mem::replace(value, Uint8Array::new(Vec::new()));
    *value = unsafe { snapshot_uint8array(env, taken) }.map_err(|e| decorate(e, "value"))?;
  }

  // Decode the LEAF `labels: Option<Vec<u32>>` ourselves, from the ORIGINAL's
  // OWN `labels` only. `get_own_named_property` never traverses the prototype, so
  // an inherited `Object.prototype.labels` is ignored (stays `None`), in lockstep
  // with the own-`undefined` shadow that already made the derived read of the
  // copy yield `None`. The element loop is NON-preallocating (`Vec::new()` +
  // push), so a sparse-wide `labels.length` never reaches `Vec::with_capacity`:
  // it fails CATCHABLY on its first hole (a `u32` conversion / array error — the
  // SAME error the derived `Vec::<u32>` decode surfaces), never a panic/abort. A
  // normal small `labels` decodes element-for-element, byte-identical to the
  // derived path. Absent/`undefined` own `labels` leaves `desc.labels` at `None`.
  if let Some(arr) = unsafe { get_own_named_property(env, elem, c"labels")? } {
    let len = unsafe { array_length(env, arr) }.map_err(|e| decorate(e, "labels"))?;
    let mut labels: Vec<u32> = Vec::new();
    for i in 0..len {
      let el = unsafe { get_element(env, arr, i) }.map_err(|e| decorate(e, "labels"))?;
      let n = unsafe { u32::from_napi_value(env, el) }.map_err(|e| decorate(e, "labels"))?;
      // FALLIBLE growth: `len` is the untrusted `Array.length` of `arr`, which a
      // `Proxy` returning a valid `u32` for EVERY numeric index (no hole to fail
      // on) can report as ~`2**32`. `try_reserve` maps that exhaustion to a
      // CATCHABLE error rather than an infallible `push` aborting the process.
      labels
        .try_reserve(1)
        .map_err(|_| decorate(too_large("branch table"), "labels"))?;
      labels.push(n);
    }
    desc.labels = Some(labels);
  }

  // Decode the LEAF `catches: Option<Vec<CatchClause>>` ourselves, from the
  // ORIGINAL's OWN `catches` only — the SAME non-preallocating pattern as
  // `labels`, so a sparse-wide `catches.length` (own OR inherited via
  // `Object.prototype.catches`) never reaches `Vec::<CatchClause>::with_capacity`:
  // it fails CATCHABLY on its first hole (a `CatchClause` object decode error, the
  // SAME error the derived `Vec` decode surfaces), never a panic/abort.
  // `get_own_named_property` never traverses the prototype, so an inherited
  // `Object.prototype.catches` is ignored (stays `None`), in lockstep with the
  // own-`undefined` shadow. Absent/`undefined` own `catches` leaves it at `None`.
  //
  // Each clause is decoded via `decode_catch_clause_shallow`, NOT the derived
  // `CatchClause::from_napi_value`: a clause's LEGACY handler body `seq`
  // (`Option<Vec<InstrDesc>>`, C8b) would otherwise recurse into
  // `Vec::<InstrDesc>::from_napi_value` on the CALL STACK (the uncatchable
  // stack-overflow abort this module removes) AND prealloc from an untrusted
  // `seq.length`. The shallow decode shadows the clause's `seq` as own `undefined`
  // so the derived read yields `None`; the DRIVER later fills each clause's `seq`
  // from the OWN `seq` handle CAPTURED HERE, in this SAME pass, as a
  // `ParentSlot::CatchSeq` child frame — `elem.catches` and each clause are read
  // EXACTLY ONCE (snapshot-once: no re-read the driver could observe a `Proxy`
  // mutate between). Capturing the handle here is OWN-only, keeping the
  // inherited-`seq`-ignored semantics in lockstep with the shadow.
  let mut catch_seq_handles: Vec<(usize, sys::napi_value)> = Vec::new();
  if let Some(arr) = unsafe { get_own_named_property(env, elem, c"catches")? } {
    let len = unsafe { array_length(env, arr) }.map_err(|e| decorate(e, "catches"))?;
    let mut catches: Vec<CatchClause> = Vec::new();
    for i in 0..len {
      let el = unsafe { get_element(env, arr, i) }.map_err(|e| decorate(e, "catches"))?;
      let clause =
        unsafe { decode_catch_clause_shallow(env, el) }.map_err(|e| decorate(e, "catches"))?;
      // FALLIBLE growth: `len` is the untrusted `Array.length` of `arr`. A `Proxy`
      // returning a valid `CatchClause` for EVERY numeric index (no hole to fail
      // on) can report ~`2**32`; `try_reserve` maps that exhaustion to a CATCHABLE
      // error rather than an infallible `push` aborting the process.
      catches
        .try_reserve(1)
        .map_err(|_| decorate(too_large("catch clause list"), "catches"))?;
      catches.push(clause);
      // Capture this clause's OWN `seq` handle NOW (same pass, from the SAME `el`
      // we already decoded), so the driver never re-reads `elem.catches`. The index
      // `i` is the clause's position in `desc.catches` (we push every clause in
      // order). FALLIBLE growth again: the handle count is bounded only by the
      // caller-controlled `catches` length.
      if let Some(seq_handle) =
        unsafe { get_own_named_property(env, el, c"seq") }.map_err(|e| decorate(e, "catches"))?
      {
        catch_seq_handles
          .try_reserve(1)
          .map_err(|_| decorate(too_large("catch handler list"), "catches"))?;
        catch_seq_handles.push((i as usize, seq_handle));
      }
    }
    desc.catches = Some(catches);
  }

  Ok((desc, catch_seq_handles))
}

/// Decode ONE `CatchClause` element with its LEGACY handler-body `seq` shadowed as
/// own `undefined`, the [`decode_element_shallow`] trick applied to a clause. A
/// `CatchClause` gained a self-referential `seq: Option<Vec<InstrDesc>>` in C8b (the
/// legacy `Try` handler body); the derived `CatchClause::from_napi_value` reads it
/// via a prototype-traversing `[[Get]]`, which would (1) recurse into
/// `Vec::<InstrDesc>::from_napi_value` on the CALL STACK — the uncatchable
/// stack-overflow abort this module exists to remove, reachable via an inherited
/// `Object.prototype.seq` — and (2) prealloc from an untrusted `seq.length`. So we
/// shadow `seq` as an own `undefined` (the derived read yields `None` on ANY
/// prototype chain) and copy every OTHER own enumerable property, dropping
/// `"__proto__"` so it can never retarget the copy's prototype. The returned clause
/// has `seq == None`; the DRIVER fills it from the ORIGINAL clause's OWN `seq` as a
/// `ParentSlot::CatchSeq` child frame (own-only, so an inherited `seq` is ignored —
/// in lockstep with this shadow).
///
/// Every OTHER field (`kind`/`tag`/`label`/`relativeDepth`/`blockType`) is a scalar
/// or small fixed record with NO untrusted-length hazard, so it keeps its derived
/// decode (and automatically tracks future flat-field additions). The user's `el`
/// is never mutated.
unsafe fn decode_catch_clause_shallow(
  env: sys::napi_env,
  el: sys::napi_value,
) -> Result<CatchClause> {
  let mut copy = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_create_object(env, &mut copy) },
    "Failed to create napi Object"
  )?;

  let mut undefined = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_get_undefined(env, &mut undefined) },
    "Failed to get undefined"
  )?;
  // Shadow the single self-referential edge `seq` (built first so the derived
  // prototype-traversing read resolves to this own `undefined` regardless of the
  // clause's prototype chain). All properties are installed in ONE
  // `napi_define_properties` call at the end.
  let mut descriptors: Vec<sys::napi_property_descriptor> =
    vec![data_descriptor_cstr(c"seq", undefined)];

  // Enumerate the ORIGINAL clause's OWN enumerable string property names only. On a
  // `null`/`undefined` element this raises the same pending TypeError the derived
  // decode's first read raised.
  let mut names = std::ptr::null_mut();
  check_status!(
    unsafe {
      sys::napi_get_all_property_names(
        env,
        el,
        sys::KeyCollectionMode::own_only,
        sys::KeyFilter::enumerable | sys::KeyFilter::skip_symbols,
        sys::KeyConversion::numbers_to_strings,
        &mut names,
      )
    },
    "Failed to get property names of given object"
  )?;
  let names_len = unsafe { array_length(env, names)? };

  // A name only needs enough buffer to distinguish it from `"seq"` (3 bytes) and
  // `"__proto__"` (9 bytes): a name that fills the buffer is longer than either and
  // is copied without further inspection.
  let mut buf = [0u8; 16];
  for i in 0..names_len {
    let name = unsafe { get_element(env, names, i)? };
    let mut written = 0usize;
    check_status!(
      unsafe {
        sys::napi_get_value_string_utf8(env, name, buf.as_mut_ptr().cast(), buf.len(), &mut written)
      },
      "Failed to read property name"
    )?;
    if written < buf.len() - 1 {
      let nm = &buf[..written];
      // `seq` keeps its own-`undefined` shadow (the driver fills the real one);
      // `"__proto__"` is dropped so it can never retarget the copy's prototype.
      if nm == b"seq" || nm == b"__proto__" {
        continue;
      }
    }
    let mut val = std::ptr::null_mut();
    check_status!(
      unsafe { sys::napi_get_property(env, el, name, &mut val) },
      "Failed to get property"
    )?;
    // FALLIBLE growth: a `Proxy` `ownKeys` trap can inflate `names_len`.
    descriptors
      .try_reserve(1)
      .map_err(|_| too_large("object property set"))?;
    descriptors.push(data_descriptor_named(name, val));
  }

  check_status!(
    unsafe { sys::napi_define_properties(env, copy, descriptors.len(), descriptors.as_ptr()) },
    "Failed to define properties"
  )?;

  // The copy carries no `seq` of its own (own `undefined`), so the derived decode
  // reads `seq == None` and cannot recurse; every flat field keeps its derived
  // decode + error behavior.
  unsafe { CatchClause::from_napi_value(env, copy) }
}

impl FromNapiValue for InstrBody {
  unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
    // The ancestry decoration below re-creates the derived error breadcrumbs:
    // an error inside a nested sequence bubbles up through each ancestor edge
    // as ` on InstrDesc.<edge>` (innermost first, root undecorated).
    let decorate_ancestors = |mut err: Error, frames: &[DecodeFrame], mut at: usize| -> Error {
      loop {
        match frames[at].parent {
          Some(ParentSlot::Edge { frame, edge, .. }) => {
            err = decorate(err, EDGE_NAMES[edge]);
            at = frame;
          }
          // A legacy handler body is `InstrDesc.catches[..].seq`; surface the
          // `catches` breadcrumb (the field on the parent `InstrDesc`).
          Some(ParentSlot::CatchSeq { frame, .. }) => {
            err = decorate(err, "catches");
            at = frame;
          }
          None => return err,
        }
      }
    };

    let root_len = unsafe { array_length(env, napi_val)? };
    // `out` is NOT pre-sized from `root_len`: the JS `Array.length` is untrusted,
    // and a sparse array can report a length near `2**32` with few/no real
    // elements. `Vec::with_capacity(root_len)` on such a length aborts the process
    // (capacity-overflow panic / `handle_alloc_error`) — uncatchable, especially
    // under WASI `panic=abort`. The push-loop below grows `out` to the ACTUAL
    // element count; a sparse hole fails catchably in `decode_element_shallow`.
    let mut frames = vec![DecodeFrame {
      js_array: napi_val,
      len: root_len,
      next: 0,
      out: Vec::new(),
      parent: None,
      depth: 1,
    }];

    loop {
      let f = frames.len() - 1;
      let frame = &mut frames[f];

      if frame.next >= frame.len {
        // Sequence complete: pop it and write it into its parent's edge slot.
        // A present-but-empty JS array stays `Some(vec![])` (this write), an
        // absent/undefined edge was simply never pushed and stays `None` —
        // the derived `Option<Vec<_>>` distinction, preserved exactly.
        let done = frames.pop().expect("work stack is non-empty");
        match done.parent {
          None => return Ok(InstrBody(done.out)),
          Some(ParentSlot::Edge { frame, elem, edge }) => {
            write_edge(&mut frames[frame].out[elem], edge, done.out)
          }
          // Write the completed legacy handler body into its clause's `seq`. The
          // clause exists (populated with `seq == None` by
          // `decode_catch_clause_shallow` before any child frame completes).
          Some(ParentSlot::CatchSeq { frame, elem, catch }) => {
            frames[frame].out[elem]
              .catches
              .as_mut()
              .expect("catches present for a CatchSeq parent")[catch]
              .seq = Some(done.out);
          }
        }
        continue;
      }

      let index = frame.next;
      frame.next += 1;
      let js_array = frame.js_array;
      let depth = frame.depth;

      let elem = unsafe { get_element(env, js_array, index) }
        .map_err(|e| decorate_ancestors(e, &frames, f))?;
      // `catch_seq_handles` are the OWN `seq` handles of the legacy catch clauses,
      // captured in the SAME single pass that decoded them (snapshot-once — the
      // driver never re-reads `elem.catches`).
      let (desc, catch_seq_handles) = unsafe { decode_element_shallow(env, elem) }
        .map_err(|e| decorate_ancestors(e, &frames, f))?;
      // FALLIBLE growth: `out` grows to the ACTUAL element count of a frame whose
      // `len` is the untrusted `Array.length` (root body or an edge sequence). A
      // `Proxy` returning a valid descriptor for EVERY index (no hole to fail on)
      // could drive this toward ~`2**32` elements; `try_reserve` maps that
      // exhaustion to a CATCHABLE error instead of an infallible `push` abort.
      frames[f]
        .out
        .try_reserve(1)
        .map_err(|_| decorate_ancestors(too_large("instruction sequence"), &frames, f))?;
      frames[f].out.push(desc);
      let elem_idx = frames[f].out.len() - 1;

      // Walk the OWN edges on the ORIGINAL element, in declaration order. The
      // read is OWN-only (`get_own_named_property`): an INHERITED edge (prototype
      // pollution) is treated as absent, so an inherited `seq`/`consequent`/
      // `alternative` is ignored exactly like the derived read's own-`undefined`
      // shadow ignores it — the two stay in lockstep.
      for (edge, &cname) in EDGE_CSTRS.iter().enumerate() {
        let raw = unsafe { get_own_named_property(env, elem, cname) }
          .map_err(|e| decorate_ancestors(e, &frames, f))?;
        let Some(raw) = raw else {
          continue; // absent/inherited/undefined edge stays `None`
        };
        // Depth-guard BEFORE materializing the child — descending counts even
        // for an empty (or non-array) child, so ANY body nested past the cap
        // fails with EXACTLY this error, deterministically, on every target.
        if depth + 1 > MAX_NESTING_DEPTH {
          return Err(nesting_too_deep());
        }
        // A non-array (including `null`) edge fails here with the same
        // decorated error the derived decode produced for it.
        let child_len = unsafe { array_length(env, raw) }
          .map_err(|e| decorate_ancestors(decorate(e, EDGE_NAMES[edge]), &frames, f))?;
        // `out` is NOT pre-sized from `child_len` — same untrusted-`Array.length`
        // abort hazard as the root frame; the push-loop grows it to the actual
        // element count, and a sparse-wide edge fails catchably on its first hole.
        frames.push(DecodeFrame {
          js_array: raw,
          len: child_len,
          next: 0,
          out: Vec::new(),
          parent: Some(ParentSlot::Edge {
            frame: f,
            elem: elem_idx,
            edge,
          }),
          depth: depth + 1,
        });
      }

      // Second child-enqueue walk (C8b): the LEGACY `Try` handler bodies, which are
      // `InstrDesc.catches[ci].seq` — a `Vec<InstrDesc>` two levels down. The clauses
      // were decoded with `seq == None` (`decode_catch_clause_shallow` shadowed it),
      // and their OWN `seq` handles were CAPTURED in the same single pass (snapshot-
      // once — no re-read of `elem.catches` here). Enqueue each as a `CatchSeq` child
      // frame at `depth + 1` (handlers are SIBLINGS of the try body — same depth); the
      // write-back slot exists because `desc.catches` already holds the clause at `ci`
      // with `seq == None`.
      for (ci, seq_handle) in catch_seq_handles {
        // Depth-guard BEFORE materializing the child (handler bodies enter at
        // `depth + 1`, matching the try body — they are siblings).
        if depth + 1 > MAX_NESTING_DEPTH {
          return Err(nesting_too_deep());
        }
        // A non-array (incl. `null`) handler `seq` fails here catchably, decorated
        // like the derived decode. `out` is NOT pre-sized from `child_len`, so a
        // sparse-wide `catches[].seq` fails catchably on its first hole, never a
        // huge prealloc/abort.
        let child_len = unsafe { array_length(env, seq_handle) }
          .map_err(|e| decorate_ancestors(decorate(e, "catches"), &frames, f))?;
        // FALLIBLE frame growth: unlike the 3 fixed edges, the catch fan-out is
        // caller-controlled and UNBOUNDED (one `Try` can carry a huge clause list,
        // each with an own `seq`), so an infallible `push` could abort under memory
        // pressure — `try_reserve` maps that to a CATCHABLE error. The mutable borrow
        // ends before the immutable `decorate_ancestors` borrow of `frames` begins.
        if frames.try_reserve(1).is_err() {
          return Err(decorate_ancestors(
            decorate(too_large("catch handler list"), "catches"),
            &frames,
            f,
          ));
        }
        frames.push(DecodeFrame {
          js_array: seq_handle,
          len: child_len,
          next: 0,
          out: Vec::new(),
          parent: Some(ParentSlot::CatchSeq {
            frame: f,
            elem: elem_idx,
            catch: ci,
          }),
          depth: depth + 1,
        });
      }
    }
  }
}

impl TypeName for InstrBody {
  fn type_name() -> &'static str {
    "Array<InstrDesc>"
  }

  fn value_type() -> ValueType {
    ValueType::Object
  }
}

impl ValidateNapiValue for InstrBody {
  unsafe fn validate(env: sys::napi_env, napi_val: sys::napi_value) -> Result<sys::napi_value> {
    // Mirrors `Vec<T>`'s validate: reject a non-array eagerly.
    let mut is_array = false;
    check_status!(
      unsafe { sys::napi_is_array(env, napi_val, &mut is_array) },
      "Failed to check given napi value is array"
    )?;
    if !is_array {
      return Err(Error::new(
        Status::InvalidArg,
        "Expected an array".to_owned(),
      ));
    }
    Ok(std::ptr::null_mut())
  }
}

// ---------------------------------------------------------------------------
// Encode: `InstrList` -> JS `Array<InstrDesc>` (iterative).
// ---------------------------------------------------------------------------

/// `instructions()`'s return value: a `Vec<InstrDesc>` whose Rust→JS encode is
/// ITERATIVE (heap work-stack, O(1) call stack at any depth). The read walk
/// that produces it is depth-capped, so no guard is needed here — the driver
/// simply never recurses. In the generated `.d.ts` it still reads
/// `Array<InstrDesc>` (via `ts_return_type`).
pub struct InstrList(pub Vec<InstrDesc>);

/// One in-flight sequence being encoded into `js_array`. Consuming the items
/// through the iterator (with the edges taken out per element) dismantles the
/// descriptor tree as it is encoded, so no deep `Drop` recursion happens on
/// this path either.
struct EncodeFrame {
  js_array: sys::napi_value,
  items: std::vec::IntoIter<InstrDesc>,
  next: u32,
}

impl ToNapiValue for InstrList {
  unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
    let root = unsafe { create_array(env, val.0.len())? };
    let mut frames = vec![EncodeFrame {
      js_array: root,
      items: val.0.into_iter(),
      next: 0,
    }];

    loop {
      let f = frames.len() - 1;
      let frame = &mut frames[f];

      let Some(mut desc) = frame.items.next() else {
        frames.pop();
        if frames.is_empty() {
          return Ok(root);
        }
        continue;
      };
      let index = frame.next;
      frame.next += 1;
      let js_array = frame.js_array;

      // Take each LEGACY `Try` handler body (`catches[ci].seq`) OUT before the
      // derived encode, so the derived `CatchClause::to_napi_value` sets no `seq`
      // property (`None` field => absent) and CANNOT recurse into
      // `Vec::<InstrDesc>::to_napi_value` on the call stack. Symmetric with the
      // `ParentSlot::CatchSeq` decode. Each `(ci, body)` is re-attached below onto
      // the corresponding derived-encoded clause object, then queued as a child
      // frame — so a handler body of ANY depth is dismantled iteratively, never a
      // deep `to_napi_value`/`Drop` recursion (the same guarantee as the edges).
      let mut catch_seqs: Vec<(usize, Vec<InstrDesc>)> = Vec::new();
      if let Some(catches) = desc.catches.as_mut() {
        for (ci, clause) in catches.iter_mut().enumerate() {
          if let Some(s) = clause.seq.take() {
            // FALLIBLE growth: the clause count is bounded here (a body read from
            // walrus), but keep it symmetric with the decode's catch fan-out so an
            // infallible `push` can never abort.
            catch_seqs
              .try_reserve(1)
              .map_err(|_| too_large("catch handler list"))?;
            catch_seqs.push((ci, s));
          }
        }
      }

      // Strip the edges so the DERIVED encode cannot recurse. With the edges
      // `None`, the derived `#[napi(object)]` impl sets no property for them —
      // `None` edges therefore stay ABSENT on the JS object, exactly as before.
      let seq = desc.seq.take();
      let consequent = desc.consequent.take();
      let alternative = desc.alternative.take();

      let obj = unsafe { InstrDesc::to_napi_value(env, desc)? };
      unsafe { set_element(env, js_array, index, obj)? };

      // Attach each present edge's (pre-created) JS array in declaration order
      // — the property insertion order the derived encode produced — and queue
      // its contents as a child frame. `Some(vec![])` attaches an empty array,
      // preserving the `Some(empty)` vs `None` distinction.
      for (edge, vec) in [seq, consequent, alternative].into_iter().enumerate() {
        let Some(vec) = vec else { continue };
        let child = unsafe { create_array(env, vec.len())? };
        // The same call the derived conditional setter makes for a `Some` edge.
        unsafe { set_named_property_raw(env, obj, EDGE_CSTRS[edge].as_ptr(), child)? };
        frames.push(EncodeFrame {
          js_array: child,
          items: vec.into_iter(),
          next: 0,
        });
      }

      // Attach each taken legacy handler body onto its clause object (fetched back
      // from the derived-encoded `catches` array) and queue it as a child frame.
      if !catch_seqs.is_empty() {
        let catches_arr = unsafe { get_named_property(env, obj, c"catches")? };
        for (ci, body) in catch_seqs {
          let clause_obj = unsafe { get_element(env, catches_arr, ci as u32)? };
          let child = unsafe { create_array(env, body.len())? };
          unsafe { set_named_property_raw(env, clause_obj, c"seq".as_ptr(), child)? };
          // FALLIBLE frame growth, symmetric with the decode's catch fan-out.
          frames
            .try_reserve(1)
            .map_err(|_| too_large("catch handler list"))?;
          frames.push(EncodeFrame {
            js_array: child,
            items: body.into_iter(),
            next: 0,
          });
        }
      }
    }
  }
}

impl TypeName for InstrList {
  fn type_name() -> &'static str {
    "Array<InstrDesc>"
  }

  fn value_type() -> ValueType {
    ValueType::Object
  }
}
