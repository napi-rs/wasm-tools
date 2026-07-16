import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/functions.wat, built with
// `wat2wasm --debug-names`). Reading the bytes keeps the test hermetic —
// wat2wasm is never invoked at runtime.
//   func 0 ($imp): imported, type (param i32)            -> kind Import
//   func 1 ($loc): local,    type (param i32) (result i32) -> kind Local
const FIXTURE = join(__dirname, 'fixtures', 'functions.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('functions collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.functions.length, 2)

  const items = m.functions.items()
  t.is(items.length, 2)
  t.is(items.length, m.functions.length)

  // Every handle exposes a stable, numeric index.
  for (const f of items) {
    t.is(typeof f.index, 'number')
  }
})

test('kind distinguishes imported from locally defined functions', (t) => {
  const m = load()
  const imp = m.functions.getByIndex(0)!
  const loc = m.functions.getByIndex(1)!

  t.is(imp.kind, 'Import')
  t.is(loc.kind, 'Local')
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const loc = m.functions.getByIndex(1)
  t.truthy(loc)
  t.is(loc!.index, 1)
  t.is(loc!.kind, 'Local')

  t.is(m.functions.getByIndex(9999), null)
})

test('byName finds a named function and returns null for a miss', (t) => {
  const m = load()
  // Names survive because the fixture was built with --debug-names.
  const loc = m.functions.byName('loc')
  t.truthy(loc)
  t.is(loc!.kind, 'Local')
  t.is(loc!.index, 1)

  const imp = m.functions.byName('imp')
  t.truthy(imp)
  t.is(imp!.kind, 'Import')

  t.is(m.functions.byName('nope'), null)
})

test('ty() returns a WasmType handle whose signature matches the function', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!

  // Cross-link into the types collection: the local function is
  // (param i32) (result i32).
  const ty = loc.ty()
  t.is(ty.kind, 'Function')
  t.deepEqual(ty.params(), [{ type: 'I32' }])
  t.deepEqual(ty.results(), [{ type: 'I32' }])

  // The imported function is (param i32) with no results.
  const impTy = m.functions.byName('imp')!.ty()
  t.deepEqual(impTy.params(), [{ type: 'I32' }])
  t.deepEqual(impTy.results(), [])
})

test('write-through: renaming a function persists through emit and re-parse', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!
  loc.name = 'renamed'
  t.is(loc.name, 'renamed')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.functions.byName('renamed')
  t.truthy(found)
  t.is(found!.kind, 'Local')
})

test('delete removes a function and the removal persists through emit and re-parse', (t) => {
  const m = load()
  const before = m.functions.length

  // Delete the LOCAL function: it is referenced by nothing (no call, export, or
  // table element), so its removal cannot dangle any reference at emit time.
  m.functions.delete(m.functions.byName('loc')!)
  t.is(m.functions.length, before - 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.functions.byName('loc'), null)
  // The imported function is untouched.
  t.truthy(reparsed.functions.byName('imp'))
})

test('delete-guard: a deleted handle throws on .kind and .name instead of aborting', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!
  m.functions.delete(loc)

  const errK = t.throws(() => loc.kind)
  t.regex(errK!.message, /deleted/)
  const errN = t.throws(() => loc.name)
  t.regex(errN!.message, /deleted/)

  // The setter path is guarded too.
  const errS = t.throws(() => {
    loc.name = 'zombie'
  })
  t.regex(errS!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(typeof loc.index, 'number')
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!
  m.functions.delete(loc)

  const err = t.throws(() => m.functions.delete(loc))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.functions.byName('loc')!
  const aLen = a.functions.length
  const bLen = b.functions.length

  t.throws(() => a.functions.delete(bHandle))
  t.is(a.functions.length, aLen)
  t.is(b.functions.length, bLen)
})
