/// <reference lib="webworker" />
//
// DEPLOY REQUIREMENT — this module is bundled to a hashed `assets/worker-*.js` and spawned from the
// COEP:require-corp /playground document. A dedicated worker created by a require-corp document is
// itself blocked unless its OWN response carries `Cross-Origin-Embedder-Policy: require-corp`. That
// header is set on `/assets/*` in void.json. BUT hashed-asset cache keys are unversioned and survive
// deploys, so a no-COEP response cached before the header existed keeps being served even after the
// rule is added — the worker then spawns with `ONERROR` and the playground hangs. The only reliable
// fix is to mint a NEW asset hash so the URL has no stale cache history. The BUILD_TAG below changes
// the emitted bytes (a comment alone is stripped by minification and does NOT change the hash). Bump
// it whenever the COEP/serving story changes and a clean asset URL is needed.
import { Buffer } from 'buffer'
import type { WorkerRequest, WorkerResponse, InspectResult, GraphNode, GraphEdge, SectionSummary, NodeKind, PropPair, Edit, BuildPresetId, BuildInstrDesc } from './protocol'
import type { WalrusMod, WasmModule, WType, WImport, WExport, InstrDesc } from './_walrus'
import { valTypeLabel } from './_walrus'

// Names the worker thread (visible in devtools) AND, as a live side effect on `self`, survives
// minification — so editing BUILD_TAG mints a fresh `assets/worker-*.js` hash. See header note.
const BUILD_TAG = 'napi-wasm-tools-engine@coep-2026-07-23'
;(self as { name?: string }).name = BUILD_TAG

// @napi-rs/wasm-tools (walrus) is emnapi too; its emit/return paths use Node Buffer, and the emnapi
// runtime needs globalThis.Buffer defined BEFORE the dynamic import, or it throws NotSupportBufferError.
if (typeof (globalThis as { Buffer?: unknown }).Buffer === 'undefined') {
  ;(globalThis as { Buffer?: unknown }).Buffer = Buffer
}

// ── wabt (wat → wasm), lazily initialized once ───────────────────────────────
type WabtFeatures = Record<string, boolean>
interface WabtModule {
  resolveNames(): void
  validate(): void
  toBinary(opts: { log: boolean; write_debug_names: boolean }): { buffer: Uint8Array }
  destroy(): void
}
interface Wabt {
  parseWat(filename: string, text: string, features?: WabtFeatures): WabtModule
}
let wabtPromise: Promise<Wabt> | null = null
async function getWabt(): Promise<Wabt> {
  if (!wabtPromise) {
    const wabtInit = (await import('wabt')).default as unknown as () => Promise<Wabt>
    wabtPromise = wabtInit()
  }
  return wabtPromise
}

function watToWasm(wabt: Wabt, watText: string): Uint8Array {
  const m = wabt.parseWat('module.wat', watText, {
    simd: true,
    threads: true,
    reference_types: true,
    bulk_memory: true,
    mutable_globals: true,
    gc: true,
    exceptions: true,
    tail_call: true,
    multi_memory: true,
  })
  // resolveNames/validate/toBinary can each throw on invalid input; destroy()
  // MUST still run or the wabt wasm-heap allocation leaks. Repeated invalid input
  // would otherwise accumulate unbounded memory in this long-lived worker.
  try {
    m.resolveNames()
    m.validate()
    const { buffer } = m.toBinary({ log: false, write_debug_names: true })
    return buffer
  } finally {
    m.destroy()
  }
}

// Copy into a FRESH ArrayBuffer so the result is transferable even when the wasm heap is a
// SharedArrayBuffer (threads → shared memory). Used by the Edit + Build ops that emit new wasm.
function toArrayBuffer(out: Uint8Array): ArrayBuffer {
  const ab = new ArrayBuffer(out.byteLength)
  new Uint8Array(ab).set(out)
  return ab
}

type Mod = WalrusMod

// ── graph construction ───────────────────────────────────────────────────────
const nid = (kind: NodeKind, index: number) => `${kind}:${index}`

