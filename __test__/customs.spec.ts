import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import test from 'ava'

import { WasmModule } from '../index'

// A minimal, valid, custom-section-free wasm module (8-byte magic + version).
const emptyModuleBytes = () => new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])

const tmpFile = (name: string) => join(mkdtempSync(join(tmpdir(), 'wasm-customs-')), name)

// Read a named custom section's bytes straight out of the wasm binary via V8,
// so we never depend on walrus round-tripping. Returns one entry per matching
// section (custom section names are not required to be unique).
const customSections = (wasm: Uint8Array, name: string): Uint8Array[] =>
  WebAssembly.Module.customSections(new WebAssembly.Module(wasm), name).map((buf) => new Uint8Array(buf))

test('addRaw persists a raw custom section through emit and re-parse', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.customs.addRaw('x.note', new Uint8Array([1, 2, 3]))

  // Present on the live instance right away.
  t.true(m.customs.list().some((s) => s.name === 'x.note'))

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const list = reparsed.customs.list()

  const found = list.find((s) => s.name === 'x.note')
  t.truthy(found)
  t.deepEqual(Array.from(found!.data!), [1, 2, 3])

  // emit also injects a `build_id` section, so assert presence rather than
  // the exact contents of the whole list.
  t.true(list.some((s) => s.name === 'build_id'))
})

test('removeRaw returns the bytes and drops the section on the next emit', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.customs.addRaw('x.note', new Uint8Array([1, 2, 3]))

  const reparsed = WasmModule.fromBuffer(m.emitWasm(false))
  const removed = reparsed.customs.removeRaw('x.note')
  t.truthy(removed)
  t.deepEqual(Array.from(removed!), [1, 2, 3])

  // Gone from the live instance immediately...
  t.false(reparsed.customs.list().some((s) => s.name === 'x.note'))

  // ...and stays gone after a round-trip through emit + re-parse.
  const reparsed2 = WasmModule.fromBuffer(reparsed.emitWasm(false))
  t.false(reparsed2.customs.list().some((s) => s.name === 'x.note'))
})

test('removeRaw returns null for a section that does not exist', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  t.is(m.customs.removeRaw('nope'), null)
})

// Invariant 1: emitting is non-destructive to the in-memory module. walrus'
// `emit_wasm` drains `self.customs`; our wrapper snapshots + restores them, so
// the section survives on the live instance AND a second emit still carries it.
test('emit is non-destructive: raw section survives on the module and across repeated emits', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.customs.addRaw('x.note', new Uint8Array([1, 2, 3]))

  const first = m.emitWasm(false)

  // Still on the live instance after emit (walrus alone would have drained it).
  t.true(m.customs.list().some((s) => s.name === 'x.note'))
  t.deepEqual(customSections(first, 'x.note'), [new Uint8Array([1, 2, 3])])

  // A SECOND emit of the very same instance still contains the section.
  const second = m.emitWasm(false)
  t.deepEqual(customSections(second, 'x.note'), [new Uint8Array([1, 2, 3])])
  t.true(m.customs.list().some((s) => s.name === 'x.note'))
})

// Invariant 2: `build_id` is added once and stays stable. Because customs are
// restored, `prepare_for_emit` sees the existing build_id on the next emit and
// does not regenerate a fresh uuid.
test('build_id is stable and unique across repeated emits of the same module', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())

  const first = m.emitWasm(false)
  const second = m.emitWasm(false)

  const firstIds = customSections(first, 'build_id')
  const secondIds = customSections(second, 'build_id')

  // Exactly one build_id each time...
  t.is(firstIds.length, 1)
  t.is(secondIds.length, 1)
  // ...and the bytes are identical across emits (no uuid regeneration).
  t.deepEqual(firstIds[0], secondIds[0])
})

// Invariant 3: emitWasmFile is non-destructive too (and therefore retry-safe:
// a failed fs write leaves customs already restored). We assert the section
// survives a successful write and a subsequent emit still carries it.
test('emitWasmFile preserves custom sections on the module', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  m.customs.addRaw('x.note', new Uint8Array([9, 8, 7]))

  const out = tmpFile('customs.wasm')
  m.emitWasmFile(out, false)

  // Section still present on the live instance after the file emit.
  t.true(m.customs.list().some((s) => s.name === 'x.note'))

  // The written file carries it, and a later in-memory emit does too.
  t.deepEqual(customSections(readFileSync(out), 'x.note'), [new Uint8Array([9, 8, 7])])
  t.deepEqual(customSections(m.emitWasm(false), 'x.note'), [new Uint8Array([9, 8, 7])])
})

// Invariant 4: adding a `.debug*` section is refused loudly, because walrus
// silently drops such sections from emit output.
test('addRaw rejects .debug* section names', (t) => {
  const m = WasmModule.fromBuffer(emptyModuleBytes())
  const err = t.throws(() => m.customs.addRaw('.debug_line', new Uint8Array([0])))
  t.regex(err!.message, /\.debug/)

  // And it really was not added.
  t.false(m.customs.list().some((s) => s.name === '.debug_line'))
})
