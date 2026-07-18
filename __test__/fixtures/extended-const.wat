;; Fixture for the F3 (ConstExprArg Extended-clone) regression in
;; elements.spec.ts. Its exported global's initializer is a MULTI-OP constant
;; expression, which walrus parses to `walrus::ConstExpr::Extended(Vec<ConstOp>)`
;; — the sole heap-owning ConstExpr variant. Reading it back via
;; `WasmGlobal.init()` yields a `ConstExpr` handle whose `.kind === 'Extended'`;
;; feeding that handle into `elements.addExpressions` must be rejected CATCHABLY
;; inside `ConstExprArg::from_napi_value` (before the infallible deep-clone),
;; not aborted.
;;
;;   type $arr: (array i32)
;;   global "g": (ref $arr), init = array.new_fixed $arr 2 (i32.const 1) (i32.const 2)
;;
;; Authoring-time only: compiled once with
;;   `wasm-tools parse extended-const.wat -o extended-const.wasm`.
;; The committed .wasm is what the (hermetic) test reads; the tool is NOT invoked
;; at test time.
(module
  (type $arr (array i32))
  (global (export "g") (ref $arr)
    (array.new_fixed $arr 2 (i32.const 1) (i32.const 2))))
