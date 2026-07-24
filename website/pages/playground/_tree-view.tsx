import type { InspectResult, GraphNode } from './protocol'

export default function TreeView({
  result,
  selectedId,
  onSelect,
}: {
  result: InspectResult
  selectedId: string | null
  onSelect: (id: string) => void
}) {
  const byId = new Map<string, GraphNode>(result.nodes.map((n) => [n.id, n]))
  const populated = result.sections.filter((s) => s.count > 0)

  if (populated.length === 0) {
    return <p className="p-6 text-sm text-(--color-faint)">This module is empty.</p>
  }

  return (
    <div className="flex flex-col gap-2">
      {populated.map((section) => (
        <details
          key={section.kind}
          open
          className="rounded-xl border border-(--color-border) bg-(--color-surface-1)"
        >
          <summary className="flex cursor-pointer list-none items-center justify-between px-4 py-3 select-none">
            <span className="flex items-center gap-2">
              <svg
                className="nav-caret h-3.5 w-3.5 text-(--color-faint)"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                aria-hidden="true"
              >
                <polyline points="6 9 12 15 18 9" />
              </svg>
              <span className="font-mono text-sm text-(--color-fg)">{section.label}</span>
            </span>
            <span className="inline-flex items-center rounded-full bg-(--color-accent-muted) px-2 py-0.5 font-mono text-xs tabular-nums text-(--color-accent-strong)">
              {section.count}
            </span>
          </summary>
          <ul className="border-t border-(--color-border) px-2 py-2">
            {section.nodeIds.map((id) => {
              const node = byId.get(id)
              if (!node) return null
              const active = id === selectedId
              return (
                <li key={id}>
                  <button
                    type="button"
                    onClick={() => onSelect(id)}
                    className={[
                      'flex w-full items-baseline justify-between gap-3 rounded-lg px-3 py-1.5 text-left transition-colors',
                      active ? 'bg-(--color-accent-muted)' : 'hover:bg-(--color-surface-2)',
                    ].join(' ')}
                  >
                    <span className="truncate font-mono text-sm text-(--color-fg)">{node.label}</span>
                    {node.sub ? (
                      <span className="shrink-0 truncate font-mono text-xs text-(--color-faint)">
                        {node.sub}
                      </span>
                    ) : null}
                  </button>
                </li>
              )
            })}
          </ul>
          {section.truncated ? (
            <p className="border-t border-(--color-border) px-4 py-2 font-mono text-xs text-(--color-faint)">
              showing first {section.nodeIds.length} of {section.count}
            </p>
          ) : null}
        </details>
      ))}
    </div>
  )
}
