// OpenTelemetry for the browser host, through the OpenTelemetry JS API
// (`@opentelemetry/api`, API only). The host installs no SDK, provider,
// propagator, or exporter: the page owns those, as native hosts own them in
// Rust. Until the page registers global providers (or passes providers in the
// `telemetry` option), every call is a no-op.
//
// Signals (every indexed value is a closed label; see README "Telemetry
// cardinality" for the full analysis):
//   tracer `scxml-host-browser`   one span per host operation, named after
//                                 the operation, attributes
//                                 scxml.module = host-browser,
//                                 scxml.operation = HOST_OPERATIONS (17),
//                                 scxml.outcome = OUTCOMES (3)
//   tracer `scxml-component-guest` guest spans and point signals opened
//                                 through the WIT telemetry imports, named
//                                 after the WIT operation, attributes
//                                 scxml.module = GUEST_MODULES (6),
//                                 scxml.operation = WIT operation enum,
//                                 scxml.outcome = WIT outcome enum (3),
//                                 error.type = compiler WIT error-category;
//                                 WIT signal events become span events
//   tracer `scxml-capability-guest` the same for the SCXML I/O processor
//                                 and clock components, with
//                                 scxml.module = capability-component and
//                                 the clock's own closed operation, outcome,
//                                 and signal-event enums
//   meter `scxml-host-browser`    counter `scxml.host.operations`
//                                 {scxml.module, scxml.operation,
//                                 scxml.outcome}; histograms
//                                 `scxml.component.*` (one per WIT
//                                 measurement-kind, no attributes)
// The tracer, span, attribute, and instrument names are the ones the
// Wasmtime host uses for the same facts (`test/contract.test.mjs` pins them
// against the Rust sources). Payloads, expressions, state ids, IR, messages,
// span handles, and trace ids never become attributes.
//
// Trace context: a guest carrier is validated with the rules of the Rust
// facade (`scxml_telemetry::extract_trace_context`) and parents the guest
// span; without one, guest spans parent to the innermost open host span, so
// command -> capability -> data-model -> guest spans form one trace. Each
// started guest span returns its own W3C carrier to the guest.
//
// Containment: provider failures are caught. Telemetry never changes machine
// behavior, results, or errors.

import {
  ROOT_CONTEXT,
  SpanStatusCode,
  context as otelContext,
  createTraceState,
  isSpanContextValid,
  metrics,
  trace,
} from '@opentelemetry/api';

/** Instrumentation scope of host-owned spans and metrics. */
export const HOST_SCOPE = 'scxml-host-browser';
/** Instrumentation scope of guest spans (the Wasmtime host's name). */
export const GUEST_SCOPE = 'scxml-component-guest';
/** Instrumentation scope version: this package's version. */
export const SCOPE_VERSION = '0.2.0';
/** `scxml.module` of host-owned spans and metrics. */
export const HOST_MODULE = 'host-browser';
/** `scxml.module` of guest spans, per linked component. */
export const GUEST_MODULES = Object.freeze({
  machine: 'machine-component',
  dataModel: 'data-model-component',
  compiler: 'compiler-component',
  snapshotCodec: 'snapshot-codec-component',
  validator: 'validator-component',
  extension: 'executable-content-extension',
});
/**
 * Instrumentation scope and `scxml.module` of the capability guests (the
 * SCXML I/O processor and its clock): the Wasmtime host's names.
 */
export const CAPABILITY_SCOPE = 'scxml-capability-guest';
export const CAPABILITY_MODULE = 'capability-component';
/** Closed `scxml.operation` values of host-owned spans. */
export const HOST_OPERATIONS = Object.freeze([
  'create-session',
  'ir-source',
  'command',
  'capability',
  'data-model',
  'compile',
  'load-core-module',
  'delayed-send',
  'invocation',
  'snapshot',
  'snapshot-diff',
  'validate',
  'extension-load',
  'extension-instantiate',
  'extension-execute',
  'extension-shutdown',
  'package-validator-validate',
]);
/** Closed `scxml.outcome` values (the WIT telemetry `outcome` enum). */
export const OUTCOMES = Object.freeze(['ok', 'rejected', 'failed']);
/** Counter of host operations. */
export const HOST_OPERATIONS_METRIC = 'scxml.host.operations';
/** WIT `measurement-kind` -> [histogram, unit] (the Wasmtime host's instruments). */
export const MEASUREMENT_INSTRUMENTS = Object.freeze({
  effects: Object.freeze(['scxml.component.effects', '{effect}']),
  'input-bytes': Object.freeze(['scxml.component.input.size', 'By']),
  'snapshot-bytes': Object.freeze(['scxml.component.snapshot.size', 'By']),
  'queue-depth': Object.freeze(['scxml.component.queue.depth', '{item}']),
  fuel: Object.freeze(['scxml.component.fuel', '{fuel}']),
});
/** Maximum concurrently open guest spans per adapter; beyond it `start` returns id 0. */
export const MAX_OPEN_SPANS = 1024;
/** W3C carrier bounds of the Rust facade. */
export const MAX_TRACEPARENT_BYTES = 55;
export const MAX_TRACESTATE_BYTES = 512;
const MAX_TRACESTATE_MEMBERS = 32;
const TRACEPARENT = /^00-([0-9a-f]{32})-([0-9a-f]{16})-(0[01])$/;
const ZERO_TRACE_ID = '0'.repeat(32);
const ZERO_SPAN_ID = '0'.repeat(16);
const OUTCOME_SET = new Set(OUTCOMES);

