#![deny(clippy::all)]

mod config;
mod customs;
mod globals;
mod handle;
mod module;
mod producers;

pub use config::ModuleConfig;
pub use customs::{RawSectionInfo, WasmCustomSections};
pub use globals::{WasmGlobal, WasmGlobals};
pub use module::WasmModule;
pub use producers::{ProducerFieldInfo, ProducerValueInfo, WasmProducers};
