;; Imports + exports fixture for the imports/exports collection tasks (B3g).
;; Compiled at authoring time with
;;   wat2wasm imports-exports.wat -o imports-exports.wasm
;; and committed as bytes; wat2wasm is never invoked at test runtime.
;;
;; imports (in order):
;;   import 0 = "e"/"f" -> Function (function index 0)
;;   import 1 = "e"/"g" -> Global   (global   index 0)
;; locally defined:
;;   memory index 0 (exported as "mem")
;;   function index 1 (exported as "run")
;;   global index 1 (exported as "g2")
;; exports (in order): "mem" (Memory 0), "run" (Function 1), "g2" (Global 1)
(module
  (import "e" "f" (func $f))
  (import "e" "g" (global $g i32))
  (memory (export "mem") 1)
  (func (export "run"))
  (global (export "g2") i32 (i32.const 0))
)
