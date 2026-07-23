// website/pages/playground/_Playground.tsx
// Interactive orchestrator island: WAT/​.wasm input → engine worker → module graph.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { PlaygroundEngine } from './_engine'
import type { RunResult } from './_engine'
import type { InspectResult, GraphNode, Edit, InspectFormat, BuildPresetId, BuildInstrDesc } from './protocol'
import { BUILD_PRESETS } from './protocol'
import { WAT_SAMPLES, DEFAULT_WAT } from './_samples'
import GraphView from './_GraphView'
import TreeView from './_TreeView'
import DetailPanel from './_DetailPanel'
import CodeBlock from '../_components/CodeBlock'

type Status = 'empty' | 'running' | 'done' | 'error'
type Mode = 'inspect' | 'edit' | 'build'
type View = 'graph' | 'tree'

// ── Edit-mode form model ─────────────────────────────────────────────────────
// A flat, index-keyed mirror of the parts of a module Edit mode can mutate. Both
// the live form and the pristine baseline use this shape; diffing the two yields
// the minimal `Edit[]` the worker applies through handles.
type EditForm = {
  moduleName: string
  exportNames: Record<number, string>
  globalMutable: Record<number, boolean>
  memoryInitial: Record<number, string>
}

function getProp(n: GraphNode, key: string): string | undefined {
  return n.props.find((p) => p.key === key)?.value
}

function buildForm(r: InspectResult): EditForm {
  const exportNames: Record<number, string> = {}
  const globalMutable: Record<number, boolean> = {}
  const memoryInitial: Record<number, string> = {}
  for (const n of r.nodes) {
    if (n.kind === 'export') exportNames[n.index] = n.label
    else if (n.kind === 'global') globalMutable[n.index] = getProp(n, 'mutable') === 'true'
    else if (n.kind === 'memory') memoryInitial[n.index] = getProp(n, 'initial') ?? '0'
  }
  return { moduleName: r.moduleName ?? '', exportNames, globalMutable, memoryInitial }
}

function diffEdits(base: EditForm, cur: EditForm): Edit[] {
  const edits: Edit[] = []
  if (cur.moduleName !== base.moduleName) {
    // Transmit the exact string the user typed. Only a truly-empty field clears the name
    // (→ null); a whitespace-only name is a real, binding-accepted value and must NOT be
    // silently normalized to null — that would desync the form from the emitted bytes.
    edits.push({ kind: 'setModuleName', name: cur.moduleName === '' ? null : cur.moduleName })
  }
  for (const key of Object.keys(cur.exportNames)) {
    const i = Number(key)
    const v = cur.exportNames[i]
    // Transmit the exact new name — including '' (a valid, binding-accepted export name).
    // Dropping empty names would make "clear this export name" a silent zero-pending no-op.
    if (v !== base.exportNames[i]) edits.push({ kind: 'renameExport', index: i, newName: v })
  }
  for (const key of Object.keys(cur.globalMutable)) {
    const i = Number(key)
    if (cur.globalMutable[i] !== base.globalMutable[i]) edits.push({ kind: 'toggleGlobalMutable', index: i })
  }
  for (const key of Object.keys(cur.memoryInitial)) {
    const i = Number(key)
    const v = cur.memoryInitial[i]
    if (v !== base.memoryInitial[i] && v.trim() !== '' && /^\d+$/.test(v.trim()))
      edits.push({ kind: 'setMemoryInitial', index: i, pages: v.trim() })
  }
  return edits
}

// ── Build-arg parsing ─────────────────────────────────────────────────────────
// Preset params are i32 and the inputs are <input type="number">, which accepts
// scientific notation and decimals. Parse with Number (NOT parseInt — that turns
// "1e3" into 1) and reject anything that isn't a whole i32, visibly, instead of
// silently coercing empty/malformed input to 0 and running a different call.
const I32_MIN = -2_147_483_648
const I32_MAX = 2_147_483_647
type ArgCheck = { value: number } | { error: string }
function parseI32Arg(raw: string): ArgCheck {
  const s = raw.trim()
  if (s === '') return { error: 'required' }
  // Require a PLAIN integer literal. Number() would silently round or underflow decimal
  // and exponent forms to an integer that passes Number.isInteger — "1e-999" → 0,
  // "1.00000000000000001" → 1 — and then run a different call than the user typed. BigInt
  // over the exact digits sidesteps all Binary64 rounding (exponent forms are rejected;
  // type a plain integer).
  if (!/^-?\d+$/.test(s)) return { error: 'must be a whole number' }
  const n = BigInt(s)
  if (n < BigInt(I32_MIN) || n > BigInt(I32_MAX)) return { error: 'out of i32 range' }
  return { value: Number(n) }
}

