#![deny(clippy::all)]

use napi::bindgen_prelude::{Result, Uint8Array};
use napi_derive::napi;
use walrus::{Module, RawCustomSection};

#[napi]
pub struct ModuleConfig {
  inner: walrus::ModuleConfig,
}

#[napi]
impl ModuleConfig {
  #[napi(constructor)]
  #[allow(clippy::new_without_default)]
  pub fn new() -> Self {
    Self {
      inner: walrus::ModuleConfig::new(),
    }
  }

  #[napi]
  /// Sets a flag to whether DWARF debug sections are generated for this
  /// module.
  ///
  /// By default this flag is `false`. Note that any emitted DWARF is
  /// currently wildly incorrect and buggy, and is also larger than the wasm
  /// itself!
  pub fn generate_dwarf(&mut self, generate_dwarf: bool) -> &Self {
    self.inner.generate_dwarf(generate_dwarf);
    self
  }

  #[napi]
  /// Sets a flag to whether the custom "name" section is generated for this
  /// module.
  ///
  /// The "name" section contains symbol names for the module, functions, and
  /// locals. When enabled, stack traces will use these names, instead of
  /// `wasm-function[123]`.
  ///
  /// By default this flag is `true`.
  pub fn generate_name_section(&mut self, generate_name_section: bool) -> &Self {
    self.inner.generate_name_section(generate_name_section);
    self
  }

  #[napi]
  /// Sets a flag to whether synthetic debugging names are generated for
  /// anonymous locals/functions/etc when parsing and running passes for this
  /// module.
  ///
  /// By default this flag is `false`, and it will generate quite a few names
  /// if enabled!
  pub fn generate_synthetic_names_for_anonymous_items(
    &mut self,
    generate_synthetic_names_for_anonymous_items: bool,
  ) -> &Self {
    self
      .inner
      .generate_synthetic_names_for_anonymous_items(generate_synthetic_names_for_anonymous_items);
    self
  }

  #[napi]
  /// Indicates whether the module, after parsing, performs strict validation
  /// of the wasm module to adhere with the current version of the wasm
  /// specification.
  ///
  /// This can be expensive for some modules and strictly isn't required to
  /// create a `Module` from a wasm file. This includes checks such as "atomic
  /// instructions require a shared memory".
  ///
  /// By default this flag is `true`
  pub fn strict_validate(&mut self, strict_validate: bool) -> &Self {
    self.inner.strict_validate(strict_validate);
    self
  }

  #[napi]
  /// Indicates whether the module will have the "producers" custom section
  /// which preserves the original producers and also includes `walrus`.
  ///
  /// This is generally used for telemetry in browsers, but for otherwise tiny
  /// wasm binaries can add some size to the binary.
  ///
  /// By default this flag is `true`
  pub fn generate_producers_section(&mut self, generate_producers_section: bool) -> &Self {
    self
      .inner
      .generate_producers_section(generate_producers_section);
    self
  }

  #[napi]
  /// Indicates whether this module is allowed to use only stable WebAssembly
  /// features or not.
  ///
  /// This is currently used to disable some validity checks required by the
  /// WebAssembly specification. It's not religiously adhered to throughout
  /// the codebase, even if set to `true` some unstable features may still be
  /// allowed.
  ///
  /// By default this flag is `false`
  pub fn only_stable_features(&mut self, only_stable_features: bool) -> &Self {
    self.inner.only_stable_features(only_stable_features);
    self
  }

  #[napi]
  /// Sets a flag to whether code transform is preverved during parsing.
  ///
  /// By default this flag is `false`.
  pub fn preserve_code_transform(&mut self, preserve: bool) -> &Self {
    self.inner.preserve_code_transform(preserve);
    self
  }

  #[napi]
  pub fn parse(&self, binary: &[u8]) -> Result<WasmModule> {
    Ok(WasmModule {
      inner: self.inner.parse(binary)?,
    })
  }
}

#[napi]
pub struct WasmModule {
  inner: Module,
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

  #[napi]
  /// Emit this module into an in-memory wasm buffer.
  pub fn emit_wasm(&mut self, demangle: bool) -> Result<Uint8Array> {
    if demangle {
      demangle_module(&mut self.inner);
    }
    // https://github.com/WebAssembly/tool-conventions/blob/9b80cd2339c648822bb845a083d9ffa6e20fb1ee/BuildId.md
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

    Ok(self.inner.emit_wasm().into())
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
