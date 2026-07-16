import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import {
  ConstExpr,
  ModuleConfig,
  WasmModule,
  type AtomicOp,
  type AtomicWidth,
  type InstrDesc,
  type LoadKind,
  type LoadSimdKind,
  type MemArg,
  type StoreKind,
  type ValType,
} from '../index'

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

test('guard: a Block naming its own in-flight signature/entry type index is rejected without aborting, and the module still emits', (t) => {
  const m = empty()
  // `FunctionBuilder::new` would mint this function's signature type at the next
  // raw arena index and its entry type at the one after. A `MultiValue` block
  // naming either is the process-abort vector under `panic = abort` (emit skips
  // the entry-type rec group, so `get_type_index` traps). The pre-call preflight
  // must reject both catchably because neither index exists in the arena yet.
  const T = m.types.length
  t.throws(
    () => m.buildFunction([], [], [], [{ type: 'Block', blockType: { type: 'MultiValue', typeIndex: T }, seq: [] }]),
    { message: new RegExp(`no type at index ${T}`) },
  )
  // The entry type (T + 1) is the one emit actually skips — the real landmine.
  t.throws(
    () =>
      m.buildFunction([], [], [], [{ type: 'Block', blockType: { type: 'MultiValue', typeIndex: T + 1 }, seq: [] }]),
    { message: new RegExp(`no type at index ${T + 1}`) },
  )
  // Process is still alive after the catchable throws (a real abort would have
  // taken the whole test run down — this line is the proof under WASI)...
  t.is(1 + 1, 2)
  // ...and the module was never mutated, so it still emits cleanly.
  t.notThrows(() => m.emitWasm(false))
})

test('guard: a body that fails late leaves the type arena unchanged, and a later valid build/emit still works', (t) => {
  const m = empty()
  const before = m.types.length

  // Valid prefix (Const i32) then a bad local index: with the old ordering this
  // failed only AFTER FunctionBuilder::new had already inserted the signature +
  // entry types, orphaning them in the arena. The preflight must fail first, so
  // the arena is untouched.
  t.throws(
    () =>
      m.buildFunction(
        [],
        [],
        [],
        [
          { type: 'Const', value: { type: 'I32', value: 1 } },
          { type: 'LocalGet', local: 99 },
        ],
      ),
    { message: /no local at index 99/ },
  )
  t.is(m.types.length, before)

  // A bad MultiValue index nested inside an otherwise-valid Block also fails late
  // but must leave the arena unchanged.
  t.throws(
    () =>
      m.buildFunction(
        [],
        [],
        [],
        [
          { type: 'Const', value: { type: 'I32', value: 0 } },
          { type: 'Drop' },
          { type: 'Block', blockType: { type: 'MultiValue', typeIndex: 123 }, seq: [] },
        ],
      ),
    { message: /no type at index 123/ },
  )
  t.is(m.types.length, before)

  // No newly-visible (orphan) type leaked from either failure.
  t.is(m.types.length, before)

  // The module is still fully usable: a normal valid function builds and emits.
  const p0 = m.locals.add(I32)
  const idx = m.buildFunction([I32], [I32], [p0.index], [{ type: 'LocalGet', local: p0.index }])
  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))
  const reparsed = WasmModule.fromBuffer(bytes)
  t.deepEqual(reparsed.functions.getByIndex(idx)!.instructions(), [{ type: 'LocalGet', local: 0 }])
})

// ---------------------------------------------------------------------------
// C1a-fix2: the nesting-depth cap that prevents an uncatchable stack-overflow
// abort. The three in-module instruction walks (buildFunction preflight + emit,
// and the instructions() read) recurse once per control-flow level; walrus
// itself is fully ITERATIVE and imposes no nesting limit, so without a cap a
// deep body (build) or a deep parsed module (read) would overflow the native
// stack — a SIGABRT that catch_unwind cannot catch, tearing down the whole Node
// process across FFI. MAX_NESTING_DEPTH converts that into a catchable error at
// the cap. Since CH, the FFI marshalling of the descriptor tree is ITERATIVE
// too (src/ir_marshal.rs) with the guard enforced during the JS→Rust decode
// itself, so the guard message is deterministic at ANY over-cap depth on every
// target and harness (see the CH tests below for the far-over-cap proof).
// The wasm32-wasi run is the real proof: a genuine overflow there aborts the
// whole run, so a passing wasi run is the evidence the abort is gone.
// ---------------------------------------------------------------------------

// Must match src/ir.rs::MAX_NESTING_DEPTH.
const MAX_NESTING_DEPTH = 250

// `n` nested empty Blocks: [Block{ seq: [Block{ seq: [ … [] ] }] }]. The read
// walk produces the identical shape, so this doubles as the deep-equal oracle.
// A body of `n` nested Blocks puts its innermost (empty) sequence at depth n + 1,
// so it builds iff n + 1 <= cap, i.e. n <= cap - 1.
function nestedBlocks(n: number): InstrDesc[] {
  let seq: InstrDesc[] = []
  for (let i = 0; i < n; i++) {
    seq = [{ type: 'Block', blockType: { type: 'Empty' }, seq }]
  }
  return seq
}

// Hand-craft a minimal module with one () -> () function whose body is `b` nested
// empty blocks (`block … block end … end`). walrus parses this iteratively at any
// depth, so it lets us feed a deeper-than-cap function straight to the read walk
// without going through the (guarded) builder. Fully hermetic — no wat2wasm.
function deepBlockModule(b: number): Uint8Array {
  const leb = (n: number): number[] => {
    const out: number[] = []
    do {
      let byte = n & 0x7f
      n >>>= 7
      if (n !== 0) byte |= 0x80
      out.push(byte)
    } while (n !== 0)
    return out
  }
  const section = (id: number, payload: number[]): number[] => [id, ...leb(payload.length), ...payload]

  const body: number[] = [0x00] // zero local declarations
  for (let i = 0; i < b; i++) body.push(0x02, 0x40) // `block` with empty block type
  for (let i = 0; i < b + 1; i++) body.push(0x0b) // `b` block `end`s + the function `end`

  const typeSec = section(0x01, [0x01, 0x60, 0x00, 0x00]) // one () -> ()
  const funcSec = section(0x03, [0x01, 0x00]) // one function, type index 0
  const codeSec = section(0x0a, [0x01, ...leb(body.length), ...body]) // one function body

  return new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, ...typeSec, ...funcSec, ...codeSec])
}

test('C1a-fix2 (build): an over-cap body throws catchably, the process survives, and the module still emits', (t) => {
  const m = empty()
  const over = nestedBlocks(MAX_NESTING_DEPTH + 5) // 255 nested Blocks — past the cap
  // Since CH the guard fires DURING the iterative JS→Rust decode, so this exact
  // message is guaranteed on every target/harness (before CH, the derived
  // recursive marshalling could exhaust the stack first at harness-dependent
  // depths, making the reachable message environment-dependent).
  t.throws(() => m.buildFunction([], [], [], over), {
    message: /instruction nesting too deep \(max 250\)/,
  })
  // No abort: control returned to the test. The all-or-nothing preflight left the
  // module untouched, so it still emits a valid (empty) module.
  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))
})

test('C1a-fix2 (read): a parsed module nested past the cap is read-bounded — instructions() throws catchably (no abort)', (t) => {
  // walrus parses arbitrarily-deep nesting iteratively (no limit), so hand-craft a
  // module whose one function nests past the cap. The read walk must refuse it with
  // a catchable error instead of overflowing the stack.
  const m = WasmModule.fromBuffer(deepBlockModule(MAX_NESTING_DEPTH + 5))
  const f = m.functions.getByIndex(0)!
  t.throws(() => f.instructions(), {
    message: /instruction nesting too deep \(max 250\)/,
  })
  // Process survived the guarded read.
  t.is(1 + 1, 2)
})

test('C1a-fix2 (round-trip): a body nested at exactly the cap builds → emits → re-parses → instructions deep-equals', (t) => {
  const m = empty()
  // cap - 1 nested Blocks => the innermost sequence sits at depth == cap, the
  // deepest level the guard allows. This proves the chosen N is actually reachable
  // for BOTH build and read — the whole napi round trip (JS→Rust marshalling,
  // validate/emit, walrus emit, read, Rust→JS marshalling, Drop) survives at the
  // cap on native and wasi alike.
  const body = nestedBlocks(MAX_NESTING_DEPTH - 1)
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'deep'

  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))
  const read = WasmModule.fromBuffer(bytes).functions.byName('deep')!.instructions()
  t.deepEqual(read, body)
})

// ---------------------------------------------------------------------------
// CH: iterative descriptor marshalling. The InstrDesc tree crosses the FFI via
// explicit heap work-stacks (src/ir_marshal.rs) in BOTH directions, with the
// nesting guard enforced DURING the JS→Rust decode. Before CH, napi's DERIVED
// marshalling recursed once per level on the call stack, so a deep body could
// exhaust the stack BEFORE the guard ran: native Node SIGSEGVed (uncatchable)
// at ≈740 levels, and under the AVA-wasi harness the ceiling sat at EXACTLY the
// at-cap canary's depth — the guard was unreachable from the over-cap side
// there at ANY depth. These tests are the class-killer proof: any over-cap
// depth — minimal (cap+1) or far past every old crash ceiling (2000) — now
// yields the deterministic catchable guard error on native and wasi alike.
// ---------------------------------------------------------------------------

test('CH (far-over-cap): a 2000-deep body throws the exact guard error — not a stack-overflow RangeError — and the module still works', (t) => {
  const m = empty()
  // 2000 nested Blocks: far past the old native decode SIGSEGV (≈740), the old
  // native encode V8 fatal (≈525), and the old AVA-wasi RangeError ceiling
  // (≈250). Reaching the guard message here proves the decode never recurses.
  const err = t.throws(() => m.buildFunction([], [], [], nestedBlocks(2000)))!
  t.is(err.message, 'instruction nesting too deep (max 250); refusing to recurse to avoid a stack overflow')
  // The process survived and the (untouched) module still emits.
  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))
})

test('CH (minimal over-cap): a cap+1 body reaches the guard deterministically', (t) => {
  const m = empty()
  // 251 nested Blocks put the innermost sequence at depth 252 — one level past
  // the deepest accepted body. The exact guard message (not a RangeError, not a
  // harness-dependent crash) is what the decode-integrated guard guarantees.
  const err = t.throws(() => m.buildFunction([], [], [], nestedBlocks(MAX_NESTING_DEPTH + 1)))!
  t.is(err.message, 'instruction nesting too deep (max 250); refusing to recurse to avoid a stack overflow')
})

// `n` levels of IfElse nesting, alternating which arm carries the next level
// (each arm is its own label frame, hence its own depth level — same as a
// Block's seq). The other arm stays a present-but-empty array. This exercises
// the consequent/alternative edges of the iterative driver, not just seq.
function nestedIfArms(n: number): InstrDesc[] {
  let inner: InstrDesc[] = []
  for (let i = 0; i < n; i++) {
    const body: InstrDesc = { type: 'IfElse', blockType: { type: 'Empty' } }
    if (i % 2 === 0) {
      body.consequent = inner
      body.alternative = []
    } else {
      body.consequent = []
      body.alternative = inner
    }
    inner = [{ type: 'Const', value: { type: 'I32', value: 0 } }, body]
  }
  return inner
}

test('CH (multi-edge depth): at-cap IfElse arm nesting round-trips; over-cap throws the guard', (t) => {
  const m = empty()
  // cap - 1 IfElse levels => the innermost (empty) arm sits at depth == cap,
  // exactly like the at-cap Block canary — but the depth chain alternates
  // through consequent and alternative, proving the driver counts EVERY edge
  // kind. In-memory read-back (no re-parse: the body is emitted as-is and this
  // shape is ill-typed at the wasm level, which MIRROR-WALRUS permits).
  const body = nestedIfArms(MAX_NESTING_DEPTH - 1)
  const idx = m.buildFunction([], [], [], body)
  t.deepEqual(m.functions.getByIndex(idx)!.instructions(), body)

  const err = t.throws(() => m.buildFunction([], [], [], nestedIfArms(MAX_NESTING_DEPTH + 1)))!
  t.is(err.message, 'instruction nesting too deep (max 250); refusing to recurse to avoid a stack overflow')
})

test('CH (edge fidelity): `seq: []` and absent seq stay distinct through the round trip; a non-array seq throws catchably', (t) => {
  const m = empty()
  // One descriptor with a PRESENT-but-empty seq next to edge-free leaves: the
  // read-back must keep `seq: []` on the Block (Some(vec![])) and NO seq/
  // consequent/alternative keys on the leaves (None => absent property).
  const body: InstrDesc[] = [
    { type: 'Block', blockType: { type: 'Empty' }, seq: [] },
    { type: 'Const', value: { type: 'I32', value: 7 } },
    { type: 'Drop' },
  ]
  const idx = m.buildFunction([], [], [], body)
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)
  t.true(Object.hasOwn(read[0], 'seq'))
  t.deepEqual(read[0].seq, [])
  for (const key of ['seq', 'consequent', 'alternative']) {
    t.false(Object.hasOwn(read[1], key))
    t.false(Object.hasOwn(read[2], key))
  }

  // A non-array edge is a catchable type error (same rejection point the
  // derived decode had), not an abort.
  t.throws(
    () =>
      m.buildFunction([], [], [], [
        { type: 'Block', blockType: { type: 'Empty' }, seq: 42 as unknown as InstrDesc[] },
      ]),
    { message: /Failed to get Array length on InstrDesc\.seq/ },
  )
})

