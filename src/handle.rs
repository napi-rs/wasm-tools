//! Shared helpers for the item-handle wrappers (`WasmGlobal`, `WasmFunction`,
//! `WasmType`, …) and their arena-id validation.
//!
//! # Known limitation: `&T` handle-param type confusion (findings 1-B / 1-C)
//!
//! Every method that takes a handle by reference — `delete(&WasmGlobal)`,
//! `addLocal(init: &ConstExpr)`, the `&ModuleConfig` factories, … — is marked
//! `#[napi(strict)]`, so napi-rs inserts an `napi_instanceof` check before it
//! unwraps the JS object to its Rust `Box<T>`. That rejects a plain wrong-class
//! argument, which is finding 1-B (a non-strict `&T` param blind-unwraps and
//! casts `*mut c_void -> *mut T` with no check at all).
//!
//! On its own, `instanceof` does NOT close the prototype-spoof hole, finding
//! 1-C: `instanceof` is prototype-based, so a REAL wrapped instance of the wrong
//! class, re-parented with `Object.setPrototypeOf(wrongInstance,
//! RightClass.prototype)`, passes the `instanceof` check while its wrapped
//! pointer is still the wrong Rust type — a type-confused cast. NATIVE builds now
//! close 1-C with an unforgeable per-class identity underneath `instanceof`; the
//! WASI build cannot yet carry that identity (see below).
//!
//! ```text
//!   wrong-class arg            -> rejected by strict `instanceof` + type tag   (1-B closed)
//!   setPrototypeOf-forged arg  -> native: rejected by the unforgeable type tag (1-C closed on native)
//!                                 wasm:   passes instanceof, blind-cast        (1-C open on wasm)
//! ```
//!
//! 1-C is CLOSED on NATIVE targets via Node-API object type tags (`napi >= 3.11`,
//! `napi8` feature — see `Cargo.toml`; the fix is napi-rs/napi-rs#3405,
//! <https://github.com/napi-rs/napi-rs/pull/3405>, now merged). The derive
//! auto-emits an `impl TypeTag` for each `#[napi]` class, stamps every instance
//! with that tag at wrap time, and checks the tag before every `&T` cast. The tag
//! is a process-internal identity that JS cannot read or forge, so a
//! `setPrototypeOf`-spoofed object — which passes `instanceof` — is still
//! rejected before its pointer is reinterpreted. On the shipped WASI target (see
//! `package.json`) the tag is a no-op: a static's address is not a process-global
//! identity on wasm, so the WASI build keeps the forgeable strict-`instanceof`
//! mitigation and 1-C stays open there until an upstream wasm-safe identity
//! exists. On every target `#[napi(strict)]` remains the baseline that closes 1-B
//! and, on native, backs the type tag that additionally closes 1-C.

use napi::bindgen_prelude::{BigInt, Error, Result};

/// Convert a JS `BigInt` to a `u64` wasm size, rejecting a negative or
/// out-of-range value with a catchable error.
///
/// `BigInt::get_u64()` returns `(sign_bit, value, lossless)` and the previous
/// code kept only `value` — so `-5n` silently stored `5` and `2n**64n` silently
/// stored `0`. Unlike a wasm `i64` VALUE (which wraps by spec), a memory/table
/// SIZE is an unsigned count with no modulo semantics, so a size that does not
/// fit losslessly in a `u64` is data corruption, not wraparound. Reject it here
/// before any arena mutation so the API never reports a bogus success.
pub(crate) fn bigint_to_u64(v: BigInt, what: &str) -> Result<u64> {
  let (sign_bit, value, lossless) = v.get_u64();
  if sign_bit || !lossless {
    return Err(Error::from_reason(format!(
      "{what} must be a non-negative integer that fits in a u64"
    )));
  }
  Ok(value)
}

/// Error returned when an item handle is used after its item was deleted from
/// the module.
///
/// walrus' arena `get`/`get_mut` panic on a deleted id, and a panic across the
/// FFI boundary aborts the process. Item wrappers guard their arena access and
/// surface this catchable JS error instead.
pub(crate) fn deleted(kind: &str) -> Error {
  Error::from_reason(format!("this {kind} has been deleted from the module"))
}

