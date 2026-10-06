// In-memory OpenTelemetry providers for the tests: the smallest objects that
// satisfy the `@opentelemetry/api` TracerProvider and MeterProvider
// interfaces the host calls, recording every span and metric sample. They
// stand in for a page-installed SDK, so the tests need no SDK dependency.
import { trace } from '@opentelemetry/api';

let nextId = 1n;
let nextEnd = 0;
const hex = (value, width) => value.toString(16).padStart(width, '0');

/**
 * Returns `{telemetry, spans, samples}`: pass `telemetry` as the host's
 * `telemetry` option; `spans` and `samples` fill as the host emits.
 */
export function recordingTelemetry() {
  const spans = [];
  const samples = [];
  const tracerProvider = {
    getTracer(scope, version) {
      return {
        startSpan(name, options, parentContext) {
          const parent = parentContext === undefined ? undefined : trace.getSpanContext(parentContext);
          const id = nextId;
          nextId += 1n;
          const record = {
            scope,
            version,
            name,
            attributes: { ...(options?.attributes ?? {}) },
            events: [],
            status: undefined,
            ended: false,
            parentSpanId: parent?.spanId,
            spanContext: {
              traceId: parent?.traceId ?? hex(id, 32),
              spanId: hex(id, 16),
              traceFlags: 1,
              isRemote: false,
            },
          };
          spans.push(record);
          return {
            spanContext: () => record.spanContext,
            setAttribute(key, value) {
              record.attributes[key] = value;
              return this;
            },
            setStatus(status) {
              record.status = status;
              return this;
            },
            addEvent(event) {
              record.events.push(event);
              return this;
            },
            end() {
              if (!record.ended) {
                record.endOrder = nextEnd;
                nextEnd += 1;
              }
              record.ended = true;
            },
            isRecording: () => !record.ended,
          };
        },
      };
    },
  };
  const meterProvider = {
    getMeter(scope, version) {
      const instrument = (kind) => (name, options) => ({
        add: (value, attributes) => samples.push({ scope, version, kind, name, unit: options?.unit, value, attributes }),
        record: (value, attributes) =>
          samples.push({ scope, version, kind, name, unit: options?.unit, value, attributes }),
      });
      return { createCounter: instrument('counter'), createHistogram: instrument('histogram') };
    },
  };
  return { telemetry: { tracerProvider, meterProvider }, spans, samples };
}
