use napi::bindgen_prelude::{BigInt, Reference, Result};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::TableId;

use crate::constexpr::ConstExpr;
use crate::valtype::ValType;
use crate::WasmModule;

/// The tables of a module. Each accessor materializes a fresh [`WasmTable`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
#[napi]
pub struct WasmTables {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmTables {
  #[napi(getter)]
  /// The number of tables in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.tables.iter().count() as u32
  }

  #[napi]
  /// Every table in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmTable>> {
    let ids: Vec<TableId> = self.module.inner.tables.iter().map(|t| t.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmTable {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The table whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmTable>> {
    let id = self
      .module
      .inner
      .tables
      .iter()
      .find(|t| t.id().index() as u32 == index)
      .map(|t| t.id());
    match id {
      Some(id) => Ok(Some(WasmTable {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// The module's single function (`funcref`) table, or `null` if it has none.
  ///
  /// Mirrors walrus' `main_function_table`: modules produced by compilers like
  /// LLVM typically have exactly one function table for indirect calls. Rejects
  /// with a catchable error if the module has more than one function table.
  pub fn main_function_table(&self, env: Env) -> Result<Option<WasmTable>> {
    let id = self.module.inner.tables.main_function_table()?;
    match id {
      Some(id) => Ok(Some(WasmTable {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete a table from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted table (walrus does not check).
  ///
  /// Same no-panic invariant as the item accessors: walrus'
  /// `ModuleTables::delete` asserts the id is live, and a panic across FFI
  /// aborts the process. Id equality includes the arena_id, so this liveness
  /// scan rejects BOTH already-deleted ids (`iter()` skips tombstoned entries)
  /// AND handles that belong to a different module (arena_id mismatch),
  /// surfacing a catchable JS error instead of aborting.
  pub fn delete(&mut self, table: &WasmTable) -> Result<()> {
    if self.module.inner.tables.iter().any(|t| t.id() == table.id) {
      self.module.inner.tables.delete(table.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("table"))
    }
  }

  #[napi]
  /// Add a new locally defined table, returning a live handle to it.
  ///
  /// `initial`/`maximum` are entry counts (`bigint`, so 64-bit `table64` tables
  /// are representable losslessly); `maximum` is `null` for an unbounded table.
  /// `elementTy` must be a reference type (e.g. a `funcref`/`externref`
  /// `{ type: 'Ref', ... }`); a non-reference type is rejected with a catchable
  /// error.
  ///
  /// This builds a NULL-initialized table (no `init` expression), so
  /// `elementTy` must be nullable — building a table with a non-nullable
  /// element type needs an initializer, which is deferred to a later task.
  ///
  /// The returned handle holds its own strong reference to the module (same as
  /// the accessor handles), so it stays valid as long as it is held.
  pub fn add_local(
    &mut self,
    env: Env,
    table64: bool,
    initial: BigInt,
    maximum: Option<BigInt>,
    element_ty: ValType,
  ) -> Result<WasmTable> {
    // Reject an unsupported / non-reference element type BEFORE touching the
    // arena, so a failed add never mutates the module.
    let wty: walrus::ValType = element_ty.try_into()?;
    let element_ty = match wty {
      walrus::ValType::Ref(rt) => rt,
      _ => {
        return Err(Error::from_reason(
          "table element type must be a reference type",
        ))
      }
    };
    // `add_local` builds a NULL-initialized table (`init: None`), and walrus
    // requires an initializer for a non-defaultable (non-nullable) element type
    // — a non-nullable element here would emit a table that fails validation.
    // Reject it BEFORE touching the arena (init-with-value is a deferred task).
    if !element_ty.nullable {
      return Err(Error::from_reason(
        "a non-nullable table element type requires an initializer; addLocal creates a null-initialized table, so its element type must be nullable",
      ));
    }
    let initial = crate::handle::bigint_to_u64(initial, "initial")?;
    let maximum = maximum
      .map(|m| crate::handle::bigint_to_u64(m, "maximum"))
      .transpose()?;
    let id = self
      .module
      .inner
      .tables
      .add_local(table64, initial, maximum, element_ty);
    Ok(WasmTable {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single table in a module, as a live handle: it holds the table's id plus a
/// strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmTable {
  pub(crate) id: TableId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmTable {
  /// Confirm the table still exists before touching the arena.
  ///
  /// walrus' `tables.get`/`get_mut` panic on a deleted id, which would abort
  /// the process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few tables); do not prematurely
  /// optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.tables.iter().any(|t| t.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("table"))
    }
  }
}

#[napi]
impl WasmTable {
  #[napi(getter)]
  /// This table's stable index — its identity for numeric lookup. Readable even
  /// after the table is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This table's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.tables.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this table's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.tables.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this is a 64-bit table (a creation-time property, read only).
  pub fn table64(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.tables.get(self.id).table64)
  }

  #[napi(getter)]
  /// This table's initial size, in entries (`bigint`).
  pub fn initial(&self) -> Result<u64> {
    self.ensure_exists()?;
    Ok(self.module.inner.tables.get(self.id).initial)
  }

  #[napi(setter)]
  /// Set this table's initial size, in entries.
  pub fn set_initial(&mut self, value: BigInt) -> Result<()> {
    // Convert (and reject a bad size) BEFORE the liveness check and the arena
    // write, so a bad input never mutates the module.
    let value = crate::handle::bigint_to_u64(value, "initial")?;
    self.ensure_exists()?;
    self.module.inner.tables.get_mut(self.id).initial = value;
    Ok(())
  }

  #[napi(getter)]
  /// This table's optional maximum size, in entries (`bigint`), or `null` if
  /// unbounded.
  pub fn maximum(&self) -> Result<Option<u64>> {
    self.ensure_exists()?;
    Ok(self.module.inner.tables.get(self.id).maximum)
  }

  #[napi(setter)]
  /// Set this table's optional maximum size, in entries. `null` clears it.
  pub fn set_maximum(&mut self, value: Option<BigInt>) -> Result<()> {
    // Convert (and reject a bad size) BEFORE the liveness check and the arena
    // write, so a bad input never mutates the module.
    let value = value
      .map(|m| crate::handle::bigint_to_u64(m, "maximum"))
      .transpose()?;
    self.ensure_exists()?;
    self.module.inner.tables.get_mut(self.id).maximum = value;
    Ok(())
  }

  #[napi(getter)]
  /// This table's element type, always a reference type (read only).
  ///
  /// Represented as the `ValType::Ref` variant. Fallible: the ref's heap type
  /// may embed a `#[non_exhaustive]` walrus heap variant a later 0.26.x adds;
  /// that surfaces as a catchable JS error, never a process-aborting panic.
  pub fn element_ty(&self) -> Result<ValType> {
    self.ensure_exists()?;
    let rt = self.module.inner.tables.get(self.id).element_ty;
    Ok(ValType::Ref {
      nullable: rt.nullable,
      heap: rt.heap_type.try_into()?,
    })
  }

  #[napi]
  /// This table's initializer expression, or `null` if it is null-initialized.
  ///
  /// A method (not a getter) because it materializes a fresh `ConstExpr`
  /// wrapper on each call. Read only: building a table WITH an initializer is
  /// deferred to a later task.
  pub fn init(&self) -> Result<Option<ConstExpr>> {
    self.ensure_exists()?;
    Ok(
      self
        .module
        .inner
        .tables
        .get(self.id)
        .init
        .as_ref()
        .map(|init| ConstExpr {
          inner: init.clone(),
        }),
    )
  }

  #[napi(getter)]
  /// Whether this table is imported. The import handle itself is exposed by a
  /// later imports task; only the boolean is available now.
  pub fn is_imported(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.tables.get(self.id).import.is_some())
  }
}
