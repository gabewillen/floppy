// Extension components (SCXML 1.0 section 4.10) in the browser host: the
// counterpart of the Wasmtime host's `ExtensionComponents`
// (hosts/scxml-host-wasmtime/src/extension.rs), with the same names,
// semantics, and ordering. The host stays a thin linker:
//
// * Binding: `ExtensionComponents.bind(package, component)` binds an exact
//   namespace URI to an extension component the page authenticated with
//   `authenticateExtensionComponent(bytes, sha256)`: the bytes must hash to
//   the trusted SHA-256, and that digest must be the release artifact of an
//   extension component this package ships transpiled (jco) and fuel-metered
//   (scripts/build.mjs, `dist/<component>/`). The checks of a binding are the
//   Wasmtime host's `check_binding`: an absolute URI of at most
//   MAX_NAMESPACE_URI_BYTES that is neither the SCXML nor the XML namespace,
//   a schema of at most MAX_NAMESPACE_SCHEMA_BYTES whose SHA-256 is the
//   package's `schemaSha256`, a URI bound once, and at most
//   MAX_EXTENSION_COMPONENTS bindings.
// * Loading: before data binding and its first macrostep, every session
//   (root and each invoked child) asks for the namespaces its document uses
//   (`machine-capabilities.load-extensions`); each bound one gets its own
//   instance for that session, synchronously from core modules compiled when
//   the session was created. The host checks the instance's `namespace-uri`
//   and calls `initialize`; an unbound namespace fails the session start
//   closed (`unavailable`). `shutdown` runs once when the session's instance
//   is dropped.
// * Execution: an element is routed by its exact namespace URI. For the
//   call, the component's `data-model-host` and `execution-host` act on the
//   executing session (its data model, SCXML Event I/O Processor, clock, and
//   configuration) exactly as the machine's own capabilities do, and only
//   during `execute` (`unavailable` outside it; `in-state` false, `now-ms`
//   0). Raised events and self-addressed sends are handed back to the
//   machine with the outcome, which queues them in document order.
// * Bounds: each call (`initialize`, `execute`, `shutdown`) runs with the
//   per-call cap (`withCallFuel`, default DEFAULT_EXTENSION_CALL_FUEL) on
//   the session's host-call budget (src/fuel.js), so its work is charged to
//   the machine call that made it. A trap (fuel exhaustion included) retires
//   the instance: later `execute` calls fail with `execution`.
// * No I/O: an extension element is executable content, so the component
//   gets no `outbound-http`, no WASI, and no `env` value or reference.
// * Session snapshot: a component that imports `session-snapshot-host`
//   makes the machine pass its executable-content image with each of its
//   elements; `snapshot(exclude)` encodes the executing session's canonical
//   document through the codec component (point `executable-content`), at
//   most MAX_EXTENSION_SNAPSHOTS times per `execute` call.
//
// Telemetry: one `extension-load` operation per `load-extensions` call,
// `extension-instantiate`, `extension-execute`, and `extension-shutdown`
// per instance call, with closed outcomes only; guest telemetry is recorded
// under the `executable-content-extension` module. Namespace URIs, element
// names, attributes, and log text never become attributes.

import {
  ImportFailure,
  ScxmlHostError,
  ScxmlSnapshotError,
  exportPayload,
  toComponentError,
} from './errors.js';
import { FuelBudget, FuelMeter, isFuel } from './fuel.js';
import { sha256Hex } from './sha256.js';
import { encodeCapture, isDataModelSnapshot, stripTraceContext } from './snapshot.js';
import { GUEST_MODULES, Telemetry, scxmlTelemetryImport } from './telemetry.js';
import { isDataValue, isWitEvent } from './values.js';

/** Default instruction fuel cap of one extension component call (the Wasmtime host's). */
export const DEFAULT_EXTENSION_CALL_FUEL = 1_000_000_000;
/** Maximum namespaces one session binds. */
export const MAX_EXTENSION_COMPONENTS = 64;
/** Maximum bytes of one bound namespace URI. */
export const MAX_NAMESPACE_URI_BYTES = 4_096;
/** Maximum bytes of one bound namespace schema. */
export const MAX_NAMESPACE_SCHEMA_BYTES = 4 * 1024 * 1024;
/** Maximum `session-snapshot-host.snapshot` calls of one `execute` call. */
export const MAX_EXTENSION_SNAPSHOTS = 64;

