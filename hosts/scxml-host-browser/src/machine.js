// Runtime-IR machine sessions over the generic machine component.
//
// The host is a thin capability linker and lifecycle adapter, with the
// same division of work as the Wasmtime session host
// (hosts/scxml-host-wasmtime/src/session_host/):
// * it serves the caller's Machine IR bytes through `ir-source` after a size
//   bound, without interpreting them (the machine validates IR and fails
//   closed; src/invoke.js only reads it for a pre-warm hint);
// * it forwards every data-model function of `machine-capabilities`
//   one-to-one to the session's data model (a bundled component or a
//   page-supplied provider), with the host's own `_sessionid` and
//   `_ioprocessors` at initialization;
// * it links one SCXML Event I/O Processor and its own project clock per
//   session (src/io.js), forwards target classification, `<send>`, and
//   `<cancel>` to it with the session's sender facts, and executes the
//   processor's decision; delayed sends are delivered when their clock
//   deadline is reached, through `route-due`, driven by the browser event
//   loop (`timers: 'event-loop'`) or by the page (`timers: 'manual'`);
// * for `<invoke>`, it compiles child documents with a dedicated compiler
//   instance (src/invoke.js; `invoke.compile: 'auto'`, the default) or takes
//   the page's `compileInvocation` provider (pre-lowered child IR), builds
//   invoked sessions synchronously from preloaded core modules (an invoked
//   session may invoke in turn, at any depth within the invoke limits), and
//   runs every session's external event queue (src/invoked-sessions.js) in the
//   Wasmtime executor's order;
// * it links one run-wide BasicHTTP Event I/O Processor (src/basichttp.js):
//   `send-basichttp` goes through the component, which encodes the request
//   and hands it to the page's `http` capability only for URLs `http.allow`
//   accepts (deny-all by default); `receiveHttp` injects inbound requests the
//   component decodes;
// * it admits commands against an optional run-wide budget, records the
//   optional observation trace, and forwards `<log>` to the page;
// * it calls the machine's lifecycle exports in order;
// * at a checkpoint, it gathers the components' read-only `snapshot` records
//   and each session's external event queue into one capture and has the optional codec
//   component encode it (src/snapshot.js; `MachineSession.snapshot`).
// Transition selection, macrostep/microstep semantics, running each command
// to a stable configuration, IR decoding and validation, send ids, error events, the
// invocation lifecycle, and every other SCXML decision happen inside the
// components.
//
// Deliberately unsupported in the browser (each a typed error, never a
// silent no-op; see UNSUPPORTED):
// * `<invoke>` under `invoke.compile: 'provided'` without a
//   `compileInvocation` provider: `compile-invocation`
//   and `start-invocation` answer `execution` with detail
//   `unsupported-invoke`, which the machine turns into `error.execution`
//   (SCXML 6.4: the invocation fails and no `done.invoke` follows); no
//   child is then ever live, so `cancel-invocation` and
//   `forward-to-invocation` have nothing to address, the case the WIT
//   defines as success without effect;
// * `load-document` and `load-script` resolve only URLs the page resolves:
//   prefetched through the `resources` option (src/resources.js; only URLs
//   the page's `allow` accepts, before the machine runs), or supplied
//   synchronously through `capabilities.loadDocument`; the host never
//   fetches a document URI itself; in an `async: 'auto'` session a
//   runtime-computed URL the page does not supply waits on the page's
//   `resources` through src/async-bridge.js, and otherwise one neither
//   answers is `unavailable` with detail `dynamic-url-unavailable`.
//
// Every host call runs on one deterministic fuel budget (src/fuel.js,
// `fuel`) shared by every component it reaches: the data-model, I/O
// processor, clock, BasicHTTP, compiler, and child-machine calls a machine
// command makes are charged to that command's budget, each also capped by
// its own per-call fuel, exactly as the Wasmtime session host's call
// budget. A command that exhausts it throws ScxmlHostError('fuel-exhausted')
// instead of hanging the page, and the instance is never entered again.

import { instantiate } from '../dist/machine/machine.js';
import { BRIDGE, DYNAMIC_URL_UNAVAILABLE, createAsyncSession } from './async-bridge.js';
import {
  DATA_MODELS,
  createDataModel,
  instantiateDataModelSync,
  joinFuelBudget,
  joinSessionClock,
} from './datamodel.js';
import {
  CapabilityError,
  ImportFailure,
  ScxmlHostError,
  ScxmlSessionError,
  ScxmlSnapshotError,
  exportPayload,
  toComponentError,
} from './errors.js';
import { DEFAULT_FUEL, FuelBudget, FuelMeter, isFuel } from './fuel.js';
import { SESSION_INVOKED, SESSION_ROOT, createBasicHttpLink, isHttpOption } from './basichttp.js';
import { InvocationCompiler, isInvokeOption, staticInvocations } from './invoke.js';
import { IoFault, createIoLink, createIoLinkSync } from './io.js';
import { fetchCoreModules } from './loader.js';
import { InvokedSessions, ROOT, byCodePoint, invokeLimitsCopy } from './invoked-sessions.js';
import {
  PREFETCHED,
  ScriptLoads,
  isResourcesOption,
  prefetchResources,
  resolvedDocument,
} from './resources.js';
import { sha256Hex } from './sha256.js';
import { SessionNamespaces, isExtensionComponents, prepareExtensions } from './extension.js';
import { canonicalHttpUrl } from './url.js';
import {
  MAX_SNAPSHOT_INVOKE_DEPTH,
  MAX_SNAPSHOT_INVOKED_SESSIONS,
  SNAPSHOT_MODES,
  collection,
  encodeCapture,
  isDataModelSnapshot,
  hasDataModelRole,
  loadCodec,
  snapshotOptions,
  stripTraceContext,
} from './snapshot.js';
import { GUEST_MODULES, Telemetry, isTelemetryOption, scxmlTelemetryImport } from './telemetry.js';
import { isDataValue, isForeachResult, isOptionalDataValue, toWitEvent } from './values.js';

/** Largest Machine IR served through `ir-source`; equals the machine's own bound. */
export const MAX_MACHINE_IR_BYTES = 16 * 1024 * 1024;
/** Default `_sessionid`, matching the Wasmtime host driver. */
export const DEFAULT_SESSION_ID = 'machine-session';
/**
 * Most deliveries (delayed sends and cross-session messages) one command,
 * `advanceTime` call, or event-loop timer turn performs; the rest stay
 * pending for the next one. A per-call scheduling slice, not a resource
 * limit: it defers deliveries and never drops or fails them.
 */
export const MAX_DUE_DELIVERIES = 10_000;
/**
 * Unix epoch ms of logical time 0 of a `manual`-timer session's clock (the
 * Wasmtime host's batch start): 2015-09-01T00:00:00Z, the date of the SCXML
 * 1.0 Recommendation. ECMA-262 leaves the current time to the
 * implementation.
 */
export const BATCH_START_EPOCH_MS = 1_441_065_600_000;

/** How delayed sends are driven (the `timers` option of createSession). */
export const TIMER_MODES = Object.freeze(['event-loop', 'manual']);
/**
 * Static `component-error.detail` values of the capabilities this host
 * deliberately does not provide.
 */
export const UNSUPPORTED = Object.freeze({
  invoke: 'unsupported-invoke',
  dynamicUrl: DYNAMIC_URL_UNAVAILABLE,
});

/** `capabilities` keys a page may supply; any other function key is invalid. */
const CAPABILITY_KEYS = new Set([
  'log',
  'raise',
  'loadScript',
  'runScript',
  'loadDocument',
  'compileInvocation',
]);

let defaultLoader;

function componentFailure(code, detail) {
  return new ImportFailure({ version: 'v1', code, detail });
}

function unavailable() {
  return componentFailure('unavailable', 'capability is not provided by this host');
}

function invalidResult() {
  return componentFailure('platform', 'capability provider returned a malformed value');
}

/** The failure the machine receives when the run faulted. */
/** The `invalid-input` a `<send>` gets when its receiving session refuses its event. */
function notAccepted() {
  return componentFailure('invalid-input', "the receiving session's document does not accept the event's data");
}

function hostFailure() {
  return componentFailure('platform', 'session host failure');
}

/** The place of the session `links` in its run, for its BasicHTTP location. */
const sessionRole = (links) => (links.parent === undefined ? SESSION_ROOT : SESSION_INVOKED);

/** The host error a component trap maps to. */
function trapError(meter, cause) {
  return meter?.exhausted() ? new ScxmlHostError('fuel-exhausted') : new ScxmlHostError('instance-lost', { cause });
}

/**
 * Which live session `uri` addresses, seen from session `sender` (the
 * Wasmtime loopback's routes): `{tag: 'own'}`, `{tag: 'parent', invokeId}`
 * (the sender's own invoke id), or `{tag: 'child', invokeId}` for a live,
 * unfinished child; `undefined` when none answers. Locations compare in
 * their canonical form (src/url.js).
 */
function loopbackRoute(link, uri, sender) {
  const canonical = (value) => canonicalHttpUrl(value) ?? value;
  const target = canonical(uri);
  // A started invoked session's location was read when it started. A read
  // that throws, here or there (Run.indexLocation), answers no location, is
  // recorded once per session (Run.locationFailed), and is not made again
  // in the same turn by either path.
  const { run } = sender;
  const locationOf = (links) => {
    if (links.location !== undefined) return links.location;
    if (links.locationReadTurn === run.turn) return undefined;
    try {
      return link.location(links.sessionId, sessionRole(links));
    } catch {
      run.locationFailed(links);
      return undefined;
    }
  };
  const at = (links) => {
    const location = locationOf(links);
    return location !== undefined && canonical(location) === target;
  };
  if (at(sender)) return { tag: 'own' };
  if (sender.parentLinks !== undefined && at(sender.parentLinks)) {
    return { tag: 'parent', invokeId: sender.parent.invokeId };
  }
  const key = run.sessionAt(target);
  const child = key === undefined ? undefined : run.session(key);
  if (child !== undefined && !child.finished && child.instance.links.parentLinks === sender) {
    return { tag: 'child', invokeId: child.instance.links.parent.invokeId };
  }
  return undefined;
}

/**
 * Pushes `event`, sent by session `sender`, onto the external queue of the
 * parent or invoked session `route` names. A queue holds any number of
 * events.
 */
function enqueueRouted(run, route, event, sender) {
  if (route.tag === 'parent') run.invokedSessions.pushToParent(sender.key, { tag: 'event', val: event });
  else run.invokedSessions.pushExternal(run.invokedSessions.child(sender.key, route.invokeId), event);
}

/**
 * State one run shares: its sessions' external event queues,
 * the command budget, the trace, and the first host fault. Single-threaded;
 * nothing here is entered re-entrantly (MachineSession rejects a call back
 * into the session from a capability provider with `reentrant-call`).
 */
class Run {
  constructor({ telemetry, capabilities, resources, maxCommands, trace, modules, invocations, fuel, budget, basichttp, bridge, invokeLimits, startEpochMs, extensions }) {
    this.telemetry = telemetry;
    /**
     * The bound extension components, prepared for every session
     * (src/extension.js `prepareExtensions`, plus the snapshot `codec` when
     * one reads the session snapshot); undefined when none is bound.
     */
    this.extensions = extensions;
    /**
     * Unix epoch ms of logical time 0 of every session clock of the run:
     * what ECMAScript `Date` reads is this plus the logical time.
     */
    this.startEpochMs = startEpochMs;
    /** The run's BasicHTTP processor link (src/basichttp.js). */
    this.basichttp = basichttp;
    /**
     * `(links, sendId) => void`: reports the later failure of one of `links`'
     * fire-and-forget BasicHTTP sends (set by MachineSession).
     */
    this.lateFailure = undefined;
    this.capabilities = capabilities;
    /** The prefetched URL answers (src/resources.js), with the `resources` option. */
    this.resources = resources;
    /** The async bridge (`async: 'auto'` with `resources`); otherwise undefined. */
    this.bridge = bridge;
    this.maxCommands = maxCommands;
    this.traceEnabled = trace;
    /** `(component, file) => WebAssembly.Module`, when invocation is enabled. */
    this.modules = modules;
    /** The automatic child compiler (src/invoke.js), unless the page compiles children. */
    this.invocations = invocations;
    /** Per-call cap of every instance the run creates. */
    this.fuel = fuel;
    /** The fuel budget every host call of the run spends (src/fuel.js). */
    this.budget = budget;
    /** Every session's external event queue (src/invoked-sessions.js). */
    this.invokedSessions = new InvokedSessions(invokeLimits);
    /** The root session's links (set once the root is built). */
    this.rootLinks = undefined;
    /** The root's machine instance, for `accepts-event-data`. */
    this.rootInstance = undefined;
    /** Every started invoked session by its key (src/invoked-sessions.js). */
    this.children = new Map();
    /** Invoked sessions the run named so far (see nextInvokedSessionId). */
    this.invokedNamed = 0;
    /**
     * The shared logical clock, in ms: the root's clock, which every move
     * of time advances. An invoked session's own clock is moved to it only
     * before that session is used (SessionLinks.syncClock), so idle
     * sessions cost nothing when time moves, as under Wasmtime.
     */
    this.clockNow = 0;
    /**
     * Each started, unfinished invoked session by the canonical form of its
     * BasicHTTP location (read when it starts, and again before a lookup,
     * at most once a turn, when that read threw), for routing an external
     * request to it.
     */
    this.locations = new Map();
    /**
     * Keys of the started, unfinished invoked sessions whose location read
     * failed (the page's `http.location` threw): read again before a
     * lookup, at most once a turn.
     */
    this.unlocated = new Set();
    /**
     * Host calls made on any session of the run (`MachineSession.#exclusive`):
     * each command, delivery, or settle runs in one turn.
     */
    this.turn = 0;
    /**
     * The invocation request (`{kind: 'start' | 'cancel', ...}`) the
     * machine call that just suspended deferred to the host.
     */
    this.deferred = undefined;
    this.commands = 0;
    this.trace = [];
    this.fault = undefined;
  }

