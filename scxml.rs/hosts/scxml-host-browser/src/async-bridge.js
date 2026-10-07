// Async bridge: lets the machine's synchronous capability calls wait on
// asynchronous page work (a runtime-computed document URL resolved through
// the page's `resources.resolve`, a Promise).
//
// Opt-in only: `createSession({async: 'auto'})`. The default (`async:
// 'off'`) never loads this bridge into a session: the session, its
// capability calls, and its results are exactly those of a synchronous
// session, with no added cost.
//
// Mechanisms (see README "Async sessions"):
// * `worker`: the unchanged synchronous MachineSession runs in a dedicated
//   module Worker (src/async-worker.js). A capability call that needs the
//   page posts one request to the page and blocks the worker with
//   `Atomics.wait` on a SharedArrayBuffer until the page has awaited its
//   answer and written it back (`waitSync`). The page stays responsive.
//   Requires `crossOriginIsolated` (COOP `same-origin` + COEP
//   `require-corp`), module Workers, and growable SharedArrayBuffers.
// * `none`: without those, the session runs synchronously in the page, and
//   a URL the page does not supply fails with the typed `unavailable`
//   error (detail `dynamic-url-unavailable`), which the machine turns into
//   `error.execution` (SCXML 5.3).
// JSPI is not used in this release: jco's JSPI output makes every machine
// export async, and JSPI cannot suspend through the host's JS capability
// layer, so it would need a second, async copy of the session scheduler
// (README "Async sessions", follow-up).
//
// Wire protocol of one worker session (all messages are structured clones):
//   page -> worker  {type: 'init', channel, options}      once
//                   {type: 'op', id, name, args}          one session method
//                   {type: 'module', id, module | failed} a core module reply
//   worker -> page  {type: 'ready'} | {type: 'init-failed', error}
//                   {type: 'result', id, value | error}
//                   {type: 'call', operation, args}       blocking (waitSync)
//                   {type: 'module', id, component, file} a core module request
//                   {type: 'timer', outcome}              `onTimer`
// The blocking reply travels through the SharedArrayBuffer `channel`:
// Int32 [0] state (IDLE, READY), Int32 [1] reply length, then the reply:
// one tag byte and its payload (see encodeReply).

import { DATA_MODELS, createDataModel } from './datamodel.js';
import {
  CapabilityError,
  ERROR_CODES,
  ImportFailure,
  ScxmlHostError,
  ScxmlSessionError,
  ScxmlSnapshotError,
  toComponentError,
} from './errors.js';
import { invokeLimitsCopy } from './invoked-sessions.js';
import { fetchCoreModules } from './loader.js';
import {
  MAX_RESOURCE_URL_BYTES,
  allowedUrl,
  byteLength,
  isResourcesOption,
  prefetchResources,
  resolvedDocument,
} from './resources.js';
import { Telemetry, isTelemetryOption } from './telemetry.js';

/** Closed values of createSession's `async` option. */
export const ASYNC_MODES = Object.freeze(['off', 'auto']);
/** Closed values of `AsyncMachineSession.mechanism`. */
export const ASYNC_MECHANISMS = Object.freeze(['worker', 'none']);
/** `component-error.detail` of a URL no async mechanism can resolve. */
export const DYNAMIC_URL_UNAVAILABLE = 'dynamic-url-unavailable';
/**
 * Address space one session's reply channel reserves, tried in order: the
 * host sets no bound of its own on a reply (a document of any size loads in
 * a worker session as in a sync one), so the channel reserves room for the
 * longest reply its Int32 length word can describe and commits memory only
 * as it grows. A platform that refuses a reservation gets the next.
 */
const CHANNEL_RESERVATIONS = Object.freeze([2 ** 31, 2 ** 30]);

/**
 * Longest `dispose` waits for a worker session to release its engine
 * before the worker is terminated anyway.
 */
export const DISPOSE_GRACE_MS = 1000;

/**
 * Internal createSession option key: the bridge a session's capability
 * calls resolve dynamic URLs through. Not exported from the package.
 */
export const BRIDGE = Symbol('scxml.async-bridge');

const HEADER_BYTES = 8;
/**
 * Longest reply the channel carries: its length travels in one Int32 word
 * of the header, so a reply is at most `2^31 - 1 - HEADER_BYTES` bytes. A
 * longer reply, or one past what the channel could reserve or grow to, is
 * the transport's `payload-too-large`.
 */
