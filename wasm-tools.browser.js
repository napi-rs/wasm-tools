// Hand-authored browser/wasm entry (see package.json `"browser"`).
//
// The wasm/browser build of the value-type constants (I32, FUNCREF, … from
// src/valtype.rs) are shared singleton objects, exactly like the native build
// (see wasm-tools.js). Without freezing, a consumer that mutates one (e.g.
// `FUNCREF.nullable = false`) would corrupt the shared value for every other
// caller in the process. Deep-freeze them here so such a mutation is a no-op
// (a throw under strict mode). Everything else is re-exported unchanged.
//
// This lives in its OWN file instead of `browser.js` because `napi build`
// OWNS `browser.js`: it regenerates it to a bare `export * from
// '…-wasm32-wasi'` on every build (the filename is hardcoded in the CLI), so a
// freeze loop written there is silently erased. Pointing `"browser"` at this
// wrapper keeps the guarantee durable — napi regenerates the throwaway
// `browser.js`; this file is never touched.
export * from '@napi-rs/wasm-tools-wasm32-wasi'

import * as binding from '@napi-rs/wasm-tools-wasm32-wasi'

const deepFreeze = (value) => {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const nested of Object.values(value)) deepFreeze(nested)
    Object.freeze(value)
  }
  return value
}

// The value/reference-type constants exported from `src/valtype.rs`. Only these
// are frozen; the pre-existing string-enum objects are left untouched.
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
