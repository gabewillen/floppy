// The bundled BasicHTTP Event I/O Processor (SCXML 1.0 appendix C.2),
// linked the way the Wasmtime session host links it
// (hosts/scxml-host-wasmtime/src/session_host/loopback.rs): one sender
// instance whose `http-transport` import is the page's `http` capability,
// and one receiver instance that decodes inbound requests and chooses each
// response. Both are per run (the root and its invoked children).
//
// The component owns every BasicHTTP decision: request encoding (form
// encoding, `_scxmleventname` first, uppercase percent escapes), response
// interpretation, inbound validation and decoding, and the status answered
// for each delivery outcome. This module only moves bytes between the
// component and the page:
// * `http-transport.post` refuses (the component then reports `unavailable`,
//   and the machine raises `error.communication`) unless the page configured
//   `http` and its `allow(url)` returns exactly `true`: the default is
//   deny-all, and a throwing or non-boolean `allow` denies. `allow`, `send`,
//   and the default fetch all get the target's canonical form
//   (src/url.js), so dot segments cannot step outside a path the policy
//   admitted; a target that is not an absolute http(s) URL without
//   userinfo is denied without asking the page;
// * an allowed request goes to `http.send` (default: `fetch` with
//   credentials omitted and redirects refused, so a redirect can never leave
//   the allowlist). A plain `{status, headers?, body?}` result is the
//   synchronous transport of the WIT: the component interprets it at once,
//   exactly as under Wasmtime. A promise is fire-and-forget: the component
//   is told `202 Accepted`, and a later failure (rejection, non-2xx, or a
//   malformed response) is reported through `onLateFailure`;
// * `receive` builds the WIT request of one inbound message and returns the
//   external event the component decoded, or its rejection.
//
// Bounds: every call runs on the session's deterministic fuel budget
// (src/fuel.js), capped per call by the session's `fuel`: a send made inside
// a machine command, and the loopback delivery it causes, spend that
// command's budget. A trap discards the instance, and that call and every later
// one throw an `IoFault`. Every request, response, header, and body bound is
// the component's (basichttp.wit); this module rejects only malformed
// shapes before lowering.

import { instantiate } from '../dist/ioprocessor-basichttp/ioprocessor-basichttp.js';
import { exportPayload, ImportFailure } from './errors.js';
import { FuelMeter } from './fuel.js';
import { IoFault } from './io.js';
import { CAPABILITY_MODULE } from './telemetry.js';
import { canonicalHttpUrl } from './url.js';

/** A run's root session, for its BasicHTTP location. */
export const SESSION_ROOT = 'root';
/** A session another session invoked, for its BasicHTTP location. */
export const SESSION_INVOKED = 'invoked';

/** Status the component is told for a request the asynchronous transport accepted. */
export const ASYNC_ACCEPTED_STATUS = 202;

/** Scheme and authority of the default BasicHTTP locations (the Wasmtime host's `LOOPBACK_ORIGIN`). */
export const LOOPBACK_ORIGIN = 'http://basichttp.loopback.invalid';

/** The default BasicHTTP location of session `sessionId` (the Wasmtime host's `loopback_location`). */
export const loopbackLocation = (sessionId) => `${LOOPBACK_ORIGIN}/sessions/${sessionId}/basichttp`;

const utf8 = new TextEncoder();
const utf8Decoder = new TextDecoder('utf-8', { fatal: false });
const TRACEPARENT = /^00-(?!0{32})[0-9a-f]{32}-(?!0{16})[0-9a-f]{16}-0[0-3]$/;
const MAX_TRACESTATE_BYTES = 512;
const MAX_TRACESTATE_MEMBERS = 32;

/**
 * Whether `value` is an accepted `http` option: `undefined`, or an object
 * with an `allow` function, an optional `send` function, and an optional
 * `location` string or `(sessionId) => string | undefined` function.
 */
