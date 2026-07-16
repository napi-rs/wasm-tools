;; Fixture for types.spec.ts — an EXPLICIT recursive type group `(rec ...)`.
;;
;;   (rec
;;     (type $a (;0;) (struct (field (ref null $b))))
;;     (type $b (;1;) (struct (field (ref null $a))))
;;   )
;;
;; Two mutually-referencing GC struct types wrapped in an explicit `(rec ...)`.
;; Used to prove `WasmType.recGroupMembers()` lists BOTH sibling handles and
;; `WasmType.isExplicitRecGroup === true` for a parsed explicit rec group.
;;
;; Parsed in the test via `new ModuleConfig().onlyStableFeatures(false).parse()`.
;;
;; Authoring-time only: compiled once with
;;   wasm-tools parse types-rec.wat -o types-rec.wasm
;; The committed types-rec.wasm is what the (hermetic) test reads; wasm-tools is
;; NOT invoked at test time.
(module
  (rec
    (type $a (struct (field (ref null $b))))
    (type $b (struct (field (ref null $a))))
  )
)
