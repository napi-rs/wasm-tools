import { join } from 'node:path'
import { readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

import { ModuleConfig } from '@napi-rs/wasm-tools'

const __dirname = join(fileURLToPath(import.meta.url), '..')

const wasm = await readFile(join(__dirname, 'panic.wasm32-wasi.wasm'))

const binary = new ModuleConfig()
  .generateDwarf(true)
  .generateNameSection(true)
  .generateProducersSection(true)
  .preserveCodeTransform(true)
  .parse(wasm)
  .emitWasm(true)

await writeFile(join(__dirname, 'panic.wasm32-wasi.wasm'), binary)