function sig(params: string[], results: string[]): string {
  const p = params.length ? params.join(', ') : ''
  const r = results.length ? results.join(', ') : ''
  return `(${p})${r ? ` → ${r}` : ''}`
}

function typeSig(t: WType): string | undefined {
  if (t.kind !== 'Function') return t.kind.toLowerCase()
  try {
    return sig(t.params().map(valTypeLabel), t.results().map(valTypeLabel))
  } catch {
    return undefined
  }
}

// Per-section node budget. The per-item getters each do an O(n) arena liveness
// scan, so materializing every node of a huge module is O(n²) and would wedge
// the worker (and swamp the SVG). We render the first MAX_PER_SECTION of each
// family and mark the section truncated; the full count is still reported.
const MAX_PER_SECTION = 250
// Global caps so a valid-but-pathological module (a body with 100k calls, an element
// segment repeating one function 20k times) can't build an unbounded edge payload that
// stalls postMessage or the SVG. Distinct call edges are separately capped so we stop
// reading function bodies once the call graph is already saturated.
//
// These bound the RESULT (node/edge counts). They do NOT bound CONSTRUCTION cost: the
// binding only exposes whole-collection `items()`, whole-body `instructions()`, and a
// cloning `data.value` — no bounded slice / raw-length / streaming-visitor APIs — so a
// giant module still materializes those before we cap. Bounding construction needs new
// binding APIs (tracked for the walrus package, out of scope for the site); here the
// worker's 60s per-request timeout + the browser's own memory ceiling are the backstop.
const MAX_EDGES = 4000
const MAX_CALL_EDGES = 1500
// The node cap bounds node COUNT but not the text each node carries: a module with
// 250 exports whose names are 100k chars each is a valid <64MB upload that would post
// a ~50MB JSON. Clip every user-controlled string (names) to keep the payload bounded.
const MAX_TEXT = 256
const clipText = (s: string) => (s.length > MAX_TEXT ? `${s.slice(0, MAX_TEXT)}… (${s.length} chars)` : s)

function cap<T>(items: T[]): { shown: T[]; total: number; truncated: boolean } {
  if (items.length <= MAX_PER_SECTION) return { shown: items, total: items.length, truncated: false }
  return { shown: items.slice(0, MAX_PER_SECTION), total: items.length, truncated: true }
}

// Walk a function body and collect the functions it references via Call / ReturnCall /
// RefFunc. Deduped AS WE GO via `seen` (a body calling one target 100k times yields a
// single edge, not 100k intermediate entries), bounded by `budget`, and descending every
// nested body: block/loop `seq`, if `consequent`/`alternative`, and legacy try-handler
// `catches[i].seq` (a Call inside a catch clause would otherwise be silently dropped).
type CallEdge = { from: number; to: number; kind: 'Call' | 'ReturnCall' | 'RefFunc' }
function collectCalls(
  instrs: InstrDesc[],
  from: number,
  seen: Set<string>,
  out: CallEdge[],
  budget: { left: number },
): void {
  for (const ins of instrs) {
    if (budget.left <= 0) return
    if (ins.func != null && (ins.type === 'Call' || ins.type === 'ReturnCall' || ins.type === 'RefFunc')) {
      const key = `${from}->${ins.func}:${ins.type}`
      if (!seen.has(key)) {
        seen.add(key)
        out.push({ from, to: ins.func, kind: ins.type })
        budget.left--
      }
    }
    if (ins.seq) collectCalls(ins.seq, from, seen, out, budget)
    if (ins.consequent) collectCalls(ins.consequent, from, seen, out, budget)
    if (ins.alternative) collectCalls(ins.alternative, from, seen, out, budget)
    if (ins.catches) for (const c of ins.catches) if (c.seq) collectCalls(c.seq, from, seen, out, budget)
  }
}

