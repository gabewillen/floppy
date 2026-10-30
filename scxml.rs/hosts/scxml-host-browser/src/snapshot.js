// Read-only session snapshots (docs/session-snapshot.md).
//
// A snapshot is the canonical JSON document `scxml.session-snapshot/v2`.
// For the same run it is byte-identical to the Wasmtime host's, except for
// two documented host differences (docs/session-snapshot.md, "Parity
// fixtures"), which also change `sha256`. First, without the `http` option
// `system.ioprocessors` has no BasicHTTP entries (the host advertises the
// BasicHTTP processor only when the page configures it; with each session's
// Wasmtime location it matches byte for byte). Second, under `preserve` the
// events' `trace_context` carriers come from the page's telemetry provider.
// The browser host
// does not encode it itself: it gathers the same per-component records the
// Wasmtime host gathers (the machine's, data model's, and I/O processor's
// read-only `snapshot` exports, the clock position, and each session's
// external event queue)
// into one WIT `capture` and hands it to the optional codec component
// (`dist/snapshot-codec/`, the `scxml-snapshot` crate's `encode` and
// `diff`), so both hosts run the same encoder on the same records.
//
// The codec is loaded on first use through the session's core-module loader
// and cached per loader. It is capability-free: it imports the telemetry
// facade, whose calls this host forwards, plus the `types`, `machine-api`,
// `data-model-api`, and `io-processor-api` interfaces for their record types
// only (codecImports supplies them empty). Each call is a host call of its
// own on its own deterministic fuel budget (DEFAULT_SNAPSHOT_FUEL), not the
// session's: the codec is shared by every session of a loader and reaches
// no session component (as the Wasmtime host runs it). A missing codec, a session created
// with `snapshot: 'none'`, or a codec that answers `unavailable` fails with
// ScxmlSnapshotError('codec-unavailable'); nothing else changes.
//
// Snapshot documents may hold trace carriers (`traceContext: 'preserve'`)
// and data-model values: never log or index them.

import { ScxmlHostError, ScxmlSnapshotError, exportPayload, snapshotDetail } from './errors.js';
import { FuelBudget, FuelMeter } from './fuel.js';
import { fetchCoreModules } from './loader.js';
import { GUEST_MODULES, Telemetry, isTelemetryOption, scxmlTelemetryImport } from './telemetry.js';

/** Sections `snapshot({exclude})` can omit, in the codec's WIT enum order. */
export const SNAPSHOT_SECTIONS = Object.freeze([
  'system',
  'data-model',
  'queues',
  'pending-sends',
  'transitions',
  'previews',
  'invocations',
]);
/** `createSession({snapshot})` values: load the codec on first use, or never. */
export const SNAPSHOT_MODES = Object.freeze(['lazy', 'none']);
/** `snapshot({traceContext})` values. */
export const TRACE_CONTEXT_POLICIES = Object.freeze(['strip', 'preserve']);
/**
 * Per-call fuel budget of the codec component. Encoding is linear in the
 * document size.
 */
export const DEFAULT_SNAPSHOT_FUEL = 4_000_000_000;

/** `data-model-api.name` -> the codec's `component-role`. */
const DATA_MODEL_ROLES = Object.freeze({
  ecmascript: 'data-model-ecmascript',
  null: 'data-model-null',
  xpath: 'data-model-xpath',
});

/**
 * Validated snapshot options: `{exclude: Set<section>, traceContext}`, or
 * `undefined` when malformed. Duplicates are accepted and collapse.
 */
export function snapshotOptions(options) {
  if (options === undefined) return { exclude: new Set(), traceContext: 'strip' };
  if (options === null || typeof options !== 'object') return undefined;
  const { exclude = [], traceContext = 'strip' } = options;
  if (!Array.isArray(exclude) || !exclude.every((section) => SNAPSHOT_SECTIONS.includes(section))) {
    return undefined;
  }
  if (!TRACE_CONTEXT_POLICIES.includes(traceContext)) return undefined;
  return { exclude: new Set(exclude), traceContext };
}

