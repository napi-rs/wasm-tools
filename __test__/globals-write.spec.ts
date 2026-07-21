import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ConstExpr, ModuleConfig, WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// The canonical 8-byte empty module: valid wasm header, zero sections. walrus
// parses it, and it carries no globals/custom sections, giving each test a
// clean slate. Building fresh instances keeps the suite hermetic (no CLI).
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const empty = () => WasmModule.fromBuffer(EMPTY_MODULE)

// Committed, pre-compiled fixture (see fixtures/typed-refnull.wat). A module
// with one concrete type `$t` and a global `(ref null $t) (ref.null $t)`, so its
// only global's initializer is a RefNull carrying a CONCRETE TypeId. The typed
// ref needs proposal features, so it must be parsed with onlyStableFeatures(false).
//   (module (type $t (func)) (global (ref null $t) (ref.null $t)))
const TYPED_REFNULL = readFileSync(join(__dirname, 'fixtures', 'typed-refnull.wasm'))
const parseTypedRefNull = () => new ModuleConfig().onlyStableFeatures(false).parse(TYPED_REFNULL)

const FUNCREF = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } } as const

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

test('addLocal creates a nullable externref global (ref type + refNull init), emits valid wasm, and round-trips', (t) => {
  const m = empty()

  // refNull is ALWAYS nullable (single-arg), so it initializes a NULLABLE
  // externref global. A non-nullable null would be invalid wasm.
  const g = m.globals.addLocal(EXTERNREF, false, false, ConstExpr.refNull(EXTERN_HEAP))
  t.is(m.globals.length, 1)
  t.deepEqual(g.ty, EXTERNREF)
  t.is(g.mutable, false)
  t.is(g.init()!.kind, 'RefNull')

  const bytes = m.emitWasm(false)
  // Independent validity check: the emitted module must actually validate.
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
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

test('addLocal rejects a concrete-heap ref to a nonexistent type index (catchable, no abort)', (t) => {
  const m = empty()
  // The empty module has no types, so `typeIndex: 0` names nothing live: the
  // module-aware converter now resolves concrete refs (B5c) and rejects a bad
  // index catchably BEFORE the arena is touched, rather than aborting at emit.
  const concreteRef = { type: 'Ref', nullable: true, heap: { type: 'Concrete', typeIndex: 0 } } as const

  const err = t.throws(() => m.globals.addLocal(concreteRef, false, false, ConstExpr.i32(0)))
  t.regex(err!.message, /no type at index 0/)

  // The rejected add left the module untouched.
  t.is(m.globals.length, 0)
})

test('ConstExpr factory kinds map to the walrus discriminants', (t) => {
  t.is(ConstExpr.i32(1).kind, 'Value')
  t.is(ConstExpr.i64(1n).kind, 'Value')
  t.is(ConstExpr.f32(1.5).kind, 'Value')
  t.is(ConstExpr.f64(1.5).kind, 'Value')
  t.is(ConstExpr.refNull({ type: 'Abstract', kind: 'Func' }).kind, 'RefNull')

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

// Regression: a ConstExpr that reads a global from ANOTHER module must be
// rejected at addLocal time with a CATCHABLE error. Before the fix, the
// foreign id survived into emit, where walrus panicked in get_global_index and
// ABORTED the whole Node process (uncatchable). This test simply COMPLETING —
// the process staying alive to run the assertion and every later test — is the
// proof that the abort is gone.
test('addLocal rejects a globalGet ConstExpr from another module (throws, never aborts)', (t) => {
  const moduleB = empty()
  const bGlobal = moduleB.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(1))

  const moduleA = empty()
  const ce = ConstExpr.globalGet(bGlobal)

  const err = t.throws(() => moduleA.globals.addLocal({ type: 'I32' }, true, false, ce))
  t.regex(err!.message, /not in this module|deleted/i)

  // The rejected add left module A untouched, and the process is still alive.
  t.is(moduleA.globals.length, 0)
})

// Regression: same guard also catches an already-DELETED global's id. After
// delete, the id has no index in the module, so emit would abort; addLocal must
// reject it up front instead.
test('addLocal rejects a globalGet ConstExpr referencing an already-deleted global (throws, never aborts)', (t) => {
  const m = empty()
  const g1 = m.globals.addLocal({ type: 'I32' }, false, false, ConstExpr.i32(1))
  m.globals.delete(g1)

  const ce = ConstExpr.globalGet(g1)
  const err = t.throws(() => m.globals.addLocal({ type: 'I32' }, false, false, ce))
  t.regex(err!.message, /not in this module|deleted/i)

  t.is(m.globals.length, 0)
})

// Regression: a RefNull ConstExpr carrying a CONCRETE TypeId from ANOTHER module
// must be rejected at addLocal time with a CATCHABLE error. Before the fix,
// validate_const_expr treated every RefNull as id-free, so the foreign TypeId
// survived into emit, where walrus panicked in get_type_index and ABORTED the
// whole Node process (SIGABRT, exit 134). This test simply COMPLETING — the
// process staying alive to run the assertions and every later test — is the
// proof that the abort is gone.
test('addLocal rejects a typed RefNull ConstExpr whose type lives in another module (throws, never aborts)', (t) => {
  const moduleA = parseTypedRefNull()
  const ce = moduleA.globals.items()[0].init()!
  t.is(ce.kind, 'RefNull')

  // module B has no types, so A's concrete TypeId is foreign to it.
  const moduleB = empty()
  const err = t.throws(() => moduleB.globals.addLocal(FUNCREF, false, false, ce))
  t.regex(err!.message, /type/i)

  // The rejected add left module B untouched, the process is still alive, and B
  // still emits + re-parses cleanly (no corruption, no abort).
  t.is(moduleB.globals.length, 0)
  const reparsed = WasmModule.fromBuffer(moduleB.emitWasm(false))
  t.is(reparsed.globals.length, 0)
})

// Positive / provenance: the guard rejects FOREIGN/deleted TypeIds, NOT every
// concrete RefNull. On module A itself — whose type IS live — the very same
// typed RefNull initializer must be accepted, emit, and re-parse. This guards
// against an over-broad fix that just rejects all concrete RefNulls.
test('addLocal accepts a typed RefNull ConstExpr whose type is live in the same module', (t) => {
  const moduleA = parseTypedRefNull()
  const init = moduleA.globals.items()[0].init()!
  t.is(init.kind, 'RefNull')

  // funcref is the abstract supertype of `(ref null $t)`, so `ref.null $t` is a
  // valid initializer for it (subtyping). (`ty` here is the abstract funcref; a
  // concrete `ty` is exercised separately in the B5c rec-group/concrete spec.)
  const g = moduleA.globals.addLocal(FUNCREF, false, false, init)
  t.is(g.init()!.kind, 'RefNull')
  t.is(moduleA.globals.length, 2)

  // Emits bytes that re-parse (with proposal features on) — provenance held.
  const out = moduleA.emitWasm(false)
  const reparsed = new ModuleConfig().onlyStableFeatures(false).parse(out)
  t.is(reparsed.globals.length, 2)
})

// elements.wasm has func $f (index 0), already in the module's declared refs
// (referenced by its element segments), so `ref.func $f` is a valid const-expr
// initializer.
const ELEMENTS = readFileSync(join(__dirname, 'fixtures', 'elements.wasm'))
const loadWithFunc = () => WasmModule.fromBuffer(ELEMENTS)

// refFunc (F2 API 2) used as a global initializer round-trips through emit +
// re-parse (the write direction of `ref.func`, mirroring globalGet/refNull).
test('addLocal accepts a refFunc initializer that round-trips', (t) => {
  const m = loadWithFunc()
  const func = m.functions.items()[0]
  const g = m.globals.addLocal(FUNCREF, false, false, ConstExpr.refFunc(func))
  t.is(g.init()!.kind, 'RefFunc')

  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))

  const reparsed = WasmModule.fromBuffer(bytes)
  const rg = reparsed.globals.items().find((x) => x.init()?.kind === 'RefFunc')
  t.truthy(rg)
})

// Consume-site guard: a refFunc off a DELETED function is rejected catchably at
// globals.addLocal (its FunctionId is no longer live), never a process abort.
test('addLocal rejects a refFunc initializer off a deleted function (throws, never aborts)', (t) => {
  const m = loadWithFunc()
  const func = m.functions.items()[0]
  const rf = ConstExpr.refFunc(func)
  m.functions.delete(func)

  const err = t.throws(() => m.globals.addLocal(FUNCREF, false, false, rf))
  t.regex(err!.message, /function that is not in this module/)
  t.is(m.globals.length, 0)
})
