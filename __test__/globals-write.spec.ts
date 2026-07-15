import test from 'ava'

import { ConstExpr, WasmModule } from '../index'

// The canonical 8-byte empty module: valid wasm header, zero sections. walrus
// parses it, and it carries no globals/custom sections, giving each test a
// clean slate. Building fresh instances keeps the suite hermetic (no CLI).
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const empty = () => WasmModule.fromBuffer(EMPTY_MODULE)

const EXTERN_HEAP = { type: 'Abstract', kind: 'Extern' } as const
const EXTERNREF = { type: 'Ref', nullable: true, heap: EXTERN_HEAP } as const

test('addLocal creates an i32 global whose fields read back, and it round-trips', (t) => {
  const m = empty()
  t.is(m.globals.length, 0)

  const g = m.globals.addLocal({ type: 'I32' }, true, false, ConstExpr.i32(42))

  // The returned handle is live and reads through to the new global.
  t.is(m.globals.length, 1)
  t.deepEqual(g.ty, { type: 'I32' })
  t.is(g.mutable, true)
  t.is(g.kind, 'Local')
  t.is(g.init()!.kind, 'Value')

  // Write-through persists: emit -> re-parse -> the extra global is present.
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.globals.length, 1)
  const rg = reparsed.globals.getByIndex(0)!
  t.deepEqual(rg.ty, { type: 'I32' })
  t.is(rg.mutable, true)
})

test('addLocal creates a nullable externref global (ref type + refNull init), emits valid wasm, and round-trips', (t) => {
  const m = empty()

  // refNull is ALWAYS nullable (single-arg), so it initializes a NULLABLE
  // externref global. A non-nullable null would be invalid wasm.
  const g = m.globals.addLocal(EXTERNREF, false, false, ConstExpr.refNull(EXTERN_HEAP))
  t.is(m.globals.length, 1)
  t.deepEqual(g.ty, EXTERNREF)
  t.is(g.mutable, false)
  t.is(g.init()!.kind, 'RefNull')

  const bytes = m.emitWasm(false)
  // Independent validity check: the emitted module must actually validate.
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
  t.is(reparsed.globals.length, 1)
  t.deepEqual(reparsed.globals.getByIndex(0)!.ty, EXTERNREF)
})

test('ConstExpr.v128 rejects the wrong byte length (catchable, never aborts)', (t) => {
  const err = t.throws(() => ConstExpr.v128(new Uint8Array([1, 2, 3])))
  t.regex(err!.message, /16 bytes/)
})

test('ConstExpr.v128 accepts exactly 16 bytes', (t) => {
  t.is(ConstExpr.v128(new Uint8Array(16)).kind, 'Value')
})

test('addLocal rejects a concrete-heap ref type (needs a type handle, deferred to GC task)', (t) => {
  const m = empty()
  const concreteRef = { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: 0 } } as const

  const err = t.throws(() => m.globals.addLocal(concreteRef, false, false, ConstExpr.i32(0)))
  t.regex(err!.message, /concrete|type handle|not yet supported/i)

  // The rejected add left the module untouched.
  t.is(m.globals.length, 0)
})

test('ConstExpr factory kinds map to the walrus discriminants', (t) => {
  t.is(ConstExpr.i32(1).kind, 'Value')
  t.is(ConstExpr.i64(1n).kind, 'Value')
  t.is(ConstExpr.f32(1.5).kind, 'Value')
  t.is(ConstExpr.f64(1.5).kind, 'Value')
  t.is(ConstExpr.refNull({ type: 'Abstract', kind: 'Func' }).kind, 'RefNull')

  const m = empty()
  const existing = m.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(7))
  t.is(ConstExpr.globalGet(existing).kind, 'Global')
})

test('init() returns the local initializer as a ConstExpr wrapper', (t) => {
  const m = empty()
  const g = m.globals.addLocal({ type: 'F64' }, false, false, ConstExpr.f64(3.5))
  const init = g.init()
  t.truthy(init)
  t.is(init!.kind, 'Value')
})

// Regression: a ConstExpr that reads a global from ANOTHER module must be
// rejected at addLocal time with a CATCHABLE error. Before the fix, the
// foreign id survived into emit, where walrus panicked in get_global_index and
// ABORTED the whole Node process (uncatchable). This test simply COMPLETING —
// the process staying alive to run the assertion and every later test — is the
// proof that the abort is gone.
test('addLocal rejects a globalGet ConstExpr from another module (throws, never aborts)', (t) => {
  const moduleB = empty()
  const bGlobal = moduleB.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(1))

  const moduleA = empty()
  const ce = ConstExpr.globalGet(bGlobal)

  const err = t.throws(() => moduleA.globals.addLocal({ type: 'I32' }, true, false, ce))
  t.regex(err!.message, /not in this module|deleted/i)

  // The rejected add left module A untouched, and the process is still alive.
  t.is(moduleA.globals.length, 0)
})

// Regression: same guard also catches an already-DELETED global's id. After
// delete, the id has no index in the module, so emit would abort; addLocal must
// reject it up front instead.
test('addLocal rejects a globalGet ConstExpr referencing an already-deleted global (throws, never aborts)', (t) => {
  const m = empty()
  const g1 = m.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(1))
  m.globals.delete(g1)

  const ce = ConstExpr.globalGet(g1)
  const err = t.throws(() => m.globals.addLocal({ type: 'I32' }, false, false, ce))
  t.regex(err!.message, /not in this module|deleted/i)

  t.is(m.globals.length, 0)
})