export function isHttpOption(value) {
  if (value === undefined) return true;
  if (value === null || typeof value !== 'object') return false;
  return (
    typeof value.allow === 'function' &&
    (value.send === undefined || typeof value.send === 'function') &&
    (value.location === undefined || typeof value.location === 'string' || typeof value.location === 'function')
  );
}

/** The processor's `outbound-send` of one machine `processor-send`. */
function outboundSend(request) {
  return {
    eventName: request.eventName,
    target: request.target,
    params: request.params.map((field) => ({ name: field.name, value: field.value })),
    content: request.content,
    traceContext: request.traceContext,
  };
}

/** The default `http.send`: `fetch`, no credentials, no redirects. */
function fetchSend(request) {
  return fetch(request.url, {
    method: request.method,
    headers: request.headers,
    body: request.body,
    credentials: 'omit',
    redirect: 'error',
  }).then((response) => {
    // Fire-and-forget: only the status matters; the body is never read.
    response.body?.cancel().catch(() => {});
    return { status: response.status };
  });
}

function imports(telemetry, post) {
  const telemetryImport = {
    observe(operation, outcome, measurement, carrier) {
      // The Wasmtime host records `body-bytes` on the input-size histogram.
      if (measurement !== undefined) telemetry.measure({ kind: 'input-bytes', value: measurement.value });
      telemetry.signalGuest(CAPABILITY_MODULE, operation, outcome, undefined, carrier);
    },
    event(operation, event, carrier) {
      telemetry.signalGuest(CAPABILITY_MODULE, operation, undefined, event, carrier);
    },
  };
  const result = {};
  for (const [key, value] of [
    ['types', {}],
    ['telemetry', telemetryImport],
    ['http-transport', { post }],
  ]) {
    result[`stateforward:scxml-basichttp/${key}`] = value;
    result[`stateforward:scxml-basichttp/${key}@0.2.0`] = value;
  }
  return result;
}

/** WIT header list -> `Record<string, string>` (repeated names joined with `, `). */
function headerRecord(headers) {
  const record = {};
  for (const { name, value } of headers) {
    record[name] = Object.hasOwn(record, name) ? `${record[name]}, ${value}` : value;
  }
  return record;
}

/**
 * Page headers (a plain object, or an iterable of `[name, value]` pairs such
 * as a `Headers`) -> WIT header list; `undefined` when malformed.
 */
function headerList(headers) {
  if (headers === undefined) return [];
  if (headers === null || typeof headers !== 'object') return undefined;
  const entries = typeof headers[Symbol.iterator] === 'function' ? [...headers] : Object.entries(headers);
  const list = [];
  for (const entry of entries) {
    if (!Array.isArray(entry) || typeof entry[0] !== 'string' || typeof entry[1] !== 'string') return undefined;
    list.push({ name: entry[0], value: entry[1] });
  }
  return list;
}

/** Page body (string or bytes) -> bytes; `undefined` when malformed. */
function bodyBytes(body) {
  if (body === undefined) return new Uint8Array();
  if (typeof body === 'string') return utf8.encode(body);
  if (body instanceof Uint8Array) return body.slice();
  return undefined;
}

/** A page response -> WIT `http-response`; `undefined` when malformed. */
function witResponse(response) {
  if (response === null || typeof response !== 'object' || !Number.isInteger(response.status)) return undefined;
  if (response.status < 0 || response.status > 0xffff) return undefined;
  const headers = headerList(response.headers);
  const body = bodyBytes(response.body);
  if (headers === undefined || body === undefined) return undefined;
  return { status: response.status, headers, body };
}

/** WIT `http-response` -> the page's `{status, headers, body}`. */
function pageResponse(response) {
  return { status: response.status, headers: headerRecord(response.headers), body: utf8Decoder.decode(response.body) };
}

const isThenable = (value) => value !== null && typeof value === 'object' && typeof value.then === 'function';

