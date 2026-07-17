import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/elements.wat). Reading the bytes
// keeps the test hermetic — wat2wasm is never invoked at runtime.
//   table 0: 4 funcref
//   func $f (index 0)
//   element[0]: ACTIVE  table 0, offset (i32.const 0), items Functions [$f]
//   element[1]: PASSIVE items Expressions funcref [(ref.func $f), (ref.null func)]
const FIXTURE = join(__dirname, 'fixtures', 'elements.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

// A rooted variant whose table is EXPORTED, so gc() treats it as a root and
// traverses its `elem_segments` back-link set (the path that aborts on a stale
// back-link). See fixtures/elements-rooted.wat.
//   table 0: 4 funcref, EXPORTED as "tbl"
//   element[0]: ACTIVE table 0, offset (i32.const 0), items Functions [$f]
const ROOTED_FIXTURE = join(__dirname, 'fixtures', 'elements-rooted.wasm')
const rootedBytes = readFileSync(ROOTED_FIXTURE)

const loadRooted = () => WasmModule.fromBuffer(rootedBytes)

const FUNCREF = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } } as const

test('elements collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.elements.length, 2)

  const items = m.elements.items()
  t.is(items.length, 2)
  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('kind distinguishes active from passive segments', (t) => {
  const m = load()
  const [active, passive] = m.elements.items()
  t.is(active.kind, 'Active')
  t.is(passive.kind, 'Passive')
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const seg = m.elements.getByIndex(1)
  t.truthy(seg)
  t.is(seg!.index, 1)
  t.is(seg!.kind, 'Passive')
  t.is(m.elements.getByIndex(99), null)
})

test('getByIndex rejects a non-u32-integer index instead of silently coercing/aliasing', (t) => {
  const m = load()
  // 2**32 -> 0, -1 -> u32::MAX, NaN -> 0, 1.5 truncates under the old ToUint32
  // decode; each must now throw catchably rather than aliasing a wrong segment.
  for (const bad of [2 ** 32, -1, 1.5, NaN]) {
    t.throws(() => m.elements.getByIndex(bad), { message: /index must be an integer in 0\.\.=4294967295/ })
  }
  t.is(m.elements.getByIndex(1)!.index, 1) // happy path intact
})

test('active segment exposes its table and offset; passive returns null for both', (t) => {
  const m = load()
  const [active, passive] = m.elements.items()

  const table = active.table()
  t.truthy(table)
  t.is(table!.index, 0)

  const off = active.offset()
  t.truthy(off)
  t.is(off!.kind, 'Value')

  t.is(passive.table(), null)
  t.is(passive.offset(), null)
})

test('a Functions element exposes its referenced functions and null expression accessors', (t) => {
  const m = load()
  const active = m.elements.items()[0]

  t.is(active.itemsKind, 'Functions')

  const funcs = active.functionItems()
  t.truthy(funcs)
  t.is(funcs!.length, 1)
  // The active segment references $f, which is function index 0.
  t.deepEqual(
    funcs!.map((f) => f.index),
    [0],
  )

  t.is(active.expressionElementType(), null)
  t.is(active.expressionItems(), null)
})

test('an Expressions element exposes its element type and const-expr items and null function accessor', (t) => {
  const m = load()
  const passive = m.elements.items()[1]

  t.is(passive.itemsKind, 'Expressions')

  const elemType = passive.expressionElementType()
  t.deepEqual(elemType, FUNCREF)

  const exprs = passive.expressionItems()
  t.truthy(exprs)
  t.is(exprs!.length, 2)
  // The passive segment holds (ref.func $f) then (ref.null func).
  t.deepEqual(
    exprs!.map((e) => e.kind),
    ['RefFunc', 'RefNull'],
  )

  t.is(passive.functionItems(), null)
})

test('name get/set round-trips through emit and re-parse', (t) => {
  const m = load()
  const seg = m.elements.items()[0]
  t.is(seg.name, null)
  seg.name = 'renamed'
  t.is(seg.name, 'renamed')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.elements.items().find((e) => e.name === 'renamed')
  t.truthy(found)
  t.is(found!.index, 0)
})

test('delete removes a segment and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.elements.delete(m.elements.items()[1])
  t.is(m.elements.length, 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.elements.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.elements.items()[1]

  m.elements.delete(handle)
  t.is(m.elements.length, 1)

  const err = t.throws(() => m.elements.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.elements.items()[0]
  t.throws(() => a.elements.delete(bHandle))

  t.is(a.elements.length, 2)
  t.is(b.elements.length, 2)
})

test('delete-guard: using a handle after delete throws instead of crashing', (t) => {
  const m = load()
  const handle = m.elements.items()[1]
  m.elements.delete(handle)

  const errKind = t.throws(() => handle.kind)
  t.regex(errKind!.message, /deleted/)

  const errName = t.throws(() => handle.name)
  t.regex(errName!.message, /deleted/)

  const errSet = t.throws(() => {
    handle.name = 'x'
  })
  t.regex(errSet!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 1)
})

// gc-abort regression: deleting an ACTIVE element used to leave a stale id in
// its owning table's `elem_segments` back-link set. A later gc() on the ROOTED
// (exported) table then called walrus' `elements.get(tombstonedId)`, which
// panics across FFI and ABORTS the whole Node process (SIGABRT, exit 134). This
// test COMPLETING — the process staying alive through gc, emit, and re-parse,
// and every later test still running — is the proof the back-link is cleaned up.
test('delete of an active element then gc() does not abort on a rooted table', (t) => {
  const m = loadRooted()
  const active = m.elements.items()[0]
  t.is(active.kind, 'Active')

  m.elements.delete(active)
  t.is(m.elements.length, 0)

  // Before the fix this aborts the worker instead of returning.
  m.gc()

  // The module is still coherent: it emits and re-parses with no elements.
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.elements.length, 0)
})
