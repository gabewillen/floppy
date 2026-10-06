// Headless smoke test of the browser host on the transpiled components.
//
// Node runs the exact ES module bindings and core Wasm modules a browser
// loads; only the core-module loader differs (file reads instead of fetch).
//
// Inputs (set by scripts/verify-browser-host.sh):
//   SCXML_BROWSER_IR   lowered IR of tests/fixtures/representative-machine.scxml
//                      compiled by the CLI.
// The expected trace and the ordered data-model calls come from the shared
// fixture tests/fixtures/representative-machine.trace.json, which the
// Wasmtime host test requires the machine component to reproduce in both
// packaged and runtime IR modes, so the two hosts cannot drift.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  CapabilityError,
  MAX_MACHINE_IR_BYTES,
  ScxmlHostError,
  ScxmlSessionError,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';
import {
  CAPABILITY_MODULE,
  CAPABILITY_SCOPE,
  GUEST_MODULES,
  GUEST_SCOPE,
  HOST_MODULE,
  HOST_OPERATIONS,
  HOST_OPERATIONS_METRIC,
  HOST_SCOPE,
  OUTCOMES,
} from '../src/telemetry.js';
import { recordingTelemetry } from './recording-telemetry.js';
import { fixtureEvent, logSink, replayDataModel, runFixtureTrace, valueJson } from './trace-harness.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const fixture = JSON.parse(
  await readFile(join(workspace, 'tests/fixtures/representative-machine.trace.json'), 'utf8'),
);
const irPath = process.env.SCXML_BROWSER_IR;
assert.ok(
  irPath,
  'SCXML_BROWSER_IR must name the CLI-compiled lowered IR of tests/fixtures/representative-machine.scxml; ' +
    'run `sh scripts/verify-browser-host.sh` from the repository root (README.md, "Tests")',
);
const ir = new Uint8Array(await readFile(irPath));
const loadCoreModule = cachedCoreModules((component, file) =>
  readFile(join(packageRoot, 'dist', component, file)),
);

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

async function runTrace() {
  const { telemetry, spans, samples } = recordingTelemetry();
  const trace = await runFixtureTrace({ ir, fixture, loadCoreModule, telemetry });
  assert.equal(trace.finalStatus, 'shut-down');
  return { ...trace, spans, samples };
}

// --- tests ------------------------------------------------------------------

test('the CLI-compiled IR is the IR the shared fixture was recorded against', () => {
  assert.equal(sha256(ir), fixture['ir-sha256']);
});

test('runtime-IR session reproduces the shared Wasmtime trace', async () => {
  const { results, spans, samples, mismatches, consumed } = await runTrace();
  assert.deepEqual(mismatches, []);
  assert.equal(consumed, fixture['data-model-calls'].length, 'every recorded call replayed');
  assert.deepEqual(results, fixture.results);

  // OpenTelemetry carries only closed labels, through the page's providers.
  const guestModules = new Set(Object.values(GUEST_MODULES));
  const modules = new Set(spans.map((span) => span.attributes['scxml.module']));
  assert.ok(modules.has(HOST_MODULE) && modules.has(GUEST_MODULES.machine));
  for (const span of spans) {
    assert.ok(span.ended, `span ${span.name} ended`);
    assert.equal(span.name, span.attributes['scxml.operation']);
    assert.match(span.name, /^[a-z-]+$/);
    const keys = Object.keys(span.attributes).sort();
    const outcome = span.attributes['scxml.outcome'];
    assert.deepEqual(
      keys,
      outcome === undefined ? ['scxml.module', 'scxml.operation'] : ['scxml.module', 'scxml.operation', 'scxml.outcome'],
    );
    if (outcome !== undefined) assert.ok(OUTCOMES.includes(outcome));
    if (span.scope === HOST_SCOPE) {
      assert.equal(span.attributes['scxml.module'], HOST_MODULE);
      assert.ok(HOST_OPERATIONS.includes(span.name), span.name);
    } else if (span.scope === CAPABILITY_SCOPE) {
      // The session's SCXML I/O processor and clock (the Wasmtime host's
      // capability-guest scope).
      assert.equal(span.attributes['scxml.module'], CAPABILITY_MODULE);
    } else {
      assert.equal(span.scope, GUEST_SCOPE);
      assert.ok(guestModules.has(span.attributes['scxml.module']));
    }
  }
  // Guest spans join the host trace: every machine guest span without its
  // own carrier parents to the host command (or create-session) span.
  const hostIds = new Set(spans.filter((s) => s.scope === HOST_SCOPE).map((s) => s.spanContext.spanId));
  const guestSpans = spans.filter((s) => s.scope === GUEST_SCOPE);
  assert.ok(guestSpans.length > 0, 'the machine emits guest spans');
  assert.ok(guestSpans.every((s) => s.parentSpanId !== undefined), 'no guest span is a root');
  assert.ok(guestSpans.some((s) => hostIds.has(s.parentSpanId)), 'guest spans parent to host spans');
  for (const sample of samples) {
    if (sample.kind === 'counter') {
      assert.equal(sample.name, HOST_OPERATIONS_METRIC);
      assert.deepEqual(Object.keys(sample.attributes).sort(), ['scxml.module', 'scxml.operation', 'scxml.outcome']);
    } else {
      assert.match(sample.name, /^scxml\.component\./);
      assert.equal(sample.attributes, undefined, 'measurements carry no attributes');
      assert.equal(typeof sample.value, 'number');
    }
  }
});

