// Shared trace harness for the Node smoke test and the headless-browser
// page. Browser-compatible ES module: no Node imports.
//
// It projects machine capability calls into the exact JSON shapes the Rust
// test writes to tests/fixtures/representative-machine.trace.json, replays
// the recorded data-model answers, and records the per-command trace.

import { CapabilityError, createSession } from '../src/index.js';

/** Canonical JSON (recursively sorted keys) for structural comparison. */
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    return `{${Object.keys(value)
      .filter((key) => value[key] !== undefined)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`)
      .join(',')}}`;
  }
  return JSON.stringify(value);
}

export const valueJson = (value) =>
  value.val === undefined ? { tag: value.tag } : { tag: value.tag, val: value.val };

export const eventJson = (event) =>
  event === undefined
    ? null
    : {
        name: event.name,
        'event-type': event.eventType,
        'send-id': event.sendId ?? null,
        origin: event.origin ?? null,
        'origin-type': event.originType ?? null,
        'invoke-id': event.invokeId ?? null,
        data: valueJson(event.data),
      };

/** The jco WIT event for one recorded fixture event (`null` -> no event). */
export const fixtureEvent = (event) =>
  event === null
    ? undefined
    : {
        name: event.name,
        eventType: event['event-type'],
        sendId: event['send-id'] ?? undefined,
        origin: event.origin ?? undefined,
        originType: event['origin-type'] ?? undefined,
        invokeId: event['invoke-id'] ?? undefined,
        data: event.data,
        raw: undefined,
        traceContext: undefined,
      };

/**
 * Data model that replays the fixture's recorded calls. Each call the
 * machine makes must equal the next recorded call exactly (operation,
 * expression/location/script, `_event`, and the configuration `In()` sees);
 * a mismatch is recorded and fails the session closed.
 */
export function replayDataModel(calls) {
  let index = 0;
  let configuration = [];
  const mismatches = [];
  const next = (actual) => {
    const expected = calls[index];
    index += 1;
    const observed = { ...actual, configuration };
    if (expected === undefined) {
      mismatches.push({ index: index - 1, observed, recorded: null });
      throw new CapabilityError('platform');
    }
    const { result, ...recorded } = expected;
    if (canonical(observed) !== canonical(recorded)) {
      mismatches.push({ index: index - 1, observed, recorded });
      throw new CapabilityError('platform');
    }
    if ('err' in result) throw new CapabilityError(result.err);
    return result.ok;
  };
  return {
    mismatches,
    consumed: () => index,
    // The fixture records evaluations only; the machine's data binding
    // (`initialize-data-model` from `start`) declares nothing here.
    initialize(declarations) {
      if (declarations.length !== 0) throw new CapabilityError('platform');
    },
    updateActiveConfiguration(exited, entered) {
      configuration = configuration.filter((stateId) => !exited.includes(stateId)).concat(entered);
    },
    evaluateCondition(expression, event) {
      return next({ op: 'evaluate-condition', expression, event: eventJson(event) });
    },
    evaluateValue(expression, event) {
      return next({ op: 'evaluate-value', expression, event: eventJson(event) });
    },
    assign(location, source, placement, event) {
      // Recorded assignments carry the default placement implicitly; any
      // other placement is part of the observed call and cannot match one.
      const typed = placement.tag === 'replace-children' ? {} : { placement };
      const assigned =
        source.tag === 'expression' ? { expression: source.val } : { value: valueJson(source.val) };
      next({ op: 'assign', location, ...assigned, event: eventJson(event), ...typed });
    },
    execute(script, event) {
      next({ op: 'execute', script, event: eventJson(event) });
    },
    evaluateLog(expression, event) {
      return next({ op: 'evaluate-log', expression, event: eventJson(event) });
    },
  };
}

/** `<log>` capability recording `label: text` lines like the Rust test. */
export function logSink() {
  const lines = [];
  return {
    lines,
    log({ label, text }) {
      if (typeof text !== 'string') throw new CapabilityError('invalid-input');
      lines.push(label === undefined ? text : `${label}: ${text}`);
    },
  };
}

/**
 * Runs the fixture's event sequence on a fresh runtime-IR session with the
 * replay data model and returns the observable trace.
 */
export async function runFixtureTrace({ ir, fixture, loadCoreModule, telemetry }) {
  const dataModel = replayDataModel(fixture['data-model-calls']);
  const logs = logSink();
  const session = await createSession({
    ir,
    dataModel,
    capabilities: logs,
    telemetry,
    sessionId: fixture['session-id'],
    loadCoreModule,
  });
  const results = [];
  const record = (step) => {
    if (step.stable !== true) throw new Error('machine step stopped before a stable configuration');
    results.push({
      status: step.status,
      configuration: step.activeConfiguration,
      logs: logs.lines.splice(0),
    });
  };
  record(session.start());
  for (const name of fixture.events) record(session.send(name));
  session.shutdown();
  const finalStatus = session.status();
  session.dispose();
  return {
    results,
    finalStatus,
    mismatches: dataModel.mismatches,
    consumed: dataModel.consumed(),
  };
}
