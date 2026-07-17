use napi::bindgen_prelude::{Reference, Result};
use napi::Env;
use napi_derive::napi;
use walrus::{ElementId, ElementItems, ElementKind, FunctionId};

use crate::constexpr::ConstExpr;
use crate::functions::WasmFunction;
use crate::tables::WasmTable;
use crate::valtype::ValType;
use crate::WasmModule;

/// Whether an element segment is passive, declared, or active.
///
/// Mirrors the discriminant of `walrus::ElementKind` (`Passive` / `Declared` /
/// `Active { table, offset }`). The active variant's table and offset are read
/// through the [`WasmElement::table`] / [`WasmElement::offset`] accessors, which
/// return `null` for a passive or declared segment.
#[napi(string_enum)]
pub enum ElementKindTag {
  /// A passive element segment (copied on demand via `table.init`).
  Passive,
  /// A declared element segment (referenced only by `ref.func`; not usable to
  /// initialize a table).
  Declared,
  /// An active element segment (copied into a table at a fixed offset at
  /// instantiation).
  Active,
}

/// Whether an element segment's items are a list of function references or a
/// list of constant expressions.
///
/// Mirrors the discriminant of `walrus::ElementItems`
/// (`Functions(Vec<FunctionId>)` / `Expressions(RefType, Vec<ConstExpr>)`). The
/// two variants are read through the accessor methods
/// [`WasmElement::function_items`] (populated for `Functions`) and
/// [`WasmElement::expression_element_type`] / [`WasmElement::expression_items`]
/// (populated for `Expressions`), each of which returns `null` for the
/// non-matching variant. They are separate methods rather than one structured
/// object because the payloads are `#[napi]` class handles (`WasmFunction` /
/// `ConstExpr`), which napi cannot nest inside a `#[napi(object)]`.
#[napi(string_enum)]
pub enum ElementItemsTag {
  /// The segment's items are function references (`Functions`).
  Functions,
  /// The segment's items are constant expressions (`Expressions`).
  Expressions,
}

/// The element segments of a module. Each accessor materializes a fresh
/// [`WasmElement`] handle that reads and writes straight through to the owning
/// [`WasmModule`]; the collection itself caches nothing.
#[napi]
pub struct WasmElements {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmElements {
  #[napi(getter)]
  /// The number of element segments in the module.
  pub fn length(&self) -> u32 {
    self.module.inner.elements.iter().count() as u32
  }

  #[napi]
  /// Every element segment in the module, as live item handles.
  pub fn items(&self, env: Env) -> Result<Vec<WasmElement>> {
    let ids: Vec<ElementId> = self.module.inner.elements.iter().map(|e| e.id()).collect();
    ids
      .into_iter()
      .map(|id| {
        Ok(WasmElement {
          id,
          module: self.module.clone(env)?,
        })
      })
      .collect()
  }