  /**
   * The `_sessionid` of the next session the run invokes: the root's, then
   * `.` and its ordinal among the run's invoked sessions in start order,
   * exactly as under Wasmtime (SCXML 5.10: "the system-generated id for
   * the current SCXML session"). Unique across the run and of a length
   * that does not grow with the nesting depth.
   */
  nextInvokedSessionId() {
    this.invokedNamed += 1;
    return `${this.rootLinks.sessionId}.${this.invokedNamed}`;
  }

  /** Records the run's first fault; later ones are consequences. */
  latch(error) {
    this.fault ??= error;
  }

  /** Takes the recorded fault, if any. */
  take() {
    const fault = this.fault;
    this.fault = undefined;
    return fault;
  }

  record(entry) {
    if (this.traceEnabled) this.trace.push(entry);
  }

  /** The started invoked session `key`, if any. */
  session(key) {
    return this.children.get(key);
  }

  /** Whether the invoked session `key` reached a top-level final state. */
  finished(key) {
    return this.session(key)?.finished ?? false;
  }

  /** Every started invoked session, in invocation order: `[key, child]`. */
  sessions() {
    return this.invokedSessions
      .keys()
      .map((key) => [key, this.children.get(key)])
      .filter(([, child]) => child !== undefined);
  }

  /** Removes the invoked session `key` and every session below it. */
  remove(key) {
    for (const gone of this.invokedSessions.remove(key)) {
      this.forgetLocation(this.children.get(gone));
      this.children.delete(gone);
    }
  }

  /**
   * Records that a read of the location of the session `links` threw this
   * turn: a failed `capability` span, once per session, and no other read
   * of it until the next turn.
   */
  locationFailed(links) {
    links.locationReadTurn = this.turn;
    if (!links.locationFailed) {
      links.locationFailed = true;
      this.telemetry.observe('capability', 'failed');
    }
  }

  /**
   * Indexes the BasicHTTP location of the started invoked session `child`.
   * A read that throws is recorded (`locationFailed`) and made again before
   * a lookup in a later turn (`sessionAt`).
   */
  indexLocation(child) {
    const { links } = child.instance;
    let location;
    try {
      location = this.basichttp.location(links.sessionId, SESSION_INVOKED);
    } catch {
      this.locationFailed(links);
      this.unlocated.add(links.key);
      return;
    }
    this.unlocated.delete(links.key);
    if (location === undefined) return;
    links.location = canonicalHttpUrl(location) ?? location;
    this.locations.set(links.location, links.key);
  }

  /**
   * The key of the started invoked session at the canonical location
   * `target`, after reading again every location whose read failed in an
   * earlier turn (so each is read at most once a turn).
   */
  sessionAt(target) {
    for (const key of [...this.unlocated]) {
      const child = this.children.get(key);
      if (child === undefined || child.finished) this.unlocated.delete(key);
      else if (child.instance.links.locationReadTurn !== this.turn) this.indexLocation(child);
    }
    return this.locations.get(target);
  }

  /** Drops the location index entry of `child`, if any (it finished or is removed). */
  forgetLocation(child) {
    if (child === undefined) return;
    this.unlocated.delete(child.instance.links.key);
    const { location } = child.instance.links;
    if (location !== undefined && this.locations.get(location) === child.instance.links.key) this.locations.delete(location);
  }
}

/**
 * Capability state of one machine session (the root or one invoked child):
 * its data model, I/O processor link, and place in the run.
 */
class SessionLinks {
  #irSha256;
  #dataModelName;

  constructor({ run, sessionId, parent, parentLinks, dataModel, dataModelName, io, ir }) {
    this.run = run;
    this.sessionId = sessionId;
    /** `{sessionId, invokeId}` of the invoking parent, for an invoked session. */
    this.parent = parent;
    /** The invoking session's links, for an invoked session. */
    this.parentLinks = parentLinks;
    /** This session's key in the run's invoked sessions (set when an invoked session registers). */
    this.key = ROOT;
    /** The sessions this one invoked, keyed by invoke id. */
    this.children = new Map();
    this.dataModel = dataModel;
    // The data model's stable name (`ecmascript`, `null`, or `xpath`), read
    // once by the caller, so a provider cannot rename itself mid-run.
    this.#dataModelName = dataModelName;
    this.io = io;
    /** Where this invoked session's own clock is; the root's is the shared clock. */
    this.clockAt = run.clockNow;
    /** The canonical BasicHTTP location this invoked session is routed by, once started. */
    this.location = undefined;
    /** Whether a read of this session's location threw (recorded once: Run.locationFailed). */
    this.locationFailed = false;
    /** The run turn of this session's last location read that threw. */
    this.locationReadTurn = undefined;
    // The data model reads this session's clock (`execution-host.now-ms`):
    // the run's start epoch plus the logical time.
    joinSessionClock(dataModel, () => run.startEpochMs + io.now());
    /** The exact Machine IR bytes this session serves (never mutated). */
    this.ir = ir;
    /** True once the machine initialized this session's data model. */
    this.dataModelInitialized = false;
    /** This session's scripts loaded from the prefetched resources. */
    this.scripts = run.resources === undefined ? undefined : new ScriptLoads(run.resources);
    /** This session's extension component instances (src/extension.js), once loaded. */
    this.namespaces = undefined;
    /**
     * The active configuration (`update-active-configuration`), kept only
     * when extension components are bound (their `in-state`).
     */
    this.configuration = new Set();
    /** This session's `machine-capabilities` (set by machineImports), for its extension components. */
    this.capabilities = undefined;
  }

  // The session as its extension components act on it (src/extension.js):
  // the same capabilities `<send>` and `<cancel>` use.
  classifyTarget(target) {
    return this.capabilities.classifyTarget(target);
  }
  send(request) {
    return this.capabilities.send(request);
  }
  cancel(sendId) {
    this.capabilities.cancel(sendId, undefined);
  }
  /** The session clock's time: the run's start epoch plus the logical time. */
  nowMs() {
    return this.run.startEpochMs + this.io.now();
  }
  /** Logical time from the clock's read-only snapshot. */
  snapshotNow() {
    return this.io.snapshotNow();
  }
  /** Pending delayed sends (read-only). */
  pendingSends() {
    return this.io.snapshot();
  }
  /** This session's external event queue, as the events it will process. */
  externalQueue() {
    return this.run.invokedSessions.external(this.key).map(queuedEvent);
  }

  /**
   * Moves an invoked session's own clock to the run's shared clock when it
   * is behind: done before every call into the session and before its due
   * sends are drained or read. A clock trap is an IoFault.
   */
  syncClock() {
    if (this.parentLinks === undefined) return;
    const now = this.run.clockNow;
    if (now > this.clockAt) {
      this.io.advance(now - this.clockAt);
      this.clockAt = now;
    }
  }

  /** SHA-256 of the IR bytes the session loaded, computed once. */
  irSha256() {
    this.#irSha256 ??= sha256Hex(this.ir);
    return this.#irSha256;
  }

  /** The linked data model's name, when it names itself. */
  dataModelName() {
    return this.#dataModelName;
  }

  /**
   * This session's addressing facts for the SCXML processor: only invoked
   * sessions that still run are live invocations, so a send to one that
   * finished is undeliverable (SCXML C.1: a session that "does not exist or
   * is inaccessible").
   */
  sender() {
    return {
      sessionId: this.sessionId,
      parentSessionId: this.parent?.sessionId,
      invocations: [...this.children]
        .filter(([, child]) => !child.finished)
        .map(([invokeId, child]) => ({ invokeId, sessionId: child.instance.links.sessionId }))
        .sort((left, right) => byCodePoint(left.invokeId, right.invokeId)),
    };
  }

  /** Latches an I/O processor trap as the run fault. */
  ioFault(fault) {
    this.run.latch(new ScxmlHostError(fault.code, { cause: fault.cause }));
    return hostFailure();
  }

  /** Latches a broken host-component contract. */
  invariant() {
    this.run.latch(new ScxmlHostError('host-invariant'));
    return hostFailure();
  }

  record(entry) {
    const parent = this.parent === undefined ? {} : { parent: this.parent.sessionId };
    this.run.record({ session: this.sessionId, ...parent, ...entry });
  }

  /**
   * `_ioprocessors`: the linked SCXML processor's type URI and aliases,
   * each with this session's location (SCXML 5.10, C.1), and, only when
   * the page configured `http`, the BasicHTTP processor's type URI and
   * aliases with this session's BasicHTTP location (C.2.3), as a canonical
   * JSON object with sorted keys.
   */
  ioProcessors() {
    const located = new Map();
    const scxmlLocation = this.io.sessionLocation(this.sessionId);
    for (const key of [this.io.typeUri(), ...this.io.aliases()]) located.set(key, scxmlLocation);
    const { basichttp } = this.run;
    if (basichttp.configured) {
      let location;
      try {
        location = basichttp.location(this.sessionId, sessionRole(this));
      } catch {
        throw componentFailure('platform', 'http.location failed');
      }
      for (const key of [basichttp.typeUri, ...basichttp.aliases]) located.set(key, location);
    }
    const entries = {};
    for (const key of [...located.keys()].sort()) entries[key] = { location: located.get(key) };
    return { tag: 'json-object', val: JSON.stringify(entries) };
  }
}

/** One machine component instance and its session resource. */
class Instance {
  constructor(session, links, meter) {
    this.session = session;
    this.links = links;
    this.meter = meter;
  }

  /**
   * Runs one call on the run's fuel budget, with every invocation request
   * it defers carried out (see callDeferred); failures are thrown raw.
   */
  call(body) {
    const outcome = callDeferred(this, body);
    if (outcome.thrown !== undefined) throw outcome.thrown;
    return outcome.value;
  }

  /** One call into this instance alone: `{value}` or `{thrown}`. */
  rawCall(body) {
    const { links } = this;
    if (links.parentLinks !== undefined) {
      try {
        links.syncClock();
      } catch (thrown) {
        if (thrown instanceof IoFault) return { thrown: links.ioFault(thrown) };
        throw thrown;
      }
      // Its pending deadline may change: the host reads it again.
      links.run.invokedSessions.touch(links.key);
    }
    try {
      const value = this.links.run.budget.run(this.meter, () => body(this.session));
      // A suspended call's continuation runs on what it left of its cap.
      if (value?.suspended === true) this.meter.suspendedLeft = this.meter.lastLeft;
      return { value };
    } catch (thrown) {
      return { thrown };
    }
  }

  dispose() {
    // The session ended (finished, failed, or cancelled): its extension
    // component instances are shut down exactly once.
    this.links.namespaces?.shutdown();
    this.session[Symbol.dispose]?.();
  }
}

/** One live invoked session of the root. */
class Child {
  constructor(instance) {
    this.instance = instance;
    this.finished = false;
    /** The child's `<donedata>` value for its `done.invoke`. */
    this.returnData = undefined;
    /** The carrier of the child's finishing batch, for its `done.invoke`. */
    this.doneTraceContext = undefined;
  }

  get io() {
    return this.instance.links.io;
  }
}

/**
 * Runs one child command. Returns `{callResult}`, or `{error, rejected}`: the
 * run's latched fault wins; a machine failure is the child's
 * ScxmlSessionError (`rejected` when it is `invalid-input` or `execution`,
 * the class a parent treats as a rejected child); a trap is a host error.
 * A call that stopped at the per-call command bound is a failure, as under
 * the Wasmtime scheduler.
 */
function runChild(run, child, body) {
  return childOutcome(run, child, callDeferred(child.instance, body));
}

/** runChild's outcome of a child call that ended as `{value}` or `{thrown}`. */
function childOutcome(run, child, { value: callResult, thrown }) {
  const fault = run.take();
  if (fault !== undefined) return { error: fault, rejected: false };
  if (thrown !== undefined) {
    const payload = exportPayload(thrown);
    if (payload === undefined) return { error: trapError(child.instance.meter, thrown), rejected: false };
    const failure = payload.error === undefined ? { error: payload } : payload;
    const rejected = failure.error.code === 'invalid-input' || failure.error.code === 'execution';
    return { error: new ScxmlSessionError(failure.error, failure), rejected };
  }
  if (callResult.status === 'finished' && !child.finished) {
    child.finished = true;
    run.invokedSessions.markFinished(child.instance.links.key);
    run.forgetLocation(child);
  }
  if (callResult.returnData !== undefined) child.returnData = callResult.returnData;
  if (callResult.doneTraceContext !== undefined) child.doneTraceContext = callResult.doneTraceContext;
  if (!callResult.stable && callResult.status === 'running') {
    const error = { version: 'v1', code: 'capacity-exhausted', detail: 'child-command-bound' };
    return { error: new ScxmlSessionError(error, { sessionFailed: true }), rejected: false };
  }
  return { callResult };
}

/**
 * Whether the machine session `key` of `run` admits `event` from another
 * session or the host (`session.accepts-event-data`, namespaces/event "At
 * run time"): true for a key that names no started machine session. A
 * pure call on the receiver's own fuel cap, charged to the run's budget
 * (nested inside a sender's call when a capability asks); a trap is
 * thrown raw.
 */
function acceptsEventData(run, key, event) {
  const instance = key === ROOT ? run.rootInstance : run.session(key)?.instance;
  if (instance === undefined) return true;
  const { meter } = instance;
  const suspendedLeft = meter.suspendedLeft;
  meter.suspendedLeft = undefined;
  try {
    return run.budget.run(meter, () => instance.session.acceptsEventData(event));
  } finally {
    meter.suspendedLeft = suspendedLeft;
  }
}

/**
 * acceptsEventData from inside a capability call: a receiver's trap is
 * latched as the run's fault (the sender's call then fails with it).
 */
function capabilityAccepts(run, key, event) {
  try {
    return acceptsEventData(run, key, event);
  } catch (thrown) {
    const instance = key === ROOT ? run.rootInstance : run.session(key)?.instance;
    run.latch(trapError(instance?.meter, thrown));
    throw hostFailure();
  }
}

