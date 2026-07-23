---
title: 'API Reference'
description: 'Every class and constant in @napi-rs/wasm-tools, grouped by role.'
---

# API Reference

Everything is exported from the package root:

```ts
import {
  WasmModule,
  ModuleConfig,
  ConstExpr,
  I32,
  I64,
  F32,
  F64,
  V128,
  FUNCREF,
  EXTERNREF,
  ANYREF,
  EQREF, // …and the rest of the reference-type constants
} from '@napi-rs/wasm-tools'
```

## Core

| Class | What it is |
| --- | --- |
| `WasmModule` | A parsed wasm module. Parse via `fromBuffer` / `fromPath`, edit through its collections, serialize with `emitWasm` / `emitWasmFile`; also `buildFunction`, `gc`, `writeGraphvizDot`, and the `name` / `start` / `mainMemory` accessors. |
| `ModuleConfig` | Chainable parse-options builder (`generateDwarf`, `generateNameSection`, `strictValidate`, `preserveCodeTransform`, …); `parse(bytes)` returns a `WasmModule`. |

## Collections

Each hangs off a `WasmModule` getter, caches nothing, and shares `.length` / `.items()` / `.getByIndex()`.

| Class | Module getter | What it holds |
| --- | --- | --- |
| `WasmFunctions` | `mod.functions` | Every function (imported + local); `byName`, `delete`. |
| `WasmGlobals` | `mod.globals` | Globals; `byName`, `addLocal`, `delete`. |
| `WasmMemories` | `mod.memories` | Memories; `addLocal`, `delete`. |
| `WasmTables` | `mod.tables` | Tables; `mainFunctionTable`, `addLocal` / `addLocalWithInit`, `delete`. |
| `WasmTypes` | `mod.types` | Type arena; `add`, `find`, `addStruct` / `addArray` / `addComposite` / `addRecGroup`. |
| `WasmDataSegments` | `mod.data` | Data segments; `addPassive`, `addActive`, `delete`. |
| `WasmElements` | `mod.elements` | Element segments; `addFunctions`, `addExpressions`, `delete`. |
| `WasmTags` | `mod.tags` | Exception-handling tags; `add`, `delete`. |
| `WasmImports` | `mod.imports` | Imports; `find`, `addFunction` / `addMemory` / `addTable` / `addGlobal` / `addTag`, `delete`. |
| `WasmExports` | `mod.exports` | Exports; `byName`, `addFunction` / `addTable` / `addMemory` / `addGlobal` / `addTag`, `delete`. |
| `WasmLocals` | `mod.locals` | Module-wide local arena; `add`. |
| `WasmProducers` | `mod.producers` | `producers` section; `addLanguage` / `addProcessedBy` / `addSdk`, `fields`, `clear`. |
| `WasmCustomSections` | `mod.customs` | Raw custom sections; `addRaw`, `removeRaw`, `list`. |

## Handles (items)

A live handle to one item — its stable `.index`, mutable `.name`, a read-only `kind`, and typed
cross-links back into the module.

| Class | What it is |
| --- | --- |
| `WasmFunction` | One function: `kind`, `ty()`, `instructions()`, `import()`. |
| `WasmGlobal` | One global: `ty`, `mutable`, `shared`, `kind`, `init()`, `import()`. |
| `WasmMemory` | One memory: `initial` / `maximum` (bigint pages), `shared`, `memory64`, `pageSizeLog2`, `import()`. |
| `WasmTable` | One table: `elementTy`, `initial` / `maximum` (bigint entries), `table64`, `init()`, `import()`. |
| `WasmType` | One type: `kind`, `params()` / `results()` / `structFields()` / `arrayElement()`, `supertype`, `recGroupMembers()`, `refNull()`. |
| `WasmData` | One data segment: `kind`, `value` bytes, `memory()`, `offset()`. |
| `WasmElement` | One element segment: `kind`, `itemsKind`, `table()`, `offset()`, `functionItems()` / `expressionItems()`. |
| `WasmTag` | One exception tag: `kind`, `ty()`, `import()`. |
| `WasmImport` | One import: `module` / `name`, `kind`, typed `func()` / `table()` / `memory()` / `global()` / `tag()`. |
| `WasmExport` | One export: `name`, `kind`, typed `func()` / `table()` / `memory()` / `global()` / `tag()`. |
| `WasmLocal` | One local: `ty` (read only), `name`. |

## Values

| Class | What it is |
| --- | --- |
| `ConstExpr` | A constant expression for initializers. Factory statics: `i32`, `i64`, `f32`, `f64`, `v128`, `globalGet`, `refFunc`, `refNull`; read `kind`. |

## Value-type constants

Numeric: `I32`, `I64`, `F32`, `F64`, `V128`.

Nullable reference types: `FUNCREF`, `EXTERNREF`, `ANYREF`, `EQREF`, `I31REF`, `STRUCTREF`,
`ARRAYREF`, `NULLREF`, `NULLFUNCREF`, `NULLEXTERNREF`, `EXNREF`, `NULLEXNREF`.

Each is a ready-made `ValType`, so `import { I32 }` replaces writing `{ type: 'I32' }` by hand.
