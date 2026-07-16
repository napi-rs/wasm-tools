#![deny(clippy::all)]

mod config;
mod constexpr;
mod convert;
mod customs;
mod globals;
mod handle;
mod memories;
mod module;
mod producers;
mod tables;
mod types;
mod valtype;

pub use config::ModuleConfig;
pub use constexpr::{ConstExpr, ConstExprKind};
pub use customs::{RawSectionInfo, WasmCustomSections};
pub use globals::{GlobalKind, WasmGlobal, WasmGlobals};
pub use memories::{WasmMemories, WasmMemory};
pub use module::WasmModule;
pub use producers::{ProducerFieldInfo, ProducerValueInfo, WasmProducers};
pub use tables::{WasmTable, WasmTables};
pub use types::{TypeKind, WasmType, WasmTypes};
pub use valtype::{AbstractHeapType, HeapType, ValType};
