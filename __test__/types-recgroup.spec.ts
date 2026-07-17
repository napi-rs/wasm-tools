import test from 'ava'

import { ConstExpr, ModuleConfig, WasmModule, type RecGroupMember, type ValType } from '../index'

// The canonical 8-byte empty module (valid header, zero sections). Building
// fresh instances keeps the suite hermetic (no CLI at runtime).
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const empty = () => WasmModule.fromBuffer(EMPTY_MODULE)
// Concrete refs and rec groups are non-stable, so re-parsing emitted bytes needs
// the non-stable-features gate opened.
const reparseGc = (bytes: Uint8Array) => new ModuleConfig().onlyStableFeatures(false).parse(bytes)

const concreteRef = (typeIndex: number): ValType => ({
  type: 'Ref',
  nullable: true,
  heap: { type: 'Concrete', typeIndex },
})
const recGroupRef = (recIndex: number): ValType => ({
  type: 'Ref',
  nullable: true,
  heap: { type: 'RecGroup', recIndex },
})
const structOf = (field: ValType): RecGroupMember => ({
  composite: { type: 'Struct', fields: [{ storage: { type: 'Val', value: field }, mutable: false }] },
  isFinal: true,
})

// ---------------------------------------------------------------------------
// PART 1 — concrete refs at the rerouted consume sites (types.add / find,
// globals.addLocal, locals.add, tables.addLocal). imports.addGlobal /
// imports.addTable are covered in imports.spec.ts.
// ---------------------------------------------------------------------------

test('PART1: types.add accepts a concrete-ref param/result and round-trips', (t) => {
  const m = empty()
  const s = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  // (func (param (ref null $s)) (result (ref null $s)))
  const fn = m.types.add([concreteRef(s.index)], [concreteRef(s.index)])
  t.is(fn.kind, 'Function')
  t.deepEqual(fn.params(), [concreteRef(s.index)])
  t.deepEqual(fn.results(), [concreteRef(s.index)])

  // find with a concrete-ref query resolves against the arena and matches.
  const found = m.types.find([concreteRef(s.index)], [concreteRef(s.index)])
  t.truthy(found)
  t.is(found!.index, fn.index)

  // Emit resolves the concrete ref through get_type_index (the abort we guard);
  // re-parse confirms it survives.
  const reparsed = reparseGc(m.emitWasm(false))
  const rfn = reparsed.types.items().find((x) => x.kind === 'Function' && x.params().length === 1)
  t.truthy(rfn)
  const p = rfn!.params()[0]
  t.is(p.type, 'Ref')
  if (p.type !== 'Ref') return t.fail('expected a ref param')
  t.is(p.heap.type, 'Concrete')
})

test('PART1: types.add / find reject a concrete ref to a nonexistent index (catchable)', (t) => {
  const m = empty()
  t.regex(t.throws(() => m.types.add([concreteRef(9999)], []))!.message, /no type at index 9999/)
  t.regex(t.throws(() => m.types.find([concreteRef(9999)], []))!.message, /no type at index 9999/)
  // Rejected before any arena mutation; the module is still usable.
  t.notThrows(() => m.types.add([{ type: 'I32' }], []))
})

test('PART1: locals.add accepts a concrete-ref local type that reads back', (t) => {
  const m = empty()
  const s = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'F64' } }, mutable: false }])
  const local = m.locals.add(concreteRef(s.index))
  t.deepEqual(local.ty, concreteRef(s.index))

  // A bad index is rejected catchably, leaving the module usable.
  t.regex(t.throws(() => m.locals.add(concreteRef(9999)))!.message, /no type at index 9999/)
})

test('PART1: globals.addLocal accepts a concrete-ref global type + a concrete ref.null init, round-trips', (t) => {
  const m = empty()
  const s = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  // (global (ref null $s) (ref.null $s)) — the concrete ty AND WasmType.refNull.
  const g = m.globals.addLocal(concreteRef(s.index), false, false, s.refNull())
  t.deepEqual(g.ty, concreteRef(s.index))
  t.is(g.init()!.kind, 'RefNull')

  const reparsed = reparseGc(m.emitWasm(false))
  t.is(reparsed.globals.length, 1)
  const rg = reparsed.globals.getByIndex(0)!
  t.is(rg.ty.type, 'Ref')
  if (rg.ty.type !== 'Ref') return t.fail('expected a ref global')
  t.is(rg.ty.heap.type, 'Concrete')
  t.is(rg.init()!.kind, 'RefNull')
})