/**
 * The codec's WIT `snapshot-options` for validated options: a checkpoint
 * document, or (`point` 'executable-content') the document an extension
 * component reads during `execute`.
 */
export function witSnapshotOptions({ exclude, traceContext, point = 'checkpoint' }) {
  return { exclude: SNAPSHOT_SECTIONS.filter((section) => exclude.has(section)), traceContext, point };
}

/**
 * What a capture gathers under validated options: excluded data is never
 * collected (docs/session-snapshot.md, "`options`").
 */
export function collection({ exclude }) {
  const transitions = !exclude.has('transitions');
  return {
    machine: { transitions, previews: transitions && !exclude.has('previews') },
    dataModel: !(exclude.has('system') && exclude.has('data-model')),
    pendingSends: !exclude.has('pending-sends'),
    queues: !exclude.has('queues'),
    children: !exclude.has('invocations'),
  };
}

let releaseDigests;

/** Release-artifact digests from `dist/identity.js` (written by scripts/build.mjs). */
async function loadReleaseDigests() {
  releaseDigests ??= import('../dist/identity.js').then(
    (module) => module.RELEASE_DIGESTS,
    () => undefined,
  );
  return releaseDigests;
}

/**
 * The capture's `components`: clock, the data models linked in the root session and its invoked sessions,
 * the SCXML I/O processor, and the machine, sorted by role, each with its
 * release-artifact SHA-256 (docs/session-snapshot.md, "`snapshot`").
 * @param {Iterable<string>} dataModels data-model names of the captured sessions (each
 *   has a role: createSession admits no other name)
 */
function componentIdentities(digests, dataModels) {
  const entries = [
    ['clock', 'clock'],
    ['io-processor-scxml', 'ioprocessor-scxml'],
    ['machine', 'machine'],
  ];
  for (const name of new Set(dataModels)) entries.push([DATA_MODEL_ROLES[name], `data-model-${name}`]);
  return entries
    .map(([role, component]) => ({ role, sha256: digests[component] }))
    .sort((a, b) => (a.role < b.role ? -1 : a.role > b.role ? 1 : 0));
}

/** Whether `name` is a data-model name with a codec role. */
export function hasDataModelRole(name) {
  return typeof name === 'string' && Object.hasOwn(DATA_MODEL_ROLES, name);
}

/**
 * One instantiated codec component. Its telemetry import forwards to the
 * adapter of the call in progress, so one cached instance serves every
 * session that shares a loader.
 */
class Codec {
  #api;
  #meter;
  #budget = new FuelBudget(DEFAULT_SNAPSHOT_FUEL);
  #telemetry;
  #lost = false;
  digests;

  constructor(api, meter, telemetry, digests) {
    this.#api = api;
    this.#meter = meter;
    this.#telemetry = telemetry;
    this.digests = digests;
  }

  get lost() {
    return this.#lost;
  }

