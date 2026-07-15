//! walrus -> napi conversions for the value-type layer (READ direction only).
//!
//! All matches over walrus enums are written out arm-by-arm so a *known*
//! variant can never be silently mismapped. `walrus::ValType` is not
//! `#[non_exhaustive]`, so its match has no catch-all and a future variant
//! would fail to compile here. `walrus::HeapType` and
//! `walrus::AbstractHeapType` ARE `#[non_exhaustive]` (see walrus `ty.rs`), so
//! the compiler forces a trailing `_` arm; every variant walrus 0.26.4 can
//! actually produce is handled explicitly before it, and the catch-all is
//! `unreachable!` — it exists only to satisfy the `#[non_exhaustive]` contract.

use crate::valtype::{AbstractHeapType, HeapType, ValType};

impl From<walrus::ValType> for ValType {
  fn from(ty: walrus::ValType) -> Self {
    match ty {
      walrus::ValType::I32 => ValType::I32,
      walrus::ValType::I64 => ValType::I64,
      walrus::ValType::F32 => ValType::F32,
      walrus::ValType::F64 => ValType::F64,
      walrus::ValType::V128 => ValType::V128,
      walrus::ValType::Ref(rt) => ValType::Ref {
        nullable: rt.nullable,
        heap: rt.heap_type.into(),
      },
    }
  }
}

impl From<walrus::HeapType> for HeapType {
  fn from(heap: walrus::HeapType) -> Self {
    match heap {
      walrus::HeapType::Abstract(abstract_type) => HeapType::Abstract {
        kind: abstract_type.into(),
      },
      walrus::HeapType::Concrete(id) => HeapType::Concrete {
        type_index: id.index() as u32,
      },
      walrus::HeapType::Exact(id) => HeapType::Exact {
        type_index: id.index() as u32,
      },
      // `walrus::HeapType` is `#[non_exhaustive]`; walrus 0.26.4 produces only
      // the three variants above, so this arm is unreachable in practice.
      _ => unreachable!("walrus produced an unknown HeapType variant"),
    }
  }
}

impl From<walrus::AbstractHeapType> for AbstractHeapType {
  fn from(abstract_type: walrus::AbstractHeapType) -> Self {
    match abstract_type {
      walrus::AbstractHeapType::Func => AbstractHeapType::Func,
      walrus::AbstractHeapType::Extern => AbstractHeapType::Extern,
      walrus::AbstractHeapType::Any => AbstractHeapType::Any,
      walrus::AbstractHeapType::None => AbstractHeapType::None,
      walrus::AbstractHeapType::NoExtern => AbstractHeapType::NoExtern,
      walrus::AbstractHeapType::NoFunc => AbstractHeapType::NoFunc,
      walrus::AbstractHeapType::Eq => AbstractHeapType::Eq,
      walrus::AbstractHeapType::Struct => AbstractHeapType::Struct,
      walrus::AbstractHeapType::Array => AbstractHeapType::Array,
      walrus::AbstractHeapType::I31 => AbstractHeapType::I31,
      walrus::AbstractHeapType::Exn => AbstractHeapType::Exn,
      walrus::AbstractHeapType::NoExn => AbstractHeapType::NoExn,
      // `walrus::AbstractHeapType` is `#[non_exhaustive]`; walrus 0.26.4
      // produces only the twelve variants above, so this arm is unreachable in
      // practice.
      _ => unreachable!("walrus produced an unknown AbstractHeapType variant"),
    }
  }
}