  #[napi]
  /// The element segment whose stable `.index` equals `index`, or `null` if none
  /// exists.
  pub fn get_by_index(&self, env: Env, index: f64) -> Result<Option<WasmElement>> {
    let index = crate::convert::checked_index(index, "index")?;
    let id = self
      .module
      .inner
      .elements
      .iter()
      .find(|e| e.id().index() as u32 == index)
      .map(|e| e.id());
    match id {
      Some(id) => Ok(Some(WasmElement {
        id,
        module: self.module.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi]
  /// Delete an element segment from the module. Takes the handle itself: a JS
  /// number can never be turned back into a walrus id, so the wrapper is the
  /// only way to name an item for removal.
  ///
  /// It is the caller's responsibility to ensure nothing still references the
  /// deleted segment (walrus does not check; a dangling `table.init` /
  /// `elem.drop` would abort emit).
  ///
  /// Same no-panic invariant as the item accessors: walrus'
  /// `ModuleElements::delete` asserts the id is live, and a panic across FFI
  /// aborts the process. Id equality includes the arena_id, so this liveness
  /// scan rejects BOTH already-deleted ids (`iter()` skips tombstoned entries)
  /// AND handles that belong to a different module (arena_id mismatch),
  /// surfacing a catchable JS error instead of aborting.
  pub fn delete(&mut self, element: &WasmElement) -> Result<()> {
    if self
      .module
      .inner
      .elements
      .iter()
      .any(|e| e.id() == element.id)
    {
      // walrus's parser records every ACTIVE segment's id in its owning table's
      // `elem_segments` back-link set, and `ModuleElements::delete` does NOT
      // cascade that removal. Left stale, a later `gc()` on a rooted table
      // iterates the set and calls `elements.get(id)` on this now-tombstoned id
      // — a panic across FFI that ABORTS the whole Node process. Restore walrus'
      // invariant: drop the back-link before tombstoning the segment.
      //
      // Extract the `TableId` (Copy) out of the kind FIRST so the shared
      // `elements.get` borrow ends before the mutable `tables.get_mut` borrow
      // (distinct `Module` fields → split borrow is fine). Passive/declared
      // segments have no back-link; if the owning table was already deleted, its
      // set is gone too — the liveness scan skips it.
      let active_table = match self.module.inner.elements.get(element.id).kind {
        ElementKind::Active { table, .. } => Some(table),
        ElementKind::Passive | ElementKind::Declared => None,
      };
      if let Some(table) = active_table {
        if self.module.inner.tables.iter().any(|t| t.id() == table) {
          self
            .module
            .inner
            .tables
            .get_mut(table)
            .elem_segments
            .remove(&element.id);
        }
      }
      self.module.inner.elements.delete(element.id);
      Ok(())
    } else {
      Err(crate::handle::deleted("element segment"))
    }
  }
}

/// A single element segment in a module, as a live handle: it holds the
/// segment's id plus a strong reference to the owning [`WasmModule`], and every
/// accessor reads or writes through to that module.
///
/// READ + delete only — building a new element segment (`add`) needs
/// function-list / expression-list id validation and is a deliberate later task.
#[napi]
pub struct WasmElement {
  pub(crate) id: ElementId,
  pub(crate) module: Reference<WasmModule>,
}

impl WasmElement {
  /// Confirm the element segment still exists before touching the arena.
  ///
  /// walrus' `elements.get`/`get_mut` panic on a deleted id, which would abort
  /// the process across FFI; this turns that into a catchable JS error.
  ///
  /// O(n) guard — acceptable here (modules have few element segments); do not
  /// prematurely optimize into a cache.
  fn ensure_exists(&self) -> Result<()> {
    if self.module.inner.elements.iter().any(|e| e.id() == self.id) {
      Ok(())
    } else {
      Err(crate::handle::deleted("element segment"))
    }
  }
}

#[napi]
impl WasmElement {
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
    Ok(self.module.inner.elements.get(self.id).name.clone())
  }

  #[napi(setter)]
  /// Set this segment's name, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) -> Result<()> {
    self.ensure_exists()?;
    self.module.inner.elements.get_mut(self.id).name = name;
    Ok(())
  }

  #[napi(getter)]
  /// Whether this segment is passive, declared, or active (read only).
  pub fn kind(&self) -> Result<ElementKindTag> {
    self.ensure_exists()?;
    // `walrus::ElementKind` is NOT `#[non_exhaustive]`, so this match is total
    // and needs no catch-all — a future variant would fail to compile.
    Ok(match self.module.inner.elements.get(self.id).kind {
      ElementKind::Passive => ElementKindTag::Passive,
      ElementKind::Declared => ElementKindTag::Declared,
      ElementKind::Active { .. } => ElementKindTag::Active,
    })
  }

  #[napi]
  /// The table this active segment initializes, or `null` if it is passive or
  /// declared.
  ///
  /// A method (not a getter) because it materializes a fresh [`WasmTable`]
  /// wrapper on each call.
  pub fn table(&self, env: Env) -> Result<Option<WasmTable>> {
    self.ensure_exists()?;
    Ok(match self.module.inner.elements.get(self.id).kind {
      ElementKind::Active { table, .. } => Some(WasmTable {
        id: table,
        module: self.module.clone(env)?,
      }),
      ElementKind::Passive | ElementKind::Declared => None,
    })
  }

  #[napi]
  /// This active segment's initialization offset as a [`ConstExpr`], or `null`
  /// if it is passive or declared.
  ///
  /// A method (not a getter) because it materializes a fresh `ConstExpr` wrapper
  /// on each call.
  pub fn offset(&self) -> Result<Option<ConstExpr>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.elements.get(self.id).kind {
      ElementKind::Active { offset, .. } => Some(ConstExpr {
        inner: offset.clone(),
      }),
      ElementKind::Passive | ElementKind::Declared => None,
    })
  }

