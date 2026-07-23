import CodeBlock from './code-block'
import SectionHeader from './section-header'
import Reveal from './_reveal'
import { builderSample } from '../_data/samples'

export default function BuilderHighlight({ html }: { html: string }) {
  return (
    <section className="border-t border-(--color-border)">
      <div className="container-page py-20 md:py-28">
        <div className="grid gap-12 lg:grid-cols-[1.1fr_0.9fr] lg:items-center">
          <Reveal className="order-2 lg:order-1">
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
              <div className="flex items-center justify-between font-mono text-sm">
                <span className="text-(--color-muted)">instance.exports.add(2, 3)</span>
                <span className="rounded-md bg-(--color-accent-muted) px-2.5 py-1 font-medium text-(--color-accent-strong)">
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
