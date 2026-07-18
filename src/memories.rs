use napi::bindgen_prelude::{BigInt, Reference, Result};
use napi::Env;
use napi_derive::napi;
use walrus::MemoryId;

use crate::imports::WasmImport;
use crate::WasmModule;

/// The memories of a module. Each accessor materializes a fresh [`WasmMemory`]
/// handle that reads and writes straight through to the owning [`WasmModule`];
/// the collection itself caches nothing.
#[napi]
pub struct WasmMemories {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmMemories {
  #[napi(getter)]
  /// The number of memories in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.memories.iter().count() as u32
  }

  #[napi]
  /// Every memory in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmMemory>> {
    let ids: Vec<MemoryId> = self.module.inner.memories.iter().map(|m| m.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmMemory {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The memory whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: f64) -> Result<Option<WasmMemory>> {
    let index = crate::convert::checked_index(index, "index")?;
    let id = self
      .module
      .inner
      .memories
      .iter()
      .find(|m| m.id().index() as u32 == index)
      .map(|m| m.id());
    match id {
      Some(id) => Ok(Some(WasmMemory {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi(strict)]
  /// Delete a memory from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted memory (walrus does not check).
  ///
  /// Same no-panic invariant as the item accessors: walrus'
  /// `ModuleMemories::delete` asserts the id is live, and a panic across FFI
  /// aborts the process. Id equality includes the arena_id, so this liveness
  /// scan rejects BOTH already-deleted ids (`iter()` skips tombstoned entries)
  /// AND handles that belong to a different module (arena_id mismatch),
  /// surfacing a catchable JS error instead of aborting.
  pub fn delete(&mut self, memory: &WasmMemory) -> Result<()> {
    if self
      .module
      .inner
      .memories
      .iter()
      .any(|m| m.id() == memory.id)
    {
      self.module.inner.memories.delete(memory.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("memory"))
    }
  }

  #[napi]
  /// Add a new locally defined memory, returning a live handle to it.
  ///
  /// `initial`/`maximum` are page counts (`bigint`, so 64-bit `memory64`
  /// memories are representable losslessly); `maximum` is `null` for an
  /// unbounded memory. `pageSizeLog2` is the custom-page-sizes proposal's log2
  /// page size, or `null` for the default 64 KiB pages.
  ///
  /// The returned handle holds its own strong reference to the module (same as
  /// the accessor handles), so it stays valid as long as it is held.
  pub fn add_local(
    &mut self,
    env: Env,
    shared: bool,
    memory64: bool,
    initial: BigInt,
    maximum: Option<BigInt>,
    page_size_log2: Option<u32>,
  ) -> Result<WasmMemory> {
    let initial = crate::handle::bigint_to_u64(initial, "initial")?;
    let maximum = maximum
      .map(|m| crate::handle::bigint_to_u64(m, "maximum"))
      .transpose()?;
    let id =
      self
        .module
        .inner
        .memories
        .add_local(shared, memory64, initial, maximum, page_size_log2);
    Ok(WasmMemory {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single memory in a module, as a live handle: it holds the memory's id plus
/// a strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmMemory {
  pub(crate) id: MemoryId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmMemory {
  /// Confirm the memory still exists before touching the arena.
  ///
  /// walrus' `memories.get`/`get_mut` panic on a deleted id, which would abort
  /// the process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few memories); do not
  /// prematurely optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.memories.iter().any(|m| m.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("memory"))
    }
  }
}

#[napi]
impl WasmMemory {
  #[napi(getter)]
  /// This memory's stable index — its identity for numeric lookup. Readable
  /// even after the memory is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This memory's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this memory's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.memories.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this memory is shared (a creation-time property, read only).
  pub fn shared(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).shared)
  }

  #[napi(getter)]
  /// Whether this is a 64-bit memory (a creation-time property, read only).
  pub fn memory64(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).memory64)
  }

  #[napi(getter)]
  /// This memory's initial size, in wasm pages (`bigint`).
  pub fn initial(&self) -> Result<u64> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).initial)
  }

  #[napi(setter)]
  /// Set this memory's initial size, in wasm pages.
  pub fn set_initial(&mut self, value: BigInt) -> Result<()> {
    // Convert (and reject a bad size) BEFORE the liveness check and the arena
    // write, so a bad input never mutates the module.
    let value = crate::handle::bigint_to_u64(value, "initial")?;
    self.ensure_exists()?;
    self.module.inner.memories.get_mut(self.id).initial = value;
    Ok(())
  }

  #[napi(getter)]
  /// This memory's optional maximum size, in wasm pages (`bigint`), or `null`
  /// if unbounded.
  pub fn maximum(&self) -> Result<Option<u64>> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).maximum)
  }

  #[napi(setter)]
  /// Set this memory's optional maximum size, in wasm pages. `null` clears it.
  pub fn set_maximum(&mut self, value: Option<BigInt>) -> Result<()> {
    // Convert (and reject a bad size) BEFORE the liveness check and the arena
    // write, so a bad input never mutates the module.
    let value = value
      .map(|m| crate::handle::bigint_to_u64(m, "maximum"))
      .transpose()?;
    self.ensure_exists()?;
    self.module.inner.memories.get_mut(self.id).maximum = value;
    Ok(())
  }

  #[napi(getter)]
  /// The log2 of this memory's custom page size (custom-page-sizes proposal),
  /// or `null` for the default 64 KiB pages. Read only (creation-time).
  pub fn page_size_log2(&self) -> Result<Option<u32>> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).page_size_log2)
  }

  #[napi(getter)]
  /// Whether this memory is imported. The import handle itself is exposed by a
  /// later imports task; only the boolean is available now.
  pub fn is_imported(&self) -> Result<bool> {
    self.ensure_exists()?;
    Ok(self.module.inner.memories.get(self.id).import.is_some())
  }

  #[napi]
  /// The import that brings this memory into the module, as a live
  /// [`WasmImport`] handle, or `null` if this memory is locally defined.
  ///
  /// A method (not a getter) because it materializes a fresh wrapper on each
  /// call. Wrapping the id is a pure cross-link (the reverse of
  /// `WasmImport.memory()`); a later access on the returned handle self-guards
  /// against the import having been deleted.
  pub fn import(&self, env: Env) -> Result<Option<WasmImport>> {
    self.ensure_exists()?;
    match self.module.inner.memories.get(self.id).import {
      Some(id) => Ok(Some(WasmImport {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }
}
