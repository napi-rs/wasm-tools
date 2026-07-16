import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ConstExpr, WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/data.wat). Reading the bytes
// keeps the test hermetic — wat2wasm is never invoked at runtime.
//   memory 0: initial 1
//   data[0]: ACTIVE  memory 0, offset (i32.const 0), bytes "hello"
//   data[1]: PASSIVE                                  bytes "world"
const FIXTURE = join(__dirname, 'fixtures', 'data.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

// The canonical 8-byte empty module (valid header, zero sections). Used to build
// a foreign module whose items are alien to the fixture module.
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const empty = () => WasmModule.fromBuffer(EMPTY_MODULE)

const bytes = (s: string) => new Uint8Array([...s].map((c) => c.charCodeAt(0)))

test('data collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.data.length, 2)

  const items = m.data.items()
  t.is(items.length, 2)
  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('kind distinguishes active from passive segments', (t) => {
  const m = load()
  const items = m.data.items()
  t.is(items[0].kind, 'Active')
  t.is(items[1].kind, 'Passive')
})

test('value reads the segment payload bytes', (t) => {
  const m = load()
  const items = m.data.items()
  t.deepEqual(items[0].value, bytes('hello'))
  t.deepEqual(items[1].value, bytes('world'))
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const seg = m.data.getByIndex(1)
  t.truthy(seg)
  t.is(seg!.index, 1)
  t.is(seg!.kind, 'Passive')
  t.is(m.data.getByIndex(99), null)
})

test('active segment exposes its memory and offset; passive returns null for both', (t) => {
  const m = load()
  const [active, passive] = m.data.items()

  const mem = active.memory()
  t.truthy(mem)
  t.is(mem!.index, 0)

  const off = active.offset()
  t.truthy(off)
  t.is(off!.kind, 'Value')

  t.is(passive.memory(), null)
  t.is(passive.offset(), null)
})

test('name get/set round-trips through emit and re-parse', (t) => {
  const m = load()
  const seg = m.data.items()[1]
  t.is(seg.name, null)
  seg.name = 'my_data'
  t.is(seg.name, 'my_data')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.data.items().find((d) => d.name === 'my_data')
  t.truthy(found)
  t.is(found!.index, 1)
})

test('write-through: setting value persists through emit and re-parse', (t) => {
  const m = load()
  const seg = m.data.items()[1]
  t.deepEqual(seg.value, bytes('world'))

  seg.value = new Uint8Array([1, 2, 3])
  t.deepEqual(seg.value, new Uint8Array([1, 2, 3]))

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.deepEqual(reparsed.data.getByIndex(1)!.value, new Uint8Array([1, 2, 3]))
})

test('addPassive appends a passive segment that round-trips', (t) => {
  const m = load()
  const seg = m.data.addPassive(new Uint8Array([9, 9]))
  t.is(seg.kind, 'Passive')
  t.is(seg.memory(), null)
  t.is(seg.offset(), null)
  t.is(m.data.length, 3)
  t.deepEqual(seg.value, new Uint8Array([9, 9]))

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.data.length, 3)
  t.deepEqual(reparsed.data.getByIndex(seg.index)!.value, new Uint8Array([9, 9]))
})

test('addActive appends an active segment tied to a memory, and it round-trips', (t) => {
  const m = load()
  const mem = m.memories.items()[0]
  const seg = m.data.addActive(mem, ConstExpr.i32(4), new Uint8Array([7]))
  t.is(seg.kind, 'Active')
  t.is(seg.memory()!.index, mem.index)
  t.is(seg.offset()!.kind, 'Value')
  t.is(m.data.length, 3)
  t.deepEqual(seg.value, new Uint8Array([7]))

  const bytesOut = m.emitWasm(false)
  t.true(WebAssembly.validate(bytesOut))
  const reparsed = WasmModule.fromBuffer(bytesOut)
  t.is(reparsed.data.length, 3)
  const rseg = reparsed.data.getByIndex(seg.index)!
  t.is(rseg.kind, 'Active')
  t.deepEqual(rseg.value, new Uint8Array([7]))
})

// id-ref guard: a memory handle from a DIFFERENT module carries a foreign
// MemoryId. Passing it to addActive would let that id reach emit, where walrus
// panics (get_memory_index) and ABORTS the whole Node process. The provenance
// scan turns that into a catchable JS error. This test COMPLETING — the process
// staying alive for the assertions and every later test — is the proof.
test('addActive rejects a memory from another module (throws, never aborts)', (t) => {
  const a = load()
  const b = load()
  const bMem = b.memories.items()[0]

  const err = t.throws(() => a.data.addActive(bMem, ConstExpr.i32(0), new Uint8Array([1])))
  t.regex(err!.message, /not in this module|deleted/i)

  // The rejected add left module A untouched; both modules still emit cleanly.
  t.is(a.data.length, 2)
  t.is(b.data.length, 2)
})

// id-ref guard: an offset ConstExpr that reads a global from ANOTHER module
// carries a foreign GlobalId. validate_const_expr must reject it before emit,
// where walrus would abort (get_global_index) otherwise.
test('addActive rejects an offset that references a foreign global (throws, never aborts)', (t) => {
  const foreign = empty()
  const foreignGlobal = foreign.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(0))

  const a = load()
  const mem = a.memories.items()[0]
  const badOffset = ConstExpr.globalGet(foreignGlobal)

  const err = t.throws(() => a.data.addActive(mem, badOffset, new Uint8Array([1])))
  t.regex(err!.message, /not in this module|deleted/i)

  t.is(a.data.length, 2)
})

test('delete removes a segment and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.data.delete(m.data.items()[1])
  t.is(m.data.length, 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.data.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.data.items()[1]

  m.data.delete(handle)
  t.is(m.data.length, 1)

  const err = t.throws(() => m.data.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.data.items()[0]
  t.throws(() => a.data.delete(bHandle))

  t.is(a.data.length, 2)
  t.is(b.data.length, 2)
})

test('delete-guard: using a handle after delete throws instead of crashing', (t) => {
  const m = load()
  const handle = m.data.items()[1]
  m.data.delete(handle)

  const errValue = t.throws(() => handle.value)
  t.regex(errValue!.message, /deleted/)

  const errKind = t.throws(() => handle.kind)
  t.regex(errKind!.message, /deleted/)

  const errSet = t.throws(() => {
    handle.value = new Uint8Array([0])
  })
  t.regex(errSet!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 1)
})