// ── Build-mode descriptor rendering ──────────────────────────────────────────
// Render one round-tripped instruction as its object-literal — the same shape
// fed to buildFunction, so the list doubles as round-trip proof.
function fmtInstr(d: BuildInstrDesc): string {
  const parts: string[] = [`type: '${d.type}'`]
  if (d.local != null) parts.push(`local: ${d.local}`)
  if (d.global != null) parts.push(`global: ${d.global}`)
  if (d.func != null) parts.push(`func: ${d.func}`)
  if (d.op != null) parts.push(`op: '${d.op}'`)
  if (d.value != null) {
    const v = d.value.value
    const lit = typeof v === 'string' ? v : String(v)
    parts.push(`value: { type: '${d.value.type}', value: ${lit} }`)
  }
  return `{ ${parts.join(', ')} }`
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function instrsToHtml(instrs: BuildInstrDesc[]): string {
  const body = instrs.map((d) => escapeHtml(fmtInstr(d))).join('\n')
  return `<pre class="font-mono text-xs leading-relaxed text-(--color-fg)">${body}</pre>`
}

// The PUBLISHED @napi-rs/wasm-tools-wasm32-wasi@1.0.1 binary predates the module-graph API
// (#158/#159). This site instead ships a vendored pre-release build (website/vendor/, aliased
// in vite.config.ts) that HAS the full API, so Inspect works today. Once a >=1.0.2 wasm is
// published to npm, delete the vendor dir and point the dep back at the registry version.
const STALE_WASM_NOTE =
  'Preview build — this playground runs a vendored pre-release of @napi-rs/wasm-tools with the full module-graph API. The public npm release (1.0.1) predates it; a ≥ 1.0.2 publish will replace the vendored copy.'

// Upload guards: reject oversized files up front, and abort a read that never
// settles, so a multi-GB or stalled (e.g. cloud-backed) file can't exhaust the main
// thread or leave the UI locked with reload as the only recovery.
const MAX_UPLOAD_MB = 64
const MAX_UPLOAD_BYTES = MAX_UPLOAD_MB * 1024 * 1024
const READ_TIMEOUT_MS = 30_000

// ── Static (non-isolated) fallback: a no-wasm code tour ──────────────────────
function StaticFallback() {
  return (
    <div className="container-page py-16">
      <p className="eyebrow mb-3">Playground</p>
      <h1 className="text-display-lg mb-4 font-display text-(--color-fg)">See the shape of your wasm</h1>
      <div className="mb-10 rounded-xl border border-(--color-border) bg-(--color-surface-1) p-6">
        <p className="mb-2 font-medium text-(--color-fg)">In-browser demo unavailable</p>
        <p className="text-sm text-(--color-muted)">
          Your browser could not enable cross-origin isolation (SharedArrayBuffer), which the
          in-browser wasm engine needs. Try a recent Chrome or Firefox, or run the tools locally:
        </p>
        <pre className="mt-4 overflow-x-auto rounded-lg border border-(--color-border) bg-(--color-bg) px-4 py-3 font-mono text-xs text-(--color-fg)">
          npm i @napi-rs/wasm-tools
        </pre>
      </div>

      <p className="mb-4 text-sm text-(--color-muted)">
        What the playground does — parse a module and walk its live handle graph:
      </p>
      <pre className="mb-4 overflow-x-auto rounded-xl border border-(--color-border) bg-(--color-surface-1) p-5 font-mono text-xs leading-relaxed text-(--color-fg)">
        {`import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(bytes)
for (const fn of mod.functions.items()) {
  console.log(fn.index, fn.name, fn.kind)
}
for (const ex of mod.exports.items()) {
  console.log(ex.name, '→', ex.func()?.index)
}`}
      </pre>
      <p className="mb-10 text-xs text-(--color-faint)">
        Heads up: <code className="font-mono">fromBuffer</code> and the collection graph above ship in{' '}
        <span className="font-mono">@napi-rs/wasm-tools ≥ 1.0.2</span>, which is not on npm yet — the current{' '}
        <span className="font-mono">1.0.1</span> release exposes <span className="font-mono">ModuleConfig</span> plus{' '}
        <span className="font-mono">fromPath</span>/<span className="font-mono">emitWasm</span>. This site runs a
        vendored pre-release build. See the{' '}
        <a href="/docs" className="underline hover:text-(--color-fg)">
          docs version note
        </a>{' '}
        for details.
      </p>

      <p className="mb-4 text-sm text-(--color-muted)">Sample modules seeded in the editor:</p>
      <div className="flex flex-col gap-4">
        {WAT_SAMPLES.map((s) => (
          <div key={s.name} className="rounded-xl border border-(--color-border) bg-(--color-surface-1) p-4">
            <p className="mb-1 font-mono text-sm text-(--color-accent)">{s.name}</p>
            <p className="text-xs text-(--color-faint)">{s.note}</p>
          </div>
        ))}
      </div>
    </div>
  )
}

export default function Playground() {
  // ----- mount guard (avoid SSR/CSR mismatch) -----
  const [mounted, setMounted] = useState(false)
  useEffect(() => {
    setMounted(true)
  }, [])

  // ----- engine (lazy) + dispose -----
  const engineRef = useRef<PlaygroundEngine | null>(null)
  useEffect(
    () => () => {
      engineRef.current?.dispose()
      engineRef.current = null
      readerRef.current?.abort()
      readerRef.current = null
    },
    [],
  )

  // ----- input / state -----
  const [mode, setMode] = useState<Mode>('inspect')
  const [wat, setWat] = useState(DEFAULT_WAT)
  const [sourceLabel, setSourceLabel] = useState<string>('module.wat')
  // Which input is the ACTIVE source: the WAT editor, or an uploaded binary. A binary
  // upload makes the WAT editor inert (the graph/edits act on the file, not the textarea),
  // so the editor is deactivated until the user switches back — keeping the displayed
  // source and the actionable source in agreement.
  const [sourceKind, setSourceKind] = useState<'wat' | 'wasm'>('wat')
  const [status, setStatus] = useState<Status>('empty')
  const [errorMsg, setErrorMsg] = useState('')
  const [result, setResult] = useState<InspectResult | null>(null)
  const [view, setView] = useState<View>('graph')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [dragging, setDragging] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  // The in-flight upload reader, tracked so a superseding selection or unmount can
  // abort it instead of letting a large/stalled read run on unattended.
  const readerRef = useRef<FileReader | null>(null)
  // Monotonic request generation. Every engine op bumps it and captures its value;
  // a reply is applied only if it's still the latest, so an out-of-order completion
  // (e.g. a slow inspect landing after a newer inspect/build) can never overwrite
  // fresher state or leave the retained source pointing at a different module.
  const genRef = useRef(0)

  // ----- edit state -----
  // The exact source that produced `result`, retained so Edit mode re-parses the
  // SAME module (a later textarea edit can't shift the indices the form keys on).
  const sourceFormatRef = useRef<InspectFormat>('wat')
  const sourceWatRef = useRef<string>('')
  const sourceWasmRef = useRef<ArrayBuffer | null>(null)
  const [form, setForm] = useState<EditForm | null>(null)
  const [afterResult, setAfterResult] = useState<InspectResult | null>(null)
  const [emitted, setEmitted] = useState<ArrayBuffer | null>(null)
  const [applying, setApplying] = useState(false)

  // ----- build state -----
  const [buildPreset, setBuildPreset] = useState<BuildPresetId>(BUILD_PRESETS[0].id)
  const [buildArgs, setBuildArgs] = useState<string[]>(BUILD_PRESETS[0].defaultArgs.map(String))
  const [building, setBuilding] = useState(false)
  const [buildResult, setBuildResult] = useState<{
    preset: BuildPresetId
    name: string
    args: number[]
    result: number | string
    instructions: BuildInstrDesc[]
    emitted: ArrayBuffer
  } | null>(null)
  const activePreset = useMemo(
    () => BUILD_PRESETS.find((p) => p.id === buildPreset) ?? BUILD_PRESETS[0],
    [buildPreset],
  )
  // One parse result per arg input; drives per-field error styling and the Build gate.
  const argChecks = useMemo(
    () => activePreset.argLabels.map((_, i) => parseI32Arg(buildArgs[i] ?? '')),
    [activePreset, buildArgs],
  )
  const argsValid = argChecks.every((c) => 'value' in c)
  // Switching presets reseeds the arg inputs and clears any prior result.
  const selectPreset = useCallback((id: BuildPresetId) => {
    const p = BUILD_PRESETS.find((x) => x.id === id) ?? BUILD_PRESETS[0]
    setBuildPreset(id)
    setBuildArgs(p.defaultArgs.map(String))
    setBuildResult(null)
  }, [])

  // Pristine baseline for diffing; rebuilt whenever a fresh inspect lands.
  const baseline = useMemo(() => (result ? buildForm(result) : null), [result])
  // Which editable names are CLIPPED previews (their full value isn't in the DTO),
  // so those fields must be locked — editing one would write the preview back.
  const clipInfo = useMemo(() => {
    const exports = new Set<number>()
    if (result) for (const n of result.nodes) if (n.kind === 'export' && n.labelClipped) exports.add(n.index)
    return { exports, moduleName: result?.moduleNameClipped ?? false }
  }, [result])
  // Any edit to the form invalidates a previously-applied result: its after-graph and
  // downloadable bytes no longer describe the current form, so drop them until re-apply.
  // (The form is reset together with `result` in applyResult, so this fires only on
  // genuine user edits — never lagging a fresh inspect.)
  useEffect(() => {
    setAfterResult(null)
    setEmitted(null)
  }, [form])

  // In Edit mode the right pane shows the after-graph once edits are applied.
  const displayResult = mode === 'edit' && afterResult ? afterResult : result

  const selectedNode: GraphNode | null = useMemo(
    () => displayResult?.nodes.find((n) => n.id === selectedId) ?? null,
    [displayResult, selectedId],
  )

  const pendingEdits = useMemo(
    () => (form && baseline ? diffEdits(baseline, form) : []),
    [form, baseline],
  )

  // Memory-pages fields must be non-negative integers. A CHANGED-but-invalid field
  // (empty, decimal, non-numeric) is dropped by diffEdits, so without this guard Apply
  // would still run any OTHER valid edit and emit bytes that contradict the shown form.
  const memoryErrors = useMemo(() => {
    const bad = new Set<number>()
    if (!form || !baseline) return bad
    for (const key of Object.keys(form.memoryInitial)) {
      const i = Number(key)
      const v = form.memoryInitial[i]
      if (v !== baseline.memoryInitial[i] && !/^\d+$/.test(v.trim())) bad.add(i)
    }
    return bad
  }, [form, baseline])
  const editFormValid = memoryErrors.size === 0

  const ensureEngine = () => {
    if (!engineRef.current) engineRef.current = new PlaygroundEngine()
    return engineRef.current
  }

  // Clears the entire inspect/edit/download session — graph, edit form, after-result,
  // selection, and emitted download. Every path that supersedes the current module (a
  // source edit, a file selection, a failed inspection) goes through this, so none can
  // leave a stale, still-actionable session behind a source that has already changed.
  // (The retained source refs are only read when `form` exists, so nulling the form is
  // enough to make Apply unavailable.)
  const resetSession = useCallback(() => {
    setResult(null)
    setForm(null)
    setAfterResult(null)
    setEmitted(null)
    setSelectedId(null)
    // Release the retained source the superseded session held. The wasm buffer can be up
    // to MAX_UPLOAD_BYTES, so keeping it would pin tens of MB per stale upload; the WAT
    // text already lives independently in `wat`. Apply is gated on `form` (also cleared
    // above), so nulling these can never strand an in-progress edit.
    sourceWasmRef.current = null
    sourceWatRef.current = ''
  }, [])

  // A failed inspection leaves NO valid current module. Clear the whole session so a
  // previously inspected module can't still be applied or downloaded while the editor
  // shows the failed new source.
  const failInspect = useCallback(
    (msg: string) => {
      resetSession()
      // A failed inspection committed no binary, so drop back to the WAT editor: leaving
      // sourceKind='wasm' would deactivate the editor and falsely claim a binary is loaded
      // and actionable. (A WAT-source failure is already 'wat', so this is a no-op there.)
      setSourceKind('wat')
      setStatus('error')
      setErrorMsg(msg)
    },
    [resetSession],
  )

  // Every change to the displayed WAT source (example pick or textarea edit) must
  // invalidate the current inspection: the graph, edit form, after-result, selection, and
  // emitted download all describe the PREVIOUS source, and the retained source refs Apply
  // reads (sourceWatRef/…) still point at it. Bumping the generation also supersedes any
  // in-flight inspect/read, so a reply for the old source can't commit against the new
  // one. Routing both controls through here keeps the actionable session (Apply/Download)
  // strictly matched to what's on screen — a stale module can never be applied or
  // downloaded behind a changed editor. Source controls are also disabled while an op is
  // running, so this never races a settling engine reply. Selecting WAT also returns the
  // active source to the editor (a prior binary upload is discarded).
  const changeSource = useCallback(
    (next: string) => {
      resetSession()
      setWat(next)
      setSourceKind('wat')
      genRef.current++
      setStatus('empty')
      setErrorMsg('')
    },
    [resetSession],
  )

  const applyResult = useCallback(
    (r: RunResult) => {
      if (r.ok && r.kind === 'inspect') {
        // Commit the result AND its derived edit form in the SAME batched update, so no
        // intermediate render ever pairs the new module with the previous module's form.
        // Otherwise a click landing in that window could apply stale (possibly clipped)
        // edits — diffed against the new baseline — to the new source.
        setResult(r.result)
        setForm(buildForm(r.result))
        setAfterResult(null)
        setEmitted(null)
        setSelectedId(r.result.nodes[0]?.id ?? null)
        setStatus('done')
      } else if (!r.ok) {
        failInspect(r.error)
      }
    },
    [failInspect],
  )

  const inspectWat = useCallback(async () => {
    const gen = ++genRef.current
    setStatus('running')
    setErrorMsg('')
    try {
      const bytes = new TextEncoder().encode(wat)
      const r = await ensureEngine().run({ kind: 'inspect', format: 'wat' }, bytes.buffer as ArrayBuffer)
      if (gen !== genRef.current) return // superseded by a newer request
      // Commit the retained source ONLY on success, together with the result, so the
      // source Edit mode re-parses always matches the graph currently on screen.
      if (r.ok && r.kind === 'inspect') {
        sourceFormatRef.current = 'wat'
        sourceWatRef.current = wat
        sourceWasmRef.current = null // drop any retained upload — WAT is the source now
        setSourceLabel('module.wat')
      }
      applyResult(r)
    } catch (err) {
      if (gen !== genRef.current) return
      failInspect(String(err))
    }
  }, [wat, applyResult, failInspect])

  const inspectWasm = useCallback(
    // `gen` is stamped by the caller (handleFiles) BEFORE the async file read, so the
    // newest SELECTION always owns the newest generation regardless of which read
    // finishes first — a slow large file selected first can't clobber a later choice.
    async (bytes: ArrayBuffer, name: string, gen: number) => {
      setStatus('running')
      setErrorMsg('')
      try {
        const r = await ensureEngine().run({ kind: 'inspect', format: 'wasm' }, bytes.slice(0))
        if (gen !== genRef.current) return // superseded by a newer request
        if (r.ok && r.kind === 'inspect') {
          sourceFormatRef.current = 'wasm'
          // `bytes` is intact (only a slice was transferred to the engine), so retain
          // it directly instead of making a second full copy of the upload.
          sourceWasmRef.current = bytes
          setSourceLabel(name)
        }
        applyResult(r)
      } catch (err) {
        if (gen !== genRef.current) return
        failInspect(String(err))
      }
    },
    [applyResult, failInspect],
  )

  const handleFiles = useCallback(
    (files: FileList | null) => {
      if (!files || files.length === 0) return
      const file = files[0]
      // Reject oversized files before reading a byte — no busy state is entered.
      if (file.size > MAX_UPLOAD_BYTES) {
        setStatus('error')
        setErrorMsg(`${file.name} is ${(file.size / (1024 * 1024)).toFixed(1)} MB — over the ${MAX_UPLOAD_MB} MB in-browser limit.`)
        return
      }
      // Stamp the generation NOW (synchronously, at selection time) so ordering is
      // decided by selection, not by which FileReader happens to finish first.
      const gen = ++genRef.current
      // Supersede the previous module BEFORE the async read: drop the whole session so a
      // stale graph/edit form/download can't stay actionable while the file loads (or if
      // the read fails), and mark the binary the active source so the WAT editor — which
      // still shows the OLD text and can't represent this binary — is deactivated.
      resetSession()
      setSourceKind('wasm')
      setSourceLabel(file.name) // name the active binary now so the banner reads right during the read
      setStatus('running')
      setErrorMsg('')
      // Abort a read still running from a superseded selection, then track this one.
      readerRef.current?.abort()
      const reader = new FileReader()
      readerRef.current = reader
      // A read that never settles (stalled/cloud-backed file) would leave the UI
      // busy forever; abort it after a ceiling so onabort can release the lock.
      const timer = setTimeout(() => reader.abort(), READ_TIMEOUT_MS)
      const settle = () => {
        clearTimeout(timer)
        if (readerRef.current === reader) readerRef.current = null
      }
      reader.onerror = () => {
        settle()
        if (gen !== genRef.current) return
        // No bytes committed — return to the WAT editor rather than leaving it deactivated
        // behind a "binary loaded" banner for a file that never loaded.
        setSourceKind('wat')
        setStatus('error')
        setErrorMsg(`Could not read ${file.name}.`)
      }
      reader.onabort = () => {
        settle()
        if (gen !== genRef.current) return // superseded/unmounted: a newer op owns the UI
        setSourceKind('wat')
        setStatus('error')
        setErrorMsg(`Reading ${file.name} was cancelled or timed out.`)
      }
      reader.onload = (e) => {
        settle()
        const buf = e.target?.result
        if (!(buf instanceof ArrayBuffer)) return
        void inspectWasm(buf, file.name, gen)
      }
      reader.readAsArrayBuffer(file)
    },
    [inspectWasm, resetSession],
  )

  const handleDrop = useCallback(
    (e: React.DragEvent<HTMLDivElement>) => {
      e.preventDefault()
      setDragging(false)
      if (status === 'running') return // serialize: one engine op at a time
      handleFiles(e.dataTransfer.files)
    },
    [handleFiles, status],
  )

  // ----- apply edits -----
  const runApplyEdits = useCallback(async () => {
    if (!form || !baseline) return
    const edits = diffEdits(baseline, form)
    if (edits.length === 0) return
    const gen = ++genRef.current
    setApplying(true)
    setStatus('running')
    setErrorMsg('')
    // Invalidate any prior applied result up front: only a SUCCESSFUL apply below
    // re-populates them, so a failed/superseded apply can never leave a stale
    // "After edits" graph or a downloadable binary that doesn't match the form.
    setAfterResult(null)
    setEmitted(null)
    try {
      const format = sourceFormatRef.current
      let bytes: ArrayBuffer
      if (format === 'wat') {
        bytes = new TextEncoder().encode(sourceWatRef.current).buffer as ArrayBuffer
      } else if (sourceWasmRef.current) {
        bytes = sourceWasmRef.current.slice(0)
      } else {
        throw new Error('No source retained — re-inspect the module before editing.')
      }
      const r = await ensureEngine().run({ kind: 'applyEdits', format, edits }, bytes)
      if (gen !== genRef.current) return // superseded by a newer request
      if (r.ok && r.kind === 'applyEdits') {
        setAfterResult(r.after)
        setEmitted(r.emitted)
        setSelectedId(r.after.nodes[0]?.id ?? null)
        setStatus('done')
      } else if (!r.ok) {
        setStatus('error')
        setErrorMsg(r.error)
      }
    } catch (err) {
      if (gen !== genRef.current) return
      setStatus('error')
      setErrorMsg(String(err))
    } finally {
      setApplying(false)
    }
  }, [form, baseline])

  const downloadEmitted = useCallback(() => {
    if (!emitted) return
    const blob = new Blob([emitted], { type: 'application/wasm' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'edited.wasm'
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
  }, [emitted])

  // ----- build & run -----
  const runBuild = useCallback(async () => {
    const preset = BUILD_PRESETS.find((p) => p.id === buildPreset) ?? BUILD_PRESETS[0]
    // Validate every arg up front; a bad field must surface an error, never run a
    // silently-coerced different call. (The Build button is also disabled while invalid.)
    const parsed = preset.argLabels.map((_, i) => parseI32Arg(buildArgs[i] ?? ''))
    const badIdx = parsed.findIndex((c) => 'error' in c)
    if (badIdx !== -1) {
      setStatus('error')
      setErrorMsg(
        `Argument "${preset.argLabels[badIdx]}" is invalid (${(parsed[badIdx] as { error: string }).error}). Enter a whole i32 number.`,
      )
      return
    }
    const args = parsed.map((c) => (c as { value: number }).value)
    const gen = ++genRef.current
    setBuilding(true)
    setStatus('running')
    setErrorMsg('')
    try {
      // Build ignores request bytes; send an empty (transferable) buffer.
      const empty = new ArrayBuffer(0)
      const r = await ensureEngine().run({ kind: 'buildFn', preset: buildPreset, args }, empty)
      if (gen !== genRef.current) return // superseded by a newer request
      if (r.ok && r.kind === 'buildFn') {
        setBuildResult({
          preset: preset.id,
          name: preset.name,
          args,
          result: r.result,
          instructions: r.instructions,
          emitted: r.emitted,
        })
        setStatus('done')
      } else if (!r.ok) {
        setStatus('error')
        setErrorMsg(r.error)
      }
    } catch (err) {
      if (gen !== genRef.current) return
      setStatus('error')
      setErrorMsg(String(err))
    } finally {
      setBuilding(false)
    }
  }, [buildPreset, buildArgs])

  const downloadBuilt = useCallback(() => {
    if (!buildResult) return
    const blob = new Blob([buildResult.emitted], { type: 'application/wasm' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    // Name the file after the artifact that was actually built, not the live
    // selection — so the download stays correct regardless of transient UI state.
    a.download = `${buildResult.name}.wasm`
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
  }, [buildResult])

  // ----- SSR shell (before mount) -----
  if (!mounted) {
    return (
      <div className="container-page py-16">
        <p className="eyebrow mb-3">Playground</p>
        <div className="flex min-h-52 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) text-sm text-(--color-muted)">
          Loading…
        </div>
      </div>
    )
  }

  // ----- cross-origin isolation GATE (only after mount) -----
  if (!self.crossOriginIsolated) {
    return <StaticFallback />
  }

  const isBuild = mode === 'build'
  // Any engine op in flight. Overlapping ops would share (and could crash) one
  // worker, and a stale op's timeout could tear down a newer one — so while busy we
  // lock every entry point (mode switch, inspect, build, file pick) to one op at a time.
  const busy = status === 'running'

  return (
    <div className="container-page py-12">
      <p className="eyebrow mb-3">Playground</p>
      <h1 className="text-display-lg mb-6 font-display text-(--color-fg)">See the shape of your wasm</h1>

      {/* stale-binary banner */}
      <div className="mb-6 flex items-start gap-3 rounded-xl border border-(--color-edit-muted) bg-(--color-edit-muted) px-4 py-3 text-sm text-(--color-edit-strong)">
        <span aria-hidden="true">⚠</span>
        <span>{STALE_WASM_NOTE}</span>
      </div>

      {/* mode tabs */}
      <div className="mb-6 inline-flex rounded-lg border border-(--color-border) bg-(--color-surface-1) p-1">
        {(['inspect', 'edit', 'build'] as Mode[]).map((m) => (
          <button
            key={m}
            type="button"
            onClick={() => setMode(m)}
            disabled={busy && mode !== m}
            className={[
              'rounded-md px-4 py-1.5 font-mono text-xs capitalize transition-colors',
              mode === m
                ? 'bg-(--color-accent) text-(--color-accent-fg)'
                : 'text-(--color-muted) hover:text-(--color-fg) disabled:cursor-not-allowed disabled:opacity-40',
            ].join(' ')}
          >
            {m}
          </button>
        ))}
      </div>

      <div className="flex flex-col gap-8 lg:flex-row">
        {/* ---- Left: input ---- */}
        <div className="flex flex-col gap-4 lg:w-96 lg:shrink-0">
          {!isBuild ? (
            <>
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="pg-example" className="font-mono text-xs text-(--color-muted)">
              Example
            </label>
            <select
              id="pg-example"
              className="min-w-0 flex-1 rounded-lg border border-(--color-border) bg-(--color-surface-1) px-3 py-1.5 font-mono text-xs text-(--color-fg) disabled:opacity-50"
              onChange={(e) => {
                const s = WAT_SAMPLES[Number(e.target.value)]
                if (s) changeSource(s.wat)
              }}
              disabled={busy || sourceKind === 'wasm'}
              defaultValue="0"
            >
              {WAT_SAMPLES.map((s, i) => (
                <option key={s.name} value={i}>
                  {s.name}
                </option>
              ))}
            </select>
          </div>

          <textarea
            spellCheck={false}
            value={wat}
            onChange={(e) => changeSource(e.target.value)}
            disabled={busy || sourceKind === 'wasm'}
            className="min-h-72 w-full resize-y rounded-xl border border-(--color-border) bg-(--color-surface-1) p-4 font-mono text-xs leading-relaxed text-(--color-fg) focus:border-(--color-accent) focus:outline-none disabled:opacity-60"
            aria-label="WAT source"
          />

          <div
            onDrop={handleDrop}
            onDragOver={(e) => {
              e.preventDefault()
              setDragging(true)
            }}
            onDragLeave={() => setDragging(false)}
            className={[
              'flex flex-col items-center gap-2 rounded-xl border-2 border-dashed p-4 text-center transition-colors',
              dragging
                ? 'border-(--color-accent) bg-(--color-accent-muted)'
                : 'border-(--color-border-strong) bg-(--color-surface-1)',
            ].join(' ')}
          >
            <p className="text-xs text-(--color-muted)">Drop a .wasm file, or</p>
            <button
              type="button"
              onClick={() => fileInputRef.current?.click()}
              disabled={busy}
              className="rounded-lg border border-(--color-border-strong) px-3 py-1.5 font-mono text-xs text-(--color-fg) hover:bg-(--color-surface-2) disabled:cursor-not-allowed disabled:opacity-40"
            >
              Choose .wasm
            </button>
          </div>
          <input
            ref={fileInputRef}
            type="file"
            accept=".wasm,application/wasm"
            className="hidden"
            onChange={(e) => {
              handleFiles(e.target.files)
              // Clear the selection so re-picking the SAME file (e.g. to retry a failed
              // read) still fires a change event. handleFiles has already captured the
              // File, so the in-flight read is unaffected.
              e.target.value = ''
            }}
          />

          {/* Binary source is active: the WAT editor above is inert (it can't represent
              this file). Surface which file drives the graph/edits, and offer a way back. */}
          {sourceKind === 'wasm' ? (
            <div className="flex items-center justify-between gap-3 rounded-xl border border-(--color-border-strong) bg-(--color-surface-1) px-4 py-3">
              <span className="min-w-0 truncate font-mono text-xs text-(--color-muted)">
                Binary loaded: <span className="text-(--color-fg)">{sourceLabel}</span> — the graph and
                edits act on this file.
              </span>
              <button
                type="button"
                onClick={() => changeSource(wat)}
                disabled={busy}
                className="shrink-0 rounded-lg border border-(--color-border-strong) px-3 py-1.5 font-mono text-xs text-(--color-fg) hover:bg-(--color-surface-2) disabled:cursor-not-allowed disabled:opacity-40"
              >
                Switch to WAT editor
              </button>
            </div>
          ) : null}

          <button
            type="button"
            onClick={inspectWat}
            disabled={status === 'running' || isBuild || sourceKind === 'wasm'}
            className="w-full rounded-lg bg-(--color-accent) px-4 py-2.5 text-sm font-semibold text-(--color-accent-fg) transition-opacity hover:bg-(--color-accent-strong) disabled:cursor-not-allowed disabled:opacity-40"
          >
            {status === 'running' && !applying ? 'Parsing…' : mode === 'edit' ? 'Inspect to edit' : 'Inspect module'}
          </button>

          {mode === 'edit' && !result ? (
            <p className="text-center font-mono text-xs text-(--color-faint)">
              Inspect a module first, then tweak its exports, globals, memory, and name below.
            </p>
          ) : null}

          {/* ---- Edit controls (amber accent = mutation) ---- */}
          {mode === 'edit' && result && form ? (
            <div className="flex flex-col gap-4 rounded-xl border border-(--color-edit-muted) bg-(--color-surface-1) p-4">
              <div className="flex items-center justify-between">
                <span className="font-mono text-xs tracking-wide text-(--color-edit-strong) uppercase">Edits</span>
                <span className="font-mono text-xs text-(--color-faint)">{pendingEdits.length} pending</span>
              </div>

              {/* module name */}
              <label className="flex flex-col gap-1">
                <span className="font-mono text-xs text-(--color-muted)">module name</span>
                <input
                  type="text"
                  value={form.moduleName}
                  placeholder="(unnamed)"
                  disabled={busy || clipInfo.moduleName}
                  title={clipInfo.moduleName ? 'Name is too long to edit here' : undefined}
                  onChange={(e) => setForm((f) => (f ? { ...f, moduleName: e.target.value } : f))}
                  className="w-full rounded-lg border border-(--color-border) bg-(--color-bg) px-3 py-1.5 font-mono text-xs text-(--color-fg) focus:border-(--color-edit) focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
                />
                {clipInfo.moduleName ? (
                  <span className="font-mono text-xs text-(--color-faint)">Name too long to edit here.</span>
                ) : null}
              </label>

              {/* exports → rename */}
              {result.nodes.some((n) => n.kind === 'export') ? (
                <div className="flex flex-col gap-2">
                  <span className="font-mono text-xs text-(--color-muted)">exports · rename</span>
                  {result.nodes
                    .filter((n) => n.kind === 'export')
                    .map((n) => (
                      <div key={n.id} className="flex items-center gap-2">
                        <span className="w-6 shrink-0 text-right font-mono text-xs text-(--color-faint)">#{n.index}</span>
                        <input
                          type="text"
                          value={form.exportNames[n.index] ?? ''}
                          disabled={busy || clipInfo.exports.has(n.index)}
                          title={clipInfo.exports.has(n.index) ? 'Name is too long to edit here' : undefined}
                          onChange={(e) =>
                            setForm((f) =>
                              f ? { ...f, exportNames: { ...f.exportNames, [n.index]: e.target.value } } : f,
                            )
                          }
                          className="min-w-0 flex-1 rounded-lg border border-(--color-border) bg-(--color-bg) px-3 py-1.5 font-mono text-xs text-(--color-fg) focus:border-(--color-edit) focus:outline-none disabled:cursor-not-allowed disabled:opacity-50"
                        />
                      </div>
                    ))}
                </div>
              ) : null}

              {/* globals → toggle mutable */}
              {result.nodes.some((n) => n.kind === 'global') ? (
                <div className="flex flex-col gap-2">
                  <span className="font-mono text-xs text-(--color-muted)">globals · mutable</span>
                  {result.nodes
                    .filter((n) => n.kind === 'global')
                    .map((n) => (
                      <label key={n.id} className="flex cursor-pointer items-center gap-2">
                        <input
                          type="checkbox"
                          checked={form.globalMutable[n.index] ?? false}
                          disabled={busy}
                          onChange={(e) =>
                            setForm((f) =>
                              f ? { ...f, globalMutable: { ...f.globalMutable, [n.index]: e.target.checked } } : f,
                            )
                          }
                          className="accent-(--color-edit) disabled:cursor-not-allowed disabled:opacity-50"
                        />
                        <span className="truncate font-mono text-xs text-(--color-fg)">{n.label}</span>
                      </label>
                    ))}
                </div>
              ) : null}

              {/* memories → initial pages */}
              {result.nodes.some((n) => n.kind === 'memory') ? (
                <div className="flex flex-col gap-2">
                  <span className="font-mono text-xs text-(--color-muted)">memories · initial pages</span>
                  {result.nodes
                    .filter((n) => n.kind === 'memory')
                    .map((n) => (
                      <div key={n.id} className="flex items-center gap-2">
                        <span className="truncate font-mono text-xs text-(--color-fg)">{n.label}</span>
                        <input
                          type="number"
                          min={0}
                          value={form.memoryInitial[n.index] ?? '0'}
                          disabled={busy}
                          aria-invalid={memoryErrors.has(n.index)}
                          title={memoryErrors.has(n.index) ? 'Enter a whole number of pages' : undefined}
                          onChange={(e) =>
                            setForm((f) =>
                              f ? { ...f, memoryInitial: { ...f.memoryInitial, [n.index]: e.target.value } } : f,
                            )
                          }
                          className={`ml-auto w-24 rounded-lg border ${memoryErrors.has(n.index) ? 'border-(--color-bad)' : 'border-(--color-border)'} bg-(--color-bg) px-3 py-1.5 font-mono text-xs text-(--color-fg) focus:border-(--color-edit) focus:outline-none disabled:cursor-not-allowed disabled:opacity-50`}
                        />
                      </div>
                    ))}
                </div>
              ) : null}

              {!editFormValid ? (
                <p className="font-mono text-xs text-(--color-bad)">
                  Memory pages must be a whole number — fix the highlighted field to apply.
                </p>
              ) : null}

              <button
                type="button"
                onClick={runApplyEdits}
                disabled={busy || pendingEdits.length === 0 || !editFormValid}
                className="w-full rounded-lg bg-(--color-edit) px-4 py-2.5 text-sm font-semibold text-(--color-accent-fg) transition-opacity hover:bg-(--color-edit-strong) disabled:cursor-not-allowed disabled:opacity-40"
              >
                {applying ? 'Applying…' : `Apply edits${pendingEdits.length ? ` (${pendingEdits.length})` : ''}`}
              </button>
            </div>
          ) : null}

            </>
          ) : (
            /* ---- Build controls (cyan accent = generative) ---- */
            <div className="flex flex-col gap-4 rounded-xl border border-(--color-border) bg-(--color-surface-1) p-4">
              <span className="font-mono text-xs tracking-wide text-(--color-accent) uppercase">
                Build a function
              </span>

              <label className="flex flex-col gap-1">
                <span className="font-mono text-xs text-(--color-muted)">preset</span>
                <select
                  value={buildPreset}
                  disabled={building}
                  onChange={(e) => selectPreset(e.target.value as BuildPresetId)}
                  className="w-full rounded-lg border border-(--color-border) bg-(--color-bg) px-3 py-1.5 font-mono text-xs text-(--color-fg) focus:border-(--color-accent) focus:outline-none disabled:cursor-not-allowed disabled:opacity-40"
                >
                  {BUILD_PRESETS.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.title}
                    </option>
                  ))}
                </select>
              </label>

              <p className="font-mono text-xs text-(--color-faint)">{activePreset.signature}</p>

              {activePreset.argLabels.length ? (
                <div className="flex flex-col gap-2">
                  <span className="font-mono text-xs text-(--color-muted)">args</span>
                  {activePreset.argLabels.map((lab, i) => (
                    <div key={lab} className="flex items-center gap-2">
                      <span className="w-6 shrink-0 text-right font-mono text-xs text-(--color-faint)">
                        {lab}
                      </span>
                      <input
                        type="number"
                        value={buildArgs[i] ?? ''}
                        disabled={building}
                        aria-invalid={'error' in argChecks[i]}
                        title={'error' in argChecks[i] ? `Invalid: ${argChecks[i].error}` : undefined}
                        onChange={(e) =>
                          setBuildArgs((a) => {
                            const next = [...a]
                            next[i] = e.target.value
                            return next
                          })
                        }
                        className={`min-w-0 flex-1 rounded-lg border ${'error' in argChecks[i] ? 'border-(--color-bad)' : 'border-(--color-border)'} bg-(--color-bg) px-3 py-1.5 font-mono text-xs text-(--color-fg) focus:border-(--color-accent) focus:outline-none disabled:cursor-not-allowed disabled:opacity-40`}
                      />
                    </div>
                  ))}
                </div>
              ) : (
                <p className="font-mono text-xs text-(--color-faint)">No arguments.</p>
              )}

              {activePreset.argLabels.length && !argsValid ? (
                <p className="font-mono text-xs text-(--color-bad)">Each argument must be a whole i32 number.</p>
              ) : null}

              <button
                type="button"
                onClick={runBuild}
                disabled={busy || !argsValid}
                className="w-full rounded-lg bg-(--color-accent) px-4 py-2.5 text-sm font-semibold text-(--color-accent-fg) transition-opacity hover:bg-(--color-accent-strong) disabled:cursor-not-allowed disabled:opacity-40"
              >
                {building ? 'Building…' : 'Build & run'}
              </button>
            </div>
          )}
          {status === 'error' ? (
            <p className="rounded-lg border border-(--color-border-strong) bg-(--color-surface-1) px-3 py-2 font-mono text-xs break-words text-(--color-bad)">
              {errorMsg}
            </p>
          ) : null}
        </div>

        {/* ---- Right: output ---- */}
        <div className="min-w-0 flex-1">
          {isBuild ? (
            <div className="flex flex-col gap-6">
              <div className="flex items-center justify-between gap-3">
                <span className="font-mono text-xs text-(--color-faint)">{activePreset.name}.wasm</span>
                {buildResult ? (
                  <button
                    type="button"
                    onClick={downloadBuilt}
                    className="rounded-lg border border-(--color-accent) bg-(--color-accent-muted) px-3 py-1.5 font-mono text-xs text-(--color-accent-strong) transition-colors hover:bg-(--color-accent-glow)"
                  >
                    Download .wasm
                  </button>
                ) : null}
              </div>

              {!buildResult ? (
                <div className="flex min-h-72 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) text-sm text-(--color-faint)">
                  {status === 'running' ? 'Building…' : 'Pick a preset and press Build & run.'}
                </div>
              ) : (
                <>
                  {/* the returned value, prominent */}
                  <div className="rounded-xl border border-(--color-accent-muted) bg-(--color-surface-1) p-6">
                    <p className="mb-3 font-mono text-xs tracking-wide text-(--color-accent) uppercase">
                      Result
                    </p>
                    <p className="font-mono text-xl break-words text-(--color-fg)">
                      {buildResult.name}({buildResult.args.join(', ')}) ={' '}
                      <span className="font-semibold text-(--color-accent)">
                        {String(buildResult.result)}
                      </span>
                    </p>
                  </div>

                  {/* round-tripped body — the descriptors fn.instructions() reads back */}
                  <div>
                    <p className="mb-2 font-mono text-xs text-(--color-muted)">
                      fn.instructions() — read back from the emitted bytes
                    </p>
                    <CodeBlock
                      html={instrsToHtml(buildResult.instructions)}
                      copyText={buildResult.instructions.map(fmtInstr).join('\n')}
                      filename="function body"
                    />
                  </div>

                  {/* the IR-builder snippet that produced it */}
                  <div>
                    <p className="mb-2 font-mono text-xs text-(--color-muted)">IR builder</p>
                    <pre className="overflow-x-auto rounded-xl border border-(--color-border) bg-(--color-surface-1) p-4 font-mono text-xs leading-relaxed text-(--color-fg)">
                      {activePreset.source}
                    </pre>
                  </div>
                </>
              )}
            </div>
          ) : (
            <>
          <div className="mb-4 flex items-center justify-between gap-3">
            <div className="inline-flex rounded-lg border border-(--color-border) bg-(--color-surface-1) p-1">
              {(['graph', 'tree'] as View[]).map((v) => (
                <button
                  key={v}
                  type="button"
                  onClick={() => setView(v)}
                  className={[
                    'rounded-md px-4 py-1.5 font-mono text-xs capitalize transition-colors',
                    view === v
                      ? 'bg-(--color-accent) text-(--color-accent-fg)'
                      : 'text-(--color-muted) hover:text-(--color-fg)',
                  ].join(' ')}
                >
                  {v}
                </button>
              ))}
            </div>
            <div className="flex items-center gap-3">
              {mode === 'edit' && afterResult ? (
                <>
                  <span className="inline-flex items-center rounded-full bg-(--color-edit-muted) px-2.5 py-0.5 font-mono text-xs text-(--color-edit-strong)">
                    After edits
                  </span>
                  <button
                    type="button"
                    onClick={downloadEmitted}
                    disabled={busy}
                    className="rounded-lg border border-(--color-edit) bg-(--color-edit-muted) px-3 py-1.5 font-mono text-xs text-(--color-edit-strong) transition-colors hover:bg-(--color-edit-glow) disabled:cursor-not-allowed disabled:opacity-40"
                  >
                    Download .wasm
                  </button>
                </>
              ) : null}
              {displayResult ? (
                <span className="font-mono text-xs text-(--color-faint)">
                  {sourceLabel}
                  {displayResult.moduleName ? ` · ${displayResult.moduleName}` : ''}
                </span>
              ) : null}
            </div>
          </div>

          {displayResult?.edgesTruncated ? (
            <p className="mb-3 rounded-lg border border-(--color-edit-muted) bg-(--color-edit-muted) px-3 py-2 font-mono text-xs text-(--color-edit-strong)">
              Large module — some edges are omitted, so the graph is a partial view. The tree tab still
              lists full section counts.
            </p>
          ) : null}

          {!displayResult ? (
            <div className="flex min-h-72 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) text-sm text-(--color-faint)">
              {status === 'running' ? 'Parsing module…' : 'Inspect a module to see its graph.'}
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-4 xl:grid-cols-[1fr_20rem]">
              <div className="min-w-0">
                {view === 'graph' ? (
                  <GraphView result={displayResult} selectedId={selectedId} onSelect={setSelectedId} />
                ) : (
                  <TreeView result={displayResult} selectedId={selectedId} onSelect={setSelectedId} />
                )}
              </div>
              <DetailPanel node={selectedNode} />
            </div>
          )}
            </>
          )}
        </div>
      </div>
    </div>
  )
}