/** The innermost open host span context of the current synchronous call. */
let current;

/** Runs `body`, containing any provider failure; returns `fallback` on one. */
function contained(body, fallback) {
  try {
    return body();
  } catch {
    // Contained by contract: a failing provider must not change semantics.
    return fallback;
  }
}

/**
 * Parses a W3C carrier into a remote parent context. Returns `undefined` for
 * an invalid carrier (the Rust facade's rules: canonical version-00
 * `traceparent`, non-zero ids, flags 00 or 01, bounded `tracestate`).
 */
export function extractTraceContext(carrier) {
  if (carrier === null || typeof carrier !== 'object') return undefined;
  const { traceparent, tracestate } = carrier;
  if (traceparent === undefined) return tracestate === undefined ? ROOT_CONTEXT : undefined;
  if (typeof traceparent !== 'string' || traceparent.length > MAX_TRACEPARENT_BYTES) return undefined;
  const match = TRACEPARENT.exec(traceparent);
  if (match === null || match[1] === ZERO_TRACE_ID || match[2] === ZERO_SPAN_ID) return undefined;
  let traceState;
  if (tracestate !== undefined) {
    if (
      typeof tracestate !== 'string' ||
      tracestate.length > MAX_TRACESTATE_BYTES ||
      tracestate.split(',').length > MAX_TRACESTATE_MEMBERS
    ) {
      return undefined;
    }
    traceState = contained(() => createTraceState(tracestate), undefined);
  }
  return trace.setSpanContext(ROOT_CONTEXT, {
    traceId: match[1],
    spanId: match[2],
    traceFlags: Number.parseInt(match[3], 16),
    isRemote: true,
    traceState,
  });
}

/** The W3C carrier fields of `span`, or `undefined` when it has no valid context. */
function injectTraceContext(span) {
  const spanContext = contained(() => span.spanContext(), undefined);
  if (spanContext === undefined || !contained(() => isSpanContextValid(spanContext), false)) {
    return undefined;
  }
  const flags = spanContext.traceFlags & 1 ? '01' : '00';
  const tracestate = contained(() => spanContext.traceState?.serialize(), undefined);
  return {
    traceparent: `00-${spanContext.traceId}-${spanContext.spanId}-${flags}`,
    tracestate: tracestate === '' ? undefined : tracestate,
  };
}

/** Ends `span` with a closed outcome (and optional closed error category). */
function endSpan(span, outcome, errorType) {
  contained(() => {
    span.setAttribute('scxml.outcome', outcome);
    if (errorType !== undefined) span.setAttribute('error.type', errorType);
    if (outcome !== 'ok') span.setStatus({ code: SpanStatusCode.ERROR, message: errorType ?? outcome });
    span.end();
  });
}

/**
 * True when `option` is an accepted `telemetry` option: `undefined`, or an
 * object whose optional `tracerProvider` has `getTracer` and whose optional
 * `meterProvider` has `getMeter`.
 */
export function isTelemetryOption(option) {
  if (option === undefined) return true;
  if (option === null || typeof option !== 'object') return false;
  const { tracerProvider, meterProvider } = option;
  return (
    (tracerProvider === undefined || typeof tracerProvider?.getTracer === 'function') &&
    (meterProvider === undefined || typeof meterProvider?.getMeter === 'function')
  );
}

/**
 * The host's OpenTelemetry handle for one adapter (session, data model,
 * compiler, or loader). Providers default to the global API providers.
 */
