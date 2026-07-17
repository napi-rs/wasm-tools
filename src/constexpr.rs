//! A napi wrapper around `walrus::ConstExpr`, the constant expressions used as
//! global initializers (and, later, element/data offsets).
//!
//! `ConstExpr` values can embed arena ids (`Global(GlobalId)` /
//! `RefFunc(FunctionId)`), so — unlike the pure value enums in
//! [`crate::valtype`] — this crosses the FFI boundary as a wrapper class with
//! factory statics rather than a structured enum. Only the WRITE direction
//! (JS -> walrus) is exposed here: build a constant, then hand it to
//! [`crate::globals::WasmGlobals::add_local`]. Reading a constant back yields
//! only its [`ConstExprKind`] discriminant (value-extraction getters are YAGNI
//! until a consumer needs them).

use napi::bindgen_prelude::{BigInt, FromNapiValue, Result, Uint8Array, ValidateNapiValue};
use napi::sys;
use napi_derive::napi;

use crate::functions::WasmFunction;
use crate::globals::WasmGlobal;
use crate::valtype::HeapType;

/// A wasm constant expression, used to initialize a global.
///
/// Construct one with the factory statics ([`ConstExpr::i32`],
/// [`ConstExpr::ref_null`], ...) and pass it to `globals.addLocal`.
#[napi]
pub struct ConstExpr {
  pub(crate) inner: walrus::ConstExpr,
}

#[napi]
impl ConstExpr {
  /// A constant `i32` value.
  #[napi(factory)]
  pub fn i32(v: i32) -> Self {
    Self {
      inner: walrus::ConstExpr::Value(walrus::ir::Value::I32(v)),
    }
  }

  /// A constant `i64` value.
  ///
  /// Takes a JS `bigint`. napi's `BigInt::get_i64` also reports whether the
  /// value fit losslessly in an `i64`; we intentionally ignore that flag and
  /// take the low 64 bits, mirroring wasm's wraparound `i64` semantics (an
  /// out-of-range `bigint` is truncated, not rejected).
  #[napi(factory)]
  pub fn i64(v: BigInt) -> Self {
    let (value, _lossless) = v.get_i64();
    Self {
      inner: walrus::ConstExpr::Value(walrus::ir::Value::I64(value)),
    }
  }

  /// A constant `f32` value.
  ///
  /// napi has no 32-bit float argument type, so this takes a JS `number`
  /// (`f64`) and narrows it to `f32`.
  #[napi(factory)]
  pub fn f32(v: f64) -> Self {
    Self {
      inner: walrus::ConstExpr::Value(walrus::ir::Value::F32(v as f32)),
    }
  }

  /// A constant `f64` value.
  #[napi(factory)]
  pub fn f64(v: f64) -> Self {
    Self {
      inner: walrus::ConstExpr::Value(walrus::ir::Value::F64(v)),
    }
  }

  /// A constant `v128` value from exactly 16 little-endian bytes.
  ///
  /// Rejects any other length with a catchable error (never a process-aborting
  /// panic). The byte order matches walrus' own `v128` decoding: byte 0 is the
  /// least-significant.
  #[napi(factory)]
  pub fn v128(bytes: Uint8Array) -> Result<Self> {
    let slice: &[u8] = &bytes;
    let array: [u8; 16] = slice.try_into().map_err(|_| {
      napi::Error::from_reason(format!(
        "v128 constant requires exactly 16 bytes, got {}",
        slice.len()
      ))
    })?;
    Ok(Self {
      inner: walrus::ConstExpr::Value(walrus::ir::Value::V128(u128::from_le_bytes(array))),
    })
  }

  /// A constant that reads the current value of another global
  /// (`global.get $g`).
  #[napi(factory)]
  pub fn global_get(global: &WasmGlobal) -> Self {
    Self {
      inner: walrus::ConstExpr::Global(global.id),
    }
  }

  /// A constant function reference (`ref.func $f`).
  ///
  /// Takes a live [`WasmFunction`] HANDLE (not an index), exactly like
  /// [`ConstExpr::global_get`]: the handle already carries the resolved
  /// `FunctionId`, so no module/index resolution is needed here. Provenance is
  /// validated at the CONSUME site — every consumer (`globals.addLocal`,
  /// `data.addActive`, `elements.addFunctions`/`addExpressions`,
  /// `tables.addLocalWithInit`) runs `validate_const_expr`, whose `RefFunc` arm
  /// rejects a function that is not live in that module. So a `refFunc` built
  /// off a foreign/deleted function handle is caught catchably where it is used,
  /// never as a process-aborting emit-time panic. Adds no new abort surface
  /// (identical risk profile to `global_get`).
  #[napi(factory)]
  pub fn ref_func(func: &WasmFunction) -> Self {
    Self {
      inner: walrus::ConstExpr::RefFunc(func.id),
    }
  }