function validTracestate(value) {
  if (value.length === 0 || value.length > MAX_TRACESTATE_BYTES || !/^[\x00-\x7f]*$/.test(value)) return false;
  const members = value.split(',');
  if (members.length > MAX_TRACESTATE_MEMBERS) return false;
  const keys = new Set();
  const keyPart = (part, maximum, digitFirst) =>
    part.length > 0 &&
    part.length <= maximum &&
    (digitFirst ? /^[a-z0-9][a-z0-9_\-*/]*$/ : /^[a-z][a-z0-9_\-*/]*$/).test(part);
  for (const member of members) {
    if (member.length === 0 || member.trim() !== member) return false;
    const separator = member.indexOf('=');
    if (separator < 0) return false;
    const key = member.slice(0, separator);
    const item = member.slice(separator + 1);
    const at = key.indexOf('@');
    const validKey =
      at < 0
        ? keyPart(key, 256, true)
        : !key.slice(at + 1).includes('@') && keyPart(key.slice(0, at), 241, true) && keyPart(key.slice(at + 1), 14, false);
    const validItem = item.length > 0 && item.length <= 256 && !item.endsWith(' ') && /^[\x20-\x2b\x2d-\x3c\x3e-\x7e]+$/.test(item);
    if (!validKey || !validItem || keys.has(key)) return false;
    keys.add(key);
  }
  return true;
}

/** Whether `character` is HTTP optional white space (RFC 9110 section 5.5 `OWS`: SP or HTAB). */
const isOptionalWhiteSpace = (character) => character === ' ' || character === '\t';

/** `value` without leading and trailing HTTP optional white space, in linear time. */
function trimOptionalWhiteSpace(value) {
  let start = 0;
  let end = value.length;
  while (start < end && isOptionalWhiteSpace(value[start])) start += 1;
  while (end > start && isOptionalWhiteSpace(value[end - 1])) end -= 1;
  return value.slice(start, end);
}

/**
 * The W3C Trace Context an external client sent, as the Wasmtime host
 * reads it (`inbound_http_trace_context`): its single `traceparent` header
 * and first `tracestate` header, without leading and trailing HTTP optional
 * white space (RFC 9110 section 5.5 `OWS`: SP and HTAB); a missing, repeated, or invalid
 * `traceparent` yields none and an invalid `tracestate` is dropped; inbound
 * baggage never crosses. Never changes semantics.
 */
export function inboundTraceContext(headers) {
  const values = (name) =>
    headers
      .filter((header) => header.name.toLowerCase() === name)
      .map((header) => trimOptionalWhiteSpace(header.value));
  const traceparents = values('traceparent');
  if (traceparents.length !== 1 || !TRACEPARENT.test(traceparents[0])) return undefined;
  const [tracestate] = values('tracestate');
  return {
    traceparent: traceparents[0],
    tracestate: tracestate !== undefined && validTracestate(tracestate) ? tracestate : undefined,
    baggage: undefined,
    baggagePolicy: 'drop',
  };
}

/** One instantiated BasicHTTP component with its fuel meter. */
class Instance {
  #api;
  #meter;
  #budget;
  #fault;

  constructor(api, meter, budget) {
    this.#api = api;
    this.#meter = meter;
    this.#budget = budget;
  }

