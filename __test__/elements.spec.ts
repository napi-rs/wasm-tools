import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ConstExpr, WasmFunction, WasmGlobal, WasmModule, WasmTable, WasmType } from '../index'

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

const FUNC_HEAP = { type: 'Abstract', kind: 'Func' } as const

// ---------------------------------------------------------------------------
// addFunctions / addExpressions (F2 API 1) — element-segment creation.
// The base fixture (elements.wasm) has table 0 (4 funcref) and func $f (index 0).
// ---------------------------------------------------------------------------

test('addFunctions(Active) writes through emit + re-parse as an active Functions segment', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  const seg = m.elements.addFunctions('Active', [0], table, ConstExpr.i32(0))
  seg.name = 'added_active'
  t.is(seg.kind, 'Active')
  t.is(seg.itemsKind, 'Functions')
  t.is(m.elements.length, 3)

  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
  const found = reparsed.elements.items().find((e) => e.name === 'added_active')
  t.truthy(found)
  t.is(found!.kind, 'Active')
  t.is(found!.table()!.index, 0)
  t.is(found!.itemsKind, 'Functions')
  t.deepEqual(
    found!.functionItems()!.map((f) => f.index),
    [0],
  )
})

// gc-survival: an ACTIVE segment on a rooted (exported) LOCAL table is reachable
// ONLY through the table's `elem_segments` back-link (walrus' initial roots do
// NOT keep active segments of a non-imported table — used.rs). So this segment
// surviving gc() PROVES addFunctions inserted the back-link; without it the
// segment would be silently dropped by gc.
test('addFunctions(Active) inserts the elem_segments back-link so the segment survives gc()', (t) => {
  const m = loadRooted()
  const table = m.tables.items()[0]
  const seg = m.elements.addFunctions('Active', [0], table, ConstExpr.i32(0))
  seg.name = 'survivor'
  t.is(m.elements.length, 2)

  // Must not abort (rooted table walks elem_segments, which now includes our id).
  m.gc()

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.elements.items().find((e) => e.name === 'survivor')
  t.truthy(found)
  t.is(found!.kind, 'Active')
})

test('addFunctions(Passive) writes through emit + re-parse as a passive Functions segment', (t) => {
  const m = load()
  const seg = m.elements.addFunctions('Passive', [0])
  seg.name = 'added_passive'
  t.is(seg.kind, 'Passive')
  t.is(seg.table(), null)
  t.is(seg.offset(), null)
  t.is(seg.itemsKind, 'Functions')

  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
  const found = reparsed.elements.items().find((e) => e.name === 'added_passive')
  t.truthy(found)
  t.is(found!.kind, 'Passive')
  t.is(found!.itemsKind, 'Functions')
  t.deepEqual(
    found!.functionItems()!.map((f) => f.index),
    [0],
  )
})

test('addExpressions(Passive) writes through emit + re-parse as a passive Expressions segment', (t) => {
  const m = load()
  const func = m.functions.items()[0]
  const seg = m.elements.addExpressions('Passive', FUNCREF, [ConstExpr.refFunc(func), ConstExpr.refNull(FUNC_HEAP)])
  seg.name = 'passive_exprs'
  t.is(seg.kind, 'Passive')
  t.is(seg.itemsKind, 'Expressions')
  t.deepEqual(seg.expressionElementType(), FUNCREF)
  t.is(seg.expressionItems()!.length, 2)

  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
  const found = reparsed.elements.items().find((e) => e.name === 'passive_exprs')
  t.truthy(found)
  t.is(found!.itemsKind, 'Expressions')
  t.deepEqual(found!.expressionElementType(), FUNCREF)
  t.deepEqual(
    found!.expressionItems()!.map((e) => e.kind),
    ['RefFunc', 'RefNull'],
  )
})

test('addFunctions(Active) without a table/offset throws catchably and leaves the module unchanged', (t) => {
  const m = load()
  const err = t.throws(() => m.elements.addFunctions('Active', [0]))
  t.regex(err!.message, /active element segment requires a table and offset/)
  t.is(m.elements.length, 2)
})

test('addFunctions(Passive) WITH a table/offset throws catchably (no silent corruption) and leaves the module unchanged', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  const err = t.throws(() => m.elements.addFunctions('Passive', [0], table, ConstExpr.i32(0)))
  t.regex(err!.message, /only valid for an active element segment/)
  t.is(m.elements.length, 2)
})

test('addFunctions rejects a funcIndex naming no live function and leaves the module unchanged', (t) => {
  const m = load()
  const err = t.throws(() => m.elements.addFunctions('Passive', [999]))
  t.regex(err!.message, /no function at index 999/)
  t.is(m.elements.length, 2)
})

