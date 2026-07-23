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
  {
    // A shared memory only VALIDATES under the threads feature. It exists to prove the
    // worker feeds the same feature set to wabt's validate() as to parseWat() — without
    // that, validate() falls back to baseline features and rejects this valid module.
    name: 'shared memory (threads)',
    note: 'shared memory — needs the threads feature at validate time',
    wat: `(module
  (memory $mem 1 1 shared)
  (export "memory" (memory $mem)))`,
  },
  {
    // A global initialized with (i32.add …) is an EXTENDED-CONST expression — it only
    // parses/validates under the extended_const feature. walrus (the .wasm upload path)
    // accepts it, so the WAT path must enable the same flag or the two disagree.
    name: 'extended-const global',
    note: 'global init uses i32.add — needs the extended_const feature',
    wat: `(module
  (global $answer i32 (i32.add (i32.const 40) (i32.const 2)))
  (export "answer" (global $answer)))`,
  },
  {
    // Exercises the table section: a funcref table (table64 = false) seeded by an active
    // element segment, with a call_indirect through it — so the graph renders a table node
    // (carrying its table64 flag) alongside the element segment.
    name: 'funcref table + call_indirect',
    note: 'table (funcref, 32-bit) · active element segment · export→table edge',
    wat: `(module
  (type $ft (func (result i32)))
  (func $forty_two (type $ft) (result i32) i32.const 42)
  (table $tbl 1 funcref)
  (elem (i32.const 0) $forty_two)
  (func $call (result i32) (call_indirect (type $ft) (i32.const 0)))
  (export "table" (table $tbl))
  (export "call" (func $call)))`,
  },
]

export const DEFAULT_WAT = WAT_SAMPLES[0].wat