// ---------------------------------------------------------------------------
// CH-fix: two adversarial `buildFunction(body)` inputs that, before the fix,
// STILL reached an UNCATCHABLE process abort inside the JS→Rust decode —
// defeating CH's whole purpose. Both must now fail catchably (or ignore the
// hostile input) with the process intact, on native AND wasm32-wasi (a genuine
// abort under WASI `panic=abort` tears down the whole test run).
//
//   1. A prototype-chain edge (`Object.prototype.seq` etc.) fed the DERIVED
//      per-field read — a prototype-traversing `[[Get]]` — an inherited array it
//      recursed into (`Vec<InstrDesc>::from_napi_value`) UNBOUNDED on the native
//      call stack, BEFORE the driver's depth guard. Fix: the decode shadows the
//      three edges as own `undefined` and the driver walks OWN edges only, so an
//      inherited edge is ignored on any prototype chain.
//   2. An untrusted `Array.length` (a sparse array with `length ≈ 2**32`) sized
//      `Vec::with_capacity`, which aborts on capacity overflow / alloc failure.
//      Fix: the vecs grow from the ACTUAL element count, so a sparse array fails
//      catchably on its first hole instead.
// ---------------------------------------------------------------------------

test('CH-fix (inherited-edge): a polluted `Object.prototype` edge is ignored (treated as None) — builds, round-trips, no abort', (t) => {
  // Run each edge in its own save/restore so a thrown assertion can never leak
  // the pollution into the rest of the suite (a leaked `Object.prototype.seq`
  // would read as unrelated failures everywhere).
  for (const edge of ['seq', 'consequent', 'alternative'] as const) {
    const saved = Object.getOwnPropertyDescriptor(Object.prototype, edge)
    t.is(saved, undefined) // sanity: nothing owned this key before us
    try {
      // Self-referential poison: the element is a valid leaf that ALSO inherits
      // this same edge, so the PRE-fix prototype-traversing derived read would
      // recurse into `Vec<InstrDesc>::from_napi_value` UNBOUNDED (uncatchable
      // native stack overflow). `enumerable: false` keeps it invisible to
      // `Object.keys`/for-in (so it cannot perturb deepEqual) while a `[[Get]]`
      // still sees it — exactly the vulnerable path.
      const poison = [{ type: 'Drop' }]
      Object.defineProperty(Object.prototype, edge, {
        value: poison,
        writable: true,
        configurable: true,
        enumerable: false,
      })

      const m = empty()
      const body: InstrDesc[] = [{ type: 'Const', value: { type: 'I32', value: 7 } }, { type: 'Drop' }]
      const idx = m.buildFunction([], [], [], body)
      const read = m.functions.getByIndex(idx)!.instructions()
      // The inherited edge was ignored (treated as None): plain round-trip, and
      // no leaf gained an own `seq`/`consequent`/`alternative` from the pollution.
      t.deepEqual(read, body)
      for (const leaf of read) {
        t.false(Object.hasOwn(leaf, edge))
      }
      // The process survived and the module still emits valid wasm.
      t.true(WebAssembly.validate(m.emitWasm(false)))
    } finally {
      // saved is always undefined here, so this deletes the injected key.
      if (saved) Object.defineProperty(Object.prototype, edge, saved)
      else delete (Object.prototype as Record<string, unknown>)[edge]
    }
    // The pollution is fully gone — no later test can observe it.
    t.false(edge in Object.prototype)
  }
})

test('CH-fix (__proto__ injection): an own `__proto__` data property carrying a CYCLIC edge cannot retarget the copy or recurse', (t) => {
  // An OWN data property literally named "__proto__" (defineProperty does NOT
  // invoke the prototype setter) whose value carries an edge. Before the fix the
  // copy loop's `napi_set_property(copy, "__proto__", …)` would run the accessor
  // and retarget the copy's prototype to this edge-bearing object; the fix
  // enumerates OWN props, skips "__proto__", and writes via a data descriptor.
  //
  // REGRESSION SENSITIVITY: the smuggled `seq` is SELF-REFERENTIAL (cyclic), so
  // if the copy's prototype were retargeted to it and the derived read walked it,
  // the derived `Vec<InstrDesc>::from_napi_value` would recurse into `cyc` FOREVER
  // (uncatchable SIGSEGV) — the exact pre-fix (af90219) failure. Post-fix the
  // "__proto__" key is skipped (copy keeps `Object.prototype`) and the edge read
  // is an own-`undefined` shadow / own-only walk, so the cyclic edge is ignored
  // (→ `None`); a finite `[{ type: 'Drop' }]` here would pass even UNFIXED and
  // prove nothing.
  const cyc: unknown[] = []
  cyc.push({ type: 'Block', blockType: { type: 'Empty' }, seq: cyc })
  const evil = { type: 'Drop' } as Record<string, unknown>
  Object.defineProperty(evil, '__proto__', {
    value: { seq: cyc },
    writable: true,
    configurable: true,
    enumerable: true,
  })
  // Sanity: it is an OWN property and evil's REAL prototype is untouched.
  t.true(Object.hasOwn(evil, '__proto__'))
  t.is(Object.getPrototypeOf(evil), Object.prototype)

  const m = empty()
  const body = [{ type: 'Const', value: { type: 'I32', value: 1 } }, evil] as unknown as InstrDesc[]
  const idx = m.buildFunction([], [], [], body)
  const read = m.functions.getByIndex(idx)!.instructions()
  // Neutralized: the leaf is a plain Drop with no smuggled edge, and the module
  // still emits — the process did not recurse or abort.
  t.deepEqual(read, [{ type: 'Const', value: { type: 'I32', value: 1 } }, { type: 'Drop' }])
  t.false(Object.hasOwn(read[1], 'seq'))
  t.true(WebAssembly.validate(m.emitWasm(false)))
})

test('CH-fix (sparse-wide body): a 2**32-1-length sparse body throws catchably (no huge alloc/abort), module survives', (t) => {
  const m = empty()
  // A sparse array reports a near-`2**32` length with NO real elements. Before
  // the fix `Vec::with_capacity(len)` requested billions of `InstrDesc` slots →
  // capacity-overflow panic / `handle_alloc_error` → uncatchable abort.
  const body: unknown[] = []
  body.length = 2 ** 32 - 1
  t.throws(() => m.buildFunction([], [], [], body as InstrDesc[]))
  // The process survived: the (untouched) module still emits valid wasm.
  t.true(WebAssembly.validate(m.emitWasm(false)))
})

test('CH-fix (sparse-wide edge): a 2**32-1-length sparse `seq` throws catchably, module survives', (t) => {
  const m = empty()
  const sparse: unknown[] = []
  sparse.length = 2 ** 32 - 1
  const body = [{ type: 'Block', blockType: { type: 'Empty' }, seq: sparse }] as unknown as InstrDesc[]
  t.throws(() => m.buildFunction([], [], [], body))
  t.true(WebAssembly.validate(m.emitWasm(false)))
})

// CH-fix2: `labels` (a `BrTable`'s target list, `Option<Vec<u32>>`) is NOT a
// recursive edge, so it flowed through the DERIVED per-element decode — whose
// `Vec::<u32>::from_napi_value` calls `Vec::with_capacity(labels.length)` BEFORE
// inspecting any element. A sparse `labels.length ≈ 2**32` therefore requested
// billions of slots → capacity-overflow panic / `handle_alloc_error` → the same
// UNCATCHABLE abort class CH exists to remove (fatal under WASI `panic=abort`).
// The fix shadows `labels` as own `undefined` (so the derived read yields `None`
// on any prototype) and decodes the OWN `labels` with a non-preallocating loop.
test('CH-fix2 (sparse-wide labels, own): a 2**32-1-length sparse own `labels` throws catchably (no huge alloc/abort), module survives', (t) => {
  const m = empty()
  const sparse: unknown[] = []
  sparse.length = 2 ** 32 - 1
  // The leaf decode grows from the ACTUAL elements: it hits the first hole
  // immediately and fails with a catchable `u32` conversion error decorated
  // `on InstrDesc.labels` — never a `Vec::with_capacity(2**32-1)` abort.
  const body = [{ type: 'BrTable', labels: sparse, defaultLabel: 0 }] as unknown as InstrDesc[]
  t.throws(() => m.buildFunction([], [], [], body))
  // The process survived: the (untouched) module still emits valid wasm.
  t.true(WebAssembly.validate(m.emitWasm(false)))
})

test('CH-fix2 (sparse-wide labels, inherited): a polluted `Object.prototype.labels` is ignored — throws catchably, no abort, module survives', (t) => {
  const saved = Object.getOwnPropertyDescriptor(Object.prototype, 'labels')
  t.is(saved, undefined) // sanity: nothing owned this key before us
  try {
    const sparse: unknown[] = []
    sparse.length = 2 ** 32 - 1
    // Pre-fix the derived read of `labels` was a prototype-traversing `[[Get]]`,
    // so this INHERITED sparse array reached `Vec::with_capacity` and aborted.
    // Post-fix the own-`undefined` shadow makes the derived read `None` and the
    // leaf decode reads OWN `labels` only, so the inherited value is ignored: the
    // `BrTable` ends up with no `labels` and `buildFunction` throws the catchable
    // `BrTable requires labels` error instead of aborting.
    Object.defineProperty(Object.prototype, 'labels', {
      value: sparse,
      writable: true,
      configurable: true,
      enumerable: false,
    })
    const m = empty()
    const body = [{ type: 'BrTable', defaultLabel: 0 }] as unknown as InstrDesc[]
    t.throws(() => m.buildFunction([], [], [], body))
    // The process survived: the module still emits valid wasm.
    t.true(WebAssembly.validate(m.emitWasm(false)))
  } finally {
    // saved is always undefined here, so this deletes the injected key.
    if (saved) Object.defineProperty(Object.prototype, 'labels', saved)
    else delete (Object.prototype as Record<string, unknown>)['labels']
  }
  // The pollution is fully gone — no later test can observe it.
  t.false('labels' in Object.prototype)
})

// CH-fix2: a NORMAL `BrTable` (a real small `labels` array + `defaultLabel`) must
// still decode element-for-element via the leaf path, byte-identical to before.
// Three enclosing blocks so labels 0/1 and default 2 are all valid targets
// (mirrors the FLAGSHIP round-trip's known-good `BrTable` shape).
test('CH-fix2 (normal BrTable): a real small `labels` + `defaultLabel` round-trips byte-identically', (t) => {
  const m = empty()
  const body: InstrDesc[] = [
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
  ]
  const idx = m.buildFunction([], [], [], body)
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)
  t.true(WebAssembly.validate(m.emitWasm(false)))
})

// ---------------------------------------------------------------------------
// C1b: numeric/comparison/conversion operators (Binop / Unop / TernOp).
//
// The three walrus operator enums have 352 FIELDLESS variants in scope
// (BinaryOp 214 + UnaryOp 129 + TernaryOp 9); the 14 lane-carrying SIMD variants
// (`*ReplaceLane` / `*ExtractLane*`) are DEFERRED to the SIMD task (C6).
//
// IMPORTANT walrus fact (verified on disk): walrus 0.26.4 ALWAYS type-checks a
// function body on parse — `strict_validate(false)` is a no-op in this version
// (the `skip_strict_validate` flag is set but never read; the wasmparser
// `FuncValidator::op` runs unconditionally). So an ill-typed body of BARE ops
// (no operands) can never re-parse. The exhaustive round-trip therefore uses the
// IN-MEMORY read path (`buildFunction` -> `instructions()` on the same module),
// which never invokes the parser/validator; a separate WELL-TYPED body proves the
// emit -> bytes -> re-parse path decodes real operator opcodes.
// ---------------------------------------------------------------------------

const FIELDLESS_OPS = JSON.parse(readFileSync(join(__dirname, 'fieldless-ops.json'), 'utf8')) as {
  binop: string[]
  unop: string[]
  ternop: string[]
}

// One bare descriptor per fieldless op, in a stable order (all binops, then all
// unops, then all ternops). Bare = no operands: buildFunction is MIRROR-WALRUS
// and does not type-check, so this ill-typed body still builds and emits.
function allFieldlessOpDescs(): InstrDesc[] {
  return [
    ...FIELDLESS_OPS.binop.map((op): InstrDesc => ({ type: 'Binop', op })),
    ...FIELDLESS_OPS.unop.map((op): InstrDesc => ({ type: 'Unop', op })),
    ...FIELDLESS_OPS.ternop.map((op): InstrDesc => ({ type: 'TernOp', op })),
  ]
}

test('C1b EXHAUSTIVE: all 352 fieldless operators build and read back (in-memory round-trip)', (t) => {
  // Sanity on the committed op list (must match the walrus enum counts).
  t.is(FIELDLESS_OPS.binop.length, 214)
  t.is(FIELDLESS_OPS.unop.length, 129)
  t.is(FIELDLESS_OPS.ternop.length, 9)

  const descs = allFieldlessOpDescs()
  t.is(descs.length, 352)

  const m = empty()
  const idx = m.buildFunction([], [], [], descs)
  // Read back from the SAME in-memory module (no emit/re-parse, so walrus'
  // always-on function-body validator never runs on this ill-typed body).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, descs)
})

// A WELL-TYPED body of stable (non-SIMD) numeric ops: correct operands so the
// emitted module is valid wasm a real engine accepts.
const STABLE_OPS_BODY: InstrDesc[] = [
  { type: 'Const', value: { type: 'I32', value: 1 } },
  { type: 'Const', value: { type: 'I32', value: 2 } },
  { type: 'Binop', op: 'I32Add' },
  { type: 'Drop' },
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I32Eqz' },
  { type: 'Drop' },
  { type: 'Const', value: { type: 'I64', value: 1n } },
  { type: 'Const', value: { type: 'I64', value: 2n } },
  { type: 'Binop', op: 'I64Add' },
  { type: 'Drop' },
  { type: 'Const', value: { type: 'F32', value: 4 } },
  { type: 'Unop', op: 'F32Sqrt' },
  { type: 'Drop' },
  { type: 'Const', value: { type: 'F64', value: 1 } },
  { type: 'Const', value: { type: 'F64', value: 2 } },
  { type: 'Binop', op: 'F64Copysign' },
  { type: 'Drop' },
  { type: 'Const', value: { type: 'I32', value: 1 } },
  { type: 'Unop', op: 'I64ExtendSI32' },
  { type: 'Drop' },
]

