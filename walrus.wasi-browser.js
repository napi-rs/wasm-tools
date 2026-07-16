import {
  createOnMessage as __wasmCreateOnMessageForFsProxy,
  getDefaultContext as __emnapiGetDefaultContext,
  instantiateNapiModuleSync as __emnapiInstantiateNapiModuleSync,
  WASI as __WASI,
} from '@napi-rs/wasm-runtime'



const __wasi = new __WASI({
  version: 'preview1',
})

const __wasmUrl = new URL('./walrus.wasm32-wasi.wasm', import.meta.url).href
const __emnapiContext = __emnapiGetDefaultContext()


const __sharedMemory = new WebAssembly.Memory({
  initial: 4000,
  maximum: 65536,
  shared: true,
})

const __wasmFile = await fetch(__wasmUrl).then((res) => res.arrayBuffer())

const {
  instance: __napiInstance,
  module: __wasiModule,
  napiModule: __napiModule,
} = __emnapiInstantiateNapiModuleSync(__wasmFile, {
  context: __emnapiContext,
  asyncWorkPoolSize: 4,
  wasi: __wasi,
  onCreateWorker() {
    const worker = new Worker(new URL('./wasi-worker-browser.mjs', import.meta.url), {
      type: 'module',
    })


    return worker
  },
  overwriteImports(importObject) {
    importObject.env = {
      ...importObject.env,
      ...importObject.napi,
      ...importObject.emnapi,
      memory: __sharedMemory,
    }
    return importObject
  },
  beforeInit({ instance }) {
    for (const name of Object.keys(instance.exports)) {
      if (name.startsWith('__napi_register__')) {
        instance.exports[name]()
      }
    }
  },
})
export default __napiModule.exports
export const ConstExpr = __napiModule.exports.ConstExpr
export const ModuleConfig = __napiModule.exports.ModuleConfig
export const WasmCustomSections = __napiModule.exports.WasmCustomSections
export const WasmData = __napiModule.exports.WasmData
export const WasmDataSegments = __napiModule.exports.WasmDataSegments
export const WasmElement = __napiModule.exports.WasmElement
export const WasmElements = __napiModule.exports.WasmElements
export const WasmFunction = __napiModule.exports.WasmFunction
export const WasmFunctions = __napiModule.exports.WasmFunctions
export const WasmGlobal = __napiModule.exports.WasmGlobal
export const WasmGlobals = __napiModule.exports.WasmGlobals
export const WasmLocal = __napiModule.exports.WasmLocal
export const WasmLocals = __napiModule.exports.WasmLocals
export const WasmMemories = __napiModule.exports.WasmMemories
export const WasmMemory = __napiModule.exports.WasmMemory
export const WasmModule = __napiModule.exports.WasmModule
export const WasmProducers = __napiModule.exports.WasmProducers
export const WasmTable = __napiModule.exports.WasmTable
export const WasmTables = __napiModule.exports.WasmTables
export const WasmTag = __napiModule.exports.WasmTag
export const WasmTags = __napiModule.exports.WasmTags
export const WasmType = __napiModule.exports.WasmType
export const WasmTypes = __napiModule.exports.WasmTypes
export const AbstractHeapType = __napiModule.exports.AbstractHeapType
export const ConstExprKind = __napiModule.exports.ConstExprKind
export const DataKindTag = __napiModule.exports.DataKindTag
export const ElementItemsTag = __napiModule.exports.ElementItemsTag
export const ElementKindTag = __napiModule.exports.ElementKindTag
export const FunctionKindTag = __napiModule.exports.FunctionKindTag
export const GlobalKind = __napiModule.exports.GlobalKind
export const TagKindTag = __napiModule.exports.TagKindTag
export const TypeKind = __napiModule.exports.TypeKind
