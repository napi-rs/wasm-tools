import CodeBlock from './CodeBlock'
import SectionHeader from './SectionHeader'
import Chip from './Chip'
import Reveal from './_Reveal'
import { storySample } from '../_data/samples'

const STEPS = [
  { n: '1', tone: 'accent' as const, label: 'parse', text: 'fromBuffer reads the module into a live graph.' },
  { n: '2', tone: 'edit' as const, label: 'edit', text: 'Assign to a handle — the write lands in the module.' },
  { n: '3', tone: 'accent' as const, label: 're-emit', text: 'emitWasm serializes the mutated graph to bytes.' },
  { n: '4', tone: 'accent' as const, label: 'prove', text: 'Re-parse the bytes: every edit is baked in.' },
]

export default function LiveHandleStory({ html }: { html: string }) {
  return (
    <section className="border-t border-(--color-border)">
      <div className="container-page py-20 md:py-28">
        <div className="grid gap-12 lg:grid-cols-[0.9fr_1.1fr] lg:items-center">
          <div>
            <SectionHeader
              index="02"
              label="Live handles"
              title={
                <>
                  Edit an export, re-emit,{' '}
                  <span className="text-(--color-edit)">it persists</span>
                </>
              }
              subhead="There is no separate mutation API and no rebuild step. A handle is a window onto the module — reads read through, writes write back. Round-trip through the bytes to prove it."
            />
            <div className="mt-8 flex flex-col gap-4">
              {STEPS.map((s) => (
                <Reveal key={s.n} className="flex items-start gap-4">
                  <Chip tone={s.tone}>{s.n}</Chip>
                  <p className="text-sm text-(--color-muted)">
                    <span className="font-mono text-(--color-fg)">{s.label}</span> — {s.text}
                  </p>
                </Reveal>
              ))}
            </div>
          </div>
          <Reveal>
            <CodeBlock html={html} copyText={storySample} filename="round-trip.ts" />
          </Reveal>
        </div>
      </div>
    </section>
  )
}
