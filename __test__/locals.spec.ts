import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Shared fixture with functions (see fixtures/functions.wat). The local function
// $loc declares a local `i64` in its body; walrus records every local (and
// parameter) in the MODULE-WIDE locals arena, so it surfaces here.
const FIXTURE = join(__dirname, 'fixtures', 'functions.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('locals collection materializes item handles for the module-wide locals', (t) => {
  const m = load()
  const items = m.locals.items()
  t.is(items.length, m.locals.length)

  // Every handle exposes a stable numeric index and a readable value type.
  for (const l of items) {
    t.is(typeof l.index, 'number')
    t.is(typeof l.ty.type, 'string')
  }
})

test('the fixture body declares an i64 local, exposed with its value type', (t) => {
  const m = load()
  const i64Local = m.locals.items().find((l) => l.ty.type === 'I64')
  t.truthy(i64Local)
  t.deepEqual(i64Local!.ty, { type: 'I64' })
})

test('add creates a local of the given type and it appears in items()', (t) => {
  const m = load()
  const before = m.locals.length

  const added = m.locals.add({ type: 'I64' })
  t.deepEqual(added.ty, { type: 'I64' })
  t.is(m.locals.length, before + 1)

  // The freshly added local is present in the collection at its stable index.
  const found = m.locals.getByIndex(added.index)
  t.truthy(found)
  t.deepEqual(found!.ty, { type: 'I64' })
  t.true(m.locals.items().some((l) => l.index === added.index))
})

test('add accepts a reference value type', (t) => {
  const m = load()
  const funcref = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } } as const
  const added = m.locals.add(funcref)
  t.deepEqual(added.ty, funcref)
})

test('getByIndex returns null for a miss', (t) => {
  const m = load()
  t.is(m.locals.getByIndex(9999), null)
})

test('write-through: a local name can be set and read back', (t) => {
  const m = load()
  const added = m.locals.add({ type: 'I32' })
  t.is(added.name, null)
  added.name = 'my_local'
  t.is(added.name, 'my_local')
})

test('an added local is independent per module (ids are module-scoped)', (t) => {
  const a = load()
  const b = load()

  const beforeA = a.locals.length
  const beforeB = b.locals.length

  // Adding to A must not affect B — the arenas are separate.
  a.locals.add({ type: 'F64' })
  t.is(a.locals.length, beforeA + 1)
  t.is(b.locals.length, beforeB)
})
