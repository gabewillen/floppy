// Public entry point of the SCXML browser host.
//
// Browsers run machines in runtime-IR mode only: the generic machine
// component (transpiled once at build time) receives lowered Machine IR as
// data through `ir-source`. Per-machine packaged components are never used
// in the browser.
export {
  DEFAULT_SESSION_ID,
  BATCH_START_EPOCH_MS,
  MAX_DUE_DELIVERIES,
  MAX_MACHINE_IR_BYTES,
  MachineSession,
  TIMER_MODES,
  UNSUPPORTED,
  createSession,
} from './machine.js';
export { ASYNC_MECHANISMS, ASYNC_MODES, AsyncMachineSession, asyncMechanism } from './async-bridge.js';
export { createEcmaScriptDataModel, createNullDataModel, createXPathDataModel } from './datamodel.js';
export { COMPILE_STATUSES, MAX_REPORT_BYTES, SCHEMA_POLICIES, createCompiler } from './compiler.js';
export { DEFAULT_COMPILE_FUEL, DEFAULT_FUEL, FUEL_EXPORT, MAX_FUEL } from './fuel.js';
export { COMPONENTS, cachedCoreModules, fetchCoreModules } from './loader.js';
export {
  DEFAULT_SNAPSHOT_FUEL,
  MAX_SNAPSHOT_BYTES,
  SNAPSHOT_MODES,
  SNAPSHOT_SECTIONS,
  TRACE_CONTEXT_POLICIES,
  diffSnapshots,
} from './snapshot.js';
export { MAX_OPEN_SPANS } from './telemetry.js';
export { MAX_RESOURCE_URL_BYTES, RESOURCE_DETAILS } from './resources.js';
export { STATUSES } from './values.js';
export {
  COMPILER_ERROR_CODES,
  CapabilityError,
  ERROR_CODES,
  HOST_ERROR_CODES,
  SNAPSHOT_ERROR_CODES,
  ScxmlCompileError,
  ScxmlHostError,
  ScxmlSessionError,
  ScxmlSnapshotError,
} from './errors.js';
