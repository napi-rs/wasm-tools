;; Fixture for the B3d gc-abort regression tests.
;; The memory is EXPORTED, so gc() treats it as a ROOT and traverses its
;; `data_segments` back-link set — the exact path that aborts when a deleted
;; active segment leaves a stale back-link behind.
;;   memory 0: initial 1, EXPORTED as "mem"
;;   data[0]: active,  memory 0, offset (i32.const 0), bytes "hi"
(module
  (memory (export "mem") 1)
  (data (i32.const 0) "hi"))
