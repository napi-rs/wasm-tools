// Compat entry for the published `@napi-rs/wasm-tools/browser.js` subpath (shipped
// since v0.0.1). `napi build` regenerates this as a BARE
// `export * from '@napi-rs/wasm-tools-wasm32-wasi'` — which would re-expose the
// value-type constants UNFROZEN — so `scripts/write-browser-entry.mjs` rewrites it
// after every build to forward to the frozen browser wrapper. A deep
// `import '@napi-rs/wasm-tools/browser.js'` therefore stays frozen.
export * from './wasm-tools.browser.js'