export const MAX_REPLY_BYTES = 2 ** 31 - 1 - HEADER_BYTES;
const INITIAL_CHANNEL_BYTES = 64 * 1024;
const IDLE = 0;
const READY = 1;
const TAG = Object.freeze({ undefined: 0, string: 1, bytes: 2, json: 3, error: 4 });
const utf8 = new TextEncoder();
const text = new TextDecoder();

let defaultLoader;

/**
 * The mechanism an `async: 'auto'` session gets in `scope` (default: this
 * realm): `worker` when module Workers, growable SharedArrayBuffers, and
 * `Atomics.wait` are available and the page is cross-origin isolated,
 * otherwise `none`.
 * @param {object} [scope]
 * @returns {'worker' | 'none'}
 */
export function asyncMechanism(scope = globalThis) {
  const shared = scope.SharedArrayBuffer;
  return typeof scope.Worker === 'function' &&
    typeof shared === 'function' &&
    typeof shared.prototype.grow === 'function' &&
    typeof scope.Atomics?.wait === 'function' &&
    scope.crossOriginIsolated === true
    ? 'worker'
    : 'none';
}

/** The typed failure of a URL no async mechanism can resolve. */
export function dynamicUrlUnavailable() {
  return new ImportFailure({ version: 'v1', code: 'unavailable', detail: DYNAMIC_URL_UNAVAILABLE });
}

/** The bridge of a session without an async mechanism: every URL is the typed error. */
export const UNAVAILABLE_BRIDGE = Object.freeze({
  waitSync() {
    throw dynamicUrlUnavailable();
  },
  resolveDynamic() {
    throw dynamicUrlUnavailable();
  },
});

// --- blocking channel -------------------------------------------------------

/** Encodes one reply: a tag byte, then the value's bytes. */
export function encodeReply(outcome) {
  let tag;
  let body;
  if (outcome.error !== undefined) {
    tag = TAG.error;
    body = utf8.encode(outcome.error);
  } else if (outcome.value === undefined) {
    tag = TAG.undefined;
    body = new Uint8Array(0);
  } else if (typeof outcome.value === 'string') {
    tag = TAG.string;
    body = utf8.encode(outcome.value);
  } else if (outcome.value instanceof Uint8Array) {
    tag = TAG.bytes;
    body = outcome.value;
  } else {
    tag = TAG.json;
    body = utf8.encode(JSON.stringify(outcome.value));
  }
  const reply = new Uint8Array(1 + body.byteLength);
  reply[0] = tag;
  reply.set(body, 1);
  return reply;
}

/** Decodes one reply: `{value}` or `{error}` (a WIT error code). */
export function decodeReply(reply) {
  const body = reply.subarray(1);
  switch (reply[0]) {
    case TAG.undefined:
      return { value: undefined };
    case TAG.string:
      return { value: text.decode(body) };
    case TAG.bytes:
      return { value: body.slice() };
    case TAG.json:
      return { value: JSON.parse(text.decode(body)) };
    case TAG.error: {
      const code = text.decode(body);
      return { error: ERROR_CODES.includes(code) ? code : 'platform' };
    }
    default:
      return { error: 'platform' };
  }
}

/** A growable SharedArrayBuffer for one session's blocking replies. */
export function createChannel() {
  for (const maxByteLength of CHANNEL_RESERVATIONS) {
    try {
      return new SharedArrayBuffer(INITIAL_CHANNEL_BYTES, { maxByteLength });
    } catch (error) {
      if (!(error instanceof RangeError)) throw error;
    }
  }
  return new SharedArrayBuffer(INITIAL_CHANNEL_BYTES, { maxByteLength: INITIAL_CHANNEL_BYTES });
}

/**
 * Worker side: posts `{type: 'call', operation, args}` and blocks until
 * the page writes the reply. Returns the value or throws
 * CapabilityError(code).
 */
export function waitOnChannel(channel, post, operation, args) {
  const control = new Int32Array(channel, 0, 2);
  Atomics.store(control, 0, IDLE);
  post({ type: 'call', operation, args });
  while (Atomics.load(control, 0) === IDLE) Atomics.wait(control, 0, IDLE);
  const length = Atomics.load(control, 1);
  // Copied out of shared memory: TextDecoder rejects shared views in browsers.
  const outcome = decodeReply(new Uint8Array(channel, HEADER_BYTES, length).slice());
  if (outcome.error !== undefined) throw new CapabilityError(outcome.error);
  return outcome.value;
}

