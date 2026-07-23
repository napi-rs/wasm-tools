import SectionHeader from './SectionHeader'
import Chip from './Chip'
import Reveal from './_Reveal'

const GROUPS: { label: string; blurb: string; items: string[] }[] = [
  {
    label: 'Core',
    blurb: 'Entry points — configure a parse, hold a module, build const exprs.',
    items: ['ModuleConfig', 'WasmModule', 'ConstExpr'],
  },
  {
    label: 'Collections',
    blurb: 'Live sets on a module: .length, .items(), lookups and add* factories.',
    items: [
      'WasmFunctions',
      'WasmGlobals',
      'WasmMemories',
      'WasmTables',
      'WasmTypes',
      'WasmImports',
      'WasmExports',
      'WasmDataSegments',
      'WasmElements',
      'WasmTags',
      'WasmLocals',
      'WasmCustomSections',
      'WasmProducers',
    ],
  },
  {
    label: 'Handles',
    blurb: 'One item each — read a property through to the module, write it back.',
    items: [
      'WasmFunction',
      'WasmGlobal',
      'WasmMemory',
      'WasmTable',
      'WasmType',
      'WasmImport',
      'WasmExport',
      'WasmData',
      'WasmElement',
      'WasmTag',
      'WasmLocal',
    ],
  },
]

const VALTYPES = [
  'I32',
  'I64',
  'F32',
  'F64',
  'V128',
  'FUNCREF',
  'EXTERNREF',
  'ANYREF',
  'EQREF',
  'I31REF',
  'STRUCTREF',
  'ARRAYREF',
  'NULLREF',
  'NULLFUNCREF',
  'NULLEXTERNREF',
  'EXNREF',
  'NULLEXNREF',
]

const classCount = GROUPS.reduce((n, g) => n + g.items.length, 0)

export default function ApiMap() {
  return (
    <section className="border-t border-(--color-border)">
      <div className="container-page py-20 md:py-28">
        <SectionHeader
          index="04"
          label="API map"
          title={
            <>
              {classCount} classes, {VALTYPES.length} value types
            </>
          }
          subhead="The full walrus module graph, exposed. Collections hand out handles; handles read and write through to the module; value types come ready-made so you import I32 instead of spelling it out."
        />

        <div className="mt-12 grid gap-6 lg:grid-cols-3">
          {GROUPS.map((g, i) => (
            <Reveal key={g.label} delay={i * 70}>
              <div className="flex h-full flex-col rounded-2xl border border-(--color-hairline) bg-(--color-surface-1)/50 p-6">
                <div className="flex items-baseline justify-between">
                  <h3 className="font-display text-lg text-(--color-fg)">{g.label}</h3>
                  <span className="font-mono text-xs text-(--color-faint) tabular-nums">
                    {g.items.length}
                  </span>
                </div>
                <p className="mt-2 text-sm text-(--color-muted)">{g.blurb}</p>
                <div className="mt-5 flex flex-wrap gap-2">
                  {g.items.map((it) => (
                    <span
                      key={it}
                      className="rounded-md border border-(--color-border) bg-(--color-surface-2)/40 px-2 py-1 font-mono text-xs text-(--color-fg)"
                    >
                      {it}
                    </span>
                  ))}
                </div>
              </div>
            </Reveal>
          ))}
        </div>

        <Reveal className="mt-6">
          <div className="rounded-2xl border border-(--color-hairline) bg-(--color-surface-1)/50 p-6">
            <div className="flex items-baseline justify-between">
              <h3 className="font-display text-lg text-(--color-fg)">Value types</h3>
              <span className="font-mono text-xs text-(--color-faint) tabular-nums">
                {VALTYPES.length}
              </span>
            </div>
            <p className="mt-2 text-sm text-(--color-muted)">
              Numeric types and the nullable reference types, exported as constants —{' '}
              <span className="font-mono text-(--color-accent)">import {'{'} I32 {'}'}</span>{' '}
              instead of <span className="font-mono text-(--color-faint)">{"{ type: 'I32' }"}</span>.
            </p>
            <div className="mt-5 flex flex-wrap gap-2">
              {VALTYPES.map((v) => (
                <Chip key={v} tone="accent">
                  {v}
                </Chip>
              ))}
            </div>
          </div>
        </Reveal>
      </div>
    </section>
  )
}
