use napi::bindgen_prelude::{Reference, Result};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::{ExportId, ExportItem};

use crate::functions::WasmFunction;
use crate::globals::WasmGlobal;
use crate::memories::WasmMemory;
use crate::tables::WasmTable;
use crate::tags::WasmTag;
use crate::WasmModule;

/// The kind of item an export exposes from a module.
///
/// Mirrors the discriminant of `walrus::ExportItem`
/// (`Function`/`Table`/`Memory`/`Global`/`Tag`). The exported item itself is
/// read through the matching typed accessor on [`WasmExport`]
/// (`func`/`table`/`memory`/`global`/`tag`), each of which returns the handle
/// for its variant and `null` for the others.
#[napi(string_enum)]
pub enum ExportItemTag {
  /// An exported function.
  Function,
  /// An exported table.
  Table,
  /// An exported memory.
  Memory,
  /// An exported global.
  Global,
  /// An exported tag (exception handling).
  Tag,
}

/// The exports of a module. Each accessor materializes a fresh [`WasmExport`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
#[napi]
pub struct WasmExports {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmExports {
  #[napi(getter)]
  /// The number of exports in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.exports.iter().count() as u32
  }

  #[napi]
  /// Every export in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmExport>> {
    let ids: Vec<ExportId> = self.module.inner.exports.iter().map(|e| e.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmExport {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The export whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: f64) -> Result<Option<WasmExport>> {
    let index = crate::convert::checked_index(index, "index")?;
    let id = self
      .module
      .inner
      .exports
      .iter()
      .find(|e| e.id().index() as u32 == index)
      .map(|e| e.id());
    match id {
      Some(id) => Ok(Some(WasmExport {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// The export with the given `name`, or `null` if none exists. Exports have no
  /// name index in walrus, so this scans (the id-bridge "scan and return the
  /// real id" design); export names are unique in a valid module.
  pub fn by_name(&self, env: Env, name: String) -> Result<Option<WasmExport>> {
    let id = self
      .module
      .inner
      .exports
      .iter()
      .find(|e| e.name == name)
      .map(|e| e.id());
    match id {
      Some(id) => Ok(Some(WasmExport {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi(strict)]
  /// Delete an export from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// Unlike active data/element segments, an export id is NOT stored in any
  /// parser-maintained back-link set that gc/emit dereferences (verified: no
  /// `exports.get(<id>)` call exists in walrus' passes/emit — exports are read
  /// only via `exports.iter()`). So a plain guarded delete is sufficient — there
  /// is no back-link to clean. Id equality includes the arena_id, so this
  /// liveness scan rejects BOTH already-deleted ids AND handles from a different
  /// module, surfacing a catchable JS error instead of tripping walrus'
  /// `delete` assertion and aborting the process across FFI.
  pub fn delete(&mut self, handle: &WasmExport) -> Result<()> {
    if self
      .module
      .inner
      .exports
      .iter()
      .any(|e| e.id() == handle.id)
    {
      self.module.inner.exports.delete(handle.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("export"))
    }
  }

  #[napi(strict)]
  /// Export the given function under `name`, returning a live handle to the new
  /// export.
  ///
  /// `add_function` is a CONSUME site for an arena id: walrus stores the raw
  /// `FunctionId` and resolves it to an index at emit via a panicking
  /// `get_func_index`; a foreign-module or already-deleted handle would abort
  /// the whole Node process there. We reject such an id with a catchable error
  /// BEFORE touching the arena, so a failed add never mutates the module. Same
  /// id-ref rule as `data.addActive` / `tags.add`.
  pub fn add_function(
    &mut self,
    env: Env,
    name: String,
    func: &WasmFunction,
  ) -> Result<WasmExport> {
    if !self.module.inner.funcs.iter().any(|f| f.id() == func.id) {
      return Err(Error::from_reason(
        "function is not in this module (or was deleted)",
      ));
    }
    let id = self.module.inner.exports.add(&name, func.id);
    Ok(WasmExport {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi(strict)]
  /// Export the given table under `name`, returning a live handle to the new
  /// export. The table must belong to THIS module (id-ref guard; see
  /// [`WasmExports::add_function`]).
  pub fn add_table(&mut self, env: Env, name: String, table: &WasmTable) -> Result<WasmExport> {
    if !self.module.inner.tables.iter().any(|t| t.id() == table.id) {
      return Err(Error::from_reason(
        "table is not in this module (or was deleted)",
      ));
    }
    let id = self.module.inner.exports.add(&name, table.id);
    Ok(WasmExport {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi(strict)]
  /// Export the given memory under `name`, returning a live handle to the new
  /// export. The memory must belong to THIS module (id-ref guard; see
  /// [`WasmExports::add_function`]).
  pub fn add_memory(&mut self, env: Env, name: String, memory: &WasmMemory) -> Result<WasmExport> {
    if !self
      .module
      .inner
      .memories
      .iter()
      .any(|m| m.id() == memory.id)
    {
      return Err(Error::from_reason(
        "memory is not in this module (or was deleted)",
      ));
    }
    let id = self.module.inner.exports.add(&name, memory.id);
    Ok(WasmExport {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi(strict)]
  /// Export the given global under `name`, returning a live handle to the new
  /// export. The global must belong to THIS module (id-ref guard; see
  /// [`WasmExports::add_function`]).
  pub fn add_global(&mut self, env: Env, name: String, global: &WasmGlobal) -> Result<WasmExport> {
    if !self
      .module
      .inner
      .globals
      .iter()
      .any(|g| g.id() == global.id)
    {
      return Err(Error::from_reason(
        "global is not in this module (or was deleted)",
      ));
    }
    let id = self.module.inner.exports.add(&name, global.id);
    Ok(WasmExport {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi(strict)]
  /// Export the given tag under `name`, returning a live handle to the new
  /// export. The tag must belong to THIS module (id-ref guard; see
  /// [`WasmExports::add_function`]).
  pub fn add_tag(&mut self, env: Env, name: String, tag: &WasmTag) -> Result<WasmExport> {
    if !self.module.inner.tags.iter().any(|t| t.id() == tag.id) {
      return Err(Error::from_reason(
        "tag is not in this module (or was deleted)",
      ));
    }
    let id = self.module.inner.exports.add(&name, tag.id);
    Ok(WasmExport {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single export in a module, as a live handle: it holds the export's id plus
/// a strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmExport {
  pub(crate) id: ExportId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmExport {
  /// Confirm the export still exists before touching the arena.
  ///
  /// walrus' `exports.get`/`get_mut` panic on a deleted id, which would abort
  /// the process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few exports); do not prematurely
  /// optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.exports.iter().any(|e| e.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("export"))
    }
  }
}

#[napi]
impl WasmExport {
  #[napi(getter)]
  /// This export's stable index — its identity for numeric lookup. Readable even
  /// after the export is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// The name this item is exported under.
  pub fn name(&self) -> Result<String> {
    self.ensure_exists()?;
    Ok(self.module.inner.exports.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set the name this item is exported under.
  pub fn set_name(&mut self, name: String) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.exports.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// The kind of item this export exposes (read only).
  pub fn kind(&self) -> Result<ExportItemTag> {
    self.ensure_exists()?;
    // `walrus::ExportItem` is NOT `#[non_exhaustive]`, so this match is total
    // and needs no catch-all — a future variant would fail to compile.
    Ok(match self.module.inner.exports.get(self.id).item {
      ExportItem::Function(_) => ExportItemTag::Function,
      ExportItem::Table(_) => ExportItemTag::Table,
      ExportItem::Memory(_) => ExportItemTag::Memory,
      ExportItem::Global(_) => ExportItemTag::Global,
      ExportItem::Tag(_) => ExportItemTag::Tag,
    })
  }

  #[napi]
  /// The exported function as a live [`WasmFunction`] handle, or `null` if this
  /// export is not a function.
  ///
  /// A method (not a getter) because it materializes a fresh wrapper on each
  /// call. Wrapping the id is a pure cross-link; a later access on the returned
  /// handle self-guards against the target having been deleted.
  pub fn func(&self, env: Env) -> Result<Option<WasmFunction>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.exports.get(self.id).item {
      ExportItem::Function(fid) => Some(WasmFunction {
        id: fid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The exported table as a live [`WasmTable`] handle, or `null` if this export
  /// is not a table.
  pub fn table(&self, env: Env) -> Result<Option<WasmTable>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.exports.get(self.id).item {
      ExportItem::Table(tid) => Some(WasmTable {
        id: tid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The exported memory as a live [`WasmMemory`] handle, or `null` if this
  /// export is not a memory.
  pub fn memory(&self, env: Env) -> Result<Option<WasmMemory>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.exports.get(self.id).item {
      ExportItem::Memory(mid) => Some(WasmMemory {
        id: mid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The exported global as a live [`WasmGlobal`] handle, or `null` if this
  /// export is not a global.
  pub fn global(&self, env: Env) -> Result<Option<WasmGlobal>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.exports.get(self.id).item {
      ExportItem::Global(gid) => Some(WasmGlobal {
        id: gid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }

  #[napi]
  /// The exported tag as a live [`WasmTag`] handle, or `null` if this export is
  /// not a tag.
  pub fn tag(&self, env: Env) -> Result<Option<WasmTag>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.exports.get(self.id).item {
      ExportItem::Tag(tid) => Some(WasmTag {
        id: tid,
        module: self.module.clone(env)?,
      }),
      _ => None,
    })
  }
}