test('addExpressions rejects a non-reference elementTy and leaves the module unchanged', (t) => {
  const m = load()
  const err = t.throws(() => m.elements.addExpressions('Passive', { type: 'I32' }, []))
  t.regex(err!.message, /reference type/)
  t.is(m.elements.length, 2)
})

// Abort-safety (#4 parity): a hostile sparse funcIndices length must fail
// CATCHABLY at decode (SafeVec grows non-preallocating), never pre-allocate
// ~2**32 slots from the untrusted JS `.length` and abort the process.
function sparseHuge(): never[] {
  const a: unknown[] = []
  a.length = 2 ** 32 - 1
  return a as never[]
}

test('addFunctions rejects a huge sparse funcIndices array instead of aborting', (t) => {
  const m = load()
  t.throws(() => m.elements.addFunctions('Passive', sparseHuge()))
  t.is(m.elements.length, 2)
})

test('addExpressions rejects a huge sparse exprs array instead of aborting', (t) => {
  const m = load()
  // Same abort-safety guard for the ConstExpr list: SafeVec grows
  // non-preallocating, so the untrusted ~2**32 `.length` never reaches a
  // `with_capacity`; the first sparse hole fails catchably at decode.
  t.throws(() => m.elements.addExpressions('Passive', FUNCREF, sparseHuge()))
  t.is(m.elements.length, 2)
})

// ---------------------------------------------------------------------------
// ConstExpr.refFunc (F2 API 2) — provenance validated at the CONSUME site.
// ---------------------------------------------------------------------------

test('ConstExpr.refFunc builds a RefFunc const expr', (t) => {
  const m = load()
  const func = m.functions.items()[0]
  t.is(ConstExpr.refFunc(func).kind, 'RefFunc')
})

test('a refFunc off a DELETED function is rejected catchably at the consume site (addExpressions)', (t) => {
  const m = load()
  const func = m.functions.items()[0]
  const rf = ConstExpr.refFunc(func)
  // Delete the function so the RefFunc now carries a dead FunctionId. (The
  // module is not emitted here — only the consume-site guard is exercised.)
  m.functions.delete(func)

  const err = t.throws(() => m.elements.addExpressions('Passive', FUNCREF, [rf]))
  t.regex(err!.message, /function that is not in this module/)
  t.is(m.elements.length, 2)
})

test('addExpressions rejects a non-ConstExpr array element instead of type-confusing it', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  // A WasmTable is a different #[napi] class; the instanceof guard must reject it.
  t.throws(() => m.elements.addExpressions('Passive', FUNCREF, [table as unknown as ConstExpr]))
  t.is(m.elements.length, 2)
})

// ---------------------------------------------------------------------------
// F1 (whole-binding hardening): every SCALAR handle-ref param (`&Wasm*` /
// `&ConstExpr`) is `#[napi(strict)]`, so the generated instanceof runs BEFORE
// the type-blind `napi_unwrap` + pointer cast. Passing a DIFFERENT wrapped
// `#[napi]` class — which `napi_unwrap` would reinterpret as the wrong Rust
// type (UB), unlike a plain object which fails catchably in `napi_unwrap` — must
// now throw a CATCHABLE error. A representative spread of handle classes at
// scalar positions; ava reaching the next test is itself proof the throw was
// catchable, not a process abort. (The list-element `&ConstExpr` case is
// covered by the addExpressions non-ConstExpr test above.)
// ---------------------------------------------------------------------------

test('F1: scalar &WasmTable param rejects a wrong wrapped class catchably (exports.addTable)', (t) => {
  const m = load()
  const notATable = m.functions.items()[0] // a WasmFunction, a different wrapped class
  t.throws(() => m.exports.addTable('x', notATable as unknown as WasmTable))
})

test('F1: scalar &WasmFunction param rejects a wrong wrapped class catchably (ConstExpr.refFunc)', (t) => {
  const m = load()
  const notAFunction = m.tables.items()[0] // a WasmTable
  t.throws(() => ConstExpr.refFunc(notAFunction as unknown as WasmFunction))
})

test('F1: scalar &WasmGlobal param rejects a wrong wrapped class catchably (ConstExpr.globalGet)', (t) => {
  const m = load()
  const notAGlobal = m.tables.items()[0] // a WasmTable
  t.throws(() => ConstExpr.globalGet(notAGlobal as unknown as WasmGlobal))
})

test('F1: scalar &WasmType param rejects a wrong wrapped class catchably (tags.add)', (t) => {
  const m = load()
  const notAType = m.functions.items()[0] // a WasmFunction
  t.throws(() => m.tags.add(notAType as unknown as WasmType))
})

test('F1: scalar &ConstExpr param rejects a wrong wrapped class catchably (addFunctions offset)', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  // `offset` expects a ConstExpr; a WasmTable is a different wrapped class. It is
  // rejected at decode (strict validate), so the module is left untouched.
  t.throws(() => m.elements.addFunctions('Active', [0], table, table as unknown as ConstExpr))
  t.is(m.elements.length, 2)
})

