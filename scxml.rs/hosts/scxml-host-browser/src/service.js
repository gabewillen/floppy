// Invoked services (SCXML 6.4) in the browser host: the counterpart of the
// Wasmtime host's `ServiceComponents` (hosts/scxml-host-wasmtime/src/
// service.rs, outbound_http.rs, wasi.rs), with the same names, bounds, and
// admission rules. The host stays a thin linker:
//
// * Binding: `ServiceComponents.bind(package, component)` binds an exact
//   service type URI to an invoked-service component the page authenticated
//   with `authenticateServiceComponent(bytes, sha256)`: the bytes must hash
//   to the trusted SHA-256, and that digest must be the release artifact of
//   a service component this package ships transpiled (jco) and
//   fuel-metered (scripts/build.mjs, `dist/services.js`). A binding's
//   checks are the Wasmtime host's: a service type URI bound once, at most
//   MAX_BOUND_SERVICES bindings, at most MAX_DECLARED_ITEMS valid
//   `httpOrigins` and `envSlots`. When a session is created, every binding's
//   `wasi:*` imports are planned against the grants (`withWasi`), as
//   `wasi.rs` plans them, and its core modules are compiled.
// * Invocation: an `<invoke>` of a bound type gets its own instance,
//   instantiated synchronously inside `start-invocation` (which completes
//   there), and its `invoked-service-api.invoke` runs on the browser event
//   loop through JavaScript Promise Integration (JSPI). Events the invoking
//   session sends to `#_<invokeid>` (and autoforwards) feed the `events`
//   stream; the events the service returns reach the invoking session on
//   its external queue stamped with the invoke id; its future resolving
//   `ok` becomes `done.invoke.<id>` after every event returned before it;
//   resolving `err`, a trap (fuel exhaustion included), or an invalid
//   returned event becomes `error.communication.<id>`.
// * Browser differences: the instance runs asynchronously, so what it
//   returns reaches the session in a later event-loop turn (the page hears
//   it through `onTimer`), not at a deterministic point of the run; an
//   `invoke` that rejects its start does so after `start-invocation`
//   completed, so it becomes `error.communication.<id>` rather than
//   `error.execution`; `wasi:clocks/monotonic-clock` reads the session's
//   logical clock, and its waits fall due in wall-clock time.
// * Cancel: the invocation's stream ends, its requests in flight are
//   aborted, its timers cleared, and nothing it returns afterwards reaches
//   the session; no `done.invoke` follows.
// * Outbound HTTP: `outbound-http.fetch` and `send` admit a request exactly
//   as the Wasmtime host does (references substituted, WIT bounds, the
//   package's declared origins or an origin a reference's value supplies
//   entirely) and hand it to the page's transport
//   (`withOutboundHttp({fetch})`, called like the WHATWG `fetch` with
//   `redirect: 'manual'` and `credentials: 'omit'`); without one every
//   request is `unavailable`.
// * Outbound WebSocket: none; every `outbound-websocket.connect` is
//   `unavailable`.
// * Values and references: the page holds the values of the variables it
//   grants (`withValues`); a service never receives one, only a reference
//   for each `env` slot its package declares and the `<invoke>` binds to a
//   variable granted to its type URI, valid only inside its own admitted
//   requests.
// * WASI: clocks, randomness, and `wasi:io` need no grant (the pinned
//   preview2-shim's browser modules, `dist/wasi/`); `wasi:filesystem` needs
//   at least one directory (`withWasi(uri, {dirs: [[tree, guestPath]]})`,
//   each tree an in-memory directory the page owns and can read back);
//   `wasi:sockets` cannot be granted in a browser, and any other `wasi:*`
//   interface is refused.
// * Bounds: the invocation runs at most the call fuel (`withCallFuel`)
//   between two inputs (its start, an event it reads, a response, a timer),
//   returns at most MAX_RETURNED_EVENTS_PER_INPUT events of at most
//   MAX_RETURNED_BYTES_PER_INPUT bytes in all per input, and has at most
//   MAX_REQUESTS_IN_FLIGHT requests in flight.
//
// Telemetry: `service-start`, `service-send`, `service-cancel`,
// `service-http-fetch`, and `service-http-send` operations with closed
// outcomes only; guest telemetry is recorded under the `invoked-service`
// module. Type URIs, invoke ids, events, payloads, URLs, and values never
// become attributes.

import { ImportFailure, ScxmlHostError, exportPayload } from './errors.js';
import { FuelMeter, isFuel } from './fuel.js';
import { shippedServices } from './loader.js';
import { sha256Hex } from './sha256.js';
import { GUEST_MODULES, scxmlTelemetryImport } from './telemetry.js';
import { isDataValue, isWitEvent } from './values.js';

/** Default instruction fuel of one invocation between two inputs (the Wasmtime host's). */
export const DEFAULT_SERVICE_CALL_FUEL = 1_000_000_000;
/** Maximum service type URIs one session binds. */
export const MAX_BOUND_SERVICES = 64;
/** Maximum entries of a package's `httpOrigins` and of its `envSlots`. */
export const MAX_DECLARED_ITEMS = 32;
/** Maximum bytes of one service type URI. */
export const MAX_SERVICE_TYPE_URI_BYTES = 4_096;
/** Maximum events one invocation may return per input. */
export const MAX_RETURNED_EVENTS_PER_INPUT = 1_024;
/** Maximum bytes of the events one invocation may return per input. */
export const MAX_RETURNED_BYTES_PER_INPUT = 4 * 1024 * 1024;
/** Maximum requests of one invocation in flight at once. */
export const MAX_REQUESTS_IN_FLIGHT = 16;
/** Maximum bytes of one variable name. */
export const MAX_ENVIRONMENT_NAME_BYTES = 1_024;
/** Maximum bytes of one variable value. */
export const MAX_ENVIRONMENT_VALUE_BYTES = 65_536;
/** Maximum bytes of one `outbound-http` URL. */
export const MAX_EXTENSION_HTTP_URL_BYTES = 16_384;
/** Maximum header fields of one `outbound-http` request. */
export const MAX_EXTENSION_HTTP_HEADERS = 64;
/** Maximum bytes of one header name. */
export const MAX_EXTENSION_HTTP_HEADER_NAME_BYTES = 256;
/** Maximum bytes of one header value. */
export const MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES = 8_192;
/** Maximum bytes of one request body. */
export const MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES = 4 * 1024 * 1024;
/** Maximum bytes of one response body. */
export const MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES = 16 * 1024 * 1024;
/** Maximum parts of one URL, header value, or body. */
export const MAX_EXTENSION_HTTP_PARTS = 64;
/** The namespace of an `<invoke>`'s `env` attributes. */
export const ENV_NAMESPACE_URI = 'https://github.com/stateforward/scxml.rs/namespaces/env';

