use napi::bindgen_prelude::{Reference, Result};
use napi::Env;
use napi_derive::napi;
use walrus::{ImportId, ImportKind};

use crate::functions::WasmFunction;
use crate::globals::WasmGlobal;
use crate::memories::WasmMemory;
use crate::tables::WasmTable;
use crate::tags::WasmTag;
use crate::WasmModule;

/// The kind of item an import brings into a module.
///
/// Mirrors the discriminant of `walrus::ImportKind`
/// (`Function`/`Table`/`Memory`/`Global`/`Tag`). The imported item itself is
/// read through the matching typed accessor on [`WasmImport`]
/// (`func`/`table`/`memory`/`global`/`tag`), each of which returns the handle
/// for its variant and `null` for the others.
#[napi(string_enum)]
pub enum ImportKindTag {
  /// An imported function.
  Function,
  /// An imported table.
  Table,
  /// An imported memory.
  Memory,
  /// An imported global.
  Global,
  /// An imported tag (exception handling).
  Tag,
}

/// The imports of a module. Each accessor materializes a fresh [`WasmImport`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
///
/// Import *creation* (`add_import_*`) is intentionally not exposed here.
#[napi]
pub struct WasmImports {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmImports {
  #[napi(getter)]
  /// The number of imports in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.imports.iter().count() as u32
  }

  #[napi]
  /// Every import in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmImport>> {
    let ids: Vec<ImportId> = self.module.inner.imports.iter().map(|i| i.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmImport {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The import whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmImport>> {
    let id = self
      .module
      .inner
      .imports
      .iter()
      .find(|i| i.id().index() as u32 == index)
      .map(|i| i.id());
    match id {
      Some(id) => Ok(Some(WasmImport {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// The import with the given `module` and `name`, or `null` if none exists.
  pub fn find(&self, env: Env, module: String, name: String) -> Result<Option<WasmImport>> {
    match self.module.inner.imports.find(&module, &name) {
      Some(id) => Ok(Some(WasmImport {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete an import from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// Deleting an import that still has a defined item (the imported function /
  /// table / memory / global / tag) ORPHANS that item, producing a wasm-invalid
  /// module — the caller's responsibility, catchable via `WebAssembly.validate`
  /// / re-parse (mirror-walrus policy: we do not try to prevent it).
  ///
  /// Unlike active data/element segments, an import id is NOT stored in any
  /// parser-maintained back-link set that gc/emit dereferences: gc and emit only
  /// read `item.import` as an `Option`/bool and never call `imports.get(id)` on
  /// a stored id (verified: no `imports.get(<id>)` call exists in walrus'
  /// passes/emit). So a plain guarded delete is sufficient — there is no
  /// back-link to clean. Id equality includes the arena_id, so this liveness
  /// scan rejects BOTH already-deleted ids (`iter()` skips tombstoned entries)
  /// AND handles that belong to a different module, surfacing a catchable JS
  /// error instead of tripping walrus' `delete` assertion and aborting the
  /// process across FFI.
  pub fn delete(&mut self, handle: &WasmImport) -> Result<()> {
    if self
      .module
      .inner
      .imports
      .iter()
      .any(|i| i.id() == handle.id)
    {
      self.module.inner.imports.delete(handle.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("import"))
    }
  }
}

/// A single import in a module, as a live handle: it holds the import's id plus
/// a strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmImport {
  pub(crate) id: ImportId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmImport {
  /// Confirm the import still exists before touching the arena.
  ///
  /// walrus' `imports.get`/`get_mut` panic on a deleted id, which would abort
  /// the process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few imports); do not prematurely
  /// optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.imports.iter().any(|i| i.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("import"))
    }
  }
}

#[napi]
impl WasmImport {
  #[napi(getter)]
  /// This import's stable index — its identity for numeric lookup. Readable even
  /// after the import is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// The module name this item is imported from (e.g. `"env"`).
  pub fn module(&self) -> Result<String> {
    self.ensure_exists()?;
    Ok(self.module.inner.imports.get(self.id).module.clone())
  }

  #[napi(setter)]
  /// Set the module name this item is imported from.
  pub fn set_module(&mut self, module: String) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.imports.get_mut(self.id).module = module;
    Ok(())
  }

  #[napi(getter)]
  /// The field name of this import within its module.
  pub fn name(&self) -> Result<String> {
    self.ensure_exists()?;
    Ok(self.module.inner.imports.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set the field name of this import within its module.
  pub fn set_name(&mut self, name: String) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.imports.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// The kind of item this import brings in (read only).
  pub fn kind(&self) -> Result<ImportKindTag> {
    self.ensure_exists()?;
    // `walrus::ImportKind` is NOT `#[non_exhaustive]`, so this match is total
    // and needs no catch-all — a future variant would fail to compile.
    Ok(match self.module.inner.imports.get(self.id).kind {
      ImportKind::Function(_) => ImportKindTag::Function,
      ImportKind::Table(_) => ImportKindTag::Table,
      ImportKind::Memory(_) => ImportKindTag::Memory,
      ImportKind::Global(_) => ImportKindTag::Global,
      ImportKind::Tag(_) => ImportKindTag::Tag,
    })
  }

  #[napi]
  /// The imported function as a live [`WasmFunction`] handle, or `null` if this
  /// import is not a function.
  ///
  /// A method (not a getter) because it materializes a fresh wrapper on each
  /// call. Wrapping the id is a pure cross-link; a later access on the returned
  /// handle self-guards against the target having been deleted.
  pub fn func(&self, env: Env) -> Result<Option<WasmFunction>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.imports.get(self.id).kind {
      ImportKind::Function(fid) => Some(WasmFunction {
        id: *fid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The imported table as a live [`WasmTable`] handle, or `null` if this import
  /// is not a table.
  pub fn table(&self, env: Env) -> Result<Option<WasmTable>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.imports.get(self.id).kind {
      ImportKind::Table(tid) => Some(WasmTable {
        id: *tid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The imported memory as a live [`WasmMemory`] handle, or `null` if this
  /// import is not a memory.
  pub fn memory(&self, env: Env) -> Result<Option<WasmMemory>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.imports.get(self.id).kind {
      ImportKind::Memory(mid) => Some(WasmMemory {
        id: *mid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The imported global as a live [`WasmGlobal`] handle, or `null` if this
  /// import is not a global.
  pub fn global(&self, env: Env) -> Result<Option<WasmGlobal>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.imports.get(self.id).kind {
      ImportKind::Global(gid) => Some(WasmGlobal {
        id: *gid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The imported tag as a live [`WasmTag`] handle, or `null` if this import is
  /// not a tag.
  pub fn tag(&self, env: Env) -> Result<Option<WasmTag>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.imports.get(self.id).kind {
      ImportKind::Tag(tid) => Some(WasmTag {
        id: *tid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }
}