function buildGraph(m: WasmModule): InspectResult {
  const nodes: GraphNode[] = []
  const edges: GraphEdge[] = []
  const sections: SectionSummary[] = []
  let edgeSeq = 0
  let edgesTruncated = false
  const edge = (from: string, to: string, label?: string) => {
    if (edges.length >= MAX_EDGES) {
      edgesTruncated = true
      return
    }
    edges.push({ id: `e${edgeSeq++}`, from, to, label })
  }

  const section = (kind: NodeKind, label: string, ids: string[], total?: number, truncated?: boolean) =>
    sections.push({ kind, label, count: total ?? ids.length, nodeIds: ids, truncated: truncated ?? false })

  // types
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.types))
    for (const t of shown) {
      const id = nid('type', t.index)
      ids.push(id)
      const s = typeSig(t)
      nodes.push({
        id,
        kind: 'type',
        index: t.index,
        label: t.name ?? `type ${t.index}`,
        sub: s,
        props: [
          { key: 'index', value: String(t.index) },
          { key: 'kind', value: t.kind },
          ...(s ? [{ key: 'signature', value: s }] : []),
        ],
      })
    }
    section('type', 'Types', ids, total, truncated)
  }

  // imports (shown slice captured so the defines-edge pass below stays within the
  // per-section cap — walking ALL imports there would re-introduce the O(n²) resolve).
  const shownImports: WImport[] = []
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.imports))
    shownImports.push(...shown)
    for (const im of shown) {
      const id = nid('import', im.index)
      ids.push(id)
      nodes.push({
        id,
        kind: 'import',
        index: im.index,
        label: `${im.module}.${im.name}`,
        sub: im.kind,
        props: [
          { key: 'index', value: String(im.index) },
          { key: 'module', value: im.module },
          { key: 'name', value: im.name },
          { key: 'kind', value: im.kind },
        ],
      })
    }
    section('import', 'Imports', ids, total, truncated)
  }

  // functions
  const shownFuncIndices = new Set<number>()
  const callEdges: CallEdge[] = []
  const callSeen = new Set<string>()
  const callBudget = { left: MAX_CALL_EDGES }
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.functions))
    for (const f of shown) {
      const id = nid('function', f.index)
      ids.push(id)
      shownFuncIndices.add(f.index)
      let s: string | undefined
      let tyIndex: number | undefined
      try {
        const t = f.ty()
        tyIndex = t.index
        s = typeSig(t)
      } catch {
        /* deleted / uninitialized */
      }
      nodes.push({
        id,
        kind: 'function',
        index: f.index,
        label: f.name ?? `func ${f.index}`,
        sub: s,
        props: [
          { key: 'index', value: String(f.index) },
          { key: 'name', value: f.name ?? '—' },
          { key: 'kind', value: f.kind },
          ...(s ? [{ key: 'signature', value: s }] : []),
          ...(tyIndex != null ? [{ key: 'type', value: `type ${tyIndex}` }] : []),
        ],
      })
      if (tyIndex != null) edge(id, nid('type', tyIndex), 'type')
      // Local functions have a body; collect their fn→fn call references (bounded).
      if (f.kind === 'Local') {
        if (callBudget.left > 0) {
          const instrs = safeCall(() => f.instructions())
          if (instrs) collectCalls(instrs, f.index, callSeen, callEdges, callBudget)
          // null ⇒ the body was too deep for the binding to read, so its calls are
          // omitted; flag the graph as partial rather than pretend it's complete.
          else edgesTruncated = true
        }
        // Once the call budget is spent (here or in an earlier function), this and
        // every later local body's calls are dropped — so the call graph is partial.
        if (callBudget.left <= 0) edgesTruncated = true
      }
    }
    section('function', 'Functions', ids, total, truncated)
  }

  // globals
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.globals))
    for (const g of shown) {
      const id = nid('global', g.index)
      ids.push(id)
      const ty = g.ty
      const tyLabel = valTypeLabel(ty)
      nodes.push({
        id,
        kind: 'global',
        index: g.index,
        label: g.name ?? `global ${g.index}`,
        sub: `${g.mutable ? 'mut ' : ''}${tyLabel}`,
        props: [
          { key: 'index', value: String(g.index) },
          { key: 'name', value: g.name ?? '—' },
          { key: 'type', value: tyLabel },
          { key: 'mutable', value: String(g.mutable) },
          { key: 'shared', value: String(g.shared) },
          { key: 'kind', value: g.kind },
        ],
      })
      // global → type edge only when the valtype is a concrete ref into the type arena
      if (ty.type === 'Ref' && (ty.heap.type === 'Concrete' || ty.heap.type === 'Exact')) {
        edge(id, nid('type', ty.heap.typeIndex), 'type')
      }
    }
    section('global', 'Globals', ids, total, truncated)
  }

  // memories
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.memories))
    for (const mem of shown) {
      const id = nid('memory', mem.index)
      ids.push(id)
      const range = `${mem.initial}${mem.maximum != null ? `..${mem.maximum}` : ''} pages`
      nodes.push({
        id,
        kind: 'memory',
        index: mem.index,
        label: mem.name ?? `mem ${mem.index}`,
        sub: range,
        props: [
          { key: 'index', value: String(mem.index) },
          { key: 'name', value: mem.name ?? '—' },
          { key: 'initial', value: String(mem.initial) },
          { key: 'maximum', value: mem.maximum != null ? String(mem.maximum) : '—' },
          { key: 'shared', value: String(mem.shared) },
          { key: 'memory64', value: String(mem.memory64) },
          { key: 'imported', value: String(mem.isImported) },
        ],
      })
    }
    section('memory', 'Memories', ids, total, truncated)
  }

  // tables
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.tables))
    for (const tb of shown) {
      const id = nid('table', tb.index)
      ids.push(id)
      const elTy = valTypeLabel(tb.elementTy)
      const range = `${tb.initial}${tb.maximum != null ? `..${tb.maximum}` : ''}`
      nodes.push({
        id,
        kind: 'table',
        index: tb.index,
        label: tb.name ?? `table ${tb.index}`,
        sub: `${elTy} ${range}`,
        props: [
          { key: 'index', value: String(tb.index) },
          { key: 'name', value: tb.name ?? '—' },
          { key: 'element', value: elTy },
          { key: 'initial', value: String(tb.initial) },
          { key: 'maximum', value: tb.maximum != null ? String(tb.maximum) : '—' },
          { key: 'imported', value: String(tb.isImported) },
        ],
      })
    }
    section('table', 'Tables', ids, total, truncated)
  }

  // data segments
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.data))
    for (const d of shown) {
      const id = nid('data', d.index)
      ids.push(id)
      let byteLen = 0
      try {
        byteLen = d.value.byteLength
      } catch {
        /* ignore */
      }
      nodes.push({
        id,
        kind: 'data',
        index: d.index,
        label: d.name ?? `data ${d.index}`,
        sub: `${d.kind} · ${byteLen}B`,
        props: [
          { key: 'index', value: String(d.index) },
          { key: 'name', value: d.name ?? '—' },
          { key: 'kind', value: d.kind },
          { key: 'bytes', value: String(byteLen) },
        ],
      })
      const mem = safeCall(() => d.memory())
      if (mem) edge(id, nid('memory', mem.index), 'inits')
    }
    section('data', 'Data', ids, total, truncated)
  }

  // element segments
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.elements))
    for (const el of shown) {
      const id = nid('element', el.index)
      ids.push(id)
      nodes.push({
        id,
        kind: 'element',
        index: el.index,
        label: el.name ?? `elem ${el.index}`,
        sub: `${el.kind} · ${el.itemsKind}`,
        props: [
          { key: 'index', value: String(el.index) },
          { key: 'name', value: el.name ?? '—' },
          { key: 'kind', value: el.kind },
          { key: 'items', value: el.itemsKind },
        ],
      })
      const table = safeCall(() => el.table())
      if (table) edge(id, nid('table', table.index), 'inits')
      // An element may repeat the same function thousands of times; one edge per
      // distinct target is enough (and keeps the payload bounded).
      const fns = safeCall(() => el.functionItems())
      if (fns) {
        const seenTargets = new Set<number>()
        for (const f of fns) {
          if (seenTargets.has(f.index)) continue
          seenTargets.add(f.index)
          edge(id, nid('function', f.index), 'ref')
        }
      } else if (el.itemsKind === 'Expressions') {
        // Expression-form segments can hold `ref.func` targets, but the binding
        // exposes them as const-expressions, not function handles — so those refs
        // are omitted here. Flag the graph partial rather than look complete.
        edgesTruncated = true
      }
    }
    section('element', 'Elements', ids, total, truncated)
  }

  // tags
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.tags))
    for (const tg of shown) {
      const id = nid('tag', tg.index)
      ids.push(id)
      let tyIndex: number | undefined
      try {
        tyIndex = tg.ty().index
      } catch {
        /* ignore */
      }
      nodes.push({
        id,
        kind: 'tag',
        index: tg.index,
        label: tg.name ?? `tag ${tg.index}`,
        sub: tg.kind,
        props: [
          { key: 'index', value: String(tg.index) },
          { key: 'name', value: tg.name ?? '—' },
          { key: 'kind', value: tg.kind },
          ...(tyIndex != null ? [{ key: 'type', value: `type ${tyIndex}` }] : []),
        ],
      })
      if (tyIndex != null) edge(id, nid('type', tyIndex), 'type')
    }
    section('tag', 'Tags', ids, total, truncated)
  }

  // exports (drawn last so the layered layout puts them on the right)
  {
    const ids: string[] = []
    const { shown, total, truncated } = cap(safeItems(m.exports))
    for (const ex of shown) {
      const id = nid('export', ex.index)
      ids.push(id)
      nodes.push({
        id,
        kind: 'export',
        index: ex.index,
        label: ex.name,
        sub: ex.kind,
        props: [
          { key: 'index', value: String(ex.index) },
          { key: 'name', value: ex.name },
          { key: 'kind', value: ex.kind },
        ],
      })
      const target = exportTarget(ex)
      if (target) edge(id, target, 'exports')
    }
    section('export', 'Exports', ids, total, truncated)
  }

  // imports → the item they define (drawn after all items exist). Only the shown
  // (capped) imports, so an import-heavy module can't restore the quadratic walk.
  for (const im of shownImports) {
    const from = nid('import', im.index)
    const target = importTarget(im)
    if (target) edge(from, target, 'defines')
  }

  // fn → fn call edges (only between functions that are both in the shown set, so
  // an edge never dangles to a node truncated by the per-section budget). Deduped
  // so a function calling the same target twice draws one edge.
  const seenCalls = new Set<string>()
  for (const c of callEdges) {
    if (!shownFuncIndices.has(c.to)) continue
    const label = c.kind === 'RefFunc' ? 'ref.func' : 'calls'
    const key = `${c.from}->${c.to}:${label}`
    if (seenCalls.has(key)) continue
    seenCalls.add(key)
    edge(nid('function', c.from), nid('function', c.to), label)
  }

  // Drop edges whose target (or source) was capped out of its section — an edge to
  // a node that was never materialized would render nowhere and just bloat the payload.
  const nodeIdSet = new Set(nodes.map((n) => n.id))
  const prunedEdges = edges.filter((e) => nodeIdSet.has(e.from) && nodeIdSet.has(e.to))
  if (prunedEdges.length !== edges.length) edgesTruncated = true

  // Clip user-controlled text once, centrally, so no node inflates the payload.
  for (const n of nodes) {
    n.label = clipText(n.label)
    if (n.sub) n.sub = clipText(n.sub)
    for (const p of n.props) p.value = clipText(p.value)
  }
  const moduleName = safeGet(() => m.name, null)

  return {
    moduleName: moduleName != null ? clipText(moduleName) : null,
    nodes,
    edges: prunedEdges,
    sections,
    edgesTruncated,
  }
}