  /** Runs one call; a component-error result is thrown unchanged, a trap as `IoFault`. */
  call(body) {
    if (this.#fault !== undefined) throw this.#fault;
    try {
      return this.#budget.run(this.#meter, () => body(this.#api));
    } catch (thrown) {
      if (exportPayload(thrown) !== undefined) throw thrown;
      this.#fault = new IoFault(this.#meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', thrown);
      throw this.#fault;
    }
  }
}

async function instantiateProcessor({ telemetry, loader, fuel, budget }, post) {
  const meter = new FuelMeter(fuel);
  try {
    const component = await instantiate(
      (file) => loader('ioprocessor-basichttp', file),
      imports(telemetry, post),
      meter.instantiateCore,
    );
    return new Instance(component.basichttpApi, meter, budget);
  } catch (cause) {
    throw new IoFault(meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', cause);
  }
}

/**
 * Creates the run's BasicHTTP link: the sender always (so every send is
 * validated and classified by the component, even with no `http`
 * configured), and the receiver only when `http` is configured.
 * @param {{telemetry: import('./telemetry.js').Telemetry, loader: Function, fuel: number, budget: import('./fuel.js').FuelBudget, http?: object}} options
 *   `fuel` is the per-call cap; calls run on the session's `budget`
 * @returns {Promise<BasicHttpLink>}
 * @throws {IoFault} when instantiation traps or fails
 */
export async function createBasicHttpLink({ telemetry, loader, fuel, budget, http }) {
  const link = new BasicHttpLink(http);
  const sender = await instantiateProcessor({ telemetry, loader, fuel, budget }, (request) => link.post(request));
  const receiver =
    http === undefined
      ? undefined
      : await instantiateProcessor({ telemetry, loader, fuel, budget }, () => {
          // The receiver never sends.
          throw new ImportFailure(undefined);
        });
  const typeUri = sender.call((api) => api.typeUri());
  const aliases = sender.call((api) => api.aliases());
  link.bind({ sender, receiver, typeUri, aliases });
  return link;
}

/**
 * Synchronous calls into the run's BasicHTTP processor instances. Only the
 * MachineSession of the run uses it; it is never entered re-entrantly
 * (sender and receiver are separate instances, as under Wasmtime).
 */
export class BasicHttpLink {
  #http;
  #send;
  #sender;
  #receiver;
  /** The send in progress: `{sender, traceContext, capturing, looped, own, onLateFailure}` (see `send`). */
  #current;
  /** `{route, enqueue}` of the run's loopback address space (see `setLoopback`). */
  #loopback;

  /** @internal use createBasicHttpLink */
  constructor(http) {
    this.#http = http;
    this.#send = http?.send ?? fetchSend;
  }

  /** @internal */
  bind({ sender, receiver, typeUri, aliases }) {
    this.#sender = sender;
    this.#receiver = receiver;
    /** The processor's type URI (`basichttp-api.type-uri`). */
    this.typeUri = typeUri;
    /** The processor's aliases (`basichttp-api.aliases`). */
    this.aliases = Object.freeze([...aliases]);
  }

  /**
   * Whether a send's type names this processor: its type URI or one of its
   * aliases, exactly (SCXML 6.2, C.2).
   * @internal
   */
  answers(processorType) {
    return processorType === this.typeUri || this.aliases.includes(processorType);
  }

  /**
   * Sets how a post into the loopback address space (`LOOPBACK_ORIGIN`)
   * reaches the session it addresses: `route(uri, sender)` names the live
   * session (`{tag: 'own' | 'parent' | 'child', invokeId?}`, or
   * `undefined`), and `enqueue(route, event, sender)` queues an event for a
   * parent or child.
   * @internal
   */
  setLoopback(loopback) {
    this.#loopback = loopback;
  }

  /** Whether the page configured `http` (BasicHTTP is then advertised and addressable). */
  get configured() {
    return this.#http !== undefined;
  }

  /**
   * The BasicHTTP location of session `sessionId`, whose place in its run
   * is `role` (SESSION_ROOT or SESSION_INVOKED) (SCXML C.2.3: the
   * `_ioprocessors` entry "MUST maintain a 'location' field"):
   * `http.location` for the root when it is a string, or what a `location`
   * function returns; for a session neither gives one, its loopback address
   * `http://basichttp.loopback.invalid/sessions/<id>/basichttp`, the
   * Wasmtime host's default location.
   * @returns {string}
   * @throws {unknown} what a `location` function threw, or a TypeError for a
   *   non-string result
   */
  location(sessionId, role) {
    const { location } = this.#http ?? {};
    let value;
    if (typeof location === 'string') {
      value = role === SESSION_ROOT ? location : undefined;
    } else if (typeof location === 'function') {
      value = location(sessionId);
      if (value !== undefined && typeof value !== 'string') throw new TypeError('http.location must return a string');
    }
    return value ?? loopbackLocation(sessionId);
  }

  /**
   * Encodes and performs one machine `processor-send` of session `sender`
   * (an opaque token) through the sender instance. While the page's `send`
   * runs, the first inbound request the page routes straight back into the
   * session is the send's loopback delivery (see `takeLoopback`), which the
   * session routes to the sender, its parent, or a live child, as the
   * Wasmtime loopback does;
   * `onLateFailure()` reports the later failure of a fire-and-forget send.
   * @returns {{own?: object}} the captured own event, if any
   * @throws {ImportFailure | IoFault} the component's typed send failure, or a trap
   */
  send(request, sender, { onLateFailure }) {
    const outbound = outboundSend(request);
    const current = { sender, traceContext: request.traceContext, capturing: false, looped: false, own: undefined, onLateFailure };
    this.#current = current;
    try {
      this.#sender.call((api) => api.send(outbound));
    } catch (thrown) {
      if (thrown instanceof IoFault) throw thrown;
      throw new ImportFailure(exportPayload(thrown));
    } finally {
      this.#current = undefined;
    }
    return { own: current.own };
  }

  /**
   * Validates and encodes one machine `processor-send` exactly as `send`
   * does, without posting it (`basichttp-api.check`; a missing target is
   * not checked). The host checks a delayed send before scheduling it
   * (SCXML 6.2: "The Processor MUST evaluate all arguments to <send> when
   * the <send> element is evaluated, and not when the message is actually
   * dispatched.").
   * @throws {ImportFailure | IoFault} the component's typed failure, or a trap
   */
  check(request) {
    const outbound = outboundSend(request);
    try {
      this.#sender.call((api) => api.check(outbound));
    } catch (thrown) {
      if (thrown instanceof IoFault) throw thrown;
      throw new ImportFailure(exportPayload(thrown));
    }
  }

  /**
   * Takes the loopback slot of the send in progress: while the page's
   * `send` of a session's BasicHTTP send runs, the first inbound request
   * the page routes straight back into the session is that send's loopback
   * delivery (as the Wasmtime loopback delivers each post). Returns the
   * sending session's token, or `undefined` when no send is capturing or
   * its one loopback request was already taken.
   */
  takeLoopback() {
    const current = this.#current;
    if (current === undefined || !current.capturing || current.looped) return undefined;
    current.looped = true;
    return current.sender;
  }

  /**
   * Decodes the loopback request of the send in progress (see
   * `takeLoopback`); the event carries the send's own trace carrier, as
   * under Wasmtime.
   * @returns {{event?: object, rejection?: string}}
   * @throws {IoFault} on a trap
   */
  receiveLoopback(request) {
    return this.receive(request, this.#current.traceContext);
  }

  /** Makes `event` the send's own event: the machine enqueues it when the send returns. */
  deliverOwn(event) {
    this.#current.own = event;
  }

  /** `http-transport.post` of the sender. */
  post(request) {
    const current = this.#current;
    // The page's policy, its `send`, and the default fetch all see the one
    // canonical form (src/url.js); a target that is not an absolute
    // http(s) URL without userinfo is denied without asking the page.
    const url = canonicalHttpUrl(request.uri);
    // The loopback address space is the run's own, as under Wasmtime: a
    // post there is delivered in-process and never reaches the page.
    if (url !== undefined && current !== undefined && this.#http !== undefined && url.startsWith(`${LOOPBACK_ORIGIN}/`)) {
      return this.#deliverLoopback(request, url, current);
    }
    let allowed = false;
    if (this.#http !== undefined && current !== undefined && url !== undefined) {
      try {
        allowed = this.#http.allow(url) === true;
      } catch {
        allowed = false;
      }
    }
    // Denied: no response was obtained (the component reports `unavailable`).
    if (!allowed) throw new ImportFailure(undefined);
    const page = {
      method: request.method,
      url,
      headers: headerRecord(request.headers),
      body: utf8Decoder.decode(request.body),
    };
    let result;
    current.capturing = true;
    try {
      result = this.#send.call(undefined, page);
    } catch {
      throw new ImportFailure(undefined);
    } finally {
      current.capturing = false;
    }
    if (isThenable(result)) {
      const late = current.onLateFailure;
      Promise.resolve(result).then(
        (response) => {
          const wit = witResponse(response);
          if (wit === undefined || wit.status < 200 || wit.status > 299) late();
        },
        () => late(),
      );
      return { status: ASYNC_ACCEPTED_STATUS, headers: [], body: new Uint8Array() };
    }
    const response = witResponse(result);
    if (response === undefined) throw new ImportFailure(undefined);
    return response;
  }

  /**
   * Delivers one post into the loopback address space in-process (the
   * Wasmtime loopback): the receiver decodes it, and it becomes the send's
   * own event, or is queued for the sender's parent or live child; a
   * location no live session answers is `unknown-session`. Returns the WIT
   * response the receiver chooses.
   * @throws {IoFault} on a trap
   */
  #deliverLoopback(request, url, current) {
    const route = this.#loopback?.route(url, current.sender);
    let delivery;
    if (route === undefined) {
      delivery = { tag: 'unknown-session' };
    } else {
      const received = this.receive(request, current.traceContext);
      if (received.event === undefined) {
        delivery = { tag: 'rejected', val: received.rejection };
      } else {
        if (route.tag === 'own') current.own = received.event;
        else this.#loopback.enqueue(route, received.event, current.sender);
        delivery = { tag: 'enqueued' };
      }
    }
    return this.#receiver.call((api) => api.respond(delivery));
  }

  /**
   * Builds the WIT request of one inbound message: `{method, headers, body,
   * url?}` from the page, addressed to `url` or else `location` (or `/`).
   * @returns {object | undefined} undefined when the shape is malformed
   */
  inboundRequest(message, location) {
    if (message === null || typeof message !== 'object' || typeof message.method !== 'string') return undefined;
    if (message.url !== undefined && typeof message.url !== 'string') return undefined;
    const headers = headerList(message.headers);
    const body = bodyBytes(message.body);
    if (headers === undefined || body === undefined) return undefined;
    return { method: message.method, uri: message.url ?? location ?? '/', headers, body };
  }

  /**
   * Decodes one inbound WIT request with the receiver.
   * @param {object} request WIT `http-request`
   * @param {object | undefined} traceContext the carrier the event carries
   * @returns {{event?: object, rejection?: string}}
   * @throws {IoFault} on a trap
   */
  receive(request, traceContext) {
    return this.#decode(request, () => traceContext);
  }

  /**
   * Decodes one inbound WIT request with the receiver; the event carries the
   * trace context of the request's own headers.
   * @param {object} request WIT `http-request`
   * @returns {{event?: object, rejection?: string}}
   * @throws {IoFault} on a trap
   */
  receiveFromHeaders(request) {
    return this.#decode(request, () => inboundTraceContext(request.headers));
  }

  /**
   * @param {object} request WIT `http-request`
   * @param {() => object | undefined} carrier the event's trace context
   * @returns {{event?: object, rejection?: string}}
   * @throws {IoFault} on a trap
   */
  #decode(request, carrier) {
    let received;
    try {
      received = this.#receiver.call((api) => api.receive(request));
    } catch (thrown) {
      if (thrown instanceof IoFault) throw thrown;
      return { rejection: exportPayload(thrown) };
    }
    return {
      event: {
        name: received.name,
        eventType: 'external',
        sendId: undefined,
        origin: undefined,
        originType: this.typeUri,
        invokeId: undefined,
        data: received.data,
        raw: received.raw,
        traceContext: carrier(),
      },
    };
  }

  /**
   * The page response the component chooses for one WIT `delivery`.
   * @throws {IoFault} on a trap
   */
  respond(delivery) {
    return pageResponse(this.#receiver.call((api) => api.respond(delivery)));
  }
}
