import type { GraphNode, NodeKind } from './protocol'

const KIND_LABEL: Record<NodeKind, string> = {
  type: 'Type',
  import: 'Import',
  function: 'Function',
  global: 'Global',
  memory: 'Memory',
  table: 'Table',
  data: 'Data segment',
  element: 'Element segment',
  tag: 'Tag',
  export: 'Export',
}

export default function DetailPanel({ node }: { node: GraphNode | null }) {
  if (!node) {
    return (
      <div className="flex h-full min-h-40 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) p-6 text-sm text-(--color-faint)">
        Select a node to see its properties.
      </div>
    )
  }
  return (
    <div className="rounded-xl border border-(--color-border) bg-(--color-surface-1) p-5">
      <div className="mb-4 flex items-center gap-2">
        <span
          className={[
            'inline-flex items-center rounded-full px-2.5 py-0.5 font-mono text-xs',
            node.edited
              ? 'bg-(--color-edit-muted) text-(--color-edit-strong)'
              : 'bg-(--color-accent-muted) text-(--color-accent-strong)',
          ].join(' ')}
        >
          {KIND_LABEL[node.kind]}
        </span>
        <span className="font-mono text-xs text-(--color-faint)">#{node.index}</span>
      </div>
      <p className="mb-4 font-mono text-lg break-all text-(--color-fg)">{node.label}</p>
      <dl className="flex flex-col gap-0 text-sm">
        {node.props.map((p) => (
          <div
            key={p.key}
            className="flex items-start justify-between gap-4 border-t border-(--color-border) py-2 first:border-t-0"
          >
            <dt className="font-mono text-xs tracking-wide text-(--color-muted) uppercase">{p.key}</dt>
            <dd className="text-right font-mono text-xs break-all text-(--color-fg)">{p.value}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}
