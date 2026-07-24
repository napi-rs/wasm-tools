---
title: 'Building functions'
description: 'Synthesize a function body from an instruction-descriptor tree with buildFunction, and read any body back with instructions().'
---

# Building functions

`buildFunction` mints a new locally-defined function from an array of **instruction descriptors**
(`InstrDesc`) and appends it to the module, returning its stable index.

> **Requires @napi-rs/wasm-tools ≥ 1.0.2** (the published 1.0.1 predates this API). See
> [Getting Started](/docs) for the version note.

```ts
buildFunction(
  params: ValType[],
  results: ValType[],
  argLocalIndices: number[],
  body: InstrDesc[],
): number
```

- `params` / `results` — the function signature.
- `argLocalIndices` — the stable indices of the locals bound to the parameters, in order. Locals
  are **module-wide**: create them first with `mod.locals.add(ty)`.
- `body` — the instruction body. Branch targets are **relative label depths** (`0` = the innermost
  enclosing `block`/`loop`/`if`), matching wasm.

## An `add` function

```ts
import { I32, WasmModule } from '@napi-rs/wasm-tools'

// Start from the 8-byte empty module (`\0asm` + version 1).
const mod = WasmModule.fromBuffer(new Uint8Array([0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0]))

// Locals are module-wide; create them first, then bind them as params.
const a = mod.locals.add(I32)
const b = mod.locals.add(I32)

const idx = mod.buildFunction(
  [I32, I32], // params
  [I32], // results
  [a.index, b.index], // which locals are the params
  [
    { type: 'LocalGet', local: a.index },
    { type: 'LocalGet', local: b.index },
    { type: 'Binop', op: 'I32Add' },
  ],
)
mod.exports.addFunction('add', mod.functions.getByIndex(idx)!)

const bytes = mod.emitWasm(false)

// It really runs (new Uint8Array(bytes) narrows the buffer type so `instantiate` picks its
// BufferSource overload — emitWasm's Uint8Array<ArrayBufferLike> resolves to the Module one):
const { instance } = await WebAssembly.instantiate(new Uint8Array(bytes))
const add = instance.exports.add as (a: number, b: number) => number
console.log(add(2, 3)) // 5
```

## InstrDesc

`InstrDesc` is one wide tagged record shared by both directions — the `type` field is the
discriminant (the walrus variant name, e.g. `"LocalGet"`, `"Const"`, `"Binop"`, `"Block"`, `"Br"`),
and only the fields relevant to that `type` are set. A `Const` carries `value` (a `ConstValue`
union); `LocalGet`/`LocalSet`/`LocalTee` carry `local`; `GlobalGet`/`GlobalSet` carry `global`;
`Call`/`RefFunc` carry `func`; `Binop`/`Unop`/`TernOp` carry `op`. Control-flow bodies nest as
`InstrDesc[]` (`seq` for `block`/`loop`; `consequent`/`alternative` for `if`/`else`).

## Reading a body back — the round-trip

`instructions()` is the exact inverse of `buildFunction`: reading a body and building it back
round-trips.

```ts
const fn = WasmModule.fromBuffer(bytes).exports.byName('add')!.func()!
console.log(fn.instructions())
// [ { type: 'LocalGet', local: 0 }, { type: 'LocalGet', local: 1 }, { type: 'Binop', op: 'I32Add' } ]
```

Only a **local** function has a body; `instructions()` throws a catchable error on an imported
function (whose body lives in the host). Bodies are not validated for wasm well-formedness — an
ill-typed body is emitted as-is, and `WebAssembly.validate` (or a re-parse) is where you catch it.

## Related builders

- `replaceExportedFunc(funcIndex, argLocalIndices, body)` — swap an exported function's body for a
  freshly built one (signature inherited).
- `replaceImportedFunc(funcIndex, argLocalIndices, body)` — turn an imported function into a local
  one in place; existing `Call` references stay valid.
