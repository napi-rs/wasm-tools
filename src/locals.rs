use napi::bindgen_prelude::{Reference, Result};
use napi::Env;
use napi_derive::napi;
use walrus::LocalId;

use crate::valtype::ValType;
use crate::WasmModule;

/// The locals of a module. Each accessor materializes a fresh [`WasmLocal`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
///
/// NOTE: `ModuleLocals` is the MODULE-WIDE local arena — it holds every local
/// (and parameter) across all function bodies, not the locals of one function.
/// A local added here but never referenced by any function body is simply not
/// emitted (walrus drops unused locals), which is harmless.
#[napi]
pub struct WasmLocals {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmLocals {
  #[napi(getter)]
  /// The number of locals in the module (across all function bodies).
  pub fn length(&self) -> u32 {
    self.module.inner.locals.iter().count() as u32
  }

  #[napi]
  /// Every local in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmLocal>> {
    let ids: Vec<LocalId> = self.module.inner.locals.iter().map(|l| l.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmLocal {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The local whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmLocal>> {
    let id = self
      .module
      .inner
      .locals
      .iter()
      .find(|l| l.id().index() as u32 == index)
      .map(|l| l.id());
    match id {
      Some(id) => Ok(Some(WasmLocal {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Add a new local of the given value type, returning a live handle to it.
  ///
  /// `ty` is the local's value type (e.g. `{ type: 'I64' }` or a `Ref`).
  /// Fallible: an unsupported `ty` — currently a concrete/indexed ref type,
  /// which needs a type handle we do not yet thread through — is rejected with a
  /// catchable error rather than aborting.
  ///
  /// The local is added to the module-wide arena; it only reaches the emitted
  /// output if some function body references it. The returned handle holds its
  /// own strong reference to the module (same as the accessor handles), so it
  /// stays valid as long as it is held.
  pub fn add(&mut self, env: Env, ty: ValType) -> Result<WasmLocal> {
    // Convert (and reject an unsupported value type) BEFORE touching the arena,
    // so a failed add never mutates the module.
    let wty: walrus::ValType = ty.try_into()?;
    let id = self.module.inner.locals.add(wty);
    Ok(WasmLocal {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single local in a module, as a live handle: it holds the local's id plus a
/// strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmLocal {
  pub(crate) id: LocalId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmLocal {
  /// Confirm the local still exists before touching the arena.
  ///
  /// walrus' `locals.get`/`get_mut` panic on a missing id, which would abort the
  /// process across FFI; this turns that into a catchable JS error. `ModuleLocals`
  /// has no `delete`, so an id only fails this scan if it belongs to a different
  /// module (arena_id mismatch).
  ///
  /// O(n) guard — acceptable here; do not prematurely optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.locals.iter().any(|l| l.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("local"))
    }
  }
}

#[napi]
impl WasmLocal {
  #[napi(getter)]
  /// This local's stable index — its identity for numeric lookup. Readable even
  /// if the local is not (or no longer) in this module's arena (it never touches
  /// the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This local's value type (read only — walrus exposes no setter).
  pub fn ty(&self) -> Result<ValType> {
    self.ensure_exists()?;
    // Fallible: the value type may embed a `#[non_exhaustive]` walrus HeapType.
    // A later 0.26.x (Cargo.lock is untracked) could add a variant we don't map;
    // surface that as a catchable JS error, never a process-aborting panic.
    self.module.inner.locals.get(self.id).ty().try_into()
  }

  #[napi(getter)]
  /// This local's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.locals.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this local's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.locals.get_mut(self.id).name = name;
    Ok(())
  }
}
