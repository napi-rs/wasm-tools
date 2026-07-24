---
title: 'Value types & constants'
description: 'Ready-made ValType constants — I32 … V128 and the reference types — plus the discriminated-union shape behind them.'
---

# Value types & constants

Value types come as ready-made constants, so you can `import { I32 }` instead of writing
`{ type: 'I32' }` by hand.

> **Requires @napi-rs/wasm-tools ≥ 1.0.2.** The `I32 … V128` value constants are not exported by
> the published 1.0.1. See [Getting Started](/docs) for the version note.

## The five numeric types

```ts
import { I32, I64, F32, F64, V128 } from '@napi-rs/wasm-tools'
```

## The twelve nullable reference types

```ts
import {
  FUNCREF,
  EXTERNREF,
  ANYREF,
  EQREF,
  I31REF,
  STRUCTREF,
  ARRAYREF,
  NULLREF,
  NULLFUNCREF,
  NULLEXTERNREF,
  EXNREF,
  NULLEXNREF,
} from '@napi-rs/wasm-tools'
```

Each is a `(ref null …)` type — e.g. `FUNCREF` is `(ref null func)`, `EXTERNREF` is
`(ref null extern)`, `EQREF` is `(ref null eq)`.

## What a ValType really is

A `ValType` is a discriminated union keyed on `type`. The constants above are just frozen values of
it, so anywhere the API asks for a `ValType` you may pass a constant or an inline object:

```ts
{ type: 'I32' }
{ type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } }   // funcref = (ref null func)
{ type: 'Ref', nullable: false, heap: { type: 'Concrete', typeIndex } }     // (ref $t) — a concrete, non-null ref
```

The `heap` of a `Ref` is itself a union: `Abstract` (an `AbstractHeapType` such as `Func`, `Extern`,
`Any`, `Eq`, `Struct`, `Array`, `I31`, `Exn`, …), `Concrete`/`Exact` (a `typeIndex` into the
module's type arena), or the write-only `RecGroup` placeholder used inside `types.addRecGroup`.

**Handle-carried fields are read-only.** A handle's type-ish getters expose the resolved value and
have no setter: `global.ty`, `local.ty`, `table.elementTy` are read only (walrus exposes no setter),
as are creation-time properties like `memory.shared`, `memory.memory64`, and every `kind`
discriminant. Mutable metadata (an item's `name`, a global's `mutable`, a memory's `initial` /
`maximum`) does have setters and writes straight back.

## Concrete refs must name a live type

Any `{ type: 'Ref', nullable: false, heap: { type: 'Concrete', typeIndex } }` you pass to `globals.addLocal`,
`locals.add`, `tables.addLocal`, `types.add`, … must index an **existing** type in this module; an
index that names no live type is rejected with a catchable error, never a process abort.
