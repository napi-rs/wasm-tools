import { join } from 'node:path'

import { panic } from '@napi-rs/wasm-panic'
import test from 'ava'

import { WasmModule } from '../index'

test('should be able to parse wasm', (t) => {
  t.notThrows(() => {
    WasmModule.fromPath(join(__dirname, '..', 'walrus.wasm32-wasi.wasm'))
  })
})

test('should throw panic with source info', (t) => {
  try {
    panic()
  } catch (err: any) {
    const [line2, line3] = err.stack
      .split('\n')
      .slice(2, 4)
      .map((line: string) => line.trim())
    t.true(line2.startsWith('at std::panicking::rust_panic_with_hook'))
    t.true(line3.startsWith('at std::panicking::begin_panic_handler::{{closure}}'))
  }
})
