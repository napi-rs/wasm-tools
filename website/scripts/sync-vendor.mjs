// Populate website/vendor/wasm-tools-wasm32-wasi/ from the repo's OWN wasm build.
//
// Why this exists: the published @napi-rs/wasm-tools-wasm32-wasi@1.0.1 on npm predates the
// module-graph + instruction-builder API (#158/#159), so the playground can't use it yet. Until
// a >=1.0.2 browser build is published, the site consumes the repo's local wasm build directly.
// The .wasm binary is git-ignored (root .gitignore `*.wasm`), so instead of committing a 2.3MB
// blob we regenerate the vendor dir from the repo root on every dev/build. This also guarantees
// the playground always runs the CURRENT source, never a stale copy.
//
// When 1.0.2 ships: delete website/vendor/, drop predev/prebuild below, and set the dependency
// "@napi-rs/wasm-tools-wasm32-wasi" back to the published version.
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '..', '..')
const vendor = resolve(here, '..', 'vendor', 'wasm-tools-wasm32-wasi')

// The napi wasm build emits these at the repo root (`napi build ... --target wasm32-wasi-preview1-threads`).
const FILES = [
  'walrus.wasm32-wasi.wasm',
  'walrus.wasi-browser.js',
  'walrus.wasi.cjs',
  'wasi-worker-browser.mjs',
  'wasi-worker.mjs',
]

mkdirSync(vendor, { recursive: true })

const missing = FILES.filter((f) => !existsSync(join(repoRoot, f)))
if (missing.length) {
  console.error(
    `\n[sync-vendor] Missing repo wasm build artifacts:\n  ${missing.join('\n  ')}\n\n` +
      `Build them at the repo root first, e.g.:\n` +
      `  napi build --platform --release --target wasm32-wasi-preview1-threads\n`,
  )
  process.exit(1)
}

for (const f of FILES) {
  copyFileSync(join(repoRoot, f), join(vendor, f))
}
console.log(`[sync-vendor] copied ${FILES.length} artifacts → vendor/wasm-tools-wasm32-wasi/`)
