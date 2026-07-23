// Snippets seeded verbatim-in-spirit from the @napi-rs/wasm-tools README. Raw source
// lives here; the SSR loader highlights each with Shiki and hands the HTML to CodeBlock.

export const inspectSample = `import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(bytes)

// Every collection is a live handle: .length, .items(), lookups.
for (const exp of mod.exports.items()) {
  if (exp.kind === 'Global') {
    console.log(exp.name, exp.global()!.ty.type)
  }
}

mod.exports.byName('run')       // WasmExport | null
mod.imports.find('env', 'log')  // WasmImport | null`

export const editSample = `// Mutations write straight back to the owning module.
mod.name = 'patched-module'
mod.exports.byName('run')!.name = 'main'              // rename an export
mod.exports.byName('counter')!.global()!.mutable = true
mod.mainMemory!.initial = 4n                          // grow memory (bigint)

const bytes = mod.emitWasm(false)`

export const buildSample = `import { I32, WasmModule } from '@napi-rs/wasm-tools'

// Locals are module-wide; make them, then bind as params.
const a = mod.locals.add(I32)
const b = mod.locals.add(I32)

const idx = mod.buildFunction(
  [I32, I32], [I32], [a.index, b.index],
  [
    { type: 'LocalGet', local: a.index },
    { type: 'LocalGet', local: b.index },
    { type: 'Binop', op: 'I32Add' },
  ],
)`

export const storySample = `import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(await readFile('./module.wasm'))

mod.exports.byName('run')!.name = 'main'   // rename an export
mod.mainMemory!.initial = 4n               // grow initial memory

// Re-parse the emitted bytes to prove every edit baked in:
const out = WasmModule.fromBuffer(mod.emitWasm(false))
out.exports.byName('run')         // null
out.exports.byName('main')!.kind  // 'Function'
out.mainMemory!.initial           // 4n`

export const builderSample = `import { I32, WasmModule } from '@napi-rs/wasm-tools'

// Start from the 8-byte empty module (\`\\0asm\` + version 1).
const mod = WasmModule.fromBuffer(new Uint8Array([0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0]))

// Locals are module-wide; make them, then bind as params.
const a = mod.locals.add(I32)
const b = mod.locals.add(I32)

const idx = mod.buildFunction(
  [I32, I32], [I32], [a.index, b.index],
  [
    { type: 'LocalGet', local: a.index },
    { type: 'LocalGet', local: b.index },
    { type: 'Binop', op: 'I32Add' },
  ],
)
mod.exports.addFunction('add', mod.functions.getByIndex(idx)!)

const bytes = mod.emitWasm(false)
// new Uint8Array(bytes) narrows the buffer type so instantiate picks its BufferSource
// overload (emitWasm's Uint8Array<ArrayBufferLike> otherwise resolves to the Module one).
const { instance } = await WebAssembly.instantiate(new Uint8Array(bytes))
const add = instance.exports.add as (a: number, b: number) => number
console.log(add(2, 3)) // 5

// The body round-trips back to descriptors:
const fn = WasmModule.fromBuffer(bytes).exports.byName('add')!.func()!
fn.instructions()
// [ { type: 'LocalGet', local: 0 }, … { type: 'Binop', op: 'I32Add' } ]`
