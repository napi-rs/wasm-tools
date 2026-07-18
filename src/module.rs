use napi::bindgen_prelude::{Reference, Result, Uint8Array};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::{ExportItem, FunctionBuilder, FunctionKind, Module, RawCustomSection};

use crate::convert::{checked_index, val_type_to_walrus_in};
use crate::ir::{emit_desc, function_id_at, local_id_at, validate_body};
use crate::ir_marshal::InstrBody;
use crate::safevec::SafeVec;
use crate::valtype::ValType;
use crate::{
  ModuleConfig, WasmCustomSections, WasmDataSegments, WasmElements, WasmExports, WasmFunction,
  WasmFunctions, WasmGlobals, WasmImports, WasmLocals, WasmMemories, WasmMemory, WasmProducers,
  WasmTables, WasmTags, WasmTypes,
};

#[napi]
pub struct WasmModule {
  pub(crate) inner: Module,
}

#[napi]
impl WasmModule {
  #[napi(factory)]
  /// Construct a new module from the given path with the default
  /// configuration.
  pub fn from_path(path: String) -> Result<Self> {
    Ok(Self {
      inner: Module::from_file(path)?,
    })
  }

  #[napi(factory, strict)]
  /// Construct a new module from the given path and configuration.
  pub fn from_file_with_config(path: String, config: &ModuleConfig) -> Result<Self> {
    Ok(Self {
      inner: Module::from_file_with_config(path, &config.inner)?,
    })
  }

  #[napi(factory)]
  /// Construct a new module from the in-memory wasm buffer with the default
  /// configuration.
  pub fn from_buffer(bytes: Uint8Array) -> Result<Self> {
    Ok(Self {
      inner: Module::from_buffer(&bytes)?,
    })
  }

  #[napi(factory, strict)]
  /// Construct a new module from the in-memory wasm buffer and configuration.
  pub fn from_buffer_with_config(bytes: Uint8Array, config: &ModuleConfig) -> Result<Self> {
    Ok(Self {
      inner: Module::from_buffer_with_config(&bytes, &config.inner)?,
    })
  }

  #[napi]
  /// Emit this module into an in-memory wasm buffer.
  ///
  /// MIRROR-WALRUS: a surviving reference (start/export/element/`Call`) to a
  /// deleted item aborts at gc/emit — caller's responsibility.
  pub fn emit_wasm(&mut self, demangle: bool) -> Result<Uint8Array> {
    Ok(self.emit_bytes(demangle)?.into())
  }

  #[napi]
  /// Emit this module into a `.wasm` file at the given path.
  ///
  /// MIRROR-WALRUS: a surviving reference (start/export/element/`Call`) to a
  /// deleted item aborts at gc/emit — caller's responsibility.
  pub fn emit_wasm_file(&mut self, path: String, demangle: bool) -> Result<()> {
    let bytes = self.emit_bytes(demangle)?;
    std::fs::write(&path, bytes)
      .map_err(|e| Error::from_reason(format!("failed to write wasm to '{path}': {e}")))
  }

  #[napi]
  /// Write the GraphViz `.dot` representation of this module to the given path.
  pub fn write_graphviz_dot(&self, path: String) -> Result<()> {
    self.inner.write_graphviz_dot(path)?;
    Ok(())
  }

  #[napi]
  /// Run garbage collection passes over this module, removing items that are
  /// not transitively referenced from any root (exports, the start function,
  /// etc.).
  ///
  /// MIRROR-WALRUS: a surviving reference (start/export/element/`Call`) to a
  /// deleted item aborts at gc/emit — caller's responsibility.
  pub fn gc(&mut self) {
    walrus::passes::gc::run(&mut self.inner);
  }