/** A child call that returns no call result (`failure`): `{value}` or `{error}`. */
function queryChild(run, child, body) {
  try {
    const value = child.instance.call(body);
    const fault = run.take();
    return fault === undefined ? { value } : { error: fault };
  } catch (thrown) {
    return { error: run.take() ?? trapError(child.instance.meter, thrown) };
  }
}

/**
 * Builds the per-session `machine-capabilities` resource class. Every
 * method is synchronous (as the WIT requires), never re-entrant, and
 * converts provider exceptions into closed WIT errors.
 */
function capabilityResource(links) {
  const { run } = links;
  const { telemetry } = run;
  const root = links.parent === undefined;

  /** Runs one host-implemented capability inside a span of `operation`. */
  const observed = (operation, body) => {
    const span = telemetry.start(operation);
    try {
      const result = telemetry.within(span, body);
      telemetry.end(span, 'ok');
      return result;
    } catch (thrown) {
      const code = thrown instanceof ImportFailure ? thrown.payload.code : 'platform';
      telemetry.end(span, code === 'platform' || code === 'capacity-exhausted' ? 'failed' : 'rejected');
      throw thrown;
    }
  };

  /** Runs one call into the session's I/O processor; a trap latches a fault. */
  const io = (body) => {
    try {
      return body(links.io);
    } catch (thrown) {
      if (thrown instanceof IoFault) throw links.ioFault(thrown);
      const payload = exportPayload(thrown);
      if (payload === undefined) throw links.invariant();
      throw new ImportFailure(payload);
    }
  };

  /** Forwards one call to a page or data-model provider. */
  const forward = (provider, method, args, check) => {
    const target = provider?.[method];
    if (typeof target !== 'function') {
      telemetry.observe('capability', 'rejected');
      throw unavailable();
    }
    const span = telemetry.start('capability');
    let result;
    try {
      result = telemetry.within(span, () => target.apply(provider, args));
    } catch (thrown) {
      const error = toComponentError(thrown);
      telemetry.end(span, error.code === 'platform' ? 'failed' : 'rejected');
      throw new ImportFailure(error);
    }
    if (check !== undefined && !check(result)) {
      telemetry.end(span, 'failed');
      throw invalidResult();
    }
    telemetry.end(span, 'ok');
    return result;
  };

  /**
   * Forwards one pure data-model function: `undefined` (WIT `none`) when
   * no provider answers, the provider throws, or its value is malformed.
   */
  const forwardPure = (method, args, check) => {
    const provider = links.dataModel;
    const target = provider?.[method];
    if (typeof target !== 'function') return undefined;
    const span = telemetry.start('capability');
    try {
      const result = telemetry.within(span, () => target.apply(provider, args));
      if (!check(result)) {
        telemetry.end(span, 'failed');
        return undefined;
      }
      telemetry.end(span, 'ok');
      return result;
    } catch {
      telemetry.end(span, 'failed');
      return undefined;
    }
  };

  const dataModel = (method, args, check) => forward(links.dataModel, method, args, check);
  const optionalString = (value) => value === undefined || typeof value === 'string';
  const unsupportedInvoke = () => {
    telemetry.observe('invocation', 'rejected');
    return componentFailure('execution', UNSUPPORTED.invoke);
  };
  /**
   * The automatic compiler's child IR, as the Wasmtime host answers
   * `compile-invocation`: a rejected document is `invalid-input`
   * (error.execution in the invoking session); a compiler fault fails the run.
   */
  const compiledChild = (sourceName, source) => {
    const outcome = run.invocations.compile(sourceName, source);
    if (outcome.ir !== undefined) return outcome.ir;
    if (outcome.fault === undefined) throw componentFailure('invalid-input', 'the compiler rejected the child document');
    run.latch(outcome.fault);
    throw hostFailure();
  };

  return class MachineCapabilities {
    updateActiveConfiguration(exited, entered) {
      if (links.namespaces !== undefined) {
        for (const state of exited) links.configuration.delete(state);
        for (const state of entered) links.configuration.add(state);
      }
      // A data model without In() support needs no configuration.
      if (typeof links.dataModel?.updateActiveConfiguration !== 'function') return;
      dataModel('updateActiveConfiguration', [[...exited], [...entered]]);
    }
    evaluateCondition(expression, event) {
      return dataModel('evaluateCondition', [expression, event], (v) => typeof v === 'boolean');
    }
    evaluateValue(expression, event) {
      return dataModel('evaluateValue', [expression, event], isDataValue);
    }
    assign(location, source, placement, event, _context) {
      dataModel('assign', [location, source, placement, event]);
    }
    execute(script, event) {
      dataModel('execute', [script, event]);
    }
    evaluateLog(expression, event) {
      return dataModel('evaluateLog', [expression, event], (v) => typeof v === 'string');
    }
    raise(event) {
      forward(run.capabilities, 'raise', [event]);
    }

    // SCXML Event I/O Processor (SCXML 6.2, 6.3, C.1).
    classifyTarget(target) {
      const span = telemetry.start('capability');
      try {
        const result = telemetry.within(span, () => links.io.classifyTarget(target, links.sender()));
        telemetry.end(span, 'ok');
        return result;
      } catch (thrown) {
        // The pure function has no failure channel: the fault fails the
        // machine call when it returns.
        telemetry.end(span, 'failed');
        if (thrown instanceof IoFault) links.ioFault(thrown);
        else links.invariant();
        return { tag: 'invalid' };
      }
    }
    send(request) {
      return observed('capability', () => {
        let action;
        try {
          action = io((processor) => processor.dispatch(request, links.sender()));
        } catch (thrown) {
          // `unavailable` is an unreachable target: the machine raises
          // error.communication. Every other error goes back unchanged.
          if (thrown instanceof ImportFailure && thrown.payload.code === 'unavailable') {
            return { tag: 'undeliverable' };
          }
          throw thrown;
        }
        switch (action.tag) {
          case 'scheduled':
            return { tag: 'scheduled' };
          case 'local':
            return { tag: 'enqueue-external', val: action.val };
          case 'parent':
            // Only an invoked session has a parent to route to; its event
            // goes onto the parent's external queue (ignored once its
            // cancellation began, official case 252), when the parent's
            // document accepts it (a refusal is the sender's
            // error.execution: `invalid-input`).
            if (root) throw links.invariant();
            if (!capabilityAccepts(run, links.parentLinks.key, action.val)) throw notAccepted();
            run.invokedSessions.pushToParent(links.key, { tag: 'event', val: action.val });
            return { tag: 'routed' };
          case 'invocation': {
            const child = run.invokedSessions.child(links.key, action.val.invokeId);
            if (child !== undefined && !capabilityAccepts(run, child, action.val.event)) throw notAccepted();
            run.invokedSessions.pushExternal(child, action.val.event);
            return { tag: 'routed' };
          }
          default:
            throw links.invariant();
        }
      });
    }
    cancel(sendId, context) {
      observed('capability', () => io((processor) => processor.cancel(sendId, context)));
    }
    // BasicHTTP Event I/O Processor (SCXML C.2), mapped exactly as the
    // Wasmtime session host maps the component's outcome.
    // A delayed send is checked by the processor and held on the session
    // clock by the SCXML processor until it falls due (SCXML 6.2: "It MUST
    // dispatch the message only when the delay interval elapses."); the
    // due delivery posts it (`MachineSession.#routeDue`).
    sendBasichttp(request) {
      return observed('capability', () => {
        let sent;
        try {
          if (request.delayMs > 0n) {
            run.basichttp.check(request);
            io((processor) => processor.scheduleBasichttp(request));
            return { tag: 'scheduled' };
          }
          sent = run.basichttp.send(request, links, {
            onLateFailure: () => run.lateFailure?.(links, request.sendId),
          });
        } catch (thrown) {
          if (thrown instanceof IoFault) throw links.ioFault(thrown);
          switch (thrown?.payload?.code) {
            case 'invalid-input':
              throw componentFailure('invalid-input', 'basichttp send is invalid');
            case 'payload-too-large':
              throw componentFailure('payload-too-large', 'basichttp send exceeds its byte bound');
            case 'unavailable':
              // Denied, unreachable, or not accepted: error.communication.
              if (run.fault !== undefined || request.delayMs > 0n) throw hostFailure();
              return { tag: 'undeliverable' };
            default:
              // A scheduling failure goes back to the machine unchanged, as
              // under Wasmtime: `capacity-exhausted` (the clock has no room)
              // raises error.execution (SCXML 3.12.2), and `duplicate-id`
              // (an author send id already pending, SCXML 3.14) ends the
              // session.
              if (request.delayMs > 0n && thrown instanceof ImportFailure) throw thrown;
              throw links.invariant();
          }
        }
        if (run.fault !== undefined) throw hostFailure();
        return sent.own === undefined ? { tag: 'routed' } : { tag: 'enqueue-external', val: sent.own };
      });
    }

    // `<log>`: always accepted, at any length (the data model's own value
    // bound, a wasm build, bounds what it renders), and recorded in the
    // trace; the root's records also reach the page's `log` provider.
    log(label, text, _context) {
      links.record({ observation: 'log', label, text });
      if (root && typeof run.capabilities?.log === 'function') {
        forward(run.capabilities, 'log', [{ label, text }]);
      }
    }

    // Run budget and trace.
    admitCommand() {
      if (run.maxCommands !== undefined && run.commands >= run.maxCommands) {
        telemetry.observe('capability', 'rejected');
        throw componentFailure('capacity-exhausted', 'session command budget exhausted');
      }
    }
    observe(observation) {
      if (observation.tag === 'command-completed') {
        run.commands += 1;
        links.record({ observation: 'command' });
      } else if (observation.tag === 'raised') {
        links.record({ observation: 'raised', name: observation.val });
      } else if (observation.tag === 'processed') {
        links.record({ observation: 'processed', name: observation.val });
      } else {
        throw componentFailure('protocol', 'unknown machine observation');
      }
    }

    // Data model, forwarded one-to-one.
    dataModelName() {
      return links.dataModelName();
    }
    initializeDataModel(declarations, machineName) {
      if (links.dataModel === undefined) {
        telemetry.observe('capability', 'rejected');
        throw unavailable();
      }
      const ioProcessors = observed('capability', () => io(() => links.ioProcessors()));
      dataModel('initialize', [[...declarations], { sessionId: links.sessionId, machineName, ioProcessors }]);
      // The gather reports a never-initialized data model as unlinked
      // (docs/session-snapshot.md, "Session object"), like the Wasmtime
      // host's `data_model_initialized`.
      links.dataModelInitialized = true;
    }
    declare(id) {
      dataModel('declare', [id]);
    }
    initializeData(id, initializer) {
      dataModel('initializeData', [id, initializer]);
    }
    setParameter(name, value) {
      return dataModel('setParameter', [name, value], (v) => typeof v === 'boolean');
    }
    evaluateLocation(location, event) {
      return dataModel('evaluateLocation', [location, event], isDataValue);
    }
    evaluateText(expression, event) {
      return dataModel('evaluateText', [expression, event], optionalString);
    }
    evaluateFormValue(source, location, event) {
      return dataModel('evaluateFormValue', [source, location, event], isDataValue);
    }
    foreachItems(expression, event) {
      return dataModel('foreachItems', [expression, event], isForeachResult);
    }
    // Called after every command: forwarded untraced (no span or counter),
    // as the Wasmtime host forwards it; a provider that throws has nothing
    // left to forget.
    releaseForeach() {
      const provider = links.dataModel;
      if (typeof provider?.releaseForeach !== 'function') return;
      try {
        provider.releaseForeach();
      } catch {
        // Nothing to report: the call answers nothing.
      }
    }
    namelistKey(location) {
      return forwardPure('namelistKey', [location], optionalString);
    }
    eventPayload(entries) {
      return forwardPure('eventPayload', [entries], isOptionalDataValue);
    }
    inlineContent(content) {
      return forwardPure('inlineContent', [content], isOptionalDataValue);
    }
    externalData(text) {
      return forwardPure('externalData', [text], isOptionalDataValue);
    }
    eventDataField(data, name) {
      return forwardPure('eventDataField', [data, name], isOptionalDataValue);
    }
    acceptsEventData(data) {
      return forwardPure('acceptsEventData', [data], (value) => typeof value === 'boolean') ?? true;
    }

    // Documents: only what the page resolves; the host never fetches.
    loadDocument(kind, uri) {
      // --- resources (src/resources.js): a prefetched URL is answered from
      // the cache.
      if (run.resources?.has(uri)) return observed('capability', () => run.resources.document(uri));
      let answer;
      if (typeof run.capabilities?.loadDocument === 'function') {
        answer = forward(run.capabilities, 'loadDocument', [kind, uri], (v) => resolvedDocument(v) !== undefined);
      } else if (run.bridge !== undefined) {
        // Async bridge hook: on a cache miss, a URL the page does
        // not supply waits on the page's `resources` (allow, then resolve);
        // `run.bridge` is set only for `async: 'auto'` sessions with
        // `resources`. The answer still passes the document bound below.
        answer = observed('capability', () => run.bridge.resolveDynamic(uri));
      } else if (run.resources !== undefined) {
        // A runtime-computed URL the page does not supply is typed.
        telemetry.observe('capability', 'rejected');
        throw componentFailure('unavailable', UNSUPPORTED.dynamicUrl);
      } else {
        answer = forward(run.capabilities, 'loadDocument', [kind, uri], (v) => resolvedDocument(v) !== undefined);
      }
      // --- end resources
      // A string, or `{text, mediaType}` with the media type the page's
      // source indicated (SCXML B.2.1), as the WIT `loaded-document`.
      const document = resolvedDocument(answer);
      if (document === undefined) throw componentFailure('platform', 'document answer is not text');
      return { text: document.text, mediaType: document.mediaType };
    }

    // Invocation (SCXML 6.4): the page's pre-lowered child IR, else the
    // automatic compiler (src/invoke.js).
    compileInvocation(sourceName, source) {
      if (run.modules === undefined) throw unsupportedInvoke();
      const ir =
        run.invocations === undefined
          ? forward(run.capabilities, 'compileInvocation', [sourceName, source], (v) => v instanceof Uint8Array)
          : compiledChild(sourceName, source);
      if (ir.byteLength > MAX_MACHINE_IR_BYTES) {
        throw componentFailure('invalid-input', 'child machine IR exceeds its byte bound');
      }
      return ir;
    }
    // `request` is the WIT `invocation` variant. This host binds no
    // invoked-service component (invoked services are a Wasmtime host
    // feature), so every service `<invoke>`, literal or `typeexpr`, answers
    // `unavailable` when it starts, which fails that `<invoke>` with
    // `error.execution`: invocation is a runtime action (SCXML 6.4), so the
    // session start is unaffected.
    startInvocation(request) {
      if (request.tag === 'service') {
        telemetry.observe('capability', 'rejected');
        throw unavailable();
      }
      if (run.modules === undefined) throw unsupportedInvoke();
      return observed('invocation', () => requestStart(links, request.val));
    }
    cancelInvocation(invokeId) {
      if (!links.children.has(invokeId)) return 'completed';
      return observed('invocation', () => requestCancel(links, invokeId));
    }
    forwardToInvocation(invokeId, event) {
      // Only a live, unfinished invoked session receives autoforwarded events.
      const child = links.children.get(invokeId);
      if (child === undefined || child.finished) return;
      observed('capability', () => run.invokedSessions.pushExternal(run.invokedSessions.child(links.key, invokeId), event));
    }

    loadScript(reference) {
      // resources: an unpinned prefetched `<script src>` loads from the cache.
      if (links.scripts?.serves(reference)) {
        observed('capability', () => links.scripts.load(reference, links.dataModelName()));
        return;
      }
      forward(run.capabilities, 'loadScript', [reference]);
    }
    runScript(reference, event) {
      // resources: a script loaded from the cache runs as data-model text.
      const text = links.scripts?.text(reference);
      if (text !== undefined) {
        dataModel('execute', [text, event]);
        return;
      }
      forward(run.capabilities, 'runScript', [reference, event]);
    }
    // An executable-content extension, routed by its exact namespace URI
    // to this session's instance of the bound extension component
    // (src/extension.js); an unbound one fails the session closed.
    // `snapshot` is the machine's executable-content image, present exactly
    // when the component reads the session snapshot.
    executeExtension(element, event, context, snapshot) {
      const instance = links.namespaces?.get(element.name.namespaceUri);
      if (instance === undefined) {
        telemetry.observe('extension-execute', 'rejected');
        throw unavailable();
      }
      const span = telemetry.start('extension-execute');
      const executed = telemetry.within(span, () => instance.execute(element, event, context, snapshot));
      const handled = executed.outcome.tag === 'ok' && executed.outcome.val.tag === 'handled';
      telemetry.end(span, handled ? 'ok' : executed.outcome.tag === 'ok' ? 'rejected' : 'failed');
      if (run.fault !== undefined) throw hostFailure();
      return executed;
    }

    // Only a bound extension component that imports `session-snapshot-host`
    // reads the machine's executable-content image.
    extensionSnapshotRequest(namespaceUri) {
      return links.namespaces?.get(namespaceUri)?.readsSnapshot === true;
    }

    // Every namespace the document uses gets this session's instance of its
    // bound extension component; a namespace no binding names fails the
    // session start closed before data binding.
    loadExtensions(namespaceUris) {
      const span = telemetry.start('extension-load');
      try {
        for (const uri of namespaceUris) {
          if (links.namespaces?.has(uri)) continue;
          const prepared = run.extensions?.prepared.get(uri);
          if (prepared === undefined) throw unavailable();
          links.namespaces ??= new SessionNamespaces();
          try {
            links.namespaces.add(prepared, links);
          } catch (fault) {
            run.latch(fault);
            throw hostFailure();
          }
        }
        telemetry.end(span, 'ok');
      } catch (thrown) {
        telemetry.end(span, thrown instanceof ImportFailure && thrown.payload.code === 'unavailable' ? 'rejected' : 'failed');
        throw thrown;
      }
    }
  };
}

