;; Fixture for globals.spec.ts — two globals with known, distinct properties.
;;
;; global 0: mutable   i32, initial 42
;; global 1: immutable i32, initial 7
;;
;; The exported func reads global 1 (NOT global 0). The delete test removes
;; global 0, which is referenced by nothing, so the emitted module stays valid
;; (walrus makes removing references the caller's responsibility).
;;
;; Authoring-time only: compiled once with `wat2wasm globals.wat -o globals.wasm`.
;; The committed globals.wasm is what the (hermetic) test reads; wat2wasm is
;; NOT invoked at test time.
(module
  (global (mut i32) (i32.const 42))
  (global i32 (i32.const 7))
  (func (export "f") (result i32) (global.get 1)))
