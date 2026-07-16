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
//! ## MAINTENANCE: adding a self-referential field (e.g. C8b `catches[].seq`)
//! ANY future `InstrDesc` field that can contain `InstrDesc`s MUST be routed
//! through BOTH drivers or the derived per-element call will recurse again:
//! * decode: extend [`EDGE_NAMES`]/[`EDGE_CSTRS`] (the copy-except skip list
//!   AND the edge walk in `InstrBody::from_napi_value`) and [`write_edge`];
//! * encode: extend the edge take/attach list in `InstrList::to_napi_value`.
//!
//! A field with a DIFFERENT shape (e.g. a struct wrapping a `Vec<InstrDesc>`)
//! additionally needs its own frame bookkeeping in both drivers.
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
//! edge lists. Today the only such field is `labels`; `catches` (C8a) is next.
//!
//! The generated `index.d.ts` is UNCHANGED: `build_function` keeps
//! `body: Array<InstrDesc>` via `#[napi(ts_arg_type = "Array<InstrDesc>")]` and
//! `instructions()` keeps `Array<InstrDesc>` via
//! `#[napi(ts_return_type = "Array<InstrDesc>")]` — napi-derive's typegen uses
//! those override strings verbatim (napi-derive-backend `typegen/fn.rs`), and
//! neither wrapper type is `#[napi]`, so no new type is emitted.

use std::ffi::CStr;

use napi::bindgen_prelude::{
  set_named_property_raw, FromNapiValue, ToNapiValue, TypeName, ValidateNapiValue,
};
use napi::{check_status, sys, Error, Result, Status, ValueType};

use crate::ir::{nesting_too_deep, InstrDesc, MAX_NESTING_DEPTH};

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

/// One in-flight sequence being decoded. `parent` locates the edge slot this
/// sequence fills when complete: `(frame index, element index, edge index)`.
/// The root frame (the `body` argument itself) has no parent and `depth == 1`,
/// matching `validate_body(body, 1)`'s root depth.
struct DecodeFrame {
  js_array: sys::napi_value,
  len: u32,
  next: u32,
  out: Vec<InstrDesc>,
  parent: Option<(usize, usize, usize)>,
  depth: usize,
}

/// Decode ONE array element via the shallow-copy-except-edges trick: onto a
/// fresh empty object, shadow the three edge names AND `labels` as own
/// `undefined` and copy every OWN enumerable non-edge, non-`labels` property of
/// `elem`, then run the DERIVED `InstrDesc::from_napi_value` on that copy and
/// finally decode `labels` ourselves as a leaf. The derived impl reads every
/// field via a prototype-traversing `[[Get]]`, so both steps are hardened
/// against an adversarial prototype chain:
///
/// * The edge shadows make the derived read of `seq`/`consequent`/`alternative`
///   find an own `undefined` FIRST — so a polluted `Object.prototype.seq` (etc.)
///   can never be read and recursed into (`Vec<InstrDesc>::from_napi_value`) on
///   the native call stack. That inherited-edge recursion is an UNCATCHABLE stack
///   overflow, exactly the abort class this module exists to remove; the shadow
///   closes it on ANY prototype chain, and the driver still walks the real edges.
/// * The `labels` shadow makes the derived read of `labels` yield `None` on any
///   prototype chain, so the untrusted-length `Vec::<u32>::with_capacity` in the
///   derived `Vec` decode is never reached (own OR inherited sparse `labels`).
///   The real OWN `labels` is then decoded below by a non-preallocating loop.
/// * Copying is OWN-only and via `napi_define_property` (a data descriptor), so
///   no inherited property is smuggled onto the copy, no accessor/setter runs,
///   and an own `"__proto__"` key can never retarget the copy's prototype.
///
/// The copy carries no edges of its own, so the derived impl cannot recurse;
/// every plain field keeps its derived decode + error behavior and automatically
/// tracks future field additions. The user's `elem` is never mutated.
unsafe fn decode_element_shallow(env: sys::napi_env, elem: sys::napi_value) -> Result<InstrDesc> {
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
  // names (max 11 bytes, "alternative"), "labels" (6 bytes), and "__proto__"
  // (9 bytes): a name that fills the buffer is longer than any of those and is
  // copied without further inspection.
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
      // `labels` keeps its own-`undefined` shadow too (decoded as a leaf below,
      // never via the untrusted-length derived `Vec` prealloc); `"__proto__"` is
      // dropped so it can never retarget the copy's prototype.
      if EDGE_NAMES.iter().any(|e| e.as_bytes() == nm) || nm == b"labels" || nm == b"__proto__" {
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
    descriptors.push(data_descriptor_named(name, val));
  }

  check_status!(
    unsafe { sys::napi_define_properties(env, copy, descriptors.len(), descriptors.as_ptr()) },
    "Failed to define properties"
  )?;

  let mut desc = unsafe { InstrDesc::from_napi_value(env, copy)? };

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
      labels.push(n);
    }
    desc.labels = Some(labels);
  }

  Ok(desc)
}

impl FromNapiValue for InstrBody {
  unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
    // The ancestry decoration below re-creates the derived error breadcrumbs:
    // an error inside a nested sequence bubbles up through each ancestor edge
    // as ` on InstrDesc.<edge>` (innermost first, root undecorated).
    let decorate_ancestors = |mut err: Error, frames: &[DecodeFrame], mut at: usize| -> Error {
      loop {
        match frames[at].parent {
          Some((pf, _, edge)) => {
            err = decorate(err, EDGE_NAMES[edge]);
            at = pf;
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
          Some((pf, pe, edge)) => write_edge(&mut frames[pf].out[pe], edge, done.out),
        }
        continue;
      }

      let index = frame.next;
      frame.next += 1;
      let js_array = frame.js_array;
      let depth = frame.depth;

      let elem = unsafe { get_element(env, js_array, index) }
        .map_err(|e| decorate_ancestors(e, &frames, f))?;
      let desc = unsafe { decode_element_shallow(env, elem) }
        .map_err(|e| decorate_ancestors(e, &frames, f))?;
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
          parent: Some((f, elem_idx, edge)),
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
