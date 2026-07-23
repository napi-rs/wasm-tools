import CodeBlock from './code-block'
import SectionHeader from './section-header'
import Reveal from './_reveal'
import { builderSample } from '../_data/samples'

export default function BuilderHighlight({ html }: { html: string }) {
  return (
    <section className="border-t border-(--color-border)">
      <div className="container-page py-20 md:py-28">
        {/* minmax(0,…) + min-w-0 — see live-handle-story: without them the code
            sample's min-content width owns the track and squeezes this column. */}
        <div className="grid grid-cols-[minmax(0,1fr)] gap-12 lg:grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)] lg:items-center">
          <Reveal className="order-2 min-w-0 lg:order-1">
            <CodeBlock html={html} copyText={builderSample} filename="build-add.ts" />
          </Reveal>
          <div className="order-1 lg:order-2">
            <SectionHeader
              index="03"
              label="Instruction builder"
              title={
                <>
                  Describe <span className="font-mono text-(--color-accent)">add(a, b)</span>,
                  run it, get <span className="text-(--color-good)">5</span>
                </>
              }
              subhead="buildFunction takes params, results, the locals that are params, and an instruction-descriptor tree. Export it, emit, and WebAssembly.instantiate runs the real thing. The body round-trips back to the same descriptors."
            />
            <div className="mt-8 rounded-xl border border-(--color-hairline) bg-(--color-surface-1)/50 p-5">
              {/* The call and its result are both one short token — let them wrap to
                  two lines on a narrow card rather than squeezing the pill until
                  "= 5" breaks across lines inside it. */}
              <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 font-mono text-sm">
                <span className="whitespace-nowrap text-(--color-muted)">
                  instance.exports.add(2, 3)
                </span>
                <span className="shrink-0 whitespace-nowrap rounded-md bg-(--color-accent-muted) px-2.5 py-1 font-medium text-(--color-accent-strong)">
                  = 5
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