test('C1b: a well-typed stable numeric body emits valid wasm and round-trips through re-parse', (t) => {
  const m = empty()
  const idx = m.buildFunction([], [], [], STABLE_OPS_BODY)
  m.functions.getByIndex(idx)!.name = 'stableops'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse and read back: the operator opcodes decode to the same names.
  const read = WasmModule.fromBuffer(bytes).functions.byName('stableops')!.instructions()
  t.deepEqual(read, STABLE_OPS_BODY)
})

// A WELL-TYPED body exercising v128 and relaxed-SIMD operators (all three
// arities). v128 operands are produced by `*.splat` unops (ConstValue has no
// V128 variant yet). WebAssembly.validate is intentionally NOT asserted here:
// relaxed-SIMD validity depends on the host V8's enabled features, whereas the
// walrus re-parse (features on) is deterministic.
const SIMD_OPS_BODY: InstrDesc[] = [
  // v128 unop via splat
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I8x16Splat' },
  { type: 'Drop' },
  // v128 binop
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I8x16Splat' },
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I8x16Splat' },
  { type: 'Binop', op: 'I8x16Eq' },
  { type: 'Drop' },
  // relaxed-SIMD ternop (integer laneselect)
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I8x16Splat' },
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I8x16Splat' },
  { type: 'Const', value: { type: 'I32', value: 0 } },
  { type: 'Unop', op: 'I8x16Splat' },
  { type: 'TernOp', op: 'I8x16RelaxedLaneselect' },
  { type: 'Drop' },
  // relaxed-SIMD ternop (float fused-multiply-add)
  { type: 'Const', value: { type: 'F32', value: 0 } },
  { type: 'Unop', op: 'F32x4Splat' },
  { type: 'Const', value: { type: 'F32', value: 0 } },
  { type: 'Unop', op: 'F32x4Splat' },
  { type: 'Const', value: { type: 'F32', value: 0 } },
  { type: 'Unop', op: 'F32x4Splat' },
  { type: 'TernOp', op: 'F32x4RelaxedMadd' },
  { type: 'Drop' },
]

test('C1b: a well-typed v128/relaxed-SIMD body round-trips through re-parse (features on)', (t) => {
  const m = empty()
  const idx = m.buildFunction([], [], [], SIMD_OPS_BODY)
  m.functions.getByIndex(idx)!.name = 'simdops'
  const bytes = m.emitWasm(false)

  // Re-parse with all proposals enabled (SIMD + relaxed-SIMD opcodes must decode)
  // and validation relaxed, then read the operator names back.
  const reparsed = new ModuleConfig().onlyStableFeatures(false).strictValidate(false).parse(bytes)
  const read = reparsed.functions.byName('simdops')!.instructions()
  t.deepEqual(read, SIMD_OPS_BODY)
})

test('C1b negative: an unknown operator name throws catchably', (t) => {
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Binop', op: 'NotARealOp' }]), {
    message: /unknown binary operator `NotARealOp`/,
  })
  // Process is still alive after the catchable throw.
  t.is(1 + 1, 2)
})

test('C1b negative: a Binop/Unop/TernOp descriptor missing its `op` field throws catchably', (t) => {
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Binop' }]), {
    message: /`Binop` instruction is missing its `op` field/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Unop' }]), {
    message: /`Unop` instruction is missing its `op` field/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'TernOp' }]), {
    message: /`TernOp` instruction is missing its `op` field/,
  })
})

test('C6a: a lane-carrier op name WITHOUT a `lane` is not buildable (rejected catchably)', (t) => {
  // C6a makes the 14 lane-carrying SIMD ops buildable, but ONLY with a `lane`
  // index: omitting it is a catchable representation error (no longer "unknown
  // operator" — the name IS known now, it just needs its lane).
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Unop', op: 'I8x16ExtractLaneS' }]), {
    message: /SIMD lane op `I8x16ExtractLaneS` requires a `lane` index/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Binop', op: 'I8x16ReplaceLane' }]), {
    message: /SIMD lane op `I8x16ReplaceLane` requires a `lane` index/,
  })
})

// A hand-authored module whose one function contains a lane-carrier op:
// (i32.const 0)(i8x16.splat)(i8x16.extract_lane_s 0)(drop). Produced from walrus
// directly, so the read path can be exercised against a genuine lane-carrying
// instruction decoded from real wasm bytes (not just our own emit).
const LANE_CARRIER_MODULE = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0x07,
  0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x0c, 0x01, 0x0a, 0x00, 0x41, 0x00, 0xfd, 0x0f, 0xfd, 0x15, 0x00, 0x1a,
  0x0b,
])

test('C6a: reading a module containing a lane-carrier op yields `op` + `lane` (no longer deferred)', (t) => {
  const m = new ModuleConfig().onlyStableFeatures(false).parse(LANE_CARRIER_MODULE)
  const f = m.functions.getByIndex(0)!
  // The lane-carrying `i8x16.extract_lane_s 0` decodes to op + lane 0.
  t.deepEqual(f.instructions(), [
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Unop', op: 'I8x16Splat' },
    { type: 'Unop', op: 'I8x16ExtractLaneS', lane: 0 },
    { type: 'Drop' },
  ])
})

// ---------------------------------------------------------------------------
// C2: memory instructions (MemorySize/Grow/Init/Fill/Copy, DataDrop) + the
// general Load/Store instructions.
//
// Load/Store carry a MemArg (align + a u64 offset as a bigint) and a
// LoadKind/StoreKind. Same walrus fact as C1b: `strict_validate(false)` is a
// no-op in 0.26.4, so an ill-typed body can never re-parse. The EXHAUSTIVE test
// therefore uses the IN-MEMORY read path (buildFunction -> instructions() on the
// same module, never touching the parser/validator); a separate WELL-TYPED body
// proves the ops survive the emit -> bytes -> re-parse boundary.
//
// MIRROR-WALRUS: buildFunction only guards process-aborting hazards (each
// memory/data index resolves; a MemArg offset is a lossless u64). It does NOT
// type-check — a non-power-of-two align, an out-of-bounds access, or an atomic
// op on a non-shared memory all build and emit as-is.
// ---------------------------------------------------------------------------

// Every LoadKind variant: each width, `atomic` true/false on the full-width
// integers, each ExtendedLoad on each narrow width, plus V128.
const ALL_LOAD_KINDS: LoadKind[] = [
  { type: 'I32', atomic: false },
  { type: 'I32', atomic: true },
  { type: 'I64', atomic: false },
  { type: 'I64', atomic: true },
  { type: 'F32' },
  { type: 'F64' },
  { type: 'V128' },
  { type: 'I32_8', kind: 'SignExtend' },
  { type: 'I32_8', kind: 'ZeroExtend' },
  { type: 'I32_8', kind: 'ZeroExtendAtomic' },
  { type: 'I32_16', kind: 'SignExtend' },
  { type: 'I32_16', kind: 'ZeroExtend' },
  { type: 'I32_16', kind: 'ZeroExtendAtomic' },
  { type: 'I64_8', kind: 'SignExtend' },
  { type: 'I64_8', kind: 'ZeroExtend' },
  { type: 'I64_8', kind: 'ZeroExtendAtomic' },
  { type: 'I64_16', kind: 'SignExtend' },
  { type: 'I64_16', kind: 'ZeroExtend' },
  { type: 'I64_16', kind: 'ZeroExtendAtomic' },
  { type: 'I64_32', kind: 'SignExtend' },
  { type: 'I64_32', kind: 'ZeroExtend' },
  { type: 'I64_32', kind: 'ZeroExtendAtomic' },
]

// Every StoreKind variant. NOTE the asymmetry with LoadKind: EVERY integer store
// (full-width AND narrow) carries `atomic`, and there is no ExtendedLoad.
const ALL_STORE_KINDS: StoreKind[] = [
  { type: 'I32', atomic: false },
  { type: 'I32', atomic: true },
  { type: 'I64', atomic: false },
  { type: 'I64', atomic: true },
  { type: 'F32' },
  { type: 'F64' },
  { type: 'V128' },
  { type: 'I32_8', atomic: false },
  { type: 'I32_8', atomic: true },
  { type: 'I32_16', atomic: false },
  { type: 'I32_16', atomic: true },
  { type: 'I64_8', atomic: false },
  { type: 'I64_8', atomic: true },
  { type: 'I64_16', atomic: false },
  { type: 'I64_16', atomic: true },
  { type: 'I64_32', atomic: false },
  { type: 'I64_32', atomic: true },
]

const U64_MAX = 18446744073709551615n // 2^64 - 1, the largest lossless u64 offset
const BIG_OFFSET = 0xdead_beef_cafen // > 2^32: proves the bigint path is exact beyond 32 bits
const ALIGNS = [1, 2, 4, 8, 16]
const OFFSETS = [0n, 7n, BIG_OFFSET, U64_MAX, 4096n]
// A varied MemArg per position so aligns/offsets differ across the body; the
// large and max-u64 offsets both appear in each of the load and store groups.
const memArgAt = (i: number): MemArg => ({ align: ALIGNS[i % ALIGNS.length], offset: OFFSETS[i % OFFSETS.length] })

// A body exercising all 8 C2 instrs and every LoadKind/StoreKind. Uses TWO
// memories so MemoryCopy's destination (`memory`) and source (`srcMemory`) are
// distinct — proving they are not swapped on the round-trip.
function memoryComprehensiveBody(mem0: number, mem1: number, data: number): InstrDesc[] {
  const body: InstrDesc[] = [
    { type: 'MemorySize', memory: mem0 },
    { type: 'MemoryGrow', memory: mem0 },
    { type: 'MemoryInit', memory: mem0, data },
    { type: 'DataDrop', data },
    { type: 'MemoryCopy', memory: mem1, srcMemory: mem0 },
    { type: 'MemoryFill', memory: mem1 },
  ]
  ALL_LOAD_KINDS.forEach((k, i) => body.push({ type: 'Load', memory: mem0, loadKind: k, memArg: memArgAt(i) }))
  ALL_STORE_KINDS.forEach((k, i) => body.push({ type: 'Store', memory: mem1, storeKind: k, memArg: memArgAt(i + 2) }))
  return body
}

test('C2 EXHAUSTIVE: all 8 memory/load-store instrs + every LoadKind/StoreKind round-trip in-memory', (t) => {
  const m = empty()
  const mem0 = m.memories.addLocal(false, false, 1n, null, null).index
  const mem1 = m.memories.addLocal(false, false, 1n, null, null).index
  const data = m.data.addPassive(new Uint8Array([1, 2, 3, 4])).index
  const body = memoryComprehensiveBody(mem0, mem1, data)

  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse; buildFunction is
  // MIRROR-WALRUS, so this ill-typed-but-well-formed body builds directly).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 8 instruction kinds are present, and a u64::MAX offset survived
  // exactly (proving the MemArg bigint path is lossless at the full width).
  const kinds = new Set(read.map((d) => d.type))
  for (const k of ['MemorySize', 'MemoryGrow', 'MemoryInit', 'DataDrop', 'MemoryCopy', 'MemoryFill', 'Load', 'Store']) {
    t.true(kinds.has(k), `body should contain a ${k}`)
  }
  t.true(
    read.some((d) => d.memArg?.offset === U64_MAX),
    'a u64::MAX MemArg offset must round-trip exactly',
  )
  // MemoryCopy's dst (memory) and src (srcMemory) are distinct and not swapped.
  const copy = read.find((d) => d.type === 'MemoryCopy')!
  t.is(copy.memory, mem1)
  t.is(copy.srcMemory, mem0)
})

test('C2: a well-typed memory.size/load/store body emits valid wasm and round-trips through re-parse', (t) => {
  const m = empty()
  const mem = m.memories.addLocal(false, false, 1n, null, null).index
  // memory.size; drop; (i32.const 0)(i32.load); drop; (i32.const 0)(i32.const 42)(i32.store).
  // align 4 = the natural i32 alignment (walrus emits it as align exponent 2).
  const body: InstrDesc[] = [
    { type: 'MemorySize', memory: mem },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Load', memory: mem, loadKind: { type: 'I32', atomic: false }, memArg: { align: 4, offset: 0n } },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I32', value: 42 } },
    { type: 'Store', memory: mem, storeKind: { type: 'I32', atomic: false }, memArg: { align: 4, offset: 0n } },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'memops'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse and read back: the ops decode to the same descriptors (align 4 and
  // offset 0 survive the log2 round-trip in the binary encoding).
  const read = WasmModule.fromBuffer(bytes).functions.byName('memops')!.instructions()
  t.deepEqual(read, body)
})

test('C2: a large MemArg offset survives the emit -> bytes -> re-parse boundary (memory64)', (t) => {
  // A 64-bit offset needs a memory64 memory to stay well-typed on re-parse.
  const m = empty()
  const mem = m.memories.addLocal(false, true, 1n, null, null).index
  const body: InstrDesc[] = [
    { type: 'Const', value: { type: 'I64', value: 0n } },
    { type: 'Load', memory: mem, loadKind: { type: 'I32', atomic: false }, memArg: { align: 1, offset: BIG_OFFSET } },
    { type: 'Drop' },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'bigoffset'
  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))
  const read = WasmModule.fromBuffer(bytes).functions.byName('bigoffset')!.instructions()
  t.deepEqual(read, body)
  // The large offset survived the byte boundary exactly.
  t.is(read[1].memArg!.offset, BIG_OFFSET)
})

