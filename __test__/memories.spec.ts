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

test('addLocal rejects a negative initial size and leaves the collection unchanged', (t) => {
  const m = load()
  const err = t.throws(() => m.memories.addLocal(false, false, -5n, null, null))
  t.regex(err!.message, /non-negative/)
  t.is(m.memories.length, 2)
})

test('addLocal rejects an out-of-range (u64 overflow) initial size and leaves the collection unchanged', (t) => {
  const m = load()
  const err = t.throws(() => m.memories.addLocal(false, false, 2n ** 64n, null, null))
  t.regex(err!.message, /non-negative/)
  t.is(m.memories.length, 2)
})

test('addLocal rejects an out-of-range maximum before mutating', (t) => {
  const m = load()
  const err = t.throws(() => m.memories.addLocal(false, false, 1n, 2n ** 64n, null))
  t.regex(err!.message, /non-negative/)
  t.is(m.memories.length, 2)
})

// F-fix6 (Codex P2): `pageSizeLog2` is carried as `f64` and narrowed LOSSLESSLY
// through `checked_index` before the walrus call. Under the OLD `Option<u32>`
// wire type napi applied ToUint32 FIRST, so an out-of-domain value silently
// ALIASED a different valid page size (`-1` -> 4294967295, `2**32` -> 0, `1.5`
// -> 1, `NaN` -> 0). That silent value corruption is now a catchable throw and
// the rejected add never mutates the collection.
test('addLocal rejects an out-of-domain pageSizeLog2 and leaves the collection unchanged', (t) => {
  const m = load()
  for (const bad of [-1, 2 ** 32, 1.5, NaN]) {
    const err = t.throws(() => m.memories.addLocal(false, false, 1n, null, bad))
    t.regex(err!.message, /pageSizeLog2 must be an integer in 0\.\.=4294967295/)
  }
  t.is(m.memories.length, 2)
})

test('addLocal accepts a valid pageSizeLog2 and the getter reads it back', (t) => {
  const m = load()
  // 16 = the default 64 KiB pages (2**16), a valid custom-page-sizes log2.
  const mem = m.memories.addLocal(false, false, 1n, null, 16)
  t.is(m.memories.length, 3)
  t.is(mem.pageSizeLog2, 16)
})

test('set initial rejects a negative size and preserves the original through emit and re-parse', (t) => {
  const m = load()
  const mem = m.memories.items()[0]
  t.is(mem.initial, 1n)

  const err = t.throws(() => {
    mem.initial = -5n
  })
  t.regex(err!.message, /non-negative/)
  // No partial mutation: in-memory value untouched...
  t.is(mem.initial, 1n)
  // ...and the emitted module still carries the original limit.
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.getByIndex(0)!.initial, 1n)
})

test('set maximum rejects an out-of-range size and preserves the original through emit and re-parse', (t) => {
  const m = load()
  const mem = m.memories.items()[0]
  t.is(mem.maximum, 2n)

  const err = t.throws(() => {
    mem.maximum = 2n ** 64n
  })
  t.regex(err!.message, /non-negative/)
  t.is(mem.maximum, 2n)
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.memories.getByIndex(0)!.maximum, 2n)
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
