// website/pages/playground/protocol.ts
// Message + graph types shared between the UI island and the engine worker. The
// UI imports NONE of the wasm package — everything it needs to render is plain
// JSON produced by the worker (`InspectResult`), mirroring the reference
// playground's "no wasm import in the UI" principle.

// The ten node families we surface from a module, one per WasmModule collection.
export type NodeKind =
  | 'type'
  | 'import'
  | 'function'
  | 'global'
  | 'memory'
  | 'table'
  | 'data'
  | 'element'
  | 'tag'
  | 'export'

// A single row of the detail panel (ordered key → value display pairs).
export type PropPair = { key: string; value: string }

export type GraphNode = {
  id: string // `${kind}:${index}`
  kind: NodeKind
  index: number
  label: string // short handle label shown in the node/row
  sub?: string // secondary line (e.g. type signature, module.name)
  props: PropPair[] // full property list for the detail panel
  edited?: boolean // reserved: amber highlight for a mutated node (Edit mode)
}

export type GraphEdge = {
  id: string
  from: string // node id
  to: string // node id
  label?: string
}

export type SectionSummary = {
  kind: NodeKind
  label: string // plural human label, e.g. "Functions"
  count: number // the FULL item count (may exceed nodeIds.length when truncated)
  nodeIds: string[] // the node ids actually emitted (capped at MAX_PER_SECTION)
  truncated?: boolean // true when count > nodeIds.length (large-module node budget hit)
}

export type InspectResult = {
  moduleName: string | null
  nodes: GraphNode[]
  edges: GraphEdge[]
  sections: SectionSummary[]
  // true when the graph omitted edges — a global edge/call budget was hit, or a
  // function body was too deep to read — so the UI can flag the graph as partial.
  edgesTruncated?: boolean
}

// ── Edits ─────────────────────────────────────────────────────────────────────
// A single write-through mutation applied to a parsed module via its live handles.
// Every edit keys the target by its STABLE index (the same index carried on a
// GraphNode), so the UI can build these straight from the inspect graph.
export type Edit =
  | { kind: 'renameExport'; index: number; newName: string }
  | { kind: 'toggleGlobalMutable'; index: number }
  | { kind: 'setModuleName'; name: string | null }
  // `pages` arrives as a string/number from the UI; the worker converts to BigInt.
  | { kind: 'setMemoryInitial'; index: number; pages: string | number }

// ── Build (IR builder) ────────────────────────────────────────────────────────
// The worker owns the actual descriptor bodies (they need the runtime ValType
// constants). The UI drives which preset to build and the integer args to call
// the emitted export with. Metadata below is shared so both sides agree on the
// preset list, arg labels, and the snippet shown to the user.
export type BuildPresetId = 'add' | 'const42' | 'identity'

export type BuildPreset = {
  id: BuildPresetId
  name: string // exported function name
  title: string // human label for the picker
  signature: string // display signature, e.g. "(a: i32, b: i32) → i32"
  argLabels: string[] // one editable integer input per param (empty ⇒ no args)
  defaultArgs: number[] // seed values, same length as argLabels
  source: string // IR-builder snippet shown alongside the result
}

export const BUILD_PRESETS: BuildPreset[] = [
  {
    id: 'add',
    name: 'add',
    title: 'add — a + b',
    signature: '(a: i32, b: i32) → i32',
    argLabels: ['a', 'b'],
    defaultArgs: [2, 3],
    source: `const a = m.locals.add(I32)
const b = m.locals.add(I32)
const idx = m.buildFunction([I32, I32], [I32], [a.index, b.index], [
  { type: 'LocalGet', local: a.index },
  { type: 'LocalGet', local: b.index },
  { type: 'Binop', op: 'I32Add' },
])
m.exports.addFunction('add', m.functions.getByIndex(idx)!)`,
  },
  {
    id: 'const42',
    name: 'const42',
    title: 'const42 — returns 42',
    signature: '() → i32',
    argLabels: [],
    defaultArgs: [],
    source: `const idx = m.buildFunction([], [I32], [], [
  { type: 'Const', value: { type: 'I32', value: 42 } },
])
m.exports.addFunction('const42', m.functions.getByIndex(idx)!)`,
  },
  {
    id: 'identity',
    name: 'identity',
    title: 'identity — returns x',
    signature: '(x: i32) → i32',
    argLabels: ['x'],
    defaultArgs: [7],
    source: `const p0 = m.locals.add(I32)
const idx = m.buildFunction([I32], [I32], [p0.index], [
  { type: 'LocalGet', local: p0.index },
])
m.exports.addFunction('identity', m.functions.getByIndex(idx)!)`,
  },
]

// A structured-clone-safe mirror of walrus `InstrDesc` (I64 const values are
// stringified so the round-tripped list survives postMessage and JSON display).
export type BuildInstrDesc = {
  type: string
  local?: number
  global?: number
  func?: number
  op?: string
  value?: { type: string; value: number | string }
}

// ── Ops ─────────────────────────────────────────────────────────────────────
// `inspect`, `applyEdits`, and `buildFn` are all implemented end-to-end.
export type InspectFormat = 'wat' | 'wasm'
export type InspectOp = { kind: 'inspect'; format: InspectFormat }
// The request `bytes` carry the ORIGINAL source (wat text or wasm); the worker
// parses it, snapshots the before-graph, applies `edits` through handles, emits,
// re-parses, and returns before/after/emitted.
export type ApplyEditsOp = { kind: 'applyEdits'; format: InspectFormat; edits: Edit[] }
// Build a preset function from an empty module, export it, emit, instantiate, and
// call the export with `args`. Request `bytes` are ignored (an empty buffer).
export type BuildFnOp = { kind: 'buildFn'; preset: BuildPresetId; args: number[] }
export type Op = InspectOp | ApplyEditsOp | BuildFnOp

// ── Worker message envelope (generic, id-correlated) ─────────────────────────
export type WorkerRequest = { id: number; op: Op; bytes: ArrayBuffer }
export type WorkerInspectOk = { id: number; ok: true; kind: 'inspect'; result: InspectResult }
export type WorkerApplyEditsOk = {
  id: number
  ok: true
  kind: 'applyEdits'
  before: InspectResult
  after: InspectResult // changed nodes carry `edited: true`
  emitted: ArrayBuffer // freshly-emitted wasm bytes (transferable)
}
export type WorkerBuildOk = {
  id: number
  ok: true
  kind: 'buildFn'
  result: number | string // the value returned by calling the emitted export
  emitted: ArrayBuffer // freshly-built wasm bytes (transferable)
  instructions: BuildInstrDesc[] // round-tripped body read back via fn.instructions()
}
export type WorkerOk = WorkerInspectOk | WorkerApplyEditsOk | WorkerBuildOk
export type WorkerErr = { id: number; ok: false; error: string }
export type WorkerResponse = WorkerOk | WorkerErr