test('C2 negative: an out-of-range memory or data index throws catchably (no abort)', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null) // memory index 0 exists; 5 does not

  t.throws(() => m.buildFunction([], [], [], [{ type: 'MemorySize', memory: 5 }]), {
    message: /no memory at index 5/,
  })
  // No data segments exist, so any data index is out of range.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'DataDrop', data: 3 }]), {
    message: /no data segment at index 3/,
  })
  // MemoryInit resolves the memory first, then the data: a bad data index throws
  // even alongside a valid memory.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'MemoryInit', memory: 0, data: 9 }]), {
    message: /no data segment at index 9/,
  })
  // MemoryCopy's SOURCE memory (srcMemory) is guarded too, not just the dest.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'MemoryCopy', memory: 0, srcMemory: 7 }]), {
    message: /no memory at index 7/,
  })
  // Process is still alive and the module was never mutated (a real abort would
  // have taken the whole run down — the proof under WASI).
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C2 negative: a memory/load-store descriptor missing a required field throws catchably', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null)

  t.throws(() => m.buildFunction([], [], [], [{ type: 'MemorySize' }]), {
    message: /`MemorySize` instruction is missing its `memory` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'MemoryInit', memory: 0 }]), {
    message: /`MemoryInit` instruction is missing its `data` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'MemoryCopy', memory: 0 }]), {
    message: /`MemoryCopy` instruction is missing its `srcMemory` field/,
  })
  // Load: loadKind is checked before memArg (matching emit's order).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'Load', memory: 0, memArg: { align: 1, offset: 0n } }]), {
    message: /`Load` instruction is missing its `loadKind` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'Load', memory: 0, loadKind: { type: 'I32', atomic: false } }]), {
    message: /`Load` instruction is missing its `memArg` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'Store', memory: 0, memArg: { align: 1, offset: 0n } }]), {
    message: /`Store` instruction is missing its `storeKind` field/,
  })
})

test('C2 negative: a MemArg offset that is not a lossless u64 throws catchably', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null)
  const load = (offset: bigint): InstrDesc => ({
    type: 'Load',
    memory: 0,
    loadKind: { type: 'I32', atomic: false },
    memArg: { align: 4, offset },
  })

  // 2^64 is one past u64::MAX — not lossless.
  t.throws(() => m.buildFunction([], [], [], [load(1n << 64n)]), {
    message: /MemArg offset must be a non-negative integer that fits in a u64/,
  })
  // A negative offset is rejected too (a u64 has no sign).
  t.throws(() => m.buildFunction([], [], [], [load(-1n)]), {
    message: /MemArg offset must be a non-negative integer that fits in a u64/,
  })
  t.is(1 + 1, 2)
})

// ---------------------------------------------------------------------------
// C3: table instructions (TableGet/Set/Grow/Size/Fill/Init/Copy, ElemDrop) +
// call_indirect.
//
// TableInit/ElemDrop need a real element segment and CallIndirect a real
// function type. The binding has no `elements.add`, so the EXHAUSTIVE test parses
// the committed elements.wasm fixture (table 0 funcref + an ACTIVE elem[0] and a
// PASSIVE elem[1]) for genuine table/element ids, adds a second table and a fresh
// function type, then round-trips all 9 instrs through the IN-MEMORY read path
// (buildFunction -> instructions() on the same module; no emit/re-parse, so the
// ill-typed-but-well-formed body builds directly). Separate WELL-TYPED bodies
// prove table.size and call_indirect survive emit -> bytes -> re-parse.
//
// MIRROR-WALRUS: buildFunction only guards process-aborting hazards (each
// table/element/type index resolves). It does NOT type-check — a table op on a
// mismatched value, or a call_indirect whose type does not match the callee,
// still builds and emits as-is.
// ---------------------------------------------------------------------------

// Committed fixture (see fixtures/elements.wat): table 0 (4 funcref), func $f
// (a `() -> ()` type at type index 0), element[0] ACTIVE, element[1] PASSIVE.
const ELEMENTS_FIXTURE = readFileSync(join(__dirname, 'fixtures', 'elements.wasm'))
const FUNCREF = { type: 'Ref', nullable: true, heap: { type: 'Abstract', kind: 'Func' } } as const

test('C3 EXHAUSTIVE: all 9 table instrs + call_indirect round-trip in-memory', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  // Fixture gives table 0 (funcref) and element segments 0 (active) / 1 (passive).
  const t0 = 0
  const e0 = 0
  const e1 = 1
  // A SECOND table so TableCopy's dst (`table`) and src (`srcTable`) are distinct
  // — proving they are not swapped on the round-trip.
  const t1 = m.tables.addLocal(false, 0n, null, FUNCREF).index
  // A fresh function type for call_indirect (a real user type, not an entry type).
  const ty = m.types.add([], []).index

  const body: InstrDesc[] = [
    { type: 'TableGet', table: t0 },
    { type: 'TableSet', table: t0 },
    { type: 'TableGrow', table: t0 },
    { type: 'TableSize', table: t1 },
    { type: 'TableFill', table: t1 },
    { type: 'TableInit', table: t0, elem: e0 },
    { type: 'TableCopy', table: t1, srcTable: t0 },
    { type: 'ElemDrop', elem: e1 },
    { type: 'CallIndirect', typeIndex: ty, table: t0 },
  ]

  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse; buildFunction is
  // MIRROR-WALRUS, so this ill-typed-but-well-formed body builds directly).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 9 instruction kinds are present.
  const kinds = new Set(read.map((d) => d.type))
  for (const k of [
    'TableGet',
    'TableSet',
    'TableGrow',
    'TableSize',
    'TableFill',
    'TableInit',
    'TableCopy',
    'ElemDrop',
    'CallIndirect',
  ]) {
    t.true(kinds.has(k), `body should contain a ${k}`)
  }
  // TableCopy's dst (table) and src (srcTable) are distinct and not swapped.
  const copy = read.find((d) => d.type === 'TableCopy')!
  t.is(copy.table, t1)
  t.is(copy.srcTable, t0)
  // CallIndirect carries BOTH its callee type and its table.
  const ci = read.find((d) => d.type === 'CallIndirect')!
  t.is(ci.typeIndex, ty)
  t.is(ci.table, t0)
})

test('C3: a well-typed table.size body emits valid wasm and round-trips through re-parse', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  // table.size; drop — a `() -> ()` body: table.size pushes an i32, drop pops it.
  const body: InstrDesc[] = [{ type: 'TableSize', table: 0 }, { type: 'Drop' }]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'tblsize'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse and read back: the op decodes to the same descriptor.
  const read = WasmModule.fromBuffer(bytes).functions.byName('tblsize')!.instructions()
  t.deepEqual(read, body)
})

test('C3: a well-typed call_indirect emits valid wasm and reads back through re-parse', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  // (i32.const 0)(call_indirect (type $v) table 0), where $v is `() -> ()`: the
  // call pops the i32 table index and calls through the funcref table 0 —
  // well-typed as-is.
  const ty = m.types.add([], []).index
  const body: InstrDesc[] = [
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'CallIndirect', typeIndex: ty, table: 0 },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'callind'
  const bytes = m.emitWasm(false)

  // Independent proof the call_indirect encoded to valid, well-typed wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse: the op decodes back to a CallIndirect through the same table. emit
  // rewrites the type section, so the type index is renumbered — only the
  // structural round-trip (kind + table) is asserted here, not the exact value.
  const read = WasmModule.fromBuffer(bytes).functions.byName('callind')!.instructions()
  t.is(read.length, 2)
  t.is(read[0].type, 'Const')
  t.is(read[1].type, 'CallIndirect')
  t.is(read[1].table, 0)
  t.is(typeof read[1].typeIndex, 'number')
})

test('C3 negative: an out-of-range table, element, or type index throws catchably (no abort)', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  // table 0 and element segments 0/1 exist; the indices below do not.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'TableGet', table: 99 }]), {
    message: /no table at index 99/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ElemDrop', elem: 7 }]), {
    message: /no element segment at index 7/,
  })
  // TableInit resolves the table first, then the elem: a bad elem throws even
  // alongside a valid table.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'TableInit', table: 0, elem: 9 }]), {
    message: /no element segment at index 9/,
  })
  // TableCopy's SOURCE table (srcTable) is guarded too, not just the dest.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'TableCopy', table: 0, srcTable: 5 }]), {
    message: /no table at index 5/,
  })
  // CallIndirect resolves the callee TYPE first (via resolve_type_id): a bad type
  // index throws even alongside a valid table.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'CallIndirect', typeIndex: 42, table: 0 }]), {
    message: /no type at index 42/,
  })
  // Process is still alive and the module was never mutated (a real abort would
  // have taken the whole run down — the proof under WASI).
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C3 negative: a table/call_indirect descriptor missing a required field throws catchably', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  const ty = m.types.add([], []).index

  t.throws(() => m.buildFunction([], [], [], [{ type: 'TableGet' }]), {
    message: /`TableGet` instruction is missing its `table` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'TableInit', table: 0 }]), {
    message: /`TableInit` instruction is missing its `elem` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'TableCopy', table: 0 }]), {
    message: /`TableCopy` instruction is missing its `srcTable` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ElemDrop' }]), {
    message: /`ElemDrop` instruction is missing its `elem` field/,
  })
  // CallIndirect: typeIndex is checked before table (matching emit's order).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'CallIndirect', table: 0 }]), {
    message: /`CallIndirect` instruction is missing its `typeIndex` field/,
  })
  // A resolvable typeIndex isolates the missing-table error.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'CallIndirect', typeIndex: ty }]), {
    message: /`CallIndirect` instruction is missing its `table` field/,
  })
})

// ---------------------------------------------------------------------------
// C4: core reference instructions (RefNull/RefIsNull/RefFunc) + tail calls
// (ReturnCall/ReturnCallIndirect).
//
// RefNull is the one instruction whose payload is a whole `RefType`
// (`{ nullable, heap }`). Its heap type goes through the MODULE-AWARE conversion,
// so a CONCRETE `(ref null $t)` resolves against the live arena and a
// foreign/deleted/entry index is rejected catchably (never a process abort — the
// B2b lesson). RefFunc/ReturnCall reuse `function_id_at`; ReturnCallIndirect
// reuses `resolve_type_id` + `table_id_at`, exactly like C3's CallIndirect.
//
// The EXHAUSTIVE test builds all 5 instrs (RefNull with an ABSTRACT heap AND a
// CONCRETE `(ref null $struct)`) and round-trips them through the IN-MEMORY read
// path (buildFunction -> instructions() on the same module; no emit/re-parse, so
// the ill-typed-but-well-formed body builds directly). Separate WELL-TYPED bodies
// prove ref.null/ref.is_null and the tail calls survive emit -> bytes -> re-parse.
//
// MIRROR-WALRUS: buildFunction only guards process-aborting hazards (each
// func/type/heap index resolves). It does NOT type-check — a non-null `ref.null`,
// a ref.func to an undeclared function, or a tail call whose signature does not
// match still builds and emits as-is.
// ---------------------------------------------------------------------------

test('C4 EXHAUSTIVE: all 5 reference/tail-call instrs round-trip in-memory', (t) => {
  const m = empty()
  // A function to reference (ref.func / return_call). An IMPORT exists before
  // buildFunction and keeps a stable index (a body cannot reference the function
  // it is defining).
  const sig = m.types.add([], [])
  const fref = m.imports.addFunction('env', 'callee', sig).index
  // A funcref table + a function type for return_call_indirect.
  const tbl = m.tables.addLocal(false, 0n, null, FUNCREF).index
  const cty = m.types.add([], []).index
  // An existing composite (struct) type so a CONCRETE `(ref null $struct)`
  // resolves.
  const structTy = m.types.addStruct([{ storage: { type: 'Val', value: I32 }, mutable: false }]).index

  const body: InstrDesc[] = [
    // RefNull with an ABSTRACT (func) heap type.
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Func' } } },
    { type: 'RefIsNull' },
    { type: 'Drop' },
    // RefNull with an ABSTRACT (extern) heap, NON-nullable — proving the flag is
    // carried verbatim (MIRROR-WALRUS does not reject a non-null null type).
    { type: 'RefNull', refType: { nullable: false, heap: { type: 'Abstract', kind: 'Extern' } } },
    { type: 'Drop' },
    // RefNull with a CONCRETE `(ref null $structTy)` — the abort-guarded path.
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Concrete', typeIndex: structTy } } },
    { type: 'Drop' },
    { type: 'RefFunc', func: fref },
    { type: 'Drop' },
    { type: 'ReturnCall', func: fref },
    { type: 'ReturnCallIndirect', typeIndex: cty, table: tbl },
  ]

  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse; buildFunction is
  // MIRROR-WALRUS, so this ill-typed-but-well-formed body builds directly).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 5 C4 instruction kinds are present.
  const kinds = new Set(read.map((d) => d.type))
  for (const k of ['RefNull', 'RefIsNull', 'RefFunc', 'ReturnCall', 'ReturnCallIndirect']) {
    t.true(kinds.has(k), `body should contain a ${k}`)
  }
  // The CONCRETE RefNull carried its heap type index unchanged (not swapped with
  // an abstract heap or dropped).
  const concrete = read.find((d) => d.type === 'RefNull' && d.refType!.heap.type === 'Concrete')!
  const heap = concrete.refType!.heap
  t.is(heap.type, 'Concrete')
  if (heap.type === 'Concrete') t.is(heap.typeIndex, structTy)
  // RefFunc and ReturnCall both carried the SAME func index.
  t.is(read.find((d) => d.type === 'RefFunc')!.func, fref)
  t.is(read.find((d) => d.type === 'ReturnCall')!.func, fref)
  // ReturnCallIndirect carried BOTH its callee type and its table.
  const rci = read.find((d) => d.type === 'ReturnCallIndirect')!
  t.is(rci.typeIndex, cty)
  t.is(rci.table, tbl)
})

