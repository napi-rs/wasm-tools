;; Fixture for the element-segments task (B3e).
;; One table, one function, and two element segments covering both the kind
;; discriminant (Active vs Passive) and the items union (Functions vs
;; Expressions):
;;   element[0]: ACTIVE  table 0, offset (i32.const 0), items Functions [$f]
;;   element[1]: PASSIVE items Expressions funcref [(ref.func $f), (ref.null func)]
;;
;; The passive segment mixes (ref.func $f) with (ref.null func) so wat2wasm is
;; forced to emit the expression encoding (a pure (ref.func) list collapses to
;; the MVP func-index / Functions form), giving us a real Expressions segment.
(module
  (table 4 funcref)
  (func $f)
  (elem (i32.const 0) $f)
  (elem funcref (ref.func $f) (ref.null func)))
