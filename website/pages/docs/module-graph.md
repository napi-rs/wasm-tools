---
title: 'The module graph'
description: 'Every part of a wasm module — functions, globals, memories, imports, exports — is a live handle that reads and writes straight through to the module.'
---

# The module graph

Every part of a module — functions, globals, memories, tables, imports, exports, types, data,
elements, locals, tags, custom sections, producers — is exposed as a **live handle**. Reading a
property reads through to the module; writing one writes back. Your edits persist when you call
`emitWasm()`.

> **Requires @napi-rs/wasm-tools ≥ 1.0.2** (the published 1.0.1 predates this API). See
> [Getting Started](/docs) for the version note.

## Collections vs. handles

A `WasmModule` exposes one **collection** per item kind — `mod.functions`, `mod.globals`,
`mod.memories`, `mod.tables`, `mod.types`, `mod.imports`, `mod.exports`, `mod.data`,
`mod.elements`, `mod.tags`, `mod.locals`, plus `mod.producers` and `mod.customs`. Each collection
caches nothing: every accessor materializes a **fresh item handle** (e.g. `WasmFunction`,
`WasmGlobal`) that carries the item's id plus a strong reference to the owning module.

Collections share a shape:

- `.length` — how many items.
- `.items()` — every item as a live handle (for `for … of`).
- `.getByIndex(i)` — the item whose stable `.index` equals `i`, or `null`.
- kind-specific lookups: `exports.byName(name)`, `imports.find(module, name)`, `globals.byName(name)`.

## The inspect loop

```ts
import { readFile } from 'node:fs/promises'
import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(await readFile('./module.wasm'))

console.log(
  `functions=${mod.functions.length} globals=${mod.globals.length} ` +
    `memories=${mod.memories.length} imports=${mod.imports.length} ` +
    `exports=${mod.exports.length}`,
)

for (const exp of mod.exports.items()) {
  switch (exp.kind) {
    case 'Function':
      console.log(`export fn   ${exp.name}`)
      break
    case 'Global':
      console.log(`export glob ${exp.name} : ${exp.global()!.ty.type}`)
      break
    case 'Memory':
      console.log(`export mem  ${exp.name} : ${exp.memory()!.initial} page(s)`)
      break
  }
}

for (const g of mod.globals.items()) {
  console.log(`global #${g.index} ${g.ty.type} mutable=${g.mutable} (${g.kind})`)
}

// Direct lookups, too:
mod.exports.byName('run') // -> WasmExport | null
mod.imports.find('env', 'log') // -> WasmImport | null
```

An export's `kind` (`'Function' | 'Table' | 'Memory' | 'Global' | 'Tag'`) tells you which typed
accessor to call — `exp.func()`, `exp.global()`, `exp.memory()`, etc. — each returning the handle
for its variant and `null` for the others.

## Edit through live handles, then emit

Mutations on a handle write straight back to the owning module. Rename an export, flip a global's
mutability, grow the initial memory, set the module name — then `emitWasm()` and re-parse to prove
every edit persisted.

```ts
const mod = WasmModule.fromBuffer(await readFile('./module.wasm'))

mod.name = 'patched-module'
mod.exports.byName('run')!.name = 'main' // rename an export
mod.exports.byName('counter')!.global()!.mutable = true // flip a global's mutability
mod.mainMemory!.initial = 4n // grow initial memory (page counts are bigint)

const bytes = mod.emitWasm(false)

const out = WasmModule.fromBuffer(bytes)
console.log(out.name) // 'patched-module'
console.log(out.exports.byName('run')) // null
console.log(out.exports.byName('main')!.kind) // 'Function'
console.log(out.mainMemory!.initial) // 4n
```

Page and entry counts are `bigint` throughout (memories/tables can be 64-bit). A handle stays valid
as long as you hold it, even across edits.
