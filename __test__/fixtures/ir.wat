;; Fixture for ir.spec.ts — a single local function exercising the whole C1a
;; instruction subset, so instructions() can be checked against a module produced
;; independently by wat2wasm (not by our own buildFunction).
;;
;; Compiled to ir.wasm with `wat2wasm --debug-names ir.wat`
;; (--debug-names emits the "name" section so $callee/$main survive parse).
;;
;;   func 0 ($callee): local, () -> i32
;;   func 1 ($main):   local, (i32 i64) -> i32, with a f32 local
;;     body covers: all four const kinds, Drop, local get/set/tee, global
;;     get/set, call, typed + plain select, a single-value block, a multi-value
;;     block, nested empty blocks with br/br_if, a br_table over three blocks, a
;;     loop-in-block br, an if/else with unreachable, and return.
(module
  (global $g (mut i32) (i32.const 0))
  (func $callee (result i32) (i32.const 7))
  (func $main (param $p0 i32) (param $p1 i64) (result i32)
    (local $l0 f32)
    ;; --- consts + drop (all four const types) ---
    (drop (f32.const 1.5))
    (drop (f64.const 2.5))
    (drop (i64.const 9223372036854775807))
    (drop (i32.const 4))
    ;; --- local set/tee/get + global set/get ---
    (local.set $l0 (f32.const 3.5))
    (drop (local.get $l0))
    (global.set $g (local.get $p0))
    (drop (global.get $g))
    (drop (local.tee $p0 (i32.const 4)))
    (drop (local.get $p1))
    ;; --- call ---
    (drop (call $callee))
    ;; --- typed + plain select ---
    (drop (select (result i32) (i32.const 1) (i32.const 2) (i32.const 0)))
    (drop (select (i32.const 1) (i32.const 2) (i32.const 0)))
    ;; --- single-value block (result i32) => Simple(Some i32) ---
    (drop (block (result i32) (i32.const 20)))
    ;; --- multi-value block (param i32)(result i32 i32) => MultiValue ---
    (i32.const 21)
    (block (param i32) (result i32 i32)
      (i32.const 22))
    (drop) (drop)
    ;; --- nested empty blocks with br/br_if at depths ---
    (block
      (block
        (br_if 0 (i32.const 0))   ;; depth 0 -> inner
        (br 1)))                  ;; depth 1 -> outer
    ;; --- br_table over three nested blocks (targets 0,1 default 2) ---
    (block
      (block
        (block
          (br_table 0 1 2 (i32.const 0)))))
    ;; --- loop inside a block, br out of the block (depth 1 from the loop) ---
    (block
      (loop
        (br 1)))
    ;; --- if/else with unreachable in the else arm ---
    (if (result i32) (i32.const 1)
      (then (i32.const 40))
      (else (unreachable)))
    (drop)
    ;; --- return ---
    (return (i32.const 99)))
  (export "main" (func $main)))
