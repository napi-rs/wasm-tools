import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ConstExpr, WasmModule, type InstrDesc, type ValType } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// The canonical 8-byte empty module: valid header, zero sections. Building fresh
// instances from it keeps the suite hermetic (no CLI at runtime).
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
const empty = () => WasmModule.fromBuffer(EMPTY_MODULE)

// Committed, pre-compiled fixture (see fixtures/ir.wat, built with
// `wat2wasm --debug-names`). One local function exercising the full C1a subset,
// produced independently of our own buildFunction so the read-walk is checked
// against real walrus IR.
const IR_FIXTURE = readFileSync(join(__dirname, 'fixtures', 'ir.wasm'))

const I32: ValType = { type: 'I32' }
const I64: ValType = { type: 'I64' }
const F32: ValType = { type: 'F32' }

// Build the supporting items (an imported callee at func 0, a global at 0, a
// multi-value block type, and the three locals) that the comprehensive body
// below references. Returns their stable indices. Using an IMPORTED callee is
// deliberate: imports keep a stable function index across emit (walrus size-sorts
// only LOCAL functions), so `Call`'s target index round-trips.
function setup(m: WasmModule) {
  const calleeSig = m.types.add([], [I32])
  const callee = m.imports.addFunction('env', 'callee', calleeSig)
  const g = m.globals.addLocal(I32, true, false, ConstExpr.i32(0))
  const mv = m.types.add([I32], [I32, I32])
  const p0 = m.locals.add(I32)
  const p1 = m.locals.add(I64)
  const l0 = m.locals.add(F32)
  return { callee: callee.index, g: g.index, mv: mv.index, p0: p0.index, p1: p1.index, l0: l0.index }
}

// A body covering every C1a instruction: all four const kinds, Drop, local
// get/set/tee, global get/set, Call, typed + plain Select, a single-value block,
// a multi-value block, nested empty blocks with br/br_if, a br_table over three
// blocks, a loop-in-block br, an if/else with unreachable, and return.
function comprehensiveBody(x: ReturnType<typeof setup>): InstrDesc[] {
  return [
    { type: 'Const', value: { type: 'F32', value: 1.5 } },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'F64', value: 2.5 } },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I64', value: 9223372036854775807n } },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 4 } },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'F32', value: 3.5 } },
    { type: 'LocalSet', local: x.l0 },
    { type: 'LocalGet', local: x.l0 },
    { type: 'Drop' },
    { type: 'LocalGet', local: x.p0 },
    { type: 'GlobalSet', global: x.g },
    { type: 'GlobalGet', global: x.g },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 4 } },
    { type: 'LocalTee', local: x.p0 },
    { type: 'Drop' },
    { type: 'LocalGet', local: x.p1 },
    { type: 'Drop' },
    { type: 'Call', func: x.callee },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 1 } },
    { type: 'Const', value: { type: 'I32', value: 2 } },
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Select', selectType: I32 },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 1 } },
    { type: 'Const', value: { type: 'I32', value: 2 } },
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Select' },
    { type: 'Drop' },
    {
      type: 'Block',
      blockType: { type: 'Value', value: I32 },
      seq: [{ type: 'Const', value: { type: 'I32', value: 20 } }],
    },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 21 } },
    {
      type: 'Block',
      blockType: { type: 'MultiValue', typeIndex: x.mv },
      seq: [{ type: 'Const', value: { type: 'I32', value: 22 } }],
    },
    { type: 'Drop' },
    { type: 'Drop' },
    {
      type: 'Block',
      blockType: { type: 'Empty' },
      seq: [
        {
          type: 'Block',
          blockType: { type: 'Empty' },
          seq: [
            { type: 'Const', value: { type: 'I32', value: 0 } },
            { type: 'BrIf', label: 0 },
            { type: 'Br', label: 1 },
          ],
        },
      ],
    },
    {
      type: 'Block',
      blockType: { type: 'Empty' },
      seq: [
        {
          type: 'Block',
          blockType: { type: 'Empty' },
          seq: [
            {
              type: 'Block',
              blockType: { type: 'Empty' },
              seq: [
                { type: 'Const', value: { type: 'I32', value: 0 } },
                { type: 'BrTable', labels: [0, 1], defaultLabel: 2 },
              ],
            },
          ],
        },
      ],
    },
    {
      type: 'Block',
      blockType: { type: 'Empty' },
      seq: [{ type: 'Loop', blockType: { type: 'Empty' }, seq: [{ type: 'Br', label: 1 }] }],
    },
    { type: 'Const', value: { type: 'I32', value: 1 } },
    {
      type: 'IfElse',
      blockType: { type: 'Value', value: I32 },
      consequent: [{ type: 'Const', value: { type: 'I32', value: 40 } }],
      alternative: [{ type: 'Unreachable' }],
    },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 99 } },
    { type: 'Return' },
  ]
}

