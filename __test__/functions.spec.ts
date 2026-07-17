import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/functions.wat, built with
// `wat2wasm --debug-names`). Reading the bytes keeps the test hermetic —
// wat2wasm is never invoked at runtime.
//   func 0 ($imp): imported, type (param i32)            -> kind Import
//   func 1 ($loc): local,    type (param i32) (result i32) -> kind Local
const FIXTURE = join(__dirname, 'fixtures', 'functions.wasm')
const fixtureBytes = readFileSync(FIXTURE)

// Two LOCAL functions with distinct function types but the SAME result
// signature (see fixtures/functions-shared-entry.wat). walrus mints one
// internal function-entry type from the result signature and dedups it, so both
// functions SHARE that single entry type.
const SHARED_ENTRY_FIXTURE = join(__dirname, 'fixtures', 'functions-shared-entry.wasm')
const sharedEntryBytes = readFileSync(SHARED_ENTRY_FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('functions collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.functions.length, 2)

  const items = m.functions.items()
  t.is(items.length, 2)
  t.is(items.length, m.functions.length)

  // Every handle exposes a stable, numeric index.
  for (const f of items) {
    t.is(typeof f.index, 'number')
  }
})

test('kind distinguishes imported from locally defined functions', (t) => {
  const m = load()
  const imp = m.functions.getByIndex(0)!
  const loc = m.functions.getByIndex(1)!

  t.is(imp.kind, 'Import')
  t.is(loc.kind, 'Local')
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const loc = m.functions.getByIndex(1)
  t.truthy(loc)
  t.is(loc!.index, 1)
  t.is(loc!.kind, 'Local')

  t.is(m.functions.getByIndex(9999), null)
})

test('getByIndex rejects a non-u32-integer index instead of silently coercing/aliasing', (t) => {
  const m = load()
  // Before hardening these were decoded via ToUint32: 2**32 -> 0, -1 -> u32::MAX,
  // NaN -> 0, a fraction truncated — each SILENTLY aliasing a wrong (often valid)
  // index. They must now throw a catchable error, never return an item.
  for (const bad of [2 ** 32, -1, 1.5, NaN]) {
    t.throws(() => m.functions.getByIndex(bad), { message: /index must be an integer in 0\.\.=4294967295/ })
  }
  // The happy path (a real, in-range integer) is unaffected.
  t.is(m.functions.getByIndex(1)!.index, 1)
  t.is(1 + 1, 2) // process still alive after the catchable throws
})

test('byName finds a named function and returns null for a miss', (t) => {
  const m = load()
  // Names survive because the fixture was built with --debug-names.
  const loc = m.functions.byName('loc')
  t.truthy(loc)
  t.is(loc!.kind, 'Local')
  t.is(loc!.index, 1)

  const imp = m.functions.byName('imp')
  t.truthy(imp)
  t.is(imp!.kind, 'Import')

  t.is(m.functions.byName('nope'), null)
})

test('ty() returns a WasmType handle whose signature matches the function', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!

  // Cross-link into the types collection: the local function is
  // (param i32) (result i32).
  const ty = loc.ty()
  t.is(ty.kind, 'Function')
  t.deepEqual(ty.params(), [{ type: 'I32' }])
  t.deepEqual(ty.results(), [{ type: 'I32' }])

  // The imported function is (param i32) with no results.
  const impTy = m.functions.byName('imp')!.ty()
  t.deepEqual(impTy.params(), [{ type: 'I32' }])
  t.deepEqual(impTy.results(), [])
})

test('write-through: renaming a function persists through emit and re-parse', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!
  loc.name = 'renamed'
  t.is(loc.name, 'renamed')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.functions.byName('renamed')
  t.truthy(found)
  t.is(found!.kind, 'Local')
})

test('delete removes a function and the removal persists through emit and re-parse', (t) => {
  const m = load()
  const before = m.functions.length

  // Delete the LOCAL function: it is referenced by nothing (no call, export, or
  // table element), so its removal cannot dangle any reference at emit time.
  m.functions.delete(m.functions.byName('loc')!)
  t.is(m.functions.length, before - 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.functions.byName('loc'), null)
  // The imported function is untouched.
  t.truthy(reparsed.functions.byName('imp'))
})