  #[napi]
  /// Build a new locally-defined function from an instruction-descriptor array
  /// and append it to the module, returning its stable index.
  ///
  /// `params`/`results` are the function signature; `argLocalIndices` are the
  /// stable indices of the locals (pre-created via `module.locals.add`) bound
  /// to the parameters, in order; `body` is the instruction body (see
  /// [`InstrDesc`]). The body is the inverse of
  /// [`crate::WasmFunction::instructions`]: the two round-trip.
  ///
  /// Branch targets in `body` are RELATIVE label depths (`0` = the innermost
  /// enclosing `block`/`loop`/`if`), matching wasm; they are resolved to
  /// walrus' absolute sequence ids internally via a label stack.
  ///
  /// MIRROR-WALRUS: only process-aborting hazards are guarded (an out-of-range
  /// local/global/func index, an out-of-range branch label, or a bad
  /// multi-value block-type index are rejected catchably BEFORE a panicking
  /// walrus lookup). The body is NOT validated for wasm well-formedness — an
  /// ill-typed body is built and emitted as-is, and `WebAssembly.validate` (or
  /// a re-parse) is the place to catch it.
  ///
  /// All-or-nothing: the entire body is validated by a read-only preflight
  /// ([`crate::ir::validate_body`]) BEFORE any arena mutation, so a rejected
  /// body leaves the module completely unchanged — no orphaned signature/entry
  /// type is ever left behind. Because that preflight runs against the pre-call
  /// arena, a body can never name the function's own in-flight signature/entry
  /// type (those indices do not exist yet, so they are out of range and
  /// rejected catchably, rather than aborting the process at emit under
  /// `panic = abort`).
  ///
  /// Self-reference limitation: a walrus `FunctionId` is only minted when the
  /// builder finishes, so a body cannot `Call` the function it is defining (the
  /// index names no live function yet, and that errs).
  pub fn build_function(
    &mut self,
    // `SafeVec` decodes NON-preallocating (see `src/safevec.rs`), so a sparse-huge
    // JS `.length` fails catchably instead of aborting on `with_capacity`;
    // `ts_arg_type` keeps the generated `.d.ts` reading `Array<ValType>` as before.
    #[napi(ts_arg_type = "Array<ValType>")] params: SafeVec<ValType>,
    #[napi(ts_arg_type = "Array<ValType>")] results: SafeVec<ValType>,
    #[napi(ts_arg_type = "Array<number>")] arg_local_indices: SafeVec<f64>,
    // `InstrBody` decodes ITERATIVELY with the nesting guard integrated (see
    // `src/ir_marshal.rs`); `ts_arg_type` keeps the generated `.d.ts` reading
    // `Array<InstrDesc>` exactly as before.
    #[napi(ts_arg_type = "Array<InstrDesc>")] body: InstrBody,
  ) -> Result<u32> {
    // Convert the signature and resolve the argument locals BEFORE creating the
    // builder. `val_type_to_walrus_in` resolves concrete refs and rejects
    // unsupported/entry-type indices catchably.
    let params_w = params
      .0
      .into_iter()
      .map(|v| val_type_to_walrus_in(&self.inner, v))
      .collect::<Result<Vec<_>>>()?;
    let results_w = results
      .0
      .into_iter()
      .map(|v| val_type_to_walrus_in(&self.inner, v))
      .collect::<Result<Vec<_>>>()?;
    let arg_ids = arg_local_indices
      .0
      .into_iter()
      .map(|i| local_id_at(&self.inner, checked_index(i, "argLocalIndex")?))
      .collect::<Result<Vec<_>>>()?;

    // Unwrap the iteratively-decoded body (same `Vec<InstrDesc>` as before; the
    // decode already enforced the nesting cap, so the preflight's own depth
    // guard below is defense in depth).
    let body = body.0;

    // Preflight the WHOLE body against the pre-call module BEFORE any mutation.
    // `FunctionBuilder::new` (below) inserts this function's signature and entry
    // types into `self.inner.types`; validating first is what makes the whole
    // call all-or-nothing and closes two aborts: (1) a body can never resolve
    // its own not-yet-created signature/entry type index (out of range against
    // the pre-call arena => caught here, not an uncatchable emit-time trap under
    // `panic = abort`), and (2) any error returns with the module completely
    // unchanged, so no orphan entry/sig type is left in the arena. The label
    // stack starts at length 1 (the entry frame emit pushes first).
    validate_body(&self.inner, &body, 1)?;

    // `FunctionBuilder::new` borrows `&mut types` only for this call (it returns
    // an owned builder holding no borrow of the module), so `emit_desc` may then
    // borrow `&self.inner` to resolve ids/types while pushing into the builder.
    // After a passing preflight its resolvers cannot fail, but it still returns
    // `Result` and propagates (defense in depth against future drift).
    let mut fb = FunctionBuilder::new(&mut self.inner.types, &params_w, &results_w);
    let entry = fb.func_body_id();
    let mut label_stack = Vec::new();
    emit_desc(&mut fb, &self.inner, entry, body, &mut label_stack)?;
    let id = fb.finish(arg_ids, &mut self.inner.funcs);
    Ok(id.index() as u32)
  }