export class Telemetry {
  #hostTracer;
  #guestTracer;
  #capabilityTracer;
  #operations;
  #meter;
  #histograms = new Map();

  /**
   * @param {{tracerProvider?: import('@opentelemetry/api').TracerProvider,
   *   meterProvider?: import('@opentelemetry/api').MeterProvider}} [option]
   *   an accepted option (see isTelemetryOption); anything else uses the
   *   global providers
   */
  constructor(option) {
    const accepted = isTelemetryOption(option) ? option : undefined;
    const tracerProvider = accepted?.tracerProvider ?? contained(() => trace.getTracerProvider(), undefined);
    const meterProvider = accepted?.meterProvider ?? contained(() => metrics.getMeterProvider(), undefined);
    this.#hostTracer = contained(() => tracerProvider.getTracer(HOST_SCOPE, SCOPE_VERSION), undefined);
    this.#guestTracer = contained(() => tracerProvider.getTracer(GUEST_SCOPE, SCOPE_VERSION), undefined);
    this.#capabilityTracer = contained(
      () => tracerProvider.getTracer(CAPABILITY_SCOPE, SCOPE_VERSION),
      undefined,
    );
    this.#meter = contained(() => meterProvider.getMeter(HOST_SCOPE, SCOPE_VERSION), undefined);
    this.#operations = contained(
      () =>
        this.#meter.createCounter(HOST_OPERATIONS_METRIC, {
          unit: '{operation}',
          description: 'Browser host operations by closed outcome',
        }),
      undefined,
    );
  }

  /** The context new spans parent to when no carrier names one. */
  static #parent() {
    return current ?? contained(() => otelContext.active(), ROOT_CONTEXT);
  }

  /** Opens one host span for `operation` (one of HOST_OPERATIONS). */
  start(operation) {
    const parent = Telemetry.#parent();
    const span = contained(
      () =>
        this.#hostTracer.startSpan(
          operation,
          { attributes: { 'scxml.module': HOST_MODULE, 'scxml.operation': operation } },
          parent,
        ),
      undefined,
    );
    const spanContext = span === undefined ? parent : contained(() => trace.setSpan(parent, span), parent);
    return { operation, span, context: spanContext };
  }

  /** Closes a host span from `start` with one of OUTCOMES and counts it. */
  end(hostSpan, outcome) {
    if (hostSpan.span !== undefined) endSpan(hostSpan.span, outcome);
    contained(() =>
      this.#operations.add(1, {
        'scxml.module': HOST_MODULE,
        'scxml.operation': hostSpan.operation,
        'scxml.outcome': outcome,
      }),
    );
  }

  /** Records one host operation that completed at once (a point span). */
  observe(operation, outcome) {
    this.end(this.start(operation), outcome);
  }

  /**
   * Runs synchronous `body` with `hostSpan` as the parent of every span
   * opened during it (host or guest, any adapter); restores the previous
   * parent afterwards, also when `body` throws.
   */
  within(hostSpan, body) {
    const previous = current;
    current = hostSpan.context;
    try {
      return body();
    } finally {
      current = previous;
    }
  }

  /**
   * Opens one guest span. Returns `{span, carrier}` or `undefined` when the
   * guest carrier is invalid.
   */
  startGuest(module, operation, carrier) {
    const parent = carrier === undefined ? Telemetry.#parent() : extractTraceContext(carrier);
    if (parent === undefined) return undefined;
    const tracer = module === CAPABILITY_MODULE ? this.#capabilityTracer : this.#guestTracer;
    const span = contained(
      () =>
        tracer.startSpan(
          operation,
          { attributes: { 'scxml.module': module, 'scxml.operation': operation } },
          parent,
        ),
      undefined,
    );
    if (span === undefined) return undefined;
    return { span, carrier: injectTraceContext(span) };
  }

  /** Ends a guest span with a closed outcome and optional closed error category. */
  endGuest(span, outcome, errorType) {
    endSpan(span, OUTCOME_SET.has(outcome) ? outcome : 'failed', errorType);
  }

  /**
   * Records one guest point signal (a zero-length span, as the Wasmtime
   * host does) with an optional outcome and event. An invalid carrier drops
   * the signal.
   */
  signalGuest(module, operation, outcome, event, carrier) {
    const started = this.startGuest(module, operation, carrier);
    if (started === undefined) return;
    if (event !== undefined) contained(() => started.span.addEvent(event));
    if (outcome === undefined) {
      contained(() => started.span.end());
    } else {
      this.endGuest(started.span, outcome);
    }
  }

  /** Records one numeric WIT measurement on its fixed instrument, with no attributes. */
  measure(measurement) {
    const instrument = MEASUREMENT_INSTRUMENTS[measurement?.kind];
    if (instrument === undefined) return;
    const [name, unit] = instrument;
    let histogram = this.#histograms.get(name);
    if (histogram === undefined) {
      histogram = contained(() => this.#meter.createHistogram(name, { unit }), undefined);
      this.#histograms.set(name, histogram);
    }
    contained(() => histogram.record(Number(measurement.value)));
  }
}

