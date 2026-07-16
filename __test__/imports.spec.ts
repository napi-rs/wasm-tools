import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/imports-exports.wat, built at
// authoring time with `wat2wasm`). Reading the bytes keeps the test hermetic —
// wat2wasm is never invoked at runtime.
//   import 0 = "e"/"f" -> Function (function index 0)
//   import 1 = "e"/"g" -> Global   (global   index 0)
//   memory index 0 (export "mem"), function index 1 (export "run"),
//   global index 1 (export "g2")
const FIXTURE = join(__dirname, 'fixtures', 'imports-exports.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('imports collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.imports.length, 2)

  const items = m.imports.items()
  t.is(items.length, 2)
  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('a function import exposes module/name/kind and the func cross-link', (t) => {
  const m = load()
  const imp = m.imports.items()[0]

  t.is(imp.module, 'e')
  t.is(imp.name, 'f')
  t.is(imp.kind, 'Function')

  // The matching cross-link resolves to the real function handle...
  const fn = imp.func()
  t.truthy(fn)
  t.is(fn!.index, 0)
  // ...and every non-matching accessor returns null.
  t.is(imp.table(), null)
  t.is(imp.memory(), null)
  t.is(imp.global(), null)
  t.is(imp.tag(), null)
})

test('a global import exposes the global cross-link', (t) => {
  const m = load()
  const imp = m.imports.items()[1]

  t.is(imp.kind, 'Global')
  const g = imp.global()
  t.truthy(g)
  t.is(g!.index, 0)
  t.is(imp.func(), null)
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const imp = m.imports.getByIndex(1)
  t.truthy(imp)
  t.is(imp!.index, 1)
  t.is(imp!.kind, 'Global')

  t.is(m.imports.getByIndex(99), null)
})

test('find locates an import by module and name, null for a miss', (t) => {
  const m = load()
  const imp = m.imports.find('e', 'f')
  t.truthy(imp)
  t.is(imp!.kind, 'Function')
  t.is(imp!.func()!.index, 0)

  t.truthy(m.imports.find('e', 'g'))
  t.is(m.imports.find('e', 'nope'), null)
  t.is(m.imports.find('nope', 'f'), null)
})

test('write-through: renaming an import module/name persists through emit and re-parse', (t) => {
  const m = load()
  const imp = m.imports.items()[0]
  imp.module = 'env'
  imp.name = 'ff'
  t.is(imp.module, 'env')
  t.is(imp.name, 'ff')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.truthy(reparsed.imports.find('env', 'ff'))
  t.is(reparsed.imports.find('e', 'f'), null)
})

test('delete removes an import and reduces the length', (t) => {
  const m = load()
  // Deleting an import orphans its item (invalid module) — mirror-walrus, the
  // user's concern; we only verify the collection shrinks and no abort occurs.
  m.imports.delete(m.imports.items()[0])
  t.is(m.imports.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.imports.items()[0]

  m.imports.delete(handle)
  t.is(m.imports.length, 1)

  const err = t.throws(() => m.imports.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.imports.items()[0]
  t.throws(() => a.imports.delete(bHandle))

  t.is(a.imports.length, 2)
  t.is(b.imports.length, 2)
})

test('delete-guard: using a handle after delete throws on kind/module/name instead of crashing', (t) => {
  const m = load()
  const handle = m.imports.items()[0]
  m.imports.delete(handle)

  const errK = t.throws(() => handle.kind)
  t.regex(errK!.message, /deleted/)
  const errM = t.throws(() => handle.module)
  t.regex(errM!.message, /deleted/)
  const errN = t.throws(() => handle.name)
  t.regex(errN!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 0)
})