test('the transpiled ECMAScript component answers every recorded data-model call identically', async () => {
  const model = await createEcmaScriptDataModel({ loadCoreModule });
  // Standalone use: initialize as the machine would from `start`.
  model.initialize([], {
    sessionId: fixture['session-id'],
    machineName: 'representative',
    ioProcessors: { tag: 'json-object', val: '{}' },
  });
  let withoutEvent = 0;
  let configuration = [];
  for (const call of fixture['data-model-calls']) {
    model.updateActiveConfiguration(configuration, call.configuration);
    configuration = call.configuration;
    const event = fixtureEvent(call.event);
    // `undefined` is the WIT `none`: SCXML defines no current event (for
    // example `<onentry>` during `start`), so `_event` stays unbound.
    if (event === undefined) withoutEvent += 1;
    let result;
    switch (call.op) {
      case 'evaluate-condition':
        result = model.evaluateCondition(call.expression, event);
        break;
      case 'evaluate-value':
        result = valueJson(model.evaluateValue(call.expression, event));
        break;
      case 'evaluate-log':
        result = model.evaluateLog(call.expression, event);
        break;
      default:
        throw new Error(`fixture operation not covered: ${call.op}`);
    }
    assert.deepEqual({ ok: result }, call.result, `${call.op} ${call.expression}`);
  }
  assert.ok(withoutEvent > 0, 'the fixture exercises a no-event evaluation');
  assert.equal(model.evaluateCondition("typeof _event === 'undefined'", undefined), true);

  // A whole session on the component data model reproduces the shared trace.
  const dataModel = await createEcmaScriptDataModel({ loadCoreModule });
  const logs = logSink();
  const session = await createSession({
    ir,
    dataModel,
    capabilities: logs,
    sessionId: fixture['session-id'],
    loadCoreModule,
  });
  const results = [];
  const record = (step) => {
    assert.equal(step.stable, true, 'the machine runs each command to a stable configuration');
    results.push({ status: step.status, configuration: step.activeConfiguration, logs: logs.lines.splice(0) });
  };
  record(session.start());
  for (const name of fixture.events) record(session.send(name));
  assert.deepEqual(results, fixture.results);
  session.shutdown();
  session.dispose();
});

