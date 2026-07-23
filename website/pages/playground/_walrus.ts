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
}

export interface WFunction {
  readonly index: number
  readonly name: string | null
  readonly kind: string // 'Local' | 'Import' | 'Uninitialized'
  ty(): WType
  import(): WImport | null
}
export interface WGlobal {
  readonly index: number
  readonly name: string | null
  readonly mutable: boolean
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
  readonly initial: bigint
  readonly maximum: bigint | null
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
  readonly name: string
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
  readonly name: string | null
  readonly functions: Collection<WFunction>
  readonly globals: Collection<WGlobal>
  readonly memories: Collection<WMemory>
  readonly tables: Collection<WTable>
  readonly types: Collection<WType>
  readonly imports: Collection<WImport>
  readonly exports: Collection<WExport>
  readonly data: Collection<WData>
  readonly elements: Collection<WElement>
  readonly tags: Collection<WTag>
}

export interface WalrusMod {
  WasmModule: { fromBuffer(bytes: Uint8Array): WasmModule }
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
