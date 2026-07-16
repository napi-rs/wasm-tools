use napi::bindgen_prelude::{Reference, Result, Uint8Array};
use napi::{Env, Error};
use napi_derive::napi;
use walrus::{Module, RawCustomSection};

use crate::{
  ModuleConfig, WasmCustomSections, WasmDataSegments, WasmElements, WasmFunctions, WasmGlobals,
  WasmLocals, WasmMemories, WasmProducers, WasmTables, WasmTags, WasmTypes,
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

  #[napi(factory)]
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

  #[napi(factory)]
  /// Construct a new module from the in-memory wasm buffer and configuration.
  pub fn from_buffer_with_config(bytes: Uint8Array, config: &ModuleConfig) -> Result<Self> {
    Ok(Self {
      inner: Module::from_buffer_with_config(&bytes, &config.inner)?,
    })
  }

  #[napi]
  /// Emit this module into an in-memory wasm buffer.
  pub fn emit_wasm(&mut self, demangle: bool) -> Result<Uint8Array> {
    Ok(self.emit_bytes(demangle)?.into())
  }

  #[napi]
  /// Emit this module into a `.wasm` file at the given path.
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
  pub fn gc(&mut self) {
    walrus::passes::gc::run(&mut self.inner);
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
  /// net: the module can hold references that only fail at encode time and are
  /// undetectable from the binding — most notably walrus' internal
  /// "function-entry" types, which emit skips when assigning type indices, so
  /// referencing one (e.g. via `tags.add`) makes `get_type_index` panic. The
  /// crate builds with the default `panic = unwind` (only the `wasm-fixture`
  /// profile sets `panic = 'abort'`), so we can catch that panic and surface it
  /// as a catchable `napi::Error` instead of letting it cross the FFI boundary
  /// and abort the whole Node process. The saved custom sections are restored on
  /// BOTH the ok and panic paths, so a caught emit leaves the module consistent
  /// (emit's only `&mut` effect is draining `customs`, which the restore loop
  /// puts back).
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
