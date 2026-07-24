---
title: 'Getting Started'
description: 'walrus bindings for Node.js — read, edit, and build WebAssembly modules from JavaScript, backed by a native Rust addon.'
---

# Getting Started

`@napi-rs/wasm-tools` is [walrus](https://github.com/rustwasm/walrus) behind a small
JavaScript API: **read, edit, and build WebAssembly modules** from Node.js. It loads a
prebuilt native binary — no compiler, no `node-gyp`.

## Install

```bash
pnpm add @napi-rs/wasm-tools -D
```

```bash
yarn add @napi-rs/wasm-tools -D
```

> **Version note.** Most of this page requires **@napi-rs/wasm-tools ≥ 1.0.2** — the module
> graph (`mod.functions`, `mod.exports`, live handles), `buildFunction`, `instructions()`,
> `WasmModule.fromBuffer`, `emitWasmFile`, and the `I32 … V128` value constants all arrive in
> 1.0.2, which is **not published yet**. The current npm release (**1.0.1**) exposes only
> `ModuleConfig` (the chainable builder plus `parse`) and `WasmModule.fromPath` /
> `fromFileWithConfig` / `emitWasm` — the [Parse with options, then emit](#parse-with-options-then-emit)
> example below works on 1.0.1 today. To try the ≥ 1.0.2 APIs now, use the playground on this
> site, which runs a vendored pre-release build.

## Get a module

Parse wasm into a live `WasmModule`:

- `WasmModule.fromBuffer(bytes)` **(≥ 1.0.2)** — parse in-memory bytes with the defaults.
- `WasmModule.fromPath(path)` / `fromFileWithConfig(path, config)` — parse a file (available in 1.0.1).
- `new ModuleConfig().….parse(bytes)` — parse in-memory bytes with walrus options (DWARF, name section, …; available in 1.0.1).

```ts
// Reading mod.functions / mod.exports requires @napi-rs/wasm-tools >= 1.0.2.
import { readFile } from 'node:fs/promises'
import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(await readFile('./module.wasm'))
console.log(mod.functions.length, mod.exports.length)
```

## Parse with options, then emit

`ModuleConfig` is a chainable builder; `parse()` returns the module and `emitWasm(demangle)`
serializes it back to bytes.

```ts
import { ModuleConfig } from '@napi-rs/wasm-tools'

const binary = new ModuleConfig()
  .generateDwarf(true)
  .generateNameSection(true)
  .generateProducersSection(true)
  .preserveCodeTransform(true)
  .parse(wasm)
  .emitWasm(true)
```

`emitWasm(true)` demangles Rust/C++ symbol names in the output; pass `false` to keep them raw.
There is also `emitWasmFile(path, demangle)` to write straight to disk.

## Where to next

- **[The module graph](/docs/module-graph)** — the collections and live handles you read and edit through.
- **[Value types & constants](/docs/value-types)** — the ready-made `I32 … V128` and reference-type constants.
- **[Building functions](/docs/building-functions)** — synthesize function bodies from instruction descriptors.
- **[API reference](/docs/api-reference)** — every class and constant.
