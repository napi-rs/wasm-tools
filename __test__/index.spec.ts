import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { WasmModule } from '../index'
import { panic } from '../crates/panic/index.js'

const __dirname = join(fileURLToPath(import.meta.url), '..')

test('should be able to parse wasm', (t) => {
  t.notThrows(() => {
    WasmModule.fromPath(join(__dirname, '..', 'crates', 'panic', 'panic.wasm32-wasi.wasm'))
  })
})

test('should throw panic with source info', (t) => {
  const err = t.throws(() => panic()) as Error
  const frames = err.stack!.split('\n').map((line) => line.trim())
  // rustc name-section symbols now include a crate hash (`std[9a03…]::…`)
  // and dropped the `helpers::` segment; keep matching the abort frames.
  const stack = frames.join('\n')
  t.true(
    frames.some((line) =>
      /at panic\.wasm\.std(?:\[[0-9a-f]+\])?::sys::pal::wasi::(?:helpers::)?abort_internal/.test(line),
    ),
    stack,
  )
  t.true(
    frames.some((line) => /at panic\.wasm\.std(?:\[[0-9a-f]+\])?::process::abort/.test(line)),
    stack,
  )
})
