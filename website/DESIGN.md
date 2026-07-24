# @napi-rs/wasm-tools — website design + architecture

## Direction C — "Graph" (dark-only)

The module graph **is** the brand. A node-and-edge module graph is the signature
visual; a fine engineering grid sits behind everything; cyan is nodes/edges; **amber
marks every mutation/edit**.

### Palette (tokens live in `app.css` `@theme`)

| Token | Value | Role |
|---|---|---|
| `--color-bg` | `#0b1222` | ground |
| `--color-surface-1` | `#0f1830` | surface |
| `--color-surface-2` | `#13213e` | elevated |
| `--color-fg` | `#e7ecff` | ink |
| `--color-muted` | `#8894b6` | muted text |
| `--color-faint` | `#6f7ca6` | de-emphasised (AA on bg/surface) |
| `--color-grid` | `#17233f` | fine grid line (`body::before`) |
| `--color-hairline` | `#1c2b4a` | structural hairline / card borders |
| `--color-accent` | `#5bc8ff` | cyan — nodes, edges, links, primary |
| `--color-accent-strong` | `#8bd8ff` | cyan hover / emphasis |
| `--color-edit` | `#ffb454` | amber — **every mutation / write-back** |
| `--color-good / warn / bad` | `#46d39a / #ffb454 / #ff6b8a` | semantic |

Accent-derived: `--color-accent-muted/-glow/-fg`, `--color-edit-muted/-strong/-glow`.

**Swap only the `@theme` block to re-palette.** Everything else reads the tokens.

### Type
- `--font-display` Space Grotesk (headings via `text-display-xl/lg/h2`)
- `--font-sans` Inter (body)
- `--font-mono` JetBrains Mono (eyebrows, code, wordmark, labels, `tabular-nums`)

All three self-hosted woff2 in `public/fonts/`, wired in `app.css` `@font-face`.

### Motion (all gated behind `prefers-reduced-motion`)
- `.graph-edge` — cyan edges flow (dashed stroke offset)
- `.graph-node-pulse` — nodes breathe
- `.graph-edit` — amber edit node/edge glows
- `.reveal` — scroll-in, gated behind `html.js` so no-JS HTML renders visible

## Architecture (Void 0.10.2 / React 19 / Vite 8 / Tailwind v4 / Shiki)

```
website/
  package.json        void + @void/react + @void/md (0.10.2), shiki ^4.0.2, tailwind 4.3.2
  vite.config.ts      voidPlugin → voidReact → voidMarkdown → tailwindcss (order matters);
                      worker.format 'es'; playground-isolation-dev COOP/COEP middleware
  void.json           output:server; /playground COOP/COEP headers pre-staged; titleTemplate
  tsconfig.json       verbatimModuleSyntax (every type import uses `import type`)
  app.css             tokens + fonts + primitives + graph keyframes
  lib/highlight.ts    Shiki singleton, theme 'night-owl', JS regex engine (workerd-safe)
  pages/
    layout.tsx        sticky header (Playground · Docs · GitHub), CSS-only mobile drawer, footer
    index.tsx         landing (server-rendered, consumes loader Props)
    index.server.ts   loader highlights 5 snippets; prerender=false; head/OG
    _components/       primitives + sections
    _data/samples.ts  raw snippet source (README-seeded)
```

### Landing sections (in `index.tsx` order)
1. `Hero` — headline "See the shape of your wasm", CTAs, `InstallSwitcher`, `ModuleGraph`.
2. `Verbs` — Inspect / Edit / Build, each a Shiki snippet card.
3. `LiveHandleStory` — edit an export → re-emit → re-parse proves it persists.
4. `BuilderHighlight` — `add(a,b)` descriptor tree → run → `= 5`.
5. `ApiMap` — 27 classes grouped Core / Collections / Handles + 17 ValType constants.
6. `CtaBand` — → /playground.

`ModuleGraph.tsx` is the signature visual: self-contained inline SVG driven by a
hardcoded sample module (no runtime wasm — landing stays prerenderable). The one amber
node/edge is the export renamed `run → main`.

### Component prop contract (for the playground agent)
- `Button` — `variant: 'primary'|'secondary'|'ghost'`, renders `<a>`, spreads anchor attrs.
- `CodeBlock` — `{ html, copyText?, filename?, className? }`, injects Shiki HTML.
- `CopyButton` — `{ text }` (client).
- `InstallSwitcher` — client, localStorage `napi-wasm-tools:pm`.
- `Reveal` — `{ children, className?, delay?, as? }` (client, IntersectionObserver).
- `Chip` — `tone: 'accent'|'edit'|'muted'`.
- `SectionHeader` — `{ index?, label, title, subhead?, align? }`.
- `ModuleGraph` — `{ className? }`, no client JS.

### Loader pattern
`highlight()` runs inside `defineHandler` in `*.server.ts` at request time (workerd SSR).
`prerender=false` + `void.json revalidate {"*":0}` = instant-live deploys. Shiki uses the
pure-JS regex engine (`createJavaScriptRegexEngine`, `forgiving:true`) because workerd
forbids runtime `WebAssembly.instantiate` — keep this if you add more highlighting.

### Playground note (next agent)
`/playground` COOP/COEP headers are already staged in `void.json` and the Vite
`playground-isolation-dev` middleware, and `worker.format:'es'` is set. The published
`@napi-rs/wasm-tools-wasm32-wasi@1.0.1` predates the module-graph / buildFunction API
(#158/#159) — a rebuilt/vendored wasm ≥ the new API is required for the interactive graph.
