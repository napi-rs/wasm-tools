use napi::bindgen_prelude::{Reference, Result, Uint8Array};
use napi::Error;
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
  ///
  /// Names starting with `.debug` are rejected: walrus manages DWARF debug
  /// sections through its own parsed representation and silently drops any
  /// `.debug*` custom section from `emitWasm` output, so adding one here would
  /// be lost data. Add real debug info through the module's DWARF handling
  /// instead.
  pub fn add_raw(&mut self, name: String, data: Uint8Array) -> Result<()> {
    if name.starts_with(".debug") {
      return Err(Error::from_reason(format!(
        "custom section names starting with '.debug' are managed by walrus's DWARF handling and are not emitted; refusing to add '{name}'"
      )));
    }
    self.module.inner.customs.add(RawCustomSection {
      name,
      data: data.to_vec(),
    });
    Ok(())
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
