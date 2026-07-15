import test from 'ava'

import { WasmModule } from '../index'

// A minimal, valid, custom-section-free wasm module (8-byte magic + version).
const emptyModuleBytes = () => new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])

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
