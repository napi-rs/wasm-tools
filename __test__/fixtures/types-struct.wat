;; Fixture for types.spec.ts — a GC struct type (non-function composite type).
;;
;;   type 0: $s = (struct (field i32))
;;
;; Used to prove that `WasmType.kind` safely reports 'Struct' and that
;; `params()` / `results()` return a catchable error (rather than aborting the
;; process on walrus' internal `unwrap_function` panic) for a non-function type.
;;
;; Parsed in the test via `new ModuleConfig().onlyStableFeatures(false).parse()`.
;;
;; Authoring-time only: compiled once with
;;   wat2wasm --enable-gc types-struct.wat -o types-struct.wasm
;; The committed types-struct.wasm is what the (hermetic) test reads; wat2wasm
;; is NOT invoked at test time.
(module
  (type $s (struct (field i32))))