/**
 * Page side: writes one reply and wakes the blocked worker. A value that
 * cannot be encoded is `platform` (`payload-too-large` when encoding ran out
 * of room), and a reply longer than MAX_REPLY_BYTES or than the channel can
 * grow to is `payload-too-large`, so the worker always wakes with an answer.
 */
export function replyOnChannel(channel, outcome) {
  let reply;
  try {
    reply = encodeReply(outcome);
  } catch (thrown) {
    reply = encodeReply({ error: thrown instanceof RangeError ? 'payload-too-large' : 'platform' });
  }
  if (reply.byteLength > Math.min(MAX_REPLY_BYTES, channel.maxByteLength - HEADER_BYTES)) {
    reply = encodeReply({ error: 'payload-too-large' });
  }
  if (channel.byteLength < HEADER_BYTES + reply.byteLength) {
    try {
      channel.grow(HEADER_BYTES + reply.byteLength);
    } catch {
      // The platform refused the memory: an error reply fits the initial channel.
      reply = encodeReply({ error: 'payload-too-large' });
    }
  }
  new Uint8Array(channel, HEADER_BYTES, reply.byteLength).set(reply);
  const control = new Int32Array(channel, 0, 2);
  Atomics.store(control, 1, reply.byteLength);
  Atomics.store(control, 0, READY);
  Atomics.notify(control, 0);
}

/**
 * The bridge of a worker session: `waitSync` blocks the worker on one
 * page operation; `resolveDynamic` is the loadDocument integration point.
 * @param {(operation: string, args: unknown[]) => unknown} wait
 */
export function workerBridge(wait) {
  return Object.freeze({
    /**
     * Blocks until the page's `operation` settles; returns its value or
     * throws CapabilityError (a closure cannot cross threads, so the page
     * operation is named, and the page runs its promise factory).
     */
    waitSync(operation, ...args) {
      return wait(operation, args);
    },
    /**
     * Resolves one runtime-computed URL through the page's `resources`
     * (allow, then await resolve). Throws the WIT import failure.
     */
    resolveDynamic(url) {
      try {
        return wait('resolve', [url]);
      } catch (thrown) {
        throw new ImportFailure(toComponentError(thrown));
      }
    },
  });
}

// --- errors across the worker boundary --------------------------------------

/** Serializes a session failure for the page. */
export function encodeError(error) {
  if (error instanceof ScxmlSessionError) {
    return {
      family: 'session',
      code: error.code,
      detail: error.detail,
      committedEffects: error.committedEffects,
      sessionFailed: error.sessionFailed,
    };
  }
  if (error instanceof ScxmlSnapshotError) return { family: 'snapshot', code: error.code };
  if (error instanceof ScxmlHostError) return { family: 'host', code: error.code };
  return { family: 'host', code: 'instance-lost' };
}

/** Rebuilds a session failure on the page. */
export function decodeError(record) {
  switch (record?.family) {
    case 'session':
      return new ScxmlSessionError(
        { code: record.code, detail: record.detail },
        { committedEffects: record.committedEffects, sessionFailed: record.sessionFailed },
      );
    case 'snapshot':
      return new ScxmlSnapshotError(record.code);
    case 'host':
      return new ScxmlHostError(record.code);
    default:
      return new ScxmlHostError('instance-lost');
  }
}

// --- page-side async session ------------------------------------------------

/**
 * A machine session whose methods return Promises (createSession with
 * `async: 'auto'`). Same methods, results, and errors as MachineSession;
 * one operation at a time: a call made before the previous one settled, or
 * from a capability provider the session is waiting on, rejects with
 * ScxmlHostError('reentrant-call').
 */
export class AsyncMachineSession {
  #backend;
  #mechanism;
  #busy = false;

  /** @internal use createSession */
  constructor(backend, mechanism) {
    this.#backend = backend;
    this.#mechanism = mechanism;
  }

  /** `worker` or `none` (see asyncMechanism). */
  get mechanism() {
    return this.#mechanism;
  }

  async #run(name, args) {
    if (this.#busy || this.#backend.serving()) throw new ScxmlHostError('reentrant-call');
    this.#busy = true;
    try {
      return await this.#backend.call(name, args);
    } finally {
      this.#busy = false;
    }
  }