// Build the comprehensive main into a fresh module, name it, and return the
// module. The supporting items are created identically every time, so the
// canonical (post-emit) indices match the build-time indices — the round-trip's
// key stability property.
function buildComprehensive(body?: InstrDesc[]): WasmModule {
  const m = empty()
  const x = setup(m)
  const idx = m.buildFunction([I32, I64], [I32], [x.p0, x.p1], body ?? comprehensiveBody(x))
  m.functions.getByIndex(idx)!.name = 'main'
  return m
}

const readMain = (bytes: Uint8Array) => WasmModule.fromBuffer(bytes).functions.byName('main')!.instructions()

test('FLAGSHIP: parse -> instructions -> buildFunction -> emit -> re-parse -> instructions is structurally stable', (t) => {
  // First build: descriptor literal -> emit -> the canonical descriptor array.
  const bytes1 = buildComprehensive().emitWasm(false)
  const read1 = readMain(bytes1)

  // The emitted module is well-typed wasm.
  t.true(WebAssembly.validate(bytes1))

  // Feed that exact descriptor array straight back into buildFunction, emit, and
  // read again. The two descriptor arrays must be deeply equal.
  const bytes2 = buildComprehensive(read1).emitWasm(false)
  const read2 = readMain(bytes2)

  t.true(WebAssembly.validate(bytes2))
  t.deepEqual(read1, read2)

  // Sanity: the round-trip actually exercised the whole subset, not an empty body.
  // Collect instruction kinds recursively (control-flow bodies nest).
  const kinds = new Set<string>()
  const collect = (ds: InstrDesc[]) => {
    for (const d of ds) {
      kinds.add(d.type)
      if (d.seq) collect(d.seq)
      if (d.consequent) collect(d.consequent)
      if (d.alternative) collect(d.alternative)
    }
  }
  collect(read1)
  for (const k of [
    'Const',
    'LocalGet',
    'LocalSet',
    'LocalTee',
    'GlobalGet',
    'GlobalSet',
    'Call',
    'Select',
    'Block',
    'Loop',
    'IfElse',
    'Br',
    'BrIf',
    'BrTable',
    'Drop',
    'Return',
    'Unreachable',
  ]) {
    t.true(kinds.has(k), `round-trip body should contain a ${k}`)
  }
})

test('buildFunction basics: a trivial (param i32)(result i32) local.get 0 emits, re-parses, and reads back', (t) => {
  const m = empty()
  const p0 = m.locals.add(I32)
  const idx = m.buildFunction([I32], [I32], [p0.index], [{ type: 'LocalGet', local: p0.index }])

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const f = reparsed.functions.getByIndex(idx)!
  t.is(f.kind, 'Local')
  t.deepEqual(f.ty().params(), [I32])
  t.deepEqual(f.ty().results(), [I32])
  t.deepEqual(f.instructions(), [{ type: 'LocalGet', local: 0 }])
})

test('instructions() reads a real wat2wasm-produced fixture', (t) => {
  const m = WasmModule.fromBuffer(IR_FIXTURE)
  const main = m.functions.byName('main')!
  t.is(main.kind, 'Local')
  const instrs = main.instructions()

  // Spot-check the structural facts (independent of our own buildFunction).
  const call = instrs.find((d) => d.type === 'Call')!
  t.truthy(call)
  const typedSelect = instrs.find((d) => d.type === 'Select' && d.selectType != null)
  const plainSelect = instrs.find((d) => d.type === 'Select' && d.selectType == null)
  t.truthy(typedSelect)
  t.truthy(plainSelect)
  t.deepEqual(typedSelect!.selectType, I32)

  // The single-value block reads its result type; the multi-value block reads a
  // MultiValue block type with a type index.
  const valueBlock = instrs.find((d) => d.type === 'Block' && d.blockType?.type === 'Value')!
  t.deepEqual(valueBlock.blockType, { type: 'Value', value: I32 })
  const mvBlock = instrs.find((d) => d.type === 'Block' && d.blockType?.type === 'MultiValue')!
  t.is(mvBlock.blockType!.type, 'MultiValue')

  // The i64 const survives as an exact bigint.
  const i64Const = instrs.find((d) => d.type === 'Const' && d.value?.type === 'I64')!
  t.is((i64Const.value as { type: 'I64'; value: bigint }).value, 9223372036854775807n)
})