test('data-model component failures surface as typed host errors, never raw traps', async () => {
  await assert.rejects(
    createEcmaScriptDataModel({ fuel: 0, loadCoreModule }),
    (error) => error instanceof ScxmlHostError && error.code === 'invalid-argument',
  );
  // The bundled component instantiates and initializes (no ambient clock
  // or randomness is read); expression failures are typed capability
  // errors, never raw traps.
  const model = await createEcmaScriptDataModel({ loadCoreModule });
  // Before the machine initializes it, the component answers nothing.
  assert.throws(
    () => model.evaluateValue('1', undefined),
    (error) => error instanceof CapabilityError && error.code === 'unavailable',
  );
  model.initialize([], {
    sessionId: 'standalone',
    machineName: 'representative',
    ioProcessors: { tag: 'json-object', val: '{}' },
  });
  // `Date` reads the session clock, never an ambient one: a data model
  // joined to no session reads time 0.
  assert.deepEqual(model.evaluateValue('Date.now()', undefined), { tag: 'number', val: 0 });
  assert.throws(
    () => model.evaluateValue('undeclared_name', undefined),
    (error) => error instanceof CapabilityError && error.code === 'execution',
  );
});

test('in-browser compilation emits the same IR bytes as the CLI', async () => {
  const compiler = await createCompiler({ loadCoreModule });
  const source = await readFile(join(workspace, 'tests/fixtures/representative-machine.scxml'), 'utf8');
  const result = await compiler.compile(source, { sourceName: 'representative-machine.scxml' });
  assert.equal(result.status, 'emitted');
  assert.equal(sha256(result.ir), sha256(ir));

  const invalid = await compiler.compile('<scxml xmlns="http://www.w3.org/2005/07/scxml"><state', {});
  assert.notEqual(invalid.status, 'emitted');
  assert.equal(invalid.ir, undefined);
  assert.ok(invalid.report.byteLength > 0, 'the report is the decision');
});

test('hostile IR fails closed with typed codes', async () => {
  const open = (bytes) =>
    createSession({ ir: bytes, dataModel: replayDataModel([]), capabilities: logSink(), loadCoreModule });
  const rejected = async (bytes, code) => {
    await assert.rejects(open(bytes), (error) => {
      assert.ok(error instanceof ScxmlSessionError, `expected a session error, got ${error}`);
      assert.equal(error.code, code);
      assert.equal(error.sessionFailed, true);
      return true;
    });
  };
  const encode = (value) => new TextEncoder().encode(JSON.stringify(value));
  const valid = JSON.parse(new TextDecoder().decode(ir));

  await assert.rejects(
    open(new Uint8Array(MAX_MACHINE_IR_BYTES + 1)),
    (error) => error instanceof ScxmlHostError && error.code === 'machine-ir-too-large',
  );
  await assert.rejects(
    createSession({ ir: 'not bytes', loadCoreModule }),
    (error) => error instanceof ScxmlHostError && error.code === 'invalid-argument',
  );
  await rejected(encode({ ...valid, version: valid.version + 1 }), 'unsupported-version');
  for (const malformed of ['', '\0asm', '{"version": 4', '[1, 2, 3]']) {
    await rejected(new TextEncoder().encode(malformed), 'invalid-input');
  }
  await rejected(encode({ ...valid, initial: { ...valid.initial, targets: [2 ** 32 - 1] } }), 'invalid-input');
  await rejected(encode({ ...valid, unexpected: true }), 'invalid-input');

  // Hostile sessions leave the host usable for valid IR.
  const { results } = await runTrace();
  assert.deepEqual(results, fixture.results);
});

test('lifecycle rejections and shutdown follow the machine contract', async () => {
  const dataModel = replayDataModel(fixture['data-model-calls']);
  const session = await createSession({ ir, dataModel, capabilities: logSink(), loadCoreModule });
  assert.equal(session.status(), 'ready');
  assert.throws(
    () => session.send('begin'),
    (error) => error instanceof ScxmlSessionError && error.sessionFailed === false,
  );
  assert.throws(() => session.send({ name: 42 }), (error) => error.code === 'invalid-argument');
  assert.equal(session.start().stable, true);
  assert.equal(session.send('begin').stable, true);
  assert.deepEqual(session.configuration(), ['l1', 'r1']);
  session.shutdown();
  session.shutdown();
  assert.equal(session.status(), 'shut-down');
  assert.deepEqual(session.configuration(), []);
  assert.throws(() => session.start(), (error) => error instanceof ScxmlSessionError && error.code === 'protocol');
});