/** The machine instance's imports for one session. */
function machineImports(links, machineIr) {
  const { telemetry } = links.run;
  const irSource = {
    machineIr() {
      telemetry.observe('ir-source', 'ok');
      return machineIr;
    },
  };
  const MachineCapabilities = capabilityResource(links);
  links.capabilities = new MachineCapabilities();
  const imports = {};
  for (const [name, value] of [
    ['types', {}],
    ['telemetry', scxmlTelemetryImport(telemetry, GUEST_MODULES.machine)],
    ['ir-source', irSource],
    ['machine-capability-types', { MachineCapabilities }],
  ]) {
    // jco reads the unversioned key; the versioned key matches its typings.
    imports[`stateforward:scxml/${name}`] = value;
    imports[`stateforward:scxml/${name}@0.2.0`] = value;
  }
  return { imports, MachineCapabilities };
}

/**
 * Constructs and registers one invoked session inside its invoking
 * session's `start-invocation` call, and defers its start to the host
 * (`deferred`): the call suspends, callDeferred starts the child, and the
 * invoking session continues, so a chain of invocations never nests calls.
 * Its data model (the one the invoked document names), SCXML I/O processor
 * and clock (advanced to the shared time), and machine session are
 * instantiated synchronously from the preloaded core modules. Errors
 * follow the WIT: `duplicate-id`, `invalid-input` for a session rejected
 * at load or beyond the invoke bounds (the invoking session raises
 * `error.execution`); any other failure is a run fault.
 */
export function requestStart(links, request) {
  const { run } = links;
  // One call defers at most one request before it suspends.
  if (run.deferred !== undefined) throw links.invariant();
  if (links.children.has(request.invokeId)) {
    throw componentFailure('duplicate-id', 'invoke id is already live');
  }
  if (!run.invokedSessions.canInvoke(links.key)) {
    throw componentFailure('invalid-input', 'the invocation exceeds the invoked sessions bounds');
  }
  const rejected = () => componentFailure('invalid-input', 'the invoked session was rejected at load');
  if (request.machineIr.byteLength > MAX_MACHINE_IR_BYTES) throw rejected();
  const sessionId = run.nextInvokedSessionId();
  const { telemetry, modules: module, fuel, budget } = run;
  let meter;
  let instance;
  try {
    // A new session's clock starts at the shared clock.
    const now = run.clockNow;
    const io = createIoLinkSync({ telemetry, module, fuel, budget });
    if (now > 0) io.advance(now);
    const dataModel = instantiateDataModelSync(request.dataModel, { telemetry, module, fuel, budget });
    const childLinks = new SessionLinks({
      run,
      sessionId,
      parent: { sessionId: links.sessionId, invokeId: request.invokeId },
      parentLinks: links,
      dataModel,
      dataModelName: dataModel?.name,
      io,
      ir: request.machineIr,
    });
    const { imports, MachineCapabilities } = machineImports(childLinks, request.machineIr);
    meter = new FuelMeter(fuel);
    const session = budget.run(meter, () => {
      const component = instantiate((file) => module('machine', file), imports, meter.instantiateCoreSync);
      return new component.machineApi.Session(sessionId, new MachineCapabilities());
    });
    instance = new Instance(session, childLinks, meter);
  } catch (thrown) {
    if (thrown instanceof IoFault) throw links.ioFault(thrown);
    run.latch(trapError(meter, thrown));
    throw hostFailure();
  }
  const child = new Child(instance);
  const loaded = queryChild(run, child, (session) => session.failure());
  if (loaded.error !== undefined || loaded.value !== undefined) {
    child.instance.dispose();
    if (loaded.error === undefined) throw rejected();
    run.latch(loaded.error);
    throw hostFailure();
  }
  try {
    child.instance.links.key = run.invokedSessions.register(links.key, request.invokeId);
  } catch {
    // `canInvoke` held and the invoking session is live, so registration
    // cannot fail: a failure breaks the host's own contract.
    child.instance.dispose();
    throw links.invariant();
  }
  run.deferred = {
    kind: 'start',
    links,
    invokeId: request.invokeId,
    child,
    context: {
      parentSessionId: links.sessionId,
      invokeId: request.invokeId,
      parameters: request.parameters,
      // The child's start batch is the invoking batch's child.
      traceContext: request.traceContext,
    },
  };
  return 'deferred';
}

/**
 * Cancels one invoked session: from now on nothing it (or a session below
 * it) sends reaches its invoking session (official case 252); the rest is
 * deferred to the host (callDeferred): when unfinished its `cancel` runs
 * (cancelling the sessions it invoked as its own exit reaches them), then
 * it is dropped with its external queue, its timers, and every session
 * below it. Events it sent earlier stay queued.
 */
export function requestCancel(links, invokeId) {
  const { run } = links;
  // One call defers at most one request before it suspends.
  if (run.deferred !== undefined) throw links.invariant();
  const child = links.children.get(invokeId);
  links.children.delete(invokeId);
  run.invokedSessions.beginCancel(child.instance.links.key);
  run.deferred = { kind: 'cancel', child };
  return 'deferred';
}

/**
 * Runs `body` on `instance` and carries out every invocation request the
 * call (or a call it leads to) defers, iteratively: the invoking session
 * is suspended right after its `<invoke>` start or cancel, the host runs
 * the child's start or cancel here, not inside the invoking call, and the
 * invoking session continues (`invocationCompleted`) with the result the
 * request would have returned inside the call (SCXML 6.4). So a chain of
 * any depth takes no JavaScript or Wasm stack per level, and every session
 * runs exactly what it ran when requests were carried out inside the call.
 * The whole sequence is one host call of the fuel budget. Returns the
 * final `{value}` or `{thrown}` of `instance`'s call; a call result is the whole
 * command's (see oneStep).
 */
export function callDeferred(instance, body) {
  const { run } = instance.links;
  return run.budget.hold(() => {
    const suspended = [];
    let current = instance;
    let raw = current.rawCall(body);
    // Effects `instance` ran in the pieces of this command so far.
    let effects = 0;
    for (;;) {
      if (current === instance && typeof raw.value?.effectCount === 'number') effects += raw.value.effectCount;
      let nothingRan = false;
      if (raw.thrown === undefined && raw.value?.suspended === true) {
        const request = run.deferred;
        run.deferred = undefined;
        if (request === undefined) {
          run.latch(new ScxmlHostError('host-invariant'));
          raw = current.rawCall((session) => session.invocationCompleted({ tag: 'err', val: hostFailure().payload }));
          continue;
        }
        suspended.push({ owner: current, request });
        if (request.kind === 'start') {
          current = request.child.instance;
          raw = current.rawCall((session) => session.startInvoked(request.context));
          continue;
        }
        if (!request.child.finished) {
          current = request.child.instance;
          raw = current.rawCall((session) => session.cancel());
          continue;
        }
        nothingRan = true;
      } else {
        discardDeferred(run);
      }
      const frame = suspended.pop();
      if (frame === undefined) return oneStep(raw, effects);
      const outcome =
        frame.request.kind === 'start'
          ? completeStart(run, frame.request, raw)
          : completeCancel(run, frame.request, nothingRan ? {} : childOutcome(run, frame.request.child, raw));
      current = frame.owner;
      raw = current.rawCall((session) => session.invocationCompleted(outcome));
    }
  });
}

/**
 * The `{value}` of a command that ran in pieces (suspended at each deferred
 * `<invoke>`, then continued): its last call result, counting the effects of
 * every piece (each continuation's `effect-count` counts its own), without
 * the `suspended` flag, which concerns the host alone. Any other outcome
 * is returned as is.
 */
function oneStep(raw, effects) {
  if (raw.thrown !== undefined || typeof raw.value?.effectCount !== 'number') return raw;
  const { suspended: _, ...callResult } = raw.value;
  return { value: { ...callResult, effectCount: effects } };
}

/**
 * Drops the request a call deferred without suspending for it (the call
 * trapped or failed right after its `<invoke>`): a child built for a start
 * is deregistered and disposed, as the Wasmtime host discards it, so
 * nothing it holds stays in the run.
 */
function discardDeferred(run) {
  const request = run.deferred;
  run.deferred = undefined;
  if (request?.kind === 'start') {
    run.remove(request.child.instance.links.key);
    disposeInvokedSession(request.child);
  }
}

/**
 * The `start-invocation` result of a deferred start whose `start-invoked`
 * ended as `raw`: a child that rejected its start fails the `<invoke>`
 * (error.execution in the invoking session) and is dropped; any other
 * failure is a run fault.
 */
function completeStart(run, request, raw) {
  const started = childOutcome(run, request.child, raw);
  if (started.error !== undefined) {
    run.remove(request.child.instance.links.key);
    disposeInvokedSession(request.child);
    if (started.rejected) {
      return { tag: 'err', val: componentFailure('invalid-input', 'the invoked session rejected its start').payload };
    }
    run.latch(started.error);
    return { tag: 'err', val: hostFailure().payload };
  }
  request.links.children.set(request.invokeId, request.child);
  run.children.set(request.child.instance.links.key, request.child);
  if (!request.child.finished) run.indexLocation(request.child);
  return { tag: 'ok', val: undefined };
}

/** The `cancel-invocation` result of a deferred cancel whose child's cancel ended as `cancelled`. */
function completeCancel(run, request, cancelled) {
  run.remove(request.child.instance.links.key);
  disposeInvokedSession(request.child);
  if (cancelled.error !== undefined) {
    run.latch(cancelled.error);
    return { tag: 'err', val: hostFailure().payload };
  }
  return { tag: 'ok', val: undefined };
}

