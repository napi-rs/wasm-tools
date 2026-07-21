;; Fixture for types.spec.ts — a single named function type, referenced by a
;; function so it survives parsing.
;;
;;   type 0: $a = (func (param i32 f32) (result i64))
;;
;; The function references $a (and repeats its signature inline), so walrus
;; keeps exactly one function type in the type arena after parse.
;;
;; Authoring-time only: compiled once with `wat2wasm types.wat -o types.wasm`.
;; The committed types.wasm is what the (hermetic) test reads; wat2wasm is NOT
;; invoked at test time.
(module
  (type $a (func (param i32 f32) (result i64)))
  (func $f (type $a) (param i32 f32) (result i64) i64.const 0))
