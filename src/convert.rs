//! Conversions between the walrus value types and their napi mirrors, in both
//! directions.
//!
//! READ (walrus -> napi): used by the getters (e.g. `WasmGlobal::ty`).
//! WRITE (napi -> walrus): used when building things from JS (e.g.
//! `globals.addLocal(ty, ...)`, `ConstExpr::ref_null(...)`).
//!
//! All matches over walrus enums are written out arm-by-arm so a *known*
//! variant can never be silently mismapped. Two of these walrus enums —
//! `walrus::HeapType` and `walrus::AbstractHeapType` — are `#[non_exhaustive]`
//! (see walrus `ty.rs:666` / `:791`), which forces every external match to
//! carry a trailing `_` arm. That catch-all must NOT panic: walrus is declared
//! `= "0.26"` in `Cargo.toml` and `Cargo.lock` is untracked/gitignored, so a
//! fresh build can resolve a later semver-compatible `0.26.x` that adds a heap
//! type variant. Reading such a value through the napi `WasmGlobal::ty` getter
//! would otherwise hit `unreachable!()`, and a panic across the FFI boundary
//! aborts the entire Node process. So these conversions are FALLIBLE: the `_`
//! arm returns a catchable `napi::Error`, and callers propagate it with `?`.
//!
//! `walrus::ValType` is NOT `#[non_exhaustive]`, so its match needs no `_` arm
//! (a future variant would fail to compile here, surfacing the gap at build
//! time). It is still fallible only because its `Ref` arm embeds a `HeapType`,
//! whose conversion can fail.

use crate::valtype::{AbstractHeapType, HeapType, ValType};

impl TryFrom<walrus::ValType> for ValType {
  type Error = napi::Error;

  fn try_from(ty: walrus::ValType) -> napi::Result<Self> {
    Ok(match ty {
      walrus::ValType::I32 => ValType::I32,
      walrus::ValType::I64 => ValType::I64,
      walrus::ValType::F32 => ValType::F32,
      walrus::ValType::F64 => ValType::F64,
      walrus::ValType::V128 => ValType::V128,
      // `walrus::ValType` is exhaustive (no `_` arm); the fallibility comes
      // solely from the embedded `HeapType`, which is `#[non_exhaustive]`.
      walrus::ValType::Ref(rt) => ValType::Ref {
        nullable: rt.nullable,
        heap: rt.heap_type.try_into()?,
      },
    })
  }
}

impl TryFrom<walrus::HeapType> for HeapType {
  type Error = napi::Error;

  fn try_from(heap: walrus::HeapType) -> napi::Result<Self> {
    Ok(match heap {
      walrus::HeapType::Abstract(abstract_type) => HeapType::Abstract {
        kind: abstract_type.try_into()?,
      },
      walrus::HeapType::Concrete(id) => HeapType::Concrete {
        type_index: id.index() as u32,
      },
      walrus::HeapType::Exact(id) => HeapType::Exact {
        type_index: id.index() as u32,
      },
      // `walrus::HeapType` is `#[non_exhaustive]` and `Cargo.lock` is
      // untracked, so a fresh build can pull a later 0.26.x with a new variant.
      // A panic here would abort the Node process through the FFI boundary, so
      // return a catchable error instead of `unreachable!()`.
      other => {
        return Err(napi::Error::from_reason(format!(
          "unsupported walrus HeapType variant {other:?}; the walrus version may have advanced beyond 0.26.4"
        )))
      }
    })
  }
}

impl TryFrom<walrus::AbstractHeapType> for AbstractHeapType {
  type Error = napi::Error;

