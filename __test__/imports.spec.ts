import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// Committed, pre-compiled fixture (see fixtures/imports-exports.wat, built at
// authoring time with `wat2wasm`). Reading the bytes keeps the test hermetic —
// wat2wasm is never invoked at runtime.
//   import 0 = "e"/"f" -> Function (function index 0)
//   import 1 = "e"/"g" -> Global   (global   index 0)
//   memory index 0 (export "mem"), function index 1 (export "run"),
//   global index 1 (export "g2")
const FIXTURE = join(__dirname, 'fixtures', 'imports-exports.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const load = () => WasmModule.fromBuffer(fixtureBytes)

// A second committed fixture (see fixtures/import-links.wat, built with
// `wat2wasm --enable-exceptions`) that additionally carries an IMPORTED memory,
// table, and tag (plus local variants), so the reverse item->import cross-link
// can be exercised for every item kind. Hermetic — the bytes are read, not
// compiled.
//   imported: memory 0, table 0, tag 0, func 0, global 0
//   local:    table 1, tag 1, func 1 ($start), global 1
const LINKS_FIXTURE = join(__dirname, 'fixtures', 'import-links.wasm')
const linksBytes = readFileSync(LINKS_FIXTURE)
const loadLinks = () => WasmModule.fromBuffer(linksBytes)

// A memory-free base (see fixtures/tables.wat: two tables, no memory) used by
// the add-import-memory tests, so a freshly added imported memory yields a
// module with exactly one memory (unambiguously valid on re-parse).
const TABLES_FIXTURE = join(__dirname, 'fixtures', 'tables.wasm')
const tablesBytes = readFileSync(TABLES_FIXTURE)
const loadNoMemory = () => WasmModule.fromBuffer(tablesBytes)

const FUNCREF = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } } as const
const NON_NULLABLE_FUNCREF = { type: 'Ref', nullable: false, heap: { type: 'Abstract', kind: 'Func' } } as const
const CONCRETE_REF = { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: 0 } } as const

test('imports collection reports length and materializes item handles', (t) => {
  const m = load()
  t.is(m.imports.length, 2)

  const items = m.imports.items()
  t.is(items.length, 2)
  t.is(items[0].index, 0)
  t.is(items[1].index, 1)
})

test('a function import exposes module/name/kind and the func cross-link', (t) => {
  const m = load()
  const imp = m.imports.items()[0]

  t.is(imp.module, 'e')
  t.is(imp.name, 'f')
  t.is(imp.kind, 'Function')

  // The matching cross-link resolves to the real function handle...
  const fn = imp.func()
  t.truthy(fn)
  t.is(fn!.index, 0)
  // ...and every non-matching accessor returns null.
  t.is(imp.table(), null)
  t.is(imp.memory(), null)
  t.is(imp.global(), null)
  t.is(imp.tag(), null)
})

test('a global import exposes the global cross-link', (t) => {
  const m = load()
  const imp = m.imports.items()[1]

  t.is(imp.kind, 'Global')
  const g = imp.global()
  t.truthy(g)
  t.is(g!.index, 0)
  t.is(imp.func(), null)
})

test('getByIndex finds by stable index and returns null for a miss', (t) => {
  const m = load()
  const imp = m.imports.getByIndex(1)
  t.truthy(imp)
  t.is(imp!.index, 1)
  t.is(imp!.kind, 'Global')

  t.is(m.imports.getByIndex(99), null)
})

test('find locates an import by module and name, null for a miss', (t) => {
  const m = load()
  const imp = m.imports.find('e', 'f')
  t.truthy(imp)
  t.is(imp!.kind, 'Function')
  t.is(imp!.func()!.index, 0)

  t.truthy(m.imports.find('e', 'g'))
  t.is(m.imports.find('e', 'nope'), null)
  t.is(m.imports.find('nope', 'f'), null)
})