test('C4: a well-typed ref.null/ref.is_null body emits valid wasm and round-trips through re-parse', (t) => {
  const m = empty()
  // (ref.null func)(ref.is_null)(drop) — a `() -> ()` body: ref.null pushes a
  // nullable funcref, ref.is_null pops it and pushes an i32, drop pops the i32.
  // Reference types are STABLE, so no feature flag is needed here.
  const body: InstrDesc[] = [
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Func' } } },
    { type: 'RefIsNull' },
    { type: 'Drop' },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'refnull'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse and read back: the ops decode to the same descriptors.
  const read = WasmModule.fromBuffer(bytes).functions.byName('refnull')!.instructions()
  t.deepEqual(read, body)
})

test('C4: well-typed return_call / return_call_indirect emit and re-parse (tail-call feature)', (t) => {
  const m = empty()
  // return_call to an imported `() -> ()` callee — a `() -> ()` tail call.
  const sig = m.types.add([], [])
  const callee = m.imports.addFunction('env', 'callee', sig).index
  const rcIdx = m.buildFunction([], [], [], [{ type: 'ReturnCall', func: callee }])
  m.functions.getByIndex(rcIdx)!.name = 'rc'
  // (i32.const 0)(return_call_indirect (type $v) table 0) through a funcref table.
  const tbl = m.tables.addLocal(false, 0n, null, FUNCREF).index
  const cty = m.types.add([], []).index
  const rciIdx = m.buildFunction(
    [],
    [],
    [],
    [
      { type: 'Const', value: { type: 'I32', value: 0 } },
      { type: 'ReturnCallIndirect', typeIndex: cty, table: tbl },
    ],
  )
  m.functions.getByIndex(rciIdx)!.name = 'rci'

  const bytes = m.emitWasm(false)

  // Tail calls are an UNSTABLE feature — re-parse with onlyStableFeatures(false)
  // so walrus accepts them; the ops decode back to the same instruction kinds.
  // emit rewrites the type section, so return_call_indirect's typeIndex is
  // renumbered — only the structural round-trip (kind + table) is asserted.
  const config = new ModuleConfig().onlyStableFeatures(false)
  const rm = WasmModule.fromBufferWithConfig(bytes, config)

  const rc = rm.functions.byName('rc')!.instructions()
  t.is(rc.length, 1)
  t.is(rc[0].type, 'ReturnCall')
  t.is(typeof rc[0].func, 'number')

  const rci = rm.functions.byName('rci')!.instructions()
  t.is(rci.length, 2)
  t.is(rci[0].type, 'Const')
  t.is(rci[1].type, 'ReturnCallIndirect')
  t.is(rci[1].table, tbl)
  t.is(typeof rci[1].typeIndex, 'number')
})

test('C4 negative: a bad concrete ref.null heap or func/type index throws catchably (no abort)', (t) => {
  const m = empty()
  const sig = m.types.add([], [])
  m.imports.addFunction('env', 'callee', sig) // func 0 exists
  const tbl = m.tables.addLocal(false, 0n, null, FUNCREF).index

  // A CONCRETE ref.null naming a nonexistent type index throws (resolved through
  // the module-aware heap conversion) rather than aborting at emit.
  t.throws(
    () =>
      m.buildFunction(
        [],
        [],
        [],
        [{ type: 'RefNull', refType: { nullable: true, heap: { type: 'Concrete', typeIndex: 999 } } }],
      ),
    { message: /no type at index 999/ },
  )
  // ref.func / return_call with an out-of-range func index throws.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'RefFunc', func: 99 }]), {
    message: /no function at index 99/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCall', func: 77 }]), {
    message: /no function at index 77/,
  })
  // return_call_indirect resolves the callee TYPE first (via resolve_type_id): a
  // bad type index throws even alongside a valid table.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCallIndirect', typeIndex: 42, table: tbl }]), {
    message: /no type at index 42/,
  })
  // Process is still alive and the module was never mutated (a real abort would
  // have taken the whole run down — the proof under WASI).
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C4 negative: a reference/tail-call descriptor missing a required field throws catchably', (t) => {
  const m = empty()
  const sig = m.types.add([], [])
  m.imports.addFunction('env', 'callee', sig)
  const tbl = m.tables.addLocal(false, 0n, null, FUNCREF).index
  const cty = m.types.add([], []).index

  t.throws(() => m.buildFunction([], [], [], [{ type: 'RefNull' }]), {
    message: /`RefNull` instruction is missing its `refType` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'RefFunc' }]), {
    message: /`RefFunc` instruction is missing its `func` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCall' }]), {
    message: /`ReturnCall` instruction is missing its `func` field/,
  })
  // ReturnCallIndirect: typeIndex is checked before table (matching emit's order).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCallIndirect', table: tbl }]), {
    message: /`ReturnCallIndirect` instruction is missing its `typeIndex` field/,
  })
  // A resolvable typeIndex isolates the missing-table error.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCallIndirect', typeIndex: cty }]), {
    message: /`ReturnCallIndirect` instruction is missing its `table` field/,
  })
})

// ---------------------------------------------------------------------------
// C5: atomic (threads) instructions — AtomicRmw/Cmpxchg/AtomicNotify/
// AtomicWait/AtomicFence.
//
// Direct sibling of C2: the four memory-bearing atomics REUSE C2's memory
// resolver and the MemArg (align + a u64 offset as a bigint) type. Two small
// string enums are added: AtomicOp (the rmw op) and AtomicWidth (the access
// width). Same walrus fact as C1b–C4: `strict_validate(false)` is a no-op in
// 0.26.4, so an ill-typed body can never re-parse. The EXHAUSTIVE test therefore
// uses the IN-MEMORY read path (buildFunction -> instructions() on the same
// module); a separate WELL-TYPED body proves the ops survive the emit -> bytes
// -> re-parse boundary (atomics validate only against a SHARED memory).
//
// MIRROR-WALRUS: buildFunction only guards process-aborting hazards (the memory
// index resolves; a MemArg offset is a lossless u64) and required-field presence.
// It does NOT type-check — an atomic on a non-shared memory, a mismatched
// alignment, or an illegal op/width combination all build and emit as-is (which
// is why the in-memory body below needs neither a shared memory nor natural
// alignment).
// ---------------------------------------------------------------------------

const ALL_ATOMIC_OPS: AtomicOp[] = ['Add', 'Sub', 'And', 'Or', 'Xor', 'Xchg']
const ALL_ATOMIC_WIDTHS: AtomicWidth[] = ['I32', 'I32_8', 'I32_16', 'I64', 'I64_8', 'I64_16', 'I64_32']

// A body exercising all 5 atomics: AtomicRmw over EVERY op x width pair (6 x 7 =
// 42), Cmpxchg over every width, AtomicNotify, AtomicWait with sixtyFour both
// true and false, and AtomicFence. MemArgs vary per position (so the large and
// u64::MAX offsets both appear), proving the reused MemArg bigint path is exact.
function atomicComprehensiveBody(mem: number): InstrDesc[] {
  const body: InstrDesc[] = []
  let i = 0
  for (const op of ALL_ATOMIC_OPS) {
    for (const width of ALL_ATOMIC_WIDTHS) {
      body.push({ type: 'AtomicRmw', memory: mem, atomicOp: op, atomicWidth: width, memArg: memArgAt(i++) })
    }
  }
  for (const width of ALL_ATOMIC_WIDTHS) {
    body.push({ type: 'Cmpxchg', memory: mem, atomicWidth: width, memArg: memArgAt(i++) })
  }
  body.push({ type: 'AtomicNotify', memory: mem, memArg: memArgAt(i++) })
  body.push({ type: 'AtomicWait', memory: mem, memArg: memArgAt(i++), sixtyFour: false })
  body.push({ type: 'AtomicWait', memory: mem, memArg: memArgAt(i++), sixtyFour: true })
  body.push({ type: 'AtomicFence' })
  return body
}

test('C5 EXHAUSTIVE: all 5 atomics + every AtomicOp/AtomicWidth round-trip in-memory', (t) => {
  const m = empty()
  // A plain (non-shared) memory: MIRROR-WALRUS builds the ill-typed body anyway.
  const mem = m.memories.addLocal(false, false, 1n, null, null).index
  const body = atomicComprehensiveBody(mem)

  const idx = m.buildFunction([], [], [], body)
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 5 atomic kinds are present.
  const kinds = new Set(read.map((d) => d.type))
  for (const k of ['AtomicRmw', 'Cmpxchg', 'AtomicNotify', 'AtomicWait', 'AtomicFence']) {
    t.true(kinds.has(k), `body should contain a ${k}`)
  }
  // Every AtomicOp and every AtomicWidth survived at least once.
  const ops = new Set(read.filter((d) => d.type === 'AtomicRmw').map((d) => d.atomicOp))
  t.is(ops.size, ALL_ATOMIC_OPS.length)
  const widths = new Set(read.filter((d) => d.atomicWidth != null).map((d) => d.atomicWidth))
  t.is(widths.size, ALL_ATOMIC_WIDTHS.length)
  // AtomicWait's sixtyFour round-trips for BOTH values.
  const waits = read.filter((d) => d.type === 'AtomicWait')
  t.deepEqual(
    waits.map((d) => d.sixtyFour).sort(),
    [false, true],
  )
  // A u64::MAX MemArg offset survived exactly (the reused bigint path is lossless
  // at the full width).
  t.true(
    read.some((d) => d.memArg?.offset === U64_MAX),
    'a u64::MAX MemArg offset must round-trip exactly',
  )
})

test('C5: a well-typed atomic body emits valid wasm and round-trips through re-parse', (t) => {
  const m = empty()
  // Atomics validate only against a SHARED memory (strictValidate, default true,
  // on the re-parse enforces this), so the well-typed body needs one. A shared
  // memory requires a maximum. Natural alignment matches each access width
  // (i32/notify/wait32 = 4, wait64 = 8).
  const mem = m.memories.addLocal(true, false, 1n, 1n, null).index
  const body: InstrDesc[] = [
    // i32.atomic.rmw.add: [addr, value] -> [old]; drop.
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I32', value: 1 } },
    { type: 'AtomicRmw', memory: mem, atomicOp: 'Add', atomicWidth: 'I32', memArg: { align: 4, offset: 0n } },
    { type: 'Drop' },
    // i32.atomic.rmw.cmpxchg: [addr, expected, replacement] -> [old]; drop.
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I32', value: 1 } },
    { type: 'Cmpxchg', memory: mem, atomicWidth: 'I32', memArg: { align: 4, offset: 0n } },
    { type: 'Drop' },
    // memory.atomic.notify: [addr, count] -> [woken]; drop.
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I32', value: 1 } },
    { type: 'AtomicNotify', memory: mem, memArg: { align: 4, offset: 0n } },
    { type: 'Drop' },
    // memory.atomic.wait32: [addr, expected:i32, timeout:i64] -> [result]; drop.
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I64', value: -1n } },
    { type: 'AtomicWait', memory: mem, memArg: { align: 4, offset: 0n }, sixtyFour: false },
    { type: 'Drop' },
    // memory.atomic.wait64: [addr, expected:i64, timeout:i64] -> [result]; drop.
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'I64', value: 0n } },
    { type: 'Const', value: { type: 'I64', value: -1n } },
    { type: 'AtomicWait', memory: mem, memArg: { align: 8, offset: 0n }, sixtyFour: true },
    { type: 'Drop' },
    // atomic.fence.
    { type: 'AtomicFence' },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'atomics'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed threads wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse (default config: strictValidate true, threads enabled) and read back:
  // every atomic decodes to the same descriptor (align exponents and offsets
  // survive the binary round-trip).
  const read = WasmModule.fromBuffer(bytes).functions.byName('atomics')!.instructions()
  t.deepEqual(read, body)
})

test('C5 negative: an out-of-range/foreign memory index throws catchably (no abort)', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null) // memory index 0 exists; 5 does not

  const rmw = { type: 'AtomicRmw', memory: 5, atomicOp: 'Add', atomicWidth: 'I32', memArg: { align: 4, offset: 0n } }
  t.throws(() => m.buildFunction([], [], [], [rmw as InstrDesc]), { message: /no memory at index 5/ })
  // The guard covers every memory-bearing atomic, not just AtomicRmw.
  t.throws(
    () => m.buildFunction([], [], [], [{ type: 'Cmpxchg', memory: 8, atomicWidth: 'I32', memArg: { align: 4, offset: 0n } }]),
    { message: /no memory at index 8/ },
  )
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicNotify', memory: 6, memArg: { align: 4, offset: 0n } }]), {
    message: /no memory at index 6/,
  })
  t.throws(
    () => m.buildFunction([], [], [], [{ type: 'AtomicWait', memory: 7, memArg: { align: 4, offset: 0n }, sixtyFour: false }]),
    { message: /no memory at index 7/ },
  )
  // Process is still alive and the module was never mutated (a real abort would
  // have taken the whole run down — the proof under WASI).
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C5 negative: an atomic descriptor missing a required field throws catchably', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null)
  const arg = { align: 4, offset: 0n }

  // AtomicRmw: atomicOp is checked before atomicWidth, which is checked before
  // memArg (matching emit's order).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicRmw', memory: 0, atomicWidth: 'I32', memArg: arg }]), {
    message: /`AtomicRmw` instruction is missing its `atomicOp` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicRmw', memory: 0, atomicOp: 'Add', memArg: arg }]), {
    message: /`AtomicRmw` instruction is missing its `atomicWidth` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicRmw', memory: 0, atomicOp: 'Add', atomicWidth: 'I32' }]), {
    message: /`AtomicRmw` instruction is missing its `memArg` field/,
  })
  // Cmpxchg requires atomicWidth (but no atomicOp).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'Cmpxchg', memory: 0, memArg: arg }]), {
    message: /`Cmpxchg` instruction is missing its `atomicWidth` field/,
  })
  // AtomicNotify requires memArg.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicNotify', memory: 0 }]), {
    message: /`AtomicNotify` instruction is missing its `memArg` field/,
  })
  // AtomicWait requires sixtyFour (checked after memArg).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicWait', memory: 0, memArg: arg }]), {
    message: /`AtomicWait` instruction is missing its `sixtyFour` field/,
  })
  // A missing memory is guarded for every atomic too.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'AtomicRmw', atomicOp: 'Add', atomicWidth: 'I32', memArg: arg }]), {
    message: /`AtomicRmw` instruction is missing its `memory` field/,
  })
})

