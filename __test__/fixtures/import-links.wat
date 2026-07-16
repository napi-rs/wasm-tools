;; Reverse item->import cross-link + start/mainMemory fixture (task B4a).
;; Compiled at authoring time with
;;   wat2wasm --enable-exceptions import-links.wat -o import-links.wasm
;; and committed as bytes; wat2wasm is never invoked at test runtime.
;;
;; imports (in order):
;;   import 0 = "e"/"mem" -> Memory   (memory index 0, imported)
;;   import 1 = "e"/"tbl" -> Table    (table  index 0, imported)
;;   import 2 = "e"/"tag" -> Tag      (tag    index 0, imported)
;;   import 3 = "e"/"f"   -> Function (function index 0, imported)
;;   import 4 = "e"/"g"   -> Global   (global index 0, imported)
;; locally defined:
;;   table  index 1  (local funcref table)
;;   tag    index 1  (local exception tag)
;;   function index 1 ($start, the start function)
;;   global index 1  (local i32 global)
;; start = function index 1 ($start)
;;
;; So each imported item's `.import()` resolves to its WasmImport, and each
;; LOCAL item's `.import()` is null; `mainMemory` is memory 0; `start` is func 1.
(module
  (import "e" "mem" (memory 1))
  (import "e" "tbl" (table 1 funcref))
  (import "e" "tag" (tag (param i32)))
  (import "e" "f" (func $f))
  (import "e" "g" (global $g i32))
  (table 1 funcref)
  (tag (param i32))
  (func $start)
  (global i32 (i32.const 0))
  (start $start)
)
