;; Fixture for the memories collection tests. Compiled with:
;;   wat2wasm --enable-multi-memory memories.wat -o memories.wasm
;; and the resulting bytes are committed so the test stays hermetic (no CLI at
;; runtime). Two memories:
;;   memory 0: initial 1, maximum 2
;;   memory 1: initial 3, no maximum
(module
  (memory 1 2)
  (memory 3))