function exportTarget(ex: WExport): string | null {
  switch (ex.kind) {
    case 'Function':
      return withIndex(() => ex.func(), 'function')
    case 'Global':
      return withIndex(() => ex.global(), 'global')
    case 'Memory':
      return withIndex(() => ex.memory(), 'memory')
    case 'Table':
      return withIndex(() => ex.table(), 'table')
    case 'Tag':
      return withIndex(() => ex.tag(), 'tag')
    default:
      return null
  }
}

function importTarget(im: WImport): string | null {
  switch (im.kind) {
    case 'Function':
      return withIndex(() => im.func(), 'function')
    case 'Global':
      return withIndex(() => im.global(), 'global')
    case 'Memory':
      return withIndex(() => im.memory(), 'memory')
    case 'Table':
      return withIndex(() => im.table(), 'table')
    case 'Tag':
      return withIndex(() => im.tag(), 'tag')
    default:
      return null
  }
}

function withIndex(get: () => { index: number } | null, kind: NodeKind): string | null {
  const item = safeCall(get)
  return item ? nid(kind, item.index) : null
}

// ── null-safe helpers (all resolver getters may throw/return null) ────────────
function safeItems<T>(col: { items(): T[] } | undefined): T[] {
  try {
    return col?.items() ?? []
  } catch {
    return []
  }
}
function safeCall<T>(fn: () => T): T | null {
  try {
    return fn()
  } catch {
    return null
  }
}
function safeGet<T>(fn: () => T, fallback: T): T {
  try {
    return fn()
  } catch {
    return fallback
  }
}