test('write-through: renaming an import module/name persists through emit and re-parse', (t) => {
  const m = load()
  const imp = m.imports.items()[0]
  imp.module = 'env'
  imp.name = 'ff'
  t.is(imp.module, 'env')
  t.is(imp.name, 'ff')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.truthy(reparsed.imports.find('env', 'ff'))
  t.is(reparsed.imports.find('e', 'f'), null)
})

test('delete removes an import and reduces the length', (t) => {
  const m = load()
  // Deleting an import orphans its item (invalid module) — mirror-walrus, the
  // user's concern; we only verify the collection shrinks and no abort occurs.
  m.imports.delete(m.imports.items()[0])
  t.is(m.imports.length, 1)
})

test('delete-guard: double-delete throws instead of aborting the process', (t) => {
  const m = load()
  const handle = m.imports.items()[0]

  m.imports.delete(handle)
  t.is(m.imports.length, 1)

  const err = t.throws(() => m.imports.delete(handle))
  t.regex(err!.message, /deleted/)
})

test('delete-guard: cross-module delete throws and leaves both modules unchanged', (t) => {
  const a = load()
  const b = load()

  const bHandle = b.imports.items()[0]
  t.throws(() => a.imports.delete(bHandle))

  t.is(a.imports.length, 2)
  t.is(b.imports.length, 2)
})

test('delete-guard: using a handle after delete throws on kind/module/name instead of crashing', (t) => {
  const m = load()
  const handle = m.imports.items()[0]
  m.imports.delete(handle)

  const errK = t.throws(() => handle.kind)
  t.regex(errK!.message, /deleted/)
  const errM = t.throws(() => handle.module)
  t.regex(errM!.message, /deleted/)
  const errN = t.throws(() => handle.name)
  t.regex(errN!.message, /deleted/)

  // The identity accessor stays usable — it never touches the arena.
  t.is(handle.index, 0)
})

test('reverse cross-link: an imported function resolves to its import; a local one is null', (t) => {
  const m = load()

  const importedFn = m.functions.getByIndex(0)!
  const imp = importedFn.import()
  t.truthy(imp)
  t.is(imp!.module, 'e')
  t.is(imp!.name, 'f')
  t.is(imp!.kind, 'Function')
  // The forward cross-link round-trips back to the same function.
  t.is(imp!.func()!.index, 0)

  const localFn = m.functions.getByIndex(1)!
  t.is(localFn.import(), null)
})

test('reverse cross-link: an imported global resolves to its import; a local one is null', (t) => {
  const m = load()

  const importedG = m.globals.getByIndex(0)!
  const imp = importedG.import()
  t.truthy(imp)
  t.is(imp!.kind, 'Global')
  t.is(imp!.global()!.index, 0)

  const localG = m.globals.getByIndex(1)!
  t.is(localG.import(), null)
})

test('reverse cross-link: a locally defined memory has no import', (t) => {
  const m = load()
  // In imports-exports the memory at index 0 is locally defined.
  t.is(m.memories.getByIndex(0)!.import(), null)
})

test('reverse cross-link: imported memory/table/tag each resolve to their import; local ones are null', (t) => {
  const m = loadLinks()

  const mem = m.memories.getByIndex(0)!.import()
  t.truthy(mem)
  t.is(mem!.name, 'mem')
  t.is(mem!.kind, 'Memory')
  t.is(mem!.memory()!.index, 0)

  const tbl = m.tables.getByIndex(0)!.import()
  t.truthy(tbl)
  t.is(tbl!.name, 'tbl')
  t.is(tbl!.kind, 'Table')
  t.is(tbl!.table()!.index, 0)

  const tag = m.tags.getByIndex(0)!.import()
  t.truthy(tag)
  t.is(tag!.name, 'tag')
  t.is(tag!.kind, 'Tag')
  t.is(tag!.tag()!.index, 0)

  // The local variants carry no import.
  t.is(m.tables.getByIndex(1)!.import(), null)
  t.is(m.tags.getByIndex(1)!.import(), null)
})

