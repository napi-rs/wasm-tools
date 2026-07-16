use napi::bindgen_prelude::{Reference, Result};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::FunctionId;

use crate::imports::WasmImport;
use crate::ir::{read_instr_seq, InstrDesc};
use crate::types::WasmType;
use crate::WasmModule;

/// Whether a function is imported, locally defined, or an uninitialized
/// placeholder.
///
/// Mirrors the discriminant of `walrus::FunctionKind`
/// (`Import(ImportedFunction) | Local(LocalFunction) | Uninitialized(TypeId)`).
/// The companion accessors that expose the import handle or the function body
/// are deferred to later tasks; only the tag is exposed here.
#[napi(string_enum)]
pub enum FunctionKindTag {
  /// An externally defined, imported function.
  Import,
  /// A locally defined function (has an in-module body).
  Local,
  /// A locally defined function whose body has not been parsed yet. This is an
  /// internal walrus transient (it should not appear on a fully parsed module),
  /// exposed for completeness.
  Uninitialized,
}

/// The functions of a module. Each accessor materializes a fresh [`WasmFunction`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
#[napi]
pub struct WasmFunctions {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmFunctions {
  #[napi(getter)]
  /// The number of functions in the module (imported and locally defined).
  pub fn length(&self) -> u32 {
    self.module.inner.funcs.iter().count() as u32
  }

