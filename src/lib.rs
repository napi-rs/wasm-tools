#![deny(clippy::all)]

mod config;
mod module;

pub use config::ModuleConfig;
pub use module::WasmModule;
