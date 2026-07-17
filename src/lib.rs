#![deny(clippy::all)]

mod config;
mod constexpr;
mod convert;
mod customs;
mod data;
mod elements;
mod exports;
mod functions;
mod globals;
mod handle;
mod imports;
mod ir;
mod ir_marshal;
mod locals;
mod memories;
mod module;
mod producers;
mod safevec;
mod tables;
mod tags;
mod types;
mod valtype;

pub use config::ModuleConfig;
pub use constexpr::{ConstExpr, ConstExprKind};
pub use customs::{RawSectionInfo, WasmCustomSections};
pub use data::{DataKindTag, WasmData, WasmDataSegments};
pub use elements::{ElementItemsTag, ElementKindTag, WasmElement, WasmElements};
pub use exports::{ExportItemTag, WasmExport, WasmExports};
pub use functions::{FunctionKindTag, WasmFunction, WasmFunctions};
pub use globals::{GlobalKind, WasmGlobal, WasmGlobals};
pub use imports::{ImportKindTag, WasmImport, WasmImports};
pub use ir::{
  BlockType, ConstValue, ExtendedLoad, InstrDesc, LoadKind, MemArg, RefType, StoreKind,
};
pub use locals::{WasmLocal, WasmLocals};
pub use memories::{WasmMemories, WasmMemory};
pub use module::WasmModule;
pub use producers::{ProducerFieldInfo, ProducerValueInfo, WasmProducers};
pub use tables::{WasmTable, WasmTables};
pub use tags::{TagKindTag, WasmTag, WasmTags};
pub use types::{TypeKind, WasmType, WasmTypes};
pub use valtype::{AbstractHeapType, FieldType, HeapType, StorageType, ValType};
