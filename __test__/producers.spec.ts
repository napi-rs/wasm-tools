import test from 'ava'

import { WasmModule } from '../index'

// A minimal, valid, custom-section-free wasm module (8-byte magic + version).
// Fresh array per call so tests never share module state.
const emptyModuleBytes = () => new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])

// Count occurrences of a named custom section via V8's WebAssembly API (no CLI).
const customSectionCount = (wasm: Uint8Array, name: string): number =>
  WebAssembly.Module.customSections(new WebAssembly.Module(wasm), name).length

test('added producer fields persist through emit and re-parse', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.producers.addSdk('napi', '2.0')
  m.producers.addLanguage('Rust', '1.0')
  m.producers.addProcessedBy('mytool', '3.0')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const fields = reparsed.producers.fields()

  const sdk = fields.find((f) => f.name === 'sdk')
  t.truthy(sdk)
  t.deepEqual(sdk?.values, [{ name: 'napi', version: '2.0' }])

  const language = fields.find((f) => f.name === 'language')
  t.truthy(language)
  t.deepEqual(language?.values, [{ name: 'Rust', version: '1.0' }])

  // walrus itself appends a `processed-by: walrus` value on every parse, so the
  // processed-by field carries both walrus and our tool — assert ours is there.
  const processedBy = fields.find((f) => f.name === 'processed-by')
  t.truthy(processedBy)
  t.true(processedBy!.values.some((v) => v.name === 'mytool' && v.version === '3.0'))
})

test('clear empties the producers section and no producers section is emitted', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.producers.addSdk('napi', '2.0')
  m.producers.addLanguage('Rust', '1.0')
  t.true(m.producers.fields().length > 0)

  m.producers.clear()
  // clear wipes every field, including walrus' own processed-by, in memory.
  t.is(m.producers.fields().length, 0)

  // With no fields, walrus emits no `producers` custom section at all — this is
  // the hermetic write-through proof (re-parsing would re-add processed-by).
  const emitted = m.emitWasm(false)
  t.is(customSectionCount(emitted, 'producers'), 0)
})

test('adding the same key twice updates the value in place', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.producers.addSdk('napi', '1.0')
  m.producers.addSdk('napi', '2.0')

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const sdk = reparsed.producers.fields().find((f) => f.name === 'sdk')
  t.deepEqual(sdk?.values, [{ name: 'napi', version: '2.0' }])
})
