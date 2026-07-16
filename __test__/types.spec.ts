import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ModuleConfig, WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixtures (see fixtures/types.wat, fixtures/types-struct.wat).
// Reading the bytes keeps the tests hermetic — wat2wasm is never invoked at runtime.
//   types.wasm:        type 0 = (func (param i32 f32) (result i64)), referenced by a func
//   types-struct.wasm: type 0 = (struct (field i32)) — a GC composite (non-function) type
const FUNC_FIXTURE = join(__dirname, 'fixtures', 'types.wasm')
const STRUCT_FIXTURE = join(__dirname, 'fixtures', 'types-struct.wasm')
const funcBytes = readFileSync(FUNC_FIXTURE)
const structBytes = readFileSync(STRUCT_FIXTURE)

// A module with one LOCAL function (see fixtures/functions.wat): func $imp is
// imported (type (param i32)), func $loc is local (type (param i32)(result i32)).
// Two REAL function types; walrus additionally mints ONE internal
// "function-entry" type for the local function ((result i32)), which the raw
// arena reports at index 2. WasmTypes must filter that entry type out.
const FUNCTIONS_FIXTURE = join(__dirname, 'fixtures', 'functions.wasm')
const functionsBytes = readFileSync(FUNCTIONS_FIXTURE)

const load = () => WasmModule.fromBuffer(funcBytes)
const loadStruct = () => new ModuleConfig().onlyStableFeatures(false).parse(structBytes)

test('types collection reports length and materializes item handles', (t) => {
  const m = load()
  t.true(m.types.length >= 1)

  const items = m.types.items()
  t.is(items.length, m.types.length)

  const fn = items.find((x) => x.kind === 'Function')
  t.truthy(fn)
  t.is(fn!.kind, 'Function')
  t.deepEqual(fn!.params(), [{ type: 'I32' }, { type: 'F32' }])
  t.deepEqual(fn!.results(), [{ type: 'I64' }])
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const fn = m.types.items().find((x) => x.kind === 'Function')!
  const byIndex = m.types.getByIndex(fn.index)
  t.truthy(byIndex)
  t.is(byIndex!.index, fn.index)
  t.is(byIndex!.kind, 'Function')

  t.is(m.types.getByIndex(9999), null)
})

test('byName returns null for a freshly parsed type (walrus drops WAT names) but finds a named type', (t) => {
  const m = load()
  // walrus' ModuleTypes::by_name always returns None for a newly parsed
  // module (type names are not preserved through parse), so the WAT name $a
  // does not survive.
  t.is(m.types.byName('a'), null)

  // A name set in memory IS findable.
  const added = m.types.add([{ type: 'F64' }], [{ type: 'F64' }])
  added.name = 'my_named_type'
  const found = m.types.byName('my_named_type')
  t.truthy(found)
  t.is(found!.index, added.index)
})

test('add creates a function type whose params/results round-trip', (t) => {
  const m = load()
  const before = m.types.length
  const added = m.types.add([{ type: 'I32' }], [{ type: 'I32' }])
  t.is(added.kind, 'Function')
  t.deepEqual(added.params(), [{ type: 'I32' }])
  t.deepEqual(added.results(), [{ type: 'I32' }])
  t.is(m.types.length, before + 1)
})

test('find returns a handle to an existing type, or null for a miss', (t) => {
  const m = load()
  const added = m.types.add([{ type: 'I32' }], [{ type: 'I32' }])

  const found = m.types.find([{ type: 'I32' }], [{ type: 'I32' }])
  t.truthy(found)
  t.is(found!.index, added.index)

  t.is(m.types.find([{ type: 'F32' }], [{ type: 'V128' }]), null)
})

test('add structurally dedups: re-adding the same signature returns the existing type', (t) => {
  const m = load()
  const first = m.types.add([{ type: 'I64' }], [{ type: 'I64' }])
  const lenAfterFirst = m.types.length

  const second = m.types.add([{ type: 'I64' }], [{ type: 'I64' }])
  // walrus' ArenaSet dedups structurally — the same signature returns the
  // existing type's id and the arena does not grow.
  t.is(m.types.length, lenAfterFirst)
  t.is(second.index, first.index)
})

test('an added function type persists through emit and re-parse', (t) => {
  const m = load()
  m.types.add([{ type: 'V128' }], [{ type: 'V128' }])

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.types.find([{ type: 'V128' }], [{ type: 'V128' }])
  t.truthy(found)
  t.deepEqual(found!.params(), [{ type: 'V128' }])
  t.deepEqual(found!.results(), [{ type: 'V128' }])
})