test('PART1: tables.addLocal accepts a nullable concrete-ref element type that reads back', (t) => {
  const m = empty()
  const s = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  const tbl = m.tables.addLocal(false, 1n, null, concreteRef(s.index))
  t.deepEqual(tbl.elementTy, concreteRef(s.index))

  // A non-nullable concrete ref is still rejected (null-init table needs a
  // defaultable element type) — the reroute keeps the nullability guard.
  const nonNullable: ValType = { type: 'Ref', nullable: false, heap: { type: 'Concrete', typeIndex: s.index } }
  t.regex(t.throws(() => m.tables.addLocal(false, 1n, null, nonNullable))!.message, /nullable|initializer/i)
})

// ---------------------------------------------------------------------------
// PART 2 — addRecGroup
// ---------------------------------------------------------------------------

test('PART2: addRecGroup builds two mutually-recursive structs whose cross-refs resolve + round-trip', (t) => {
  const m = empty()
  // (rec (type $a (struct (field (ref null $b)))) (type $b (struct (field (ref null $a)))))
  const handles = m.types.addRecGroup([structOf(recGroupRef(1)), structOf(recGroupRef(0))])
  t.is(handles.length, 2)
  const [a, b] = handles
  t.is(a.kind, 'Struct')
  t.is(b.kind, 'Struct')
  t.is(a.isExplicitRecGroup, true)
  t.is(b.isExplicitRecGroup, true)
  // The whole group (both members) is reported for each.
  t.is(a.recGroupMembers().length, 2)

  // In memory, each sibling ref reads back as a concrete ref to the OTHER member.
  t.deepEqual(a.structFields(), [{ storage: { type: 'Val', value: concreteRef(b.index) }, mutable: false }])
  t.deepEqual(b.structFields(), [{ storage: { type: 'Val', value: concreteRef(a.index) }, mutable: false }])

  // Emit -> re-parse: both structs survive and each field targets a struct.
  const reparsed = reparseGc(m.emitWasm(false))
  const structs = reparsed.types.items().filter((x) => x.kind === 'Struct')
  t.is(structs.length, 2)
  for (const s of structs) {
    const f = s.structFields()[0]
    t.is(f.storage.type, 'Val')
    if (f.storage.type !== 'Val' || f.storage.value.type !== 'Ref' || f.storage.value.heap.type !== 'Concrete') {
      return t.fail('expected a concrete-ref field')
    }
    const target = reparsed.types.getByIndex(f.storage.value.heap.typeIndex)
    t.truthy(target)
    t.is(target!.kind, 'Struct')
  }
})

test('PART2: an addRecGroup member can reference an EXISTING (pre-group) type', (t) => {
  const m = empty()
  const base = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  const [c] = m.types.addRecGroup([structOf(concreteRef(base.index))])
  t.is(c.kind, 'Struct')
  t.deepEqual(c.structFields(), [{ storage: { type: 'Val', value: concreteRef(base.index) }, mutable: false }])
  t.notThrows(() => reparseGc(m.emitWasm(false)))
})

test('PART2: a single-member addRecGroup is an explicit group of one', (t) => {
  const m = empty()
  const handles = m.types.addRecGroup([structOf({ type: 'I32' })])
  t.is(handles.length, 1)
  t.is(handles[0].kind, 'Struct')
  // An explicit singleton group is still explicit (distinct from an implicit one).
  t.is(handles[0].isExplicitRecGroup, true)
  t.is(handles[0].recGroupMembers().length, 1)
})

test('PART2: addRecGroup supports a sibling supertype (recIndex) and an existing-type supertype', (t) => {
  const m = empty()
  const existingBase = m.types.addComposite(
    { type: 'Struct', fields: [{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: false }] },
    false,
  )
  // member 0: non-final base; member 1: extends member 0 (sibling supertype);
  // member 2: extends the existing base (existing supertype).
  const members: RecGroupMember[] = [
    {
      composite: { type: 'Struct', fields: [{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: false }] },
      isFinal: false,
    },
    {
      composite: {
        type: 'Struct',
        fields: [
          { storage: { type: 'Val', value: { type: 'I32' } }, mutable: false },
          { storage: { type: 'Val', value: { type: 'F64' } }, mutable: false },
        ],
      },
      isFinal: true,
      supertype: { type: 'RecGroup', recIndex: 0 },
    },
    {
      composite: {
        type: 'Struct',
        fields: [
          { storage: { type: 'Val', value: { type: 'I32' } }, mutable: false },
          { storage: { type: 'Val', value: { type: 'F32' } }, mutable: false },
        ],
      },
      isFinal: true,
      supertype: { type: 'Existing', typeIndex: existingBase.index },
    },
  ]
  const [m0, m1, m2] = m.types.addRecGroup(members)
  t.is(m1.supertype!.index, m0.index)
  t.is(m2.supertype!.index, existingBase.index)
})