/** Disposes an invoked session and every session below it, deepest first, iteratively. */
function disposeInvokedSession(child) {
  const order = [];
  const pending = [child];
  while (pending.length > 0) {
    const next = pending.pop();
    order.push(next);
    pending.push(...next.instance.links.children.values());
  }
  for (const session of order.reverse()) {
    session.instance.links.children.clear();
    try {
      session.instance.dispose();
    } catch {
      // The instance is discarded either way: its destructor may trap on
      // the fuel a failed call left it (none), and nothing of the run
      // depends on it any more.
    }
  }
}

/**
 * The run's started invoked sessions in invocation order (`[key, child]`)
 * when one snapshot document can hold them: at most
 * MAX_SNAPSHOT_INVOKED_SESSIONS, none deeper than
 * MAX_SNAPSHOT_INVOKE_DEPTH, with the encoder's codes for them
 * (`bounds-exceeded`, `depth-exceeded`). Reads no session.
 */
export function withinDocumentBounds(run) {
  if (run.invokedSessions.size - 1 > MAX_SNAPSHOT_INVOKED_SESSIONS) throw new ScxmlSnapshotError('bounds-exceeded');
  const order = run.sessions();
  if (order.some(([key]) => run.invokedSessions.depth(key) > MAX_SNAPSHOT_INVOKE_DEPTH)) {
    throw new ScxmlSnapshotError('depth-exceeded');
  }
  return order;
}

/** One machine session (with its invoked children) bound to component instances. */
export class MachineSession {
  #instance;
  #run;
  #telemetry;
  #lost = false;
  /** Why the run was lost, when a host fault lost it (see #fail). */
  #loss;
  /**
   * Whether a session operation is in progress. Capability providers run
   * synchronously inside it; a call they make back into the session is
   * rejected (run to completion: one command at a time).
   */
  #busy = false;
  #status = 'ready';
  #stable = true;
  // Scheduling.
  #mode;
  #wallClock;
  #origin;
  #onTimer;
  #timer;
  /** Drained, not yet delivered root delayed sends, in clock order (at most one batch). */
  #pending = [];
  /** Whether the last pass stopped with cross-session work left. */
  #backlog = false;
  /** `'lazy'` or `'none'` (createSession `snapshot`). */
  #snapshotMode;
  /** The page's core-module loader the snapshot codec is loaded and cached with. */
  #codecLoader;
  /**
   * Failed sends (`{links, sendId}`: the sending session's links and the
   * `<send>`'s id) whose fire-and-forget BasicHTTP request failed later, in
   * order: each is owed one `error.communication`.
   */
  #late = [];
  #lateTimer;

  /** @internal use createSession */
  constructor(instance, run, scheduling, snapshot = { mode: 'none' }) {
    this.#instance = instance;
    this.#run = run;
    this.#telemetry = run.telemetry;
    this.#snapshotMode = snapshot.mode;
    this.#codecLoader = snapshot.loader;
    this.#mode = scheduling.mode;
    this.#wallClock = scheduling.wallClock;
    this.#origin = scheduling.wallClock();
    this.#onTimer = scheduling.onTimer;
    run.lateFailure = (links, sendId) => {
      this.#late.push({ links, sendId });
      this.#scheduleLate();
    };
  }