const MAX_ALLOWLIST_ENTRY_BYTES = 2_048;
const MAX_ENV_SLOT_BYTES = 64;
const MAX_WASI_DIRS = 32;
const MAX_WASI_GUEST_PATH_BYTES = 1_024;
const MAX_EVENT_METADATA_BYTES = 16_384;
const MAX_EVENT_DATA_BYTES = 65_536;
const MAX_EVENT_BYTES = 96 * 1024;
const MAX_NAME_BYTES = 1_024;
const NANOS_PER_MS = 1_000_000n;
const TRANSPORT_HEADERS = new Set(['host', 'content-length', 'transfer-encoding', 'connection']);
const SCXML_INVOKE_TYPES = new Set(['scxml', 'http://www.w3.org/TR/scxml/', 'http://www.w3.org/TR/scxml']);
const encoder = new TextEncoder();
const byteLength = (text) => encoder.encode(text).byteLength;

/**
 * The service components this build ships, by `dist/` component name
 * (`dist/services.js`: `service-example` and those the
 * build's `--services` added): their jco bindings are imported only when a
 * page binds one.
 */
const SHIPPED = await shippedServices();

let identities;

/** `dist/identity.js`: each shipped component's release digest and imports. */
function loadIdentities() {
  identities ??= import('../dist/identity.js').then(
    (module) => module,
    () => ({ RELEASE_DIGESTS: {}, COMPONENT_IMPORTS: {} }),
  );
  return identities;
}

const componentError = (code, detail) => ({ version: 'v1', code, detail });
const failure = (code, detail) => new ImportFailure(componentError(code, detail));
const invalid = (detail) => failure('invalid-input', detail);
const unavailable = (detail) => failure('unavailable', detail);

/**
 * Authenticates one invoked-service component: `bytes` must hash to
 * `sha256` (64 lowercase hex characters, the trust anchor), and that digest
 * must be the release artifact of a service component this package ships
 * built.
 * @param {Uint8Array} bytes the exact component bytes
 * @param {string} sha256 the trusted SHA-256
 * @returns {Promise<{kind: 'service-component', sha256: string, component: string}>}
 * @throws {ScxmlHostError} `invalid-argument` for malformed arguments,
 *   `artifact-digest-mismatch` when the bytes do not hash to `sha256`,
 *   `artifact-kind-mismatch` when no shipped service component has it
 */
export async function authenticateServiceComponent(bytes, sha256) {
  if (!(bytes instanceof Uint8Array) || typeof sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(sha256)) {
    throw new ScxmlHostError('invalid-argument');
  }
  if (sha256Hex(bytes) !== sha256) throw new ScxmlHostError('artifact-digest-mismatch');
  const { RELEASE_DIGESTS } = await loadIdentities();
  const component = Object.keys(SHIPPED).find((name) => RELEASE_DIGESTS[name] === sha256);
  if (component === undefined) throw new ScxmlHostError('artifact-kind-mismatch');
  return Object.freeze({ kind: 'service-component', sha256, component });
}

/**
 * Whether `value` names an invoked service (`scxml_types::is_service_type_uri`):
 * not an SCXML invoke type, an absolute URI, at most MAX_SERVICE_TYPE_URI_BYTES.
 */
export function isServiceTypeUri(value) {
  return (
    typeof value === 'string' &&
    !SCXML_INVOKE_TYPES.has(value) &&
    byteLength(value) <= MAX_SERVICE_TYPE_URI_BYTES &&
    /^[A-Za-z][A-Za-z0-9+.-]*:\S*$/.test(value)
  );
}

/** An `env-slots` entry: `[a-z][a-z0-9-]*`, at most 64 bytes, never ending in `expr`. */
function isEnvSlot(slot) {
  return typeof slot === 'string' && slot.length <= MAX_ENV_SLOT_BYTES && !slot.endsWith('expr') && /^[a-z][a-z0-9-]*$/.test(slot);
}

/** A variable name an `env` slot may name: `[A-Za-z_][A-Za-z0-9_]*`. */
function isEnvName(name) {
  return name.length <= MAX_ENVIRONMENT_NAME_BYTES && /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
}

/** An absolute `http`/`https` URL with a host and without userinfo, parsed; else undefined. */
function parseHttpUrl(text) {
  let url;
  try {
    url = new URL(text);
  } catch {
    return undefined;
  }
  if ((url.protocol !== 'http:' && url.protocol !== 'https:') || url.host === '' || url.username !== '' || url.password !== '') {
    return undefined;
  }
  return url;
}

/**
 * The canonical allowlist of one package's `httpOrigins` (the Wasmtime
 * host's `HttpAllowlist`): origins admit every path; a path prefix admits
 * itself, everything below it, and nothing beside it. Undefined when an
 * entry is refused.
 */