test('delete-guard: a deleted handle throws on .kind and .name instead of aborting', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!
  m.functions.delete(loc)

  const errK = t.throws(() => loc.kind)
  t.regex(errK!.message, /deleted/)
  const errN = t.throws(() => loc.name)
  t.regex(errN!.message, /deleted/)

  // The setter path is guarded too.
  const errS = t.throws(() => {
    loc.name = 'zombie'
  })
  t.regex(errS!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(typeof loc.index, 'number')
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const loc = m.functions.byName('loc')!
  m.functions.delete(loc)

  const err = t.throws(() => m.functions.delete(loc))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.functions.byName('loc')!
  const aLen = a.functions.length
  const bLen = b.functions.length

  t.throws(() => a.functions.delete(bHandle))
  t.is(a.functions.length, aLen)
  t.is(b.functions.length, bLen)
})

test('delete drops the orphaned internal entry type of the last local function', (t) => {
  // functions.wasm has one import ($imp) and one LOCAL function ($loc). The
  // local function owns an internal "function-entry" type (raw arena index 2)
  // that WasmTypes hides. walrus' ModuleFunctions::delete only tombstones the
  // function and leaves that entry type live; deleting its last owner must drop
  // it too, or it orphans and leaks back through the entry-type filter.
  const m = load()
  const typesBefore = m.types.length // 2 real function types; entry type hidden

  // Locate the hidden entry-type raw arena index the way the B5a entry-type
  // test does: the first arena slot items()/getByIndex hides (a gap), bounded
  // to the arena so we never pick a genuinely out-of-range index.
  const realIndices = m.types.items().map((it) => it.index)
  const maxReal = Math.max(...realIndices)
  let entryIndex = -1
  for (let i = 0; i <= maxReal + 1; i++) {
    if (m.types.getByIndex(i) === null) {
      entryIndex = i
      break
    }
  }
  t.true(entryIndex >= 0, 'found the hidden entry-type arena slot')
  t.false(realIndices.includes(entryIndex))

  // Delete the sole local function. Nothing references it, so no dangling ref.
  m.functions.delete(m.functions.byName('loc')!)

  // (1) The entry type is STILL not user-visible and the arena did not grow.
  t.true(m.types.length <= typesBefore)
  const itemsAfter = m.types.items()
  t.is(itemsAfter.length, m.types.length)
  t.false(itemsAfter.map((it) => it.index).includes(entryIndex))

  // (2) The orphan was actually removed from the arena: a concrete ref to its
  // former index no longer resolves to a live entry TypeId — it is rejected at
  // creation exactly like a nonexistent index, never wired to an unemittable
  // ref that aborts at emit (on WASI the emit catch_unwind is a no-op, so this
  // cleanup is what keeps the process alive). The process stays alive.
  const err = t.throws(() =>
    m.types.addStruct([
      {
        storage: { type: 'Val', value: { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: entryIndex } } },
        mutable: false,
      },
    ]),
  )
  t.regex(err!.message, new RegExp(`no type at index ${entryIndex} in this module`))

  // (3) Emit succeeds after the delete (no abort).
  t.notThrows(() => m.emitWasm(false))
})

test('delete keeps a shared internal entry type until its last owner is gone', (t) => {
  // Two local functions with distinct function types but the same result
  // signature share ONE internal entry type (hidden). Two real function types
  // are exposed.
  const m = WasmModule.fromBuffer(sharedEntryBytes)
  t.is(m.functions.length, 2)
  t.is(m.types.length, 2)

  // Delete ONE owner. The shared entry type is still used by the other, so it
  // must NOT be dropped — the survivor's entry block still points at a LIVE
  // type, so emit does not abort and the survivor is fully intact.
  m.functions.delete(m.functions.byName('a')!)
  t.is(m.functions.length, 1)
  const b = m.functions.byName('b')
  t.truthy(b)
  t.is(b!.kind, 'Local')
  t.notThrows(() => b!.ty())
  t.notThrows(() => m.emitWasm(false))

  // Delete the LAST owner. Now the entry type is orphaned and is dropped; emit
  // still succeeds (no orphan leaks to trap at emit).
  m.functions.delete(b!)
  t.is(m.functions.length, 0)
  t.notThrows(() => m.emitWasm(false))
})
