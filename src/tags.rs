use napi::bindgen_prelude::{Reference, Result};
use napi::Env;
use napi_derive::napi;
use walrus::TagId;

use crate::imports::WasmImport;
use crate::types::WasmType;
use crate::WasmModule;

/// Whether a tag is imported or locally defined.
///
/// Mirrors the discriminant of `walrus::TagKind` (`Import(ImportId)` /
/// `Local`). The companion accessor that exposes the import handle for an
/// imported tag is deferred to the imports task; only the tag is exposed here.
#[napi(string_enum)]
pub enum TagKindTag {
  /// An imported tag (defined by the host).
  Import,
  /// A locally defined tag.
  Local,
}

/// The tags (exception-handling tags) of a module. Each accessor materializes a
/// fresh [`WasmTag`] handle that reads and writes straight through to the owning
/// [`WasmModule`]; the collection itself caches nothing.
#[napi]
pub struct WasmTags {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmTags {
  #[napi(getter)]
  /// The number of tags in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.tags.iter().count() as u32
  }

  #[napi]
  /// Every tag in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmTag>> {
    let ids: Vec<TagId> = self.module.inner.tags.iter().map(|t| t.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmTag {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The tag whose stable `.index` equals `index`, or `null` if none exists.
  pub fn get_by_index(&self, env: Env, index: f64) -> Result<Option<WasmTag>> {
    let index = crate::convert::checked_index(index, "index")?;
    let id = self
      .module
      .inner
      .tags
      .iter()
      .find(|t| t.id().index() as u32 == index)
      .map(|t| t.id());
    match id {
      Some(id) => Ok(Some(WasmTag {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi(strict)]
  /// Delete a tag from the module. Takes the handle itself: a JS number can
  /// never be turned back into a walrus id, so the wrapper is the only way to
  /// name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted tag (walrus does not check; a dangling export or `throw`
  /// instruction aborts at emit time). Unlike active data/element segments,
  /// tags carry NO parser-maintained back-link set on any owner item (the only
  /// `IdHashSet<Tag>` in walrus is the gc pass's own transient `Used` set, built
  /// fresh from exports and `throw` sites each run), so a plain guarded delete
  /// is sufficient — there is no back-link to clean.
  ///
  /// Same no-panic invariant as the item accessors: walrus' `ModuleTags::delete`
  /// tombstones the arena entry and a later access asserts liveness, and a panic
  /// across FFI aborts the process. Id equality includes the arena_id, so this
  /// liveness scan rejects BOTH already-deleted ids (`iter()` skips tombstoned
  /// entries) AND handles that belong to a different module (arena_id mismatch),
  /// surfacing a catchable JS error instead of aborting.
  pub fn delete(&mut self, tag: &WasmTag) -> Result<()> {
    if self.module.inner.tags.iter().any(|t| t.id() == tag.id) {
      self.module.inner.tags.delete(tag.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("tag"))
    }
  }

  #[napi(strict)]
  /// Add a new locally defined tag with the given type, returning a live handle
  /// to it. `ty` is the tag's (function) type signature — for an exception tag
  /// its params are the exception's payload value types.
  ///
  /// Fallible: `ty` must be a live type in THIS module. walrus stores the raw
  /// `TypeId` and resolves it to an index at emit time via a panicking
  /// `get_type_index`; a foreign-module or already-deleted type handle would
  /// abort the whole Node process there. We reject such an id with a catchable
  /// error BEFORE touching the arena, so a failed add never mutates the module.
  /// Same id-ref rule as `globals.addLocal` / `data.addActive`.
  ///
  /// The returned handle holds its own strong reference to the module (same as
  /// the accessor handles), so it stays valid as long as it is held.
  pub fn add(&mut self, env: Env, ty: &WasmType) -> Result<WasmTag> {
    if !self.module.inner.types.iter().any(|t| t.id() == ty.id) {
      return Err(napi::Error::from_reason(
        "type is not in this module (or was deleted)",
      ));
    }
    let id = self.module.inner.tags.add(ty.id);
    Ok(WasmTag {
      id,
      module: self.module.clone(env)?,
    })
  }
}

/// A single tag in a module, as a live handle: it holds the tag's id plus a
/// strong reference to the owning [`WasmModule`], and every accessor reads or
/// writes through to that module.
#[napi]
pub struct WasmTag {
  pub(crate) id: TagId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmTag {
  /// Confirm the tag still exists before touching the arena.
  ///
  /// walrus' `tags.get`/`get_mut` panic on a deleted id, which would abort the
  /// process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few tags); do not prematurely
  /// optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.tags.iter().any(|t| t.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("tag"))
    }
  }
}

#[napi]
impl WasmTag {
  #[napi(getter)]
  /// This tag's stable index — its identity for numeric lookup. Readable even
  /// after the tag is deleted (it never touches the arena).
  pub fn index(&self) -> u32 {
    self.id.index() as u32
  }

  #[napi(getter)]
  /// This tag's name from the wasm "name" custom section, if any.
  pub fn name(&self) -> Result<Option<String>> {
    self.ensure_exists()?;
    Ok(self.module.inner.tags.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this tag's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.tags.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this tag is imported or locally defined (read only).
  pub fn kind(&self) -> Result<TagKindTag> {
    self.ensure_exists()?;
    // `walrus::TagKind` is NOT `#[non_exhaustive]`, so this match is total and
    // needs no catch-all — a future variant would fail to compile.
    Ok(match self.module.inner.tags.get(self.id).kind {
      walrus::TagKind::Import(_) => TagKindTag::Import,
      walrus::TagKind::Local => TagKindTag::Local,
    })
  }

  #[napi]
  /// This tag's type, as a live [`WasmType`] handle into the module's type
  /// arena.
  ///
  /// A method (not a getter) because it materializes a fresh `WasmType` wrapper
  /// on each call. For an exception tag this type's `params()` are the
  /// exception's payload value types.
  pub fn ty(&self, env: Env) -> Result<WasmType> {
    self.ensure_exists()?;
    let id = self.module.inner.tags.get(self.id).ty();
    Ok(WasmType {
      id,
      module: self.module.clone(env)?,
    })
  }

  #[napi]
  /// The import that brings this tag into the module, as a live [`WasmImport`]
  /// handle, or `null` if this tag is locally defined.
  ///
  /// A method (not a getter) because it materializes a fresh wrapper on each
  /// call. Wrapping the id is a pure cross-link (the reverse of
  /// `WasmImport.tag()`); a later access on the returned handle self-guards
  /// against the import having been deleted.
  pub fn import(&self, env: Env) -> Result<Option<WasmImport>> {
    self.ensure_exists()?;
    match &self.module.inner.tags.get(self.id).kind {
      walrus::TagKind::Import(id) => Ok(Some(WasmImport {
        id: *id,
        module: self.module.clone(env)?,
      })),
      walrus::TagKind::Local => Ok(None),
    }
  }
}