  #[napi]
  /// Replace an EXPORTED, locally-defined function's body with one built from an
  /// instruction-descriptor array, returning the NEW function's stable index.
  ///
  /// This mirrors walrus' [`Module::replace_exported_func`] using its PUBLIC API
  /// (we cannot call walrus' method directly: it fills the body inside a closure
  /// that holds `&mut Module`, but our [`emit_desc`] needs `&Module` to resolve
  /// indices — an unavoidable borrow conflict). The surgery is identical: mint a
  /// new local function with the SAME signature as the target and repoint the
  /// export at it. The old function is left in the arena (walrus does the same);
  /// run [`Self::gc`] to reclaim it if nothing else references it.
  ///
  /// The signature is INHERITED from the target function — the caller supplies
  /// only `funcIndex` (which function), `argLocalIndices` (the stable indices of
  /// the pre-created parameter locals, in order), and `body` (see [`InstrDesc`]),
  /// exactly like [`Self::build_function`].
  ///
  /// Errors (all catchable — nothing aborts): `funcIndex` names no function; the
  /// function is not exported; the function is exported but not locally defined
  /// (an export may point at an imported func); the inherited signature type was
  /// deleted; the inherited signature type is not a function type (a GC
  /// Struct/Array); an `argLocalIndex` names no local; or `body` fails the
  /// [`crate::ir::validate_body`] preflight (an out-of-range index / label /
  /// block-type). MIRROR-WALRUS: no arg-count- or arg-type-vs-signature check is
  /// added — a mismatch builds a semantically invalid (but non-aborting) module
  /// that `WebAssembly.validate` rejects.
  ///
  /// All-or-nothing: every fallible check runs BEFORE any arena mutation, so a
  /// rejected call leaves the module completely unchanged (no repointed export,
  /// no orphaned func/type). Resolving `funcIndex` through
  /// [`crate::ir::function_id_at`] (a live-arena scan) neutralizes walrus'
  /// `funcs.get`-on-a-tombstoned-id panic. The inherited signature is read via a
  /// no-panic liveness scan + `Type::as_function()` (an `Option`), never
  /// `types.get(ty_id)` / `Type::params()`, which panic on a deleted or
  /// non-function type — a func's signature type is user-deletable and can be a
  /// non-function type, so both are caller-reachable and must not abort.
  pub fn replace_exported_func(
    &mut self,
    func_index: f64,
    // Non-preallocating decode (see `src/safevec.rs`); `ts_arg_type` keeps the
    // generated `.d.ts` reading `Array<number>` verbatim.
    #[napi(ts_arg_type = "Array<number>")] arg_local_indices: SafeVec<f64>,
    #[napi(ts_arg_type = "Array<InstrDesc>")] body: InstrBody,
  ) -> Result<u32> {
    let func_index = checked_index(func_index, "funcIndex")?;
    // Resolve the target against the LIVE arena first: a returned id can never be
    // tombstoned/foreign, so the later `funcs.get(fid)` cannot panic.
    let fid = function_id_at(&self.inner, func_index)?;

    // Require an export that points at this function (walrus' first check).
    let export_id = self
      .inner
      .exports
      .get_exported_func(fid)
      .map(|e| e.id())
      .ok_or_else(|| {
        Error::from_reason(format!(
          "function at index {func_index} is not an exported function"
        ))
      })?;

    // Require that function to be LOCAL (an export can point at an imported
    // func). Extract only the signature `TypeId` inside the match — `TypeId` is
    // `Copy`, so the shared `funcs` borrow (via `lf`) ends at the match, and we
    // deliberately do NOT dereference the type here. Dereferencing an inherited
    // type is the abort hole this method must avoid: `types.get(ty_id)` PANICS on
    // a deleted id (a func's signature type is user-deletable) and
    // `Type::params()`/`results()` PANIC on a non-function (Struct/Array) type.
    let ty_id = match &self.inner.funcs.get(fid).kind {
      FunctionKind::Local(lf) => lf.ty(),
      _ => {
        return Err(Error::from_reason(format!(
          "cannot replace function at index {func_index}: it is exported but not a local function"
        )))
      }
    };

    // Resolve the signature type by a NO-PANIC liveness scan: `types.iter()`
    // skips tombstoned entries, so a deleted `ty_id` yields `None` (a catchable
    // error, not `types.get`'s arena-index panic). We must NOT filter entry types
    // here — a function's signature type may itself be an entry type — so this is
    // the direct id scan, not `resolve_type_id`. `as_function()` returns `Option`
    // (walrus' `params()`/`results()` go through a panicking `unwrap_function()`),
    // so a non-function signature type is a catchable error too. `.to_vec()`
    // produces owned `Vec`s, ending the `&Type`/`&types` borrow before
    // `FunctionBuilder::new` takes `&mut self.inner.types` below.
    let sig_ty = self
      .inner
      .types
      .iter()
      .find(|t| t.id() == ty_id)
      .ok_or_else(|| {
        Error::from_reason(format!(
          "cannot replace function at index {func_index}: its signature type was deleted"
        ))
      })?;
    let fty = sig_ty.as_function().ok_or_else(|| {
      Error::from_reason(format!(
        "cannot replace function at index {func_index}: its signature type is not a function type"
      ))
    })?;
    let (params_w, results_w) = (fty.params().to_vec(), fty.results().to_vec());

    // Resolve the argument locals against the live arena.
    let arg_ids = arg_local_indices
      .0
      .into_iter()
      .map(|i| local_id_at(&self.inner, checked_index(i, "argLocalIndex")?))
      .collect::<Result<Vec<_>>>()?;

    let body = body.0;

    // Preflight the WHOLE body before any mutation — the same all-or-nothing
    // guard as `build_function` (label stack starts at length 1).
    validate_body(&self.inner, &body, 1)?;

    // Build the replacement local function, then repoint the export at it.
    let mut fb = FunctionBuilder::new(&mut self.inner.types, &params_w, &results_w);
    let entry = fb.func_body_id();
    let mut label_stack = Vec::new();
    emit_desc(&mut fb, &self.inner, entry, body, &mut label_stack)?;
    let new_fid = fb.finish(arg_ids, &mut self.inner.funcs);
    self.inner.exports.get_mut(export_id).item = ExportItem::Function(new_fid);
    Ok(new_fid.index() as u32)
  }

