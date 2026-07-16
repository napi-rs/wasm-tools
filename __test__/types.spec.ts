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

// An empty (8-byte) module we can build GC types into from scratch, hermetically.
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const emptyModule = () => WasmModule.fromBuffer(EMPTY_MODULE)
// GC composite types are non-stable, so re-parsing emitted GC bytes needs the
// non-stable-features gate opened (same as loadStruct above).
const reparseGc = (bytes: Uint8Array) => new ModuleConfig().onlyStableFeatures(false).parse(bytes)

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

// ---------------------------------------------------------------------------
// GC struct / array composite types (B5a)
// ---------------------------------------------------------------------------

test('addStruct creates a struct whose fields (incl. packed i8) round-trip', (t) => {
  const m = emptyModule()
  const s = m.types.addStruct([
    { storage: { type: 'Val', value: { type: 'I32' } }, mutable: true },
    { storage: { type: 'I8' }, mutable: false },
  ])

  t.is(s.kind, 'Struct')
  t.is(s.isFinal, true)
  t.is(s.supertype, null)
  t.deepEqual(s.structFields(), [
    { storage: { type: 'Val', value: { type: 'I32' } }, mutable: true },
    { storage: { type: 'I8' }, mutable: false },
  ])

  // Wrong-kind accessor throws catchably (never a walrus unwrap panic).
  const err = t.throws(() => s.arrayElement())
  t.regex(err!.message, /not an array type/)

  // Emit -> re-parse -> the struct (packed field included) survives.
  const reparsed = reparseGc(m.emitWasm(false))
  const rs = reparsed.types.items().find((x) => x.kind === 'Struct')
  t.truthy(rs)
  t.deepEqual(rs!.structFields(), [
    { storage: { type: 'Val', value: { type: 'I32' } }, mutable: true },
    { storage: { type: 'I8' }, mutable: false },
  ])
})

test('addArray creates an array whose element type round-trips', (t) => {
  const m = emptyModule()
  const a = m.types.addArray({ storage: { type: 'Val', value: { type: 'F64' } }, mutable: true })

  t.is(a.kind, 'Array')
  t.is(a.isFinal, true)
  t.is(a.supertype, null)
  t.deepEqual(a.arrayElement(), { storage: { type: 'Val', value: { type: 'F64' } }, mutable: true })

  const err = t.throws(() => a.structFields())
  t.regex(err!.message, /not a struct type/)

  const reparsed = reparseGc(m.emitWasm(false))
  const ra = reparsed.types.items().find((x) => x.kind === 'Array')
  t.truthy(ra)
  t.deepEqual(ra!.arrayElement(), { storage: { type: 'Val', value: { type: 'F64' } }, mutable: true })
})

test('a struct field can be a concrete ref to another type (the crux) and round-trips', (t) => {
  const m = emptyModule()
  // Struct A: a plain primitive struct.
  const a = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  // Struct B: a field that is `(ref null $a)` — a concrete ref to A.
  const b = m.types.addStruct([
    {
      storage: { type: 'Val', value: { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: a.index } } },
      mutable: false,
    },
  ])

  // In memory, B's field reads back as a concrete ref that targets A's index.
  t.deepEqual(b.structFields(), [
    {
      storage: { type: 'Val', value: { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: a.index } } },
      mutable: false,
    },
  ])

  // Round-trip: emit (which resolves the concrete ref through get_type_index —
  // the abort we guard against) then re-parse. Emit may reorder types, so we
  // identify B by shape (its field is a concrete ref) and verify the ref still
  // targets A's signature (a struct with a single mutable i32 field).
  const reparsed = reparseGc(m.emitWasm(false))
  const structs = reparsed.types.items().filter((x) => x.kind === 'Struct')
  const rb = structs.find((x) => {
    const f = x.structFields()[0]
    return f?.storage.type === 'Val' && f.storage.value.type === 'Ref' && f.storage.value.heap.type === 'Concrete'
  })
  t.truthy(rb)
  const field = rb!.structFields()[0]
  // Narrow for TS then read the concrete target index.
  if (field.storage.type !== 'Val' || field.storage.value.type !== 'Ref' || field.storage.value.heap.type !== 'Concrete') {
    return t.fail('expected a concrete ref field')
  }
  const target = reparsed.types.getByIndex(field.storage.value.heap.typeIndex)
  t.truthy(target)
  t.is(target!.kind, 'Struct')
  t.deepEqual(target!.structFields(), [{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
})

test('concrete-ref resolution guard: a field referencing a nonexistent type index throws (no abort)', (t) => {
  const m = emptyModule()
  const err = t.throws(() =>
    m.types.addStruct([
      {
        storage: { type: 'Val', value: { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: 9999 } } },
        mutable: false,
      },
    ]),
  )
  t.regex(err!.message, /no type at index 9999/)

  // The rejection happens BEFORE any arena mutation, and the process is alive:
  // the module is still fully usable.
  t.is(m.types.length, 0)
  t.notThrows(() => m.types.addStruct([{ storage: { type: 'I16' }, mutable: true }]))
  t.is(m.types.length, 1)
})

