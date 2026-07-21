;; Fixture for functions.spec.ts and locals.spec.ts.
;; Compiled to functions.wasm with `wat2wasm --debug-names functions.wat`
;; (--debug-names emits the "name" custom section so $imp/$loc survive parse).
;;
;;   func 0 ($imp): imported, type (param i32)            -> kind Import
;;   func 1 ($loc): local, type (param i32) (result i32)  -> kind Local
;;                  body declares a local i64 and returns its i32 param.
(module
  (import "env" "imported_fn" (func $imp (param i32)))
  (func $loc (param i32) (result i32)
    (local i64)
    local.get 0))