  /** MachineSession.start. */
  start() {
    return this.#run('start', []);
  }
  /** MachineSession.send. */
  send(event) {
    return this.#run('send', [event]);
  }
  /** MachineSession.resume. */
  resume() {
    return this.#run('resume', []);
  }
  /** MachineSession.configuration. */
  configuration() {
    return this.#run('configuration', []);
  }
  /** MachineSession.status. */
  status() {
    return this.#run('status', []);
  }
  /** MachineSession.failure. */
  failure() {
    return this.#run('failure', []);
  }
  /** MachineSession.now. */
  now() {
    return this.#run('now', []);
  }
  /** MachineSession.nextDeadline. */
  nextDeadline() {
    return this.#run('nextDeadline', []);
  }
  /** MachineSession.advanceTime. */
  advanceTime(ms) {
    return this.#run('advanceTime', [ms]);
  }
  /** MachineSession.commands. */
  commands() {
    return this.#run('commands', []);
  }
  /** MachineSession.trace. */
  trace() {
    return this.#run('trace', []);
  }
  /** MachineSession.shutdown. */
  shutdown() {
    return this.#run('shutdown', []);
  }
  /** MachineSession.snapshot. */
  snapshot(options) {
    return this.#run('snapshot', [options]);
  }

  /** Releases the engine (and the worker); idempotent. */
  async dispose() {
    await this.#backend.dispose(this.#busy);
  }

  [Symbol.asyncDispose]() {
    return this.dispose();
  }
}
/** Backend of a `none` session: the synchronous session in the page. */
class InPageBackend {
  constructor(session) {
    this.session = session;
  }

  serving() {
    return false;
  }

  async call(name, args) {
    return this.session[name](...args);
  }

  async dispose() {
    this.session.dispose();
  }
}

/** Backend of a `worker` session. */
class WorkerBackend {
  #worker;
  #channel;
  #telemetry;
  #capabilities;
  #resources;
  #loader;
  #onTimer;
  #pending = new Map();
  #next = 0;
  #serving = false;
  #lost;
  /** Runtime URLs that reached the page's `allow`, and their resolved bytes. */