test('C5 negative: a non-lossless MemArg offset on an atomic throws catchably', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null)
  const rmw = (offset: bigint): InstrDesc => ({
    type: 'AtomicRmw',
    memory: 0,
    atomicOp: 'Add',
    atomicWidth: 'I32',
    memArg: { align: 4, offset },
  })

  // 2^64 is one past u64::MAX — not lossless.
  t.throws(() => m.buildFunction([], [], [], [rmw(1n << 64n)]), {
    message: /MemArg offset must be a non-negative integer that fits in a u64/,
  })
  // A negative offset is rejected too (a u64 has no sign).
  t.throws(() => m.buildFunction([], [], [], [rmw(-1n)]), {
    message: /MemArg offset must be a non-negative integer that fits in a u64/,
  })
  t.is(1 + 1, 2)
})

// ---------------------------------------------------------------------------
// C6a: SIMD part 1 — the v128 const, the 14 lane-carrying Binop/Unop ops
// (`*ReplaceLane` / `*ExtractLane*`, carried as `op` + `lane`), and the three
// fixed-shape SIMD instructions (`V128Bitselect`, `I8x16Swizzle`,
// `I8x16Shuffle`). Same walrus fact as the earlier operator tasks:
// `strict_validate(false)` is a no-op in 0.26.4, so an ill-typed body can never
// re-parse. The EXHAUSTIVE test therefore uses the IN-MEMORY read path
// (`buildFunction` -> `instructions()` on the same module, never touching the
// parser/validator); a separate WELL-TYPED body proves the ops (and the v128
// little-endian byte order) survive the emit -> bytes -> re-parse boundary.
//
// MIRROR-WALRUS: `buildFunction` only guards representation hazards — a v128
// const's bytes are EXACTLY 16, an i8x16.shuffle's indices are EXACTLY 16, a
// lane op has a `lane` (napi range-checks it to a u8). It does NOT type-check: a
// lane index past the vector width, a shuffle index >= 32, or an ill-typed body
// all build and emit as-is.
// ---------------------------------------------------------------------------

// A distinctive 16-byte v128 pattern: asymmetric so a byte-order or off-by-one
// error in the round-trip is visible.
const V128_PATTERN = new Uint8Array([0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb])

// The 6 BinaryOp `*ReplaceLane` and 8 UnaryOp `*ExtractLane*` lane-carriers, each
// with a representative lane index (distinct values catch any op/lane mixup).
const LANE_BINOPS: InstrDesc[] = [
  { type: 'Binop', op: 'I8x16ReplaceLane', lane: 15 },
  { type: 'Binop', op: 'I16x8ReplaceLane', lane: 7 },
  { type: 'Binop', op: 'I32x4ReplaceLane', lane: 3 },
  { type: 'Binop', op: 'I64x2ReplaceLane', lane: 1 },
  { type: 'Binop', op: 'F32x4ReplaceLane', lane: 2 },
  { type: 'Binop', op: 'F64x2ReplaceLane', lane: 0 },
]
const LANE_UNOPS: InstrDesc[] = [
  { type: 'Unop', op: 'I8x16ExtractLaneS', lane: 1 },
  { type: 'Unop', op: 'I8x16ExtractLaneU', lane: 2 },
  { type: 'Unop', op: 'I16x8ExtractLaneS', lane: 3 },
  { type: 'Unop', op: 'I16x8ExtractLaneU', lane: 4 },
  { type: 'Unop', op: 'I32x4ExtractLane', lane: 3 },
  { type: 'Unop', op: 'I64x2ExtractLane', lane: 1 },
  { type: 'Unop', op: 'F32x4ExtractLane', lane: 2 },
  { type: 'Unop', op: 'F64x2ExtractLane', lane: 0 },
]

test('C6a EXHAUSTIVE: v128 const, all 14 lane ops, and the 3 fixed-shape SIMD instrs round-trip in-memory', (t) => {
  const body: InstrDesc[] = [
    // v128 const with the distinctive pattern.
    { type: 'Const', value: { type: 'V128', value: V128_PATTERN } },
    // every lane-carrying op with its representative lane.
    ...LANE_BINOPS,
    ...LANE_UNOPS,
    // the fieldless fixed-shape SIMD instrs.
    { type: 'V128Bitselect' },
    { type: 'I8x16Swizzle' },
    // i8x16.shuffle with a distinctive 16-byte index pattern.
    {
      type: 'I8x16Shuffle',
      shuffleIndices: new Uint8Array([15, 0, 14, 1, 13, 2, 12, 3, 11, 4, 10, 5, 9, 6, 8, 7]),
    },
  ]

  const m = empty()
  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse, so walrus'
  // always-on function-body validator never runs on this ill-typed body).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)
})

// A WELL-TYPED, stack-balanced SIMD body: correct operands and in-range lane /
// shuffle immediates so the emitted module is valid wasm a real engine accepts.
// It exercises the v128 const, a Unop lane op, a Binop lane op, and all three
// fixed-shape instrs.
const V128_A = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
const V128_B = new Uint8Array(16).fill(0xff)
const WELL_TYPED_SIMD_BODY: InstrDesc[] = [
  // v128.const A ; i8x16.extract_lane_s 3 ; drop
  { type: 'Const', value: { type: 'V128', value: V128_A } },
  { type: 'Unop', op: 'I8x16ExtractLaneS', lane: 3 },
  { type: 'Drop' },
  // v128.const A ; i32.const 42 ; i8x16.replace_lane 5 ; drop
  { type: 'Const', value: { type: 'V128', value: V128_A } },
  { type: 'Const', value: { type: 'I32', value: 42 } },
  { type: 'Binop', op: 'I8x16ReplaceLane', lane: 5 },
  { type: 'Drop' },
  // v128.const A ; v128.const B ; i8x16.shuffle <0..15> ; drop
  { type: 'Const', value: { type: 'V128', value: V128_A } },
  { type: 'Const', value: { type: 'V128', value: V128_B } },
  { type: 'I8x16Shuffle', shuffleIndices: new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]) },
  { type: 'Drop' },
  // v128.const A ; v128.const B ; i8x16.swizzle ; drop
  { type: 'Const', value: { type: 'V128', value: V128_A } },
  { type: 'Const', value: { type: 'V128', value: V128_B } },
  { type: 'I8x16Swizzle' },
  { type: 'Drop' },
  // v128.const A ; v128.const B ; v128.const B ; v128.bitselect ; drop
  { type: 'Const', value: { type: 'V128', value: V128_A } },
  { type: 'Const', value: { type: 'V128', value: V128_B } },
  { type: 'Const', value: { type: 'V128', value: V128_B } },
  { type: 'V128Bitselect' },
  { type: 'Drop' },
]

test('C6a: a well-typed SIMD body emits valid wasm and round-trips through re-parse (v128 endianness proven)', (t) => {
  const m = empty()
  const idx = m.buildFunction([], [], [], WELL_TYPED_SIMD_BODY)
  m.functions.getByIndex(idx)!.name = 'simd6a'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed (stable) SIMD wasm — this is
  // the endianness proof: the v128 const bytes survive the LE binary encoding.
  t.true(WebAssembly.validate(bytes))

  // Re-parse (all proposals on so every SIMD opcode decodes) and read back: the
  // ops, lanes, shuffle indices, and v128 bytes all decode to the same body.
  const reparsed = new ModuleConfig().onlyStableFeatures(false).parse(bytes)
  const read = reparsed.functions.byName('simd6a')!.instructions()
  t.deepEqual(read, WELL_TYPED_SIMD_BODY)
})

test('C6a negative: a lane op missing its `lane` throws catchably (both Binop and Unop)', (t) => {
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Binop', op: 'F64x2ReplaceLane' }]), {
    message: /SIMD lane op `F64x2ReplaceLane` requires a `lane` index/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Unop', op: 'F32x4ExtractLane' }]), {
    message: /SIMD lane op `F32x4ExtractLane` requires a `lane` index/,
  })
})

test('C6a negative: a v128 const whose byte length != 16 throws catchably', (t) => {
  const short = { type: 'Const', value: { type: 'V128', value: new Uint8Array(15) } } as InstrDesc
  t.throws(() => empty().buildFunction([], [], [], [short]), {
    message: /v128 const requires exactly 16 bytes, got 15/,
  })
  const long = { type: 'Const', value: { type: 'V128', value: new Uint8Array(17) } } as InstrDesc
  t.throws(() => empty().buildFunction([], [], [], [long]), {
    message: /v128 const requires exactly 16 bytes, got 17/,
  })
})

test('C6a negative: an I8x16Shuffle whose indices length != 16 (or absent) throws catchably', (t) => {
  const short = { type: 'I8x16Shuffle', shuffleIndices: new Uint8Array([1, 2, 3]) } as InstrDesc
  t.throws(() => empty().buildFunction([], [], [], [short]), {
    message: /i8x16.shuffle requires exactly 16 lane indices, got 3/,
  })
  // Absent shuffleIndices is a missing-field error.
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'I8x16Shuffle' }]), {
    message: /`I8x16Shuffle` instruction is missing its `shuffleIndices` field/,
  })
})

test('C6a negative: an unknown SIMD op string still throws (C1b behavior intact)', (t) => {
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Binop', op: 'I8x16NotARealLaneOp', lane: 0 }]), {
    message: /unknown binary operator `I8x16NotARealLaneOp`/,
  })
})

// ---------------------------------------------------------------------------
// C6b: SIMD part 2 — the `LoadSimd` instruction (the vector load / load-lane /
// store-lane family). A near-twin of C2's Load/Store: same `memory` (MemoryId)
// and `memArg` (MemArg) fields, but its own `loadSimdKind` (LoadSimdKind: 12
// fieldless whole-vector loads + 8 lane-carrying load/store-lane ops). Same
// walrus fact as the earlier tasks: `strict_validate(false)` is a no-op in
// 0.26.4, so an ill-typed body can never re-parse. The EXHAUSTIVE test therefore
// uses the IN-MEMORY read path (`buildFunction` -> `instructions()` on the same
// module, never touching the parser/validator); a separate WELL-TYPED body
// proves the ops survive the emit -> bytes -> re-parse boundary.
//
// MIRROR-WALRUS: `buildFunction` only guards process-aborting hazards (the
// `memory` index resolves; the MemArg offset is a lossless u64). It does NOT
// type-check — a lane index past the vector width, an alignment that is not the
// natural access size, or an ill-typed body all build and emit as-is.
// ---------------------------------------------------------------------------

// Every LoadSimdKind: all 12 fieldless whole-vector loads plus every one of the
// 8 lane-carrying ops. The load-lane and store-lane variants use DISTINCT lane
// values (15/7/3/1 for loads, 14/6/2/0 for stores) so any kind/lane mixup — e.g.
// a Load8Lane read back as a Store8Lane, or a lane dropped/swapped — is visible.
const ALL_LOAD_SIMD_KINDS: LoadSimdKind[] = [
  { type: 'Splat8' },
  { type: 'Splat16' },
  { type: 'Splat32' },
  { type: 'Splat64' },
  { type: 'V128Load8x8S' },
  { type: 'V128Load8x8U' },
  { type: 'V128Load16x4S' },
  { type: 'V128Load16x4U' },
  { type: 'V128Load32x2S' },
  { type: 'V128Load32x2U' },
  { type: 'V128Load32Zero' },
  { type: 'V128Load64Zero' },
  { type: 'V128Load8Lane', lane: 15 },
  { type: 'V128Load16Lane', lane: 7 },
  { type: 'V128Load32Lane', lane: 3 },
  { type: 'V128Load64Lane', lane: 1 },
  { type: 'V128Store8Lane', lane: 14 },
  { type: 'V128Store16Lane', lane: 6 },
  { type: 'V128Store32Lane', lane: 2 },
  { type: 'V128Store64Lane', lane: 0 },
]

test('C6b EXHAUSTIVE: LoadSimd across every LoadSimdKind (+ a max-u64 MemArg offset) round-trips in-memory', (t) => {
  const m = empty()
  const mem = m.memories.addLocal(false, false, 1n, null, null).index
  // A varied MemArg per position (reusing the C2 `memArgAt` cycle, whose OFFSETS
  // include BIG_OFFSET and U64_MAX) so the bigint offset path is exercised at the
  // full width across the body.
  const body: InstrDesc[] = ALL_LOAD_SIMD_KINDS.map((k, i) => ({
    type: 'LoadSimd',
    memory: mem,
    loadSimdKind: k,
    memArg: memArgAt(i),
  }))

  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse; buildFunction is
  // MIRROR-WALRUS, so this ill-typed-but-well-formed body builds directly).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 20 kinds are present, and a u64::MAX offset survived exactly
  // (proving the MemArg bigint path is lossless at the full width for LoadSimd).
  const kinds = new Set(read.map((d) => d.loadSimdKind?.type))
  t.is(kinds.size, ALL_LOAD_SIMD_KINDS.length)
  t.true(
    read.some((d) => d.memArg?.offset === U64_MAX),
    'a u64::MAX MemArg offset must round-trip exactly',
  )
  // A lane variant carries its exact lane (not swapped with another kind's).
  const store8 = read.find((d) => d.loadSimdKind?.type === 'V128Store8Lane')!
  t.deepEqual(store8.loadSimdKind, { type: 'V128Store8Lane', lane: 14 })
})

