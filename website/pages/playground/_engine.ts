// website/pages/playground/_engine.ts
import type { Op, WorkerResponse, InspectResult, BuildInstrDesc } from './protocol'

export type RunResult =
  | { ok: true; kind: 'inspect'; result: InspectResult }
  | { ok: true; kind: 'applyEdits'; before: InspectResult; after: InspectResult; emitted: ArrayBuffer }
  | {
      ok: true
      kind: 'buildFn'
      result: number | string
      instructions: BuildInstrDesc[]
    }
  | { ok: false; error: string }

// A single op should never legitimately run this long. A worker still busy past
// it is presumed wedged (non-terminating parse, runaway allocation) and is torn
// down so the next run() gets a fresh one instead of hanging forever.
const REQUEST_TIMEOUT_MS = 60_000

type Pending = {
  resolve: (r: RunResult) => void
  reject: (e: Error) => void
  timer: ReturnType<typeof setTimeout>
}

export class PlaygroundEngine {
  private worker: Worker | null = null
  private seq = 0
  private pending = new Map<number, Pending>()

  // Lazily (re)create the worker. A crash nulls `this.worker`, so the next run()
  // transparently spins up a replacement.
  private ensureWorker(): Worker {
    if (this.worker) return this.worker
    const w = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' })
    w.onmessage = (e: MessageEvent<WorkerResponse>) => {
      const p = this.pending.get(e.data.id)
      if (!p) return
      clearTimeout(p.timer)
      this.pending.delete(e.data.id)
      p.resolve(e.data as RunResult)
      // A fatal load failure poisoned this worker's ESM module registry — no later
      // request can recover in it. THIS request is already resolved (its error text
      // reaches the user above); now discard the worker so the next run() gets a fresh
      // one, and fail any other in-flight requests that would re-hit the same rejection.
      if (e.data.ok === false && e.data.fatal) {
        this.failAll(new Error('The wasm worker was reset after a load failure — please retry.'))
      }
    }
    // A worker-level failure (module bootstrap error, uncaught throw, OOM abort)
    // never arrives as an onmessage — without this, every in-flight request would
    // hang forever. Reject them all and discard the dead worker.
    w.onerror = (ev) => this.failAll(new Error(ev.message || 'The wasm worker crashed.'))
    w.onmessageerror = () => this.failAll(new Error('The wasm worker sent an unreadable message.'))
    this.worker = w
    return w
  }

  // Settle every pending request as failed and drop the worker so the next run()
  // starts a fresh one. Shared by onerror/onmessageerror, the per-request timeout,
  // and dispose().
  private failAll(err: Error) {
    for (const [, p] of this.pending) {
      clearTimeout(p.timer)
      p.reject(err)
    }
    this.pending.clear()
    this.worker?.terminate()
    this.worker = null
  }

  run(op: Op, bytes: ArrayBuffer): Promise<RunResult> {
    const worker = this.ensureWorker()
    const id = ++this.seq
    // The worker takes ownership of `bytes` (transferred); callers must pass a copy
    // if they still need the original (the UI keeps the original source separately).
    return new Promise<RunResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        // Presume the worker is wedged: fail everything and rebuild on next run().
        this.failAll(
          new Error(`Timed out after ${REQUEST_TIMEOUT_MS / 1000}s — the module may be too large to process in-browser.`),
        )
      }, REQUEST_TIMEOUT_MS)
      this.pending.set(id, { resolve, reject, timer })
      worker.postMessage({ id, op, bytes }, [bytes])
    })
  }

  dispose() {
    this.failAll(new Error('Playground engine disposed.'))
  }
}