function allowlist(entries) {
  const parsed = [];
  for (const entry of entries) {
    if (typeof entry !== 'string' || byteLength(entry) > MAX_ALLOWLIST_ENTRY_BYTES || /[?#]/.test(entry)) return undefined;
    const url = parseHttpUrl(entry);
    if (url === undefined) return undefined;
    parsed.push({ origin: url.origin, path: url.pathname });
  }
  return (url) =>
    parsed.some(
      ({ origin, path }) =>
        url.origin === origin &&
        (path === '/' ||
          url.pathname === path ||
          (path.endsWith('/') && url.pathname.startsWith(path)) ||
          (url.pathname.startsWith(path) && url.pathname[path.length] === '/')),
    );
}

/** True when `value` is an in-memory directory tree: `{dir: {name: entry}}`. */
function isDirectoryTree(value) {
  return value !== null && typeof value === 'object' && value.dir !== null && typeof value.dir === 'object';
}

/** Invoked-service components one session links, keyed by exact type URI. */
export class ServiceComponents {
  #bindings = [];
  #transport;
  #values = [];
  #wasi = new Map();
  #callFuel = DEFAULT_SERVICE_CALL_FUEL;

  /**
   * Binds `component` to `pkg.serviceUri`, compared byte for byte with each
   * `<invoke type>`.
   * @param {object} pkg ServicePackage: `serviceUri`, `packageVersion`,
   *   `serviceArtifact`, `witWorld`, `witVersion` (strings), `httpOrigins`
   *   and `envSlots` (arrays of strings)
   * @param {object} component an authenticateServiceComponent result
   * @returns {ServiceComponents} this
   * @throws {ScxmlHostError} `artifact-kind-mismatch` for anything but an
   *   authenticated service component; `namespace-binding` for a binding the
   *   checks refuse
   */
  bind(pkg, component) {
    if (component?.kind !== 'service-component' || !Object.hasOwn(SHIPPED, component.component)) {
      throw new ScxmlHostError('artifact-kind-mismatch');
    }
    const fields = ['serviceUri', 'packageVersion', 'serviceArtifact', 'witWorld', 'witVersion'];
    if (
      pkg === null ||
      typeof pkg !== 'object' ||
      !fields.every((field) => typeof pkg[field] === 'string') ||
      !Array.isArray(pkg.httpOrigins) ||
      !Array.isArray(pkg.envSlots) ||
      !isServiceTypeUri(pkg.serviceUri) ||
      this.#bindings.length >= MAX_BOUND_SERVICES ||
      this.#bindings.some((bound) => bound.package.serviceUri === pkg.serviceUri) ||
      pkg.httpOrigins.length > MAX_DECLARED_ITEMS ||
      allowlist(pkg.httpOrigins) === undefined ||
      pkg.envSlots.length > MAX_DECLARED_ITEMS ||
      !pkg.envSlots.every(isEnvSlot)
    ) {
      throw new ScxmlHostError('namespace-binding');
    }
    const copy = Object.freeze({
      ...Object.fromEntries(fields.map((field) => [field, pkg[field]])),
      httpOrigins: Object.freeze([...pkg.httpOrigins]),
      envSlots: Object.freeze([...pkg.envSlots]),
    });
    this.#bindings.push(Object.freeze({ package: copy, component }));
    return this;
  }

  /**
   * The values services' `env` slots may reference (the variables the page
   * grants). Services receive references only, never a value.
   * @param {Array<{name: string, value: string, packages: string[]}>} variables
   *   each granted to exactly the type URIs in `packages`
   * @returns {ServiceComponents} this
   * @throws {ScxmlHostError} `invalid-argument` for a duplicate name, an
   *   empty name or one containing `=` or NUL, a name or value over its
   *   bound, a value containing NUL, or a variable granted to no package,
   *   to one package twice, or to a package URI that is not absolute
   */
  withValues(variables) {
    if (!Array.isArray(variables)) throw new ScxmlHostError('invalid-argument');
    const copies = [];
    for (const variable of variables) {
      const { name, value, packages } = variable ?? {};
      if (
        typeof name !== 'string' ||
        typeof value !== 'string' ||
        !Array.isArray(packages) ||
        name === '' ||
        byteLength(name) > MAX_ENVIRONMENT_NAME_BYTES ||
        /[=\0]/.test(name) ||
        byteLength(value) > MAX_ENVIRONMENT_VALUE_BYTES ||
        value.includes('\0') ||
        packages.length === 0 ||
        new Set(packages).size !== packages.length ||
        !packages.every((uri) => typeof uri === 'string' && byteLength(uri) <= MAX_SERVICE_TYPE_URI_BYTES && /^[A-Za-z][A-Za-z0-9+.-]*:\S*$/.test(uri)) ||
        copies.some((held) => held.name === name)
      ) {
        throw new ScxmlHostError('invalid-argument');
      }
      copies.push(Object.freeze({ name, value, packages: Object.freeze([...packages]) }));
    }
    this.#values = copies;
    return this;
  }

  /**
   * The transport admitted `outbound-http` requests go through:
   * `{fetch(url, init) => Promise<Response>}`, called like the WHATWG
   * `fetch`. Without one (the default) every request is `unavailable`.
   * @returns {ServiceComponents} this
   * @throws {ScxmlHostError} `invalid-argument`
   */
  withOutboundHttp(transport) {
    if (transport === null || typeof transport !== 'object' || typeof transport.fetch !== 'function') {
      throw new ScxmlHostError('invalid-argument');
    }
    this.#transport = transport;
    return this;
  }

  /**
   * The WASI resources the run grants the service bound to exactly `uri`,
   * replacing an earlier grant to it: `dirs`, each `[tree, guestPath]`, an
   * in-memory directory tree (`{dir: {}}`, preview2-shim's form, read and
   * written in place) preopened read-write at its absolute guest path.
   * @returns {ServiceComponents} this
   * @throws {ScxmlHostError} `invalid-argument` for a malformed grant
   */
  withWasi(uri, options) {
    const dirs = options?.dirs ?? [];
    const keys = options === null || typeof options !== 'object' ? [] : Object.keys(options);
    if (
      typeof uri !== 'string' ||
      !keys.every((key) => key === 'dirs') ||
      !Array.isArray(dirs) ||
      dirs.length > MAX_WASI_DIRS ||
      !dirs.every(
        (dir) =>
          Array.isArray(dir) &&
          dir.length === 2 &&
          isDirectoryTree(dir[0]) &&
          typeof dir[1] === 'string' &&
          dir[1].startsWith('/') &&
          byteLength(dir[1]) <= MAX_WASI_GUEST_PATH_BYTES &&
          !dir[1].includes('\0'),
      ) ||
      new Set(dirs.map((dir) => dir[1])).size !== dirs.length
    ) {
      throw new ScxmlHostError('invalid-argument');
    }
    this.#wasi.set(uri, Object.freeze({ dirs: Object.freeze(dirs.map(([tree, guest]) => Object.freeze([tree, guest]))) }));
    return this;
  }

  /**
   * Sets the instruction fuel an invocation may run between two inputs.
   * @param {number} fuel a positive safe integer
   * @returns {ServiceComponents} this
   * @throws {ScxmlHostError} `invalid-argument`
   */
  withCallFuel(fuel) {
    if (!isFuel(fuel)) throw new ScxmlHostError('invalid-argument');
    this.#callFuel = fuel;
    return this;
  }

  /** Whether no service is bound. */
  isEmpty() {
    return this.#bindings.length === 0;
  }

  /** The bindings, transport, values, grants, and call fuel the session links (internal). */
  parts() {
    return {
      bindings: [...this.#bindings],
      transport: this.#transport,
      values: this.#values,
      wasi: new Map(this.#wasi),
      callFuel: this.#callFuel,
    };
  }
}

/** Whether `value` is a ServiceComponents (createSession's option). */
export function isServiceComponents(value) {
  return value === undefined || value instanceof ServiceComponents;
}

/**
 * How one component's imports are linked (`wasi.rs`'s plan): `filesystem`
 * when it imports `wasi:filesystem`, `shim` when it imports any interface
 * the preview2-shim serves.
 * @throws {ScxmlHostError} `unknown-import`, `wasi-resource-ungranted`,
 *   `wasi-grant-undeclared`
 */
function plan(imports, grant) {
  let filesystem = false;
  let shim = false;
  for (const name of imports) {
    if (['stateforward:scxml/types', 'stateforward:scxml/telemetry', 'stateforward:scxml/outbound-http', 'stateforward:scxml/outbound-websocket'].includes(name)) continue;
    // The service world's clock, run on the session's logical clock.
    if (name === 'wasi:clocks/monotonic-clock' || name === 'wasi:clocks/types') continue;
    if (name === 'wasi:clocks/wall-clock' || name.startsWith('wasi:io/') || name.startsWith('wasi:random/')) {
      shim = true;
      continue;
    }
    if (name.startsWith('wasi:filesystem/')) {
      filesystem = true;
      shim = true;
      continue;
    }
    // A browser opens no socket: TCP and name lookup cannot be granted.
    if (name.startsWith('wasi:sockets/')) throw new ScxmlHostError('wasi-resource-ungranted');
    throw new ScxmlHostError('unknown-import');
  }
  const dirs = grant?.dirs ?? [];
  if (filesystem && dirs.length === 0) throw new ScxmlHostError('wasi-resource-ungranted');
  if (!filesystem && dirs.length > 0) throw new ScxmlHostError('wasi-grant-undeclared');
  return { filesystem, shim, dirs };
}

/** Whether this browser has JavaScript Promise Integration, which service components run on. */
const jspi = () => typeof WebAssembly.Suspending === 'function' && typeof WebAssembly.promising === 'function';

/** Sentinel that stops a recording instantiation once every core module is requested. */
const RECORDED = Symbol('recorded');

/**
 * One bound service, prepared for a session: its jco bindings, its core
 * modules compiled (so each invocation instantiates synchronously), and its
 * WASI plan.
 */
async function prepare(binding, loader, grant) {
  const { component } = binding.component;
  const { COMPONENT_IMPORTS } = await loadIdentities();
  const wasi = plan(COMPONENT_IMPORTS?.[component] ?? [], grant);
  // Without JSPI nothing is compiled: each invocation's start answers
  // `unavailable`.
  if (!jspi()) return { package: binding.package, wasi };
  const bindings = await SHIPPED[component]();
  const compiled = new Map();
  const requested = [];
  const recording = (file) => {
    const module = Promise.resolve(loader(component, file)).then((module) => {
      compiled.set(file, module);
      return module;
    });
    requested.push(module);
    return module;
  };
  // jco requests every core module before it instantiates the first one:
  // the instantiation stops there, and nothing is instantiated.
  const anyImport = new Proxy({}, { get: () => new Proxy(() => {}, { get: () => () => {} }) });
  try {
    await bindings.instantiate(recording, anyImport, () => {
      throw RECORDED;
    });
  } catch (thrown) {
    if (thrown !== RECORDED) throw thrown;
  }
  await Promise.all(requested);
  return {
    package: binding.package,
    allow: allowlist(binding.package.httpOrigins),
    bindings,
    wasi,
    shim: wasi.shim ? await loadShim() : undefined,
    module: (file) => {
      const found = compiled.get(file);
      if (found === undefined) throw new TypeError('core module was not preloaded');
      return found;
    },
  };
}

let shimModules;

/** The preview2-shim browser modules (`dist/wasi/`), loaded once. */
function loadShim() {
  shimModules ??= Promise.all([
    import('../dist/wasi/filesystem.js'),
    import('../dist/wasi/io.js'),
    import('../dist/wasi/clocks.js'),
    import('../dist/wasi/random.js'),
  ]).then(([filesystem, io, clocks, random]) => ({ filesystem, io, clocks, random }));
  return shimModules;
}

/**
 * Prepares every binding of `services` (a ServiceComponents) for a session;
 * `undefined` when nothing is bound.
 * @throws {ScxmlHostError} `wasi-grant-undeclared` for a grant to a URI no
 *   binding names, and every planning failure
 */
export async function prepareServices(services, loader) {
  if (services === undefined || services.isEmpty()) return undefined;
  const { bindings, transport, values, wasi, callFuel } = services.parts();
  for (const uri of wasi.keys()) {
    if (!bindings.some((binding) => binding.package.serviceUri === uri)) throw new ScxmlHostError('wasi-grant-undeclared');
  }
  const prepared = new Map();
  for (const binding of bindings) {
    const uri = binding.package.serviceUri;
    prepared.set(uri, await prepare(binding, loader, wasi.get(uri)));
  }
  return { prepared, transport, values, callFuel };
}

/**
 * The slot references of one invocation (the Wasmtime host's
 * `bind_references`): for each `env` attribute whose slot the package
 * declares, a fresh reference (numbered from 1) when the page holds a value
 * for the variable granted to `typeUri`; otherwise the slot is absent.
 * @throws {ImportFailure} `invalid-input`
 */
function bindReferences(attributes, typeUri, envSlots, values) {
  if (attributes.length > MAX_DECLARED_ITEMS) throw invalid('the invocation binds more env slots than a package declares');
  const slots = [];
  const references = new Map();
  const seen = new Set();
  for (const attribute of attributes) {
    if (attribute?.name?.namespaceUri !== ENV_NAMESPACE_URI) throw invalid('an invoke attribute is not in the env namespace');
    const slot = attribute.name.localName;
    if (!envSlots.includes(slot)) throw invalid("an env slot is not declared by the service's package");
    if (seen.has(slot)) throw invalid('the invocation binds one env slot twice');
    seen.add(slot);
    if (typeof attribute.value !== 'string' || !isEnvName(attribute.value)) throw invalid('an env slot names no valid variable');
    const held = values.find((variable) => variable.name === attribute.value && variable.packages.includes(typeUri));
    if (held === undefined) continue;
    const reference = references.size + 1;
    references.set(reference, held.value);
    slots.push({ slot, reference });
  }
  return { slots, references };
}

/**
 * Substitutes, bounds, and admits one `outbound-http` request (the Wasmtime
 * host's `admit_request`): the canonical request the transport sends.
 * @throws {ImportFailure}
 */
function admitRequest(request, references, allow, transport) {
  const value = (reference) => {
    if (!references.has(reference)) throw invalid('the request names a reference this execution did not receive');
    return references.get(reference);
  };
  const text = (parts) => {
    if (!Array.isArray(parts) || parts.length > MAX_EXTENSION_HTTP_PARTS) throw invalid('a request field has too many parts');
    let joined = '';
    for (const part of parts) {
      if (part?.tag === 'literal' && typeof part.val === 'string') joined += part.val;
      else if (part?.tag === 'reference') joined += value(part.val);
      else throw invalid('a request part is malformed');
    }
    return joined;
  };
  if (request === null || typeof request !== 'object' || !Array.isArray(request.headers) || !Array.isArray(request.body)) {
    throw invalid('the request is malformed');
  }
  const url = text(request.url);
  let suppliedOrigin;
  const first = request.url[0];
  if (first?.tag === 'reference') {
    const supplied = value(first.val);
    const rest = url.slice(supplied.length);
    const parsed = parseHttpUrl(supplied);
    if (parsed !== undefined && (rest === '' || /^[/?#]/.test(rest))) suppliedOrigin = parsed.origin;
  }
  const headers = request.headers.map((header) => [header?.name, text(header?.value)]);
  if (request.body.length > MAX_EXTENSION_HTTP_PARTS) throw invalid('a request field has too many parts');
  const chunks = [];
  for (const part of request.body) {
    if (part?.tag === 'bytes' && part.val instanceof Uint8Array) chunks.push(part.val);
    else if (part?.tag === 'reference') chunks.push(encoder.encode(value(part.val)));
    else throw invalid('a request part is malformed');
  }
  const method = request.method;
  if (method !== 'GET' && method !== 'POST') throw invalid('the request method is not GET or POST');
  if (byteLength(url) > MAX_EXTENSION_HTTP_URL_BYTES) throw invalid('the request URL exceeds its byte bound');
  const target = parseHttpUrl(url);
  if (target === undefined) throw invalid('the request URL is not an absolute http or https URL');
  if (headers.length > MAX_EXTENSION_HTTP_HEADERS) throw invalid('the request has too many header fields');
  for (const [name, headerValue] of headers) {
    if (
      typeof name !== 'string' ||
      name.length < 1 ||
      byteLength(name) > MAX_EXTENSION_HTTP_HEADER_NAME_BYTES ||
      !/^[A-Za-z0-9!#$%&'*+\-.^_`|~]+$/.test(name) ||
      TRANSPORT_HEADERS.has(name.toLowerCase())
    ) {
      throw invalid('a request header name is invalid or owned by the transport');
    }
    if (byteLength(headerValue) > MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES || /[\r\n\0]/.test(headerValue)) {
      throw invalid('a request header value is invalid');
    }
  }
  const size = chunks.reduce((total, chunk) => total + chunk.byteLength, 0);
  if (size > MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES || (method === 'GET' && size > 0)) {
    throw invalid('the request body exceeds its bound or accompanies GET');
  }
  if (transport === undefined) throw unavailable('the host offers this component no outbound HTTP');
  if (!allow(target) && suppliedOrigin !== target.origin) {
    throw unavailable('the request target is neither declared by the package nor supplied by the run');
  }
  const body = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    body.set(chunk, at);
    at += chunk.byteLength;
  }
  return { method, url: target.href, headers, body: method === 'GET' ? undefined : body };
}

/**
 * Runs `instantiate` (one synchronous jco instantiation) with each export
 * jco wraps for JSPI reporting a trap to `onTrap`. jco wraps the guest's
 * `[async-lift]` and `[callback]` exports with `WebAssembly.promising` while
 * it instantiates; after `invoke` returned, the invocation runs only inside
 * the callback's calls, whose traps jco reports to no caller. The wrapper is
 * installed only for the synchronous instantiation and restored after it.
 */
function reportingTraps(instantiate, onTrap) {
  const promising = WebAssembly.promising;
  const trapped = (thrown) => {
    if (thrown instanceof WebAssembly.RuntimeError) onTrap();
  };
  WebAssembly.promising = (fn) => {
    const wrapped = promising(fn);
    return (...args) => {
      const result = wrapped(...args);
      result.then(undefined, trapped);
      return result;
    };
  };
  try {
    return instantiate();
  } finally {
    WebAssembly.promising = promising;
  }
}

/** A transport failure as the service sees it. */
const transportFailed = () => unavailable('the HTTP transport failed');
const tooLarge = () => failure('payload-too-large', 'the response body exceeds its byte bound');

/** The UTF-8 bytes one event counts toward its bounds, or undefined past one (`Event::validate`'s). */
function eventBytes(event) {
  let total = byteLength(event.name);
  if (event.name === '' || total > MAX_NAME_BYTES) return undefined;
  for (const value of [event.origin, event.originType]) {
    if (value === undefined) continue;
    const bytes = byteLength(value);
    if (bytes > MAX_EVENT_METADATA_BYTES) return undefined;
    total += bytes;
  }
  for (const value of [event.sendId, event.invokeId]) if (value !== undefined) total += byteLength(value);
  const { data } = event;
  if (data.tag === 'boolean') total += 1;
  else if (data.tag === 'number') total += 8;
  else if (typeof data.val === 'string') {
    const bytes = byteLength(data.val);
    if (bytes > MAX_EVENT_DATA_BYTES) return undefined;
    total += bytes;
  }
  if (event.raw !== undefined) {
    const bytes = byteLength(event.raw);
    if (bytes > MAX_EVENT_DATA_BYTES) return undefined;
    total += bytes;
  }
  const context = event.traceContext;
  if (context !== undefined) {
    for (const value of [context.traceparent, context.tracestate, context.baggage]) if (value !== undefined) total += byteLength(value);
  }
  return total > MAX_EVENT_BYTES ? undefined : total;
}

/**
 * The events the invoking session sends one invocation, as the `events`
 * stream's async iterable. Each event the service reads is one input.
 */
class EventStream {
  #queued = [];
  #waiting;
  #closed = false;
  #onRead;

  constructor(onRead) {
    this.#onRead = onRead;
  }

  push(event) {
    if (this.#closed) return;
    if (this.#waiting === undefined) {
      this.#queued.push(event);
      return;
    }
    const resolve = this.#waiting;
    this.#waiting = undefined;
    this.#onRead();
    resolve({ value: event, done: false });
  }

  close() {
    this.#closed = true;
    this.#queued = [];
    const resolve = this.#waiting;
    this.#waiting = undefined;
    resolve?.({ value: undefined, done: true });
  }

  next() {
    if (this.#queued.length > 0) {
      this.#onRead();
      return Promise.resolve({ value: this.#queued.shift(), done: false });
    }
    if (this.#closed) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve) => {
      this.#waiting = resolve;
    });
  }

  return() {
    this.close();
    return Promise.resolve({ value: undefined, done: true });
  }

  [Symbol.asyncIterator]() {
    return this;
  }
}

/**
 * One invocation of a bound service. `host` is what the invoking session
 * lends it: `{telemetry, transport, values, callFuel, now() => ms,
 * deliver(message), wake()}`; `deliver` queues one invocation message
 * (`{tag: 'event' | 'done' | 'communication-error'}`) on the invoking
 * session, and `wake` asks the host to deliver what is queued.
 */
export class ServiceInvocation {
  #prepared;
  #host;
  #meter;
  #api;
  #stream;
  #references = new Map();
  #request;
  #controllers = new Set();
  /** Pending waits: timer -> resolve. */
  #timers = new Map();
  #filesystem;
  /** The iterator of the events the service returns, once it started. */
  #returnedEvents;
  #inFlight = 0;
  #returned = 0;
  #returnedBytes = 0;
  /** `'live'`, then `'finished'` (done, failed) or `'cancelled'`. */
  state = 'live';

  constructor(prepared, host, request) {
    this.#prepared = prepared;
    this.#host = host;
    this.#request = request;
  }

  /**
   * Starts the invocation inside `start-invocation`: binds its references,
   * instantiates its component, and calls `invoke`, which runs on.
   * @throws {ImportFailure} `invalid-input` for a malformed `env` binding;
   *   `unavailable` where JSPI is missing; `invalid-input` when the
   *   component cannot be instantiated
   */
  start() {
    const { package: pkg, bindings, module } = this.#prepared;
    const request = this.#request;
    const { slots, references } = bindReferences(request.attributes, request.typeUri, pkg.envSlots, this.#host.values);
    this.#references = references;
    if (bindings === undefined || !jspi()) {
      throw unavailable('this browser runs no invoked service (JavaScript Promise Integration is missing)');
    }
    this.#meter = new FuelMeter(this.#host.callFuel);
    this.#meter.grant(this.#meter.cap);
    try {
      this.#api = reportingTraps(
        () => bindings.instantiate(module, this.#imports(), this.#meter.instantiateCoreSync).invokedServiceApi,
        () => this.#fail(),
      );
    } catch (cause) {
      this.#closeFilesystem();
      throw invalid('the service component could not be instantiated');
    }
    this.#stream = new EventStream(() => this.#input());
    this.#run({ ...request, references: slots });
  }

  /** Queues one event the invoking session sent to `#_<invokeid>` (or autoforwarded). */
  send(event) {
    if (this.state !== 'live') return;
    this.#host.telemetry.observe('service-send', 'ok');
    this.#stream.push(event);
  }

  /** Cancels the invocation: nothing it returns from now on reaches the session. */
  cancel() {
    if (this.state === 'cancelled') return;
    const live = this.state === 'live';
    this.state = 'cancelled';
    this.#finish();
    if (live) this.#host.telemetry.observe('service-cancel', 'ok');
  }

  /** One input: the invocation may run the call fuel again and return a fresh batch of events. */
  #input() {
    if (this.state !== 'live') return;
    this.#meter.grant(this.#meter.cap);
    this.#returned = 0;
    this.#returnedBytes = 0;
  }

  async #run(request) {
    // `invoke` runs outside the machine call that started it.
    await null;
    if (this.state !== 'live') return;
    const span = this.#host.telemetry.start('service-start');
    let events;
    let done;
    try {
      [events, done] = await this.#api.invoke(request, this.#stream);
    } catch (thrown) {
      this.#host.telemetry.end(span, exportPayload(thrown) === undefined ? 'failed' : 'rejected');
      this.#fail();
      return;
    }
    this.#host.telemetry.end(span, 'ok');
    this.#returnedEvents = events[Symbol.asyncIterator]();
    if (this.state !== 'live') {
      this.#release();
      return;
    }
    // Every event returned before the future resolved reaches the session
    // before `done.invoke`.
    const returned = (async () => {
      for (;;) {
        const { value: event, done: ended } = await this.#returnedEvents.next();
        if (ended || this.state !== 'live') return;
        if (!this.#accept(event)) throw new TypeError('invalid returned event');
      }
    })();
    let outcome;
    try {
      // The future is read only once the stream ended: a read polls until
      // the future resolves, which a dropped instance never does.
      await returned;
      if (this.state !== 'live') return;
      outcome = await done;
    } catch {
      this.#fail();
      return;
    }
    if (this.state !== 'live') return;
    if (outcome?.tag === 'err') {
      this.#fail();
      return;
    }
    const data = outcome?.tag === 'ok' ? outcome.val : outcome;
    if (data !== undefined && !isDataValue(data)) {
      this.#fail();
      return;
    }
    this.state = 'finished';
    this.#release();
    this.#host.deliver({ tag: 'done', val: { data, traceContext: undefined } });
  }

  /** Validates, stamps, and queues one returned event; false when it breaks its bounds. */
  #accept(event) {
    if (!isWitEvent(event)) return false;
    const stamped = {
      name: event.name,
      eventType: 'external',
      sendId: event.sendId,
      origin: event.origin,
      originType: this.#request.typeUri,
      // The invoking session stamps the invoke id on delivery.
      invokeId: undefined,
      data: event.data,
      raw: event.raw,
      traceContext: event.traceContext ?? this.#request.traceContext,
    };
    const bytes = eventBytes(stamped);
    if (bytes === undefined) return false;
    this.#returned += 1;
    this.#returnedBytes += bytes;
    if (this.#returned > MAX_RETURNED_EVENTS_PER_INPUT || this.#returnedBytes > MAX_RETURNED_BYTES_PER_INPUT) return false;
    this.#host.deliver({ tag: 'event', val: stamped });
    return true;
  }

  /** The invocation failed after its start: `error.communication.<id>`, once. */
  #fail() {
    if (this.state !== 'live') return;
    this.state = 'finished';
    this.#finish();
    this.#host.deliver({ tag: 'communication-error' });
  }

  /**
   * Drops the instance as a store is dropped: it gets no fuel from now on,
   * and everything it waits on settles at once (its stream ends, its
   * requests are aborted, its waits resolve), so whatever of it still runs
   * traps at its next charge and its tasks end instead of staying suspended.
   */
  #finish() {
    this.#meter?.grant(0n);
    this.#release();
  }

  /** Ends its stream, aborts its requests, resolves its waits, and closes its directories. */
  #release() {
    this.#stream?.close();
    this.#returnedEvents?.return?.()?.catch?.(() => {});
    for (const controller of this.#controllers) controller.abort();
    this.#controllers.clear();
    for (const [timer, resolve] of this.#timers) {
      clearTimeout(timer);
      resolve();
    }
    this.#timers.clear();
    this.#closeFilesystem();
  }

  #closeFilesystem() {
    try {
      this.#filesystem?.dispose();
    } catch {
      // The trees stay the page's.
    }
    this.#filesystem = undefined;
  }

  /** Resolves after `ms` of wall-clock time, as one input (at once, with no fuel, once dropped). */
  #wait(ms) {
    return new Promise((resolve) => {
      if (this.state !== 'live') {
        resolve();
        return;
      }
      const timer = setTimeout(
        () => {
          this.#timers.delete(timer);
          this.#input();
          resolve();
        },
        Math.max(0, Math.min(ms, 2 ** 31 - 1)),
      );
      this.#timers.set(timer, resolve);
    });
  }

  /** The session's logical time as a monotonic-clock mark (ns, 1 ms resolution). */
  #now() {
    return BigInt(Math.max(0, Math.floor(this.#host.now()))) * NANOS_PER_MS;
  }

  #imports() {
    const telemetry = scxmlTelemetryImport(this.#host.telemetry, GUEST_MODULES.service);
    const monotonicClock = {
      now: () => this.#now(),
      getResolution: () => NANOS_PER_MS,
      waitUntil: (when) => this.#wait(Number((BigInt(when) - this.#now()) / NANOS_PER_MS)),
      waitFor: (howLong) => this.#wait(Number(BigInt(howLong) / NANOS_PER_MS)),
    };
    const outboundHttp = {
      fetch: (request) => this.#fetch(request),
      send: (request) => this.#send(request),
    };
    // This host offers no outbound WebSocket: every connection is refused.
    const outboundWebsocket = {
      connect: async () => {
        throw unavailable('the host offers this component no outbound WebSocket');
      },
    };
    const imports = {};
    for (const [name, value] of [
      ['stateforward:scxml/types', {}],
      ['stateforward:scxml/telemetry', telemetry],
      ['stateforward:scxml/outbound-http', outboundHttp],
      ['stateforward:scxml/outbound-websocket', outboundWebsocket],
    ]) {
      imports[name] = value;
      imports[`${name}@0.2.0`] = value;
    }
    for (const [name, value] of [
      ['wasi:clocks/monotonic-clock', monotonicClock],
      ['wasi:clocks/types', {}],
    ]) {
      imports[name] = value;
      imports[`${name}@0.3.0`] = value;
    }
    const { shim, wasi } = this.#prepared;
    if (shim !== undefined) {
      Object.assign(imports, {
        'wasi:io/error': shim.io.error,
        'wasi:io/poll': shim.io.poll,
        'wasi:io/streams': shim.io.streams,
        'wasi:clocks/wall-clock': shim.clocks.wallClock,
        'wasi:random/random': shim.random.random,
        'wasi:random/insecure': shim.random.insecure,
        'wasi:random/insecure-seed': shim.random.insecureSeed,
      });
      if (wasi.filesystem) {
        this.#filesystem = shim.filesystem.createFilesystem({
          adapter: new shim.filesystem.InMemoryFilesystemAdapter(),
          preopens: Object.fromEntries(wasi.dirs.map(([tree, guest]) => [guest, tree])),
        });
        imports['wasi:filesystem/types'] = this.#filesystem.types;
        imports['wasi:filesystem/preopens'] = this.#filesystem.preopens;
      }
    }
    return imports;
  }

  /** Admits one request and counts it in flight: `{request, controller}`. */
  #begin(request) {
    if (this.state !== 'live') throw unavailable('the call was made outside an invocation');
    const admitted = admitRequest(request, this.#references, this.#prepared.allow, this.#host.transport);
    if (this.#inFlight >= MAX_REQUESTS_IN_FLIGHT) {
      throw failure('capacity-exhausted', 'the invocation has too many requests in flight');
    }
    this.#inFlight += 1;
    const controller = new AbortController();
    this.#controllers.add(controller);
    return { request: admitted, controller };
  }

  #end(controller) {
    this.#inFlight -= 1;
    this.#controllers.delete(controller);
  }

  /** The transport's response to one admitted request; a status outside 100..=599 is a transport failure. */
  async #respond({ request, controller }) {
    let response;
    try {
      response = await this.#host.transport.fetch(request.url, {
        method: request.method,
        headers: request.headers,
        body: request.body,
        signal: controller.signal,
        redirect: 'manual',
        credentials: 'omit',
      });
    } catch {
      throw transportFailed();
    }
    if (!Number.isInteger(response?.status) || response.status < 100 || response.status > 599) throw transportFailed();
    return response;
  }

  async #fetch(request) {
    const span = this.#host.telemetry.start('service-http-fetch');
    let begun;
    try {
      begun = this.#begin(request);
    } catch (thrown) {
      this.#host.telemetry.end(span, 'rejected');
      throw thrown;
    }
    try {
      const response = await this.#respond(begun);
      const body = await readBody(response);
      this.#host.telemetry.end(span, 'ok');
      return { status: response.status, body };
    } catch (thrown) {
      this.#host.telemetry.end(span, 'failed');
      throw thrown instanceof ImportFailure ? thrown : transportFailed();
    } finally {
      this.#end(begun.controller);
      this.#input();
    }
  }

  async #send(request) {
    const span = this.#host.telemetry.start('service-http-send');
    let begun;
    try {
      begun = this.#begin(request);
    } catch (thrown) {
      this.#host.telemetry.end(span, 'rejected');
      throw thrown;
    }
    let response;
    try {
      response = await this.#respond(begun);
    } catch (thrown) {
      this.#host.telemetry.end(span, 'failed');
      this.#end(begun.controller);
      this.#input();
      throw thrown;
    }
    this.#host.telemetry.end(span, 'ok');
    this.#input();
    let settle;
    const ended = new Promise((resolve) => {
      settle = resolve;
    });
    const invocation = this;
    async function* body() {
      let received = 0;
      try {
        const reader = response.body?.getReader();
        if (reader !== undefined) {
          for (;;) {
            const { value, done } = await reader.read();
            invocation.#input();
            if (done) break;
            received += value.byteLength;
            if (received > MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES) {
              reader.cancel().catch(() => {});
              settle({ tag: 'err', val: tooLarge().payload });
              return;
            }
            for (const byte of value) yield byte;
          }
        }
        settle({ tag: 'ok', val: undefined });
      } catch {
        settle({ tag: 'err', val: transportFailed().payload });
      } finally {
        invocation.#end(begun.controller);
      }
    }
    return [response.status, body(), ended];
  }
}

/** A response's whole body within MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES. */
async function readBody(response) {
  const reader = response.body?.getReader();
  if (reader === undefined) return new Uint8Array(0);
  const chunks = [];
  let size = 0;
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES) {
      reader.cancel().catch(() => {});
      throw tooLarge();
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    body.set(chunk, at);
    at += chunk.byteLength;
  }
  return body;
}
