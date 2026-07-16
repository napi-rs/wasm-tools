use std::collections::HashSet;

use napi::bindgen_prelude::{Reference, Result};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::ir::InstrSeqType;
use walrus::TypeId;

use crate::valtype::{FieldType, ValType};
use crate::WasmModule;

/// Ids of internal function-entry types (one per local function). walrus keeps
/// these in the type arena but never assigns them emit indices, so referencing
/// one at emit aborts; they are not user-meaningful. Recover them from the
/// public function side: every local function's entry block has type
/// `MultiValue(entry_ty)`.
pub(crate) fn entry_type_ids(module: &walrus::Module) -> HashSet<TypeId> {
  module
    .funcs
    .iter_local()
    .filter_map(|(_, lf)| match lf.block(lf.entry_block()).ty {
      InstrSeqType::MultiValue(ty) => Some(ty),
      InstrSeqType::Simple(_) => None,
    })
    .collect()
}

/// The kind of a wasm type: which composite-type shape it is.
///
/// Mirrors the discriminant of `walrus::CompositeType`
/// (`Function | Struct | Array`). Only `Function` types have `params()` /
/// `results()`; the `Struct` / `Array` GC composite types expose their field
/// types through `structFields()` / `arrayElement()` instead.
#[napi(string_enum)]
pub enum TypeKind {
  /// A function type: has parameter and result value types.
  Function,
  /// A GC struct type.
  Struct,
  /// A GC array type.
  Array,
}

/// Convert a JS `ValType[]` into `Vec<walrus::ValType>`, rejecting an
/// unsupported value type BEFORE it can reach the arena.
///
/// Fallible: a concrete/indexed ref type cannot be rebuilt from a bare
/// `type_index` (that needs a real type handle, deferred to the GC-types task),
/// so it surfaces a catchable error rather than aborting.
fn to_walrus_valtypes(values: Vec<ValType>) -> Result<Vec<walrus::ValType>> {
  values.into_iter().map(walrus::ValType::try_from).collect()
}

/// Convert a slice of `walrus::ValType` into a JS `ValType[]`.
///
/// Fallible for the same reason as [`crate::convert`]: a `Ref` embeds a
/// `#[non_exhaustive]` walrus `HeapType`, and a later 0.26.x could add a
/// variant we do not map; surface that as a catchable error, never a panic.
fn to_napi_valtypes(values: &[walrus::ValType]) -> Result<Vec<ValType>> {
  values.iter().copied().map(ValType::try_from).collect()
}

/// The types of a module. Each accessor materializes a fresh [`WasmType`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
///
/// walrus keeps internal function-entry types in its type arena (one per local
/// function, used for multi-value block entries). walrus never assigns them an
/// emit index, so referencing one at emit aborts, and they are not
/// user-meaningful. This collection FILTERS them out of `length` / `items` /
/// `getByIndex` — identified via each local function's `MultiValue` entry-block
/// type — so it exposes only user-meaningful types and a user can never obtain a
/// handle to an entry type.
#[napi]
pub struct WasmTypes {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmTypes {
  #[napi(getter)]
  /// The number of types in the module.
  pub fn length(&self) -> u32 {
    let entry_ids = entry_type_ids(&self.module.inner);
    self
      .module
      .inner
      .types
      .iter()
      .filter(|t| !entry_ids.contains(&t.id()))
      .count() as u32
  }

