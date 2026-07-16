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

use crate::valtype::{AbstractHeapType, CompositeType, FieldType, HeapType, StorageType, ValType};

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

// `walrus::StorageType` is NOT `#[non_exhaustive]` (it has exactly `I8`, `I16`,
// `Val(ValType)`), so this match is total and needs no `_` arm. It is fallible
// only because the `Val` arm embeds a `ValType`, whose `Ref` variant embeds a
// `#[non_exhaustive]` `HeapType`.
impl TryFrom<walrus::StorageType> for StorageType {
  type Error = napi::Error;

  fn try_from(st: walrus::StorageType) -> napi::Result<Self> {
    Ok(match st {
      walrus::StorageType::I8 => StorageType::I8,
      walrus::StorageType::I16 => StorageType::I16,
      walrus::StorageType::Val(vt) => StorageType::Val {
        value: vt.try_into()?,
      },
    })
  }
}

impl TryFrom<walrus::FieldType> for FieldType {
  type Error = napi::Error;

  fn try_from(ft: walrus::FieldType) -> napi::Result<Self> {
    Ok(FieldType {
      storage: ft.element_type.try_into()?,
      mutable: ft.mutable,
    })
  }
}

// ---------------------------------------------------------------------------
// Pure WRITE direction (napi -> walrus), with NO module access.
//
// Used by the value-only write paths (`globals.addLocal(ty, ...)`,
// `ConstExpr::ref_null(...)`) that have no live type arena to consult.
// `ValType` -> `walrus::ValType` is fallible only because a `Ref` embeds a
// `HeapType`, and this pure conversion REJECTS a concrete/indexed heap: a bare
// `type_index` cannot be rebuilt into a walrus `TypeId` without the arena. The
// module-aware `*_in` converters further below ARE given the arena and resolve
// concrete refs instead of rejecting them (used by struct/array field
// creation). The rejection here is a catchable `napi::Error`, never a panic.
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
      // A bare `type_index` (a stable arena index) cannot rebuild a walrus
      // `TypeId` without the type arena, which this pure conversion has no
      // access to. Callers that CAN reach the arena (struct/array field
      // creation) use `heap_type_to_walrus_in` instead, which resolves it.
      HeapType::Concrete { .. } | HeapType::Exact { .. } => Err(napi::Error::from_reason(
        "concrete/indexed ref types cannot be resolved without module access; use the module-aware conversion path",
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

// ---------------------------------------------------------------------------
// Module-aware WRITE direction (napi -> walrus, resolving concrete refs).
//
// The pure `TryFrom<HeapType> for walrus::HeapType` above REJECTS a
// concrete/exact heap: rebuilding a walrus `TypeId` from a bare `type_index`
// needs the module's type arena, which a free-standing `TryFrom` cannot reach.
// These `*_in` converters take the live `&walrus::Module` and are the
// module-aware supersets used by the write paths that CAN reach the arena
// (struct/array field creation). Non-concrete inputs behave identically to the
// pure conversions; only the concrete/exact heap path differs.
// ---------------------------------------------------------------------------

/// Resolve a JS `type_index` (a stable arena `.index()`) to the live
/// `walrus::TypeId` it names, by scanning this module's type arena.
///
/// A `type_index` that names no live type in this module returns a catchable
/// error rather than an unvalidated `TypeId`. This is a hard requirement: a
/// made-up id would pass creation but ABORT the whole process at emit time —
/// `HeapType::to_wasmencoder_heap_type` resolves it through the panicking
/// `IdsToIndices::get_type_index`, and a panic across the FFI boundary is
/// uncatchable. Rejecting the bad index here turns that abort into a normal JS
/// exception.
pub(crate) fn resolve_type_id(
  module: &walrus::Module,
  type_index: u32,
) -> napi::Result<walrus::TypeId> {
  let entry_ids = crate::types::entry_type_ids(module);
  module
    .types
    .iter()
    .find(|t| t.id().index() as u32 == type_index)
    .map(|t| t.id())
    .filter(|id| !entry_ids.contains(id))
    .ok_or_else(|| {
      napi::Error::from_reason(format!("no type at index {type_index} in this module"))
    })
}

/// Module-aware `HeapType` -> `walrus::HeapType`: like the pure `TryFrom`, but
/// resolves a concrete/exact `type_index` against the live arena instead of
/// rejecting it.
pub(crate) fn heap_type_to_walrus_in(
  module: &walrus::Module,
  heap: HeapType,
) -> napi::Result<walrus::HeapType> {
  // `HeapType` is our own (exhaustive) napi enum, so this match needs no `_`.
  match heap {
    HeapType::Abstract { kind } => Ok(walrus::HeapType::Abstract(kind.into())),
    HeapType::Concrete { type_index } => Ok(walrus::HeapType::Concrete(resolve_type_id(
      module, type_index,
    )?)),
    HeapType::Exact { type_index } => Ok(walrus::HeapType::Exact(resolve_type_id(
      module, type_index,
    )?)),
  }
}

/// Module-aware `ValType` -> `walrus::ValType`: primitives map directly; a
/// `Ref` delegates its heap to [`heap_type_to_walrus_in`] so concrete refs
/// resolve against the live arena.
pub(crate) fn val_type_to_walrus_in(
  module: &walrus::Module,
  ty: ValType,
) -> napi::Result<walrus::ValType> {
  Ok(match ty {
    ValType::I32 => walrus::ValType::I32,
    ValType::I64 => walrus::ValType::I64,
    ValType::F32 => walrus::ValType::F32,
    ValType::F64 => walrus::ValType::F64,
    ValType::V128 => walrus::ValType::V128,
    ValType::Ref { nullable, heap } => walrus::ValType::Ref(walrus::RefType {
      nullable,
      heap_type: heap_type_to_walrus_in(module, heap)?,
    }),
  })
}

/// Module-aware `StorageType` -> `walrus::StorageType`.
pub(crate) fn storage_type_to_walrus_in(
  module: &walrus::Module,
  st: StorageType,
) -> napi::Result<walrus::StorageType> {
  Ok(match st {
    StorageType::I8 => walrus::StorageType::I8,
    StorageType::I16 => walrus::StorageType::I16,
    StorageType::Val { value } => walrus::StorageType::Val(val_type_to_walrus_in(module, value)?),
  })
}

/// Module-aware `FieldType` -> `walrus::FieldType`, used when building a GC
/// struct/array so a field can reference another type via `(ref $t)`.
pub(crate) fn field_type_to_walrus_in(
  module: &walrus::Module,
  ft: FieldType,
) -> napi::Result<walrus::FieldType> {
  Ok(walrus::FieldType {
    element_type: storage_type_to_walrus_in(module, ft.storage)?,
    mutable: ft.mutable,
  })
}

/// Module-aware `CompositeType` -> `walrus::CompositeType`, used by
/// `WasmTypes::add_composite`.
///
/// Every field / param / result is converted through the module-aware `*_in`
/// path (reusing [`field_type_to_walrus_in`] / [`val_type_to_walrus_in`]), so a
/// `Concrete`/`Exact` ref to an EXISTING type resolves and a bad/entry-type
/// index surfaces a catchable error. The caller builds this BEFORE mutating the
/// arena, so a failed conversion never leaves a half-built type behind.
pub(crate) fn composite_type_to_walrus_in(
  module: &walrus::Module,
  comp: CompositeType,
) -> napi::Result<walrus::CompositeType> {
  // `CompositeType` is our own (exhaustive) napi enum, so this match needs no
  // `_` arm — a future variant would fail to compile here.
  Ok(match comp {
    CompositeType::Struct { fields } => {
      let fields = fields
        .into_iter()
        .map(|f| field_type_to_walrus_in(module, f))
        .collect::<napi::Result<Vec<_>>>()?;
      walrus::CompositeType::Struct(walrus::StructType {
        fields: fields.into_boxed_slice(),
      })
    }
    CompositeType::Array { element } => walrus::CompositeType::Array(walrus::ArrayType {
      field: field_type_to_walrus_in(module, element)?,
    }),
    CompositeType::Function { params, results } => {
      let params = params
        .into_iter()
        .map(|v| val_type_to_walrus_in(module, v))
        .collect::<napi::Result<Vec<_>>>()?;
      let results = results
        .into_iter()
        .map(|v| val_type_to_walrus_in(module, v))
        .collect::<napi::Result<Vec<_>>>()?;
      walrus::CompositeType::Function(walrus::FunctionType::new(
        params.into_boxed_slice(),
        results.into_boxed_slice(),
      ))
    }
  })
}
