;; Fixture for globals-types.spec.ts — globals covering every non-concrete
;; `ValType` variant so the walrus -> napi value-type conversion can be
;; exercised end to end.
;;
;;   global 0: mutable   i32       -> ValType::I32
;;   global 1: immutable i64       -> ValType::I64
;;   global 2: immutable f32       -> ValType::F32
;;   global 3: immutable f64       -> ValType::F64
;;   global 4: immutable externref -> ValType::Ref { nullable: true, heap: Abstract(Extern) }
;;   global 5: immutable funcref   -> ValType::Ref { nullable: true, heap: Abstract(Func) }
;;   global 6: immutable v128      -> ValType::V128
;;
;; All seven are LOCAL globals (GlobalKind::Local), so `.kind` reads 'Local'.
;;
;; HeapType::Concrete / HeapType::Exact are intentionally NOT produced here:
;; they require GC / typed-function-reference globals whose init expressions are
;; awkward to author, and the read path is exhaustively mapped in convert.rs
;; regardless. See task report.
;;
;; Authoring-time only: compiled once with
;;   wat2wasm --enable-all global-types.wat -o global-types.wasm
;; The committed global-types.wasm is what the (hermetic) test reads; wat2wasm
;; is NOT invoked at test time.
(module
  (global (mut i32) (i32.const 1))
  (global i64 (i64.const 2))
  (global f32 (f32.const 3))
  (global f64 (f64.const 4))
  (global externref (ref.null extern))
  (global funcref (ref.null func))
  (global v128 (v128.const i32x4 0 0 0 0)))
