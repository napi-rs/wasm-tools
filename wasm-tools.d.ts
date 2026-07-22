// Types for the hand-authored entry `wasm-tools.js` (see package.json `"types"`).
//
// Re-export the generated surface unchanged, then re-declare the value-type
// constants as deeply `readonly`. `wasm-tools.js` deep-freezes those constants at
// runtime, so a mutation like `FUNCREF.nullable = false` throws; typing them
// readonly makes it a COMPILE error too, matching the runtime contract. An
// explicit local export wins over the `export *` for the same name, so these
// override the generated `export const I32: ValType` declarations. The constants
// stay assignable wherever a `ValType` is expected.
export * from './index'

import type { ValType } from './index'

/** Deeply-`readonly` view of `T` (nested fields like `FUNCREF.heap.kind` too). */
type DeepReadonly<T> = T extends (infer U)[]
  ? ReadonlyArray<DeepReadonly<U>>
  : T extends object
    ? { readonly [K in keyof T]: DeepReadonly<T[K]> }
    : T

export declare const I32: DeepReadonly<ValType>
export declare const I64: DeepReadonly<ValType>
export declare const F32: DeepReadonly<ValType>
export declare const F64: DeepReadonly<ValType>
export declare const V128: DeepReadonly<ValType>
export declare const FUNCREF: DeepReadonly<ValType>
export declare const EXTERNREF: DeepReadonly<ValType>
export declare const ANYREF: DeepReadonly<ValType>
export declare const EQREF: DeepReadonly<ValType>
export declare const I31REF: DeepReadonly<ValType>
export declare const STRUCTREF: DeepReadonly<ValType>
export declare const ARRAYREF: DeepReadonly<ValType>
export declare const NULLREF: DeepReadonly<ValType>
export declare const NULLFUNCREF: DeepReadonly<ValType>
export declare const NULLEXTERNREF: DeepReadonly<ValType>
export declare const EXNREF: DeepReadonly<ValType>
export declare const NULLEXNREF: DeepReadonly<ValType>
