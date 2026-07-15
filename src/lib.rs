#![deny(clippy::all)]

mod config;
mod convert;
mod customs;
mod globals;
mod handle;
mod module;
mod producers;
mod valtype;

pub use config::ModuleConfig;
pub use customs::{RawSectionInfo, WasmCustomSections};
pub use globals::{GlobalKind, WasmGlobal, WasmGlobals};
pub use module::WasmModule;
pub use producers::{ProducerFieldInfo, ProducerValueInfo, WasmProducers};
pub use valtype::{AbstractHeapType, HeapType, ValType};
