;; Fixture for the tables collection tests. Compiled with:
;;   wat2wasm tables.wat -o tables.wasm
;; and the resulting bytes are committed so the test stays hermetic (no CLI at
;; runtime). Two tables:
;;   table 0: initial 1, maximum 4, funcref
;;   table 1: initial 2, no maximum, externref
(module
  (table 1 4 funcref)
  (table 2 externref))