// ── edit application (write-through handles) ─────────────────────────────────
// Each edit resolves a live handle by stable index and mutates it in place.
// Missing handles (stale index) are skipped rather than throwing so one bad edit
// can't sink a whole batch. `pages` is normalized to BigInt for the memory setter.
function applyEditList(m: WasmModule, edits: Edit[]): void {
  for (const e of edits) {
    switch (e.kind) {
      case 'renameExport': {
        const ex = m.exports.getByIndex(e.index)
        if (ex) ex.name = e.newName
        break
      }
      case 'toggleGlobalMutable': {
        const g = m.globals.getByIndex(e.index)
        if (g) g.mutable = !g.mutable
        break
      }
      case 'setModuleName': {
        m.name = e.name
        break
      }
      case 'setMemoryInitial': {
        const mem = m.memories.getByIndex(e.index)
        if (mem) mem.initial = BigInt(e.pages)
        break
      }
    }
  }
}

// Flag the after-graph nodes that an edit targeted, so the UI can paint them amber.
// (setModuleName has no owning node — it surfaces via the module-name field.)
function markEdited(res: InspectResult, edits: Edit[]): void {
  const ids = new Set<string>()
  for (const e of edits) {
    if (e.kind === 'renameExport') ids.add(nid('export', e.index))
    else if (e.kind === 'toggleGlobalMutable') ids.add(nid('global', e.index))
    else if (e.kind === 'setMemoryInitial') ids.add(nid('memory', e.index))
  }
  for (const n of res.nodes) if (ids.has(n.id)) n.edited = true
}

