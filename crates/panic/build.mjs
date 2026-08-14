import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

import { ModuleConfig } from '@napi-rs/wasm-tools'

const __dirname = join(fileURLToPath(import.meta.url), '..')

// napi-rs 3.8 emits a stripped `*.wasm` (no name section) plus a sibling
// `*.debug.wasm` that still has names/DWARF. The loader prefers the debug
// file when it exists (local `napi build`) but CI only checks out the
// tracked release wasm. Feed the debug artifact into this pass so the
// committed file keeps source-mapped rust frames.
const debugPath = join(__dirname, 'panic.wasm32-wasi.debug.wasm')
const releasePath = join(__dirname, 'panic.wasm32-wasi.wasm')
const wasm = await readFile(existsSync(debugPath) ? debugPath : releasePath)

const binary = new ModuleConfig()
  .generateDwarf(true)
  .generateNameSection(true)
  .generateProducersSection(true)
  .preserveCodeTransform(true)
  .parse(wasm)
  .emitWasm(true)

await writeFile(releasePath, binary)
