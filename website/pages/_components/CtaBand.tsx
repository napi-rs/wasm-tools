import Button from './Button'
import Reveal from './_Reveal'

export default function CtaBand() {
  return (
    <section className="relative overflow-hidden border-t border-(--color-border)">
      <div className="accent-glow" />
      <div className="container-page py-24 md:py-32 text-center">
        <Reveal>
          <p className="eyebrow">PLAYGROUND</p>
          <h2 className="font-display text-display-lg mt-4 text-(--color-fg)">
            Walk your module in the <span className="text-(--color-accent)">browser</span>
          </h2>
          <p className="mt-5 text-base text-(--color-muted) max-w-md mx-auto">
            Paste WAT or drop a .wasm and watch the graph build itself — parse, inspect,
            edit and re-emit, all client-side.
          </p>
          <div className="mt-10 flex flex-wrap items-center justify-center gap-4">
            <Button variant="primary" href="/playground">
              Open the playground
            </Button>
            <Button variant="secondary" href="https://github.com/napi-rs/wasm-tools">
              Star on GitHub
            </Button>
          </div>
        </Reveal>
      </div>
    </section>
  )
}
