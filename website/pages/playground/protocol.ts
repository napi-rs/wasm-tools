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
  count: number
  nodeIds: string[]
}

export type InspectResult = {
  moduleName: string | null
  nodes: GraphNode[]
  edges: GraphEdge[]
  sections: SectionSummary[]
}

// ── Ops ─────────────────────────────────────────────────────────────────────
// `inspect` is implemented end-to-end. `applyEdits` / `buildFn` are stubbed
// (worker returns a "coming soon" error) so the plumbing + UI can land now.
export type InspectFormat = 'wat' | 'wasm'
export type InspectOp = { kind: 'inspect'; format: InspectFormat }
export type ApplyEditsOp = { kind: 'applyEdits' }
export type BuildFnOp = { kind: 'buildFn' }
export type Op = InspectOp | ApplyEditsOp | BuildFnOp

// ── Worker message envelope (generic, id-correlated) ─────────────────────────
export type WorkerRequest = { id: number; op: Op; bytes: ArrayBuffer }
export type WorkerOk = { id: number; ok: true; kind: 'inspect'; result: InspectResult }
export type WorkerErr = { id: number; ok: false; error: string }
export type WorkerResponse = WorkerOk | WorkerErr