  get #links() {
    return this.#instance.links;
  }

  /** Marks the run lost (no instance is entered again) and returns `error`. */
  #fail(error) {
    this.#lost = true;
    this.#loss ??= error;
    this.#cancelTimer();
    this.#cancelLate();
    return error;
  }

  /**
   * Runs one public session operation to completion. A call that arrives
   * while another is in progress (a capability provider or data model
   * calling back into the session) throws ScxmlHostError('reentrant-call')
   * without touching any instance, observed as a rejected `operation`; the
   * operation in progress continues unaffected.
   */
  #exclusive(operation, body) {
    if (this.#busy) {
      this.#telemetry.observe(operation, 'rejected');
      throw new ScxmlHostError('reentrant-call');
    }
    this.#busy = true;
    if (this.#run !== undefined) this.#run.turn += 1;
    try {
      return body();
    } finally {
      this.#busy = false;
    }
  }

  /**
   * Throws when the run was lost while an operation that otherwise
   * succeeded was in progress, so a lost session never reports success.
   */
  #requireLive() {
    if (this.#lost) throw this.#loss ?? new ScxmlHostError('instance-lost');
  }

  #call(body) {
    if (this.#lost) {
      this.#telemetry.observe('command', 'rejected');
      throw new ScxmlHostError('instance-lost');
    }
    const span = this.#telemetry.start('command');
    let result;
    let failure;
    try {
      result = this.#telemetry.within(span, () => this.#instance.call(body));
    } catch (thrown) {
      failure = thrown;
    }
    // A run fault latched by a capability call wins: the machine only
    // echoes it as `platform`.
    const fault = this.#run.take();
    if (fault !== undefined) {
      this.#telemetry.end(span, 'failed');
      throw this.#fail(fault);
    }
    if (failure === undefined) {
      if (this.#lost) {
        this.#telemetry.end(span, 'failed');
        this.#requireLive();
      }
      this.#telemetry.end(span, 'ok');
      return result;
    }
    const payload = exportPayload(failure);
    if (payload === undefined) {
      // A trap or foreign exception: the instance state is unknown, so it
      // is never entered again.
      this.#telemetry.end(span, 'failed');
      throw this.#fail(trapError(this.#instance.meter, failure));
    }
    const sessionFailure = payload.error === undefined ? { error: payload } : payload;
    if (sessionFailure.sessionFailed) {
      this.#status = 'failed';
      this.#cancelTimer();
    }
    this.#telemetry.end(span, sessionFailure.sessionFailed ? 'failed' : 'rejected');
    throw new ScxmlSessionError(sessionFailure.error, sessionFailure);
  }

  /** Runs one root command that returns a `call-result` and tracks its status. */
  #rootCall(body) {
    const callResult = this.#call(body);
    this.#status = callResult.status;
    this.#stable = callResult.stable;
    return callResult;
  }

  /** acceptsEventData at executor level; a receiver's trap fails the run. */
  #accepts(key, event) {
    try {
      return acceptsEventData(this.#run, key, event);
    } catch (thrown) {
      const instance = key === ROOT ? this.#run.rootInstance : this.#run.session(key)?.instance;
      throw this.#fail(trapError(instance?.meter, thrown));
    }
  }

  /** Runs one invoked session's command at executor level; a failure fails the run. */
  #childAt(key, body) {
    const child = this.#run.session(key);
    if (child === undefined) throw this.#fail(new ScxmlHostError('host-invariant'));
    const span = this.#telemetry.start('command');
    const result = this.#telemetry.within(span, () => runChild(this.#run, child, body));
    this.#telemetry.end(span, result.error === undefined ? 'ok' : 'failed');
    if (result.error !== undefined) throw this.#fail(result.error);
  }

  /**
   * Runs one call into an I/O processor outside a machine command; a trap
   * or a broken contract fails the run.
   */
  #io(io, body) {
    try {
      return body(io);
    } catch (thrown) {
      if (thrown instanceof IoFault) throw this.#fail(new ScxmlHostError(thrown.code, { cause: thrown.cause }));
      throw this.#fail(new ScxmlHostError('host-invariant'));
    }
  }

  /**
   * Earliest pending deadline across the root and every unfinished invoked
   * session: the root's, and the earliest of the invoked sessions' index,
   * whose touched entries are read again first.
   */
  #earliest() {
    const root = this.#io(this.#links.io, (io) => io.nextDeadline());
    this.#refreshDeadlines();
    const invoked = this.#run.invokedSessions.earliestDeadline();
    if (invoked === undefined) return root;
    return root === undefined || invoked < root ? invoked : root;
  }

  /** Reads again the pending deadline of every invoked session a command touched. */
  #refreshDeadlines() {
    const run = this.#run;
    for (const key of run.invokedSessions.takeTouched()) {
      const child = run.session(key);
      const deadline = child === undefined || child.finished ? undefined : this.#io(child.io, (io) => io.nextDeadline());
      run.invokedSessions.record(key, deadline);
    }
  }

  /**
   * Advances the shared clock by `delta` ms: the root's clock now, every
   * invoked session's before it is next used.
   */
  #advance(delta) {
    if (!(delta > 0)) return;
    this.#io(this.#links.io, (io) => io.advance(delta));
    this.#run.clockNow += delta;
  }

  /**
   * `event-loop` mode: moves logical time up to the wall-clock time elapsed
   * since the session was created, never past the earliest pending
   * deadline (the timer delivers that one, in clock order).
   */
  #syncClock() {
    if (this.#mode !== 'event-loop' || this.#lost) return;
    const wall = Math.floor(this.#wallClock() - this.#origin);
    if (!Number.isSafeInteger(wall)) return;
    const now = this.#io(this.#links.io, (io) => io.now());
    const earliest = this.#earliest();
    this.#advance((earliest === undefined ? wall : Math.min(wall, earliest)) - now);
  }

  #cancelTimer() {
    if (this.#timer !== undefined) {
      clearTimeout(this.#timer);
      this.#timer = undefined;
    }
  }

  #cancelLate() {
    this.#late = [];
    if (this.#lateTimer !== undefined) {
      clearTimeout(this.#lateTimer);
      this.#lateTimer = undefined;
    }
  }

  /** Arms one event-loop turn that delivers the owed `error.communication` events. */
  #scheduleLate() {
    if (this.#lateTimer !== undefined || this.#late.length === 0) return;
    this.#lateTimer = setTimeout(() => {
      this.#lateTimer = undefined;
      this.#deliverLate();
    }, 0);
  }

  /**
   * Delivers each owed `error.communication` (a fire-and-forget BasicHTTP
   * send that failed after its `<send>` completed) on the sender's internal
   * event queue with `_event.type` `platform` and `_event.sendid` the failed
   * `<send>`'s id (SCXML 5.10.1: the platform raised it for a failed send;
   * 3.12.2: errors go "in the internal event queue"; 6.2.4: "If it is
   * unable to dispatch the message, the Processor MUST place the error
   * error.communication on the internal event queue"). The machine's
   * `communicationError` places it there and runs the macrostep that
   * processes it at once, dequeuing no external event (Appendix D mapping in
   * docs/live-session.md). Root and child senders alike, then cross-session
   * work settles, after the clock synced as for a page command; the page
   * hears the outcome through `onTimer`. Nothing is
   * delivered while an operation runs or the last call stopped at its
   * command bound (the next command re-arms it), and everything owed is
   * dropped once the session stops running.
   */
  #deliverLate() {
    if (this.#lost || this.#status !== 'running') {
      this.#late = [];
      return;
    }
    if (this.#busy || !this.#stable) return;
    let outcome;
    try {
      outcome = this.#exclusive('command', () => {
        // Like every command, the delivery runs at the current logical time.
        this.#syncClock();
        const results = [];
        const budget = { used: 0 };
        while (this.#late.length > 0 && this.#status === 'running' && this.#stable) {
          const { links, sendId } = this.#late.shift();
          if (links === this.#links) {
            results.push(this.#rootCall((session) => session.communicationError(sendId)));
          } else {
            const child = this.#run.session(links.key);
            if (child !== undefined && child.instance.links === links && !child.finished) {
              this.#childAt(links.key, (session) => session.communicationError(sendId));
            }
          }
          if (this.#run.modules !== undefined && !this.#settle(results, budget)) {
            this.#backlog = true;
            break;
          }
        }
        if (this.#status !== 'running') this.#late = [];
        this.#requireLive();
        return { results, complete: this.#late.length === 0 };
      });
    } catch (error) {
      outcome = { error };
    }
    this.#notify(outcome);
    this.#arm();
  }

  /** `event-loop` mode: arms one timer for the next pending delivery. */
  #arm() {
    this.#cancelTimer();
    if (this.#mode !== 'event-loop' || this.#lost || this.#status !== 'running' || !this.#stable) return;
    let delay;
    try {
      if (this.#pending.length > 0 || this.#backlog) {
        delay = 0;
      } else {
        const deadline = this.#earliest();
        if (deadline === undefined) return;
        delay = Math.max(0, deadline - this.#io(this.#links.io, (io) => io.now()));
      }
    } catch (error) {
      this.#notify({ error });
      return;
    }
    this.#timer = setTimeout(() => this.#fire(), delay);
  }

  #fire() {
    this.#timer = undefined;
    let outcome;
    try {
      const wall = Math.floor(this.#wallClock() - this.#origin);
      outcome = this.#exclusive('delayed-send', () => {
        const due = this.#pending.length > 0 || this.#backlog ? 0 : (this.#earliest() ?? 0);
        const delivery = this.#deliver(Number.isSafeInteger(wall) ? Math.max(wall, due) : due);
        this.#requireLive();
        return delivery;
      });
    } catch (error) {
      outcome = { error };
    }
    this.#notify(outcome);
    this.#arm();
  }

  /** Reports one timer-driven delivery to the page; its failures are contained. */
  #notify(outcome) {
    if (this.#onTimer === undefined) return;
    try {
      this.#onTimer(outcome);
    } catch {
      // Contained: page callback failures never change host behavior.
    }
  }

  /**
   * Queued work without time (the Wasmtime executor's results 1-4): queue
   * each finished invoked session's `done.invoke` on its invoking session,
   * after every event it sent, then process the oldest entry of the first
   * unfinished session, in invocation order, whose external queue holds one, until
   * nothing is left.
   * Returns false when it stopped early (a root call did not end stable, or
   * the delivery bound was reached).
   */
  #settle(results, budget) {
    const run = this.#run;
    for (;;) {
      if (this.#status !== 'running') return true;
      if (!this.#stable || budget.used >= MAX_DUE_DELIVERIES) return false;
      for (const key of run.invokedSessions.takeDoneDue()) {
        const child = run.session(key);
        // WIT `invocation-done`: the `<donedata>` and the finishing
        // batch's carrier, so `done.invoke` names the session's operation.
        const done = { data: child.returnData, traceContext: child.doneTraceContext };
        child.returnData = undefined;
        child.doneTraceContext = undefined;
        run.invokedSessions.pushToParent(key, { tag: 'done', val: done });
      }
      const next = run.invokedSessions.popNext((key) => key === ROOT || !run.finished(key));
      if (next !== undefined) {
        const [key, entry] = next;
        budget.used += 1;
        const body =
          entry.tag === 'external'
            ? (session) => session.send(entry.val)
            : (session) => session.deliverFromInvocation(entry.invokeId, entry.message);
        if (key === ROOT) results.push(this.#rootCall(body));
        else this.#childAt(key, body);
        continue;
      }
      return true;
    }
  }

  /**
   * Delivers everything due at or before logical time `target`, in the
   * Wasmtime scheduler's order: settle cross-session work, advance every
   * clock to the earliest deadline, deliver the root's due batch (each
   * routed through its processor at delivery time), then each unfinished
   * child's batch due at that deadline; stop once the root is no longer
   * running. Then time moves to `target`.
   */
  #deliver(target) {
    const span = this.#telemetry.start('delayed-send');
    const results = [];
    let complete;
    try {
      complete = this.#telemetry.within(span, () => this.#deliverUntil(target, results, { used: 0 }));
    } catch (error) {
      this.#telemetry.end(span, error instanceof ScxmlSessionError && !error.sessionFailed ? 'rejected' : 'failed');
      throw error;
    }
    this.#backlog = !complete;
    this.#telemetry.end(span, 'ok');
    return { results, complete };
  }

  #deliverUntil(target, results, budget) {
    const rootIo = this.#links.io;
    for (;;) {
      if (!this.#settle(results, budget)) return false;
      if (this.#status !== 'running') break;
      if (this.#pending.length > 0) {
        if (!this.#deliverRootDue(results, budget)) return false;
        continue;
      }
      const earliest = this.#earliest();
      if (earliest === undefined || earliest > target) break;
      const now = this.#io(rootIo, (io) => io.now());
      if (earliest < now) throw this.#fail(new ScxmlHostError('host-invariant'));
      this.#advance(earliest - now);
      const batch = this.#io(rootIo, (io) => io.drainDue());
      if (batch.rejected.length > 0) throw this.#fail(new ScxmlHostError('host-invariant'));
      this.#pending.push(...batch.delivered);
      if (!this.#deliverRootDue(results, budget)) return false;
      if (this.#status !== 'running') break;
      this.#deliverChildrenDue(earliest, budget);
    }
    const now = this.#io(rootIo, (io) => io.now());
    this.#advance(target - now);
    return true;
  }

  /** Delivers the root's drained due sends; false when it stopped early. */
  #deliverRootDue(results, budget) {
    while (this.#pending.length > 0) {
      if (this.#status !== 'running') return true;
      if (!this.#stable || budget.used >= MAX_DUE_DELIVERIES) return false;
      const due = this.#pending.shift();
      budget.used += 1;
      const route = this.#routeDue(this.#links, due);
      switch (route.tag) {
        case 'dispatched':
          break;
        // An undeliverable due send (SCXML 3.12.2, 6.2.4, C.1): the sender
        // raises error.communication on its internal event queue at once.
        case 'undeliverable':
          results.push(this.#rootCall((session) => session.communicationError(route.val)));
          break;
        case 'local':
          results.push(this.#rootCall((session) => session.send(route.val)));
          break;
        case 'invocation':
          this.#run.invokedSessions.pushExternal(this.#run.invokedSessions.child(ROOT, route.val.invokeId), route.val.event);
          break;
        default:
          // A root session has no parent to route to.
          throw this.#fail(new ScxmlHostError('host-invariant'));
      }
    }
    return true;
  }

  /**
   * Routes one due delayed send of session `links` at delivery time: an
   * SCXML send through the processor's `route-due`; a BasicHTTP send is
   * posted now (SCXML 6.2: "It MUST dispatch the message only when the
   * delay interval elapses."): `dispatched` when the recipient accepted it
   * (a loopback delivery to the parent or a child is already queued),
   * `local` for the session's own location, and `undeliverable` with the
   * send's id when it cannot be dispatched (6.2.4); a fetch that fails
   * later reports through the late-failure path with the same id.
   */
  #routeDue(links, due) {
    if (due.tag === 'scxml') {
      const route = this.#io(links.io, (io) => io.routeDue(due.val, links.sender()));
      // A delayed send to another session that its document does not
      // accept when it falls due is undeliverable: the sender's
      // error.communication (SCXML 6.2), as under the Wasmtime host.
      let receiver;
      let event;
      if (route.tag === 'parent') {
        receiver = links.parentLinks?.key;
        event = route.val;
      } else if (route.tag === 'invocation') {
        receiver = this.#run.invokedSessions.child(links.key, route.val.invokeId);
        event = route.val.event;
      }
      if (receiver !== undefined && !this.#accepts(receiver, event)) {
        return { tag: 'undeliverable', val: due.val.sendId };
      }
      return route;
    }
    if (due.tag !== 'basichttp') throw this.#fail(new ScxmlHostError('host-invariant'));
    const send = due.val;
    let sent;
    try {
      sent = this.#run.basichttp.send(send, links, {
        onLateFailure: () => this.#run.lateFailure?.(links, send.sendId),
      });
    } catch (thrown) {
      if (thrown instanceof IoFault) throw this.#fail(new ScxmlHostError(thrown.code, { cause: thrown.cause }));
      const fault = this.#run.take();
      if (fault !== undefined) throw this.#fail(fault);
      // Checked before it was scheduled, so only a delivery failure remains.
      if (thrown?.payload?.code === 'unavailable') return { tag: 'undeliverable', val: send.sendId };
      throw this.#fail(new ScxmlHostError('host-invariant'));
    }
    const fault = this.#run.take();
    if (fault !== undefined) throw this.#fail(fault);
    return sent.own === undefined ? { tag: 'dispatched' } : { tag: 'local', val: sent.own };
  }

  /** Delivers each unfinished invoked session's whole batch due at `deadline`, in invocation order. */
  #deliverChildrenDue(deadline, budget) {
    const run = this.#run;
    this.#refreshDeadlines();
    for (const key of run.invokedSessions.dueAt(deadline)) {
      const child = run.session(key);
      if (child === undefined || child.finished) continue;
      const links = child.instance.links;
      this.#io(child.io, () => links.syncClock());
      run.invokedSessions.touch(key);
      const batch = this.#io(child.io, (io) => io.drainDue());
      if (batch.rejected.length > 0) throw this.#fail(new ScxmlHostError('host-invariant'));
      for (const due of batch.delivered) {
        budget.used += 1;
        const route = this.#routeDue(child.instance.links, due);
        switch (route.tag) {
          case 'dispatched':
            break;
          case 'parent':
            this.#run.invokedSessions.pushToParent(key, { tag: 'event', val: route.val });
            break;
          case 'invocation':
            this.#run.invokedSessions.pushExternal(this.#run.invokedSessions.child(key, route.val.invokeId), route.val.event);
            break;
          case 'undeliverable':
            this.#childAt(key, (session) => session.communicationError(route.val));
            break;
          case 'local':
            this.#childAt(key, (session) => session.send(route.val));
            break;
          default:
            throw this.#fail(new ScxmlHostError('host-invariant'));
        }
      }
    }
  }

  /**
   * Runs one page command: syncs the clock first, then settles the
   * cross-session work it caused, and re-arms the timer after. Returns the
   * root's last call result.
   */
  #command(body) {
    try {
      return this.#exclusive('command', () => this.#commandStep(body));
    } finally {
      this.#scheduleLate();
    }
  }

  /**
   * The body of one page command (see #command); the caller holds the
   * session. Without `body` it only settles cross-session work (an event
   * queued for a child).
   */
  #commandStep(body) {
    let callResult;
    try {
      if (!this.#lost) this.#syncClock();
      if (body !== undefined) callResult = this.#rootCall(body);
      if (this.#run.modules !== undefined) {
        const results = [];
        this.#backlog = !this.#settle(results, { used: 0 });
        // The command's result: the last root call it made, counting the
        // effects of every one (its own and each delivery settling ran).
        if (results.length > 0) {
          const effectCount = results.reduce((sum, settled) => sum + settled.effectCount, callResult?.effectCount ?? 0);
          callResult = { ...results.at(-1), effectCount };
        }
      }
    } finally {
      this.#arm();
    }
    this.#requireLive();
    return callResult;
  }

  /** Runs one query of the root machine as an exclusive session operation. */
  #query(body) {
    return this.#exclusive('command', () => this.#call(body));
  }

  /**
   * Enters the initial configuration and runs to a stable configuration inside the
   * machine (status must be `ready`). `callResult.stable` is false only when the
   * machine stopped at its per-call command bound; call `resume`.
   */
  start() {
    return this.#command((session) => session.start());
  }

  /**
   * Processes one external event and runs to a stable configuration inside the machine
   * (status must be `running` and the previous call ended stable).
   * @param {string | object} event an event name or event fields
   */
  send(event) {
    const witEvent = toWitEvent(event);
    if (witEvent === undefined) {
      this.#telemetry.observe('command', 'rejected');
      throw new ScxmlHostError('invalid-argument');
    }
    // The page's own input meets the root document's contracts
    // (`accepts-event-data`) or is rejected as the machine rejects an
    // invalid event: the session is left as it was.
    if (this.#status === 'running' && !this.#accepts(ROOT, witEvent)) {
      this.#telemetry.observe('command', 'rejected');
      throw new ScxmlSessionError({
        code: 'invalid-input',
        detail: "the root document does not accept the event's data",
      });
    }
    return this.#command((session) => session.send(witEvent));
  }

  /**
   * Injects one inbound BasicHTTP request addressed to this (root) session
   * or one of its live invoked sessions at any depth, by its location in
   * canonical form (SCXML C.2; requires the `http` option). The BasicHTTP component
   * validates and decodes it exactly as under Wasmtime: the event name is
   * the first `_scxmleventname` form parameter, else header, else
   * `HTTP.POST`; `_event.data` holds the other form parameters (or the
   * body text); `_event.raw` is the message as received; `_event.origintype`
   * is the BasicHTTP type URI. The event is processed as an external event
   * before this returns, and the component chooses the response: 200 once
   * enqueued, 400/405/413 for a rejected message, 404 when `url` (this
   * session's location when absent) is neither this session's location nor
   * that of a live, unfinished invoked session at any depth, or the session
   * is not running, 503 while the last call stopped at its command bound.
   *
   * While a session's BasicHTTP send runs the page's `http.send`, the first
   * request the page routes straight back here (the in-process loopback) is
   * delivered as the Wasmtime loopback delivers it: to the sending session
   * itself (enqueued when the send returns), its parent, or one of its live
   * children, by location in its canonical form (src/url.js); any other
   * call during a session operation throws `reentrant-call`.
   * @param {{method: string, headers?: Record<string, string> | Iterable<[string, string]>,
   *   body?: string | Uint8Array, url?: string}} request
   * @returns {{status: number, headers: Record<string, string>, body: string}}
   * @throws {ScxmlHostError} `invalid-argument` (malformed request, or no
   *   `http` option), `reentrant-call`, `instance-lost`, or `fuel-exhausted`
   * @throws {ScxmlSessionError} when the machine rejects or fails the event
   */
  receiveHttp(request) {
    const link = this.#run.basichttp;
    let inbound;
    if (link.configured) {
      try {
        inbound = link.inboundRequest(request, link.location(this.#links.sessionId, SESSION_ROOT));
      } catch {
        inbound = undefined;
      }
    }
    if (inbound === undefined) {
      this.#telemetry.observe('command', 'rejected');
      throw new ScxmlHostError('invalid-argument');
    }
    const sender = this.#busy ? link.takeLoopback() : undefined;
    if (sender !== undefined) return this.#receiveLoopback(inbound, sender);
    return this.#exclusive('command', () => {
      if (this.#lost) {
        this.#telemetry.observe('command', 'rejected');
        throw new ScxmlHostError('instance-lost');
      }
      const respond = (delivery) => this.#io(link, (processor) => processor.respond(delivery));
      const route = this.#status === 'running' ? this.#externalRoute(inbound.uri) : undefined;
      if (route === undefined) return respond({ tag: 'unknown-session' });
      if (!this.#stable) return respond({ tag: 'queue-full' });
      const received = this.#io(link, (processor) => processor.receiveFromHeaders(inbound));
      if (received.event === undefined) return respond({ tag: 'rejected', val: received.rejection });
      if (route.tag === 'session') this.#run.invokedSessions.pushExternal(route.key, received.event);
      try {
        this.#commandStep(route.tag === 'own' ? (session) => session.send(received.event) : undefined);
      } finally {
        this.#scheduleLate();
      }
      return respond({ tag: 'enqueued' });
    });
  }

  /**
   * The running session an external request to `uri` addresses, as the
   * Wasmtime session host routes one: the root at its own location
   * (`{tag: 'own'}`), or any started, unfinished invoked session at its
   * location, at any depth (`{tag: 'session', key}`); `undefined` when
   * none answers. Locations compare in their canonical form; an invoked
   * session's is read when it starts (again before a lookup, at most once
   * a turn, when that read threw) and found by one lookup.
   */
  #externalRoute(uri) {
    if (this.#route(uri, this.#links)?.tag === 'own') return { tag: 'own' };
    const key = this.#run.sessionAt(canonicalHttpUrl(uri) ?? uri);
    const child = key === undefined ? undefined : this.#run.session(key);
    return child === undefined || child.finished ? undefined : { tag: 'session', key };
  }

  /** See `loopbackRoute`. */
  #route(uri, sender) {
    return loopbackRoute(this.#run.basichttp, uri, sender);
  }

  /** See `enqueueRouted`. */
  #enqueueRouted(route, event, sender) {
    enqueueRouted(this.#run, route, event, sender);
  }

  /**
   * The loopback delivery of session `sender`'s BasicHTTP send (see
   * `receiveHttp`), inside that send: the component decodes it, and it
   * becomes the send's own event, or is queued for the sender's parent or
   * live child, which the run delivers once the sending call ends. A trap
   * fails the run.
   */
  #receiveLoopback(inbound, sender) {
    const link = this.#run.basichttp;
    try {
      const route = this.#route(inbound.uri, sender);
      if (route === undefined) return link.respond({ tag: 'unknown-session' });
      const received = link.receiveLoopback(inbound);
      if (received.event === undefined) return link.respond({ tag: 'rejected', val: received.rejection });
      if (route.tag === 'own') link.deliverOwn(received.event);
      else this.#enqueueRouted(route, received.event, sender);
      return link.respond({ tag: 'enqueued' });
    } catch (thrown) {
      const error =
        thrown instanceof IoFault ? new ScxmlHostError(thrown.code, { cause: thrown.cause }) : new ScxmlHostError('host-invariant');
      this.#run.latch(error);
      throw error;
    }
  }

  /** Continues a machine whose previous call stopped at its command bound. */
  resume() {
    return this.#command((session) => session.resume());
  }

  /** Active leaf states in document order. */
  configuration() {
    return this.#query((session) => session.configuration());
  }

  /** Current status: ready | running | finished | failed | shut-down. */
  status() {
    return this.#query((session) => session.status());
  }

  /** The closed cause of a failed session, if any. */
  failure() {
    return this.#query((session) => session.failure());
  }

  /** Current logical time of the session's clock, in milliseconds. */
  now() {
    return this.#exclusive('command', () => this.#now());
  }

  #now() {
    if (this.#lost) throw new ScxmlHostError('instance-lost');
    return this.#io(this.#links.io, (io) => io.now());
  }

  /**
   * Deadline of the earliest pending delayed send across the session and
   * its unfinished children, in logical milliseconds; the current time
   * while drained sends or cross-session work are still pending.
   */
  nextDeadline() {
    return this.#exclusive('command', () => {
      if (this.#lost) throw new ScxmlHostError('instance-lost');
      if (this.#pending.length > 0 || this.#backlog) return this.#now();
      return this.#earliest();
    });
  }

  /**
   * `timers: 'manual'` only: advances logical time by `ms` milliseconds and
   * delivers everything that falls due, in clock order (delayed sends as
   * external events, and the cross-session work they cause). Returns the
   * root's results and whether the pass completed (`false` when a root call
   * stopped at its command bound, so `resume` must run first, or
   * MAX_DUE_DELIVERIES were delivered; call again to continue).
   * @param {number} ms a non-negative safe integer
   * @returns {{results: object[], complete: boolean}}
   */
  advanceTime(ms) {
    return this.#exclusive('delayed-send', () => {
      if (this.#mode !== 'manual' || !Number.isSafeInteger(ms) || ms < 0) {
        this.#telemetry.observe('delayed-send', 'rejected');
        throw new ScxmlHostError('invalid-argument');
      }
      if (this.#lost) {
        this.#telemetry.observe('delayed-send', 'rejected');
        throw new ScxmlHostError('instance-lost');
      }
      const target = this.#io(this.#links.io, (io) => io.now()) + ms;
      if (!Number.isSafeInteger(target)) {
        this.#telemetry.observe('delayed-send', 'rejected');
        throw new ScxmlHostError('invalid-argument');
      }
      const delivery = this.#deliver(target);
      this.#requireLive();
      return delivery;
    });
  }

  /**
   * Read-only canonical snapshot (`scxml.session-snapshot/v2`) of the
   * session and its invoked children, taken at a checkpoint
   * (docs/session-snapshot.md, "When a snapshot can be taken"): no session
   * operation in progress, the last call ended stable, no drained delayed send
   * or cross-session work pending, and a session that is neither failed nor
   * shut down. It applies no effects, charges no
   * command, records no trace entry, and moves no clock: every later result
   * is what it would have been without it. Invoked sessions follow the
   * root in invocation order (each after the session that invoked it,
   * siblings by ascending invoke id, at any depth, within the document's
   * 255-session and depth-32 bounds); excluded sections are never
   * collected. The optional codec component is loaded on the first call.
   * The document can hold data-model values and (under `preserve`) trace
   * carriers: never log or index it.
   * @param {{exclude?: string[], traceContext?: 'strip' | 'preserve'}} [options]
   * @returns {Promise<Uint8Array>}
   * @throws {ScxmlHostError} `invalid-argument`, `instance-lost`, or
   *   `fuel-exhausted`
   * @throws {ScxmlSnapshotError} `status-invalid` for a failed or shut-down
   *   session, `not-stable` outside a checkpoint, `codec-unavailable`,
   *   or an encoder code
   * @throws {ScxmlSessionError} when a component's read-only `snapshot`
   *   export rejects (the session is unaffected)
   */
  async snapshot(options) {
    const parsed = snapshotOptions(options);
    if (parsed === undefined) {
      this.#telemetry.observe('snapshot', 'rejected');
      throw new ScxmlHostError('invalid-argument');
    }
    this.#requireCheckpoint();
    if (this.#snapshotMode === 'none') {
      this.#telemetry.observe('snapshot', 'rejected');
      throw new ScxmlSnapshotError('codec-unavailable');
    }
    const codec = await loadCodec(this.#codecLoader);
    if (codec === undefined) {
      this.#telemetry.observe('snapshot', 'rejected');
      throw new ScxmlSnapshotError('codec-unavailable');
    }
    // Everything below is synchronous: no other session operation can run
    // between gathering the capture and encoding it.
    this.#requireCheckpoint();
    return this.#exclusive('snapshot', () => {
      const span = this.#telemetry.start('snapshot');
      try {
        const bytes = this.#telemetry.within(span, () =>
          encodeCapture(codec, this.#telemetry, this.#capture(parsed), parsed),
        );
        this.#telemetry.measure({ kind: 'snapshot-bytes', value: BigInt(bytes.byteLength) });
        this.#telemetry.end(span, 'ok');
        return bytes;
      } catch (error) {
        const rejected = error instanceof ScxmlSnapshotError || error instanceof ScxmlSessionError;
        this.#telemetry.end(span, rejected ? 'rejected' : 'failed');
        throw error;
      }
    });
  }

  /** Throws (observed as a rejected `snapshot`) unless the session is at a checkpoint. */
  #requireCheckpoint() {
    let error;
    if (this.#status === 'failed' || this.#status === 'shut-down') error = new ScxmlSnapshotError('status-invalid');
    else if (this.#lost) error = new ScxmlHostError('instance-lost');
    else if (this.#busy || !this.#stable || (this.#status === 'running' && (this.#pending.length > 0 || this.#backlog))) {
      // Drained sends left over when the root finished are never delivered
      // (the Wasmtime drain loop drops its local batch at the end of the
      // run), so a finished session is at a checkpoint.
      error = new ScxmlSnapshotError('not-stable');
    }
    if (error === undefined) return;
    this.#telemetry.observe('snapshot', 'rejected');
    throw error;
  }

  /**
   * Gathers one capture (WIT `capture` without `components`): the root,
   * then every invoked session at any depth, finished or not, in invocation
   * order (each after the session that invoked it, siblings by ascending
   * invoke id), with only the records the requested sections need. `dataModels`
   * names the data models of the captured sessions, exactly as the Wasmtime
   * host derives its data-model roles from each session's name.
   */
  #capture(options) {
    const gather = collection(options);
    const strip = options.traceContext === 'strip';
    const run = this.#run;
    const nowMs = this.#io(this.#links.io, (io) => io.snapshotNow());
    const dataModels = [];
    const capture = (instance, fields) => {
      const session = this.#gatherSession(instance, gather, fields);
      // A data-model role for every session whose data model the machine
      // initialized (the gather reports a never-initialized one as
      // unlinked), bundled or page-supplied: the name is present exactly then.
      if (session.dataModelName !== undefined) dataModels.push(session.dataModelName);
      return strip ? stripTraceContext(session) : session;
    };
    const external = (key) => (gather.queues ? run.invokedSessions.external(key).map(queuedEvent) : []);
    // Invocation order: each invoked session after the session that invoked
    // it, siblings in ascending invoke-id order, at any depth. The document
    // format's bounds are checked before any session is read (as the
    // Wasmtime host does), so a run of any size or depth fails at once and
    // the read-only snapshot never risks the run.
    const order = gather.children ? withinDocumentBounds(run) : [];
    const sessions = [capture(this.#instance, { parent: undefined, finished: false, external: external(ROOT) })];
    const indexes = new Map([[ROOT, 0]]);
    for (const [key, child] of order) {
      // One shared virtual clock: each session's own clock is moved to it
      // before it is read.
      this.#io(child.io, () => child.instance.links.syncClock());
      if (this.#io(child.io, (io) => io.snapshotNow()) !== nowMs) throw new ScxmlSnapshotError('clock-invariant');
      const parent = indexes.get(child.instance.links.parentLinks.key);
      indexes.set(key, sessions.length);
      sessions.push(capture(child.instance, { parent, finished: child.finished, external: external(key) }));
    }
    return { nowMs, sessions, dataModels };
  }

  /** One WIT `session-capture` from an instance's read-only exports. */
  #gatherSession(instance, gather, fields) {
    const { links } = instance;
    const machine = this.#readOnly(instance, (session) => session.snapshot(gather.machine));
    // A data model the machine never initialized is reported unlinked
    // (docs/session-snapshot.md, "Session object"): no image and no name.
    // An initialized one keeps its name (and so its data-model role) even
    // when `system` and `datamodel` are both excluded; only its read-only
    // export is skipped then, and for a `ready` session. This is exactly
    // the Wasmtime host's gather.
    let dataModel;
    let dataModelName;
    if (links.dataModel !== undefined && links.dataModelInitialized) {
      dataModelName = links.dataModelName();
      if (gather.dataModel && machine.status !== 'ready') dataModel = this.#dataModelSnapshot(links.dataModel);
    }
    const pendingSends = gather.pendingSends ? this.#pendingSends(links.io) : [];
    return {
      parent: fields.parent,
      sessionId: links.sessionId,
      irSha256: links.irSha256(),
      dataModelName,
      machine,
      dataModel,
      pendingSends,
      external: fields.external,
      finished: fields.finished,
    };
  }

  /**
   * Runs one read-only machine export. A component error is a
   * ScxmlSessionError (`not-stable` from the machine is
   * ScxmlSnapshotError); a trap loses the run.
   */
  #readOnly(instance, body) {
    let value;
    try {
      value = instance.call(body);
    } catch (thrown) {
      const payload = exportPayload(thrown);
      if (payload === undefined) throw this.#fail(trapError(instance.meter, thrown));
      if (payload.code === 'protocol' && payload.detail === 'not-stable') {
        throw new ScxmlSnapshotError('not-stable');
      }
      throw new ScxmlSessionError(payload);
    }
    const fault = this.#run.take();
    if (fault !== undefined) throw this.#fail(fault);
    return value;
  }

  /** The data model's read-only dump; a provider without `snapshot()` is `unavailable`. */
  #dataModelSnapshot(provider) {
    if (typeof provider.snapshot !== 'function') {
      throw new ScxmlSessionError({ version: 'v1', code: 'unavailable', detail: 'data-model-snapshot-unsupported' });
    }
    let value;
    try {
      value = provider.snapshot();
    } catch (thrown) {
      const code = thrown instanceof CapabilityError ? thrown.code : 'platform';
      const detail = thrown instanceof CapabilityError && typeof thrown.detail === 'string' ? thrown.detail : undefined;
      throw new ScxmlSessionError({ version: 'v1', code, detail });
    }
    if (!isDataModelSnapshot(value, isDataValue)) throw new ScxmlSnapshotError('schema-violation');
    return value;
  }

  /** The I/O processor's pending delayed sends; a trap loses the run. */
  #pendingSends(io) {
    try {
      return io.snapshot();
    } catch (thrown) {
      if (thrown instanceof IoFault) throw this.#fail(new ScxmlHostError(thrown.code, { cause: thrown.cause }));
      const payload = exportPayload(thrown);
      if (payload === undefined) throw this.#fail(new ScxmlHostError('host-invariant'));
      throw new ScxmlSessionError(payload);
    }
  }

  /** Engine commands the run completed (charged against `maxCommands`). */
  commands() {
    return this.#exclusive('command', () => this.#run.commands);
  }

  /**
   * The recorded observations (`trace: true`): commands, applied `<raise>`
   * events, and `<log>` records of the session and its children, in order,
   * every one of them.
   */
  trace() {
    return this.#exclusive('command', () => ({ entries: [...this.#run.trace] }));
  }

  /** Releases the engine and moves to `shut-down`; idempotent. */
  shutdown() {
    this.#exclusive('command', () => this.#shutdown());
  }

  #shutdown() {
    this.#cancelTimer();
    this.#cancelLate();
    this.#call((session) => session.shutdown());
    this.#status = 'shut-down';
  }

  /**
   * Shuts the session down (when the instance is usable) and drops every
   * component resource, children included; afterwards every call fails
   * with `instance-lost`.
   */
  dispose() {
    this.#exclusive('command', () => {
      this.#cancelTimer();
      if (this.#lost) return;
      try {
        this.#shutdown();
      } finally {
        this.#lost = true;
        for (const child of this.#links.children.values()) disposeInvokedSession(child);
        this.#links.children.clear();
        this.#instance.dispose();
      }
    });
  }

  [Symbol.dispose]() {
    this.dispose();
  }
}

