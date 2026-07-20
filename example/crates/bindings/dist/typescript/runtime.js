import initializeNative, * as native from "./native.js";

const wasmUrl = new URL("./native_bg.wasm", import.meta.url);
let wasmInput = wasmUrl;
if (wasmUrl.protocol === "file:" && globalThis.process?.versions?.node) {
  const nodeModule = "node:fs/promises";
  const { readFile } = await import(nodeModule);
  wasmInput = await readFile(wasmUrl);
}
await initializeNative({ module_or_path: wasmInput });

export { native };

export function prepareHost(value) {
  if (value instanceof Date) return value.toISOString();
  if (ArrayBuffer.isView(value)) return value;
  if (Array.isArray(value)) return value.map(prepareHost);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item)]));
  }
  return value;
}

const bufferConstructors = {
  u8: Uint8Array, i8: Int8Array, u16: Uint16Array, i16: Int16Array,
  u32: Uint32Array, i32: Int32Array, u64: BigUint64Array, i64: BigInt64Array,
  f32: Float32Array, f64: Float64Array,
};

export function restoreHost(value, spec) {
  if (value == null || spec == null) return value;
  const [kind, detail, variants] = spec;
  if (kind === "bytes") return new Uint8Array(value);
  if (kind === "buffer") return new bufferConstructors[detail](value);
  if (kind === "list") return Array.from(value, item => restoreHost(item, detail));
  if (kind === "map") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail)]));
  if (kind === "tuple") return value.map((item, index) => restoreHost(item, detail[index]));
  if (kind === "named") return restoreHost(value, nativeSchemas[detail]);
  if (kind === "alias") return restoreHost(value, detail);
  if (kind === "struct") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail[key])]));
  if (kind === "tagged") {
    const fields = variants[value[detail]] ?? {};
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, fields[key])]));
  }
  return value;
}

export function nativeError(error, ErrorType) {
  const text = String(error);
  const line = text.indexOf("\n");
  return line < 0 ? error : new ErrorType(text.slice(0, line), text.slice(line + 1));
}

const nativeSchemas = {
  "example-dice::example_dice::fair::roll::RollRequest": ["struct", {sides: null, count: null}],
  "example-dice::example_dice::fair::roll::RollResult": ["struct", {values: ["list", null], total: null}],
  "example-dice::example_dice::loaded::roll::RollResult": ["struct", {value: null, favoredValue: null}],
  "example-dice::example_dice::summary::RollSummary": ["struct", {label: null, result: ["named", "example-dice::example_dice::fair::roll::RollResult"]}],
};
