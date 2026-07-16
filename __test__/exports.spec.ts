import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ConstExpr, WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/imports-exports.wat). Exports:
//   "mem" -> Memory   (memory index 0)
//   "run" -> Function (function index 1, a LOCAL function)
//   "g2"  -> Global   (global index 1, a LOCAL global)
const FIXTURE = join(__dirname, 'fixtures', 'imports-exports.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

test('exports collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.exports.length, 3)

  const items = m.exports.items()
  t.is(items.length, 3)
  t.is(items[0].index, 0)
})

test('a memory export exposes kind and the memory cross-link', (t) => {
  const m = load()
  const exp = m.exports.byName('mem')
  t.truthy(exp)
  t.is(exp!.kind, 'Memory')

  const mem = exp!.memory()
  t.truthy(mem)
  t.is(mem!.index, 0)
  // Non-matching accessors return null.
  t.is(exp!.func(), null)
  t.is(exp!.table(), null)
  t.is(exp!.global(), null)
  t.is(exp!.tag(), null)
})

test('byName resolves the function and global exports to the right handles', (t) => {
  const m = load()

  const run = m.exports.byName('run')
  t.truthy(run)
  t.is(run!.kind, 'Function')
  t.is(run!.func()!.index, 1)

  const g2 = m.exports.byName('g2')
  t.truthy(g2)
  t.is(g2!.kind, 'Global')
  t.is(g2!.global()!.index, 1)

  t.is(m.exports.byName('nope'), null)
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const exp = m.exports.getByIndex(0)
  t.truthy(exp)
  t.is(exp!.index, 0)

  t.is(m.exports.getByIndex(99), null)
})

test('addFunction exports a function; it round-trips through emit and re-parse', (t) => {
  const m = load()
  const before = m.exports.length

  const fn = m.functions.getByIndex(1)! // the local "run" function
  const added = m.exports.addFunction('run2', fn)
  t.is(added.kind, 'Function')
  t.is(added.func()!.index, 1)
  t.is(m.exports.length, before + 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.exports.byName('run2')
  t.truthy(found)
  t.is(found!.kind, 'Function')
  t.is(found!.func()!.index, 1)
})

test('addGlobal exports a global; it round-trips through emit and re-parse', (t) => {
  const m = load()
  const before = m.exports.length

  const g = m.globals.getByIndex(1)! // the local "g2" global
  const added = m.exports.addGlobal('g3', g)
  t.is(added.kind, 'Global')
  t.is(added.global()!.index, 1)
  t.is(m.exports.length, before + 1)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.exports.byName('g3')
  t.truthy(found)
  t.is(found!.kind, 'Global')
})

test('addMemory exports a memory and round-trips', (t) => {
  const m = load()
  const mem = m.memories.getByIndex(0)!
  const added = m.exports.addMemory('mem2', mem)
  t.is(added.kind, 'Memory')
  t.is(added.memory()!.index, 0)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.truthy(reparsed.exports.byName('mem2'))
})

test('id-ref guard: addFunction rejects a function from a different module instead of aborting', (t) => {
  const a = load()
  const b = load()

  // A WasmFunction minted from module B carries B's arena_id; walrus would
  // resolve it via a panicking get_func_index at A's emit and abort the whole
  // process. The id-ref guard rejects it with a catchable JS error first.
  const foreign = b.functions.getByIndex(1)!
  const err = t.throws(() => a.exports.addFunction('x', foreign))
  t.regex(err!.message, /not in this module/)

  // The rejected add did not mutate A, and the process is still alive.
  t.is(a.exports.length, 3)
})

test('id-ref guard: addGlobal rejects a deleted global from the same module', (t) => {
  const m = load()
  const g = m.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(0))
  m.globals.delete(g)

  const err = t.throws(() => m.exports.addGlobal('x', g))
  t.regex(err!.message, /not in this module|deleted/)
  t.is(m.exports.length, 3)
})

test('write-through: renaming an export persists through emit and re-parse', (t) => {
  const m = load()
  const run = m.exports.byName('run')!
  run.name = 'r'
  t.is(run.name, 'r')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.truthy(reparsed.exports.byName('r'))
  t.is(reparsed.exports.byName('run'), null)
})

test('delete removes an export and the removal persists through emit and re-parse', (t) => {
  const m = load()
  m.exports.delete(m.exports.byName('g2')!)
  t.is(m.exports.length, 2)

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.exports.length, 2)
  t.is(reparsed.exports.byName('g2'), null)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.exports.byName('g2')!

  m.exports.delete(handle)
  t.is(m.exports.length, 2)

  const err = t.throws(() => m.exports.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.exports.byName('g2')!
  t.throws(() => a.exports.delete(bHandle))

  t.is(a.exports.length, 3)
  t.is(b.exports.length, 3)
})

test('delete-guard: using a handle after delete throws on kind/name instead of crashing', (t) => {
  const m = load()
  const handle = m.exports.byName('g2')!
  const idx = handle.index
  m.exports.delete(handle)

  const errK = t.throws(() => handle.kind)
  t.regex(errK!.message, /deleted/)
  const errN = t.throws(() => handle.name)
  t.regex(errN!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, idx)
})