// ---------------------------------------------------------------------------
// F-fix5 (1-C, prototype-spoof): strict `instanceof` alone CANNOT close 1-C — a
// REAL wrapped instance of the WRONG class, re-parented with
// `Object.setPrototypeOf(x, RightClass.prototype)`, passes `instanceof
// RightClass` while its wrapped pointer is still the wrong Rust type. Adopting
// napi >= 3.11 with the `napi8` feature activates Node-API object type tags: the
// derive stamps each instance at wrap time and checks the (unforgeable, JS-
// invisible) tag before the `&T` cast, so the spoofed object is rejected AFTER
// `instanceof` passes. This is native-only: on WASI the tag is a no-op (a
// static's address is not a process-global identity there), so the spoof would
// still blind-cast — 1-C stays open on wasm — and this test is skipped under
// `NAPI_RS_FORCE_WASI`. ava reaching the next test proves the throw was catchable.
// ---------------------------------------------------------------------------
const nativeOnly = process.env.NAPI_RS_FORCE_WASI ? test.skip : test
nativeOnly(
  'F-fix5 (1-C): a setPrototypeOf-spoofed WasmTable that passes `instanceof WasmFunction` is still rejected by the type tag (native)',
  (t) => {
    const m = load()
    const table = m.tables.items()[0] // a REAL WasmTable handle (wrong class)
    // Re-parent so the prototype-based `instanceof` check is fully defeated.
    Object.setPrototypeOf(table, WasmFunction.prototype)
    t.true(table instanceof WasmFunction) // the spoof succeeds: instanceof now lies
    // `ConstExpr.refFunc` wants `&WasmFunction`; the unforgeable type tag catches
    // the still-WasmTable pointer that `instanceof` waved through.
    t.throws(() => ConstExpr.refFunc(table as unknown as WasmFunction))
  },
)

// ---------------------------------------------------------------------------
// F2 (element re-entrancy): the item list decodes BEFORE the `table`/`offset`
// handle refs, so no borrowed `&WasmTable` is held live while `SafeVec`'s
// per-element getter runs arbitrary JS. A getter that synchronously calls a
// `&mut` setter on the same table (`table.name = ...`) therefore cannot form a
// `&`/`&mut` alias. The call must complete (or throw) WITHOUT aborting the
// worker — this test finishing, and later tests running, is the proof.
// ---------------------------------------------------------------------------

test('F2: a re-entrant funcIndices getter mutating the table during decode does not abort', (t) => {
  const m = load()
  const table = m.tables.items()[0]
  const offset = ConstExpr.i32(0)

  // A real array (Array.isArray stays true, passing SafeVec's validate) with an
  // integer-indexed accessor at [0] that re-enters a &mut setter on the SAME
  // table handle mid-decode.
  const arr: number[] = []
  Object.defineProperty(arr, 0, {
    enumerable: true,
    configurable: true,
    get() {
      table.name = 'reentrant'
      return 0
    },
  })
  t.is(arr.length, 1) // defining index 0 bumped the array length

  const seg = m.elements.addFunctions('Active', arr, table, offset)
  t.is(seg.kind, 'Active')
  t.is(seg.itemsKind, 'Functions')
  // The getter's &mut side effect landed and the process is intact.
  t.is(table.name, 'reentrant')
})

// ---------------------------------------------------------------------------
// F3 (ConstExprArg Extended deep-clone): `Extended` is the sole heap-owning
// ConstExpr variant; `ConstExprArg::from_napi_value` now rejects it BEFORE the
// infallible `handle.inner.clone()`, closing the unbounded-deep-clone abort. It
// is invalid at this consume site anyway (`validate_const_expr` rejects it), so
// the early reject is behavior-preserving and CATCHABLE.
// ---------------------------------------------------------------------------

const EXTENDED_FIXTURE = join(__dirname, 'fixtures', 'extended-const.wasm')
const extendedBytes = readFileSync(EXTENDED_FIXTURE)

test('F3: an Extended const expr handle is rejected catchably in addExpressions (no clone abort)', (t) => {
  const m = load()
  // A genuine Extended ConstExpr: the fixture's exported global has a multi-op
  // (array.new_fixed) initializer, which walrus parses to ConstExpr::Extended.
  const src = WasmModule.fromBuffer(extendedBytes)
  const extended = src.globals.items()[0].init()!
  t.is(extended.kind, 'Extended') // it really is the heap-owning variant

  const err = t.throws(() => m.elements.addExpressions('Passive', FUNCREF, [extended]))
  t.regex(err!.message, /extended const expression/)
  t.is(m.elements.length, 2)
})
