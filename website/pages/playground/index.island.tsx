import Playground from './_playground' with { island: 'load' }

// Everything outside <Playground /> is plain SSR markup, so it is in the HTML before any
// JavaScript runs. That matters here: the tool is a `load` island, so a crawler or an LLM
// fetcher used to receive a bare "Playground / Loading…" shell with NO heading at all, and
// the hydrated page then reused the LANDING page's h1 ("See the shape of your wasm"). The
// page heading and its intro live here so the route describes itself without JS, and so the
// only thing hydration swaps is the tool body below it.
export default function PlaygroundPage() {
  return (
    <section className="container-wide py-12">
      <p className="eyebrow mb-3">Playground</p>
      <h1 className="text-display-lg mb-4 font-display text-(--color-fg)">
        WebAssembly playground
      </h1>
      <p className="mb-8 max-w-2xl text-(--color-muted)">
        Compile WAT or drop in a <span className="font-mono text-(--color-fg)">.wasm</span> file and
        walk its module graph in the browser — types, functions, globals, memories, tables, imports
        and exports, each one a live handle. Rename an export or grow a memory and re-emit the
        bytes, or compose a function from an instruction tree and run it. Everything runs locally
        in a Web Worker; no module ever leaves the page.
      </p>
      <Playground />
    </section>
  )
}
