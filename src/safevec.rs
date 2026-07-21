//! [`SafeVec<T>`] — a drop-in replacement for `Vec<T>` as a `#[napi]` INPUT type
//! whose JS→Rust decode NEVER pre-allocates from the untrusted JS `Array.length`.
//!
//! napi's derived `Vec::<T>::from_napi_value` does
//! `Vec::with_capacity(arr.len() as usize)` (napi-3.10.5 `array.rs`), where
//! `arr.len()` is the JS-reported `.length`. A sparse `new Array(4e9)` (or
//! `{ length: 4e9 }`) reports a near-`2**32` length with few/no real elements, so
//! `with_capacity` aborts the process (capacity-overflow panic /
//! `handle_alloc_error`) BEFORE the element loop ever runs — uncatchable,
//! especially under WASI `panic=abort`.
//!
//! `SafeVec<T>` grows its `Vec` with a `try_reserve`-guarded push loop over the
//! ACTUAL elements — exactly the non-preallocating mechanics
//! [`crate::ir_marshal::InstrBody`] already uses for `build_function`'s `body`,
//! but generic over any `T: FromNapiValue`. A hostile length therefore fails
//! CATCHABLY (a per-element decode error on the first sparse hole, or a caught
//! allocation failure) and the process survives.
//!
//! Its trait surface mirrors `Vec<T>` (`FromNapiValue` + `ToNapiValue` +
//! `TypeName` + `ValidateNapiValue`) so it drops into any `#[napi]` method param
//! or `#[napi]`/`#[napi(object)]` field in place of `Vec<T>`. Every use site pins
//! the TypeScript type with `#[napi(ts_arg_type = ...)]` (params) or
//! `#[napi(ts_type = ...)]` (fields), so the generated `index.d.ts` stays
//! byte-identical — the `type_name()` placeholder below is never surfaced.

use napi::bindgen_prelude::{Array, FromNapiValue, ToNapiValue, TypeName, ValidateNapiValue};
use napi::{check_status, sys, Error, Result, Status, ValueType};

/// A `Vec<T>` newtype whose napi decode is non-preallocating (see the module
/// docs). Read the inner `Vec` via `.0`, exactly like
/// [`crate::ir_marshal::InstrBody`].
pub struct SafeVec<T>(pub Vec<T>);

impl<T> FromNapiValue for SafeVec<T>
where
  T: FromNapiValue,
{
  unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
    // `Array::from_napi_value` merely WRAPS the `napi_value` and reads its
    // `.length` (`napi_get_array_length`); it allocates NO `Vec` (napi-3.10.5
    // `array.rs`), so reading the length is safe even for a hostile sparse
    // length. A non-array is rejected here (and eagerly in `validate` below).
    let arr = unsafe { Array::from_napi_value(env, napi_val)? };
    let len = arr.len();
    // `out` is NOT pre-sized from `len` (the untrusted JS `Array.length`): a
    // sparse array can report a length near `2**32` with few/no real elements,
    // and `Vec::with_capacity(len)` on such a length aborts the process
    // (capacity-overflow / `handle_alloc_error`) — uncatchable under WASI
    // `panic=abort`, the exact abort class this newtype exists to remove. The
    // push loop grows `out` to the ACTUAL element count; a sparse hole fails
    // catchably in `T::from_napi_value` (a decode error on `undefined`).
    let mut out: Vec<T> = Vec::new();
    for i in 0..len {
      match arr.get::<T>(i)? {
        Some(val) => {
          // FALLIBLE growth: a `Proxy` returning a valid element for EVERY index
          // (no hole to fail on) could drive this toward `2**32` elements; an
          // infallible `push` could then abort under memory pressure, so
          // `try_reserve` maps that exhaustion to a CATCHABLE error instead.
          out.try_reserve(1).map_err(|_| {
            Error::new(
              Status::GenericFailure,
              "array too large to decode".to_owned(),
            )
          })?;
          out.push(val);
        }
        // `Array::get` only yields `None` for an OUT-OF-RANGE index; within
        // `0..len` it never does. Kept identical to the derived `Vec` decode's
        // own consistency guard so the observable error behavior matches.
        None => {
          return Err(Error::new(
            Status::InvalidArg,
            "Found inconsistent data type in Array<T> when converting to Rust Vec<T>".to_owned(),
          ))
        }
      }
    }
    Ok(SafeVec(out))
  }
}

impl<T> ToNapiValue for SafeVec<T>
where
  T: ToNapiValue,
{
  unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
    // The ENCODE side is NOT the vulnerability class: `Vec::<T>::to_napi_value`
    // sizes its array from the REAL Rust `Vec` length, never a hostile JS
    // `.length`. Delegating keeps a type that is ALSO returned to JS compiling —
    // notably the `#[napi]` `CompositeType` enum, whose derived `ToNapiValue`
    // encodes each `SafeVec` variant field.
    unsafe { Vec::<T>::to_napi_value(env, val.0) }
  }
}

impl<T> TypeName for SafeVec<T> {
  fn type_name() -> &'static str {
    // Never surfaces in `index.d.ts`: every use site overrides the TS type via
    // `#[napi(ts_arg_type = ...)]` / `#[napi(ts_type = ...)]`. Matches the
    // placeholder napi uses for `Vec<T>` itself.
    "Array<T>"
  }

  fn value_type() -> ValueType {
    ValueType::Object
  }
}

impl<T> ValidateNapiValue for SafeVec<T>
where
  T: FromNapiValue,
{
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