  #[napi]
  /// Every function in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmFunction>> {
    let ids: Vec<FunctionId> = self.module.inner.funcs.iter().map(|f| f.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmFunction {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The function whose stable `.index` equals `index`, or `null` if none
  /// exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmFunction>> {
    let id = self
      .module
      .inner
      .funcs
      .iter()
      .find(|f| f.id().index() as u32 == index)
      .map(|f| f.id());
    match id {
      Some(id) => Ok(Some(WasmFunction {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// The first function with the given name, or `null` if none is named that.
  ///
  /// The name matched is the wasm "name" custom section name, not the export
  /// name. walrus preserves function names through parsing, so a name from the
  /// source module is findable here.
  pub fn by_name(&self, env: Env, name: String) -> Result<Option<WasmFunction>> {
    // `ModuleFunctions` has a native `by_name` (unlike globals/memories, which
    // must be scanned).
    match self.module.inner.funcs.by_name(&name) {
      Some(id) => Ok(Some(WasmFunction {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete a function from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted function (walrus does not check, and a dangling `call`/export/table
  /// element aborts at emit time).
  ///
  /// Same no-panic invariant as the item accessors: walrus'
  /// `ModuleFunctions::delete` tombstones the arena entry and a later access
  /// asserts liveness, and a panic across FFI aborts the process. Id equality
  /// includes the arena_id, so this liveness scan rejects BOTH already-deleted
  /// ids (`iter()` skips tombstoned entries) AND handles that belong to a
  /// different module (arena_id mismatch), surfacing a catchable JS error
  /// instead of aborting.
  pub fn delete(&mut self, func: &WasmFunction) -> Result<()> {
    if !self.module.inner.funcs.iter().any(|f| f.id() == func.id) {
      return Err(crate::handle::deleted("function"));
    }
    // walrus mints an internal "function-entry" type per local function (the
    // `MultiValue` type of its entry block) but `ModuleFunctions::delete` only
    // tombstones the function — it leaves that entry type live in the type
    // arena. An orphaned entry type is invisible to `entry_type_ids` (nothing
    // references it any more), so it would leak back into `WasmTypes` and
    // become resolvable by `add*` — a concrete ref to it then aborts at emit
    // (uncatchable on WASI). So after deleting a LOCAL function we drop its
    // entry type too, but only once NO remaining local function still uses it:
    // walrus dedups entry types structurally, so several functions with the
    // same result signature share one entry type and it must survive until its
    // last owner is gone.
    //
    // Capture the entry type of a LOCAL function BEFORE deleting it (a `TypeId`
    // is `Copy`, so this immutable borrow of `funcs` ends before the delete).
    let entry_ty: Option<walrus::TypeId> = match &self.module.inner.funcs.get(func.id).kind {
      walrus::FunctionKind::Local(lf) => match lf.block(lf.entry_block()).ty {
        walrus::ir::InstrSeqType::MultiValue(id) => Some(id),
        walrus::ir::InstrSeqType::Simple(_) => None,
      },
      _ => None,
    };
    self.module.inner.funcs.delete(func.id);
    // Drop the now-orphaned internal entry type, but only if no remaining local
    // function still shares it (entry types are deduped by result signature).
    if let Some(ety) = entry_ty {
      let still_used = self.module.inner.funcs.iter_local().any(|(_, lf)| {
        matches!(lf.block(lf.entry_block()).ty, walrus::ir::InstrSeqType::MultiValue(id) if id == ety)
      });
      if !still_used {
        self.module.inner.types.delete(ety);
      }
    }
    Ok(())
  }
}

/// A single function in a module, as a live handle: it holds the function's id
/// plus a strong reference to the owning [`WasmModule`], and every accessor
/// reads or writes through to that module.
///
/// Only metadata is exposed here — the function's kind, name, and type. The
/// function body (instructions) and the import handle for imported functions are
/// deferred to later tasks.
#[napi]
pub struct WasmFunction {
  pub(crate) id: FunctionId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmFunction {
  /// Confirm the function still exists before touching the arena.
  ///
  /// walrus' `funcs.get`/`get_mut` panic on a deleted id, which would abort the
  /// process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few functions relative to the
  /// cost of an FFI call); do not prematurely optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.funcs.iter().any(|f| f.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("function"))
    }
  }
}

#[napi]
impl WasmFunction {
  #[napi(getter)]
  /// This function's stable index — its identity for numeric lookup. Readable
  /// even after the function is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This function's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.funcs.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this function's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.funcs.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this function is imported, locally defined, or an uninitialized
  /// placeholder (read only).
  pub fn kind(&self) -> Result<FunctionKindTag> {
    self.ensure_exists()?;
    // `walrus::FunctionKind` is NOT `#[non_exhaustive]`, so this match is total
    // and needs no catch-all — a future variant would fail to compile.
    Ok(match self.module.inner.funcs.get(self.id).kind {
      walrus::FunctionKind::Import(_) => FunctionKindTag::Import,
      walrus::FunctionKind::Local(_) => FunctionKindTag::Local,
      walrus::FunctionKind::Uninitialized(_) => FunctionKindTag::Uninitialized,
    })
  }

  #[napi]
  /// This function's type, as a live [`WasmType`] handle into the module's type
  /// arena.
  ///
  /// A method (not a getter) because it materializes a fresh `WasmType` wrapper
  /// on each call. walrus' `Function::ty()` is a unified, panic-free accessor
  /// across all three kinds (import/local/uninitialized), so the only failure
  /// mode is the delete guard.
  pub fn ty(&self, env: Env) -> Result<WasmType> {
    self.ensure_exists()?;
    let id = self.module.inner.funcs.get(self.id).ty();
    Ok(WasmType {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi]
  /// This function's instruction body, as an array of [`InstrDesc`] descriptors.
  ///
  /// Only a LOCAL function has a body: this throws a catchable error for an
  /// imported function (whose body lives in the host) or an uninitialized
  /// placeholder. The descriptors are the exact inverse of
  /// [`crate::WasmModule::build_function`] — reading a body and building it back
  /// round-trips.
  ///
  /// Branch targets are read back as RELATIVE label depths (`0` = the innermost
  /// enclosing `block`/`loop`/`if`), matching wasm and `buildFunction`; walrus'
  /// absolute sequence ids are inverted via a label stack.
  ///
  /// This is the C1a subset (leaf ops, `Const`, local/global get/set/tee,
  /// `Call`, `Select`, `Block`/`Loop`/`IfElse`, `Br`/`BrIf`/`BrTable`). An
  /// instruction outside that subset throws a catchable error naming it, rather
  /// than aborting the process (later tasks add the remaining families).
  pub fn instructions(&self) -> Result<Vec<InstrDesc>> {
    self.ensure_exists()?;
    let lf = match &self.module.inner.funcs.get(self.id).kind {
      walrus::FunctionKind::Local(lf) => lf,
      _ => {
        return Err(Error::from_reason(
          "cannot read instructions of a non-local function (it is imported or uninitialized)",
        ))
      }
    };
    let mut label_stack = Vec::new();
    read_instr_seq(lf, lf.entry_block(), &mut label_stack)
  }

  #[napi]
  /// The import that brings this function into the module, as a live
  /// [`WasmImport`] handle, or `null` if this function is locally defined (or an
  /// uninitialized placeholder).
  ///
  /// A method (not a getter) because it materializes a fresh wrapper on each
  /// call. Wrapping the id is a pure cross-link (the reverse of
  /// `WasmImport.func()`); a later access on the returned handle self-guards
  /// against the import having been deleted.
  pub fn import(&self, env: Env) -> Result<Option<WasmImport>> {
    self.ensure_exists()?;
    match &self.module.inner.funcs.get(self.id).kind {
      walrus::FunctionKind::Import(f) => Ok(Some(WasmImport {
        id: f.import,
        module: self.module.clone(env)?,
      })),
      _ => Ok(None),
    }
  }
}
