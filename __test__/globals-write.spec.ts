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

test('addLocal creates an externref global (ref type + refNull init) and round-trips', (t) => {
  const m = empty()

  const g = m.globals.addLocal(EXTERNREF, false, false, ConstExpr.refNull(true, EXTERN_HEAP))
  t.is(m.globals.length, 1)
  t.deepEqual(g.ty, EXTERNREF)
  t.is(g.mutable, false)
  t.is(g.init()!.kind, 'RefNull')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
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
  t.is(ConstExpr.refNull(true, { type: 'Abstract', kind: 'Func' }).kind, 'RefNull')

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