test('reverse cross-link delete-guard: import() on a deleted item throws instead of aborting', (t) => {
  const m = loadLinks()

  const mem = m.memories.getByIndex(0)!
  m.memories.delete(mem)
  const errMem = t.throws(() => mem.import())
  t.regex(errMem!.message, /deleted/)

  const g = m.globals.getByIndex(1)!
  m.globals.delete(g)
  const errG = t.throws(() => g.import())
  t.regex(errG!.message, /deleted/)
})

// ---------------------------------------------------------------------------
// Import CREATION (add_import_* on WasmImports) — task B4b.
// ---------------------------------------------------------------------------

test('addFunction creates an imported function whose import round-trips', (t) => {
  const m = load()
  const beforeImports = m.imports.length
  const beforeFuncs = m.functions.length

  // A distinctive signature so it is findable after re-parse.
  const ty = m.types.add([{ type: 'F64' }], [{ type: 'F64' }])
  const fn = m.imports.addFunction('env', 'fi', ty)

  // The returned handle is the ITEM; its import record is reachable via import().
  t.is(fn.kind, 'Import')
  const imp = fn.import()
  t.truthy(imp)
  t.is(imp!.module, 'env')
  t.is(imp!.name, 'fi')
  t.is(imp!.kind, 'Function')
  t.is(m.imports.length, beforeImports + 1)
  t.is(m.functions.length, beforeFuncs + 1)
  t.deepEqual(fn.ty().params(), [{ type: 'F64' }])
  t.deepEqual(fn.ty().results(), [{ type: 'F64' }])

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.imports.find('env', 'fi')
  t.truthy(found)
  t.is(found!.kind, 'Function')
  const rfn = found!.func()!
  t.deepEqual(rfn.ty().params(), [{ type: 'F64' }])
  t.deepEqual(rfn.ty().results(), [{ type: 'F64' }])
})

test('addMemory creates an imported memory whose initial/maximum/shared round-trip', (t) => {
  const m = loadNoMemory()
  t.is(m.memories.length, 0)

  const mem = m.imports.addMemory('env', 'm', false, false, 2n, 5n, null)
  t.is(m.memories.length, 1)
  t.is(mem.initial, 2n)
  t.is(mem.maximum, 5n)
  t.is(mem.shared, false)
  t.is(mem.memory64, false)

  const imp = mem.import()
  t.truthy(imp)
  t.is(imp!.module, 'env')
  t.is(imp!.name, 'm')
  t.is(imp!.kind, 'Memory')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.imports.find('env', 'm')!
  t.is(found.kind, 'Memory')
  const rmem = found.memory()!
  t.is(rmem.initial, 2n)
  t.is(rmem.maximum, 5n)
  t.is(rmem.shared, false)

  // The `shared` flag is wired through (asserted in-memory; a shared+re-parse
  // would additionally depend on the threads feature, out of scope here).
  const m2 = loadNoMemory()
  const sharedMem = m2.imports.addMemory('env', 's', true, false, 1n, 2n, null)
  t.is(sharedMem.shared, true)
})

test('addTable creates an imported table whose element type and limits round-trip', (t) => {
  const m = load()
  const before = m.tables.length

  const tbl = m.imports.addTable('env', 't', false, 1n, 4n, FUNCREF)
  t.is(m.tables.length, before + 1)
  t.is(tbl.initial, 1n)
  t.is(tbl.maximum, 4n)
  t.is(tbl.table64, false)
  t.deepEqual(tbl.elementTy, FUNCREF)

  const imp = tbl.import()
  t.truthy(imp)
  t.is(imp!.module, 'env')
  t.is(imp!.name, 't')
  t.is(imp!.kind, 'Table')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.imports.find('env', 't')!
  t.is(found.kind, 'Table')
  const rtbl = found.table()!
  t.deepEqual(rtbl.elementTy, FUNCREF)
  t.is(rtbl.initial, 1n)
  t.is(rtbl.maximum, 4n)
})