test('a struct (GC composite) type reports kind Struct and its params/results throw', (t) => {
  const m = loadStruct()
  t.is(m.types.length, 1)

  const s = m.types.items()[0]
  t.is(s.kind, 'Struct')

  // walrus' Type::params/results call unwrap_function() and PANIC on a
  // non-function type; a panic across FFI aborts the process. We guard with
  // as_function() and surface a catchable error instead.
  const errP = t.throws(() => s.params())
  t.regex(errP!.message, /not a function type/)
  const errR = t.throws(() => s.results())
  t.regex(errR!.message, /not a function type/)
})

test('delete removes a type and length drops by one', (t) => {
  const m = load()
  // Add an unused type so deletion cannot dangle any reference.
  const added = m.types.add([{ type: 'F32' }], [{ type: 'F32' }])
  const before = m.types.length

  m.types.delete(added)
  t.is(m.types.length, before - 1)
})

test('delete-guard: a deleted handle throws on params() and .name instead of aborting', (t) => {
  const m = load()
  const added = m.types.add([{ type: 'F32' }], [{ type: 'F32' }])
  m.types.delete(added)

  const errP = t.throws(() => added.params())
  t.regex(errP!.message, /deleted/)
  const errN = t.throws(() => added.name)
  t.regex(errN!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(typeof added.index, 'number')
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const added = m.types.add([{ type: 'F32' }], [{ type: 'F32' }])
  m.types.delete(added)

  const err = t.throws(() => m.types.delete(added))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.types.add([{ type: 'F32' }], [{ type: 'F32' }])
  const aLen = a.types.length
  const bLen = b.types.length

  t.throws(() => a.types.delete(bHandle))
  t.is(a.types.length, aLen)
  t.is(b.types.length, bLen)
})

test('WasmTypes hides walrus internal function-entry types', (t) => {
  const m = WasmModule.fromBuffer(functionsBytes)

  // Two real function types are exposed; the local function's internal entry
  // type (raw arena index 2) is filtered out (the raw arena has 3 types).
  t.is(m.types.length, 2)
  const items = m.types.items()
  t.is(items.length, 2)
  t.is(items.length, m.types.length)

  // Every exposed type is a real, emittable function type: its params/results
  // resolve (an entry type would still resolve, but the point is it is gone).
  for (const it of items) {
    t.is(it.kind, 'Function')
    t.notThrows(() => it.params())
    t.notThrows(() => it.results())
  }
  // The two real signatures are present: (param i32) and (param i32)(result i32).
  t.truthy(m.types.find([{ type: 'I32' }], []))
  t.truthy(m.types.find([{ type: 'I32' }], [{ type: 'I32' }]))
})

test('getByIndex is consistent with items() and hides the entry type index', (t) => {
  const m = WasmModule.fromBuffer(functionsBytes)
  const realIndices = m.types.items().map((it) => it.index)

  // Every exposed type still resolves by its own real index (no renumbering).
  for (const idx of realIndices) {
    t.is(m.types.getByIndex(idx)!.index, idx)
  }

  // The entry type occupies raw arena index 2 but is not among the exposed
  // types, so its index resolves to null.
  t.false(realIndices.includes(2))
  t.is(m.types.getByIndex(2), null)

  // A truly out-of-range index is null too.
  t.is(m.types.getByIndex(9999), null)
})

test('case-1 trap closed: every exposed type can be tagged and emitted (no entry type leaks)', (t) => {
  // The B3f regression this mirrors used to expect the entry type to throw at
  // emit; now the entry type is filtered out of items(), so EVERY add+emit must
  // succeed. The process must also stay alive across all indices (an abort would
  // kill the ava worker).
  const typeCount = WasmModule.fromBuffer(functionsBytes).types.length
  t.true(typeCount >= 1)

  let emittedOk = 0
  const emitErrors: string[] = []
  for (let i = 0; i < typeCount; i++) {
    const m = WasmModule.fromBuffer(functionsBytes)
    m.tags.add(m.types.items()[i])
    try {
      m.emitWasm(false)
      emittedOk++
    } catch (e) {
      emitErrors.push((e as Error).message)
    }
  }

  t.is(emittedOk, typeCount, 'every exposed type emits successfully')
  t.deepEqual(emitErrors, [], 'no exposed type triggers an emit-time error')
})

test('real function types survive emit + re-parse after filtering', (t) => {
  const m = WasmModule.fromBuffer(functionsBytes)
  t.is(m.types.length, 2)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  // The re-parsed module re-mints an entry type for the local function; it is
  // filtered too, so the exposed count is stable, and both real signatures
  // round-trip.
  t.is(reparsed.types.length, 2)
  t.truthy(reparsed.types.find([{ type: 'I32' }], []))
  t.truthy(reparsed.types.find([{ type: 'I32' }], [{ type: 'I32' }]))
})