// ── build (IR builder) ────────────────────────────────────────────────────────
// The canonical 8-byte empty module every preset builds on top of.
const EMPTY_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0])

// Structured-clone / JSON safety: I64 const values are BigInt and must be
// stringified before postMessage. Everything else is already a plain number.
function normalizeInstr(d: InstrDesc): BuildInstrDesc {
  const out: BuildInstrDesc = { type: d.type }
  if (d.local != null) out.local = d.local
  if (d.global != null) out.global = d.global
  if (d.func != null) out.func = d.func
  if (d.op != null) out.op = d.op
  if (d.value != null) {
    const v = d.value.value
    out.value = { type: d.value.type, value: typeof v === 'bigint' ? v.toString() : (v as number) }
  }
  return out
}

// Build one preset into a fresh module, export it, and hand back the exported
// name + emitted bytes + the round-tripped instruction body. The `mod` namespace
// supplies both the WasmModule ctor and the ready-made ValType constants.
function buildPreset(
  mod: Mod,
  preset: BuildPresetId,
): { name: string; emitted: Uint8Array; instructions: BuildInstrDesc[] } {
  const { WasmModule, I32 } = mod
  const m = WasmModule.fromBuffer(EMPTY_MODULE)

  let idx: number
  let name: string
  if (preset === 'add') {
    const a = m.locals.add(I32)
    const b = m.locals.add(I32)
    idx = m.buildFunction([I32, I32], [I32], [a.index, b.index], [
      { type: 'LocalGet', local: a.index },
      { type: 'LocalGet', local: b.index },
      { type: 'Binop', op: 'I32Add' },
    ])
    name = 'add'
  } else if (preset === 'const42') {
    idx = m.buildFunction([], [I32], [], [{ type: 'Const', value: { type: 'I32', value: 42 } }])
    name = 'const42'
  } else {
    const p0 = m.locals.add(I32)
    idx = m.buildFunction([I32], [I32], [p0.index], [{ type: 'LocalGet', local: p0.index }])
    name = 'identity'
  }

  const fn = m.functions.getByIndex(idx)
  if (!fn) throw new Error('buildFunction returned an index with no function')
  m.exports.addFunction(name, fn)
  const emitted = m.emitWasm(false)
  // Re-parse the emitted bytes and read the body back from the FRESH module — a
  // true round-trip proof (the pre-emit handle would just echo the input tree,
  // proving nothing about what actually baked into the wasm). Building on an empty
  // module, the sole function keeps index `idx` (0) across emit + re-parse.
  const reparsed = WasmModule.fromBuffer(emitted)
  const rfn = reparsed.functions.getByIndex(idx)
  if (!rfn) throw new Error('re-parsed module is missing the built function')
  const instructions = rfn.instructions().map(normalizeInstr)
  return { name, emitted, instructions }
}