test('addGlobal creates an imported global whose type and mutability round-trip', (t) => {
  const m = load()
  const before = m.globals.length

  const g = m.imports.addGlobal('env', 'gi', { type: 'I64' }, true, false)
  t.is(m.globals.length, before + 1)
  t.is(g.kind, 'Import')
  t.deepEqual(g.ty, { type: 'I64' })
  t.is(g.mutable, true)

  const imp = g.import()
  t.truthy(imp)
  t.is(imp!.module, 'env')
  t.is(imp!.name, 'gi')
  t.is(imp!.kind, 'Global')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.imports.find('env', 'gi')!
  t.is(found.kind, 'Global')
  const rg = found.global()!
  t.deepEqual(rg.ty, { type: 'I64' })
  t.is(rg.mutable, true)
})

test('addTag creates an imported tag whose type round-trips', (t) => {
  const m = load()
  const before = m.tags.length

  const ty = m.types.add([{ type: 'I32' }], [])
  const tag = m.imports.addTag('env', 'ti', ty)
  t.is(m.tags.length, before + 1)
  t.is(tag.kind, 'Import')
  t.deepEqual(tag.ty().params(), [{ type: 'I32' }])

  const imp = tag.import()
  t.truthy(imp)
  t.is(imp!.module, 'env')
  t.is(imp!.name, 'ti')
  t.is(imp!.kind, 'Tag')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const found = reparsed.imports.find('env', 'ti')!
  t.is(found.kind, 'Tag')
  t.deepEqual(found.tag()!.ty().params(), [{ type: 'I32' }])
})

test('id-ref guard: addFunction rejects a type from a different module instead of aborting', (t) => {
  const a = load()
  const b = load()

  // A WasmType minted from module B carries B's arena_id; walrus would resolve
  // it via a panicking get_type_index at A's emit and abort the whole process.
  // The id-ref guard rejects it with a catchable JS error first.
  const foreignType = b.types.add([{ type: 'F64' }], [])
  const err = t.throws(() => a.imports.addFunction('env', 'f', foreignType))
  t.regex(err!.message, /not in this module/)

  // The rejected add did not mutate A, and the process is still alive.
  t.is(a.imports.length, 2)
})

test('id-ref guard: addTag rejects a type from a different module instead of aborting', (t) => {
  const a = load()
  const b = load()

  const foreignType = b.types.add([{ type: 'I32' }], [])
  const err = t.throws(() => a.imports.addTag('env', 't', foreignType))
  t.regex(err!.message, /not in this module/)
  t.is(a.imports.length, 2)
})

test('id-ref guard: addFunction rejects a deleted type from the same module', (t) => {
  const m = load()
  const ty = m.types.add([{ type: 'F32' }], [])
  m.types.delete(ty)

  const err = t.throws(() => m.imports.addFunction('env', 'f', ty))
  t.regex(err!.message, /not in this module|deleted/)
  t.is(m.imports.length, 2)
})

test('bigint corruption guard: addMemory rejects a negative initial size', (t) => {
  const m = loadNoMemory()
  const err = t.throws(() => m.imports.addMemory('env', 'm', false, false, -1n, null, null))
  t.regex(err!.message, /non-negative/)
  // The rejected add never mutated the module.
  t.is(m.memories.length, 0)
})

test('bigint corruption guard: addMemory rejects an out-of-range (u64 overflow) initial size', (t) => {
  const m = loadNoMemory()
  const err = t.throws(() => m.imports.addMemory('env', 'm', false, false, 2n ** 64n, null, null))
  t.regex(err!.message, /non-negative/)
  t.is(m.memories.length, 0)
})

// F-fix6 (Codex P2): `pageSizeLog2` is carried as `f64` and narrowed LOSSLESSLY
// through `checked_index` before the walrus call. Under the OLD `Option<u32>`
// wire type napi applied ToUint32 FIRST, so an out-of-domain value silently
// ALIASED a different valid page size; it is now a catchable throw and the
// rejected add never mutates the module.
test('F-fix6 (Codex P2): addMemory rejects an out-of-domain pageSizeLog2 and leaves the module unchanged', (t) => {
  const m = loadNoMemory()
  for (const bad of [-1, 2 ** 32, 1.5, NaN]) {
    const err = t.throws(() => m.imports.addMemory('env', 'm', false, false, 1n, null, bad))
    t.regex(err!.message, /pageSizeLog2 must be an integer in 0\.\.=4294967295/)
  }
  t.is(m.memories.length, 0)
})

