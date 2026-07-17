//! Conversions between the walrus value types and their napi mirrors, in both
//! directions.
//!
//! READ (walrus -> napi): used by the getters (e.g. `WasmGlobal::ty`).
//! WRITE (napi -> walrus): the module-aware `*_in` converters build things from
//! JS while resolving concrete refs against the live arena (e.g.
//! `globals.addLocal(ty, ...)`, struct/array fields, rec-group members). The
//! pure `TryFrom` variants below have no arena and reject concrete refs; the
//! only remaining pure consumer is the abstract-only `ConstExpr::ref_null`.
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

use crate::valtype::{
  AbstractHeapType, CompositeType, FieldType, HeapType, RecGroupMember, RecGroupRef, StorageType,
  ValType,
};

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
        type_index: id.index() as f64,
      },
      walrus::HeapType::Exact(id) => HeapType::Exact {
        type_index: id.index() as f64,
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
// Used by the abstract-only `ConstExpr::ref_null(...)` factory, which has no
// live type arena to consult. `ValType` -> `walrus::ValType` is fallible only
// because a `Ref` embeds a `HeapType`, and this pure conversion REJECTS a
// concrete/indexed heap (and the `RecGroup` sibling placeholder): a bare
// `type_index` cannot be rebuilt into a walrus `TypeId` without the arena. The
// module-aware `*_in` converters further below ARE given the arena and resolve
// concrete refs instead of rejecting them (used by every other consume site).
// The rejection here is a catchable `napi::Error`, never a panic.
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
      // creation, `globals.addLocal`, ...) use `heap_type_to_walrus_in`
      // instead, which resolves it. For a concrete `ref.null $t` initializer,
      // use `WasmType.refNull()` (it carries the live type handle).
      HeapType::Concrete { .. } | HeapType::Exact { .. } => Err(napi::Error::from_reason(
        "concrete/indexed ref types cannot be resolved without module access; use a module-aware consume site (e.g. WasmType.refNull() for a `ref.null $t`)",
      )),
      // A rec-group sibling reference is only meaningful while a rec group is
      // being built; it must never reach walrus outside `addRecGroup`.
      HeapType::RecGroup { .. } => Err(napi::Error::from_reason(
        "a RecGroup heap reference is only valid inside a types.addRecGroup member descriptor",
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

/// Validate a JS number that must be a `u32` index, losslessly.
///
/// A numeric index param/field is decoded by napi from JS via `napi_get_value_double`
/// when its Rust type is `f64` (the exact IEEE double, no coercion). Had it been a
/// `u32`, napi would apply ECMAScript ToUint32 FIRST (`2**32`->0, `-1`->`u32::MAX`,
/// `NaN`/`Infinity`->0, a fraction truncates), silently ALIASING a wrong index before
/// any Rust range check runs. Carrying the value as `f64` and validating here rejects
/// an out-of-domain index catchably (a normal JS exception) instead.
pub(crate) fn checked_index(n: f64, what: &str) -> napi::Result<u32> {
  if n.is_finite() && n.fract() == 0.0 && n >= 0.0 && n <= u32::MAX as f64 {
    Ok(n as u32)
  } else {
    Err(napi::Error::from_reason(format!(
      "{what} must be an integer in 0..=4294967295, got {n}"
    )))
  }
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
      module,
      checked_index(type_index, "typeIndex")?,
    )?)),
    HeapType::Exact { type_index } => Ok(walrus::HeapType::Exact(resolve_type_id(
      module,
      checked_index(type_index, "typeIndex")?,
    )?)),
    // A rec-group sibling reference has no arena index yet; it is resolved only
    // by `addRecGroup`'s bespoke two-phase converter, never here.
    HeapType::RecGroup { .. } => Err(napi::Error::from_reason(
      "a RecGroup heap reference is only valid inside a types.addRecGroup member descriptor",
    )),
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

// ---------------------------------------------------------------------------
// Rec-group members — bespoke two-phase conversion for `addRecGroup`.
//
// walrus's `ModuleTypes::add_rec_group(count, build)` pre-allocates `count`
// placeholder `TypeId`s and hands them to `build` — a closure returning a plain
// `Vec`, with NO `Result`. Anything the closure does must therefore be
// INFALLIBLE: a panic inside it (a `Vec` out-of-bounds on `type_ids[k]`, or a
// bogus `TypeId` reaching the panicking emit-time `get_type_index`) aborts the
// whole process across FFI. The existing `composite_type_to_walrus_in` cannot be
// reused here because it (correctly) rejects the `RecGroup` sibling variant, so
// every member conversion is split in two:
//
//   * PREFLIGHT (`plan_*`, FALLIBLE, holds `&Module`): resolve every
//     EXISTING-type ref to an owned `TypeId` via `resolve_type_id`, and
//     range-check every in-group `RecGroup { rec_index }` sibling ref against
//     `count`. Yields an owned [`MemberPlan`] carrying no borrows.
//   * BUILD (`build_*`, INFALLIBLE, holds `&[TypeId]`): substitute the
//     now-known `type_ids[rec_index]` for each sibling marker and hand back the
//     finished `walrus::CompositeType`. Every lookup was guaranteed by preflight
//     (`rec_index < count == type_ids.len()`), so it cannot panic.
//
// All fallible work is in preflight, which runs BEFORE `add_rec_group` allocates
// any placeholder into the arena, so a rejected `addRecGroup` leaves the module
// completely unchanged (all-or-nothing).
// ---------------------------------------------------------------------------

/// A value type in a rec-group member, resolved except for in-group siblings.
enum PlanVal {
  /// Fully resolved: a primitive, or a ref to an abstract / existing type.
  Resolved(walrus::ValType),
  /// A `(ref [null] $sibling)` whose target is the `rec_index`-th member (which
  /// has no arena id yet — filled in during the infallible build phase).
  Sibling { nullable: bool, rec_index: u32 },
}

/// A storage type in a rec-group member (the `Val` case may be a sibling ref).
enum PlanStorage {
  I8,
  I16,
  Val(PlanVal),
}

/// A field type in a rec-group member.
struct PlanField {
  storage: PlanStorage,
  mutable: bool,
}

/// A composite type in a rec-group member.
enum PlanComposite {
  Struct(Vec<PlanField>),
  Array(PlanField),
  Function {
    params: Vec<PlanVal>,
    results: Vec<PlanVal>,
  },
}

/// A rec-group member's supertype, resolved except for in-group siblings.
enum PlanSuper {
  /// An existing type in the module, already resolved to its live id.
  Existing(walrus::TypeId),
  /// A sibling member of the group, by position (filled in during build).
  Sibling(u32),
}

/// A fully-resolved, owned plan for one rec-group member. Building it from the
/// pre-allocated `type_ids` is infallible.
pub(crate) struct MemberPlan {
  composite: PlanComposite,
  is_final: bool,
  supertype: Option<PlanSuper>,
}

fn plan_val(module: &walrus::Module, ty: ValType, count: usize) -> napi::Result<PlanVal> {
  match ty {
    ValType::Ref {
      nullable,
      heap: HeapType::RecGroup { rec_index },
    } => {
      let rec_index = checked_index(rec_index, "recIndex")?;
      if rec_index as usize >= count {
        return Err(napi::Error::from_reason(format!(
          "rec-group reference recIndex {rec_index} is out of range for a group of {count} member(s)"
        )));
      }
      Ok(PlanVal::Sibling {
        nullable,
        rec_index,
      })
    }
    // Primitives + refs to abstract / existing types resolve fully now; the
    // module-aware converter rejects a bad/entry-type index catchably.
    other => Ok(PlanVal::Resolved(val_type_to_walrus_in(module, other)?)),
  }
}

fn plan_storage(
  module: &walrus::Module,
  st: StorageType,
  count: usize,
) -> napi::Result<PlanStorage> {
  Ok(match st {
    StorageType::I8 => PlanStorage::I8,
    StorageType::I16 => PlanStorage::I16,
    StorageType::Val { value } => PlanStorage::Val(plan_val(module, value, count)?),
  })
}

fn plan_field(module: &walrus::Module, ft: FieldType, count: usize) -> napi::Result<PlanField> {
  Ok(PlanField {
    storage: plan_storage(module, ft.storage, count)?,
    mutable: ft.mutable,
  })
}

fn plan_composite(
  module: &walrus::Module,
  comp: CompositeType,
  count: usize,
) -> napi::Result<PlanComposite> {
  Ok(match comp {
    CompositeType::Struct { fields } => PlanComposite::Struct(
      fields
        .into_iter()
        .map(|f| plan_field(module, f, count))
        .collect::<napi::Result<Vec<_>>>()?,
    ),
    CompositeType::Array { element } => PlanComposite::Array(plan_field(module, element, count)?),
    CompositeType::Function { params, results } => PlanComposite::Function {
      params: params
        .into_iter()
        .map(|v| plan_val(module, v, count))
        .collect::<napi::Result<Vec<_>>>()?,
      results: results
        .into_iter()
        .map(|v| plan_val(module, v, count))
        .collect::<napi::Result<Vec<_>>>()?,
    },
  })
}

fn plan_super(module: &walrus::Module, sup: RecGroupRef, count: usize) -> napi::Result<PlanSuper> {
  match sup {
    RecGroupRef::RecGroup { rec_index } => {
      let rec_index = checked_index(rec_index, "recIndex")?;
      if rec_index as usize >= count {
        return Err(napi::Error::from_reason(format!(
          "rec-group supertype recIndex {rec_index} is out of range for a group of {count} member(s)"
        )));
      }
      Ok(PlanSuper::Sibling(rec_index))
    }
    RecGroupRef::Existing { type_index } => Ok(PlanSuper::Existing(resolve_type_id(
      module,
      checked_index(type_index, "typeIndex")?,
    )?)),
  }
}

/// PREFLIGHT: resolve one napi [`RecGroupMember`] into an owned, borrow-free
/// [`MemberPlan`], rejecting a bad existing-type index or an out-of-range
/// sibling `rec_index` with a catchable error BEFORE `add_rec_group` runs.
pub(crate) fn plan_rec_group_member(
  module: &walrus::Module,
  member: RecGroupMember,
  count: usize,
) -> napi::Result<MemberPlan> {
  Ok(MemberPlan {
    composite: plan_composite(module, member.composite, count)?,
    is_final: member.is_final,
    supertype: member
      .supertype
      .map(|s| plan_super(module, s, count))
      .transpose()?,
  })
}

fn build_val(plan: &PlanVal, type_ids: &[walrus::TypeId]) -> walrus::ValType {
  match plan {
    PlanVal::Resolved(v) => *v,
    PlanVal::Sibling {
      nullable,
      rec_index,
    } => walrus::ValType::Ref(walrus::RefType {
      nullable: *nullable,
      heap_type: walrus::HeapType::Concrete(type_ids[*rec_index as usize]),
    }),
  }
}

fn build_storage(plan: &PlanStorage, type_ids: &[walrus::TypeId]) -> walrus::StorageType {
  match plan {
    PlanStorage::I8 => walrus::StorageType::I8,
    PlanStorage::I16 => walrus::StorageType::I16,
    PlanStorage::Val(pv) => walrus::StorageType::Val(build_val(pv, type_ids)),
  }
}

fn build_field(plan: &PlanField, type_ids: &[walrus::TypeId]) -> walrus::FieldType {
  walrus::FieldType {
    element_type: build_storage(&plan.storage, type_ids),
    mutable: plan.mutable,
  }
}

fn build_composite(plan: &PlanComposite, type_ids: &[walrus::TypeId]) -> walrus::CompositeType {
  match plan {
    PlanComposite::Struct(fields) => walrus::CompositeType::Struct(walrus::StructType {
      fields: fields
        .iter()
        .map(|f| build_field(f, type_ids))
        .collect::<Vec<_>>()
        .into_boxed_slice(),
    }),
    PlanComposite::Array(element) => walrus::CompositeType::Array(walrus::ArrayType {
      field: build_field(element, type_ids),
    }),
    PlanComposite::Function { params, results } => {
      walrus::CompositeType::Function(walrus::FunctionType::new(
        params
          .iter()
          .map(|v| build_val(v, type_ids))
          .collect::<Vec<_>>()
          .into_boxed_slice(),
        results
          .iter()
          .map(|v| build_val(v, type_ids))
          .collect::<Vec<_>>()
          .into_boxed_slice(),
      ))
    }
  }
}

/// BUILD (infallible): turn a preflighted [`MemberPlan`] into the
/// `(CompositeType, is_final, supertype)` tuple walrus's `add_rec_group` closure
/// must return, substituting the pre-allocated `type_ids` for sibling refs.
pub(crate) fn build_rec_group_member(
  plan: &MemberPlan,
  type_ids: &[walrus::TypeId],
) -> (walrus::CompositeType, bool, Option<walrus::TypeId>) {
  let supertype = plan.supertype.as_ref().map(|s| match s {
    PlanSuper::Existing(id) => *id,
    PlanSuper::Sibling(rec_index) => type_ids[*rec_index as usize],
  });
  (
    build_composite(&plan.composite, type_ids),
    plan.is_final,
    supertype,
  )
}
