import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ConstExpr, ModuleConfig, WasmModule, type InstrDesc, type ValType } from '../index'

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
    () => m.buildFunction([], [], [], [{ type: 'Block', blockType: { type: 'MultiValue', typeIndex: T + 1 }, seq: [] }]),
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
// abort. All three instruction walks (buildFunction preflight + emit, and the
// instructions() read) recurse once per control-flow level; walrus itself is
// fully ITERATIVE and imposes no nesting limit, so without a cap a deep body
// (build) or a deep parsed module (read) would overflow the native stack — a
// SIGABRT that catch_unwind cannot catch, tearing down the whole Node process
// across FFI. MAX_NESTING_DEPTH converts that into a catchable error at the cap.
// The wasm32-wasi run is the real proof: a genuine overflow there aborts the
// whole run, so a passing wasi run is the evidence the abort is gone.
// ---------------------------------------------------------------------------

// Must match src/ir.rs::MAX_NESTING_DEPTH.
const MAX_NESTING_DEPTH = 256

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
  const over = nestedBlocks(MAX_NESTING_DEPTH + 5) // 261 nested Blocks — past the cap
  t.throws(() => m.buildFunction([], [], [], over), {
    message: /instruction nesting too deep \(max 256\)/,
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
    message: /instruction nesting too deep \(max 256\)/,
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

test('C1b negative: a deferred lane-carrier op name is not buildable (rejected catchably)', (t) => {
  // The 14 lane-carrying SIMD ops are deferred to C6; from_str rejects their
  // names (they are not buildable without a lane index this task does not model).
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Unop', op: 'I8x16ExtractLaneS' }]), {
    message: /unknown unary operator `I8x16ExtractLaneS`/,
  })
  t.throws(() => empty().buildFunction([], [], [], [{ type: 'Binop', op: 'I8x16ReplaceLane' }]), {
    message: /unknown binary operator `I8x16ReplaceLane`/,
  })
})

// A hand-authored module whose one function contains a lane-carrier op:
// (i32.const 0)(i8x16.splat)(i8x16.extract_lane_s 0)(drop). Produced from walrus
// directly (buildFunction cannot emit a lane op), so the read path can be
// exercised against a genuine lane-carrying instruction.
const LANE_CARRIER_MODULE = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00, 0x03, 0x02,
  0x01, 0x00, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x0c, 0x01, 0x0a, 0x00, 0x41, 0x00,
  0xfd, 0x0f, 0xfd, 0x15, 0x00, 0x1a, 0x0b,
])

test('C1b negative: reading a module containing a lane-carrier op throws catchably (deferred to C6)', (t) => {
  const m = new ModuleConfig().onlyStableFeatures(false).parse(LANE_CARRIER_MODULE)
  const f = m.functions.getByIndex(0)!
  t.throws(() => f.instructions(), {
    message: /deferred to the SIMD task \(C6\)/,
  })
  // Process survived the guarded read.
  t.is(1 + 1, 2)
})
