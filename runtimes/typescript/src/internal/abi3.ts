/**
 * Versioned generator-facing ABI-3 surface.
 *
 * This subpath is emitted by `rspyts generate`; application code should use
 * generated clients and the package root instead.
 */

export {
  boolFromWire,
  boundedIntFromWire,
  bufferFromWire,
  bytesFromWire,
  callDrop,
  callFn,
  enumFromWire,
  f32FromWire,
  floatFromWire,
  i64FromWire,
  i64ToWire,
  jsonFromWire,
  listFromWire,
  mapFromWire,
  nullFromWire,
  objectFromWire,
  stringEnumFromWire,
  stringFromWire,
  tupleFromWire,
  u64FromWire,
  u64ToWire,
  wireResponse,
  type SliceArg,
  type WireResponse,
  type WireVariantShape,
} from "../call.js";
export { jsonToWire, wireBuffer } from "../envelope.js";
export {
  RspytsError,
  type BridgeErrorRegistry,
} from "../errors.js";
export {
  type BridgeModule,
  verifyModuleContract,
} from "../module.js";
