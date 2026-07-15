use napi::bindgen_prelude::{Reference, Uint8Array};
use napi_derive::napi;
use walrus::RawCustomSection;

use crate::WasmModule;

/// The custom sections of a module. Mutations write straight through to the
/// owning [`WasmModule`].
#[napi]
pub struct WasmCustomSections {
  pub(crate) module: Reference<WasmModule>,
}

#[napi]
impl WasmCustomSections {
  #[napi]
  /// Add a raw, unparsed custom section with the given name and data.
  pub fn add_raw(&mut self, name: String, data: Uint8Array) {
    self.module.inner.customs.add(RawCustomSection {
      name,
      data: data.to_vec(),
    });
  }

  #[napi]
  /// Remove the first raw custom section with the given name, returning its
  /// data if it was present.
  pub fn remove_raw(&mut self, name: String) -> Option<Uint8Array> {
    self
      .module
      .inner
      .customs
      .remove_raw(&name)
      .map(|section| section.data.into())
  }

  #[napi]
  /// List the custom sections currently present in the module. `data` is the
  /// raw bytes for unparsed (raw) sections, or `null` for sections that walrus
  /// has parsed into a typed representation.
  pub fn list(&self) -> Vec<RawSectionInfo> {
    self
      .module
      .inner
      .customs
      .iter()
      .map(|(_, section)| RawSectionInfo {
        name: section.name().to_string(),
        data: section
          .as_any()
          .downcast_ref::<RawCustomSection>()
          .map(|raw| raw.data.clone().into()),
      })
      .collect()
  }
}

/// Information about a single custom section in the module.
#[napi(object)]
pub struct RawSectionInfo {
  pub name: String,
  pub data: Option<Uint8Array>,
}
