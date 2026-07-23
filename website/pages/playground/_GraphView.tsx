import { useMemo } from 'react'
import type { InspectResult, GraphNode, NodeKind } from './protocol'

// Left→right column order. Imports feed the module; types/defs sit in the
// middle; exports collect on the right. Only kinds with nodes take a column, so
// empty families never leave a gap.
const KIND_ORDER: NodeKind[] = [
  'import',
  'type',
  'function',
  'global',
  'memory',
  'table',
  'tag',
  'data',
  'element',
  'export',
]

const NODE_W = 168
const NODE_H = 48
const COL_GAP = 236 // x pitch between columns
const ROW_GAP = 68 // y pitch within a column
const PAD = 28

type Placed = { node: GraphNode; x: number; y: number }

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + '…' : s
}

export default function GraphView({
  result,
  selectedId,
  onSelect,
}: {
  result: InspectResult
  selectedId: string | null
  onSelect: (id: string) => void
}) {
  const { placed, pos, width, height } = useMemo(() => {
    const byKind = new Map<NodeKind, GraphNode[]>()
    for (const n of result.nodes) {
      const arr = byKind.get(n.kind) ?? []
      arr.push(n)
      byKind.set(n.kind, arr)
    }
    const columns = KIND_ORDER.filter((k) => (byKind.get(k)?.length ?? 0) > 0)
    const placed: Placed[] = []
    const pos = new Map<string, { x: number; y: number }>()
    let maxRows = 0
    columns.forEach((kind, col) => {
      const list = byKind.get(kind) ?? []
      maxRows = Math.max(maxRows, list.length)
      list.forEach((node, row) => {
        const x = PAD + col * COL_GAP
        const y = PAD + row * ROW_GAP
        placed.push({ node, x, y })
        pos.set(node.id, { x, y })
      })
    })
    const width = PAD * 2 + Math.max(0, columns.length - 1) * COL_GAP + NODE_W
    const height = PAD * 2 + Math.max(0, maxRows - 1) * ROW_GAP + NODE_H
    return { placed, pos, width, height }
  }, [result])

  if (result.nodes.length === 0) {
    return (
      <div className="flex min-h-64 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) text-sm text-(--color-faint)">
        This module is empty.
      </div>
    )
  }

  const edgePath = (from: { x: number; y: number }, to: { x: number; y: number }) => {
    const x1 = from.x + NODE_W
    const y1 = from.y + NODE_H / 2
    const x2 = to.x
    const y2 = to.y + NODE_H / 2
    const dx = Math.max(40, Math.abs(x2 - x1) * 0.5)
    return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`
  }

  return (
    <div className="overflow-auto rounded-xl border border-(--color-border) bg-(--color-surface-1) p-2">
      <svg
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        className="block"
        role="img"
        aria-label="Module graph"
      >
        <defs>
          <marker
            id="pg-arrow"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="6"
            markerHeight="6"
            orient="auto-start-reverse"
          >
            <path d="M0,0 L10,5 L0,10 z" fill="var(--color-accent)" opacity="0.55" />
          </marker>
          <marker
            id="pg-arrow-edit"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="6"
            markerHeight="6"
            orient="auto-start-reverse"
          >
            <path d="M0,0 L10,5 L0,10 z" fill="var(--color-edit)" />
          </marker>
        </defs>

        {/* edges */}
        <g fill="none">
          {result.edges.map((e) => {
            const from = pos.get(e.from)
            const to = pos.get(e.to)
            if (!from || !to) return null
            const active = e.from === selectedId || e.to === selectedId
            const stroke = active ? 'var(--color-accent-strong)' : 'var(--color-accent)'
            return (
              <path
                key={e.id}
                d={edgePath(from, to)}
                stroke={stroke}
                strokeWidth={active ? 1.8 : 1}
                opacity={active ? 0.9 : 0.32}
                markerEnd="url(#pg-arrow)"
              />
            )
          })}
        </g>

        {/* nodes */}
        <g>
          {placed.map(({ node, x, y }) => {
            const selected = node.id === selectedId
            const stroke = node.edited
              ? 'var(--color-edit)'
              : selected
                ? 'var(--color-accent-strong)'
                : 'var(--color-accent)'
            const fill = node.edited ? 'var(--color-edit-muted)' : 'var(--color-accent-muted)'
            return (
              <g
                key={node.id}
                transform={`translate(${x} ${y})`}
                onClick={() => onSelect(node.id)}
                style={{ cursor: 'pointer' }}
              >
                <rect
                  width={NODE_W}
                  height={NODE_H}
                  rx={9}
                  fill={fill}
                  stroke={stroke}
                  strokeWidth={selected ? 2 : 1.2}
                />
                <text
                  x={12}
                  y={19}
                  fontSize={12}
                  fontFamily="var(--font-mono)"
                  fill="var(--color-fg)"
                >
                  {truncate(node.label, 20)}
                </text>
                <text
                  x={12}
                  y={35}
                  fontSize={10}
                  fontFamily="var(--font-mono)"
                  fill="var(--color-faint)"
                >
                  {truncate(node.sub ?? node.kind, 22)}
                </text>
              </g>
            )
          })}
        </g>

        {/* edge labels (drawn last so they sit above nodes/edges) */}
        <g>
          {result.edges.map((e) => {
            const from = pos.get(e.from)
            const to = pos.get(e.to)
            if (!from || !to || !e.label) return null
            const x1 = from.x + NODE_W
            const y1 = from.y + NODE_H / 2
            const x2 = to.x
            const y2 = to.y + NODE_H / 2
            // The straight midpoint coincides with the cubic bezier's t=0.5 point
            // (the control points are horizontal offsets), so this sits on the curve.
            const mx = (x1 + x2) / 2
            const my = (y1 + y2) / 2
            const active = e.from === selectedId || e.to === selectedId
            return (
              <text
                key={`${e.id}-label`}
                x={mx}
                y={my - 3}
                textAnchor="middle"
                fontSize={9}
                fontFamily="var(--font-mono)"
                fill={active ? 'var(--color-accent-strong)' : 'var(--color-faint)'}
                opacity={active ? 1 : 0.7}
                style={{ pointerEvents: 'none' }}
              >
                {e.label}
              </text>
            )
          })}
        </g>
      </svg>
    </div>
  )
}