/**
 * Whether `value` is an accepted `capabilities` option: every known
 * provider is a function when present, and no own function-valued property
 * names a capability this host does not take from the page (for example a
 * `send` or `cancel` provider: those are the linked I/O processor's), so a
 * page provider is never silently ignored.
 */
function isCapabilities(value) {
  if (value === undefined) return true;
  if (value === null || typeof value !== 'object') return false;
  for (const key of CAPABILITY_KEYS) {
    if (value[key] !== undefined && typeof value[key] !== 'function') return false;
  }
  return Object.entries(value).every(([key, provider]) => typeof provider !== 'function' || CAPABILITY_KEYS.has(key));
}

/**
 * Compiles every core module an invoked session is built from, so
 * `start-invocation` can instantiate synchronously. Instantiating each
 * bundled data model once records its modules; the session's own machine,
 * I/O processor, and clock instantiation (through the returned recording
 * loader) records theirs.
 */
async function preloadChildModules(loader, telemetry) {
  const compiled = new Map();
  const recording = (component, file) =>
    Promise.resolve(loader(component, file)).then((module) => {
      compiled.set(`${component}/${file}`, module);
      return module;
    });
  for (const name of DATA_MODELS) await createDataModel(name, { loadCoreModule: recording, telemetry });
  const module = (component, file) => {
    const found = compiled.get(`${component}/${file}`);
    if (found === undefined) throw new TypeError('core module was not preloaded');
    return found;
  };
  return { module, recording };
}