// ── dispatcher ────────────────────────────────────────────────────────────────
self.onmessage = async (e: MessageEvent<WorkerRequest>) => {
  const { id, op, bytes } = e.data
  const post = (msg: WorkerResponse, transfer: Transferable[] = []) =>
    (self as unknown as Worker).postMessage(msg, transfer)
  try {
    if (op.kind === 'inspect') {
      let wasmBytes: Uint8Array
      if (op.format === 'wat') {
        const wabt = await getWabt()
        const text = new TextDecoder('utf-8').decode(new Uint8Array(bytes))
        wasmBytes = watToWasm(wabt, text)
      } else {
        wasmBytes = new Uint8Array(bytes)
      }
      const mod = (await import('@napi-rs/wasm-tools')) as unknown as Mod
      const module = mod.WasmModule.fromBuffer(wasmBytes)
      const result = buildGraph(module)
      post({ id, ok: true, kind: 'inspect', result })
      return
    }
    if (op.kind === 'applyEdits') {
      let wasmBytes: Uint8Array
      if (op.format === 'wat') {
        const wabt = await getWabt()
        const text = new TextDecoder('utf-8').decode(new Uint8Array(bytes))
        wasmBytes = watToWasm(wabt, text)
      } else {
        wasmBytes = new Uint8Array(bytes)
      }
      const mod = (await import('@napi-rs/wasm-tools')) as unknown as Mod
      const module = mod.WasmModule.fromBuffer(wasmBytes)
      const before = buildGraph(module)
      applyEditList(module, op.edits)
      // Emit → re-parse so the after-graph reflects what actually baked into bytes.
      const emitted = module.emitWasm(false)
      const after = buildGraph(mod.WasmModule.fromBuffer(emitted))
      markEdited(after, op.edits)
      const ab = toArrayBuffer(emitted)
      post({ id, ok: true, kind: 'applyEdits', before, after, emitted: ab }, [ab])
      return
    }
    if (op.kind === 'buildFn') {
      const mod = (await import('@napi-rs/wasm-tools')) as unknown as Mod
      const { name, emitted, instructions } = buildPreset(mod, op.preset)
      // Copy into a plain ArrayBuffer: it's both the BufferSource we instantiate
      // from AND the transferable we return (the wasm heap may be a SharedArrayBuffer).
      const ab = toArrayBuffer(emitted)
      const { instance } = await WebAssembly.instantiate(ab, {})
      const fn = instance.exports[name] as (...args: number[]) => number | bigint
      const raw = fn(...op.args)
      const result = typeof raw === 'bigint' ? raw.toString() : raw
      post({ id, ok: true, kind: 'buildFn', result, emitted: ab, instructions }, [ab])
      return
    }
    post({ id, ok: false, error: `Unknown op: ${(op as { kind: string }).kind}` })
  } catch (err) {
    post({ id, ok: false, error: err instanceof Error ? err.message : String(err) })
  }
}
