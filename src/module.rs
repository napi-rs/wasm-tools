use napi::bindgen_prelude::{Result, Uint8Array};
use napi_derive::napi;
use walrus::{Module, RawCustomSection};

use crate::ModuleConfig;

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
    self.prepare_for_emit(demangle);
    Ok(self.inner.emit_wasm().into())
  }

  #[napi]
  /// Emit this module into a `.wasm` file at the given path.
  pub fn emit_wasm_file(&mut self, path: String, demangle: bool) -> Result<()> {
    self.prepare_for_emit(demangle);
    self.inner.emit_wasm_file(path)?;
    Ok(())
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
}

impl WasmModule {
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
