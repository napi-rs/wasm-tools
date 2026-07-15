import { existsSync, mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test, { type ExecutionContext } from 'ava'

import { ModuleConfig, WasmModule } from '../index'

const __dirname = join(fileURLToPath(import.meta.url), '..')
const FIXTURE = join(__dirname, '..', 'crates', 'panic', 'panic.wasm32-wasi.wasm')
const fixtureBytes = readFileSync(FIXTURE)

const tmpFile = (name: string) => join(mkdtempSync(join(tmpdir(), 'wasm-a1-')), name)

// A minimal, valid wasm module: the 8-byte magic + version header with no
// sections. walrus parses it and `new WebAssembly.Module` always compiles it,
// which makes it a clean, custom-section-free baseline for asserting that a
// `build_id` section is *added* on emit. Returns a fresh array each call so
// tests never share module state.
const emptyModuleBytes = () => new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])

// Structural-soundness check that stays fully hermetic (no external process):
// re-parsing runs walrus' strict validation (on by default), and
// `WebAssembly.validate` is an independent second opinion from V8's decoder.
const assertValid = (t: ExecutionContext, wasm: Uint8Array) => {
  t.notThrows(() => WasmModule.fromBuffer(wasm))
  t.true(WebAssembly.validate(wasm))
}

// Count occurrences of a named custom section via V8's WebAssembly API, which
// returns an `ArrayBuffer[]` (one entry per matching section). No CLI involved.
const customSectionCount = (wasm: Uint8Array, name: string): number =>
  WebAssembly.Module.customSections(new WebAssembly.Module(wasm), name).length

test('fromBuffer parses and emitWasm(false) produces valid, re-parseable wasm', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  const emitted = m.emitWasm(false)
  t.true(emitted instanceof Uint8Array)
  t.true(emitted.length > 0)
  // Re-parsing proves the emitted bytes are structurally sound (walrus parses
  // with strict validation on by default); WebAssembly.validate agrees.
  assertValid(t, emitted)
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
  assertValid(t, emitted)
})

test('emitWasmFile writes a file whose bytes re-parse and validate', (t) => {
  const m = WasmModule.fromBuffer(fixtureBytes)
  const out = tmpFile('emitted.wasm')
  m.emitWasmFile(out, false)
  t.true(existsSync(out))

  const written = readFileSync(out)
  t.true(written.length > 0)
  assertValid(t, written)
})

test('emit adds exactly one build_id section when the module has none', (t) => {
  // Fresh, custom-section-free module: emitWasm must ADD a single build_id.
  const viaBuffer = WasmModule.fromBuffer(emptyModuleBytes()).emitWasm(false)
  t.is(customSectionCount(viaBuffer, 'build_id'), 1)

  // A *different* fresh instance emitted to a file must independently produce
  // exactly one build_id in the written bytes — proving emitWasmFile runs the
  // same pre-emit preparation as emitWasm, not just that presence is preserved.
  const out = tmpFile('buildid.wasm')
  WasmModule.fromBuffer(emptyModuleBytes()).emitWasmFile(out, false)
  const written = readFileSync(out)
  t.is(customSectionCount(written, 'build_id'), 1)

  // Idempotence: re-parsing bytes that ALREADY carry a build_id and emitting
  // again must still yield exactly one — the prep must not duplicate it.
  const reEmitted = WasmModule.fromBuffer(viaBuffer).emitWasm(false)
  t.is(customSectionCount(reEmitted, 'build_id'), 1)
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
  assertValid(t, emitted)
  // With the producers section disabled the emitted module drops it entirely.
  t.is(customSectionCount(emitted, 'producers'), 0)
})
