// Typed failures crossing the browser host boundary.
//
// Five closed families:
// * `ScxmlHostError`: the host rejected an argument or lost its instance
//   before or outside a machine call (closed `code`, see HOST_ERROR_CODES);
// * `ScxmlSessionError`: the machine component returned a closed WIT
//   `component-error` (optionally from a `session-failure`);
// * `ScxmlCompileError`: the compiler component rejected a request;
// * `ScxmlSnapshotError`: a session snapshot or snapshot diff failed with
//   one of the `scxml-snapshot` crate's closed codes;
// * `CapabilityError`: thrown by page-supplied capability providers to
//   return one closed WIT `error-code` to the machine.
//
// Messages and details are static text; caller payloads, expressions, IR,
// and identifiers are never reflected into them.

/** Closed WIT `error-code` values (stateforward:scxml/types@0.2.0). */
export const ERROR_CODES = Object.freeze([
  'invalid-input',
  'duplicate-id',
  'unknown-id',
  'capacity-exhausted',
  'payload-too-large',
  'unsupported-version',
  'protocol',
  'execution',
  'unavailable',
  'platform',
]);

/** Closed host-side failure codes. */
export const HOST_ERROR_CODES = Object.freeze([
  // `ir` exceeds MAX_MACHINE_IR_BYTES; nothing was instantiated.
  'machine-ir-too-large',
  // A createSession/compile argument has the wrong type or shape.
  'invalid-argument',
  // A component could not be instantiated or trapped; the instance is
  // discarded and every later call fails with `instance-lost`.
  'instance-lost',
  // A call exhausted its deterministic fuel budget (src/fuel.js); the
  // instance is discarded and every later call fails with `instance-lost`.
  'fuel-exhausted',
  // A compiler result failed its report integrity check.
  'report-mismatch',
  // A host-linked capability component (the session's SCXML I/O processor
  // or its clock) broke its contract; the session is never entered again.
  'host-invariant',
  // A session method was called while another session operation was in
  // progress (a capability provider or data model calling back into the
  // session); nothing was entered and the session stays usable.
  'reentrant-call',
  // An extension component's bytes do not hash to the trusted SHA-256
  // (authenticateExtensionComponent).
  'artifact-digest-mismatch',
  // A component is not an extension component this package ships built
  // (authenticateExtensionComponent, ExtensionComponents.bind).
  'artifact-kind-mismatch',
  // An extension package binding was refused (ExtensionComponents.bind).
  'namespace-binding',
  // A bound extension component does not implement its namespace URI; the
  // session start fails.
  'extension-component-uri-mismatch',
  // A bound extension component's `initialize` failed; the session start
  // fails.
  'extension-component-initialize-failed',
  // A bound service component imports an interface the host never links
  // (ServiceComponents; createSession fails).
  'unknown-import',
  // A bound service component imports a WASI resource the run does not
  // grant, or one a browser cannot grant (`wasi:sockets`).
  'wasi-resource-ungranted',
  // A WASI grant names no bound service, or a resource its component does
  // not import.
  'wasi-grant-undeclared',
]);

const ERROR_CODE_SET = new Set(ERROR_CODES);

export class ScxmlHostError extends Error {
  /**
   * @param {string} code one of HOST_ERROR_CODES
   * @param {{cause?: unknown}} [options]
   */
  constructor(code, options) {
    super(`scxml browser host: ${code}`, options);
    this.name = 'ScxmlHostError';
    this.code = code;
  }
}

export class ScxmlSessionError extends Error {
  /**
   * @param {{code: string, detail?: string}} error closed component-error
   * @param {{committedEffects?: number, sessionFailed?: boolean}} [failure]
   */
  constructor(error, failure = {}) {
    super(`scxml machine: ${error.code}`);
    this.name = 'ScxmlSessionError';
    this.code = error.code;
    this.detail = error.detail;
    this.committedEffects = failure.committedEffects ?? 0;
    this.sessionFailed = failure.sessionFailed ?? false;
  }
}

/** Closed compiler `error-code` values (stateforward:scxml-compiler/types@0.2.0). */
export const COMPILER_ERROR_CODES = Object.freeze([
  'invalid-request',
  'invalid-package',
  'unsupported-artifact',
  'canonical-report-failed',
  'invalid-template',
  'packaging-failed',
]);

export class ScxmlCompileError extends Error {
  /** @param {string} code one of COMPILER_ERROR_CODES */
  constructor(code) {
    super(`scxml compiler: ${code}`);
    this.name = 'ScxmlCompileError';
    this.code = code;
  }
}

/**
 * Closed session snapshot failure codes: the `scxml-snapshot` crate's
 * `SnapshotErrorCode` wire values (crates/scxml-snapshot/src/lib.rs).
 */
export const SNAPSHOT_ERROR_CODES = Object.freeze([
  'too-large',
  'malformed-json',
  'unsupported-version',
  'schema-violation',
  'non-canonical',
  'integrity-mismatch',
  'bounds-exceeded',
  'invalid-name',
  'inconsistent',
  'depth-exceeded',
  'clock-invariant',
  'not-stable',
  'status-invalid',
  'codec-unavailable',
]);

const SNAPSHOT_ERROR_CODE_SET = new Set(SNAPSHOT_ERROR_CODES);

export class ScxmlSnapshotError extends Error {
  /**
   * @param {string} code one of SNAPSHOT_ERROR_CODES
   * @param {{cause?: unknown}} [options]
   */
  constructor(code, options = {}) {
    super(`scxml snapshot: ${code}`, options.cause === undefined ? undefined : { cause: options.cause });
    this.name = 'ScxmlSnapshotError';
    this.code = code;
  }
}

/**
 * Maps a codec `component-error` detail (the static `snapshot-<code>`; the
 * codec sends no pointer, name, or payload across the boundary) to its
 * closed code; `undefined` for any other detail.
 */
export function snapshotDetail(detail) {
  if (typeof detail !== 'string' || !detail.startsWith('snapshot-')) return undefined;
  const code = detail.slice('snapshot-'.length);
  return SNAPSHOT_ERROR_CODE_SET.has(code) ? code : undefined;
}

export class CapabilityError extends Error {
  /** @param {string} code one of ERROR_CODES */
  constructor(code) {
    if (!ERROR_CODE_SET.has(code)) throw new TypeError('CapabilityError code must be a WIT error-code');
    super(`capability failure: ${code}`);
    this.name = 'CapabilityError';
    this.code = code;
  }
}

/** Static details the host attaches; never caller-derived text. */
const HOST_DETAIL = Object.freeze({
  'invalid-input': 'capability rejected input',
  execution: 'capability evaluation failed',
  unavailable: 'capability is not provided by this host',
  platform: 'capability provider failed',
});

/**
 * Maps anything a provider threw to one closed WIT component-error.
 * Unknown throwables (bugs, foreign exceptions) become `platform`.
 */
export function toComponentError(thrown) {
  const code = thrown instanceof CapabilityError ? thrown.code : 'platform';
  return { version: 'v1', code, detail: HOST_DETAIL[code] };
}

/**
 * The value jco expects an import to throw for a WIT `result` error: an
 * object carrying the payload as an own `payload` property.
 */
export class ImportFailure {
  constructor(payload) {
    this.payload = payload;
  }
}

/** Reads a closed component-error from a jco export rejection, if any. */
export function exportPayload(thrown) {
  if (thrown === null || typeof thrown !== 'object' || !Object.hasOwn(thrown, 'payload')) {
    return undefined;
  }
  return thrown.payload;
}
