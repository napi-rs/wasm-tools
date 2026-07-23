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
const GUTTER = COL_GAP - NODE_W // empty space between two columns (68)
const SAME_COL_BOW = 24 // base bow of a same-column edge (kept < GUTTER so its curve fits)
const LANE_STEP = 10 // extra bow per parallel edge between the same pair
const LANE_STAGGER = 12 // vertical offset per parallel label so they don't stack
const LABEL_PAD = 40 // right-side room a same-column edge label needs
// Smallest downscale the canvas may take to fit its pane. 0.85 keeps the 12px node
// label at ~10px and the 9px edge label at ~7.6px; past that the graph scrolls.
const MIN_SCALE = 0.85

type Placed = { node: GraphNode; x: number; y: number }
type XY = { x: number; y: number }
// A pre-routed edge: its SVG path, the point to anchor its label on, and the
// max x/y its geometry reaches (so the canvas can size to fit and never clip).
type PlacedEdge = {
  id: string
  from: string
  to: string
  label?: string
  sameColumn: boolean
  path: string
  lx: number
  ly: number
  maxX: number
  maxY: number
}

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + '…' : s
}

// Route one edge with DIRECTION-FACING ports: it always leaves the source on the
// side facing the target and enters the target on the side facing the source, so a
// right→left edge (e.g. export→function) curves left instead of overshooting the
// canvas. Same-column edges (fn→fn calls, self-recursion) bow to the right of the
// column into a per-parallel lane. Labels are anchored in guaranteed-empty space —
// the gap immediately beside the source for cross-column edges, out on the bow for
// same-column — never at a global midpoint that could land on an intervening node.
function routeEdge(from: XY, to: XY, lane: number): Omit<PlacedEdge, 'id' | 'from' | 'to' | 'label' | 'sameColumn'> {
  if (from.x !== to.x) {
    const leftToRight = to.x > from.x
    const x1 = leftToRight ? from.x + NODE_W : from.x
    const y1 = from.y + NODE_H / 2
    const x2 = leftToRight ? to.x : to.x + NODE_W
    const y2 = to.y + NODE_H / 2
    const dx = Math.max(40, Math.abs(x2 - x1) * 0.5)
    const c1x = leftToRight ? x1 + dx : x1 - dx
    const c2x = leftToRight ? x2 - dx : x2 + dx
    // Anchor the label in the MIDDLE of the first inter-column gutter beside the
    // source — always empty space, whatever column the edge ends in — so a column-
    // skipping edge's label can never land on an intervening node.
    const gapOff = GUTTER / 2
    return {
      path: `M ${x1} ${y1} C ${c1x} ${y1}, ${c2x} ${y2}, ${x2} ${y2}`,
      lx: x1 + (leftToRight ? gapOff : -gapOff),
      ly: y1 - 6,
      maxX: Math.max(x1, x2, c1x, c2x),
      maxY: Math.max(y1, y2),
    }
  }
  // same column: the curve bows to the right, one lane per parallel edge so
  // reciprocal and Call/RefFunc pairs never trace the identical curve. The LABEL is
  // start-anchored right at the node edge (so it stays inside the gutter regardless
  // of lane) and staggered vertically per lane so parallel labels don't stack.
  const bow = SAME_COL_BOW + lane * LANE_STEP
  const x1 = from.x + NODE_W
  const cx = x1 + bow
  const labelX = x1 + 6 // start-anchored, inside the gutter beside the node
  const stagger = (lane % 2 === 0 ? 1 : -1) * Math.ceil(lane / 2) * LANE_STAGGER
  if (from.y === to.y) {
    const y = from.y + NODE_H / 2 // self-loop
    return {
      path: `M ${x1} ${y - 9} C ${cx} ${y - 9}, ${cx} ${y + 9}, ${x1} ${y + 9}`,
      lx: labelX,
      ly: y + stagger,
      maxX: cx + LABEL_PAD,
      maxY: y + 9 + Math.abs(stagger),
    }
  }
  const y1 = from.y + NODE_H / 2
  const y2 = to.y + NODE_H / 2
  return {
    path: `M ${x1} ${y1} C ${cx} ${y1}, ${cx} ${y2}, ${x1} ${y2}`,
    lx: labelX,
    ly: (y1 + y2) / 2 + stagger,
    maxX: cx + LABEL_PAD,
    maxY: Math.max(y1, y2) + Math.abs(stagger),
  }
}

// Group parallel edges by UNORDERED endpoint pair, so A→B and B→A share a lane
// counter and get distinct lanes instead of overlapping on the identical curve.
const laneKey = (a: string, b: string) => (a < b ? `${a}|${b}` : `${b}|${a}`)

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

    const laneSeen = new Map<string, number>()
    const edges: PlacedEdge[] = []
    // Start bounds at the node extent; each routed edge can only grow them.
    let maxX = PAD + Math.max(0, columns.length - 1) * COL_GAP + NODE_W
    let maxY = PAD + Math.max(0, maxRows - 1) * ROW_GAP + NODE_H
    for (const e of result.edges) {
      const from = pos.get(e.from)
      const to = pos.get(e.to)
      if (!from || !to) continue // endpoint capped out of its section — skip
      const key = laneKey(e.from, e.to)
      const lane = laneSeen.get(key) ?? 0
      laneSeen.set(key, lane + 1)
      const g = routeEdge(from, to, lane)
      maxX = Math.max(maxX, g.maxX)
      maxY = Math.max(maxY, g.maxY)
      edges.push({ id: e.id, from: e.from, to: e.to, label: e.label, sameColumn: from.x === to.x, ...g })
    }
    return { placed, edges, width: maxX + PAD, height: maxY + PAD }
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
        // Draw at natural size when it fits, scale DOWN to the pane when it doesn't
        // (h-auto keeps the viewBox aspect), never up. The floor is RELATIVE, not a
        // fixed 520px: a 536-wide graph in a 342px phone pane hit 0.445x, rendering
        // 12px labels at 5px. Below MIN_SCALE the svg stops shrinking and the wrapper
        // scrolls instead — a scrollbar beats an illegible one.
        className="block h-auto max-w-full"
        style={{ minWidth: Math.round(width * MIN_SCALE) }}
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

        {/* edge labels. Same-column labels (fn→fn calls / ref.func) are routed out to
            the right into their own lanes, so they never touch a node — show them
            always. Cross-column labels can only be placed in a column gap and a
            column-skipping edge's label could still fall near an intervening node, so
            show those ONLY for the selected node's edges. This keeps the graph readable
            (a dense module would otherwise carpet the canvas) while always surfacing the
            call graph and revealing every relationship on focus. */}
        <g>
          {edges.map((e) => {
            if (!e.label) return null
            const active = e.from === selectedId || e.to === selectedId
            if (!e.sameColumn && !active) return null
            return (
              <text
                key={`${e.id}-label`}
                x={e.lx}
                y={e.ly}
                textAnchor={e.sameColumn ? 'start' : 'middle'}
                fontSize={9}
                fontFamily="var(--font-mono)"
                fill={active ? 'var(--color-accent-strong)' : 'var(--color-faint)'}
                opacity={active ? 1 : 0.75}
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
