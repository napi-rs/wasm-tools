;; Fixture for the data-segments task (B3d).
;; One memory, one ACTIVE data segment at offset 0, one PASSIVE data segment.
;;   data[0]: active,  memory 0, offset (i32.const 0), bytes "hello"
;;   data[1]: passive,                                  bytes "world"
(module
  (memory 1)
  (data (i32.const 0) "hello")
  (data "world"))