/// Validate that a `ConstExpr` about to be stored in `module` only references
/// arena ids that are LIVE in that module.
///
/// walrus panics HARD at emit time (`get_global_index`/`get_func_index`) if a
/// stored `ConstExpr` references a global/function id that has no index in the
/// module — a foreign-module handle or an already-deleted id. That panic
/// crosses the FFI boundary and ABORTS the whole Node process, uncatchable.
/// Call this at every CONSUME site (e.g. `globals.addLocal(init)`) so a bad id
/// is rejected with a catchable JS error before it can reach emit.
///
/// This catches foreign + already-deleted ids at wiring time. It does NOT
/// prevent a LATER deletion of a still-referenced item from aborting emit —
/// walrus `delete` does not cascade (a known limitation); not solved here.
pub(crate) fn validate_const_expr(module: &walrus::Module, ce: &walrus::ConstExpr) -> Result<()> {
  use walrus::ConstExpr::*;
  match ce {
    Value(_) => Ok(()),
    // A `RefNull` looks id-free but is not: a typed `ref.null $t` carries a
    // `HeapType::Concrete(TypeId)` / `Exact(TypeId)`. Our own `ref_null` factory
    // rejects concrete heaps, but a const expr cloned off a PARSED global
    // (`WasmGlobal.init()`) can surface a foreign/deleted `TypeId` that would
    // abort emit (`get_type_index`), so validate the heap type's provenance.
    RefNull(rt) => validate_heap_type(module, rt.heap_type),
    Global(gid) => {
      if module.globals.iter().any(|g| g.id() == *gid) {
        Ok(())
      } else {
        Err(Error::from_reason(
          "ConstExpr references a global that is not in this module (or was deleted)",
        ))
      }
    }
    RefFunc(fid) => {
      if module.funcs.iter().any(|f| f.id() == *fid) {
        Ok(())
      } else {
        Err(Error::from_reason(
          "ConstExpr references a function that is not in this module (or was deleted)",
        ))
      }
    }
    // Our factories never build extended const expressions; reject rather than
    // risk an unvalidated embedded id reaching emit.
    Extended(_) => Err(Error::from_reason(
      "extended const expressions are not supported",
    )),
  }
}

/// Validate that a `HeapType` about to reach emit references only type arena ids
/// that are LIVE in `module`.
///
/// A `Concrete(id)`/`Exact(id)` heap embeds a `TypeId`. walrus panics HARD at
/// emit (`IdsToIndices::get_type_index`) if that id has no index in the module —
/// a foreign-module type handle or an already-deleted type — and that panic
/// crosses the FFI boundary and ABORTS the whole Node process. `Abstract(_)`
/// heaps carry no id and are always fine.
fn validate_heap_type(module: &walrus::Module, heap: walrus::HeapType) -> Result<()> {
  use walrus::HeapType::*;
  match heap {
    // Abstract heaps (`func`, `extern`, `any`, ...) embed no arena id.
    Abstract(_) => Ok(()),
    // A concrete/exact heap references a defined type by id; it must be live in
    // THIS module's type arena or emit aborts the process.
    Concrete(id) | Exact(id) => {
      if module.types.iter().any(|t| t.id() == id) {
        Ok(())
      } else {
        Err(Error::from_reason(
          "ConstExpr references a type that is not in this module (or was deleted)",
        ))
      }
    }
    // `walrus::HeapType` is `#[non_exhaustive]` and `Cargo.lock` is untracked, so
    // a fresh build can pull a later 0.26.x with a new heap variant we cannot
    // validate. Reject it (a catchable error) rather than let an unknown,
    // possibly id-carrying heap reach emit — same safe default as `Extended`.
    _ => Err(Error::from_reason(
      "ConstExpr references an unsupported ref heap type; the walrus version may have advanced beyond 0.26.4",
    )),
  }
}
