// Hand-authored package entry (see package.json `"main"`).
//
// The generated `index.js` exports the value-type constants (I32, FUNCREF, …
// from `src/valtype.rs`) as shared singleton objects: `#[napi] pub const`
// registers each value once, so every `import { FUNCREF }` returns the SAME
// object. Without this wrapper a consumer that mutates one (e.g.
// `FUNCREF.nullable = false`) would corrupt the shared value for every other
// caller in the process, including downstream `buildFunction`/`locals.add`.
// Deep-freeze them here so such a mutation is a no-op (a throw under strict mode).
//
// Everything else (classes, string-enum objects, functions) is re-exported
// unchanged. `module.exports = require('./index.js')` is the canonical CJS
// re-export form, so `import { WasmModule, I32 } from '@napi-rs/wasm-tools'`
// still resolves every named export through cjs-module-lexer.
module.exports = require('./index.js')

const deepFreeze = (value) => {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    for (const nested of Object.values(value)) deepFreeze(nested)
    Object.freeze(value)
  }
  return value
}

// The value/reference-type constants exported from `src/valtype.rs`. Only these
// are frozen; the pre-existing string-enum objects are left untouched.
const VALTYPE_CONSTANTS = [
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
]

for (const name of VALTYPE_CONSTANTS) {
  deepFreeze(module.exports[name])
}