  constructor({ worker, channel, telemetry, capabilities, resources, loader, onTimer }) {
    this.#worker = worker;
    this.#channel = channel;
    this.#telemetry = telemetry;
    this.#capabilities = capabilities;
    this.#resources = resources;
    this.#loader = loader;
    this.#onTimer = onTimer;
    worker.addEventListener('message', (message) => this.#receive(message.data));
    worker.addEventListener('error', () => this.#lose());
  }

  /** Starts the worker session; rejects with the typed failure it reported. */
  init(options) {
    return new Promise((resolve, reject) => {
      this.#pending.set('init', { resolve, reject });
      try {
        this.#worker.postMessage({ type: 'init', channel: this.#channel, options });
      } catch {
        // An option that cannot be cloned is malformed.
        this.#pending.delete('init');
        this.#lose();
        reject(new ScxmlHostError('invalid-argument'));
      }
    });
  }

  serving() {
    return this.#serving;
  }

  call(name, args) {
    if (this.#lost !== undefined) return Promise.reject(new ScxmlHostError('instance-lost'));
    const id = (this.#next += 1);
    return new Promise((resolve, reject) => {
      try {
        this.#worker.postMessage({ type: 'op', id, name, args });
      } catch {
        // An argument that cannot be cloned is malformed; nothing was entered.
        reject(new ScxmlHostError('invalid-argument'));
        return;
      }
      this.#pending.set(id, { resolve, reject });
    });
  }

  /**
   * Disposes the worker session, then terminates the worker. A worker that
   * is blocked on the page (a page operation a timer-driven call waits on)
   * cannot read the request, and one that does not answer within
   * DISPOSE_GRACE_MS is not waited for: either is terminated at once.
   */
  async dispose(busy) {
    if (this.#lost === undefined && !busy && !this.#serving) {
      let timer;
      const grace = new Promise((resolve) => {
        timer = setTimeout(resolve, DISPOSE_GRACE_MS);
      });
      try {
        await Promise.race([this.call('dispose', []), grace]);
      } catch {
        // The worker is terminated below either way.
      } finally {
        clearTimeout(timer);
      }
    }
    this.#lose();
  }

  /** Terminates the worker; every pending call rejects with `instance-lost`. */
  #lose() {
    this.#lost ??= true;
    // A page operation still pending answers a worker that is gone.
    this.#serving = false;
    this.#worker.terminate();
    for (const { reject } of this.#pending.values()) reject(new ScxmlHostError('instance-lost'));
    this.#pending.clear();
  }

  #settle(id, error, value) {
    const pending = this.#pending.get(id);
    if (pending === undefined) return;
    this.#pending.delete(id);
    if (error === undefined) pending.resolve(value);
    else pending.reject(decodeError(error));
  }

  #receive(message) {
    switch (message?.type) {
      case 'ready':
        this.#settle('init', undefined, undefined);
        break;
      case 'init-failed':
        this.#settle('init', message.error);
        this.#lose();
        break;
      case 'result':
        this.#settle(message.id, message.error, message.value);
        break;
      case 'call':
        // #serve answers every call; if even its answer cannot be written
        // the worker would stay blocked, so the session is lost instead.
        this.#serve(message.operation, message.args).catch(() => this.#lose());
        break;
      case 'module':
        this.#module(message.id, message.component, message.file);
        break;
      case 'timer':
        this.#timer(message.outcome);
        break;
      default:
        break;
    }
  }

  /** Answers one blocking call through the channel, inside a `capability` span. */
  async #serve(operation, args) {
    const span = this.#telemetry.start('capability');
    this.#serving = true;
    let outcome;
    try {
      outcome = { value: await this.#perform(operation, args) };
    } catch (thrown) {
      // A runtime URL that fails to resolve, whatever the rejection (a
      // CapabilityError code included), is unreachable: `unavailable`
      // (error.execution), as for a prefetched URL.
      const code =
        operation === 'resolve' ? 'unavailable' : thrown instanceof CapabilityError ? thrown.code : 'platform';
      outcome = { error: code };
    } finally {
      this.#serving = false;
    }
    this.#telemetry.end(span, outcome.error === undefined ? 'ok' : outcome.error === 'platform' ? 'failed' : 'rejected');
    try {
      replyOnChannel(this.#channel, outcome);
    } catch {
      replyOnChannel(this.#channel, { error: 'platform' });
    }
  }

  async #perform(operation, args) {
    if (operation === 'resolve') {
      // A runtime URL has the bound of a prefetched one (src/resources.js):
      // at most MAX_RESOURCE_URL_BYTES long, or it is `unavailable`. Neither
      // the URLs one session resolves nor their total text are bounded.
      const [url] = args;
      const resources = this.#resources;
      if (resources === undefined || typeof url !== 'string') throw new CapabilityError('unavailable');
      if (byteLength(url, MAX_RESOURCE_URL_BYTES) === Infinity) throw new CapabilityError('unavailable');
      // `allow` and `resolve` get the canonical form (src/url.js).
      const canonical = allowedUrl(resources, url);
      if (canonical === undefined) throw new CapabilityError('unavailable');
      // An answer that is neither text nor `{text, mediaType?}` names no
      // document: `unavailable`, as for a prefetched URL.
      const resolved = resolvedDocument(await resources.resolve(canonical));
      if (resolved === undefined) throw new CapabilityError('unavailable');
      return resolved;
    }
    const target = this.#capabilities?.[operation];
    if (typeof target !== 'function') throw new CapabilityError('unavailable');
    return target.apply(this.#capabilities, args);
  }

  async #module(id, component, file) {
    try {
      this.#worker.postMessage({ type: 'module', id, module: await this.#loader(component, file) });
    } catch {
      this.#worker.postMessage({ type: 'module', id, failed: true });
    }
  }

  #timer(outcome) {
    try {
      this.#onTimer?.(outcome.error === undefined ? outcome : { error: decodeError(outcome.error) });
    } catch {
      // Contained, as for a synchronous session.
    }
  }
}

/**
 * createSession with `async: 'auto'` (called by machine.js). Accepts the
 * synchronous options, except that `dataModel` is a bundled data-model
 * name (`ecmascript`, `xpath`, or `null`), `now` is not accepted (a worker
 * session keeps its own clock), and `http` and `extensionComponents` are
 * not accepted (their providers and bound components cannot cross into a
 * worker), plus `workerUrl` (the module Worker entry;
 * default this package's src/async-worker.js). Static URLs in `resources`
 * are prefetched in the page under both mechanisms (src/resources.js); a
 * worker session gets the prefetched answers in `init`.
 * @param {object} options
 * @param {(options: object) => Promise<object>} createSyncSession
 * @param {number} maxIrBytes the synchronous session's IR bound: larger IR
 *   is never scanned or prefetched here (the session rejects it)
 * @returns {Promise<AsyncMachineSession>}
 */
export async function createAsyncSession(options, createSyncSession, maxIrBytes) {
  const {
    async: mode,
    dataModel,
    resources,
    now,
    telemetry: telemetryOption,
    capabilities,
    loadCoreModule,
    onTimer,
    fuel,
    workerUrl,
    http,
    extensionComponents,
    ...rest
  } = options;
  const telemetry = new Telemetry(telemetryOption);
  // Read once into a frozen copy: all the worker receives.
  const limits = rest.invokeLimits === undefined ? {} : invokeLimitsCopy(rest.invokeLimits);
  if (
    mode !== 'auto' ||
    !(rest.ir instanceof Uint8Array) ||
    (dataModel !== undefined && !DATA_MODELS.includes(dataModel)) ||
    !isResourcesOption(resources) ||
    now !== undefined ||
    http !== undefined ||
    extensionComponents !== undefined ||
    !isTelemetryOption(telemetryOption) ||
    (capabilities !== undefined && (capabilities === null || typeof capabilities !== 'object')) ||
    (loadCoreModule !== undefined && typeof loadCoreModule !== 'function') ||
    (onTimer !== undefined && typeof onTimer !== 'function') ||
    (workerUrl !== undefined && typeof workerUrl !== 'string' && !(workerUrl instanceof URL)) ||
    // Checked here, in the page: what crosses into a worker is a structured
    // clone, which drops symbol keys, non-enumerable properties, accessors,
    // and an object's class, so the worker could not see a rejected object.
    limits === undefined
  ) {
    telemetry.observe('create-session', 'rejected');
    throw new ScxmlHostError('invalid-argument');
  }
  const loader = loadCoreModule ?? (defaultLoader ??= fetchCoreModules());
  if (asyncMechanism() === 'none') {
    const model =
      dataModel === undefined
        ? undefined
        : await createDataModel(dataModel, { loadCoreModule: loader, telemetry: telemetryOption, fuel });
    const session = await createSyncSession({
      ...rest,
      // The copy read above, so the page's object is read only once.
      invokeLimits: rest.invokeLimits === undefined ? undefined : limits,
      dataModel: model,
      telemetry: telemetryOption,
      capabilities,
      resources,
      loadCoreModule: loader,
      onTimer,
      fuel,
      [BRIDGE]: resources === undefined ? undefined : UNAVAILABLE_BRIDGE,
    });
    return new AsyncMachineSession(new InPageBackend(session), 'none');
  }
  // Only cloneable facts cross; providers stay in the page and are called
  // through the blocking channel.
  const capabilityKeys = Object.keys(capabilities ?? {}).filter((key) => typeof capabilities[key] === 'function');
  const span = telemetry.start('create-session');
  // Static URLs are prefetched here, in the page, exactly as a synchronous
  // session prefetches them; the worker gets the answers as data, so its
  // loads (and their `platform` and `payload-too-large` answers) are the
  // synchronous session's.
  const prefetched =
    resources === undefined || rest.ir.byteLength > maxIrBytes
      ? undefined
      : (await telemetry.within(span, () => prefetchResources(rest.ir.slice(), resources, telemetry))).entries();
  // Bundlers resolve this pattern; an unbundled page whose import map
  // resolves `@opentelemetry/api` passes a bundled entry as `workerUrl`
  // (import maps do not apply inside workers).
  const worker = new Worker(workerUrl ?? new URL('./async-worker.js', import.meta.url), { type: 'module' });
  const backend = new WorkerBackend({
    worker,
    channel: createChannel(),
    telemetry,
    capabilities,
    resources,
    loader,
    onTimer,
  });
  try {
    await backend.init({
      ir: rest.ir,
      sessionId: rest.sessionId,
      timers: rest.timers,
      maxCommands: rest.maxCommands,
      trace: rest.trace,
      snapshot: rest.snapshot,
      dataModel,
      fuel,
      invoke: rest.invoke,
      // The validated copy, so a non-enumerable limit the page set
      // survives the structured clone.
      invokeLimits: limits,
      capabilityKeys,
      resources: resources !== undefined,
      prefetched,
      onTimer: onTimer !== undefined,
    });
  } catch (error) {
    telemetry.end(span, 'failed');
    throw error;
  }
  telemetry.end(span, 'ok');
  return new AsyncMachineSession(backend, 'worker');
}
