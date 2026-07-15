import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/tables.wat). Reading the bytes
// keeps the test hermetic — wat2wasm is never invoked at runtime.
//   table 0: initial 1, maximum 4, funcref
//   table 1: initial 2, no maximum, externref
const FIXTURE = join(__dirname, 'fixtures', 'tables.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

const FUNCREF = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } } as const
const EXTERNREF = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Extern' } } as const

test('tables collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.tables.length, 2)

  const items = m.tables.items()
  t.is(items.length, 2)

  t.is(items[0].initial, 1n)
  t.is(items[0].maximum, 4n)
  t.deepEqual(items[0].elementTy, FUNCREF)
  t.is(items[0].table64, false)
  t.is(items[0].isImported, false)
  t.is(items[0].init(), null)

  t.is(items[1].initial, 2n)
  t.is(items[1].maximum, null)
  t.deepEqual(items[1].elementTy, EXTERNREF)

  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const table = m.tables.getByIndex(1)
  t.truthy(table)
  t.is(table!.index, 1)
  t.deepEqual(table!.elementTy, EXTERNREF)

  t.is(m.tables.getByIndex(99), null)
})

test('mainFunctionTable returns the single funcref table', (t) => {
  const m = load()
  const main = m.tables.mainFunctionTable()
  t.truthy(main)
  t.is(main!.index, 0)
  t.deepEqual(main!.elementTy, FUNCREF)
})

test('write-through: setting maximum persists through emit and re-parse', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  t.is(table.maximum, 4n)
  table.maximum = 16n
  t.is(table.maximum, 16n)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.tables.getByIndex(0)!.maximum, 16n)
})

test('write-through: setting initial persists through emit and re-parse', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  table.initial = 3n
  t.is(table.initial, 3n)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.tables.getByIndex(0)!.initial, 3n)
})

test('write-through: renaming a table persists through emit and re-parse', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  t.is(table.name, null)
  table.name = 'renamed_table'
  t.is(table.name, 'renamed_table')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.tables.items().find((x) => x.name === 'renamed_table')
  t.truthy(found)
  t.is(found!.index, 0)
})

test('addLocal creates an externref table that round-trips', (t) => {
  const m = load()
  const table = m.tables.addLocal(false, 1n, null, EXTERNREF)
  t.is(m.tables.length, 3)
  t.is(table.initial, 1n)
  t.is(table.maximum, null)
  t.deepEqual(table.elementTy, EXTERNREF)
  t.is(table.table64, false)
  t.is(table.isImported, false)

  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
  t.is(reparsed.tables.length, 3)
  const rt = reparsed.tables.getByIndex(table.index)!
  t.deepEqual(rt.elementTy, EXTERNREF)
  t.is(rt.initial, 1n)
})

test('addLocal rejects a non-reference element type (catchable, never aborts)', (t) => {
  const m = load()
  const err = t.throws(() => m.tables.addLocal(false, 1n, null, { type: 'I32' }))
  t.regex(err!.message, /reference type/i)

  // The rejected add left the module untouched.
  t.is(m.tables.length, 2)
})

test('delete removes a table and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.tables.delete(m.tables.items()[1])
  t.is(m.tables.length, 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.tables.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.tables.items()[0]

  m.tables.delete(handle)
  t.is(m.tables.length, 1)

  const err = t.throws(() => m.tables.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.tables.items()[0]
  t.throws(() => a.tables.delete(bHandle))

  t.is(a.tables.length, 2)
  t.is(b.tables.length, 2)
})

test('delete-guard: using a handle after delete throws instead of crashing', (t) => {
  const m = load()
  const handle = m.tables.items()[0]
  m.tables.delete(handle)

  const err = t.throws(() => handle.initial)
  t.regex(err!.message, /deleted/)

  const err2 = t.throws(() => {
    handle.maximum = 9n
  })
  t.regex(err2!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 0)
})