  #[napi]
  /// Replace an IMPORTED function with a locally-defined body built from an
  /// instruction-descriptor array, returning the SAME function index (the import
  /// becomes a local function in place, so existing `Call` references stay
  /// valid). The import record is removed.
  ///
  /// This mirrors walrus' [`Module::replace_imported_func`] using its PUBLIC API
  /// (same borrow conflict as [`Self::replace_exported_func`]). The surgery is
  /// identical EXCEPT for the argument locals: walrus allocates fresh parameter
  /// locals inside its closure and hands them to it, but our `body` is
  /// materialized BEFORE this call and references locals by index, so the caller
  /// pre-allocates them (via `module.locals.add`) and passes their indices as
  /// `argLocalIndices` — identical ergonomics to [`Self::build_function`].
  ///
  /// The signature is INHERITED from the imported function. Errors (all
  /// catchable — nothing aborts): `funcIndex` names no function; the function is
  /// not imported; the inherited signature type was deleted; the inherited
  /// signature type is not a function type (a GC Struct/Array — `imports.addFunction`
  /// does not check the type's kind); an `argLocalIndex` names no local; or `body`
  /// fails the preflight. MIRROR-WALRUS: no arg-count/type-vs-signature check is
  /// added.
  ///
  /// All-or-nothing / abort-safety: identical to [`Self::replace_exported_func`]
  /// — every fallible check runs before any mutation, `function_id_at`
  /// neutralizes the `funcs.get` panic surface, and the inherited signature is
  /// read via a no-panic liveness scan + `Type::as_function()` (never
  /// `types.get(ty_id)` / `Type::params()`, which panic on a deleted or
  /// non-function type — both caller-reachable here).
  pub fn replace_imported_func(
    &mut self,
    func_index: f64,
    // Non-preallocating decode (see `src/safevec.rs`); `ts_arg_type` keeps the
    // generated `.d.ts` reading `Array<number>` verbatim.
    #[napi(ts_arg_type = "Array<number>")] arg_local_indices: SafeVec<f64>,
    #[napi(ts_arg_type = "Array<InstrDesc>")] body: InstrBody,
  ) -> Result<u32> {
    let func_index = checked_index(func_index, "funcIndex")?;
    // Resolve the target against the LIVE arena first (abort guard, as above).
    let fid = function_id_at(&self.inner, func_index)?;

    // Require an import record for this function (walrus' first check).
    let import_id = self
      .inner
      .imports
      .get_imported_func(fid)
      .map(|i| i.id())
      .ok_or_else(|| {
        Error::from_reason(format!(
          "function at index {func_index} is not an imported function"
        ))
      })?;

    // Confirm the kind is `Import` and extract only the signature `TypeId` (a
    // `Copy`, so the `funcs` borrow ends at the match). We deliberately do NOT
    // dereference the inherited type here — that is the abort hole:
    // `types.get(ty_id)` PANICS on a deleted id (`imports.addFunction` takes a
    // user-deletable func-type handle) and `Type::params()`/`results()` PANIC on
    // a non-function type (`imports.addFunction` does not check the type's KIND,
    // so an imported func can carry a Struct/Array type).
    let ty_id = match &self.inner.funcs.get(fid).kind {
      FunctionKind::Import(imported) => imported.ty,
      _ => {
        return Err(Error::from_reason(format!(
          "cannot replace function at index {func_index}: it is not an imported function"
        )))
      }
    };

    // Same NO-PANIC signature resolution as `replace_exported_func`: a liveness
    // scan (`types.iter()` skips tombstoned entries, so a deleted `ty_id` is a
    // catchable error, not `types.get`'s panic) with no entry-type filter (a
    // signature type may be an entry type), then `as_function()` (walrus'
    // `params()`/`results()` panic via `unwrap_function()` on a non-function
    // type). `.to_vec()` ends the borrow before `FunctionBuilder::new` takes
    // `&mut self.inner.types`.
    let sig_ty = self
      .inner
      .types
      .iter()
      .find(|t| t.id() == ty_id)
      .ok_or_else(|| {
        Error::from_reason(format!(
          "cannot replace function at index {func_index}: its signature type was deleted"
        ))
      })?;
    let fty = sig_ty.as_function().ok_or_else(|| {
      Error::from_reason(format!(
        "cannot replace function at index {func_index}: its signature type is not a function type"
      ))
    })?;
    let (params_w, results_w) = (fty.params().to_vec(), fty.results().to_vec());

    // Resolve the argument locals against the live arena.
    let arg_ids = arg_local_indices
      .0
      .into_iter()
      .map(|i| local_id_at(&self.inner, checked_index(i, "argLocalIndex")?))
      .collect::<Result<Vec<_>>>()?;

    let body = body.0;

    // Preflight the WHOLE body before any mutation (all-or-nothing).
    validate_body(&self.inner, &body, 1)?;

    // Build the replacement, swap the kind IN PLACE (same id), then drop the
    // import record. Order mirrors walrus exactly.
    let mut fb = FunctionBuilder::new(&mut self.inner.types, &params_w, &results_w);
    let entry = fb.func_body_id();
    let mut label_stack = Vec::new();
    emit_desc(&mut fb, &self.inner, entry, body, &mut label_stack)?;
    let new_local = fb.local_func(arg_ids);
    self.inner.funcs.get_mut(fid).kind = FunctionKind::Local(new_local);
    self.inner.imports.delete(import_id);
    Ok(fid.index() as u32)
  }