  #[napi(getter)]
  /// Whether this segment's items are function references (`Functions`) or
  /// constant expressions (`Expressions`) (read only).
  pub fn items_kind(&self) -> Result<ElementItemsTag> {
    self.ensure_exists()?;
    // `walrus::ElementItems` is NOT `#[non_exhaustive]`, so this match is total.
    Ok(match self.module.inner.elements.get(self.id).items {
      ElementItems::Functions(_) => ElementItemsTag::Functions,
      ElementItems::Expressions(..) => ElementItemsTag::Expressions,
    })
  }

  #[napi]
  /// The functions this segment references, as live [`WasmFunction`] handles, or
  /// `null` if this segment's items are constant expressions (`Expressions`).
  ///
  /// A method (not a getter) because it materializes fresh `WasmFunction`
  /// wrappers on each call. The order matches the segment's item order.
  pub fn function_items(&self, env: Env) -> Result<Option<Vec<WasmFunction>>> {
    self.ensure_exists()?;
    // Snapshot the ids while the shared `elements.get` borrow is held, then
    // release it before cloning module references to build the handles.
    let func_ids: Option<Vec<FunctionId>> = match &self.module.inner.elements.get(self.id).items {
      ElementItems::Functions(ids) => Some(ids.clone()),
      ElementItems::Expressions(..) => None,
    };
    match func_ids {
      Some(ids) => ids
        .into_iter()
        .map(|id| {
          Ok(WasmFunction {
            id,
            module: self.module.clone(env)?,
          })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some),
      None => Ok(None),
    }
  }

  #[napi]
  /// The element type of this segment's expression items as a [`ValType`] (the
  /// `Ref` variant), or `null` if this segment's items are function references
  /// (`Functions`).
  ///
  /// Fallible: the ref's heap type may embed a `#[non_exhaustive]` walrus heap
  /// variant a later 0.26.x adds; that surfaces as a catchable JS error, never a
  /// process-aborting panic.
  pub fn expression_element_type(&self) -> Result<Option<ValType>> {
    self.ensure_exists()?;
    match &self.module.inner.elements.get(self.id).items {
      ElementItems::Expressions(ref_type, _) => Ok(Some(ValType::Ref {
        nullable: ref_type.nullable,
        heap: ref_type.heap_type.try_into()?,
      })),
      ElementItems::Functions(_) => Ok(None),
    }
  }

  #[napi]
  /// This segment's constant-expression items as [`ConstExpr`] wrappers, or
  /// `null` if this segment's items are function references (`Functions`).
  ///
  /// A method (not a getter) because it materializes fresh `ConstExpr` wrappers
  /// on each call. Read only: the items are cloned out, so no id validation is
  /// needed (that would only be required when building a new segment).
  pub fn expression_items(&self) -> Result<Option<Vec<ConstExpr>>> {
    self.ensure_exists()?;
    Ok(match &self.module.inner.elements.get(self.id).items {
      ElementItems::Expressions(_, exprs) => Some(
        exprs
          .iter()
          .map(|expr| ConstExpr {
            inner: expr.clone(),
          })
          .collect(),
      ),
      ElementItems::Functions(_) => None,
    })
  }
}