/** Open guest spans of one adapter, keyed by non-zero handle. */
class SpanTable {
  #next = 1n;
  #open = new Map();

  open(span) {
    if (this.#open.size >= MAX_OPEN_SPANS) return 0n;
    const handle = this.#next;
    this.#next += 1n;
    this.#open.set(handle, span);
    return handle;
  }

  peek(handle) {
    return this.#open.get(handle);
  }

  close(handle) {
    const span = this.#open.get(handle);
    this.#open.delete(handle);
    return span;
  }
}

/** The WIT `trace-context` of `stateforward:scxml` for a W3C carrier. */
function scxmlCarrier(carrier) {
  return carrier === undefined
    ? undefined
    : { traceparent: carrier.traceparent, tracestate: carrier.tracestate, baggage: undefined, baggagePolicy: 'drop' };
}

/**
 * `stateforward:scxml/telemetry@0.2.0` for the machine, data-model, and
 * SCXML I/O processor components.
 * @param {Telemetry} telemetry
 * @param {string} module one of GUEST_MODULES, or CAPABILITY_MODULE
 */
export function scxmlTelemetryImport(telemetry, module) {
  const spans = new SpanTable();
  return {
    start(operation, parent) {
      const started = telemetry.startGuest(module, operation, parent);
      if (started === undefined) return [0n, undefined];
      const handle = spans.open(started.span);
      if (handle === 0n) {
        contained(() => started.span.end());
        return [0n, undefined];
      }
      return [handle, scxmlCarrier(started.carrier)];
    },
    end(handle, outcome) {
      const span = spans.close(handle);
      if (span !== undefined) telemetry.endGuest(span, outcome);
    },
    observe(operation, outcome, measurement, carrier) {
      if (measurement !== undefined) telemetry.measure(measurement);
      telemetry.signalGuest(module, operation, outcome, undefined, carrier);
    },
    event(operation, event, carrier) {
      telemetry.signalGuest(module, operation, undefined, event, carrier);
    },
  };
}

/**
 * `stateforward:scxml-clock/telemetry@0.2.0` for the clock component: point
 * signals only (the clock opens no spans), with the clock's closed
 * operation, outcome, and signal-event labels, as the Wasmtime host records
 * them.
 * @param {Telemetry} telemetry
 */
export function clockTelemetryImport(telemetry) {
  return {
    observe(operation, outcome, measurement, carrier) {
      if (measurement !== undefined) telemetry.measure(measurement);
      telemetry.signalGuest(CAPABILITY_MODULE, operation, outcome, undefined, carrier);
    },
    event(operation, event, carrier) {
      telemetry.signalGuest(CAPABILITY_MODULE, operation, undefined, event, carrier);
    },
  };
}

/**
 * `stateforward:scxml-compiler/telemetry@0.2.0` for the compiler component,
 * and `stateforward:scxml-validator/telemetry@0.2.0` (the same shape) for the
 * validator component (`module` GUEST_MODULES.validator).
 * @param {Telemetry} telemetry
 * @param {string} [module] one of GUEST_MODULES (default the compiler's)
 */
export function compilerTelemetryImport(telemetry, module = GUEST_MODULES.compiler) {
  const spans = new SpanTable();
  return {
    start(_module, operation, parent) {
      const started = telemetry.startGuest(module, operation, parent);
      if (started === undefined) return [0n, undefined];
      const handle = spans.open(started.span);
      if (handle === 0n) {
        contained(() => started.span.end());
        return [0n, undefined];
      }
      return [handle, started.carrier];
    },
    event(handle, event) {
      const span = spans.peek(handle);
      if (span !== undefined) contained(() => span.addEvent(event));
    },
    end(handle, error) {
      const span = spans.close(handle);
      if (span !== undefined) telemetry.endGuest(span, error === undefined ? 'ok' : 'failed', error);
    },
  };
}
