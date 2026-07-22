// Restore `browser.js` to a frozen forwarding shim after `napi build`.
//
// `napi build` regenerates the root `browser.js` as a BARE
// `export * from '@napi-rs/wasm-tools-wasm32-wasi'` — the filename is hardcoded
// in @napi-rs/cli, so it cannot be redirected. That bare form re-exports the
// value-type constants UNFROZEN, and `browser.js` is a published subpath
// (`@napi-rs/wasm-tools/browser.js`, shipped since v0.0.1). Forwarding it to the
// frozen browser wrapper keeps that long-standing subpath resolvable AND frozen.
//
// The publish job packs the committed source (it never runs `napi build`), so the
// committed forward is what actually ships; this script just keeps local/CI builds
// from leaving the bare form behind for someone to commit by accident.
import { writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const CONTENT = `// Compat entry for the published \`@napi-rs/wasm-tools/browser.js\` subpath (shipped
// since v0.0.1). \`napi build\` regenerates this as a BARE
// \`export * from '@napi-rs/wasm-tools-wasm32-wasi'\` — which would re-expose the
// value-type constants UNFROZEN — so \`scripts/write-browser-entry.mjs\` rewrites it
// after every build to forward to the frozen browser wrapper. A deep
// \`import '@napi-rs/wasm-tools/browser.js'\` therefore stays frozen.
export * from './wasm-tools.browser.js'
`

writeFileSync(fileURLToPath(new URL('../browser.js', import.meta.url)), CONTENT)
