// Types for the hand-authored entry `wasm-tools.js` (see package.json `"types"`).
//
// Re-export the generated surface unchanged, then re-declare the value-type
// constants as deeply `readonly` AND narrowed to the exact `ValType` variant each
// one represents. Two goals:
//   1. Discriminant preservation. Each constant is typed as its extracted union
//      member (`Extract<ValType, { type: 'I32' }>`, and `Extract<ValType,
//      { type: 'Ref' }>` for the 12 reference constants) instead of the whole
//      `ValType` union. This keeps the discriminant, so reads like
//      `FUNCREF.nullable` type-check and `I32` is assignable to a narrow
//      `{ type: 'I32' }` parameter. (The generated union has a single `Ref`
//      member and encodes no per-constant heap kind, so all reference constants
//      share `Extract<ValType, { type: 'Ref' }>` — the most precise faithful
//      type available; reach a specific `heap.kind` by narrowing on `heap.type`.)
//   2. Read-only. `wasm-tools.js` deep-freezes those constants at runtime, so a
//      mutation like `FUNCREF.nullable = false` throws; wrapping in `DeepReadonly`
//      makes it a COMPILE error too, matching the runtime contract.
// An explicit local export wins over the `export *` for the same name, so these
// override the generated `export const I32: ValType` declarations. Because each
// extracted subtype is still structurally a `ValType`, the constants stay
// assignable wherever a `ValType` is expected (`locals.add`, `buildFunction`, ...).
export * from './index'

import type { ValType } from './index'

/** Deeply-`readonly` view of `T` (nested fields like `FUNCREF.heap.kind` too). */
type DeepReadonly<T> = T extends (infer U)[]
  ? ReadonlyArray<DeepReadonly<U>>
  : T extends object
    ? { readonly [K in keyof T]: DeepReadonly<T[K]> }
    : T

export declare const I32: DeepReadonly<Extract<ValType, { type: 'I32' }>>
export declare const I64: DeepReadonly<Extract<ValType, { type: 'I64' }>>
export declare const F32: DeepReadonly<Extract<ValType, { type: 'F32' }>>
export declare const F64: DeepReadonly<Extract<ValType, { type: 'F64' }>>
export declare const V128: DeepReadonly<Extract<ValType, { type: 'V128' }>>
export declare const FUNCREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const EXTERNREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const ANYREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const EQREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const I31REF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const STRUCTREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const ARRAYREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const NULLREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const NULLFUNCREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const NULLEXTERNREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const EXNREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
export declare const NULLEXNREF: DeepReadonly<Extract<ValType, { type: 'Ref' }>>
