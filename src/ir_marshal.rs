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
//! * Decode ([`InstrBody`]): per element, a fresh empty JS object receives a
//!   shallow copy of every enumerable property of the element EXCEPT the three
//!   self-referential edges, and the DERIVED `InstrDesc::from_napi_value` runs
//!   on that copy — it cannot recurse (the copy has no edges) and every plain
//!   field keeps its derived decoding and error behavior, automatically
//!   tracking future field additions. The edges are then walked by the driver.
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
//! The generated `index.d.ts` is UNCHANGED: `build_function` keeps
//! `body: Array<InstrDesc>` via `#[napi(ts_arg_type = "Array<InstrDesc>")]` and
//! `instructions()` keeps `Array<InstrDesc>` via
//! `#[napi(ts_return_type = "Array<InstrDesc>")]` — napi-derive's typegen uses
//! those override strings verbatim (napi-derive-backend `typegen/fn.rs`), and
//! neither wrapper type is `#[napi]`, so no new type is emitted.

use std::ffi::CStr;

use napi::bindgen_prelude::{
  get_named_property_raw, set_named_property_raw, FromNapiValue, ToNapiValue, TypeName,
  ValidateNapiValue,
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

/// Decode ONE array element via the shallow-copy-except-edges trick: copy every
/// enumerable property (own + prototype chain — the same set the derived
/// per-field `napi_get_named_property` reads see) of `elem` EXCEPT the three
/// edge names onto a fresh empty object, then run the DERIVED
/// `InstrDesc::from_napi_value` on the copy. The copy has no edges, so the
/// derived impl cannot recurse; every plain field keeps its derived decode +
/// error behavior and automatically tracks future field additions. The user's
/// `elem` is never mutated.
unsafe fn decode_element_shallow(env: sys::napi_env, elem: sys::napi_value) -> Result<InstrDesc> {
  let mut copy = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_create_object(env, &mut copy) },
    "Failed to create napi Object"
  )?;

  // Enumerate the ORIGINAL's property names (a JS array of strings). On a
  // `null`/`undefined` element this raises the same pending TypeError the
  // derived decode's first property read raised ("Cannot convert undefined or
  // null to object").
  let mut names = std::ptr::null_mut();
  check_status!(
    unsafe { sys::napi_get_property_names(env, elem, &mut names) },
    "Failed to get property names of given object"
  )?;
  let names_len = unsafe { array_length(env, names)? };

  // A property name only needs enough buffer to distinguish it from the edge
  // names (max 11 bytes, "alternative"): a name that fills the buffer is
  // longer than any edge name and therefore copied without further inspection.
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
    if written < buf.len() - 1 && EDGE_NAMES.iter().any(|e| e.as_bytes() == &buf[..written]) {
      continue; // an edge: the driver walks it — never the derived impl
    }
    let mut val = std::ptr::null_mut();
    check_status!(
      unsafe { sys::napi_get_property(env, elem, name, &mut val) },
      "Failed to get property"
    )?;
    check_status!(
      unsafe { sys::napi_set_property(env, copy, name, val) },
      "Failed to set property"
    )?;
  }

  unsafe { InstrDesc::from_napi_value(env, copy) }
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
    let mut frames = vec![DecodeFrame {
      js_array: napi_val,
      len: root_len,
      next: 0,
      out: Vec::with_capacity(root_len as usize),
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

      // Walk the edges on the ORIGINAL element, in declaration order.
      for (edge, cname) in EDGE_CSTRS.iter().enumerate() {
        let raw = unsafe { get_named_property_raw(env, elem, cname.as_ptr()) }
          .map_err(|e| decorate_ancestors(e, &frames, f))?;
        let Some(raw) = raw else {
          continue; // absent/undefined edge stays `None`
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
        frames.push(DecodeFrame {
          js_array: raw,
          len: child_len,
          next: 0,
          out: Vec::with_capacity(child_len as usize),
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
