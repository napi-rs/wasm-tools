;; Fixture for functions.spec.ts (shared internal function-entry type).
;; Compiled to functions-shared-entry.wasm with
;;   `wat2wasm --debug-names functions-shared-entry.wat`
;; (--debug-names emits the "name" custom section so $a/$b survive parse).
;;
;; Two LOCAL functions with DISTINCT function types but the SAME result
;; signature `(result i32)`. walrus mints internal function-entry types from the
;; result signature only and dedups them structurally, so both entry blocks
;; share ONE entry type in the arena:
;;   func 0 ($a): local, type (param i32) (result i32) -> results (i32)
;;   func 1 ($b): local, type (param f64) (result i32) -> results (i32)
;;   arena types: [0]=(i32)->(i32), [1]=(f64)->(i32), [2]=shared entry (result i32)
;; Deleting ONE function must NOT drop the shared entry type (the other still
;; uses it); deleting the SECOND must drop it.
(module
  (func $a (param i32) (result i32)
    local.get 0)
  (func $b (param f64) (result i32)
    i32.const 2))