// ---------------------------------------------------------------------------
// PART 2 negatives — every rejection is CATCHABLE (no process abort)
// ---------------------------------------------------------------------------

test('PART2 neg: a member field recIndex >= count throws catchably, module unchanged', (t) => {
  const m = empty()
  // count == 1, recIndex 1 is out of range.
  const err = t.throws(() => m.types.addRecGroup([structOf(recGroupRef(1))]))
  t.regex(err!.message, /recIndex 1 is out of range|out of range/)
  t.is(m.types.length, 0)
  // Process alive + module usable.
  t.notThrows(() => m.types.addStruct([{ storage: { type: 'I8' }, mutable: false }]))
})

test('PART2 neg: a supertype recIndex >= count throws catchably', (t) => {
  const m = empty()
  const err = t.throws(() =>
    m.types.addRecGroup([
      {
        composite: { type: 'Struct', fields: [{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: false }] },
        isFinal: false,
        supertype: { type: 'RecGroup', recIndex: 5 },
      },
    ]),
  )
  t.regex(err!.message, /out of range/)
  t.is(m.types.length, 0)
})

test('PART2 neg: a member ref to a nonexistent existing type index throws catchably', (t) => {
  const m = empty()
  const err = t.throws(() => m.types.addRecGroup([structOf(concreteRef(9999))]))
  t.regex(err!.message, /no type at index 9999/)
  t.is(m.types.length, 0)
})

test('PART2: an empty members array returns [] (mirror-walrus), module unchanged', (t) => {
  const m = empty()
  const handles = m.types.addRecGroup([])
  t.deepEqual(handles, [])
  t.is(m.types.length, 0)
  // Emitting an empty explicit rec group is still valid.
  t.notThrows(() => reparseGc(m.emitWasm(false)))
})

// ---------------------------------------------------------------------------
// PART 2 — the RecGroup heap variant is rejected everywhere EXCEPT addRecGroup
// ---------------------------------------------------------------------------

test('the RecGroup heap variant is rejected outside addRecGroup (catchably, never reaches walrus)', (t) => {
  const m = empty()

  // globals.addLocal
  t.regex(
    t.throws(() => m.globals.addLocal(recGroupRef(0), false, false, ConstExpr.i32(0)))!.message,
    /addRecGroup/,
  )
  // buildFunction (param signature conversion rejects it before any mutation)
  t.regex(t.throws(() => m.buildFunction([recGroupRef(0)], [], [], []))!.message, /addRecGroup/)
  // ConstExpr.refNull (pure, no module access)
  t.regex(t.throws(() => ConstExpr.refNull({ type: 'RecGroup', recIndex: 0 }))!.message, /addRecGroup/)

  // The process is alive and the module untouched.
  t.is(m.types.length, 0)
})

// ---------------------------------------------------------------------------
// PART 3 — WasmType.refNull
// ---------------------------------------------------------------------------

test('PART3: WasmType.refNull builds a concrete ref.null usable as a (ref null $s) global init, round-trips', (t) => {
  const m = empty()
  const s = m.types.addStruct([{ storage: { type: 'Val', value: { type: 'I32' } }, mutable: true }])
  const init = s.refNull()
  t.is(init.kind, 'RefNull')

  const g = m.globals.addLocal(concreteRef(s.index), false, false, init)
  t.deepEqual(g.ty, concreteRef(s.index))
  t.is(g.init()!.kind, 'RefNull')

  const reparsed = reparseGc(m.emitWasm(false))
  t.is(reparsed.globals.length, 1)
  t.is(reparsed.globals.getByIndex(0)!.init()!.kind, 'RefNull')
})

test('PART3: WasmType.refNull on a deleted handle throws catchably (no abort)', (t) => {
  const m = empty()
  const s = m.types.addStruct([{ storage: { type: 'I8' }, mutable: false }])
  m.types.delete(s)
  t.regex(t.throws(() => s.refNull())!.message, /deleted/)
})
