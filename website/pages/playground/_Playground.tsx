// website/pages/playground/_Playground.tsx
// Interactive orchestrator island: WAT/​.wasm input → engine worker → module graph.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { PlaygroundEngine } from './_engine'
import type { RunResult } from './_engine'
import type { InspectResult, GraphNode } from './protocol'
import { WAT_SAMPLES, DEFAULT_WAT } from './_samples'
import GraphView from './_GraphView'
import TreeView from './_TreeView'
import DetailPanel from './_DetailPanel'

type Status = 'empty' | 'running' | 'done' | 'error'
type Mode = 'inspect' | 'edit' | 'build'
type View = 'graph' | 'tree'

// The PUBLISHED @napi-rs/wasm-tools-wasm32-wasi@1.0.1 binary predates the module-graph API
// (#158/#159). This site instead ships a vendored pre-release build (website/vendor/, aliased
// in vite.config.ts) that HAS the full API, so Inspect works today. Once a >=1.0.2 wasm is
// published to npm, delete the vendor dir and point the dep back at the registry version.
const STALE_WASM_NOTE =
  'Preview build — this playground runs a vendored pre-release of @napi-rs/wasm-tools with the full module-graph API. The public npm release (1.0.1) predates it; a ≥ 1.0.2 publish will replace the vendored copy.'

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
      <pre className="mb-10 overflow-x-auto rounded-xl border border-(--color-border) bg-(--color-surface-1) p-5 font-mono text-xs leading-relaxed text-(--color-fg)">
        {`import { WasmModule } from '@napi-rs/wasm-tools'

const mod = WasmModule.fromBuffer(bytes)
for (const fn of mod.functions.items()) {
  console.log(fn.index, fn.name, fn.kind)
}
for (const ex of mod.exports.items()) {
  console.log(ex.name, '→', ex.func()?.index)
}`}
      </pre>

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
    },
    [],
  )

  // ----- input / state -----
  const [mode, setMode] = useState<Mode>('inspect')
  const [wat, setWat] = useState(DEFAULT_WAT)
  const [sourceLabel, setSourceLabel] = useState<string>('module.wat')
  const [status, setStatus] = useState<Status>('empty')
  const [errorMsg, setErrorMsg] = useState('')
  const [result, setResult] = useState<InspectResult | null>(null)
  const [view, setView] = useState<View>('graph')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [dragging, setDragging] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const selectedNode: GraphNode | null = useMemo(
    () => result?.nodes.find((n) => n.id === selectedId) ?? null,
    [result, selectedId],
  )

  const ensureEngine = () => {
    if (!engineRef.current) engineRef.current = new PlaygroundEngine()
    return engineRef.current
  }

  const applyResult = useCallback((r: RunResult) => {
    if (r.ok && r.kind === 'inspect') {
      setResult(r.result)
      setSelectedId(r.result.nodes[0]?.id ?? null)
      setStatus('done')
    } else if (!r.ok) {
      setStatus('error')
      setErrorMsg(r.error)
    }
  }, [])

  const inspectWat = useCallback(async () => {
    setStatus('running')
    setErrorMsg('')
    try {
      const bytes = new TextEncoder().encode(wat)
      const r = await ensureEngine().run({ kind: 'inspect', format: 'wat' }, bytes.buffer as ArrayBuffer)
      setSourceLabel('module.wat')
      applyResult(r)
    } catch (err) {
      setStatus('error')
      setErrorMsg(String(err))
    }
  }, [wat, applyResult])

  const inspectWasm = useCallback(
    async (bytes: ArrayBuffer, name: string) => {
      setStatus('running')
      setErrorMsg('')
      try {
        const r = await ensureEngine().run({ kind: 'inspect', format: 'wasm' }, bytes.slice(0))
        setSourceLabel(name)
        applyResult(r)
      } catch (err) {
        setStatus('error')
        setErrorMsg(String(err))
      }
    },
    [applyResult],
  )

  const handleFiles = useCallback(
    (files: FileList | null) => {
      if (!files || files.length === 0) return
      const file = files[0]
      const reader = new FileReader()
      reader.onload = (e) => {
        const buf = e.target?.result
        if (!(buf instanceof ArrayBuffer)) return
        void inspectWasm(buf, file.name)
      }
      reader.readAsArrayBuffer(file)
    },
    [inspectWasm],
  )

  const handleDrop = useCallback(
    (e: React.DragEvent<HTMLDivElement>) => {
      e.preventDefault()
      setDragging(false)
      handleFiles(e.dataTransfer.files)
    },
    [handleFiles],
  )

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

  const editing = mode !== 'inspect'

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
            className={[
              'rounded-md px-4 py-1.5 font-mono text-xs capitalize transition-colors',
              mode === m
                ? 'bg-(--color-accent) text-(--color-accent-fg)'
                : 'text-(--color-muted) hover:text-(--color-fg)',
            ].join(' ')}
          >
            {m}
            {m !== 'inspect' ? <span className="ml-1 opacity-60">soon</span> : null}
          </button>
        ))}
      </div>

      <div className="flex flex-col gap-8 lg:flex-row">
        {/* ---- Left: input ---- */}
        <div className="flex flex-col gap-4 lg:w-96 lg:shrink-0">
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="pg-example" className="font-mono text-xs text-(--color-muted)">
              Example
            </label>
            <select
              id="pg-example"
              className="min-w-0 flex-1 rounded-lg border border-(--color-border) bg-(--color-surface-1) px-3 py-1.5 font-mono text-xs text-(--color-fg)"
              onChange={(e) => {
                const s = WAT_SAMPLES[Number(e.target.value)]
                if (s) setWat(s.wat)
              }}
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
            onChange={(e) => setWat(e.target.value)}
            className="min-h-72 w-full resize-y rounded-xl border border-(--color-border) bg-(--color-surface-1) p-4 font-mono text-xs leading-relaxed text-(--color-fg) focus:border-(--color-accent) focus:outline-none"
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
              className="rounded-lg border border-(--color-border-strong) px-3 py-1.5 font-mono text-xs text-(--color-fg) hover:bg-(--color-surface-2)"
            >
              Choose .wasm
            </button>
          </div>
          <input
            ref={fileInputRef}
            type="file"
            accept=".wasm,application/wasm"
            className="hidden"
            onChange={(e) => handleFiles(e.target.files)}
          />

          <button
            type="button"
            onClick={inspectWat}
            disabled={status === 'running' || editing}
            className="w-full rounded-lg bg-(--color-accent) px-4 py-2.5 text-sm font-semibold text-(--color-accent-fg) transition-opacity hover:bg-(--color-accent-strong) disabled:cursor-not-allowed disabled:opacity-40"
          >
            {status === 'running' ? 'Parsing…' : 'Inspect module'}
          </button>

          {editing ? (
            <p className="text-center font-mono text-xs text-(--color-faint)">
              {mode === 'edit' ? 'Edit' : 'Build'} mode is coming soon — Inspect is live.
            </p>
          ) : null}
          {status === 'error' ? (
            <p className="rounded-lg border border-(--color-border-strong) bg-(--color-surface-1) px-3 py-2 font-mono text-xs break-words text-(--color-bad)">
              {errorMsg}
            </p>
          ) : null}
        </div>

        {/* ---- Right: output ---- */}
        <div className="min-w-0 flex-1">
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
            {result ? (
              <span className="font-mono text-xs text-(--color-faint)">
                {sourceLabel}
                {result.moduleName ? ` · ${result.moduleName}` : ''}
              </span>
            ) : null}
          </div>

          {!result ? (
            <div className="flex min-h-72 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) text-sm text-(--color-faint)">
              {status === 'running' ? 'Parsing module…' : 'Inspect a module to see its graph.'}
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-4 xl:grid-cols-[1fr_20rem]">
              <div className="min-w-0">
                {view === 'graph' ? (
                  <GraphView result={result} selectedId={selectedId} onSelect={setSelectedId} />
                ) : (
                  <TreeView result={result} selectedId={selectedId} onSelect={setSelectedId} />
                )}
              </div>
              <DetailPanel node={selectedNode} />
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
