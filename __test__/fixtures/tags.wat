;; Exception-handling tags fixture (compiled at authoring time with
;;   wat2wasm --enable-exceptions tags.wat -o tags.wasm
;; and committed as bytes; wat2wasm is never invoked at test runtime).
;;   type 0 = (func (param i32))
;;   tag  0 = local exception tag of type 0
(module
  (type $e (func (param i32)))
  (tag $t (type $e))
)
