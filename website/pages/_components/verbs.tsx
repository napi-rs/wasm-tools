import CodeBlock from './code-block'
import SectionHeader from './section-header'
import Reveal from './_reveal'
import { inspectSample, editSample, buildSample } from '../_data/samples'

const VERBS = [
  {
    key: 'inspect',
    tag: 'Inspect',
    color: 'var(--color-accent)',
    desc: 'Walk every collection through live handles — exports, imports, globals, memory, types. Iterate, or look items up by name.',
    filename: 'inspect.ts',
  },
  {
    key: 'edit',
    tag: 'Edit',
    color: 'var(--color-edit)',
    desc: 'Assign to a handle and the write lands in the owning module. Rename an export, flip a global, grow memory — then emit.',
    filename: 'edit.ts',
  },
  {
    key: 'build',
    tag: 'Build',
    color: 'var(--color-accent)',
    desc: 'Synthesize functions from an instruction-descriptor tree, add locals, wire exports, and emit a module that really runs.',
    filename: 'build.ts',
  },
] as const

export default function Verbs({
  inspectHtml,
  editHtml,
  buildHtml,
}: {
  inspectHtml: string
  editHtml: string
  buildHtml: string
}) {
  const htmlByKey: Record<string, string> = {
    inspect: inspectHtml,
    edit: editHtml,
    build: buildHtml,
  }
  const rawByKey: Record<string, string> = {
    inspect: inspectSample,
    edit: editSample,
    build: buildSample,
  }

  return (
    <section className="border-t border-(--color-border)">
      <div className="container-page py-20 md:py-28">
        <SectionHeader
          index="01"
          label="Three verbs"
          title={
            <>
              Inspect. Edit. Build.
            </>
          }
          subhead="One module handle, three things to do with it. The same graph you read is the graph you mutate and the graph you emit."
        />

        {/* minmax(0,…) + min-w-0 — a bare fr track sizes to the code sample's
            min-content width, which is wider than a phone. */}
        <div className="mt-12 grid grid-cols-[minmax(0,1fr)] gap-6 lg:grid-cols-[repeat(3,minmax(0,1fr))]">
          {VERBS.map((v, i) => (
            <Reveal key={v.key} className="min-w-0" delay={i * 80}>
              <div className="flex h-full flex-col overflow-hidden rounded-2xl border border-(--color-hairline) bg-(--color-surface-1)/50">
                <div className="flex items-center gap-3 border-b border-(--color-border) px-5 py-4">
                  <span
                    className="h-2.5 w-2.5 rounded-full"
                    style={{ background: v.color }}
                    aria-hidden="true"
                  />
                  <span className="font-mono text-sm font-medium text-(--color-fg)">{v.tag}</span>
                </div>
                <div className="px-5 py-5">
                  <p className="text-sm text-(--color-muted)">{v.desc}</p>
                </div>
                <div className="mt-auto px-5 pb-5">
                  <CodeBlock
                    html={htmlByKey[v.key]}
                    copyText={rawByKey[v.key]}
                    filename={v.filename}
                  />
                </div>
              </div>
            </Reveal>
          ))}
        </div>
      </div>
    </section>
  )
}