const SCXML_NAMESPACE = 'http://www.w3.org/2005/07/scxml';
const XML_NAMESPACE = 'http://www.w3.org/XML/1998/namespace';
const SESSION_SNAPSHOT_HOST_IMPORT = 'stateforward:scxml/session-snapshot-host';
const SCXML_PROCESSOR_TYPES = new Set(['scxml', 'http://www.w3.org/TR/scxml/#SCXMLEventProcessor']);
const encoder = new TextEncoder();

/**
 * The extension components this package ships, by `dist/` component name:
 * their jco bindings are imported only when a page binds one.
 */
const SHIPPED = Object.freeze({
  'extension-example': () => import('../dist/extension-example/extension-example.js'),
  scx: () => import('../dist/scx/scx.js'),
});

let identities;

/** `dist/identity.js`: each shipped component's release digest and imports. */
function loadIdentities() {
  identities ??= import('../dist/identity.js').then(
    (module) => module,
    () => ({ RELEASE_DIGESTS: {}, COMPONENT_IMPORTS: {} }),
  );
  return identities;
}

const unavailable = (detail) => new ImportFailure({ version: 'v1', code: 'unavailable', detail });
const componentError = (code, detail) => ({ version: 'v1', code, detail });
const outsideExecution = () => unavailable('session capabilities are available only during execution');

/**
 * Authenticates one extension component: `bytes` must hash to `sha256` (64
 * lowercase hex characters, the trust anchor), and that digest must be the
 * release artifact of an extension component this package ships built.
 * @param {Uint8Array} bytes the exact component bytes
 * @param {string} sha256 the trusted SHA-256
 * @returns {Promise<{kind: 'extension-component', sha256: string, component: string}>}
 * @throws {ScxmlHostError} `invalid-argument` for malformed arguments,
 *   `artifact-digest-mismatch` when the bytes do not hash to `sha256`,
 *   `artifact-kind-mismatch` when no shipped extension component has it
 */
export async function authenticateExtensionComponent(bytes, sha256) {
  if (!(bytes instanceof Uint8Array) || typeof sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(sha256)) {
    throw new ScxmlHostError('invalid-argument');
  }
  if (sha256Hex(bytes) !== sha256) throw new ScxmlHostError('artifact-digest-mismatch');
  const { RELEASE_DIGESTS } = await loadIdentities();
  const component = Object.keys(SHIPPED).find((name) => RELEASE_DIGESTS[name] === sha256);
  if (component === undefined) throw new ScxmlHostError('artifact-kind-mismatch');
  return Object.freeze({ kind: 'extension-component', sha256, component });
}

/** True when `value` is an absolute URI (an RFC 3986 scheme followed by `:`). */
function isAbsoluteUri(value) {
  return /^[A-Za-z][A-Za-z0-9+.-]*:\S*$/.test(value) && !/\s/.test(value);
}

/** Extension components one session links, keyed by exact namespace URI. */
export class ExtensionComponents {
  #bindings = [];
  #callFuel = DEFAULT_EXTENSION_CALL_FUEL;

