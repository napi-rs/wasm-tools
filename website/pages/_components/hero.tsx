import Button from './button'
import Chip from './chip'
import ModuleGraph from './module-graph'
import InstallSwitcher from './_install-switcher'

export default function Hero() {
  return (
    <section className="relative overflow-hidden border-b border-(--color-border)">
      <div className="accent-glow" />
      <div className="container-page grid grid-cols-[minmax(0,1fr)] items-center gap-12 py-20 md:grid-cols-[minmax(0,1.05fr)_minmax(0,0.95fr)] md:py-28">
        <div className="min-w-0">
          <span className="eyebrow">walrus bindings for JavaScript</span>
          <h1 className="mt-5 font-display text-display-xl text-(--color-fg)">
            See the shape of your <span className="text-(--color-accent)">wasm</span>
          </h1>
          <p className="mt-6 max-w-xl text-lg text-(--color-muted)">
            Read, edit and build WebAssembly modules from JavaScript. Every function,
            global, memory, import and export is a{' '}
            <span className="text-(--color-fg)">live handle</span> — read through to the
            module, write straight back, then{' '}
            <span className="font-mono text-(--color-edit-strong)">emitWasm()</span>.
          </p>

          <div className="mt-8 flex flex-wrap items-center gap-4">
            <Button variant="primary" href="/playground">
              Open playground
            </Button>
            <Button variant="secondary" href="https://github.com/napi-rs/wasm-tools">
              GitHub
            </Button>
          </div>

          <div className="mt-10">
            <InstallSwitcher />
          </div>
        </div>

        <div className="min-w-0">
          <div className="rounded-2xl border border-(--color-hairline) bg-(--color-surface-1)/60 p-5 shadow-[0_0_60px_-20px_var(--color-accent-glow)] backdrop-blur-sm">
            <div className="mb-4 flex items-center justify-between">
              <span className="font-mono text-xs text-(--color-faint)">module.wasm</span>
              <div className="flex gap-2">
                <Chip tone="accent">8 nodes</Chip>
                <Chip tone="edit">1 edit</Chip>
              </div>
            </div>
            <ModuleGraph />
          </div>
        </div>
      </div>
    </section>
  )
}
