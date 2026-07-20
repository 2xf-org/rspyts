/* tslint:disable */
/* eslint-disable */

export class RspytsWasmDiceCup {
    free(): void;
    [Symbol.dispose](): void;
    close(): void;
    constructor(sides: any, seed: any);
    roll(count: any): any;
}

export function __rspyts_export_rollDice(request: any, seed: any): any;

export function __rspyts_export_rollValues(request: any, seed: any): any;

export function __rspyts_export_seedFromBytes(bytes: any): any;

export function rspyts_contract_json(): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly rspyts_contract_json: () => [number, number];
    readonly __rspyts_export_seedFromBytes: (a: any) => [number, number, number];
    readonly __rspyts_export_rollValues: (a: any, b: any) => [number, number, number];
    readonly __rspyts_export_rollDice: (a: any, b: any) => [number, number, number];
    readonly rspytswasmdicecup_new: (a: any, b: any) => [number, number, number];
    readonly rspytswasmdicecup_roll: (a: number, b: any) => [number, number, number];
    readonly rspytswasmdicecup_close: (a: number) => void;
    readonly __wbg_rspytswasmdicecup_free: (a: number, b: number) => void;
    readonly __wbindgen_malloc_command_export: (a: number, b: number) => number;
    readonly __wbindgen_realloc_command_export: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store_command_export: (a: number) => void;
    readonly __externref_table_alloc_command_export: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc_command_export: (a: number) => void;
    readonly __wbindgen_free_command_export: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
