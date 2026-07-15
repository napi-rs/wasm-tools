use napi::bindgen_prelude::Reference;
use napi_derive::napi;

use crate::WasmModule;

/// The `producers` custom section of a module, exposing the tools that produced
/// it. Mutations write straight through to the owning [`WasmModule`].
#[napi]
pub struct WasmProducers {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmProducers {
  #[napi]
  /// Add (or update) a `language` entry in the producers section.
  pub fn add_language(&mut self, language: String, version: String) {
    self
      .module
      .inner
      .producers
      .add_language(&language, &version);
  }

  #[napi]
  /// Add (or update) a `processed-by` entry in the producers section.
  pub fn add_processed_by(&mut self, tool: String, version: String) {
    self
      .module
      .inner
      .producers
      .add_processed_by(&tool, &version);
  }

  #[napi]
  /// Add (or update) an `sdk` entry in the producers section.
  pub fn add_sdk(&mut self, sdk: String, version: String) {
    self.module.inner.producers.add_sdk(&sdk, &version);
  }

  #[napi]
  /// Remove every field from the producers section.
  pub fn clear(&mut self) {
    self.module.inner.producers.clear();
  }

  #[napi]
  /// List the fields currently present in the producers section.
  pub fn fields(&self) -> Vec<ProducerFieldInfo> {
    self
      .module
      .inner
      .producers
      .fields()
      .iter()
      .map(|field| ProducerFieldInfo {
        name: field.name().to_string(),
        values: field
          .values()
          .iter()
          .map(|value| ProducerValueInfo {
            name: value.name().to_string(),
            version: value.version().to_string(),
          })
          .collect(),
      })
      .collect()
  }
}

/// A single field (e.g. `language`, `sdk`, `processed-by`) of the producers
/// section, along with its versioned values.
#[napi(object)]
pub struct ProducerFieldInfo {
  pub name: String,
  pub values: Vec<ProducerValueInfo>,
}

/// A single versioned value within a producers field.
#[napi(object)]
pub struct ProducerValueInfo {
  pub name: String,
  pub version: String,
}