test('C6b: a well-typed LoadSimd body (splat/load-lane/store-lane) emits valid wasm and round-trips through re-parse', (t) => {
  const m = empty()
  const mem = m.memories.addLocal(false, false, 1n, null, null).index
  // All 8-bit ops so align 1 (the natural access size) keeps the body valid wasm.
  const body: InstrDesc[] = [
    // i32.const 0 ; v128.load8_splat ; drop
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'LoadSimd', memory: mem, loadSimdKind: { type: 'Splat8' }, memArg: { align: 1, offset: 0n } },
    { type: 'Drop' },
    // i32.const 0 ; v128.const A ; v128.load8_lane 0 ; drop
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'V128', value: V128_A } },
    { type: 'LoadSimd', memory: mem, loadSimdKind: { type: 'V128Load8Lane', lane: 0 }, memArg: { align: 1, offset: 0n } },
    { type: 'Drop' },
    // i32.const 0 ; v128.const A ; v128.store8_lane 3  (pops address + vector)
    { type: 'Const', value: { type: 'I32', value: 0 } },
    { type: 'Const', value: { type: 'V128', value: V128_A } },
    { type: 'LoadSimd', memory: mem, loadSimdKind: { type: 'V128Store8Lane', lane: 3 }, memArg: { align: 1, offset: 0n } },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'simd6b'
  const bytes = m.emitWasm(false)

  // Independent proof the bytes are real, well-typed (stable) SIMD wasm.
  t.true(WebAssembly.validate(bytes))

  // Re-parse (all proposals on so every SIMD opcode decodes) and read back: the
  // ops, lanes, and MemArgs all decode to the same body.
  const reparsed = new ModuleConfig().onlyStableFeatures(false).parse(bytes)
  const read = reparsed.functions.byName('simd6b')!.instructions()
  t.deepEqual(read, body)
})

test('C6b: a large MemArg offset on a LoadSimd survives the emit -> bytes -> re-parse boundary (memory64)', (t) => {
  // A 64-bit offset needs a memory64 memory to stay well-typed on re-parse.
  const m = empty()
  const mem = m.memories.addLocal(false, true, 1n, null, null).index
  const body: InstrDesc[] = [
    { type: 'Const', value: { type: 'I64', value: 0n } },
    { type: 'LoadSimd', memory: mem, loadSimdKind: { type: 'Splat8' }, memArg: { align: 1, offset: BIG_OFFSET } },
    { type: 'Drop' },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'bigsimd'
  const bytes = m.emitWasm(false)
  t.true(WebAssembly.validate(bytes))
  const reparsed = new ModuleConfig().onlyStableFeatures(false).parse(bytes)
  const read = reparsed.functions.byName('bigsimd')!.instructions()
  t.deepEqual(read, body)
  t.is(read[1].memArg!.offset, BIG_OFFSET)
})

test('C6b negative: an out-of-range/foreign memory index on a LoadSimd throws catchably (no abort)', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null) // memory 0 exists; 5 does not
  t.throws(
    () =>
      m.buildFunction([], [], [], [
        { type: 'LoadSimd', memory: 5, loadSimdKind: { type: 'Splat8' }, memArg: { align: 1, offset: 0n } },
      ]),
    { message: /no memory at index 5/ },
  )
  // Process is still alive and the module was never mutated (the proof under WASI
  // that the bad index was caught pre-emit, not aborted).
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C6b negative: a LoadSimd descriptor missing a required field throws catchably', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null)
  // memory is resolved first.
  t.throws(
    () =>
      m.buildFunction([], [], [], [
        { type: 'LoadSimd', loadSimdKind: { type: 'Splat8' }, memArg: { align: 1, offset: 0n } },
      ]),
    { message: /`LoadSimd` instruction is missing its `memory` field/ },
  )
  // loadSimdKind is checked before memArg (matching emit's order).
  t.throws(
    () => m.buildFunction([], [], [], [{ type: 'LoadSimd', memory: 0, memArg: { align: 1, offset: 0n } }]),
    { message: /`LoadSimd` instruction is missing its `loadSimdKind` field/ },
  )
  t.throws(
    () => m.buildFunction([], [], [], [{ type: 'LoadSimd', memory: 0, loadSimdKind: { type: 'Splat8' } }]),
    { message: /`LoadSimd` instruction is missing its `memArg` field/ },
  )
})

test('C6b negative: a non-lossless MemArg offset on a LoadSimd throws catchably', (t) => {
  const m = empty()
  m.memories.addLocal(false, false, 1n, null, null)
  const ls = (offset: bigint): InstrDesc => ({
    type: 'LoadSimd',
    memory: 0,
    loadSimdKind: { type: 'Splat8' },
    memArg: { align: 1, offset },
  })
  // 2^64 is one past u64::MAX — not lossless.
  t.throws(() => m.buildFunction([], [], [], [ls(1n << 64n)]), {
    message: /MemArg offset must be a non-negative integer that fits in a u64/,
  })
  // A negative offset is rejected too (a u64 has no sign).
  t.throws(() => m.buildFunction([], [], [], [ls(-1n)]), {
    message: /MemArg offset must be a non-negative integer that fits in a u64/,
  })
  t.is(1 + 1, 2)
})

// ---------------------------------------------------------------------------
// C7a: wasm-GC struct + array instructions (20 variants: 6 struct + 14 array).
//
// The type-bearing ops REUSE `resolve_type_id` (rejects a nonexistent AND an
// internal function-entry-type index), exactly like C3's call_indirect; the
// data/elem array ops reuse `data_id_at` / `element_id_at`. `ArrayCopy` carries
// BOTH a destination (`typeIndex`) and a source (`srcTypeIndex`) array type.
//
// Same walrus fact as C1b/C2: `strict_validate(false)` is a no-op in 0.26.4, so
// an ill-typed body can never re-parse. The EXHAUSTIVE test therefore uses the
// IN-MEMORY read path (buildFunction -> instructions() on the same module, never
// touching the parser/validator), building the struct type via `types.addStruct`
// and the array types via `types.addArray` (B5a), a passive data segment via
// `data.addPassive`, and reusing the committed elements.wasm fixture's element
// segment (the binding has no `elements.add`). A separate WELL-TYPED body proves
// the ops survive the emit -> bytes -> re-parse boundary (GC needs
// onlyStableFeatures(false) on parse).
//
// MIRROR-WALRUS: buildFunction only guards process-aborting hazards (each
// type/data/element index resolves; required fields present). It does NOT
// type-check — a struct.get on an out-of-range field, an array.copy between
// mismatched element types, or a struct.new for an array type all build and emit
// as-is.
// ---------------------------------------------------------------------------

// A GC struct field list with an unpacked (i32) and a packed (i8) field, so the
// struct.get / struct.get_s / struct.get_u ops each have a plausible field.
const STRUCT_FIELDS = [
  { storage: { type: 'Val', value: I32 }, mutable: true },
  { storage: { type: 'I8' }, mutable: false },
] as const
const ARRAY_ELEMENT = { storage: { type: 'Val', value: I32 }, mutable: true } as const
const ARRAY_ELEMENT_2 = { storage: { type: 'Val', value: F32 }, mutable: false } as const

test('C7a EXHAUSTIVE: all 20 GC struct/array instrs round-trip in-memory', (t) => {
  // Start from the committed elements fixture (table 0 funcref + element[0]
  // ACTIVE / element[1] PASSIVE) so a genuine ElementId exists for the elem ops,
  // then add the GC types and a passive data segment programmatically.
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  const s = m.types.addStruct([...STRUCT_FIELDS]).index
  // TWO distinct array types so ArrayCopy's dst (`typeIndex`) and src
  // (`srcTypeIndex`) are provably not swapped on the round-trip.
  const a1 = m.types.addArray({ ...ARRAY_ELEMENT }).index
  const a2 = m.types.addArray({ ...ARRAY_ELEMENT_2 }).index
  const d = m.data.addPassive(new Uint8Array([1, 2, 3, 4])).index
  const e = 0 // element[0] from the fixture

  const body: InstrDesc[] = [
    // 6 struct ops.
    { type: 'StructNew', typeIndex: s },
    { type: 'StructNewDefault', typeIndex: s },
    { type: 'StructGet', typeIndex: s, field: 0 },
    { type: 'StructGetS', typeIndex: s, field: 1 },
    { type: 'StructGetU', typeIndex: s, field: 1 },
    { type: 'StructSet', typeIndex: s, field: 0 },
    // 14 array ops.
    { type: 'ArrayNew', typeIndex: a1 },
    { type: 'ArrayNewDefault', typeIndex: a1 },
    { type: 'ArrayNewFixed', typeIndex: a1, len: 3 },
    { type: 'ArrayNewData', typeIndex: a1, data: d },
    { type: 'ArrayNewElem', typeIndex: a1, elem: e },
    { type: 'ArrayGet', typeIndex: a1 },
    { type: 'ArrayGetS', typeIndex: a1 },
    { type: 'ArrayGetU', typeIndex: a1 },
    { type: 'ArraySet', typeIndex: a1 },
    { type: 'ArrayLen' },
    { type: 'ArrayFill', typeIndex: a1 },
    { type: 'ArrayCopy', typeIndex: a1, srcTypeIndex: a2 },
    { type: 'ArrayInitData', typeIndex: a1, data: d },
    { type: 'ArrayInitElem', typeIndex: a1, elem: e },
  ]

  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse; buildFunction is
  // MIRROR-WALRUS, so this ill-typed-but-well-formed body builds directly).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 20 instruction kinds are present.
  const kinds = new Set(read.map((dd) => dd.type))
  for (const k of [
    'StructNew',
    'StructNewDefault',
    'StructGet',
    'StructGetS',
    'StructGetU',
    'StructSet',
    'ArrayNew',
    'ArrayNewDefault',
    'ArrayNewFixed',
    'ArrayNewData',
    'ArrayNewElem',
    'ArrayGet',
    'ArrayGetS',
    'ArrayGetU',
    'ArraySet',
    'ArrayLen',
    'ArrayFill',
    'ArrayCopy',
    'ArrayInitData',
    'ArrayInitElem',
  ]) {
    t.true(kinds.has(k), `body should contain a ${k}`)
  }
  // ArrayCopy's dst (typeIndex) and src (srcTypeIndex) are distinct and not swapped.
  const copy = read.find((dd) => dd.type === 'ArrayCopy')!
  t.is(copy.typeIndex, a1)
  t.is(copy.srcTypeIndex, a2)
  // StructGet carries BOTH its struct type and its field index.
  const sget = read.find((dd) => dd.type === 'StructGet')!
  t.is(sget.typeIndex, s)
  t.is(sget.field, 0)
  // ArrayNewFixed carries its len; the data/elem ops carry their segment indices.
  t.is(read.find((dd) => dd.type === 'ArrayNewFixed')!.len, 3)
  t.is(read.find((dd) => dd.type === 'ArrayNewData')!.data, d)
  t.is(read.find((dd) => dd.type === 'ArrayNewElem')!.elem, e)
})

test('C7a: a well-typed struct/array body emits valid GC wasm and round-trips through re-parse', (t) => {
  const m = empty()
  const s = m.types.addStruct([...STRUCT_FIELDS]).index
  const a = m.types.addArray({ ...ARRAY_ELEMENT }).index

  // WELL-TYPED: struct.new_default pushes (ref $s), drop it; i32.const feeds
  // array.new_default's length, drop the resulting (ref $a).
  const body: InstrDesc[] = [
    { type: 'StructNewDefault', typeIndex: s },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 3 } },
    { type: 'ArrayNewDefault', typeIndex: a },
    { type: 'Drop' },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'gcwelltyped'
  const bytes = m.emitWasm(false)

  // Re-parse with GC enabled (onlyStableFeatures(false)); read the ops back. Look
  // up the struct/array type indices in the re-parsed module so the assertion is
  // robust to any type re-indexing across the emit -> parse boundary.
  const reparsed = new ModuleConfig().onlyStableFeatures(false).strictValidate(false).parse(bytes)
  const sIdx = reparsed.types.items().find((x) => x.kind === 'Struct')!.index
  const aIdx = reparsed.types.items().find((x) => x.kind === 'Array')!.index
  const read = reparsed.functions.byName('gcwelltyped')!.instructions()
  t.deepEqual(read, [
    { type: 'StructNewDefault', typeIndex: sIdx },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 3 } },
    { type: 'ArrayNewDefault', typeIndex: aIdx },
    { type: 'Drop' },
  ])
})

