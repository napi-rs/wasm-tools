use napi::bindgen_prelude::{Reference, Result};
use napi::Env;
use napi_derive::napi;
use walrus::GlobalId;

use crate::constexpr::ConstExpr;
use crate::imports::WasmImport;
use crate::valtype::ValType;
use crate::WasmModule;

/// Whether a global is imported or locally defined.
///
/// Mirrors the discriminant of `walrus::GlobalKind` (`Import(ImportId)` /
/// `Local(ConstExpr)`). The companion accessors that expose the import id or
/// the local initializer are deferred to a later task.
#[napi(string_enum)]
pub enum GlobalKind {
  /// An imported global (its initializer lives in the host).
  Import,
  /// A locally defined global (has an in-module initializer).
  Local,
}

/// The globals of a module. Each accessor materializes a fresh [`WasmGlobal`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
#[napi]
pub struct WasmGlobals {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmGlobals {
  #[napi(getter)]
  /// The number of globals in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.globals.iter().count() as u32
  }

  #[napi]
  /// Every global in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmGlobal>> {
    let ids: Vec<GlobalId> = self.module.inner.globals.iter().map(|g| g.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmGlobal {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The global whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmGlobal>> {
    let id = self
      .module
      .inner
      .globals
      .iter()
      .find(|g| g.id().index() as u32 == index)
      .map(|g| g.id());
    match id {
      Some(id) => Ok(Some(WasmGlobal {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// The first global with the given name, or `null` if none is named that.
  pub fn by_name(&self, env: Env, name: String) -> Result<Option<WasmGlobal>> {
    let id = self
      .module
      .inner
      .globals
      .iter()
      .find(|g| g.name.as_deref() == Some(name.as_str()))
      .map(|g| g.id());
    match id {
      Some(id) => Ok(Some(WasmGlobal {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete a global from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted global (walrus does not check).
  ///
  /// Same no-panic invariant as the item accessors: walrus'
  /// `ModuleGlobals::delete` asserts the id is live, and a panic across FFI
  /// aborts the process. Id equality includes the arena_id, so this liveness
  /// scan rejects BOTH already-deleted ids (`iter()` skips tombstoned entries)
  /// AND handles that belong to a different module (arena_id mismatch),
  /// surfacing a catchable JS error instead of aborting.
  pub fn delete(&mut self, global: &WasmGlobal) -> Result<()> {
    if self
      .module
      .inner
      .globals
      .iter()
      .any(|g| g.id() == global.id)
    {
      self.module.inner.globals.delete(global.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("global"))
    }
  }

  #[napi]
  /// Add a new locally defined global, returning a live handle to it.
  ///
  /// `ty` is the global's value type (e.g. `{ type: 'I32' }` or a `Ref`), and
  /// `init` is its constant initializer (build one with the `ConstExpr`
  /// factories). `ty` may be a concrete ref to an EXISTING type in this module
  /// (`{ type: 'Ref', heap: { type: 'Concrete', typeIndex } }`); an index that
  /// names no live type is rejected with a catchable error rather than aborting.
  ///
  /// The returned handle holds its own strong reference to the module (same as
  /// the accessor handles), so it stays valid as long as it is held.
  pub fn add_local(
    &mut self,
    env: Env,
    ty: ValType,
    mutable: bool,
    shared: bool,
    init: &ConstExpr,
  ) -> Result<WasmGlobal> {
    // Convert (resolving a concrete ref, rejecting a bad index) BEFORE touching
    // the arena, so a failed add never mutates the module.
    let wty = crate::convert::val_type_to_walrus_in(&self.module.inner, ty)?;
    // Reject an initializer that references a global/function id not live in
    // THIS module (a foreign-module handle or an already-deleted id) BEFORE it
    // can reach emit, where walrus would abort the whole Node process.
    crate::handle::validate_const_expr(&self.module.inner, &init.inner)?;
    let id = self
      .module
      .inner
      .globals
      .add_local(wty, mutable, shared, init.inner.clone());
    Ok(WasmGlobal {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single global in a module, as a live handle: it holds the global's id plus
/// a strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmGlobal {
  pub(crate) id: GlobalId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmGlobal {
  /// Confirm the global still exists before touching the arena.
  ///
  /// walrus' `globals.get`/`get_mut` panic on a deleted id, which would abort
  /// the process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — see B3 note. Acceptable here (modules have few globals); do
  /// not prematurely optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.globals.iter().any(|g| g.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("global"))
    }
  }
}

#[napi]
impl WasmGlobal {
  #[napi(getter)]
  /// This global's stable index — its identity for numeric lookup. Readable
  /// even after the global is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This global's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.globals.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this global's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.globals.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this global is mutable.
  pub fn mutable(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.globals.get(self.id).mutable)
  }

  #[napi(setter)]
  /// Set whether this global is mutable.
  pub fn set_mutable(&mut self, value: bool) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.globals.get_mut(self.id).mutable = value;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this global is shared (a creation-time property, read only).
  pub fn shared(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.globals.get(self.id).shared)
  }

  #[napi(getter)]
  /// This global's value type (read only).
  pub fn ty(&self) -> Result<ValType> {
    self.ensure_exists()?;
    // Fallible: the value type may embed a `#[non_exhaustive]` walrus HeapType.
    // A later 0.26.x (Cargo.lock is untracked) could add a variant we don't
    // map; surface that as a catchable JS error, never a process-aborting panic.
    self.module.inner.globals.get(self.id).ty.try_into()
  }

  #[napi(getter)]
  /// Whether this global is imported or locally defined (read only).
  pub fn kind(&self) -> Result<GlobalKind> {
    self.ensure_exists()?;
    Ok(match self.module.inner.globals.get(self.id).kind {
      walrus::GlobalKind::Import(_) => GlobalKind::Import,
      walrus::GlobalKind::Local(_) => GlobalKind::Local,
    })
  }

  #[napi]
  /// This global's constant initializer, or `null` if it is imported.
  ///
  /// A locally defined global carries a `ConstExpr` initializer; an imported
  /// global's initial value lives in the host, so this returns `null` for it.
  /// A method (not a getter) because it materializes a fresh `ConstExpr`
  /// wrapper on each call.
  pub fn init(&self) -> Result<Option<ConstExpr>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.globals.get(self.id).kind {
      walrus::GlobalKind::Local(init) => Some(ConstExpr {
        inner: init.clone(),
      }),
      walrus::GlobalKind::Import(_) => None,
    })
  }

  #[napi]
  /// The import that brings this global into the module, as a live
  /// [`WasmImport`] handle, or `null` if this global is locally defined.
  ///
  /// A method (not a getter) because it materializes a fresh wrapper on each
  /// call. Wrapping the id is a pure cross-link (the reverse of
  /// `WasmImport.global()`); a later access on the returned handle self-guards
  /// against the import having been deleted.
  pub fn import(&self, env: Env) -> Result<Option<WasmImport>> {
    self.ensure_exists()?;
    match &self.module.inner.globals.get(self.id).kind {
      walrus::GlobalKind::Import(id) => Ok(Some(WasmImport {
        id: *id,
        module: self.module.clone(env)?,
      })),
      walrus::GlobalKind::Local(_) => Ok(None),
    }
  }
}
