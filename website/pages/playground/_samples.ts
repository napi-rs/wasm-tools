// Seed WAT modules for the playground examples dropdown. Each is verified valid
// via wat2wasm and exercises a distinct slice of the module graph.
export type WatSample = { name: string; note: string; wat: string }

export const WAT_SAMPLES: WatSample[] = [
  {
    name: 'add — exported i32 add',
    note: '1 type · 1 local function · 1 export→func edge',
    wat: `(module
  (func $add (param $a i32) (param $b i32) (result i32)
    local.get $a
    local.get $b
    i32.add)
  (export "add" (func $add)))`,
  },
  {
    name: 'memory + mutable global + exports',
    note: 'memory · mutable global (global→type) · three export edges',
    wat: `(module
  (memory $mem 1 2)
  (global $counter (mut i32) (i32.const 42))
  (func $get (result i32) global.get $counter)
  (export "memory" (memory $mem))
  (export "counter" (global $counter))
  (export "get" (func $get)))`,
  },
  {
    name: 'imported env.log',
    note: 'import node (env.log) · import→func edge · fn→fn call · export',
    wat: `(module
  (import "env" "log" (func $log (param i32)))
  (func $run (call $log (i32.const 7)))
  (export "run" (func $run)))`,
  },
]

export const DEFAULT_WAT = WAT_SAMPLES[0].wat