  #[napi(getter)]
  /// The name of this module, as stored in the wasm "name" custom section.
  pub fn name(&self) -> Option<String> {
    self.inner.name.clone()
  }

  #[napi(setter)]
  /// Set the name of this module, stored in the wasm "name" custom section.
  pub fn set_name(&mut self, name: Option<String>) {
    self.inner.name = name;
  }

  #[napi(getter)]
  /// This module's start function — run automatically at instantiation — as a
  /// live [`WasmFunction`] handle, or `null` if the module has none.
  ///
  /// A pure id-wrap: `null` is returned only when no start is set, never as a
  /// liveness check. If the stored start function was later deleted the returned
  /// handle self-guards on its own accessors (and the module would abort at emit
  /// — see `set_start`).
  pub fn start(&self, this: Reference<WasmModule>, env: Env) -> Result<Option<WasmFunction>> {
    match self.inner.start {
      Some(id) => Ok(Some(WasmFunction {
        id,
        module: this.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi(setter, strict)]
  /// Set (or clear, with `null`) this module's start function.
  ///
  /// Id-ref guarded: walrus stores the raw `FunctionId` and resolves it to an
  /// index at emit via a panicking `get_function_index`, so a function handle
  /// from a different module (or an already-deleted one) would abort the whole
  /// Node process there. We reject such a handle with a catchable error BEFORE
  /// storing it, leaving the current start unchanged. Passing `null` clears the
  /// start unconditionally (always safe).
  pub fn set_start(&mut self, start: Option<&WasmFunction>) -> Result<()> {
    match start {
      Some(f) => {
        if !self.inner.funcs.iter().any(|x| x.id() == f.id) {
          return Err(Error::from_reason(
            "function is not in this module (or was deleted)",
          ));
        }
        self.inner.start = Some(f.id);
        Ok(())
      }
      None => {
        self.inner.start = None;
        Ok(())
      }
    }
  }

  #[napi(getter)]
  /// This module's main memory — the first memory (wasm memory index 0) — as a
  /// live [`WasmMemory`] handle, or `null` if the module has no memory.
  ///
  /// A documented convenience, NOT a walrus passthrough: walrus has no
  /// `main_memory()` accessor, so "main memory" here means the conventional
  /// first memory that single-memory modules use for all loads/stores.
  pub fn main_memory(&self, this: Reference<WasmModule>, env: Env) -> Result<Option<WasmMemory>> {
    match self.inner.memories.iter().next().map(|m| m.id()) {
      Some(id) => Ok(Some(WasmMemory {
        id,
        module: this.clone(env)?,
      })),
      None => Ok(None),
    }
  }

  #[napi(getter)]
  /// The `producers` custom section of this module, describing the tools that
  /// produced it. Mutations through the returned object write back to this
  /// module.
  pub fn producers(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmProducers> {
    Ok(WasmProducers {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The custom sections of this module. Mutations through the returned object
  /// write back to this module.
  pub fn customs(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmCustomSections> {
    Ok(WasmCustomSections {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The globals of this module. Each handle materialized through the returned
  /// object reads and writes back to this module.
  pub fn globals(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmGlobals> {
    Ok(WasmGlobals {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The memories of this module. Each handle materialized through the returned
  /// object reads and writes back to this module.
  pub fn memories(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmMemories> {
    Ok(WasmMemories {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The tables of this module. Each handle materialized through the returned
  /// object reads and writes back to this module.
  pub fn tables(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmTables> {
    Ok(WasmTables {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The types of this module. Each handle materialized through the returned
  /// object reads and writes back to this module.
  pub fn types(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmTypes> {
    Ok(WasmTypes {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The functions of this module (imported and locally defined). Each handle
  /// materialized through the returned object reads and writes back to this
  /// module.
  pub fn functions(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmFunctions> {
    Ok(WasmFunctions {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The locals of this module (across all function bodies). Each handle
  /// materialized through the returned object reads and writes back to this
  /// module.
  pub fn locals(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmLocals> {
    Ok(WasmLocals {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The data segments of this module. Each handle materialized through the
  /// returned object reads and writes back to this module.
  pub fn data(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmDataSegments> {
    Ok(WasmDataSegments {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The element segments of this module. Each handle materialized through the
  /// returned object reads and writes back to this module.
  pub fn elements(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmElements> {
    Ok(WasmElements {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The tags (exception-handling tags) of this module. Each handle
  /// materialized through the returned object reads and writes back to this
  /// module.
  pub fn tags(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmTags> {
    Ok(WasmTags {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The imports of this module. Each handle materialized through the returned
  /// object reads and writes back to this module.
  pub fn imports(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmImports> {
    Ok(WasmImports {
      module: this.clone(env)?,
    })
  }

  #[napi(getter)]
  /// The exports of this module. Each handle materialized through the returned
  /// object reads and writes back to this module.
  pub fn exports(&self, this: Reference<WasmModule>, env: Env) -> Result<WasmExports> {
    Ok(WasmExports {
      module: this.clone(env)?,
    })
  }
}

impl WasmModule {
  /// Emit this module to wasm bytes without mutating the in-memory module's
  /// custom sections.
  ///
  /// walrus' [`Module::emit_wasm`] does `mem::take(&mut self.customs)` and never
  /// restores it, so a naive emit would leave `self.inner.customs` empty:
  /// `customs.list()` would come back empty after an emit, a second emit would
  /// be missing every raw custom section, and `build_id` would be regenerated on
  /// every emit (since `prepare_for_emit` would no longer see the previous one).
  /// We snapshot the raw custom sections before emitting and add them back after,
  /// so emission is non-destructive: the output bytes are identical (walrus emits
  /// the sections before draining them) while the module keeps its state.
  ///
  /// Every section reachable through our API lives in `customs` as a
  /// [`RawCustomSection`]: `addRaw` only ever adds that type, and parsing stores
  /// unknown sections as `RawCustomSection` too (the `name`/`producers`/`.debug`
  /// sections are parsed into dedicated fields, never `customs`). The downcast
  /// filter therefore captures the full set with nothing dropped.
  ///
  /// Emit itself is wrapped in [`std::panic::catch_unwind`] as a general safety
  /// net: the module can hold references that only fail at encode time. The
  /// prime example — walrus' internal "function-entry" types, which emit skips
  /// when assigning type indices, so referencing one (e.g. via `tags.add`) makes
  /// `get_type_index` panic — is now removed at the source: [`crate::WasmTypes`]
  /// filters those types out of every accessor, so a user can no longer obtain a
  /// handle to one (this closes it on ALL targets, WASI included). Orphaned entry
  /// types cannot reach here either: [`crate::WasmFunctions::delete`] drops a
  /// local function's entry type when its last owner is deleted, so no orphan
  /// ever persists in the arena to leak back through the filter. The one narrow
  /// residue that still reaches here and relies on this net is emit after
  /// deleting a still-referenced item (walrus' `get_*_index` panics). The crate
  /// builds with the default `panic = unwind` (only the `wasm-fixture` profile
  /// sets `panic = 'abort'`), so we can catch that panic and surface it as a
  /// catchable `napi::Error` instead of letting it cross the FFI boundary and
  /// abort the whole Node process. The saved custom sections are restored on
  /// BOTH the ok and panic paths, so a caught emit leaves the module consistent
  /// (emit's only `&mut` effect is draining `customs`, which the restore loop
  /// puts back).
  ///
  /// PLATFORM LIMITATION: `catch_unwind` only unwinds under `panic = unwind`.
  /// The published `wasm32-wasip1-threads` target builds with `panic = abort`
  /// (the target default; `-C panic=unwind` needs nightly + build-std, not
  /// viable), so on WASI this catch is a no-op and that residual panic TRAPS the
  /// process instead of becoming a catchable error. The entry-type filter in
  /// [`crate::WasmTypes`] plus the orphan cleanup in
  /// [`crate::WasmFunctions::delete`] are the target-independent defenses; this
  /// `catch_unwind` is the native-only backstop for the rest.
  fn emit_bytes(&mut self, demangle: bool) -> Result<Vec<u8>> {
    self.prepare_for_emit(demangle);
    let saved: Vec<RawCustomSection> = self
      .inner
      .customs
      .iter()
      .filter_map(|(_, section)| section.as_any().downcast_ref::<RawCustomSection>().cloned())
      .collect();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.inner.emit_wasm()));
    for section in saved {
      self.inner.customs.add(section);
    }
    match result {
      Ok(out) => Ok(out),
      Err(payload) => {
        let msg = payload
          .downcast_ref::<&str>()
          .map(|s| (*s).to_string())
          .or_else(|| payload.downcast_ref::<String>().cloned())
          .unwrap_or_else(|| "unknown panic".to_string());
        Err(Error::from_reason(format!(
          "failed to emit wasm: the module references an item that cannot be emitted \
           (e.g. an internal function-entry type, or an item deleted while still referenced): {msg}"
        )))
      }
    }
  }

  /// Shared pre-emit preparation used by both `emit_wasm` and `emit_wasm_file`:
  /// optionally demangle Rust symbol names, then add a `build_id` custom
  /// section if one is not already present.
  ///
  /// <https://github.com/WebAssembly/tool-conventions/blob/9b80cd2339c648822bb845a083d9ffa6e20fb1ee/BuildId.md>
  pub(crate) fn prepare_for_emit(&mut self, demangle: bool) {
    if demangle {
      demangle_module(&mut self.inner);
    }
    if self
      .inner
      .customs
      .iter()
      .all(|(_, section)| section.name() != "build_id")
    {
      self.inner.customs.add(RawCustomSection {
        name: "build_id".to_string(),
        data: uuid::Uuid::new_v4().as_bytes().into(),
      });
    }
  }
}

fn demangle_module(module: &mut Module) -> &mut Module {
  for func in module.funcs.iter_mut() {
    let name = match &func.name {
      Some(name) => name,
      None => continue,
    };
    if let Ok(sym) = rustc_demangle::try_demangle(name) {
      func.name = Some(sym.to_string());
    }
  }
  module
}
