# `@napi-rs/wasm-tools`

![https://github.com/napi-rs/wasm-tools/actions](https://github.com/napi-rs/wasm-tools/workflows/CI/badge.svg)

> [walrus](https://github.com/rustwasm/walrus) bindings — read, edit, and build WebAssembly modules from JavaScript.

## Install this package

```
pnpm add @napi-rs/wasm-tools -D
yarn add @napi-rs/wasm-tools -D
```

## Usage

Two ways to get a module:

- `new ModuleConfig().…​.parse(bytes)` — parse with walrus options (DWARF, name section, …).
- `WasmModule.fromBuffer(bytes)` / `WasmModule.fromPath(path)` — parse directly.

Every part of the module — functions, globals, memories, tables, imports, exports,
types, data, elements, locals, tags, custom sections, producers — is exposed as a
**live handle**. Reading a property reads through to the module; writing one writes
back. Your edits persist when you call `emitWasm()`.

Value types come as ready-made constants — `I32`, `I64`, `F32`, `F64`, `V128` and the
nullable reference types (`FUNCREF`, `EXTERNREF`, `ANYREF`, `EQREF`, `I31REF`,
`STRUCTREF`, `ARRAYREF`, `NULLREF`, `NULLFUNCREF`, `NULLEXTERNREF`, `EXNREF`,
`NULLEXNREF`) — so you can `import { I32 }` instead of writing `{ type: 'I32' }` by hand.

### Parse with options and emit

```ts
import { join } from 'node:path'
import { readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

import { ModuleConfig } from '@napi-rs/wasm-tools'

const __dirname = join(fileURLToPath(import.meta.url), '..')

const wasm = await readFile(join(__dirname, 'panic.wasm32-wasi.wasm'))

const binary = new ModuleConfig()
  .generateDwarf(true)
  .generateNameSection(true)
  .generateProducersSection(true)
  .preserveCodeTransform(true)
  .parse(wasm)
  .emitWasm(true)

await writeFile(join(__dirname, 'panic.wasm32-wasi.wasm'), binary)
```

### Inspect a module

Walk the exports, imports, globals and memory through the live collection handles.

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

for (const imp of mod.imports.items()) {
  console.log(`import ${imp.module}/${imp.name} : ${imp.kind}`)
}

for (const g of mod.globals.items()) {
  console.log(`global #${g.index} ${g.ty.type} mutable=${g.mutable} (${g.kind})`)
}

// Direct lookups, too:
mod.exports.byName('run') // -> WasmExport | null
mod.imports.find('env', 'log') // -> WasmImport | null
```

### Edit through live handles, then emit

Mutations on a handle write straight back to the owning module. Rename an export,
flip a global's mutability, grow the initial memory, set the module name — then
`emitWasm()` and re-parse to prove every edit persisted.

```ts
import { readFile } from 'node:fs/promises'
import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(await readFile('./module.wasm'))

mod.name = 'patched-module'
mod.exports.byName('run')!.name = 'main' // rename an export
mod.exports.byName('counter')!.global()!.mutable = true // flip a global's mutability
mod.mainMemory!.initial = 4n // grow initial memory (page counts are bigint)

const bytes = mod.emitWasm(false)

// Re-parse to prove the edits are baked into the emitted wasm:
const out = WasmModule.fromBuffer(bytes)
console.log(out.name) // 'patched-module'
console.log(out.exports.byName('run')) // null
console.log(out.exports.byName('main')!.kind) // 'Function'
console.log(out.mainMemory!.initial) // 4n
```

### Build a function

Create a locally-defined function from an instruction descriptor tree, export it,
and emit a runnable module.

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

// It really runs:
const { instance } = await WebAssembly.instantiate(bytes)
const add = instance.exports.add as (a: number, b: number) => number
console.log(add(2, 3)) // 5

// And the body round-trips back to descriptors:
const fn = WasmModule.fromBuffer(bytes).exports.byName('add')!.func()!
console.log(fn.instructions())
// [ { type: 'LocalGet', local: 0 }, { type: 'LocalGet', local: 1 }, { type: 'Binop', op: 'I32Add' } ]
```

### Create globals and memories

Collection `add*` methods synthesize new items and hand you a live handle.

```ts
import { ConstExpr, I32, WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(new Uint8Array([0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0]))

// globals.addLocal(ty, mutable, shared, init)
const answer = mod.globals.addLocal(I32, false, false, ConstExpr.i32(42))

// memories.addLocal(shared, memory64, initial, maximum?) — page counts are bigint
const mem = mod.memories.addLocal(false, false, 1n, 2n)

mod.exports.addGlobal('answer', answer)
mod.exports.addMemory('memory', mem)

const { instance } = await WebAssembly.instantiate(mod.emitWasm(false))
const answerGlobal = instance.exports.answer as WebAssembly.Global
console.log(answerGlobal.value) // 42
console.log(instance.exports.memory instanceof WebAssembly.Memory) // true
```

### Record toolchain provenance

The `producers` custom section writes through the module like every other handle.

```ts
import { readFile } from 'node:fs/promises'
import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(await readFile('./module.wasm'))

mod.producers.addLanguage('Rust', '1.79')
mod.producers.addProcessedBy('my-bundler', '0.3.1')

const out = WasmModule.fromBuffer(mod.emitWasm(false))
for (const field of out.producers.fields()) {
  console.log(`${field.name}: ${field.values.map((v) => `${v.name}@${v.version}`).join(', ')}`)
}
// processed-by: walrus@0.26.4, my-bundler@0.3.1  (walrus appends its own entry on emit)
// language: Rust@1.79
```
