//! napi representations of the wasm value types, mirroring
//! `walrus::{ValType, HeapType, AbstractHeapType}`.
//!
//! These are the READ-ONLY value layer: walrus values are converted into these
//! enums for JavaScript (see [`crate::convert`]). The reverse direction (JS ->
//! walrus, needed for building `ConstExpr`/`addLocal`) is a separate later task
//! and is intentionally not implemented here (YAGNI).

use napi_derive::napi;

/// A wasm value type.
///
/// Generated as a TypeScript discriminated union keyed on `type`, e.g.
/// `{ type: 'I32' } | ... | { type: 'Ref'; nullable: boolean; heap: HeapType }`.
///
/// `walrus::RefType { nullable, heap_type }` is inlined into the `Ref` variant;
/// there is no standalone `RefType` napi type.
///
/// `Clone` is derived so the read-only `buildFunction` preflight
/// ([`crate::ir::validate_body`]) can hand a borrowed value type to the same
/// consuming resolver (`val_type_to_walrus_in`) the emit path uses, without
/// diverging from it.
#[napi]
#[derive(Clone)]
pub enum ValType {
  /// 32-bit integer.
  I32,
  /// 64-bit integer.
  I64,
  /// 32-bit float.
  F32,
  /// 64-bit float.
  F64,
  /// 128-bit vector.
  V128,
  /// A reference type: `nullable` mirrors `RefType::nullable`, `heap` mirrors
  /// `RefType::heap_type`.
  Ref { nullable: bool, heap: HeapType },
}

/// A heap type for reference (`ValType::Ref`) values.
///
/// The `Concrete` and `Exact` variants carry a `type_index` — the stable
/// `.index()` of the referenced type in the module's type arena. The pure
/// value-layer conversion is read-only (an index alone cannot rebuild a walrus
/// `TypeId`); the reverse direction is resolved against a live module by the
/// module-aware converters in [`crate::convert`] (`resolve_type_id`), used when
/// a struct/array field references another type via `(ref $t)`.
#[napi]
#[derive(Clone)]
pub enum HeapType {
  /// An abstract heap type (`func`, `extern`, `any`, ...).
  Abstract { kind: AbstractHeapType },
  /// A concrete (indexed) heap type: `(ref $t)`, referencing a defined type by
  /// its stable index.
  Concrete { type_index: u32 },
  /// An exact heap type: `(ref exact $t)` (custom-descriptors proposal),
  /// referencing a defined type by its stable index.
  Exact { type_index: u32 },
}

/// An abstract heap type, mirroring `walrus::AbstractHeapType` 1:1.
#[napi(string_enum)]
#[derive(Clone)]
pub enum AbstractHeapType {
  /// The abstract `func` heap type (any function).
  Func,
  /// The abstract `extern` heap type (external/host references).
  Extern,
  /// The abstract `any` heap type (any internal reference).
  Any,
  /// The abstract `none` heap type (bottom type for internal refs).
  None,
  /// The abstract `noextern` heap type (bottom type for external refs).
  NoExtern,
  /// The abstract `nofunc` heap type (bottom type for function refs).
  NoFunc,
  /// The abstract `eq` heap type (comparable references: i31, struct, array).
  Eq,
  /// The abstract `struct` heap type.
  Struct,
  /// The abstract `array` heap type.
  Array,
  /// The abstract `i31` heap type (31-bit integers).
  I31,
  /// The abstract `exn` heap type (exceptions).
  Exn,
  /// The abstract `noexn` heap type (bottom type for exception refs).
  NoExn,
}

/// A packed storage type for GC struct and array fields, mirroring
/// `walrus::StorageType`.
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'I8' } | { type: 'I16' } | { type: 'Val'; value: ValType }`.
///
/// The packed `I8` / `I16` variants store a smaller integer than a full value
/// type (read back through `struct.get_s` / `struct.get_u`); they unpack to
/// `i32`. The `Val` variant wraps any ordinary [`ValType`] (including a
/// `(ref $t)` reference to another type).
#[napi]
pub enum StorageType {
  /// An 8-bit packed integer field.
  I8,
  /// A 16-bit packed integer field.
  I16,
  /// A standard (unpacked) value type field.
  Val { value: ValType },
}

/// A field type for GC struct and array fields, mirroring `walrus::FieldType`.
///
/// Combines a [`StorageType`] with a mutability flag. Generated as a TypeScript
/// object `{ storage: StorageType; mutable: boolean }`.
#[napi(object)]
pub struct FieldType {
  /// The storage type of this field.
  pub storage: StorageType,
  /// Whether this field is mutable.
  pub mutable: bool,
}

/// A composite type to create via [`crate::types::WasmTypes::add_composite`],
/// mirroring the shape of `walrus::CompositeType` (`Function | Struct | Array`).
///
/// Generated as a TypeScript discriminated union keyed on `type`:
/// `{ type: 'Struct'; fields: FieldType[] }`
/// `| { type: 'Array'; element: FieldType }`
/// `| { type: 'Function'; params: ValType[]; results: ValType[] }`.
///
/// This is the *creation* input for a composite type; the created type's shape
/// is read back through the individual `structFields()` / `arrayElement()` /
/// `params()` / `results()` accessors. Fields/params/results are converted via
/// the module-aware write path, so a `Concrete`/`Exact` ref to an EXISTING type
/// in the module resolves (a bad/entry-type index is rejected catchably).
#[napi]
pub enum CompositeType {
  /// A GC struct type: a sequence of field types.
  Struct {
    /// The struct's fields, in order.
    fields: Vec<FieldType>,
  },
  /// A GC array type: a single element field type shared by all elements.
  Array {
    /// The element field type.
    element: FieldType,
  },
  /// A function type: parameter and result value types.
  Function {
    /// The parameter value types.
    params: Vec<ValType>,
    /// The result value types.
    results: Vec<ValType>,
  },
}