test('C7a negative: a nonexistent struct/array type index (or dst_ty/src_ty) throws catchably (no abort)', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  const a = m.types.addArray({ ...ARRAY_ELEMENT }).index

  t.throws(() => m.buildFunction([], [], [], [{ type: 'StructNew', typeIndex: 9999 }]), {
    message: /no type at index 9999/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNew', typeIndex: 9999 }]), {
    message: /no type at index 9999/,
  })
  // ArrayCopy resolves the DESTINATION (typeIndex) first: a bad dst throws.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayCopy', typeIndex: 9999, srcTypeIndex: a }]), {
    message: /no type at index 9999/,
  })
  // ...and the SOURCE (srcTypeIndex) is guarded too, alongside a valid dst.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayCopy', typeIndex: a, srcTypeIndex: 9999 }]), {
    message: /no type at index 9999/,
  })
  // Process is still alive and the module was never mutated (a real abort would
  // have taken the whole run down — the proof under WASI).
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C7a negative: a type index naming an internal entry type is rejected (resolve_type_id filter)', (t) => {
  const m = empty()
  // A local function with a MULTI-VALUE result signature has a MultiValue entry
  // block, so walrus records an internal ENTRY type in the raw type arena. That
  // entry type is never a real user struct/array type, so resolve_type_id must
  // reject a GC instr that names it (it would otherwise abort at emit via
  // get_type_index) — exactly the guard C3's call_indirect relies on.
  m.buildFunction(
    [],
    [I32, I32],
    [],
    [
      { type: 'Const', value: { type: 'I32', value: 0 } },
      { type: 'Const', value: { type: 'I32', value: 0 } },
    ],
  )
  // Add a struct AFTER, so it sits at a raw index ABOVE the entry type. The entry
  // type is then the "hole": a raw index the visible-type accessor skips (it
  // filters entry types out, exactly as resolve_type_id does).
  const s = m.types.addStruct([...STRUCT_FIELDS]).index
  let entryIndex = -1
  for (let i = 0; i < s; i++) {
    if (m.types.getByIndex(i) === null) {
      entryIndex = i
      break
    }
  }
  t.true(entryIndex >= 0, 'expected an internal entry type below the struct index')

  // Naming the entry type is rejected catchably for a struct ty AND for ArrayCopy's src_ty.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'StructNew', typeIndex: entryIndex }]), {
    message: new RegExp(`no type at index ${entryIndex}`),
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayCopy', typeIndex: s, srcTypeIndex: entryIndex }]), {
    message: new RegExp(`no type at index ${entryIndex}`),
  })
  t.is(1 + 1, 2)
})

test('C7a negative: an out-of-range data or element segment index throws catchably (no abort)', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  const a = m.types.addArray({ ...ARRAY_ELEMENT }).index
  // The fixture has element segments 0/1 but NO data segments.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNewData', typeIndex: a, data: 0 }]), {
    message: /no data segment at index 0/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayInitData', typeIndex: a, data: 5 }]), {
    message: /no data segment at index 5/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNewElem', typeIndex: a, elem: 7 }]), {
    message: /no element segment at index 7/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayInitElem', typeIndex: a, elem: 7 }]), {
    message: /no element segment at index 7/,
  })
  // The array type resolves BEFORE the data/elem segment (matching emit's order):
  // a bad type index throws even with a valid segment.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNewElem', typeIndex: 9999, elem: 0 }]), {
    message: /no type at index 9999/,
  })
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C7a negative: a struct/array descriptor missing a required field throws catchably', (t) => {
  const m = WasmModule.fromBuffer(ELEMENTS_FIXTURE)
  const s = m.types.addStruct([...STRUCT_FIELDS]).index
  const a = m.types.addArray({ ...ARRAY_ELEMENT }).index

  // Missing typeIndex on the type-bearing ops.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'StructNew' }]), {
    message: /`StructNew` instruction is missing its `typeIndex` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNew' }]), {
    message: /`ArrayNew` instruction is missing its `typeIndex` field/,
  })
  // struct.get needs its field index (resolved AFTER the type, matching emit).
  t.throws(() => m.buildFunction([], [], [], [{ type: 'StructGet', typeIndex: s }]), {
    message: /`StructGet` instruction is missing its `field` field/,
  })
  // array.new_fixed needs its len.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNewFixed', typeIndex: a }]), {
    message: /`ArrayNewFixed` instruction is missing its `len` field/,
  })
  // array.new_data needs its data segment; array.new_elem its element segment.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNewData', typeIndex: a }]), {
    message: /`ArrayNewData` instruction is missing its `data` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayNewElem', typeIndex: a }]), {
    message: /`ArrayNewElem` instruction is missing its `elem` field/,
  })
  // ArrayCopy needs BOTH type indices: typeIndex is checked before srcTypeIndex.
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayCopy', srcTypeIndex: a }]), {
    message: /`ArrayCopy` instruction is missing its `typeIndex` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ArrayCopy', typeIndex: a }]), {
    message: /`ArrayCopy` instruction is missing its `srcTypeIndex` field/,
  })
})

// ---------------------------------------------------------------------------
// C7b: GC reference instructions — the 11 label-free ops (the `br_on_*`
// label-carriers are C7c). NO new InstrDesc field: CallRef/ReturnCallRef REUSE
// `typeIndex` + `resolve_type_id` (rejects a nonexistent AND an internal
// function-entry-type index), exactly like C3's call_indirect; RefTest/RefCast
// REUSE `refType` + the module-aware heap resolution, exactly like C4's RefNull
// (walrus stores the payload as two fields, `nullable` + `heap_type`, which the
// shared `RefType` object bundles); the other 7 ops are fieldless.
//
// Same walrus fact as C1b/C2/C7a: `strict_validate(false)` is a no-op in
// 0.26.4, so the EXHAUSTIVE test uses the IN-MEMORY read path (buildFunction ->
// instructions() on the same module). A separate WELL-TYPED body proves the ops
// survive the emit -> bytes -> re-parse boundary (GC/func-refs need
// onlyStableFeatures(false) on parse).
//
// MIRROR-WALRUS: buildFunction only guards process-aborting hazards (the callee
// type / concrete heap type resolves; required fields present). It does NOT
// type-check — a call_ref naming a STRUCT type, a ref.test on an empty stack,
// or an i31.get_s with no i31 operand all build as-is.
// ---------------------------------------------------------------------------

test('C7b EXHAUSTIVE: all 11 GC reference instrs round-trip in-memory', (t) => {
  const m = empty()
  const fn = m.types.add([I32], [I32]).index
  const s = m.types.addStruct([...STRUCT_FIELDS]).index

  const body: InstrDesc[] = [
    { type: 'RefAsNonNull' },
    // The typed calls reference a real FUNCTION type.
    { type: 'CallRef', typeIndex: fn },
    { type: 'ReturnCallRef', typeIndex: fn },
    { type: 'RefI31' },
    { type: 'I31GetS' },
    { type: 'I31GetU' },
    // RefTest with an ABSTRACT heap, nullable TRUE...
    { type: 'RefTest', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Any' } } },
    // ...and nullable FALSE — proving the flag round-trips rather than defaults.
    { type: 'RefTest', refType: { nullable: false, heap: { type: 'Abstract', kind: 'Eq' } } },
    // RefTest with a CONCRETE struct heap — the abort-guarded path.
    { type: 'RefTest', refType: { nullable: false, heap: { type: 'Concrete', typeIndex: s } } },
    // RefCast: abstract (i31) AND concrete, with BOTH nullabilities.
    { type: 'RefCast', refType: { nullable: false, heap: { type: 'Abstract', kind: 'I31' } } },
    { type: 'RefCast', refType: { nullable: true, heap: { type: 'Concrete', typeIndex: s } } },
    { type: 'AnyConvertExtern' },
    { type: 'ExternConvertAny' },
    { type: 'RefEq' },
  ]

  const idx = m.buildFunction([], [], [], body)
  // Read back from the SAME in-memory module (no emit/re-parse; buildFunction is
  // MIRROR-WALRUS, so this ill-typed-but-well-formed body builds directly).
  const read = m.functions.getByIndex(idx)!.instructions()
  t.deepEqual(read, body)

  // Sanity: all 11 instruction kinds are present.
  const kinds = new Set(read.map((dd) => dd.type))
  for (const k of [
    'RefAsNonNull',
    'CallRef',
    'ReturnCallRef',
    'RefI31',
    'I31GetS',
    'I31GetU',
    'RefTest',
    'RefCast',
    'AnyConvertExtern',
    'ExternConvertAny',
    'RefEq',
  ]) {
    t.true(kinds.has(k), `body should contain a ${k}`)
  }
  // The typed calls carried the FUNCTION type index (not the struct's).
  t.is(read.find((dd) => dd.type === 'CallRef')!.typeIndex, fn)
  t.is(read.find((dd) => dd.type === 'ReturnCallRef')!.typeIndex, fn)
  // The concrete RefTest/RefCast carried the struct type index unchanged, and
  // the nullable flag is per-instruction (not defaulted).
  const concreteTest = read.find((dd) => dd.type === 'RefTest' && dd.refType!.heap.type === 'Concrete')!
  t.is((concreteTest.refType!.heap as { type: 'Concrete'; typeIndex: number }).typeIndex, s)
  t.false(concreteTest.refType!.nullable)
  const concreteCast = read.find((dd) => dd.type === 'RefCast' && dd.refType!.heap.type === 'Concrete')!
  t.is((concreteCast.refType!.heap as { type: 'Concrete'; typeIndex: number }).typeIndex, s)
  t.true(concreteCast.refType!.nullable)
})

test('C7b: a well-typed GC reference body emits valid wasm and round-trips through re-parse', (t) => {
  const m = empty()
  // WELL-TYPED throughout (traps only at runtime, which validation permits):
  // ref.i31 on an i32.const feeds i31.get_s / i31.get_u / ref.as_non_null;
  // ref.null any feeds ref.test / ref.cast (eq is a supertype-hierarchy sibling
  // under any, so the test/cast validate); two eqrefs feed ref.eq; a null extern
  // feeds any.convert_extern and a null any feeds extern.convert_any.
  const body: InstrDesc[] = [
    { type: 'Const', value: { type: 'I32', value: 5 } },
    { type: 'RefI31' },
    { type: 'I31GetS' },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 6 } },
    { type: 'RefI31' },
    { type: 'I31GetU' },
    { type: 'Drop' },
    { type: 'Const', value: { type: 'I32', value: 7 } },
    { type: 'RefI31' },
    { type: 'RefAsNonNull' },
    { type: 'Drop' },
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Any' } } },
    { type: 'RefTest', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Eq' } } },
    { type: 'Drop' },
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Any' } } },
    { type: 'RefCast', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Eq' } } },
    { type: 'Drop' },
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Eq' } } },
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Eq' } } },
    { type: 'RefEq' },
    { type: 'Drop' },
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Extern' } } },
    { type: 'AnyConvertExtern' },
    { type: 'Drop' },
    { type: 'RefNull', refType: { nullable: true, heap: { type: 'Abstract', kind: 'Any' } } },
    { type: 'ExternConvertAny' },
    { type: 'Drop' },
  ]
  const idx = m.buildFunction([], [], [], body)
  m.functions.getByIndex(idx)!.name = 'gcref'
  const bytes = m.emitWasm(false)

  // Re-parse with GC enabled (onlyStableFeatures(false)); the abstract-heap-only
  // body needs no index fixup, so the read must deep-equal what was built.
  const reparsed = new ModuleConfig().onlyStableFeatures(false).strictValidate(false).parse(bytes)
  const read = reparsed.functions.byName('gcref')!.instructions()
  t.deepEqual(read, body)
})

test('C7b negative: a nonexistent or entry-type callee type on call_ref/return_call_ref throws catchably (no abort)', (t) => {
  const m = empty()
  t.throws(() => m.buildFunction([], [], [], [{ type: 'CallRef', typeIndex: 9999 }]), {
    message: /no type at index 9999/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCallRef', typeIndex: 9999 }]), {
    message: /no type at index 9999/,
  })

  // A MULTI-VALUE result signature records an internal ENTRY type in the raw
  // arena (the C7a trick): naming it must be rejected by resolve_type_id too.
  m.buildFunction(
    [],
    [I32, I32],
    [],
    [
      { type: 'Const', value: { type: 'I32', value: 0 } },
      { type: 'Const', value: { type: 'I32', value: 0 } },
    ],
  )
  const s = m.types.addStruct([...STRUCT_FIELDS]).index
  let entryIndex = -1
  for (let i = 0; i < s; i++) {
    if (m.types.getByIndex(i) === null) {
      entryIndex = i
      break
    }
  }
  t.true(entryIndex >= 0, 'expected an internal entry type below the struct index')
  t.throws(() => m.buildFunction([], [], [], [{ type: 'CallRef', typeIndex: entryIndex }]), {
    message: new RegExp(`no type at index ${entryIndex}`),
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCallRef', typeIndex: entryIndex }]), {
    message: new RegExp(`no type at index ${entryIndex}`),
  })
  // Process is still alive and the failed builds left the module emittable.
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C7b negative: a RefTest/RefCast concrete or exact heap naming a bad type index throws catchably (no abort)', (t) => {
  const m = empty()
  t.throws(
    () =>
      m.buildFunction(
        [],
        [],
        [],
        [{ type: 'RefTest', refType: { nullable: true, heap: { type: 'Concrete', typeIndex: 9999 } } }],
      ),
    { message: /no type at index 9999/ },
  )
  // The Exact heap path is guarded by the same resolution.
  t.throws(
    () =>
      m.buildFunction(
        [],
        [],
        [],
        [{ type: 'RefCast', refType: { nullable: false, heap: { type: 'Exact', typeIndex: 9999 } } }],
      ),
    { message: /no type at index 9999/ },
  )
  // Process survived and the module was never mutated.
  t.is(1 + 1, 2)
  t.notThrows(() => m.emitWasm(false))
})

test('C7b negative: a GC reference descriptor missing a required field throws catchably', (t) => {
  const m = empty()
  t.throws(() => m.buildFunction([], [], [], [{ type: 'CallRef' }]), {
    message: /`CallRef` instruction is missing its `typeIndex` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'ReturnCallRef' }]), {
    message: /`ReturnCallRef` instruction is missing its `typeIndex` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'RefTest' }]), {
    message: /`RefTest` instruction is missing its `refType` field/,
  })
  t.throws(() => m.buildFunction([], [], [], [{ type: 'RefCast' }]), {
    message: /`RefCast` instruction is missing its `refType` field/,
  })
})
