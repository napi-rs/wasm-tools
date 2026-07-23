// The signature visual: a node-and-edge module graph. Pure inline SVG driven by a
// small hardcoded sample module (no runtime wasm — the landing stays prerenderable).
// Cyan = nodes/edges; AMBER marks the one mutation (the export renamed run -> main).
// All motion lives in app.css and is gated behind prefers-reduced-motion.

type Kind = 'import' | 'function' | 'global' | 'memory' | 'type' | 'export'

type Node = {
  id: string
  x: number
  y: number
  label: string
  kind: Kind
  labelAnchor?: 'start' | 'end' | 'middle'
  edit?: boolean
}

type Edge = { from: string; to: string; edit?: boolean }

const NODES: Node[] = [
  { id: 'type', x: 70, y: 300, label: 'type (i32,i32)→i32', kind: 'type', labelAnchor: 'start' },
  { id: 'import', x: 70, y: 90, label: 'import env.log', kind: 'import', labelAnchor: 'start' },
  { id: 'run', x: 250, y: 90, label: 'fn run', kind: 'function', labelAnchor: 'middle' },
  { id: 'counter', x: 250, y: 195, label: 'global counter', kind: 'global', labelAnchor: 'middle' },
  { id: 'mem', x: 250, y: 300, label: 'memory', kind: 'memory', labelAnchor: 'middle' },
  { id: 'exMain', x: 450, y: 90, label: 'export "main"', kind: 'export', labelAnchor: 'end', edit: true },
  { id: 'exCounter', x: 450, y: 195, label: 'export "counter"', kind: 'export', labelAnchor: 'end' },
  { id: 'exMem', x: 450, y: 300, label: 'export "memory"', kind: 'export', labelAnchor: 'end' },
]

const EDGES: Edge[] = [
  { from: 'import', to: 'run' },
  { from: 'type', to: 'run' },
  { from: 'run', to: 'exMain', edit: true },
  { from: 'counter', to: 'exCounter' },
  { from: 'mem', to: 'exMem' },
]

const byId = (id: string) => NODES.find((n) => n.id === id)!

export default function ModuleGraph({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 520 380"
      fill="none"
      role="img"
      aria-label="A WebAssembly module graph: imports, functions, a global and memory wired to their exports, with one export renamed."
      style={{ width: '100%', height: 'auto', display: 'block' }}
    >
      <defs>
        <radialGradient id="mg-node" cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="#8bd8ff" />
          <stop offset="100%" stopColor="#5bc8ff" />
        </radialGradient>
        <radialGradient id="mg-edit" cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="#ffc879" />
          <stop offset="100%" stopColor="#ffb454" />
        </radialGradient>
      </defs>

      {/* edges */}
      <g strokeWidth="1.5" strokeLinecap="round">
        {EDGES.map((e) => {
          const a = byId(e.from)
          const b = byId(e.to)
          return (
            <line
              key={`${e.from}-${e.to}`}
              x1={a.x}
              y1={a.y}
              x2={b.x}
              y2={b.y}
              className={e.edit ? 'graph-edge graph-edit' : 'graph-edge'}
              stroke={e.edit ? '#ffb454' : '#5bc8ff'}
              strokeOpacity={e.edit ? 0.9 : 0.5}
            />
          )
        })}
      </g>

      {/* nodes */}
      <g>
        {NODES.map((n) => {
          const anchor = n.labelAnchor ?? 'middle'
          const dx = anchor === 'start' ? 14 : anchor === 'end' ? -14 : 0
          const dy = anchor === 'middle' ? -16 : 4
          return (
            <g key={n.id}>
              <circle
                cx={n.x}
                cy={n.y}
                r={n.edit ? 7.5 : 6}
                fill={n.edit ? 'url(#mg-edit)' : 'url(#mg-node)'}
                className={n.edit ? 'graph-edit' : 'graph-node-pulse'}
              />
              {n.edit ? (
                <circle
                  cx={n.x}
                  cy={n.y}
                  r={13}
                  fill="none"
                  stroke="#ffb454"
                  strokeOpacity="0.35"
                  strokeWidth="1"
                />
              ) : null}
              <text
                x={n.x + dx}
                y={n.y + dy}
                textAnchor={anchor}
                fontFamily="'JetBrains Mono', ui-monospace, monospace"
                fontSize="11"
                fill={n.edit ? '#ffc879' : '#8894b6'}
              >
                {n.label}
              </text>
            </g>
          )
        })}
      </g>
    </svg>
  )
}
