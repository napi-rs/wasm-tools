import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/tags.wat, built at authoring
// time with `wat2wasm --enable-exceptions`). Reading the bytes keeps the test
// hermetic — wat2wasm is never invoked at runtime.
//   type 0 = (func (param i32))
//   tag  0 = local exception tag of type 0
const FIXTURE = join(__dirname, 'fixtures', 'tags.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

// A module with a LOCAL function: walrus mints an internal "function-entry"
// type per local function, so this arena holds one alongside the real types.
// See fixtures/functions.wat (func $loc is local).
const FUNCTIONS_FIXTURE = join(__dirname, 'fixtures', 'functions.wasm')
const functionsBytes = readFileSync(FUNCTIONS_FIXTURE)

test('tags collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.tags.length, 1)

  const items = m.tags.items()
  t.is(items.length, 1)
  t.is(items[0].index, 0)
  t.is(items[0].kind, 'Local')
})

test('a parsed tag exposes its type via the tag -> type cross-link', (t) => {
  const m = load()
  const tag = m.tags.items()[0]

  // .ty() materializes a live WasmType handle; its signature is the tag's
  // exception payload (param i32, no results).
  const ty = tag.ty()
  t.is(ty.kind, 'Function')
  t.deepEqual(ty.params(), [{ type: 'I32' }])
  t.deepEqual(ty.results(), [])
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const tag = m.tags.getByIndex(0)
  t.truthy(tag)
  t.is(tag!.index, 0)
  t.is(tag!.kind, 'Local')

  t.is(m.tags.getByIndex(99), null)
})

test('add creates a local tag whose type round-trips through emit and re-parse', (t) => {
  const m = load()
  const before = m.tags.length

  const ty = m.types.add([{ type: 'I32' }], [])
  const added = m.tags.add(ty)
  t.is(m.tags.length, before + 1)
  t.is(added.kind, 'Local')
  t.deepEqual(added.ty().params(), [{ type: 'I32' }])
  t.deepEqual(added.ty().results(), [])

  // Local tags are emitted unconditionally (no gc runs on emit), so the added
  // tag survives a round-trip.
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.tags.length, before + 1)
})

test('add rejects a type from a different module instead of aborting at emit', (t) => {
  const a = load()
  const b = load()

  // A WasmType minted from module B carries B's arena_id; walrus would resolve
  // it via a panicking get_type_index at A's emit and abort the whole process.
  // The id-ref guard rejects it with a catchable JS error first.
  const foreignType = b.types.add([{ type: 'F64' }], [])
  const err = t.throws(() => a.tags.add(foreignType))
  t.regex(err!.message, /not in this module/)

  // The rejected add did not mutate A, and the process is still alive.
  t.is(a.tags.length, 1)
})

test('add rejects a deleted type from the same module', (t) => {
  const m = load()
  const ty = m.types.add([{ type: 'F32' }], [])
  m.types.delete(ty)

  const err = t.throws(() => m.tags.add(ty))
  t.regex(err!.message, /not in this module|deleted/)
  t.is(m.tags.length, 1)
})

test('write-through: renaming a tag persists through emit and re-parse', (t) => {
  const m = load()
  const tag = m.tags.items()[0]
  t.is(tag.name, null)
  tag.name = 'renamed'
  t.is(tag.name, 'renamed')

  // Name persists via the name section (generateNameSection defaults true).
  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.tags.items().find((x) => x.name === 'renamed')
  t.truthy(found)
  t.is(found!.index, 0)
})

test('delete removes a tag and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.tags.delete(m.tags.items()[0])
  t.is(m.tags.length, 0)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.tags.length, 0)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.tags.items()[0]

  m.tags.delete(handle)
  t.is(m.tags.length, 0)

  // Deleting the SAME (now dead) handle again must raise a catchable JS error
  // rather than tripping walrus' `assert(contains(id))` and aborting via FFI.
  const err = t.throws(() => m.tags.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  // A handle minted from module B must never be accepted by module A: the ids
  // carry an arena_id, so the liveness scan rejects the foreign handle before
  // walrus can assert on it.
  const bHandle = b.tags.items()[0]
  t.throws(() => a.tags.delete(bHandle))

  t.is(a.tags.length, 1)
  t.is(b.tags.length, 1)
})

test('add + emit never aborts the process: emit-time panics surface as catchable errors', (t) => {
  // walrus keeps INTERNAL function-entry types in the type arena (one per local
  // function). They are indistinguishable from a plain `(func)` in our API, so
  // `tags.add` cannot reject them — referencing one makes walrus' emit panic in
  // `get_type_index`, which (before the catch_unwind guard) aborts the whole
  // Node worker via FFI. This exhaustively adds EACH type as a tag on a fresh
  // module and emits: the process must stay alive across ALL indices, and the
  // entry-type index(es) must throw a CATCHABLE "emit" error rather than abort.
  const probe = WasmModule.fromBuffer(functionsBytes)
  const typeCount = probe.types.length
  t.true(typeCount >= 1)

  let emittedOk = 0
  const emitErrors: string[] = []
  for (let i = 0; i < typeCount; i++) {
    // Fresh module per iteration: a caught emit leaves the module consistent,
    // but a fresh parse also isolates each index so one bad type can't taint
    // another's emit.
    const m = WasmModule.fromBuffer(functionsBytes)
    m.tags.add(m.types.items()[i])
    try {
      m.emitWasm(false)
      emittedOk++
    } catch (e) {
      emitErrors.push((e as Error).message)
    }
  }

  // Reaching here at all proves no index aborted the process (an abort would
  // kill the ava worker, failing the whole file).
  t.true(emittedOk >= 1, 'the real function types emit successfully')
  t.true(emitErrors.length >= 1, 'at least one internal entry type is rejected at emit')
  t.true(
    emitErrors.some((msg) => /emit/.test(msg)),
    'the emit failure is a catchable error mentioning "emit"',
  )
})

test('delete-guard: using a handle after delete throws on kind/name/ty instead of crashing', (t) => {
  const m = load()
  const handle = m.tags.items()[0]
  m.tags.delete(handle)

  const errK = t.throws(() => handle.kind)
  t.regex(errK!.message, /deleted/)
  const errN = t.throws(() => handle.name)
  t.regex(errN!.message, /deleted/)
  const errT = t.throws(() => handle.ty())
  t.regex(errT!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 0)
})