  /**
   * Binds `component` to `pkg.namespaceUri` (compared byte for byte with each
   * element's namespace).
   * @param {object} pkg ExtensionPackage: `namespaceUri`, `packageVersion`,
   *   `extensionArtifact`, `schemaArtifact`, `schemaSha256`, `witWorld`,
   *   `witVersion`, and `schema` (Uint8Array)
   * @param {object} component an authenticateExtensionComponent result
   * @returns {ExtensionComponents} this
   * @throws {ScxmlHostError} `artifact-kind-mismatch` for anything but an
   *   authenticated extension component; `namespace-binding` for a binding
   *   the checks refuse
   */
  bind(pkg, component) {
    if (component?.kind !== 'extension-component' || !Object.hasOwn(SHIPPED, component.component)) {
      throw new ScxmlHostError('artifact-kind-mismatch');
    }
    const fields = ['namespaceUri', 'packageVersion', 'extensionArtifact', 'schemaArtifact', 'schemaSha256', 'witWorld', 'witVersion'];
    const uri = pkg?.namespaceUri;
    if (
      pkg === null ||
      typeof pkg !== 'object' ||
      !fields.every((field) => typeof pkg[field] === 'string') ||
      !(pkg.schema instanceof Uint8Array) ||
      encoder.encode(uri).byteLength > MAX_NAMESPACE_URI_BYTES ||
      !isAbsoluteUri(uri) ||
      uri === SCXML_NAMESPACE ||
      uri === XML_NAMESPACE ||
      pkg.schema.byteLength > MAX_NAMESPACE_SCHEMA_BYTES ||
      sha256Hex(pkg.schema) !== pkg.schemaSha256 ||
      this.#bindings.length >= MAX_EXTENSION_COMPONENTS ||
      this.#bindings.some((bound) => bound.package.namespaceUri === uri)
    ) {
      throw new ScxmlHostError('namespace-binding');
    }
    const copy = Object.freeze({ ...Object.fromEntries(fields.map((field) => [field, pkg[field]])), schema: pkg.schema.slice() });
    this.#bindings.push(Object.freeze({ package: copy, component }));
    return this;
  }

  /**
   * Sets the fuel cap of one extension component call.
   * @param {number} fuel a positive safe integer
   * @returns {ExtensionComponents} this
   * @throws {ScxmlHostError} `invalid-argument`
   */
  withCallFuel(fuel) {
    if (!isFuel(fuel)) throw new ScxmlHostError('invalid-argument');
    this.#callFuel = fuel;
    return this;
  }

  /** Whether no namespace is bound. */
  isEmpty() {
    return this.#bindings.length === 0;
  }

