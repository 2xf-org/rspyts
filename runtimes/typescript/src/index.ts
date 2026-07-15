/** Application-facing rspyts ABI-3 runtime surface. */

export {
  ABI_VERSION,
  ContractFingerprintMismatchError,
  instantiate,
  type BridgeModule,
  type InstantiateOptions,
} from "./module.js";
export {
  InstancePoisonedError,
  RspytsError,
  RspytsPanicError,
  StaleHandleError,
} from "./errors.js";
