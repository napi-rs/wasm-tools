import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/global-types.wat). Reading the
// bytes keeps the test hermetic — wat2wasm is never invoked at runtime.
//   global 0: mutable   i32       -> { type: 'I32' }
//   global 1: immutable i64       -> { type: 'I64' }
//   global 2: immutable f32       -> { type: 'F32' }
//   global 3: immutable f64       -> { type: 'F64' }
//   global 4: immutable externref -> Ref -> Abstract Extern
//   global 5: immutable funcref   -> Ref -> Abstract Func
//   global 6: immutable v128      -> { type: 'V128' }
const FIXTURE = join(__dirname, 'fixtures', 'global-types.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('global.ty maps the scalar value types', (t) => {
  const items = load().globals.items()
  t.is(items.length, 7)

  t.deepEqual(items[0].ty, { type: 'I32' })
  t.deepEqual(items[1].ty, { type: 'I64' })
  t.deepEqual(items[2].ty, { type: 'F32' })
  t.deepEqual(items[3].ty, { type: 'F64' })
  t.deepEqual(items[6].ty, { type: 'V128' })
})

test('global.ty maps reference value types (nullable + abstract heap type)', (t) => {
  const items = load().globals.items()

  t.deepEqual(items[4].ty, {
    type: 'Ref',
    nullable: true,
    heap: { type: 'Abstract', kind: 'Extern' },
  })
  t.deepEqual(items[5].ty, {
    type: 'Ref',
    nullable: true,
    heap: { type: 'Abstract', kind: 'Func' },
  })
})

test('global.kind reports Local for locally defined globals', (t) => {
  const items = load().globals.items()
  for (const g of items) {
    t.is(g.kind, 'Local')
  }
})

test('the .ty getter is guarded by the delete-guard', (t) => {
  const m = load()
  const handle = m.globals.items()[0]
  m.globals.delete(handle)

  const err = t.throws(() => handle.ty)
  t.regex(err!.message, /deleted/)

  // .kind is guarded the same way.
  const err2 = t.throws(() => handle.kind)
  t.regex(err2!.message, /deleted/)
})