test('F-fix6 (Codex P2): addMemory accepts a valid pageSizeLog2 and the getter reads it back', (t) => {
  const m = loadNoMemory()
  // 16 = the default 64 KiB pages (2**16), a valid custom-page-sizes log2.
  const mem = m.imports.addMemory('env', 'm', false, false, 1n, null, 16)
  t.is(m.memories.length, 1)
  t.is(mem.pageSizeLog2, 16)
})

test('element-type guard: addTable rejects a non-reference element type', (t) => {
  const m = load()
  const before = m.tables.length
  const err = t.throws(() => m.imports.addTable('env', 't', false, 1n, null, { type: 'I32' }))
  t.regex(err!.message, /reference type/i)
  t.is(m.tables.length, before)
})

test('element-type: addTable accepts a concrete ref to an existing type, rejects a nonexistent index', (t) => {
  // B5c: the element type now routes through the module-aware converter, so a
  // concrete ref to an EXISTING type (type 0 is the imported function's type)
  // is a valid typed-function-ref element type.
  const m = load()
  const before = m.tables.length
  const tbl = m.imports.addTable('env', 't', false, 1n, null, CONCRETE_REF)
  t.deepEqual(tbl.elementTy, CONCRETE_REF)
  t.is(m.tables.length, before + 1)

  // A concrete ref to a NONEXISTENT index is rejected catchably (never aborts).
  const bad = { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: 9999 } } as const
  const err = t.throws(() => m.imports.addTable('env', 't2', false, 1n, null, bad))
  t.regex(err!.message, /no type at index 9999/)
  t.is(m.tables.length, before + 1)
})

test('element-type guard: addTable accepts BOTH a nullable and a non-nullable funcref (imported table, no init)', (t) => {
  // A nullable funcref imported table is MVP-valid and validates.
  const m1 = load()
  const nullable = m1.imports.addTable('env', 'tn', false, 1n, null, FUNCREF)
  t.deepEqual(nullable.elementTy, FUNCREF)
  t.true(WebAssembly.validate(m1.emitWasm(false)))

  // Unlike tables.addLocal, a NON-nullable element type is ALSO accepted here:
  // an imported table has no init segment, so a non-nullable element is valid.
  // (Validated via walrus re-parse, which enables the function-references
  // feature; Node's WebAssembly.validate may gate non-nullable refs.)
  const m2 = load()
  const nonNullable = m2.imports.addTable('env', 'tnn', false, 1n, null, NON_NULLABLE_FUNCREF)
  t.deepEqual(nonNullable.elementTy, NON_NULLABLE_FUNCREF)
  const reparsed = WasmModule.fromBuffer(m2.emitWasm(false))
  t.deepEqual(reparsed.imports.find('env', 'tnn')!.table()!.elementTy, NON_NULLABLE_FUNCREF)
})

test('mirror-walrus: addMemory stores min>max verbatim (no semantic check); WebAssembly.validate flags it', (t) => {
  const m = loadNoMemory()

  // min > max is wasm-invalid, but mirror-walrus stores it verbatim rather than
  // second-guessing the caller — WebAssembly.validate is the user's tool. The
  // add must NOT throw and the values must be stored exactly as given.
  const mem = m.imports.addMemory('env', 'm', false, false, 5n, 1n, null)
  t.is(m.memories.length, 1)
  t.is(mem.initial, 5n)
  t.is(mem.maximum, 1n)

  // Emit still succeeds (walrus does not validate on emit), producing bytes...
  const bytes = m.emitWasm(false)
  // ...that WebAssembly.validate correctly rejects (the user's tool catches the
  // semantic error we intentionally did not). A walrus re-parse would also
  // reject it (from_buffer validates), which is exactly the point.
  t.false(WebAssembly.validate(bytes))
})
