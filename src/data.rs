use napi::bindgen_prelude::{Reference, Result, Uint8Array};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::{DataId, DataKind};

use crate::constexpr::ConstExpr;
use crate::memories::WasmMemory;
use crate::WasmModule;

/// Whether a data segment is active (auto-initialized into a memory at
/// instantiation) or passive (copied on demand via `memory.init`).
///
/// Mirrors the discriminant of `walrus::DataKind` (`Active { memory, offset }` /
/// `Passive`). The active variant's memory and offset are read through the
/// [`WasmData::memory`] / [`WasmData::offset`] accessors, which return `null`
/// for a passive segment.
#[napi(string_enum)]
pub enum DataKindTag {
  /// An active data segment (initialized into a memory at a fixed offset).
  Active,
  /// A passive data segment (copied on demand via `memory.init`).
  Passive,
}

/// The data segments of a module. Each accessor materializes a fresh
/// [`WasmData`] handle that reads and writes straight through to the owning
/// [`WasmModule`]; the collection itself caches nothing.
#[napi]
pub struct WasmDataSegments {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmDataSegments {
  #[napi(getter)]
  /// The number of data segments in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.data.iter().count() as u32
  }

  #[napi]
  /// Every data segment in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmData>> {
    let ids: Vec<DataId> = self.module.inner.data.iter().map(|d| d.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmData {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The data segment whose stable `.index` equals `index`, or `null` if none
  /// exists.
  pub fn get_by_index(&self, env: Env, index: u32) -> Result<Option<WasmData>> {
    let id = self
      .module
      .inner
      .data
      .iter()
      .find(|d| d.id().index() as u32 == index)
      .map(|d| d.id());
    match id {
      Some(id) => Ok(Some(WasmData {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete a data segment from the module. Takes the handle itself: a JS number
  /// can never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted segment (walrus does not check; a dangling `memory.init` /
  /// `data.drop` would abort emit).
  ///
  /// Same no-panic invariant as the item accessors: walrus' `ModuleData::delete`
  /// asserts the id is live, and a panic across FFI aborts the process. Id
  /// equality includes the arena_id, so this liveness scan rejects BOTH
  /// already-deleted ids (`iter()` skips tombstoned entries) AND handles that
  /// belong to a different module (arena_id mismatch), surfacing a catchable JS
  /// error instead of aborting.
  pub fn delete(&mut self, data: &WasmData) -> Result<()> {
    if self.module.inner.data.iter().any(|d| d.id() == data.id) {
      // walrus's parser records every ACTIVE segment's id in its owning
      // memory's `data_segments` back-link set, and `ModuleData::delete` does
      // NOT cascade that removal. Left stale, a later `gc()` on a rooted memory
      // iterates the set and calls `data.get(id)` on this now-tombstoned id — a
      // panic across FFI that ABORTS the whole Node process. Restore walrus'
      // invariant: drop the back-link before tombstoning the segment.
      //
      // Extract the `MemoryId` (Copy) out of the kind FIRST so the shared
      // `data.get` borrow ends before the mutable `memories.get_mut` borrow
      // (distinct `Module` fields → split borrow is fine). Passive segments have
      // no back-link; if the owning memory was already deleted, its set is gone
      // too — the liveness scan skips it.
      let active_memory = match self.module.inner.data.get(data.id).kind {
        DataKind::Active { memory, .. } => Some(memory),
        DataKind::Passive => None,
      };
      if let Some(mem) = active_memory {
        if self.module.inner.memories.iter().any(|m| m.id() == mem) {
          self
            .module
            .inner
            .memories
            .get_mut(mem)
            .data_segments
            .remove(&data.id);
        }
      }
      self.module.inner.data.delete(data.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("data segment"))
    }
  }

  #[napi]
  /// Add a new passive data segment, returning a live handle to it.
  ///
  /// A passive segment carries only its payload bytes; it is copied into a
  /// memory on demand via `memory.init`. `value` is freely chosen (bytes are not
  /// validated — mirror-walrus policy).
  ///
  /// The returned handle holds its own strong reference to the module (same as
  /// the accessor handles), so it stays valid as long as it is held.
  pub fn add_passive(&mut self, env: Env, value: Uint8Array) -> Result<WasmData> {
    let id = self
      .module
      .inner
      .data
      .add(DataKind::Passive, value.to_vec());
    Ok(WasmData {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi]
  /// Add a new active data segment, returning a live handle to it.
  ///
  /// An active segment is copied into `memory` at `offset` (a constant
  /// expression) at instantiation time. `value` is freely chosen (bytes are not
  /// validated — mirror-walrus policy).
  ///
  /// `add_active` is a CONSUME site for arena ids: both the `memory` handle and
  /// any ids embedded in `offset` are resolved by walrus at emit via panicking
  /// index lookups, and a panic across FFI ABORTS the whole Node process. So we
  /// validate both BEFORE touching the arena:
  ///   1. the `memory` handle must be live in THIS module (a foreign/deleted
  ///      MemoryId would abort `get_memory_index`), and
  ///   2. every id in `offset` must be live in THIS module
  ///      (`validate_const_expr` — a foreign/deleted Global/RefFunc/typed-RefNull
  ///      id would abort `get_global_index`/`get_func_index`/`get_type_index`).
  /// Either check failing yields a catchable JS error and leaves the module
  /// untouched.
  ///
  /// Per mirror-walrus policy this adds NO wasm semantic-validity checks: the
  /// offset is not range-checked against the memory's size, nor is its value
  /// type matched to the memory's index type. A semantically-invalid-but-stored
  /// module is the caller's responsibility, catchable via `WebAssembly.validate`
  /// / re-parse.
  pub fn add_active(
    &mut self,
    env: Env,
    memory: &WasmMemory,
    offset: &ConstExpr,
    value: Uint8Array,
  ) -> Result<WasmData> {
    // 1) Memory provenance: reject a handle that is not live in THIS module (a
    //    foreign-module handle or an already-deleted id) before it can reach
    //    emit, where walrus would abort the whole Node process.
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
    // 2) Offset provenance: reject an offset that references a global/function/
    //    type id not live in THIS module (same abort guard as globals.addLocal).
    crate::handle::validate_const_expr(&self.module.inner, &offset.inner)?;
    let id = self.module.inner.data.add(
      DataKind::Active {
        memory: memory.id,
        offset: offset.inner.clone(),
      },
      value.to_vec(),
    );
    // Maintain walrus' ACTIVE-segment back-link invariant (which its parser
    // upholds at data.rs:218): record the new segment's id in its owning
    // memory's `data_segments` set, symmetric with the removal in `delete`.
    // (In walrus 0.26.4 gc unconditionally roots every active segment
    // — used.rs:178-184 — so this insert is not what keeps the segment alive
    // across gc; it keeps the memory<->data invariant consistent and defensive
    // against any future consumer of `data_segments`.) `memory.id` was
    // validated live above, so `get_mut` cannot panic.
    self
      .module
      .inner
      .memories
      .get_mut(memory.id)
      .data_segments
      .insert(id);
    Ok(WasmData {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single data segment in a module, as a live handle: it holds the segment's
/// id plus a strong reference to the owning [`WasmModule`], and every accessor
/// reads or writes through to that module.
#[napi]
pub struct WasmData {
  pub(crate) id: DataId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmData {
  /// Confirm the data segment still exists before touching the arena.
  ///
  /// walrus' `data.get`/`get_mut` panic on a deleted id, which would abort the
  /// process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few data segments); do not
  /// prematurely optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.data.iter().any(|d| d.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("data segment"))
    }
  }
}

#[napi]
impl WasmData {
  #[napi(getter)]
  /// This segment's stable index — its identity for numeric lookup. Readable
  /// even after the segment is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This segment's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.data.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this segment's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.data.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this segment is active or passive (read only).
  pub fn kind(&self) -> Result<DataKindTag> {
    self.ensure_exists()?;
    Ok(match self.module.inner.data.get(self.id).kind {
      DataKind::Active { .. } => DataKindTag::Active,
      DataKind::Passive => DataKindTag::Passive,
    })
  }

  #[napi(getter)]
  /// This segment's payload bytes.
  pub fn value(&self) -> Result<Uint8Array> {
    self.ensure_exists()?;
    Ok(self.module.inner.data.get(self.id).value.clone().into())
  }

  #[napi(setter)]
  /// Set this segment's payload bytes. Bytes are freely mutable (no validation —
  /// mirror-walrus policy).
  pub fn set_value(&mut self, bytes: Uint8Array) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.data.get_mut(self.id).value = bytes.to_vec();
    Ok(())
  }

  #[napi]
  /// The memory this active segment initializes, or `null` if it is passive.
  ///
  /// A method (not a getter) because it materializes a fresh [`WasmMemory`]
  /// wrapper on each call.
  pub fn memory(&self, env: Env) -> Result<Option<WasmMemory>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.data.get(self.id).kind {
      DataKind::Active { memory, .. } => Some(WasmMemory {
        id: memory,
        module: self.module.clone(env)?,
      }),
      DataKind::Passive => None,
    })
  }

  #[napi]
  /// This active segment's initialization offset as a [`ConstExpr`], or `null`
  /// if it is passive.
  ///
  /// A method (not a getter) because it materializes a fresh `ConstExpr` wrapper
  /// on each call.
  pub fn offset(&self) -> Result<Option<ConstExpr>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.data.get(self.id).kind {
      DataKind::Active { offset, .. } => Some(ConstExpr {
        inner: offset.clone(),
      }),
      DataKind::Passive => None,
    })
  }
}
