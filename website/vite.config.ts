import { defineConfig } from 'vite'
import { voidPlugin } from 'void'
import { voidReact } from '@void/react/plugin'
import { voidMarkdown } from '@void/md/plugin'
import tailwindcss from '@tailwindcss/vite'

// The /playground route (added by a follow-up agent) runs the @napi-rs/wasm-tools
// wasm build inside a Worker. That browser bundle is a top-level-await ESM that spawns
// its own nested module worker, so both worker layers must be emitted as ES modules —
// the default 'iife' worker format cannot handle the top-level await and fails the
// build. Set format:'es' up front so the playground drops in cleanly.
// Shared dev/preview middleware that (1) stamps COOP/COEP on the document and CORP on
// every response so the cross-origin-isolated /playground can load its worker, wasm and
// sample subresources, and (2) de-doubles a nested-worker path if `vite dev` ever
// resolves the @napi-rs/wasm-tools-wasm32-wasi internal worker with its package dir
// duplicated. Unlike @napi-rs/image, wasm-tools spawns its nested worker with a RELATIVE
// specifier (`new Worker(new URL('./wasi-worker-browser.mjs', import.meta.url))`), which
// does not double the same way — so the rewrite is a harmless safety net that only fires
// if a doubled segment ever appears (verify in the dev network tab against a rebuilt wasm).
const DUPLICATED_WASM_PKG = '@napi-rs/wasm-tools-wasm32-wasi/@napi-rs/wasm-tools-wasm32-wasi/'
function playgroundIsolationMiddleware(
  req: { url?: string },
  res: { setHeader: (name: string, value: string) => void },
  next: () => void,
) {
  res.setHeader('Cross-Origin-Opener-Policy', 'same-origin')
  res.setHeader('Cross-Origin-Resource-Policy', 'same-origin')
  res.setHeader('Cross-Origin-Embedder-Policy', 'require-corp')
  if (req.url && req.url.includes(DUPLICATED_WASM_PKG)) {
    req.url = req.url.replace(DUPLICATED_WASM_PKG, '@napi-rs/wasm-tools-wasm32-wasi/')
  }
  next()
}

export default defineConfig({
  worker: { format: 'es' },
  resolve: {
    // The playground worker imports the public `@napi-rs/wasm-tools` API, but in the
    // browser that wrapper's `main` (index.js) is the universal NATIVE loader — it
    // `require()`s a platform `.node` binary, which Vite/rolldown cannot parse as JS.
    // The package's `browser` field points at `wasm-tools.browser.mjs`, which is just
    // `export * from '@napi-rs/wasm-tools-wasm32-wasi'`, so alias straight to the wasm
    // build everywhere Vite bundles. The native loader is never touched.
    alias: {
      '@napi-rs/wasm-tools': '@napi-rs/wasm-tools-wasm32-wasi',
    },
  },
  plugins: [
    // Cross-origin isolation for /playground so it can use SharedArrayBuffer + wasm
    // threads under `vite dev` / `vite preview`. The deployed worker sets COOP/COEP/CORP
    // per-route via void.json; the dev/preview server does not, so we replay the headers
    // here (plus the nested-worker path de-doubling). apply:'serve' — a production
    // `vite build` bundles the worker and the edge headers cover the rest.
    {
      name: 'playground-isolation-dev',
      apply: 'serve',
      configureServer(server) {
        server.middlewares.use(playgroundIsolationMiddleware)
      },
      configurePreviewServer(server) {
        server.middlewares.use(playgroundIsolationMiddleware)
      },
    },
    voidPlugin(),
    voidReact(),
    voidMarkdown(), // enforce:'pre', auto-detects React → MUST come after voidReact()
    tailwindcss(),
  ],
})
