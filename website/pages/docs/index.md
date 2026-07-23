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

## Get a module

Two ways to parse wasm into a live `WasmModule`:

- `WasmModule.fromBuffer(bytes)` / `WasmModule.fromPath(path)` — parse directly with the defaults.
- `new ModuleConfig().….parse(bytes)` — parse with walrus options (DWARF, name section, …).

```ts
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
