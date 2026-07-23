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
const SAME_COL_BOW = 46 // how far a same-column edge bows to the right of the column
const LANE_STEP = 20 // extra bow per parallel edge between the same pair

type Placed = { node: GraphNode; x: number; y: number }
type XY = { x: number; y: number }
// A pre-routed edge: its SVG path plus the point to anchor its label on.
type PlacedEdge = { id: string; from: string; to: string; label?: string; path: string; lx: number; ly: number }

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + '…' : s
}

// Route one edge. Cross-column edges keep the horizontal S-curve with the label
// in the empty inter-column gap. Same-column edges (fn→fn calls, self-recursion)
// bow out to the RIGHT of the column into a per-parallel lane, so neither the
// curve nor its label ever crosses the node boxes or a node between the rows.
function routeEdge(from: XY, to: XY, lane: number): { path: string; lx: number; ly: number } {
  if (from.x !== to.x) {
    const x1 = from.x + NODE_W
    const y1 = from.y + NODE_H / 2
    const x2 = to.x
    const y2 = to.y + NODE_H / 2
    const dx = Math.max(40, Math.abs(x2 - x1) * 0.5)
    return {
      path: `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`,
      lx: (x1 + x2) / 2,
      ly: (y1 + y2) / 2 - 3,
    }
  }
  const bow = SAME_COL_BOW + lane * LANE_STEP
  const x1 = from.x + NODE_W // both endpoints on the right side of their boxes
  if (from.y === to.y) {
    // self-loop: a small right-side loop next to the node
    const y = from.y + NODE_H / 2
    return {
      path: `M ${x1} ${y - 9} C ${x1 + bow} ${y - 9}, ${x1 + bow} ${y + 9}, ${x1} ${y + 9}`,
      lx: x1 + bow + 5,
      ly: y,
    }
  }
  const y1 = from.y + NODE_H / 2
  const y2 = to.y + NODE_H / 2
  const cx = x1 + bow
  return {
    path: `M ${x1} ${y1} C ${cx} ${y1}, ${cx} ${y2}, ${x1} ${y2}`,
    lx: cx + 5,
    ly: (y1 + y2) / 2,
  }
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
  const { placed, edges, width, height } = useMemo(() => {
    const byKind = new Map<NodeKind, GraphNode[]>()
    for (const n of result.nodes) {
      const arr = byKind.get(n.kind) ?? []
      arr.push(n)
      byKind.set(n.kind, arr)
    }
    const columns = KIND_ORDER.filter((k) => (byKind.get(k)?.length ?? 0) > 0)
    const placed: Placed[] = []
    const pos = new Map<string, XY>()
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

    // Pre-route edges, assigning each parallel edge between the same pair its own
    // lane so overlapping Call/RefFunc curves and labels fan apart.
    const laneSeen = new Map<string, number>()
    const edges: PlacedEdge[] = []
    for (const e of result.edges) {
      const from = pos.get(e.from)
      const to = pos.get(e.to)
      if (!from || !to) continue // endpoint capped out of its section — skip
      const key = `${e.from}->${e.to}`
      const lane = laneSeen.get(key) ?? 0
      laneSeen.set(key, lane + 1)
      edges.push({ id: e.id, from: e.from, to: e.to, label: e.label, ...routeEdge(from, to, lane) })
    }
    return { placed, edges, width, height }
  }, [result])

  if (result.nodes.length === 0) {
    return (
      <div className="flex min-h-64 items-center justify-center rounded-xl border border-(--color-border) bg-(--color-surface-1) text-sm text-(--color-faint)">
        This module is empty.
      </div>
    )
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
          {edges.map((e) => {
            const active = e.from === selectedId || e.to === selectedId
            const stroke = active ? 'var(--color-accent-strong)' : 'var(--color-accent)'
            return (
              <path
                key={e.id}
                d={e.path}
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
          {edges.map((e) => {
            if (!e.label) return null
            const active = e.from === selectedId || e.to === selectedId
            return (
              <text
                key={`${e.id}-label`}
                x={e.lx}
                y={e.ly}
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