test('concrete-ref to an internal entry-type index throws catchably (WASI-safe, no emit abort)', (t) => {
  // functions.wasm raw arena: [0]=func type, [1]=func type, [2]=internal
  // function-entry type minted for the LOCAL function. items() hides the entry
  // type, so it is a real arena slot that resolves to null via getByIndex — the
  // exact footgun a concrete ref could otherwise wire to an unemittable type.
  const m = WasmModule.fromBuffer(functionsBytes)
  const realIndices = m.types.items().map((it) => it.index)
  const maxReal = Math.max(...realIndices)

  // The entry type sits in the contiguous arena at the first slot items() hides.
  // Bound the scan to the arena (real-max + the single local-fn entry type) so
  // we pick a REAL hidden slot, never a genuinely out-of-range index.
  let entryIndex = -1
  for (let i = 0; i <= maxReal + 1; i++) {
    if (m.types.getByIndex(i) === null) {
      entryIndex = i
      break
    }
  }
  t.true(entryIndex >= 0, 'found the hidden entry-type arena slot')
  t.false(realIndices.includes(entryIndex))

  // A concrete ref to that entry-type index must be REJECTED at creation with a
  // catchable error — identical to a nonexistent index — never resolved to an
  // unvalidated entry TypeId that aborts the process at emit (on WASI the emit
  // catch_unwind backstop is a no-op, so the filter is what keeps us alive).
  const err = t.throws(() =>
    m.types.addStruct([
      {
        storage: { type: 'Val', value: { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: entryIndex } } },
        mutable: false,
      },
    ]),
  )
  t.regex(err!.message, new RegExp(`no type at index ${entryIndex} in this module`))

  // The process is alive and the module is untouched + still usable.
  t.notThrows(() => m.types.items())
  t.notThrows(() => m.types.addStruct([{ storage: { type: 'I16' }, mutable: true }]))
})

test('addStruct structurally dedups (mirror walrus): an identical struct returns the same index', (t) => {
  const m = emptyModule()
  const first = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  const lenAfterFirst = m.types.length

  const second = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  // walrus' ArenaSet dedups structurally identical composite types — the arena
  // does not grow and the same id comes back. This mirrors walrus and is
  // intended behavior (documented, not fought).
  t.is(m.types.length, lenAfterFirst)
  t.is(second.index, first.index)

  // An array dedups the same way.
  const arr1 = m.types.addArray({ storage: { type: 'Val', value: { type: 'F32' } }, mutable: false })
  const lenAfterArr = m.types.length
  const arr2 = m.types.addArray({ storage: { type: 'Val', value: { type: 'F32' } }, mutable: false })
  t.is(m.types.length, lenAfterArr)
  t.is(arr2.index, arr1.index)
})

test('supertype/isFinal read on a freshly parsed final struct fixture', (t) => {
  const m = loadStruct()
  const s = m.types.items()[0]
  t.is(s.kind, 'Struct')
  t.is(s.supertype, null)
  t.is(s.isFinal, true)
})

test('delete-guard: a deleted struct handle throws on structFields/isFinal/supertype', (t) => {
  const m = emptyModule()
  const s = m.types.addStruct([{ storage: { type: 'I8' }, mutable: false }])
  m.types.delete(s)

  t.regex(t.throws(() => s.structFields())!.message, /deleted/)
  t.regex(t.throws(() => s.isFinal)!.message, /deleted/)
  t.regex(t.throws(() => s.supertype)!.message, /deleted/)
})