test('branch depth: block(block(br 1)) round-trips with the correct relative depths', (t) => {
  const m = empty()
  // outer block { inner block { br 1 } }  -- br 1 targets the OUTER block.
  const body: InstrDesc[] = [
    {
      type: 'Block',
      blockType: { type: 'Empty' },
      seq: [{ type: 'Block', blockType: { type: 'Empty' }, seq: [{ type: 'Br', label: 1 }] }],
    },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'bt'

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const read = reparsed.functions.byName('bt')!.instructions()
  t.deepEqual(read, body)

  // The inner br really is depth 1 (targets the outer), not 0.
  const outer = read[0]
  const inner = outer.seq![0]
  t.is(inner.seq![0].type, 'Br')
  t.is(inner.seq![0].label, 1)
})

test('guard: a body referencing an out-of-range local/global/func index throws catchably', (t) => {
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'LocalGet', local: 99 }]), {
    message: /no local at index 99/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'GlobalGet', global: 99 }]), {
    message: /no global at index 99/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Call', func: 99 }]), {
    message: /no function at index 99/,
  })
  // process is still alive after the catchable throws.
  t.is(1 + 1, 2)
})

test('guard: a bad argLocalIndices index and a bad multi-value block type index throw catchably', (t) => {
  t.throws(() => empty().buildFunction([I32], [], [42], []), { message: /no local at index 42/ })
  t.throws(
    () =>
      empty().buildFunction([], [], [], [{ type: 'Block', blockType: { type: 'MultiValue', typeIndex: 77 }, seq: [] }]),
    { message: /no type at index 77/ },
  )
})

test('guard: a branch label deeper than the enclosing blocks throws catchably (no abort)', (t) => {
  // No enclosing block at all: only the function entry frame exists (depth 0), so
  // label 5 is out of range.
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Br', label: 5 }]), {
    message: /branch label depth 5 is out of range/,
  })
  // A br_table default that is too deep is rejected too.
  t.throws(
    () =>
      empty().buildFunction(
        [],
        [],
        [],
        [{ type: 'Block', blockType: { type: 'Empty' }, seq: [{ type: 'BrTable', labels: [0], defaultLabel: 9 }] }],
      ),
    { message: /out of range/ },
  )
})

test('guard: instructions() on an imported function throws catchably', (t) => {
  const m = empty()
  const sig = m.types.add([], [])
  const imported = m.imports.addFunction('env', 'f', sig)
  t.throws(() => imported.instructions(), { message: /cannot read instructions of a non-local function/ })
  t.is(1 + 1, 2)
})

test('i64 BigInt: a full-width i64 const round-trips exactly (min and max)', (t) => {
  const m = empty()
  const MAX = 9223372036854775807n // 2^63 - 1
  const MIN = -9223372036854775808n // -2^63
  const idx = m.buildFunction(
    [],
    [],
    [],
    [
      { type: 'Const', value: { type: 'I64', value: MAX } },
      { type: 'Drop' },
      { type: 'Const', value: { type: 'I64', value: MIN } },
      { type: 'Drop' },
    ],
  )
  const read = WasmModule.fromBuffer(m.emitWasm(false)).functions.getByIndex(idx)!.instructions()
  t.deepEqual(read[0].value, { type: 'I64', value: MAX })
  t.deepEqual(read[2].value, { type: 'I64', value: MIN })
})

test('i64 BigInt: a value that does not fit losslessly in a signed i64 throws', (t) => {
  const tooBig = 1n << 70n // needs 71 bits
  t.throws(
    () =>
      empty().buildFunction([], [], [], [{ type: 'Const', value: { type: 'I64', value: tooBig } }, { type: 'Drop' }]),
    {
      message: /does not fit losslessly/,
    },
  )
})