/**
 * Creates one runtime-IR machine session on a fresh generic machine
 * instance, with its own SCXML Event I/O Processor and clock.
 *
 * @param {object} options
 * @param {Uint8Array} options.ir lowered Machine IR bytes (untrusted; at most
 *   MAX_MACHINE_IR_BYTES; validated by the machine, never parsed here)
 * @param {object} [options.dataModel] data-model provider: a bundled one
 *   (createEcmaScriptDataModel, createXPathDataModel, createNullDataModel)
 *   or a page-supplied one; one per session. Its `name`, read once, is
 *   absent or one of `ecmascript`, `null`, `xpath` (otherwise
 *   `invalid-argument`)
 * @param {object} [options.capabilities] `log` (`{label, text}`),
 *   `loadDocument` (`(kind, uri) => text`), `compileInvocation`
 *   (`(sourceName, source) => Uint8Array`, pre-lowered child IR; overrides
 *   the automatic child compiler), `raise`, `loadScript`,
 *   `runScript` providers; any other function key is invalid
 * @param {object} [options.extensionComponents] an ExtensionComponents
 *   (src/extension.js): the extension components of the namespaces the
 *   document and every document it invokes use
 * @param {{compile?: 'auto' | 'provided', fuel?: number}} [options.invoke]
 *   `<invoke>` child compilation: `auto` (default) compiles children with a
 *   dedicated bundled compiler (src/invoke.js), pre-warmed before `start`;
 *   `provided` uses only `capabilities.compileInvocation`. `fuel` is the
 *   per-compile budget (default DEFAULT_COMPILE_FUEL)
 *   (`(sourceName, source) => Uint8Array`, pre-lowered child IR; enables
 *   `<invoke>`), `raise`, `loadScript`, `runScript`
 *   providers; any other function key is invalid
 * @param {object} [options.resources] `{allow, resolve}`: the page's URL
 *   policy and resolver (src/resources.js). Every statically known
 *   `<data src>`, `<script src>`, and `<invoke src>` URL that `allow(url)`
 *   accepts (exactly `true`) is resolved through `resolve(url)` (a promise
 *   of its text) before the machine runs; absent, no URL is resolved
 *   (deny-all)
 * @param {object} [options.telemetry] OpenTelemetry providers
 *   `{tracerProvider?, meterProvider?}` (OpenTelemetry JS API objects);
 *   omitted ones are the global API providers the page registered
 * @param {string} [options.sessionId] `_sessionid` (default
 *   DEFAULT_SESSION_ID)
 * @param {'event-loop' | 'manual'} [options.timers] how delayed sends are
 *   delivered (default `event-loop`)
 * @param {object} [options.http] the BasicHTTP capability
 *   `{allow, send?, location?}`: `allow(url)` must return exactly `true`
 *   for a `<send>` to reach `send(request)` (default `fetch`, no
 *   credentials, no redirects); every other URL raises
 *   `error.communication`, as does every BasicHTTP send without this
 *   option. `location` (a string for the root, or `(sessionId) => string`
 *   for every session) is the BasicHTTP location `_ioprocessors`
 *   advertises, else `http://basichttp.loopback.invalid/sessions/<id>/basichttp`;
 *   the processor is advertised only with this option
 * @param {Function} [options.onTimer] called with `{results, complete}` or
 *   `{error}` after each asynchronous delivery: an `event-loop` timer turn,
 *   or an `error.communication` owed by a fire-and-forget BasicHTTP send
 *   that failed later
 * @param {Function} [options.now] `event-loop` mode: monotonic milliseconds
 *   (default `performance.now`)
 * @param {number} [options.maxCommands] run-wide engine command budget
 * @param {boolean} [options.trace] record the observation trace
 * @param {Function} [options.loadCoreModule] `(component, file) => Module`
 *   loader; defaults to fetching this package's `dist/`
 * @param {number} [options.fuel] fuel of one host call (default
 *   DEFAULT_FUEL), shared by every component the call reaches, and the
 *   per-call cap of every instance the run creates
 * @param {'lazy' | 'none'} [options.snapshot] `lazy` (default) loads the
 *   optional snapshot codec through `loadCoreModule` on the first
 *   `snapshot()`; `none` never loads it (`snapshot()` then fails with
 *   `codec-unavailable`)
 * @param {'off' | 'auto'} [options.async] `off` (default): this synchronous
 *   session. `auto`: an AsyncMachineSession (src/async-bridge.js) whose
 *   capability calls can wait on the page's Promises (`resources`)
 * @returns {Promise<MachineSession>}
 * @throws {ScxmlHostError} `invalid-argument`, `machine-ir-too-large`,
 *   `fuel-exhausted`, or `instance-lost` when instantiation fails
 * @throws {ScxmlSessionError} when the machine rejects the IR or session id
 *   (for example `unsupported-version` or `invalid-input`), or when a
 *   `loadScript` call fails (the document is rejected); the session is
 *   already shut down
 */
export async function createSession(options) {
  // Async bridge hook: `async: 'auto'` sessions are built by
  // src/async-bridge.js; `'off'` (the default) is this synchronous session.
  if (options?.async !== undefined && options.async !== 'off') return createAsyncSession(options, createSession, MAX_MACHINE_IR_BYTES);
  const {
    ir,
    dataModel,
    capabilities,
    resources,
    telemetry: telemetryOption,
    sessionId = DEFAULT_SESSION_ID,
    timers = 'event-loop',
    onTimer,
    now = () => performance.now(),
    maxCommands,
    trace = false,
    loadCoreModule,
    fuel = DEFAULT_FUEL,
    snapshot = 'lazy',
    http,
    invoke,
    invokeLimits,
    extensionComponents,
  } = options ?? {};
  const telemetry = new Telemetry(telemetryOption);
  const isProvider = dataModel === undefined || (dataModel !== null && typeof dataModel === 'object');
  // Read once: the name selects the machine's data-model semantics and the
  // snapshot's data-model role, so it is one of the closed names or absent.
  const dataModelName = isProvider ? dataModel?.name : undefined;
  // Read once into a frozen copy, which is all the session ever reads.
  const limits = invokeLimits === undefined ? {} : invokeLimitsCopy(invokeLimits);
  if (
    !(ir instanceof Uint8Array) ||
    typeof sessionId !== 'string' ||
    !isProvider ||
    (dataModelName !== undefined && !hasDataModelRole(dataModelName)) ||
    !isCapabilities(capabilities) ||
    !isResourcesOption(resources) ||
    !TIMER_MODES.includes(timers) ||
    (onTimer !== undefined && typeof onTimer !== 'function') ||
    typeof now !== 'function' ||
    (maxCommands !== undefined && !(Number.isSafeInteger(maxCommands) && maxCommands > 0)) ||
    typeof trace !== 'boolean' ||
    !isFuel(fuel) ||
    !SNAPSHOT_MODES.includes(snapshot) ||
    !isHttpOption(http) ||
    !isInvokeOption(invoke) ||
    !isExtensionComponents(extensionComponents) ||
    limits === undefined ||
    (loadCoreModule !== undefined && typeof loadCoreModule !== 'function') ||
    !isTelemetryOption(telemetryOption)
  ) {
    telemetry.observe('create-session', 'rejected');
    throw new ScxmlHostError('invalid-argument');
  }
  if (ir.byteLength > MAX_MACHINE_IR_BYTES) {
    telemetry.observe('ir-source', 'rejected');
    throw new ScxmlHostError('machine-ir-too-large');
  }
  const span = telemetry.start('create-session');
  // One budget per host call, shared by every component the session reaches
  // (src/fuel.js); a bundled data model's calls join it.
  const budget = new FuelBudget(fuel);
  joinFuelBudget(dataModel, budget);
  // The host owns an immutable copy, taken before any await: the caller may
  // reuse its buffer, and the prefetch scans the bytes the machine loads.
  const irCopy = ir.slice();
  // resources: resolve every statically known URL before the machine runs
  // (an async worker session gets the page's prefetch instead, PREFETCHED).
  const resourceCache =
    options[PREFETCHED] ??
    (resources === undefined
      ? undefined
      : await telemetry.within(span, () => prefetchResources(irCopy, resources, telemetry)));
  const pageLoader = loadCoreModule ?? (defaultLoader ??= fetchCoreModules());
  let loader = pageLoader;
  let modules;
  let invocations;
  // Extension components: every binding's core modules are compiled now, so
  // each session instantiates its own instance synchronously; one that reads
  // the session snapshot needs the codec loaded too.
  let extensions;
  try {
    extensions = await prepareExtensions(extensionComponents, pageLoader);
    if (extensions !== undefined && [...extensions.prepared.values()].some((prepared) => prepared.readsSnapshot)) {
      extensions.codec = await loadCodec(pageLoader);
    }
  } catch (cause) {
    telemetry.end(span, 'failed');
    throw cause instanceof ScxmlHostError ? cause : new ScxmlHostError('instance-lost', { cause });
  }
  // <invoke>: the page's compileInvocation wins; otherwise, in
  // `auto` mode, a document that declares <invoke> gets the automatic
  // compiler, pre-warmed with its statically known children.
  const pageCompiles = typeof capabilities?.compileInvocation === 'function';
  const found = pageCompiles || (invoke?.compile ?? 'auto') !== 'auto' ? undefined : staticInvocations(irCopy);
  if (pageCompiles || found?.invokes) {
    try {
      const preloaded = await preloadChildModules(loader, telemetryOption);
      loader = preloaded.recording;
      modules = preloaded.module;
      if (found !== undefined) {
        const options = {
          loadCoreModule: loader,
          telemetry: telemetryOption,
          hostTelemetry: telemetry,
          fuel: invoke?.fuel,
          budget,
          packages: extensions?.packages,
        };
        invocations = await InvocationCompiler.open(options);
        await invocations.prewarm(found.documents);
      }
    } catch (cause) {
      telemetry.end(span, 'failed');
      throw cause instanceof ScxmlHostError ? cause : new ScxmlHostError('instance-lost', { cause });
    }
  }
  let io;
  let basichttp;
  try {
    io = await createIoLink({ telemetry, loader, fuel, budget });
    basichttp = await createBasicHttpLink({ telemetry, loader, fuel, budget, http });
  } catch (fault) {
    telemetry.end(span, 'failed');
    throw new ScxmlHostError(fault.code ?? 'instance-lost', { cause: fault.cause });
  }
  const run = new Run({
    telemetry,
    capabilities,
    resources: resourceCache,
    maxCommands,
    trace,
    modules,
    invocations,
    fuel,
    budget,
    basichttp,
    bridge: options[BRIDGE],
    invokeLimits: limits,
    extensions,
    // `event-loop` timers follow the page's clock: logical time 0 is the
    // wall-clock time the session was created; `manual` timers are logical
    // only and start at the fixed batch epoch, as the Wasmtime host's batch
    // runs do.
    startEpochMs: timers === 'event-loop' ? Date.now() : BATCH_START_EPOCH_MS,
  });
  // A post into the loopback address space is delivered in-process to the
  // session it addresses, as the Wasmtime loopback delivers it.
  basichttp.setLoopback({
    route: (uri, sender) => loopbackRoute(basichttp, uri, sender),
    enqueue: (route, event, sender) => enqueueRouted(run, route, event, sender),
  });
  const links = new SessionLinks({ run, sessionId, parent: undefined, dataModel, dataModelName, io, ir: irCopy });
  run.rootLinks = links;
  const { imports, MachineCapabilities } = machineImports(links, irCopy);

  const meter = new FuelMeter(fuel);
  let component;
  try {
    component = await instantiate((file) => loader('machine', file), imports, meter.instantiateCore);
  } catch (cause) {
    telemetry.end(span, 'failed');
    throw trapError(meter, cause);
  }
  let session;
  try {
    session = telemetry.within(span, () =>
      budget.run(meter, () => new component.machineApi.Session(sessionId, new MachineCapabilities())),
    );
  } catch (cause) {
    telemetry.end(span, 'failed');
    throw trapError(meter, cause);
  }
  const rootInstance = new Instance(session, links, meter);
  run.rootInstance = rootInstance;
  const machine = new MachineSession(
    rootInstance,
    run,
    { mode: timers, wallClock: now, onTimer },
    { mode: snapshot, loader: pageLoader },
  );
  const failure = telemetry.within(span, () => machine.failure());
  if (failure !== undefined) {
    telemetry.within(span, () => machine.dispose());
    telemetry.end(span, 'rejected');
    throw new ScxmlSessionError(failure, { sessionFailed: true });
  }
  telemetry.end(span, 'ok');
  return machine;
}

/**
 * One entry of an external event queue as the event the session will
 * process: a message from an invoked session is stamped with its invoke id,
 * and its completion is `done.invoke.<id>` (the WIT `event` record).
 */
function queuedEvent(entry) {
  if (entry.tag === 'external') return entry.val;
  if (entry.message.tag === 'event') return { ...entry.message.val, invokeId: entry.invokeId };
  const done = entry.message.val ?? {};
  return {
    name: `done.invoke.${entry.invokeId}`,
    eventType: 'external',
    sendId: undefined,
    origin: undefined,
    originType: undefined,
    invokeId: entry.invokeId,
    data: done.data ?? { tag: 'absent' },
    raw: undefined,
    traceContext: done.traceContext,
  };
}