  /** The bindings and call fuel the session links (internal). */
  parts() {
    return { bindings: [...this.#bindings], callFuel: this.#callFuel };
  }
}

/**
 * The imports of one extension component instance. `state.call` is the
 * `execute` call in progress (`undefined` outside one), `state.links` the
 * session the instance belongs to.
 */
function extensionImports(state, telemetry) {
  const call = () => {
    if (state.call === undefined) throw outsideExecution();
    return state.call;
  };
  const dataModel = () => {
    const model = state.links.dataModel;
    if (model === undefined) throw unavailable('no data model is linked');
    return model;
  };
  const forward = (body) => {
    try {
      return body();
    } catch (thrown) {
      if (thrown instanceof ImportFailure) throw thrown;
      throw new ImportFailure(toComponentError(thrown));
    }
  };
  const dataModelHost = {
    evaluate(expression) {
      const { event } = call();
      const value = forward(() => dataModel().evaluateValue(expression, event));
      if (!isDataValue(value)) throw new ImportFailure(componentError('platform', 'data model answered no value'));
      return value;
    },
    assign(location, value) {
      const { event } = call();
      forward(() => dataModel().assign(location, { tag: 'value', val: value }, { tag: 'replace-children' }, event));
    },
  };
  const internalEvent = (event, sendId) => {
    if (!isWitEvent(event)) throw new ImportFailure(componentError('invalid-input', 'the event is invalid'));
    return {
      name: event.name,
      eventType: 'internal',
      sendId,
      origin: undefined,
      originType: undefined,
      invokeId: undefined,
      data: event.data,
      raw: undefined,
      traceContext: event.traceContext,
    };
  };
  const executionHost = {
    raise(event) {
      const current = call();
      current.internal.push(internalEvent(event, undefined));
    },
    send(request) {
      const current = call();
      // Only the SCXML Event I/O Processor's sends route here, exactly as
      // `<send>` routes them.
      if (request?.processorType !== undefined && !SCXML_PROCESSOR_TYPES.has(request.processorType)) {
        throw new ImportFailure(
          componentError('invalid-input', 'an extension component sends only through the SCXML Event I/O Processor'),
        );
      }
      const target = state.links.classifyTarget(request.target);
      switch (target.tag) {
        case 'internal':
          if (request.delayMs > 0n) {
            throw new ImportFailure(componentError('invalid-input', 'a send to #_internal cannot be delayed'));
          }
          if (request.event?.sendId !== undefined && request.sendId !== request.event.sendId) {
            throw new ImportFailure(componentError('invalid-input', "the event's send id differs from the request's"));
          }
          current.internal.push(internalEvent(request.event, request.sendId));
          return;
        case 'invalid':
          throw new ImportFailure(componentError('invalid-input', 'send target is not a valid target'));
        case 'unreachable':
          throw unavailable('no live session answers the send target');
        default: {
          const disposition = state.links.send(request);
          if (disposition.tag === 'enqueue-external') current.external.push(disposition.val);
          else if (disposition.tag === 'undeliverable') throw unavailable('no live session answers the send target');
        }
      }
    },
    cancel(sendId) {
      call();
      state.links.cancel(sendId);
    },
    log() {
      // The message is component payload and never enters telemetry.
    },
    inState(stateId) {
      return state.call !== undefined && state.links.configuration.has(stateId);
    },
    nowMs() {
      return state.call === undefined ? 0n : BigInt(state.links.nowMs());
    },
  };
  const sessionSnapshotHost = {
    snapshot(exclude) {
      const current = call();
      if (current.snapshots >= MAX_EXTENSION_SNAPSHOTS) {
        throw new ImportFailure(componentError('capacity-exhausted', 'an `execute` call takes at most 64 session snapshots'));
      }
      current.snapshots += 1;
      if (current.image === undefined || state.codec === undefined) {
        throw unavailable('the session snapshot is not available for this `execute` call');
      }
      const bytes = executableContentDocument(state, current, exclude, telemetry);
      return new TextDecoder().decode(bytes);
    },
  };
  const imports = {};
  for (const [name, value] of [
    ['types', {}],
    ['telemetry', scxmlTelemetryImport(telemetry, GUEST_MODULES.extension)],
    ['data-model-host', dataModelHost],
    ['execution-host', executionHost],
    ['session-snapshot-host', sessionSnapshotHost],
  ]) {
    imports[`stateforward:scxml/${name}`] = value;
    imports[`stateforward:scxml/${name}@0.2.0`] = value;
  }
  return imports;
}

/**
 * The executing session's canonical `executable-content` document (the
 * Wasmtime host's `extension_document`): the machine's image with the
 * events this call queued so far, the data model, pending sends, and
 * external queue read at this call; never invoked sessions or previews.
 */
function executableContentDocument(state, current, exclude, telemetry) {
  const { links } = state;
  const sections = new Set([...exclude, 'invocations', 'previews']);
  const image = current.image;
  const machine = {
    ...image,
    transitions: sections.has('transitions')
      ? []
      : image.transitions.map((transition) => ({ ...transition, exit: [], entry: [] })),
    internal: [...image.internal, ...current.internal],
  };
  let dataModel;
  let dataModelName;
  if (links.dataModel !== undefined && links.dataModelInitialized) {
    dataModelName = links.dataModelName();
    if (!(sections.has('system') && sections.has('data-model')) && machine.status !== 'ready') {
      let value;
      try {
        value = links.dataModel.snapshot();
      } catch {
        throw new ImportFailure(componentError('execution', 'the session snapshot could not be read'));
      }
      if (!isDataModelSnapshot(value, isDataValue)) {
        throw new ImportFailure(componentError('execution', 'the session snapshot could not be read'));
      }
      dataModel = value;
    }
  }
  let pendingSends = [];
  let nowMs;
  try {
    nowMs = links.snapshotNow();
    if (!sections.has('pending-sends')) pendingSends = links.pendingSends();
  } catch {
    throw new ImportFailure(componentError('platform', 'the host failed to read the session'));
  }
  const external = sections.has('queues') ? [] : [...links.externalQueue(), ...current.external];
  const session = stripTraceContext({
    parent: undefined,
    sessionId: links.sessionId,
    irSha256: links.irSha256(),
    dataModelName,
    machine,
    dataModel,
    pendingSends,
    external,
    finished: false,
  });
  const options = { exclude: sections, traceContext: 'strip', point: 'executable-content' };
  try {
    return encodeCapture(
      state.codec,
      telemetry,
      { nowMs, sessions: [session], dataModels: dataModelName === undefined ? [] : [dataModelName] },
      options,
    );
  } catch (error) {
    if (error instanceof ScxmlSnapshotError) {
      if (['bounds-exceeded', 'too-large', 'depth-exceeded'].includes(error.code)) {
        throw new ImportFailure(componentError('payload-too-large', 'the session snapshot exceeds a bound of its format'));
      }
      if (error.code === 'codec-unavailable') throw unavailable('the session snapshot is not available');
      throw new ImportFailure(componentError('execution', 'the session snapshot could not be read'));
    }
    throw new ImportFailure(componentError('platform', 'the host failed to read the session'));
  }
}

/**
 * Keeps the contract's codes an extension component may report, with static
 * details; any other code is `protocol` (the Wasmtime host's `sanitize`).
 */
function sanitize(payload) {
  const details = {
    'invalid-input': 'extension component rejected its input',
    execution: 'extension component execution failed',
    unavailable: 'extension component reported a capability unavailable',
    'capacity-exhausted': 'extension component exhausted a capacity bound',
    'payload-too-large': 'extension component exceeded a byte bound',
    platform: 'extension component reported a platform failure',
  };
  if (payload?.version !== 'v1' || !Object.hasOwn(details, payload.code)) {
    return componentError('protocol', 'extension component broke the extension component error contract');
  }
  return componentError(payload.code, details[payload.code]);
}

/**
 * One bound namespace, prepared for a session: its jco bindings, its core
 * modules compiled (so every session instantiates it synchronously), and
 * whether it reads the session snapshot.
 */
async function prepare(binding, loader, callFuel) {
  const { component } = binding.component;
  const [bindings, { COMPONENT_IMPORTS }] = await Promise.all([SHIPPED[component](), loadIdentities()]);
  const compiled = new Map();
  const recording = (file) =>
    Promise.resolve(loader(component, file)).then((module) => {
      compiled.set(file, module);
      return module;
    });
  // One instantiation compiles and records every core module.
  const state = { call: undefined, links: undefined, codec: undefined };
  const meter = new FuelMeter(callFuel);
  await new FuelBudget(callFuel).hold(() =>
    bindings.instantiate(recording, extensionImports(state, new Telemetry()), meter.instantiateCore),
  );
  return {
    package: binding.package,
    bindings,
    module: (file) => {
      const found = compiled.get(file);
      if (found === undefined) throw new TypeError('core module was not preloaded');
      return found;
    },
    readsSnapshot: (COMPONENT_IMPORTS?.[component] ?? []).includes(SESSION_SNAPSHOT_HOST_IMPORT),
  };
}

/**
 * Prepares every binding of `extensions` (an ExtensionComponents) for a
 * session; `undefined` when nothing is bound.
 */
export async function prepareExtensions(extensions, loader) {
  if (extensions === undefined || extensions.isEmpty()) return undefined;
  const { bindings, callFuel } = extensions.parts();
  const prepared = new Map();
  for (const binding of bindings) prepared.set(binding.package.namespaceUri, await prepare(binding, loader, callFuel));
  return { prepared, callFuel, packages: bindings.map((binding) => binding.package) };
}

/** Whether `value` is an ExtensionComponents (createSession's option). */
export function isExtensionComponents(value) {
  return value === undefined || value instanceof ExtensionComponents;
}

/** One extension component instance of one session. */
class ExtensionInstance {
  #api;
  #meter;
  #state;
  #run;
  #lifecycle = 'live';

  constructor(api, meter, state, run, readsSnapshot) {
    this.#api = api;
    this.#meter = meter;
    this.#state = state;
    this.#run = run;
    this.readsSnapshot = readsSnapshot;
  }

  /** Runs one component call on the session's budget: `{value}` or `{payload}` or `{trap}`. */
  #call(body) {
    try {
      return { value: this.#run.budget.run(this.#meter, () => body(this.#api)) };
    } catch (thrown) {
      const payload = exportPayload(thrown);
      return payload === undefined ? { trap: thrown } : { payload };
    }
  }

  initialize() {
    return this.#call((api) => api.initialize());
  }

  namespaceUri() {
    return this.#call((api) => api.namespaceUri());
  }

  /** `machine-capabilities.execute-extension` for one element. */
  execute(element, event, context, image) {
    if (this.#lifecycle !== 'live') {
      return {
        outcome: { tag: 'err', val: componentError('execution', 'extension component was retired after a trap') },
        internal: [],
        external: [],
      };
    }
    const current = { event, internal: [], external: [], snapshots: 0, image };
    this.#state.call = current;
    let result;
    try {
      result = this.#call((api) => api.execute(element, context));
    } finally {
      this.#state.call = undefined;
    }
    let outcome;
    if (result.trap !== undefined) {
      this.#lifecycle = 'retired';
      outcome = { tag: 'err', val: componentError('execution', 'extension component trapped or exhausted its fuel') };
    } else if (result.payload !== undefined) {
      outcome = { tag: 'err', val: sanitize(result.payload) };
    } else {
      outcome = { tag: 'ok', val: result.value };
    }
    return { outcome, internal: current.internal, external: current.external };
  }

  /** Calls `shutdown` once; a live instance only. */
  shutdown() {
    if (this.#lifecycle !== 'live') return;
    this.#lifecycle = 'shut-down';
    const telemetry = this.#run.telemetry;
    const span = telemetry.start('extension-shutdown');
    const result = this.#call((api) => api.shutdown());
    telemetry.end(span, result.value !== undefined || (result.payload === undefined && result.trap === undefined) ? 'ok' : 'failed');
  }

  retire() {
    this.#lifecycle = 'retired';
  }
}

/** Every extension component instance of one session, by namespace URI. */
export class SessionNamespaces {
  #instances = new Map();

  has(uri) {
    return this.#instances.has(uri);
  }

  get(uri) {
    return this.#instances.get(uri);
  }

  /**
   * Instantiates `prepared` for the session `links` and initializes it. A
   * mismatched `namespace-uri` or a failed `initialize` fails the run.
   * @throws {ScxmlHostError}
   */
  add(prepared, links) {
    const { run } = links;
    const span = run.telemetry.start('extension-instantiate');
    const state = { call: undefined, links, codec: run.extensions?.codec };
    const meter = new FuelMeter(run.extensions.callFuel);
    let api;
    try {
      api = run.budget.run(meter, () =>
        prepared.bindings.instantiate(prepared.module, extensionImports(state, run.telemetry), meter.instantiateCoreSync),
      ).executableContentExtensionApi;
    } catch (cause) {
      run.telemetry.end(span, 'failed');
      throw new ScxmlHostError(meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', { cause });
    }
    const instance = new ExtensionInstance(api, meter, state, run, prepared.readsSnapshot);
    const uri = instance.namespaceUri();
    if (uri.value !== prepared.package.namespaceUri) {
      run.telemetry.end(span, 'failed');
      throw new ScxmlHostError('extension-component-uri-mismatch');
    }
    const initialized = instance.initialize();
    if (initialized.trap !== undefined) {
      instance.retire();
      run.telemetry.end(span, 'failed');
      throw new ScxmlHostError(meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', { cause: initialized.trap });
    }
    if (initialized.payload !== undefined) {
      instance.shutdown();
      run.telemetry.end(span, 'failed');
      throw new ScxmlHostError('extension-component-initialize-failed');
    }
    run.telemetry.end(span, 'ok');
    this.#instances.set(prepared.package.namespaceUri, instance);
  }

  /** Shuts every live instance down (the session ended). */
  shutdown() {
    for (const instance of this.#instances.values()) {
      try {
        instance.shutdown();
      } catch {
        // The session's outcome is already decided.
      }
    }
    this.#instances.clear();
  }
}

