;; Fixture for the B3e gc-abort regression test.
;; The table is EXPORTED, so gc() treats it as a ROOT and traverses its
;; `elem_segments` back-link set — the exact path that aborts when a deleted
;; active element leaves a stale back-link behind.
;;   table 0: 4 funcref, EXPORTED as "tbl"
;;   element[0]: ACTIVE table 0, offset (i32.const 0), items Functions [$f]
(module
  (table (export "tbl") 4 funcref)
  (func $f)
  (elem (i32.const 0) $f))
