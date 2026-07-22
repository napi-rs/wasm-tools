import * as binding from '@napi-rs/wasm-tools-wasm32-wasi'

// See wasm-tools.js: the value-type constants (I32, FUNCREF, … from
// src/valtype.rs) are shared singleton objects. Deep-freeze them on the
// wasm/browser build too, so a consumer that mutates one cannot corrupt the
// shared value for every other caller. Everything else is re-exported unchanged.
const deepFreeze = (value) => {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const nested of Object.values(value)) deepFreeze(nested)
    Object.freeze(value)
  }
  return value
}

for (const name of [
  'I32',
  'I64',
  'F32',
  'F64',
  'V128',
  'FUNCREF',
  'EXTERNREF',
  'ANYREF',
  'EQREF',
  'I31REF',
  'STRUCTREF',
  'ARRAYREF',
  'NULLREF',
  'NULLFUNCREF',
  'NULLEXTERNREF',
  'EXNREF',
  'NULLEXNREF',
]) {
  deepFreeze(binding[name])
}

export * from '@napi-rs/wasm-tools-wasm32-wasi'