  #[napi]
  /// Every type in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmType>> {
    let entry_ids = entry_type_ids(&self.module.inner);
    let ids: Vec<TypeId> = self
      .module
      .inner
      .types
      .iter()
      .map(|t| t.id())
      .filter(|id| !entry_ids.contains(id))
      .collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmType {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The type whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmType>> {
    let entry_ids = entry_type_ids(&self.module.inner);
    let id = self
      .module
      .inner
      .types
      .iter()
      .find(|t| t.id().index() as u32 == index)
      .map(|t| t.id())
      .filter(|id| !entry_ids.contains(id));
    match id {
      Some(id) => Ok(Some(WasmType {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// The first type with the given name, or `null` if none is named that.
  ///
  /// Note: walrus does not preserve type names through parsing, so this always
  /// returns `null` for a freshly parsed module. It matches names set in
  /// memory (via the `name` setter).
  pub fn by_name(&self, env: Env, name: String) -> Result<Option<WasmType>> {
    let id = self.module.inner.types.by_name(&name);
    match id {
      Some(id) => Ok(Some(WasmType {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete a type from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted type (walrus does not check, and a dangling reference aborts at
  /// emit time).
  ///
  /// Same no-panic invariant as the item accessors: walrus' `ModuleTypes::delete`
  /// indexes the arena first, which panics on a deleted/foreign id, and a panic
  /// across FFI aborts the process. Id equality includes the arena_id, so this
  /// liveness scan rejects BOTH already-deleted ids (`iter()` skips tombstoned
  /// entries) AND handles that belong to a different module (arena_id mismatch),
  /// surfacing a catchable JS error instead of aborting.
  pub fn delete(&mut self, ty: &WasmType) -> Result<()> {
    if self.module.inner.types.iter().any(|t| t.id() == ty.id) {
      self.module.inner.types.delete(ty.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("type"))
    }
  }

  #[napi]
  /// Add a new function type, returning a live handle to it.
  ///
  /// `params`/`results` are the function signature's value types. Fallible: an
  /// unsupported value type — currently a concrete/indexed ref type, which
  /// needs a type handle we do not yet thread through — is rejected with a
  /// catchable error rather than aborting.
  ///
  /// walrus deduplicates structurally: adding a signature identical to an
  /// existing type returns a handle to that existing type (the arena does not
  /// grow). This mirrors walrus and is intended behavior.
  ///
  /// The returned handle holds its own strong reference to the module (same as
  /// the accessor handles), so it stays valid as long as it is held.
  pub fn add(&mut self, env: Env, params: Vec<ValType>, results: Vec<ValType>) -> Result<WasmType> {
    // Convert (and reject unsupported value types) BEFORE touching the arena,
    // so a failed add never mutates the module.
    let params = to_walrus_valtypes(params)?;
    let results = to_walrus_valtypes(results)?;
    let id = self.module.inner.types.add(&params, &results);
    Ok(WasmType {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi]
  /// Find an existing function type with the given signature, or `null` if none
  /// matches.
  ///
  /// Fallible only because an unsupported (concrete/indexed) ref type in the
  /// query is rejected with a catchable error, same as `add`.
  pub fn find(
    &self,
    env: Env,
    params: Vec<ValType>,
    results: Vec<ValType>,
  ) -> Result<Option<WasmType>> {
    let params = to_walrus_valtypes(params)?;
    let results = to_walrus_valtypes(results)?;
    match self.module.inner.types.find(&params, &results) {
      Some(id) => Ok(Some(WasmType {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Add a new GC `struct` type (final, no supertype), returning a live handle.
  ///
  /// Each field's storage type is converted through the module-aware path, so a
  /// field may reference another type via a concrete ref
  /// (`{ type: 'Ref', heap: { type: 'Concrete', typeIndex } }`); an index that
  /// names no live type in this module is rejected with a catchable error
  /// BEFORE any arena mutation (a bogus index would otherwise abort at emit).
  /// Fields may reference only EXISTING types — a self-referential struct needs
  /// a rec group (a later task).
  ///
  /// walrus deduplicates structurally: adding a struct identical to an existing
  /// type returns a handle to that existing type (the arena does not grow).
  /// This mirrors walrus and is intended behavior.
  ///
  /// The returned handle holds its own strong reference to the module, so it
  /// stays valid as long as it is held.
  pub fn add_struct(&mut self, env: Env, fields: Vec<FieldType>) -> Result<WasmType> {
    // Convert (resolving each concrete ref, rejecting a bad index) BEFORE
    // touching the arena, so a failed add never mutates the module.
    let fields = fields
      .into_iter()
      .map(|f| crate::convert::field_type_to_walrus_in(&self.module.inner, f))
      .collect::<Result<Vec<_>>>()?;
    let id = self.module.inner.types.add_struct(fields);
    Ok(WasmType {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi]
  /// Add a new GC `array` type (final, no supertype), returning a live handle.
  ///
  /// The element's storage type is converted through the module-aware path, so
  /// it may be a concrete ref to another EXISTING type; a bad index is rejected
  /// with a catchable error before any arena mutation. Same structural
  /// deduplication as [`WasmTypes::add_struct`].
  pub fn add_array(&mut self, env: Env, element: FieldType) -> Result<WasmType> {
    let element = crate::convert::field_type_to_walrus_in(&self.module.inner, element)?;
    let id = self.module.inner.types.add_array(element);
    Ok(WasmType {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single type in a module, as a live handle: it holds the type's id plus a
/// strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmType {
  pub(crate) id: TypeId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmType {
  /// Confirm the type still exists before touching the arena.
  ///
  /// walrus' `types.get`/`get_mut` panic on a deleted id, which would abort the
  /// process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few types); do not prematurely
  /// optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.types.iter().any(|t| t.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("type"))
    }
  }
}

#[napi]
impl WasmType {
  #[napi(getter)]
  /// This type's stable index — its identity for numeric lookup. Readable even
  /// after the type is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This type's name, if any. walrus does not preserve type names through
  /// parsing, so this is `null` for a freshly parsed type; it reflects a name
  /// set in memory via the setter.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.types.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this type's name.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.types.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// This type's kind (`Function`, `Struct`, or `Array`). `Function` types have
  /// `params()` / `results()`; `Struct` / `Array` types have `structFields()` /
  /// `arrayElement()`.
  pub fn kind(&self) -> Result<TypeKind> {
    self.ensure_exists()?;
    // `walrus::CompositeType` is NOT `#[non_exhaustive]`, so this match is
    // total and needs no catch-all — a future variant would fail to compile.
    Ok(match self.module.inner.types.get(self.id).kind() {
      walrus::CompositeType::Function(_) => TypeKind::Function,
      walrus::CompositeType::Struct(_) => TypeKind::Struct,
      walrus::CompositeType::Array(_) => TypeKind::Array,
    })
  }

  #[napi]
  /// This function type's parameter value types.
  ///
  /// A method (not a getter) because it can fail: it throws a catchable error
  /// if this type is not a function type (a `Struct`/`Array` GC type). This is
  /// deliberate — walrus' `Type::params` calls `unwrap_function()` and PANICS
  /// on a non-function type, which would abort the process across FFI, so we
  /// go through `as_function()` and surface an error instead.
  pub fn params(&self) -> Result<Vec<ValType>> {
    self.ensure_exists()?;
    match self.module.inner.types.get(self.id).as_function() {
      Some(ft) => to_napi_valtypes(ft.params()),
      None => Err(Error::from_reason(format!(
        "type {} is not a function type",
        self.id.index()
      ))),
    }
  }

  #[napi]
  /// This function type's result value types.
  ///
  /// Same fallibility as [`WasmType::params`]: throws for a non-function type
  /// rather than hitting walrus' `unwrap_function` panic.
  pub fn results(&self) -> Result<Vec<ValType>> {
    self.ensure_exists()?;
    match self.module.inner.types.get(self.id).as_function() {
      Some(ft) => to_napi_valtypes(ft.results()),
      None => Err(Error::from_reason(format!(
        "type {} is not a function type",
        self.id.index()
      ))),
    }
  }

  #[napi]
  /// This GC `struct` type's field types.
  ///
  /// A method (not a getter) because it can fail: it throws a catchable error
  /// if this type is not a struct type (a `Function`/`Array` type). Same guard
  /// shape as [`WasmType::params`] — walrus' `unwrap_struct()` would PANIC on a
  /// non-struct type and abort the process across FFI, so we go through
  /// `as_struct()` and surface an error instead.
  pub fn struct_fields(&self) -> Result<Vec<FieldType>> {
    self.ensure_exists()?;
    match self.module.inner.types.get(self.id).as_struct() {
      Some(st) => st.fields.iter().copied().map(FieldType::try_from).collect(),
      None => Err(Error::from_reason(format!(
        "type {} is not a struct type",
        self.id.index()
      ))),
    }
  }

  #[napi]
  /// This GC `array` type's element field type.
  ///
  /// Same fallibility as [`WasmType::struct_fields`]: throws for a non-array
  /// type rather than hitting walrus' `unwrap_array` panic.
  pub fn array_element(&self) -> Result<FieldType> {
    self.ensure_exists()?;
    match self.module.inner.types.get(self.id).as_array() {
      Some(at) => FieldType::try_from(at.field),
      None => Err(Error::from_reason(format!(
        "type {} is not an array type",
        self.id.index()
      ))),
    }
  }

  #[napi(getter)]
  /// This type's declared supertype, or `null` if it has none.
  ///
  /// A pure id-wrap of `walrus::Type::supertype`. The returned handle holds its
  /// own strong reference to the module.
  pub fn supertype(&self, env: Env) -> Result<Option<WasmType>> {
    self.ensure_exists()?;
    match self.module.inner.types.get(self.id).supertype {
      Some(sup) => Ok(Some(WasmType {
        id: sup,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi(getter)]
  /// Whether this type is final (cannot be further subtyped). Types created by
  /// `addStruct` / `addArray` are final; walrus also defaults freshly parsed
  /// types without an explicit subtype declaration to final.
  pub fn is_final(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.types.get(self.id).is_final)
  }
}
