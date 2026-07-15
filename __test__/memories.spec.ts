import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/memories.wat). Reading the bytes
// keeps the test hermetic — wat2wasm is never invoked at runtime.
//   memory 0: initial 1, maximum 2
//   memory 1: initial 3, no maximum
const FIXTURE = join(__dirname, 'fixtures', 'memories.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('memories collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.memories.length, 2)

  const items = m.memories.items()
  t.is(items.length, 2)

  // initial / maximum cross as bigint (memory64-capable, lossless).
  t.is(items[0].initial, 1n)
  t.is(items[0].maximum, 2n)
  t.is(items[1].initial, 3n)
  t.is(items[1].maximum, null)

  // creation-time flags, read only.
  t.is(items[0].shared, false)
  t.is(items[0].memory64, false)
  t.is(items[0].pageSizeLog2, null)
  t.is(items[0].isImported, false)

  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const mem = m.memories.getByIndex(1)
  t.truthy(mem)
  t.is(mem!.index, 1)
  t.is(mem!.initial, 3n)

  t.is(m.memories.getByIndex(99), null)
})

test('write-through: setting maximum persists through emit and re-parse', (t) => {
  const m = load()
  const mem = m.memories.items()[0]
  t.is(mem.maximum, 2n)
  mem.maximum = 8n
  t.is(mem.maximum, 8n)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.getByIndex(0)!.maximum, 8n)
})

test('write-through: setting initial persists through emit and re-parse', (t) => {
  const m = load()
  const mem = m.memories.items()[1]
  t.is(mem.initial, 3n)
  mem.initial = 5n
  t.is(mem.initial, 5n)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.getByIndex(1)!.initial, 5n)
})

test('write-through: renaming a memory persists through emit and re-parse', (t) => {
  const m = load()
  const mem = m.memories.items()[0]
  t.is(mem.name, null)
  mem.name = 'renamed_mem'
  t.is(mem.name, 'renamed_mem')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.memories.items().find((x) => x.name === 'renamed_mem')
  t.truthy(found)
  t.is(found!.index, 0)
})

test('addLocal creates a memory whose fields read back, and it round-trips', (t) => {
  const m = load()
  const mem = m.memories.addLocal(false, false, 2n, 5n, null)
  t.is(m.memories.length, 3)
  t.is(mem.initial, 2n)
  t.is(mem.maximum, 5n)
  t.is(mem.shared, false)
  t.is(mem.memory64, false)
  t.is(mem.isImported, false)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.length, 3)
  const rm = reparsed.memories.getByIndex(mem.index)!
  t.is(rm.initial, 2n)
  t.is(rm.maximum, 5n)
})

test('addLocal accepts a null maximum', (t) => {
  const m = load()
  const mem = m.memories.addLocal(false, false, 4n, null, null)
  t.is(mem.initial, 4n)
  t.is(mem.maximum, null)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.getByIndex(mem.index)!.maximum, null)
})

test('delete removes a memory and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.memories.delete(m.memories.items()[0])
  t.is(m.memories.length, 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.memories.items()[0]

  m.memories.delete(handle)
  t.is(m.memories.length, 1)

  const err = t.throws(() => m.memories.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.memories.items()[0]
  t.throws(() => a.memories.delete(bHandle))

  t.is(a.memories.length, 2)
  t.is(b.memories.length, 2)
})

test('delete-guard: using a handle after delete throws instead of crashing', (t) => {
  const m = load()
  const handle = m.memories.items()[0]
  m.memories.delete(handle)

  const err = t.throws(() => handle.initial)
  t.regex(err!.message, /deleted/)

  const err2 = t.throws(() => {
    handle.maximum = 9n
  })
  t.regex(err2!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 0)
})