  fn try_from(abstract_type: walrus::AbstractHeapType) -> napi::Result<Self> {
    Ok(match abstract_type {
      walrus::AbstractHeapType::Func => AbstractHeapType::Func,
      walrus::AbstractHeapType::Extern => AbstractHeapType::Extern,
      walrus::AbstractHeapType::Any => AbstractHeapType::Any,
      walrus::AbstractHeapType::None => AbstractHeapType::None,
      walrus::AbstractHeapType::NoExtern => AbstractHeapType::NoExtern,
      walrus::AbstractHeapType::NoFunc => AbstractHeapType::NoFunc,
      walrus::AbstractHeapType::Eq => AbstractHeapType::Eq,
      walrus::AbstractHeapType::Struct => AbstractHeapType::Struct,
      walrus::AbstractHeapType::Array => AbstractHeapType::Array,
      walrus::AbstractHeapType::I31 => AbstractHeapType::I31,
      walrus::AbstractHeapType::Exn => AbstractHeapType::Exn,
      walrus::AbstractHeapType::NoExn => AbstractHeapType::NoExn,
      // `walrus::AbstractHeapType` is `#[non_exhaustive]` and `Cargo.lock` is
      // untracked, so a fresh build can pull a later 0.26.x with a new variant.
      // A panic here would abort the Node process through the FFI boundary, so
      // return a catchable error instead of `unreachable!()`.
      other => {
        return Err(napi::Error::from_reason(format!(
          "unsupported walrus AbstractHeapType variant {other:?}; the walrus version may have advanced beyond 0.26.4"
        )))
      }
    })
  }
}

// ---------------------------------------------------------------------------
// WRITE direction (napi -> walrus).
//
// Used when building walrus values from JS: `globals.addLocal(ty, ...)` and
// `ConstExpr::ref_null(...)`. `ValType` -> `walrus::ValType` is fallible only
// because a `Ref` embeds a `HeapType`; the heap-type conversion itself rejects
// concrete/indexed heap types, which cannot be reconstructed from a bare
// `type_index` (rebuilding a walrus `TypeId` needs a type handle — deferred to
// the GC-types task). That rejection is a catchable `napi::Error`, never a
// panic.
// ---------------------------------------------------------------------------

impl TryFrom<ValType> for walrus::ValType {
  type Error = napi::Error;

  fn try_from(ty: ValType) -> napi::Result<Self> {
    Ok(match ty {
      ValType::I32 => walrus::ValType::I32,
      ValType::I64 => walrus::ValType::I64,
      ValType::F32 => walrus::ValType::F32,
      ValType::F64 => walrus::ValType::F64,
      ValType::V128 => walrus::ValType::V128,
      ValType::Ref { nullable, heap } => walrus::ValType::Ref(walrus::RefType {
        nullable,
        heap_type: heap.try_into()?,
      }),
    })
  }
}

impl TryFrom<HeapType> for walrus::HeapType {
  type Error = napi::Error;

  fn try_from(heap: HeapType) -> napi::Result<Self> {
    match heap {
      HeapType::Abstract { kind } => Ok(walrus::HeapType::Abstract(kind.into())),
      // A `type_index` (a stable arena index) cannot rebuild a walrus `TypeId`;
      // that requires a type handle. Reject it with a catchable error until the
      // GC-types task threads real type handles through.
      HeapType::Concrete { .. } | HeapType::Exact { .. } => Err(napi::Error::from_reason(
        "concrete/indexed ref types require a type handle; not yet supported (see GC types task)",
      )),
    }
  }
}

impl From<AbstractHeapType> for walrus::AbstractHeapType {
  fn from(kind: AbstractHeapType) -> Self {
    // Total: our napi enum has exactly the 12 walrus abstract heap types, so
    // this maps 1:1 with no catch-all (and no fallibility) needed.
    match kind {
      AbstractHeapType::Func => walrus::AbstractHeapType::Func,
      AbstractHeapType::Extern => walrus::AbstractHeapType::Extern,
      AbstractHeapType::Any => walrus::AbstractHeapType::Any,
      AbstractHeapType::None => walrus::AbstractHeapType::None,
      AbstractHeapType::NoExtern => walrus::AbstractHeapType::NoExtern,
      AbstractHeapType::NoFunc => walrus::AbstractHeapType::NoFunc,
      AbstractHeapType::Eq => walrus::AbstractHeapType::Eq,
      AbstractHeapType::Struct => walrus::AbstractHeapType::Struct,
      AbstractHeapType::Array => walrus::AbstractHeapType::Array,
      AbstractHeapType::I31 => walrus::AbstractHeapType::I31,
      AbstractHeapType::Exn => walrus::AbstractHeapType::Exn,
      AbstractHeapType::NoExn => walrus::AbstractHeapType::NoExn,
    }
  }
}