  /// A null reference of the given (abstract) heap type (`ref.null`).
  ///
  /// `ref.null` is ALWAYS nullable — a non-nullable null is invalid wasm
  /// (`WebAssembly.validate` rejects it), so this factory takes only the heap
  /// type and always builds a nullable `RefType`.
  ///
  /// This static factory is ABSTRACT-ONLY: it has no module access, so a
  /// concrete/indexed heap (`Concrete`/`Exact`) is rejected catchably — use the
  /// handle-based `WasmType.refNull()` for a `ref.null $t` to a concrete type. A
  /// `RecGroup` heap (only meaningful inside `addRecGroup`) is likewise rejected.
  #[napi(factory)]
  pub fn ref_null(heap: HeapType) -> Result<Self> {
    let heap_type: walrus::HeapType = heap.try_into()?;
    Ok(Self {
      inner: walrus::ConstExpr::RefNull(walrus::RefType {
        nullable: true,
        heap_type,
      }),
    })
  }

  /// Which kind of constant expression this is.
  #[napi(getter)]
  pub fn kind(&self) -> ConstExprKind {
    match &self.inner {
      walrus::ConstExpr::Value(_) => ConstExprKind::Value,
      walrus::ConstExpr::Global(_) => ConstExprKind::Global,
      walrus::ConstExpr::RefNull(_) => ConstExprKind::RefNull,
      walrus::ConstExpr::RefFunc(_) => ConstExprKind::RefFunc,
      walrus::ConstExpr::Extended(_) => ConstExprKind::Extended,
    }
  }
}

/// An abort-safe napi decode of ONE JS `ConstExpr` class handle into an owned
/// clone of its inner `walrus::ConstExpr`.
///
/// This exists so a LIST of `ConstExpr` handles (`elements.addExpressions`'
/// `exprs`) can be decoded through [`crate::safevec::SafeVec`] without ever
/// pre-allocating from the untrusted JS `Array.length` (the finding #4 abort
/// class). `SafeVec<T>` requires `T: FromNapiValue`, and neither `&ConstExpr` (a
/// borrow, not owned) nor `ClassInstance<ConstExpr>` (carries an `'env`
/// lifetime) drops cleanly into the lifetime-free `SafeVec<T>`; this owned
/// newtype does.
///
/// The decode is byte-for-byte the same path the derived `&ConstExpr` param
/// takes: it runs the generated `instanceof ConstExpr` validation FIRST, so a JS
/// array element that is some OTHER `#[napi]` class can never have its native
/// pointer type-confused into a `walrus::ConstExpr` (`napi_unwrap` is
/// type-blind). The inner const expr is cloned out immediately; the provenance
/// of any id it embeds is validated by the caller via `validate_const_expr`.
pub struct ConstExprArg(pub(crate) walrus::ConstExpr);

impl FromNapiValue for ConstExprArg {
  unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
    // Same instanceof guard the derived `&ConstExpr` param runs (napi-derive's
    // `ValidateNapiValue for &ConstExpr`): reject anything that is not a
    // `ConstExpr` instance BEFORE the type-blind `napi_unwrap` inside
    // `<&ConstExpr>::from_napi_value` can reinterpret a foreign class's native
    // pointer as a `walrus::ConstExpr`.
    unsafe { <&ConstExpr as ValidateNapiValue>::validate(env, napi_val)? };
    let handle: &ConstExpr =
      unsafe { <&ConstExpr as FromNapiValue>::from_napi_value(env, napi_val)? };
    Ok(ConstExprArg(handle.inner.clone()))
  }
}

/// The discriminant of a [`ConstExpr`], mirroring the five `walrus::ConstExpr`
/// variants 1:1.
#[napi(string_enum)]
pub enum ConstExprKind {
  /// An immediate constant value (`i32`/`i64`/`f32`/`f64`/`v128`).
  Value,
  /// Reads another global's value (`global.get`).
  Global,
  /// A typed null reference (`ref.null`).
  RefNull,
  /// A function reference (`ref.func`).
  RefFunc,
  /// An extended constant expression (a sequence of const operations).
  Extended,
}
