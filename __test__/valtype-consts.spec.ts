import test from 'ava'

import {
  ANYREF,
  ARRAYREF,
  ConstExpr,
  EQREF,
  EXNREF,
  EXTERNREF,
  F32,
  F64,
  FUNCREF,
  I31REF,
  I32,
  I64,
  NULLEXNREF,
  NULLEXTERNREF,
  NULLFUNCREF,
  NULLREF,
  STRUCTREF,
  V128,
  WasmModule,
  type ValType,
} from '../index'

// The canonical 8-byte empty module gives each test a clean slate (see
// globals-write.spec.ts): valid wasm header, zero sections, no CLI needed.
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const empty = () => WasmModule.fromBuffer(EMPTY_MODULE)

// Each exported constant deep-equals the hand-written discriminated-union shape
// it replaces, so `import { I32 }` is interchangeable with `{ type: 'I32' }`.
test('numeric value-type constants deep-equal their { type } shapes', (t) => {
  t.deepEqual(I32, { type: 'I32' })
  t.deepEqual(I64, { type: 'I64' })
  t.deepEqual(F32, { type: 'F32' })
  t.deepEqual(F64, { type: 'F64' })
  t.deepEqual(V128, { type: 'V128' })
})

// A constant is usable anywhere a `ValType` is expected: adding a local reads
// the same value type back through the live handle.
test('a value-type constant is accepted where a ValType is expected (locals.add)', (t) => {
  const m = empty()
  const added = m.locals.add(I32)
  t.deepEqual(added.ty, { type: 'I32' })
})

// The same constant drives a global's declared type on the write path.
test('a value-type constant drives a global type (globals.addLocal)', (t) => {
  const m = empty()
  const g = m.globals.addLocal(I64, false, false, ConstExpr.i64(0n))
  t.deepEqual(g.ty, { type: 'I64' })
  t.is(g.mutable, false)

  // Write-through persists: emit -> re-parse -> the global is present.
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.globals.length, 1)
  t.deepEqual(reparsed.globals.items()[0].ty, { type: 'I64' })
})

// Each nullable ref constant maps `import { <NAME>REF }` to the `(ref null <heap>)`
// shorthand: a `{ type: 'Ref', nullable: true, heap: { type: 'Abstract', kind } }`
// value with the matching abstract heap kind.
const refConst = (c: ValType, kind: string) => ({ const: c, kind })
const REF_CONSTS = [
  refConst(FUNCREF, 'Func'),
  refConst(EXTERNREF, 'Extern'),
  refConst(ANYREF, 'Any'),
  refConst(EQREF, 'Eq'),
  refConst(I31REF, 'I31'),
  refConst(STRUCTREF, 'Struct'),
  refConst(ARRAYREF, 'Array'),
  refConst(NULLREF, 'None'),
  refConst(NULLFUNCREF, 'NoFunc'),
  refConst(NULLEXTERNREF, 'NoExtern'),
  refConst(EXNREF, 'Exn'),
  refConst(NULLEXNREF, 'NoExn'),
]

test('nullable ref-type constants deep-equal their { type: Ref, nullable, heap } shapes', (t) => {
  for (const { const: c, kind } of REF_CONSTS) {
    t.deepEqual(c, { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind } })
  }
})

// A ref constant is usable anywhere a `ValType` is expected: adding a local of a
// ref type reads the same ref value type back through the live handle.
test('a ref-type constant is accepted where a ValType is expected (locals.add)', (t) => {
  const m = empty()
  const added = m.locals.add(FUNCREF)
  t.deepEqual(added.ty, { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } })
})
