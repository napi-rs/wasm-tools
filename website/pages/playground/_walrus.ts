// Minimal hand-written type surface for the walrus module graph API exposed by
// @napi-rs/wasm-tools (post-#158/#159). We deliberately do NOT type the dynamic
// import as `typeof import('@napi-rs/wasm-tools')`: the currently-published npm
// binary (1.0.1) predates this API and its .d.ts only declares ModuleConfig +
// WasmModule, so referencing the new methods through the package's own types
// would not compile. These interfaces cover exactly the read-only "inspect"
// surface the worker calls; the worker casts the imported module to `WalrusMod`.

export type ValType =
  | { type: 'I32' }
  | { type: 'I64' }
  | { type: 'F32' }
  | { type: 'F64' }
  | { type: 'V128' }
  | { type: 'Ref'; nullable: boolean; heap: HeapType }

export type HeapType =
  | { type: 'Abstract'; kind: string }
  | { type: 'Concrete'; typeIndex: number }
  | { type: 'Exact'; typeIndex: number }
  | { type: 'RecGroup'; recIndex: number }

interface Collection<T> {
  readonly length: number
  items(): T[]
  // Lookup by stable index (used by Edit mode to resolve a handle from a GraphNode).
  getByIndex(index: number): T | null
}

// ── Build (IR builder) surface (post-#158) ───────────────────────────────────
// A single const value nested under InstrDesc.value. I64 carries a BigInt.
export type ConstValue =
  | { type: 'I32'; value: number }
  | { type: 'I64'; value: bigint }
  | { type: 'F32'; value: number }
  | { type: 'F64'; value: number }
  | { type: 'V128'; value: Uint8Array }

// A wide tagged instruction record; only `type` (the walrus variant name) is
// required. The build presets use the LocalGet / Const / Binop subset. The three
// nested-body fields carry control-flow children (Block/Loop → `seq`; IfElse →
// `consequent`/`alternative`) so a reader can walk the full instruction tree.
export interface InstrDesc {
  type: string
  value?: ConstValue
  local?: number
  global?: number
  func?: number
  op?: string
  seq?: InstrDesc[]
  consequent?: InstrDesc[]
  alternative?: InstrDesc[]
  // Try/TryTable exception handlers: each clause's legacy handler body lives in
  // `catches[i].seq`, a third nesting level a full body walk must also descend.
  catches?: { seq?: InstrDesc[] }[]
}

// A module-wide local created before it is named in a function's arg list.
export interface WLocal {
  readonly index: number
}

interface LocalCollection {
  add(ty: ValType): WLocal
}

// Exports gain an `addFunction` factory in Build mode (name → new function export).
interface ExportCollection extends Collection<WExport> {
  addFunction(name: string, func: WFunction): WExport
}

export interface WFunction {
  readonly index: number
  readonly name: string | null
  readonly kind: string // 'Local' | 'Import' | 'Uninitialized'
  ty(): WType
  import(): WImport | null
  // Inverse of buildFunction — reads the body back as descriptors (round-trip).
  instructions(): InstrDesc[]
}
export interface WGlobal {
  readonly index: number
  readonly name: string | null
  mutable: boolean // has a real setter (Edit mode toggles it)
  readonly shared: boolean
  readonly ty: ValType
  readonly kind: string // 'Local' | 'Import'
  import(): WImport | null
}
export interface WMemory {
  readonly index: number
  readonly name: string | null
  readonly shared: boolean
  readonly memory64: boolean
  initial: bigint // has a real setter (Edit mode bumps it; value is BigInt)
  maximum: bigint | null
  readonly isImported: boolean
  import(): WImport | null
}
export interface WTable {
  readonly index: number
  readonly name: string | null
  readonly table64: boolean
  readonly initial: bigint
  readonly maximum: bigint | null
  readonly elementTy: ValType
  readonly isImported: boolean
  import(): WImport | null
}
export interface WType {
  readonly index: number
  readonly name: string | null
  readonly kind: string // 'Function' | 'Struct' | 'Array'
  params(): ValType[]
  results(): ValType[]
}
export interface WImport {
  readonly index: number
  readonly module: string
  readonly name: string
  readonly kind: string // 'Function' | 'Global' | 'Memory' | 'Table' | 'Tag'
  func(): WFunction | null
  table(): WTable | null
  memory(): WMemory | null
  global(): WGlobal | null
  tag(): WTag | null
}
export interface WExport {
  readonly index: number
  name: string // has a real setter (Edit mode renames it)
  readonly kind: string // 'Function' | 'Table' | 'Memory' | 'Global' | 'Tag'
  func(): WFunction | null
  table(): WTable | null
  memory(): WMemory | null
  global(): WGlobal | null
  tag(): WTag | null
}
export interface WData {
  readonly index: number
  readonly name: string | null
  readonly kind: string // 'Active' | 'Passive'
  readonly value: Uint8Array
  memory(): WMemory | null
}
export interface WElement {
  readonly index: number
  readonly name: string | null
  readonly kind: string // 'Passive' | 'Declared' | 'Active'
  readonly itemsKind: string // 'Functions' | 'Expressions'
  table(): WTable | null
  functionItems(): WFunction[] | null
}
export interface WTag {
  readonly index: number
  readonly name: string | null
  readonly kind: string
  ty(): WType
  import(): WImport | null
}

export interface WasmModule {
  name: string | null // get string|null / set string|undefined|null (Edit mode)
  readonly mainMemory: WMemory | null
  readonly functions: Collection<WFunction>
  readonly globals: Collection<WGlobal>
  readonly memories: Collection<WMemory>
  readonly tables: Collection<WTable>
  readonly types: Collection<WType>
  readonly imports: Collection<WImport>
  readonly exports: ExportCollection
  readonly data: Collection<WData>
  readonly elements: Collection<WElement>
  readonly tags: Collection<WTag>
  // Build mode: module-wide locals + the IR builder.
  readonly locals: LocalCollection
  buildFunction(
    params: ValType[],
    results: ValType[],
    argLocalIndices: number[],
    body: InstrDesc[],
  ): number
  // Emit the (possibly edited) module into an in-memory wasm buffer.
  // `demangle` runs Rust name demangling; false for a plain passthrough.
  emitWasm(demangle: boolean): Uint8Array
}

export interface WalrusMod {
  WasmModule: { fromBuffer(bytes: Uint8Array): WasmModule }
  // Ready-made ValType constants (a constant IS just its object, e.g. I32 === { type:'I32' }).
  I32: ValType
  I64: ValType
  F32: ValType
  F64: ValType
  V128: ValType
}

// ── ValType → display string ────────────────────────────────────────────────
export function valTypeLabel(v: ValType): string {
  switch (v.type) {
    case 'I32':
    case 'I64':
    case 'F32':
    case 'F64':
    case 'V128':
      return v.type.toLowerCase()
    case 'Ref': {
      const h = heapLabel(v.heap)
      return v.nullable ? `(ref null ${h})` : `(ref ${h})`
    }
  }
}

function heapLabel(h: HeapType): string {
  switch (h.type) {
    case 'Abstract':
      return h.kind.toLowerCase()
    case 'Concrete':
      return `type ${h.typeIndex}`
    case 'Exact':
      return `exact ${h.typeIndex}`
    case 'RecGroup':
      return `rec ${h.recIndex}`
  }
}