  /**
   * Runs one codec export for `telemetry`. Returns the bytes; throws
   * ScxmlSnapshotError for a codec rejection, and ScxmlHostError for a trap
   * (the instance is then discarded).
   */
  call(telemetry, body) {
    if (this.#lost) throw new ScxmlSnapshotError('codec-unavailable');
    this.#telemetry.current = scxmlTelemetryImport(telemetry, GUEST_MODULES.snapshotCodec);
    let bytes;
    try {
      bytes = this.#budget.run(this.#meter, () => body(this.#api));
    } catch (thrown) {
      const payload = exportPayload(thrown);
      if (payload === undefined) {
        this.#lost = true;
        throw new ScxmlHostError(this.#meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', { cause: thrown });
      }
      if (payload.code === 'unavailable') throw new ScxmlSnapshotError('codec-unavailable');
      const code = snapshotDetail(payload.detail);
      if (code === undefined) throw new ScxmlHostError('host-invariant');
      throw new ScxmlSnapshotError(code);
    }
    if (!(bytes instanceof Uint8Array)) throw new ScxmlHostError('host-invariant');
    return bytes;
  }
}

/** The codec's imports: telemetry (forwarded per call) and the types-only interfaces it uses. */
function codecImports() {
  const forward = { current: undefined };
  const telemetry = {
    start: (...args) => forward.current.start(...args),
    end: (...args) => forward.current.end(...args),
    observe: (...args) => forward.current.observe(...args),
    event: (...args) => forward.current.event(...args),
  };
  const imports = {};
  for (const [name, value] of [
    ['types', {}],
    ['telemetry', telemetry],
    ['machine-api', {}],
    ['data-model-api', {}],
    ['io-processor-api', {}],
  ]) {
    imports[`stateforward:scxml/${name}`] = value;
    imports[`stateforward:scxml/${name}@0.2.0`] = value;
  }
  return { imports, forward };
}

/** Codec per loader: `loader -> Promise<Codec | undefined>` (undefined: unavailable). */
const codecs = new WeakMap();
let defaultLoader;

/**
 * The cached codec for `loader`, instantiated on first use; `undefined` when
 * the codec is not shipped or does not instantiate. A discarded (trapped)
 * instance is replaced on the next call.
 * @param {Function} [loader] `(component, file) => Module`; defaults to this
 *   package's `dist/`
 */
export async function loadCodec(loader) {
  const key = loader ?? (defaultLoader ??= fetchCoreModules());
  let pending = codecs.get(key);
  if (pending !== undefined) {
    const cached = await pending;
    if (cached === undefined || !cached.lost) return cached;
  }
  pending = instantiateCodec(key);
  codecs.set(key, pending);
  return pending;
}

async function instantiateCodec(loader) {
  try {
    const [bindings, digests] = await Promise.all([
      import('../dist/snapshot-codec/snapshot-codec.js'),
      loadReleaseDigests(),
    ]);
    if (digests === undefined) return undefined;
    const { imports, forward } = codecImports();
    const meter = new FuelMeter(DEFAULT_SNAPSHOT_FUEL);
    // Instantiation spans go nowhere until a call installs its adapter.
    forward.current = scxmlTelemetryImport(new Telemetry(), GUEST_MODULES.snapshotCodec);
    const root = await bindings.instantiate((file) => loader('snapshot-codec', file), imports, meter.instantiateCore);
    return new Codec(root.sessionSnapshotApi, meter, forward, digests);
  } catch {
    return undefined;
  }
}

/**
 * Encodes one gathered capture: `sessions` is the invocation-order list of
 * `{parent, sessionId, irSha256, dataModelName, machine, dataModel,
 * pendingSends, external, finished}`
 * (WIT `session-capture` in jco shape).
 */
export function encodeCapture(codec, telemetry, { nowMs, sessions, dataModels }, options) {
  const capture = {
    components: componentIdentities(codec.digests, dataModels),
    nowMs: BigInt(nowMs),
    sessions,
  };
  return codec.call(telemetry, (api) => api.encode(capture, witSnapshotOptions(options)));
}

/**
 * Canonical `scxml.session-snapshot-diff/v2` bytes of two valid snapshot
 * documents, through the codec component.
 *
 * @param {Uint8Array} before
 * @param {Uint8Array} after
 * @param {object} [options]
 * @param {boolean} [options.includeTraceContext] diff trace carriers too
 *   (default false)
 * @param {Function} [options.loadCoreModule] `(component, file) => Module`
 *   loader; defaults to fetching this package's `dist/`
 * @param {object} [options.telemetry] OpenTelemetry providers
 * @returns {Promise<Uint8Array>}
 * @throws {ScxmlHostError} `invalid-argument` for malformed arguments,
 *   `fuel-exhausted` or `instance-lost` when the codec traps, and
 *   `host-invariant` for an unknown codec error detail or a non-bytes result
 * @throws {ScxmlSnapshotError} `codec-unavailable`, `too-large`, or the
 *   decoder's code for an invalid document
 */
export async function diffSnapshots(before, after, options) {
  const { includeTraceContext = false, loadCoreModule, telemetry: telemetryOption } = options ?? {};
  const telemetry = new Telemetry(telemetryOption);
  if (
    !(before instanceof Uint8Array) ||
    !(after instanceof Uint8Array) ||
    (options !== undefined && (options === null || typeof options !== 'object')) ||
    typeof includeTraceContext !== 'boolean' ||
    (loadCoreModule !== undefined && typeof loadCoreModule !== 'function') ||
    !isTelemetryOption(telemetryOption)
  ) {
    telemetry.observe('snapshot-diff', 'rejected');
    throw new ScxmlHostError('invalid-argument');
  }
  const span = telemetry.start('snapshot-diff');
  try {
    const codec = await loadCodec(loadCoreModule);
    if (codec === undefined) throw new ScxmlSnapshotError('codec-unavailable');
    const bytes = telemetry.within(span, () =>
      codec.call(telemetry, (api) => api.diff(before.slice(), after.slice(), includeTraceContext)),
    );
    telemetry.end(span, 'ok');
    return bytes;
  } catch (error) {
    telemetry.end(span, error instanceof ScxmlSnapshotError ? 'rejected' : 'failed');
    throw error;
  }
}

/** Largest WIT `u64` (jco lowers it from a BigInt). */
const MAX_U64 = (1n << 64n) - 1n;

/**
 * True when `value` is a jco WIT `variable-value`: a portable `value`, the
 * type (`kind`) and string form (`text`) of a value with no portable form,
 * or the byte size of an `oversized` one (a variable over the per-value
 * bound, whose content the data model does not dump).
 */
function isVariableValue(value, isDataValue) {
  if (value === null || typeof value !== 'object') return false;
  switch (value.tag) {
    case 'value':
      return isDataValue(value.val);
    case 'text':
      return (
        value.val !== null &&
        typeof value.val === 'object' &&
        typeof value.val.kind === 'string' &&
        typeof value.val.text === 'string'
      );
    case 'oversized':
      return typeof value.val === 'bigint' && value.val >= 0n && value.val <= MAX_U64;
    default:
      return false;
  }
}

/**
 * True when `value` is a jco WIT `data-model-snapshot` (every provider's
 * `snapshot()` result is checked before it reaches the codec).
 */
export function isDataModelSnapshot(value, isDataValue) {
  if (value === null || typeof value !== 'object') return false;
  const { system, variables } = value;
  return (
    system !== null &&
    typeof system === 'object' &&
    typeof system.sessionId === 'string' &&
    (system.machineName === undefined || typeof system.machineName === 'string') &&
    isDataValue(system.ioProcessors) &&
    Array.isArray(variables) &&
    variables.every(
      (variable) =>
        variable !== null &&
        typeof variable === 'object' &&
        typeof variable.name === 'string' &&
        isVariableValue(variable.value, isDataValue),
    )
  );
}

/** `event` without its trace carrier. */
const bare = (event) => ({ ...event, traceContext: undefined });

/**
 * A `session-capture` with every event's trace carrier removed (the
 * `strip` policy, applied before encoding exactly as the Wasmtime host
 * applies it). The input is not modified.
 */
export function stripTraceContext(session) {
  return {
    ...session,
    machine: { ...session.machine, internal: session.machine.internal.map(bare) },
    pendingSends: session.pendingSends.map((pending) => ({
      ...pending,
      request:
        pending.request.tag === 'scxml'
          ? { tag: 'scxml', val: { ...pending.request.val, event: bare(pending.request.val.event) } }
          : { tag: pending.request.tag, val: bare(pending.request.val) },
    })),
    external: session.external.map(bare),
  };
}
