import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/globals.wat). Reading the bytes
// keeps the test hermetic — wat2wasm is never invoked at runtime.
//   global 0: mutable   i32 = 42
//   global 1: immutable i32 = 7
const FIXTURE = join(__dirname, 'fixtures', 'globals.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('globals collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.globals.length, 2)

  const items = m.globals.items()
  t.is(items.length, 2)
  t.is(items[0].mutable, true)
  t.is(items[1].mutable, false)
  t.is(items[0].shared, false)
  t.is(items[1].shared, false)
  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const g = m.globals.getByIndex(1)
  t.truthy(g)
  t.is(g!.index, 1)
  t.is(g!.mutable, false)

  t.is(m.globals.getByIndex(99), null)
})

test('byName finds a named global and returns null for a miss', (t) => {
  const m = load()
  // No global is named in the fixture yet.
  t.is(m.globals.byName('nope'), null)

  m.globals.items()[0].name = 'g_named'
  const found = m.globals.byName('g_named')
  t.truthy(found)
  t.is(found!.index, 0)
})

test('write-through: renaming a global persists through emit and re-parse', (t) => {
  const m = load()
  const g = m.globals.items()[0]
  t.is(g.name, null)
  g.name = 'renamed'
  t.is(g.name, 'renamed')

  // Name persists via the name section (generateNameSection defaults true).
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.globals.byName('renamed')
  t.truthy(found)
  t.is(found!.index, 0)
})

test('write-through: flipping mutable persists through emit and re-parse', (t) => {
  const m = load()
  t.is(m.globals.items()[1].mutable, false)
  m.globals.items()[1].mutable = true
  t.is(m.globals.items()[1].mutable, true)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.globals.getByIndex(1)!.mutable, true)
})

test('delete removes a global and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.globals.delete(m.globals.items()[0])
  t.is(m.globals.length, 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.globals.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.globals.items()[0]

  // First delete succeeds and drops the count.
  m.globals.delete(handle)
  t.is(m.globals.length, 1)

  // Deleting the SAME (now dead) handle again must raise a catchable JS error
  // rather than tripping walrus' `assert(contains(id))` and aborting via FFI.
  const err = t.throws(() => m.globals.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  // A handle minted from module B must never be accepted by module A: the ids
  // carry an arena_id, so the liveness scan rejects the foreign handle before
  // walrus can assert on it.
  const bHandle = b.globals.items()[0]
  t.throws(() => a.globals.delete(bHandle))

  // Neither module was mutated by the rejected delete.
  t.is(a.globals.length, 2)
  t.is(b.globals.length, 2)
})

test('delete-guard: using a handle after delete throws instead of crashing', (t) => {
  const m = load()
  const handle = m.globals.items()[0]
  m.globals.delete(handle)

  // Reading a scalar through the dead handle must raise a catchable JS error
  // (walrus would otherwise panic and abort the process across FFI).
  const err = t.throws(() => handle.mutable)
  t.regex(err!.message, /deleted/)

  // The setter path is guarded too.
  const err2 = t.throws(() => {
    handle.name = 'zombie'
  })
  t.regex(err2!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 0)
})
