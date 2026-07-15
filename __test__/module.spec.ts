import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { ModuleConfig, WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')
const FIXTURE = join(__dirname, '..', 'crates', 'panic', 'panic.wasm32-wasi.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const tmpFile = (name: string) => join(mkdtempSync(join(tmpdir(), 'wasm-a1-')), name)

// `wasm-tools validate` returns non-zero and throws when the wasm is invalid,
// and is silent (no stdout) on success, so it keeps test output pristine.
const validate = (wasm: Uint8Array) => {
  const path = tmpFile('validate.wasm')
  writeFileSync(path, wasm)
  execFileSync('wasm-tools', ['validate', path])
}

const objdumpSections = (wasm: Uint8Array): string => {
  const path = tmpFile('objdump.wasm')
  writeFileSync(path, wasm)
  return execFileSync('wasm-tools', ['objdump', path], { encoding: 'utf8' })
}

test('fromBuffer parses and emitWasm(false) produces valid, re-parseable wasm', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  const emitted = m.emitWasm(false)
  t.true(emitted instanceof Uint8Array)
  t.true(emitted.length > 0)
  // Re-parsing proves the emitted bytes are structurally sound (walrus parses
  // with strict validation on by default).
  t.notThrows(() => WasmModule.fromBuffer(emitted))
  // And an independent validator agrees.
  t.notThrows(() => validate(emitted))
})

test('module name round-trips through emit and re-parse', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  m.name = 'hello'
  t.is(m.name, 'hello')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  t.is(reparsed.name, 'hello')

  // Clearing the name drops it from the emitted name section too.
  reparsed.name = null
  t.falsy(reparsed.name)
  const cleared = WasmModule.fromBuffer(reparsed.emitWasm(false))
  t.falsy(cleared.name)
})

test('gc does not throw and the module still emits and re-parses', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  t.notThrows(() => m.gc())
  const emitted = m.emitWasm(false)
  t.notThrows(() => WasmModule.fromBuffer(emitted))
  t.notThrows(() => validate(emitted))
})

test('emitWasmFile writes a file whose bytes re-parse and validate', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  const out = tmpFile('emitted.wasm')
  m.emitWasmFile(out, false)
  t.true(existsSync(out))

  const written = readFileSync(out)
  t.true(written.length > 0)
  t.notThrows(() => WasmModule.fromBuffer(written))
  t.notThrows(() => validate(written))

  // emitWasmFile shares the same pre-emit preparation as emitWasm: both keep a
  // `build_id` custom section in the output.
  t.regex(objdumpSections(written), /custom "build_id"/)
  t.regex(objdumpSections(m.emitWasm(false)), /custom "build_id"/)
})

test('writeGraphvizDot writes a non-empty dot graph', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  const out = tmpFile('module.dot')
  m.writeGraphvizDot(out)
  t.true(existsSync(out))
  const dot = readFileSync(out, 'utf8')
  t.true(dot.length > 0)
  t.regex(dot, /digraph|->/)
})

test('fromBufferWithConfig honors the supplied ModuleConfig', (t) => {
  const config = new ModuleConfig().generateProducersSection(false)
  const m = WasmModule.fromBufferWithConfig(fixtureBytes, config)
  const emitted = m.emitWasm(false)
  t.notThrows(() => WasmModule.fromBuffer(emitted))
  t.notThrows(() => validate(emitted))
  // With the producers section disabled the emitted module drops it.
  t.notRegex(objdumpSections(emitted), /custom "producers"/)
})
