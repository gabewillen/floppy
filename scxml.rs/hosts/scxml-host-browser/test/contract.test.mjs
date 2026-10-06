// Contract tests of the browser host: deterministic fuel bounds, argument
// validation that never loses an instance, and drift checks of the public
// contract (index.d.ts, closed vocabularies) against the runtime exports and
// the canonical WIT and Rust sources.
//
// Runs in Node on the same ES modules and metered core Wasm a browser loads.
// Hostile documents are compiled in-page with the bundled compiler.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, readdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import * as host from '../src/index.js';
import * as hostTelemetry from '../src/telemetry.js';
import { sha256Hex } from '../src/sha256.js';
import { isAssignPlacement, isAssignSource } from '../src/values.js';
import { recordingTelemetry } from './recording-telemetry.js';
import {
  COMPILER_ERROR_CODES,
  COMPILE_STATUSES,
  CapabilityError,
  DEFAULT_FUEL,
  ERROR_CODES,
  FUEL_EXPORT,
  MAX_MACHINE_IR_BYTES,
  SCHEMA_POLICIES,
  STATUSES,
  ScxmlHostError,
  ScxmlSessionError,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createNullDataModel,
  createSession,
} from '../src/index.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const loadCoreModule = cachedCoreModules((component, file) =>
  readFile(join(packageRoot, 'dist', component, file)),
);
const read = (path) => readFile(join(workspace, path), 'utf8');

/**
 * The per-frame Boa loop limit (100 000 iterations) restarts in every call
 * frame, so a loop that calls a looping function runs about 1e10
 * iterations. Without fuel this hangs the page's thread for hours.
 */
const NESTED_LOOP =
  'function f(){for(var i=0;i<99999;i++){}} for(var j=0;j<99999;j++) f();';

const hostError = (code) => (error) => {
  assert.ok(error instanceof ScxmlHostError, `expected ScxmlHostError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};
const capabilityError = (code) => (error) => {
  assert.ok(error instanceof CapabilityError, `expected CapabilityError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};

/**
 * A bundled ECMAScript data model used without a session, initialized as
 * the machine initializes it from `start`.
 */
async function standaloneModel(options) {
  const model = await createEcmaScriptDataModel({ loadCoreModule, ...options });
  model.initialize([], {
    sessionId: 'standalone',
    machineName: 'm',
    ioProcessors: { tag: 'json-object', val: '{}' },
  });
  return model;
}

async function compileIr(source) {
  const compiler = await createCompiler({ loadCoreModule });
  const result = await compiler.compile(source, { sourceName: 'hostile.scxml' });
  assert.equal(result.status, 'emitted');
  return result.ir;
}

const scxml = (body) =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="a">${body}</scxml>`;

// --- fuel -------------------------------------------------------------------

test('a nested-loop expression exhausts the data-model fuel instead of hanging', async () => {
  // About 1e10 iterations would otherwise run. The default budget is a
  // deterministic count of function entries and loop iterations, so the
  // bound is the budget, never wall-clock time: exhaustion is the typed
  // trap (the next test pins that its verdict is deterministic).
  const model = await standaloneModel();
  assert.throws(() => model.execute(NESTED_LOOP, undefined), capabilityError('platform'));
  // The trapped instance is never entered again.
  assert.throws(() => model.evaluateValue('1', undefined), capabilityError('platform'));
});

test('fuel exhaustion is deterministic for identical inputs and budgets', async () => {
  // A 20 000-iteration loop fits the budget below; 50 calls of a
  // 99 999-iteration loop (each frame inside Boa's own loop limit) do not.
  // Each verdict is identical on fresh instances.
  const outcome = async (calls) => {
    const model = await standaloneModel({ fuel: 20_000_000 });
    try {
      model.evaluateValue(
        `(function(){ function f(n){ var s = 0; for (var i = 0; i < n; i++) s += 1; return s; } ` +
          `var t = 0; for (var j = 0; j < ${calls}; j++) t += f(${calls === 1 ? 20_000 : 99_999}); return t; })()`,
        undefined,
      );
      return 'completed';
    } catch (error) {
      return error instanceof CapabilityError ? error.code : String(error);
    }
  };
  for (const calls of [1, 50]) {
    const first = await outcome(calls);
    assert.equal(await outcome(calls), first);
    assert.equal(first, calls === 1 ? 'completed' : 'platform');
  }
});

test('a hostile <script> fails the machine session closed', async () => {
  const ir = await compileIr(
    scxml(
      `<state id="a"><onentry><script>${NESTED_LOOP.replaceAll('<', '&lt;')}</script></onentry></state>`,
    ),
  );
  // The script spends the whole command's budget (the data model's cap
  // equals it): the command runs out of fuel, as under Wasmtime, and the
  // run is lost.
  const dataModel = await createEcmaScriptDataModel({ loadCoreModule });
  const session = await createSession({ ir, dataModel, loadCoreModule });
  assert.throws(() => session.start(), hostError('fuel-exhausted'));
  assert.throws(() => session.status(), hostError('instance-lost'));
  session.dispose();
  // A data model capped below the command's budget exhausts only its own
  // call: the data model is poisoned and the machine fails the session.
  const capped = await createEcmaScriptDataModel({ loadCoreModule, fuel: 20_000_000 });
  const failing = await createSession({ ir, dataModel: capped, loadCoreModule });
  assert.throws(
    () => failing.start(),
    (error) => {
      assert.ok(error instanceof ScxmlSessionError, `expected a session error, got ${error}`);
      assert.equal(error.code, 'platform');
      assert.equal(error.sessionFailed, true);
      return true;
    },
  );
  assert.equal(failing.status(), 'failed');
  failing.dispose();
});

test('machine, data-model, and compiler budgets are enforced per call', async () => {
  const ir = await compileIr(scxml('<state id="a"/>'));
  await assert.rejects(createSession({ ir, fuel: 1, loadCoreModule }), hostError('fuel-exhausted'));

  const compiler = await createCompiler({ fuel: 1_000, loadCoreModule });
  await assert.rejects(compiler.compile(scxml('<state id="a"/>')), hostError('fuel-exhausted'));
  await assert.rejects(compiler.compile(scxml('<state id="a"/>')), hostError('instance-lost'));

  // Instantiation runs on the build's baked budget; the first call (the
  // machine's `initialize`) runs on the per-call one and poisons the instance.
  const starved = await createEcmaScriptDataModel({ fuel: 1_000, loadCoreModule });
  const context = { sessionId: 's', machineName: 'm', ioProcessors: { tag: 'json-object', val: '{}' } };
  assert.throws(() => starved.initialize([], context), capabilityError('platform'));
  assert.throws(() => starved.evaluateValue('1', undefined), capabilityError('platform'));
  assert.equal(starved.namelistKey('x'), undefined, 'a poisoned instance answers pure calls with none');

  // The default budgets comfortably run ordinary work.
  const session = await createSession({ ir, loadCoreModule });
  assert.equal(session.start().status, 'running');
  session.dispose();
});

test('a bundled data model answers acceptsEventData true when the call answers nothing, as the Wasmtime host does', async () => {
  // WIT `accepts-event-data` returns a `bool`; the Wasmtime host reads a
  // malformed value, a trap, and a lost instance as accepted.
  const NULL_VALUE = { tag: 'null' };
  for (const create of [createEcmaScriptDataModel, host.createXPathDataModel, createNullDataModel]) {
    const model = await create({ loadCoreModule });
    assert.equal(model.acceptsEventData(NULL_VALUE), true, model.name);
    assert.equal(model.acceptsEventData({ bogus: 1 }), true, `${model.name}: a value that is not a data value`);
  }
  // One unit of fuel traps even the cheapest call.
  const STARVING_FUEL = 1;
  const starved = await createEcmaScriptDataModel({ fuel: STARVING_FUEL, loadCoreModule });
  assert.equal(starved.acceptsEventData(NULL_VALUE), true, 'the call traps');
  assert.equal(starved.namelistKey('x'), undefined, 'the trap poisoned the instance');
  assert.equal(starved.acceptsEventData(NULL_VALUE), true, 'a lost instance');
});

/** `burn()`: one 99 000-iteration loop (inside Boa's per-frame loop limit). */
const BURN = 'function burn(){var x=0;for(var i=0;i&lt;99000;i++){x+=i}return x}';
/** An ECMAScript document whose `start` runs `loops` over an `items`-item array. */
const foreachDocument = (items, loops) =>
  scxml(
    `<datamodel><data id="arr" expr="(function(){var r=[];for(var i=0;i&lt;${items};i++)r.push(i);return r})()"/>` +
      `<data id="acc" expr="0"/></datamodel><script>${BURN}</script>` +
      `<state id="a"><onentry>${loops}<log label="done"/></onentry></state>`,
  );
/** Outcome of `start` on a fresh session: `'running'` or the error code. */
async function startOutcome(ir, { fuel, dataModelFuel = fuel } = {}) {
  const dataModel = await createEcmaScriptDataModel({ loadCoreModule, fuel: dataModelFuel });
  const session = await createSession({ ir, dataModel, loadCoreModule, fuel });
  try {
    return session.start().status;
  } catch (error) {
    return error instanceof ScxmlHostError || error instanceof ScxmlSessionError ? error.code : String(error);
  } finally {
    session.dispose();
  }
}

test('nested data-model work is charged to the machine command budget', async () => {
  // Twenty <foreach> iterations each run a <script> of about 3.6 million
  // charges: every data-model call fits the 40 million budget alone, the
  // command's total does not. The command fails deterministically instead
  // of granting each nested call a fresh budget.
  const ir = await compileIr(foreachDocument(20, '<foreach array="arr" item="it"><script>acc+=burn()</script></foreach>'));
  const budget = { fuel: 40_000_000 };
  assert.equal(await startOutcome(ir, budget), 'fuel-exhausted');
  assert.equal(await startOutcome(ir, budget), 'fuel-exhausted');
  // A command whose nested work fits the budget runs.
  const light = await compileIr(foreachDocument(5, '<foreach array="arr" item="it"><script>acc+=burn()</script></foreach>'));
  assert.equal(await startOutcome(light, budget), 'running');
});

test('the default budget runs the documented workloads in one host call each', async () => {
  // The Wasmtime default's workloads (hosts/scxml-host-wasmtime DEFAULT_FUEL):
  // 90 and 1 000 <foreach> iterations with an <assign> in one command, and a
  // 64-region parallel toggle (crates/scxml-bench `parallel-wide-64`); their
  // charges are measured by scripts/fuel-sizing.mjs.
  for (const items of [90, 1000]) {
    const foreach = await compileIr(
      scxml(
        `<datamodel><data id="arr" expr="(function(){var r=[];for(var i=0;i&lt;${items};i++)r.push(i);return r})()"/>` +
          '<data id="acc" expr="0"/></datamodel>' +
          '<state id="a"><onentry><foreach array="arr" item="it"><assign location="acc" expr="acc + it"/></foreach></onentry></state>',
      ),
    );
    assert.equal(await startOutcome(foreach), 'running');
  }

  let regions = '';
  for (let r = 0; r < 64; r += 1) {
    regions +=
      `<state id="r${r}" initial="r${r}a"><state id="r${r}a"><transition event="t" target="r${r}b"/></state>` +
      `<state id="r${r}b"><transition event="t" target="r${r}a"/></state></state>`;
  }
  const wide = await compileIr(
    `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="wide"><parallel id="wide">${regions}</parallel></scxml>`,
  );
  const session = await createSession({ ir: wide, dataModel: await createNullDataModel({ loadCoreModule }), loadCoreModule });
  session.start();
  for (let toggle = 0; toggle < 4; toggle += 1) assert.equal(session.send('t').status, 'running');
  assert.equal(session.configuration().length, 64);
  session.dispose();
});

test('the default budget bounds one heavy command (the Wasmtime host test document)', async () => {
  // Ten <foreach> loops over 200 items, each running a heavy <script>, in
  // one <onentry> (hosts/scxml-host-wasmtime/tests/session_host.rs,
  // `nested_data_model_work_is_charged_to_the_machine_call_budget`): about
  // 2.3e10 charges, which ran for minutes when every data-model call got a
  // fresh budget. The default budget stops it inside `start`.
  const loops = '<foreach array="arr" item="it"><script>for(var j=0;j&lt;5;j++){acc+=burn()}</script></foreach>'.repeat(10);
  const ir = await compileIr(foreachDocument(200, loops));
  assert.equal(await startOutcome(ir), 'fuel-exhausted');
});

test('invalid fuel options and unmetered core modules fail closed', async () => {
  const ir = await compileIr(scxml('<state id="a"/>'));
  for (const fuel of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 2, '1000', 10n]) {
    await assert.rejects(createSession({ ir, fuel, loadCoreModule }), hostError('invalid-argument'));
    await assert.rejects(
      createEcmaScriptDataModel({ fuel, loadCoreModule }),
      hostError('invalid-argument'),
    );
    await assert.rejects(createCompiler({ fuel, loadCoreModule }), hostError('invalid-argument'));
  }
  // A core module without the fuel export is never run.
  const unmetered = cachedCoreModules(() => new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]));
  await assert.rejects(
    createSession({ ir, loadCoreModule: unmetered }),
    hostError('instance-lost'),
  );
});

// --- argument validation never loses an instance ------------------------------

const sessionError = (code) => (error) => {
  assert.ok(error instanceof ScxmlSessionError, `expected ScxmlSessionError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};

// --- the external-event boundary -----------------------------------------------

test('send rejects invocation provenance and non-external event types', async () => {
  // A page cannot forge an event from a live invocation: invoke ids are set
  // only by deliver-from-invocation (SCXML 6.4), so <finalize> never runs
  // for them; internal types only inside the machine, and platform types
  // only by the host (a late error.communication), never by a page.
  const child = scxml('<state id="c"/>').replace('initial="a"', 'initial="c"');
  const childIr = await compileIr(child);
  const ir = await compileIr(
    scxml(
      `<datamodel><data id="fin" expr="0"/></datamodel>
       <state id="a">
         <invoke id="kid" src="file:child.scxml"><finalize><assign location="fin" expr="fin + 1"/></finalize></invoke>
         <transition event="poke" cond="fin === 1" target="forged"/>
         <transition event="poke" target="notforged"/>
       </state>
       <final id="forged"/><final id="notforged"/>`,
    ),
  );
  const dataModel = await createEcmaScriptDataModel({ loadCoreModule });
  const session = await createSession({
    ir,
    dataModel,
    timers: 'manual',
    loadCoreModule,
    capabilities: { loadDocument: () => child, compileInvocation: () => childIr },
  });
  session.start();
  for (const forged of [
    { name: 'poke', invokeId: 'kid' },
    { name: 'poke', type: 'external', invokeId: 'kid' },
  ]) {
    assert.throws(() => session.send(forged), sessionError('invalid-input'));
  }
  for (const forged of [
    { name: 'poke', type: 'internal' },
    { name: 'poke', type: 'platform' },
    { name: 'poke', type: 'internal', invokeId: 'kid' },
  ]) {
    assert.throws(() => session.send(forged), hostError('invalid-argument'));
  }
  assert.deepEqual(dataModel.evaluateValue('fin', undefined), { tag: 'number', val: 0 }, '<finalize> never ran');
  assert.deepEqual(session.configuration(), ['a']);
  assert.deepEqual(session.send({ name: 'poke', type: 'external' }).activeConfiguration, ['notforged']);
  session.dispose();
});

// --- run to completion: no re-entry from a capability provider -----------------

/**
 * `go` logs `in-go` (where the provider re-enters) and moves a -> b; `inner`
 * moves a or b -> c. `onentry` of a schedules `late` after 1 s.
 */
const REENTRY = scxml(
  '<datamodel><data id="n" expr="0"/></datamodel>' +
    '<state id="a"><onentry><send event="late" delay="1s"/></onentry>' +
    '<transition event="go" target="b"><log expr="\'in-go\'"/><assign location="n" expr="n+1"/></transition>' +
    '<transition event="inner" target="c"/></state>' +
    '<state id="b"><transition event="late"><log expr="\'in-late\'"/></transition>' +
    '<transition event="inner" target="c"/></state>' +
    '<state id="c"/>',
);

/**
 * Opens a manual-timer session over REENTRY whose `log` provider runs
 * `reenter(session)` once, at `<log>` text `trigger`, and records what the
 * re-entrant call returned or threw.
 */
async function reentrySession(trigger, reenter, options = {}) {
  const outcomes = [];
  let session;
  session = await createSession({
    ir: await compileIr(REENTRY),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    timers: 'manual',
    loadCoreModule,
    capabilities: {
      log: ({ text }) => {
        if (text !== trigger) return;
        try {
          outcomes.push({ value: reenter(session) });
        } catch (error) {
          outcomes.push({ error });
        }
      },
    },
    ...options,
  });
  return { session, outcomes };
}

test('a provider re-entering send is reentrant-call and the outer send completes', async () => {
  // Without the guard the inner send would trap the machine (instance-lost)
  // while the outer send reported success.
  const { session, outcomes } = await reentrySession('in-go', (s) => s.send('inner'));
  session.start();
  const step = session.send('go');
  assert.equal(outcomes.length, 1);
  assert.ok(outcomes[0].error !== undefined, 'the re-entrant send must not succeed');
  assert.ok(hostError('reentrant-call')(outcomes[0].error));
  assert.equal(step.status, 'running');
  assert.deepEqual(step.activeConfiguration, ['b']);
  // The instance was never entered re-entrantly, so it stays usable.
  assert.deepEqual(session.configuration(), ['b']);
  assert.deepEqual(session.send('inner').activeConfiguration, ['c']);
  session.dispose();
});

test('a provider re-entering advanceTime is reentrant-call and time does not move', async () => {
  const { session, outcomes } = await reentrySession('in-go', (s) => s.advanceTime(5000));
  session.start();
  const step = session.send('go');
  assert.equal(outcomes.length, 1);
  assert.ok(outcomes[0].error !== undefined, 'the re-entrant advanceTime must not succeed');
  assert.ok(hostError('reentrant-call')(outcomes[0].error));
  assert.deepEqual(step.activeConfiguration, ['b']);
  assert.equal(session.now(), 0);
  assert.equal(session.nextDeadline(), 1000);
  const delivered = session.advanceTime(1000);
  assert.equal(delivered.complete, true);
  assert.equal(delivered.results.length, 1);
  assert.deepEqual(session.configuration(), ['b']);
  session.dispose();
});

test('every session method is reentrant-call during a command and the session stays usable', async () => {
  const calls = {
    start: (s) => s.start(),
    send: (s) => s.send('inner'),
    resume: (s) => s.resume(),
    configuration: (s) => s.configuration(),
    status: (s) => s.status(),
    failure: (s) => s.failure(),
    now: (s) => s.now(),
    nextDeadline: (s) => s.nextDeadline(),
    advanceTime: (s) => s.advanceTime(0),
    commands: (s) => s.commands(),
    trace: (s) => s.trace(),
    shutdown: (s) => s.shutdown(),
    dispose: (s) => s.dispose(),
    'Symbol.dispose': (s) => s[Symbol.dispose](),
  };
  for (const [name, call] of Object.entries(calls)) {
    const { telemetry, samples } = recordingTelemetry();
    const { session, outcomes } = await reentrySession('in-go', call, { telemetry });
    session.start();
    const step = session.send('go');
    assert.equal(outcomes.length, 1, name);
    assert.ok(outcomes[0].error !== undefined, `${name} must not run during a command`);
    assert.ok(hostError('reentrant-call')(outcomes[0].error), name);
    assert.ok(
      samples.some(
        (sample) =>
          sample.attributes?.['scxml.module'] === hostTelemetry.HOST_MODULE &&
          sample.attributes?.['scxml.outcome'] === 'rejected',
      ),
      `${name} is observed as rejected`,
    );
    assert.deepEqual(step.activeConfiguration, ['b'], name);
    assert.equal(session.status(), 'running', name);
    assert.deepEqual(session.send('inner').activeConfiguration, ['c'], name);
    session.dispose();
  }
});

test('a provider re-entering during a delayed-send delivery is reentrant-call', async () => {
  const { session, outcomes } = await reentrySession('in-late', (s) => s.send('inner'));
  session.start();
  session.send('go');
  const delivered = session.advanceTime(1000);
  assert.equal(outcomes.length, 1);
  assert.ok(hostError('reentrant-call')(outcomes[0].error));
  assert.equal(delivered.complete, true);
  assert.deepEqual(delivered.results.at(-1).activeConfiguration, ['b']);
  assert.deepEqual(session.send('inner').activeConfiguration, ['c']);
  session.dispose();
});

test('an event-loop timer delivery rejects re-entry and onTimer may call the session', async () => {
  const outcomes = [];
  const observed = [];
  let session;
  let wake;
  const woken = new Promise((resolveWake) => {
    wake = resolveWake;
  });
  session = await createSession({
    ir: await compileIr(
      scxml(
        '<state id="a"><onentry><send event="late" delay="10ms"/></onentry>' +
          '<transition event="late" target="b"><log expr="\'in-late\'"/></transition></state>' +
          '<state id="b"><transition event="inner" target="c"/></state><state id="c"/>',
      ),
    ),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    loadCoreModule,
    capabilities: {
      log: () => {
        try {
          outcomes.push({ value: session.send('inner') });
        } catch (error) {
          outcomes.push({ error });
        }
      },
    },
    onTimer: (outcome) => {
      try {
        observed.push(outcome, session.configuration());
      } catch (error) {
        observed.push(outcome, error);
      } finally {
        wake();
      }
    },
  });
  session.start();
  await woken;
  assert.equal(outcomes.length, 1);
  assert.ok(hostError('reentrant-call')(outcomes[0].error));
  assert.equal(observed[0].complete, true);
  assert.deepEqual(observed[1], ['b'], 'onTimer runs outside the delivery');
  assert.deepEqual(session.send('inner').activeConfiguration, ['c']);
  session.dispose();
});

test('the machine initializes the data model with the host system values', async () => {
  const ir = await compileIr(
    `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" name="named" initial="a">` +
      `<datamodel><data id="x" expr="1"/></datamodel><state id="a"/></scxml>`,
  );
  const dataModel = await createEcmaScriptDataModel({ loadCoreModule });
  const session = await createSession({ ir, dataModel, sessionId: 'page-session', timers: 'manual', loadCoreModule });
  session.start();
  assert.deepEqual(dataModel.evaluateValue('_sessionid', undefined), { tag: 'text', val: 'page-session' });
  assert.deepEqual(dataModel.evaluateValue('_name', undefined), { tag: 'text', val: 'named' });
  assert.deepEqual(dataModel.evaluateValue('x', undefined), { tag: 'number', val: 1 });
  // `_ioprocessors` lists the linked SCXML processor under its type URI and
  // alias, with this session's location; no BasicHTTP processor is linked.
  assert.deepEqual(JSON.parse(dataModel.evaluateValue('JSON.stringify(_ioprocessors)', undefined).val), {
    'http://www.w3.org/TR/scxml/#SCXMLEventProcessor': { location: '#_scxml_page-session' },
    scxml: { location: '#_scxml_page-session' },
  });
  session.dispose();
});

test('invalid compile options are invalid-argument and the compiler stays usable', async () => {
  const compiler = await createCompiler({ loadCoreModule });
  for (const options of [{ schemaPolicy: 'x' }, { schemaPolicy: 1 }, { sourceName: 5 }]) {
    await assert.rejects(compiler.compile(scxml('<state id="a"/>'), options), (error) => {
      hostError('invalid-argument')(error);
      assert.equal(error.cause, undefined, 'no caller value is attached');
      assert.equal(error.message, 'scxml browser host: invalid-argument');
      return true;
    });
  }
  await assert.rejects(compiler.compile(42), hostError('invalid-argument'));
  const result = await compiler.compile(scxml('<state id="a"/>'), { schemaPolicy: 'lax' });
  assert.ok(COMPILE_STATUSES.includes(result.status));
});

test('data-model arguments are validated and component effects fail closed', async () => {
  const { telemetry, spans } = recordingTelemetry();
  const model = await standaloneModel({ telemetry });
  assert.throws(() => model.evaluateValue(1, undefined), capabilityError('invalid-input'));
  assert.throws(() => model.evaluateCondition('true', null), capabilityError('invalid-input'));
  const replaceChildren = { tag: 'replace-children' };
  const nullValue = { tag: 'value', val: { tag: 'null' } };
  for (const args of [
    ['x', { tag: 'value', val: { tag: 'nope' } }, replaceChildren, undefined],
    ['x', { tag: 'expression' }, replaceChildren, undefined],
    ['x', nullValue, undefined, undefined],
    ['x', nullValue, { tag: 'add-attribute' }, undefined],
    ['x', nullValue, replaceChildren, null],
  ]) {
    assert.throws(() => model.assign(...args), capabilityError('invalid-input'));
  }
  assert.equal(model.evaluateCondition('true', undefined), true, 'the instance stays usable');
  assert.ok(
    spans.some((s) => s.name === 'data-model' && s.attributes['scxml.outcome'] === 'rejected'),
    'rejections are observed',
  );
});

// --- contract drift ----------------------------------------------------------

/** Names of `export const|function|class` declarations in index.d.ts. */
async function declaredValueExports() {
  const declarations = await readFile(join(packageRoot, 'src/index.d.ts'), 'utf8');
  // An overloaded function is declared once per signature.
  const names = [...declarations.matchAll(/^export (?:declare )?(?:const|function|class) ([A-Za-z_$][\w$]*)/gm)];
  return [...new Set(names.map((match) => match[1]))].sort();
}

/** Members of one WIT `enum name { ... }` block, comments stripped. */
function witEnum(wit, name) {
  const match = wit.match(new RegExp(`enum ${name} \\{([^}]*)\\}`));
  assert.ok(match, `WIT enum ${name} exists`);
  return match[1]
    .split('\n')
    .map((line) => line.replace(/\/\/.*$/, ''))
    .join('')
    .split(',')
    .map((member) => member.trim())
    .filter((member) => member.length > 0);
}

test('assign placements match the canonical WIT variant and fail closed on other placements', async () => {
  const scxmlWit = await read('crates/scxml-wit/wit/scxml/scxml.wit');
  const variant = scxmlWit.match(/variant assign-placement \{([^}]*)\}/);
  assert.ok(variant, 'WIT variant assign-placement exists');
  const cases = [...variant[1].matchAll(/^\s*([a-z-]+)(\(string\))?,/gm)];
  assert.equal(cases.length, 8);
  for (const [, tag, payload] of cases) {
    assert.equal(isAssignPlacement(payload ? { tag, val: 'a' } : { tag }), true, tag);
    assert.equal(isAssignPlacement(payload ? { tag } : { tag, val: 'a' }), false, tag);
  }
  assert.equal(isAssignPlacement({ tag: 'nope' }), false);
  const source = scxmlWit.match(/variant assign-source \{([^}]*)\}/);
  assert.ok(source, 'WIT variant assign-source exists');
  assert.deepEqual(
    [...source[1].matchAll(/^\s*([a-z-]+)\(/gm)].map((match) => match[1]),
    ['expression', 'value', 'foreach-value'],
  );
  assert.equal(isAssignSource({ tag: 'expression', val: 'x' }), true);
  assert.equal(isAssignSource({ tag: 'value', val: { tag: 'absent' } }), true);
  assert.equal(isAssignSource({ tag: 'foreach-value', val: { tag: 'number', val: 1 } }), true);
  assert.equal(isAssignSource({ tag: 'value', val: 'x' }), false);
  assert.equal(isAssignSource({ tag: 'foreach-value', val: 'x' }), false);

  // SCXML B.2.7 defines only replace-children: the component answers any
  // other placement with `execution` and leaves the data unchanged; an
  // expression source is evaluated inside the component.
  const model = await standaloneModel();
  model.execute('var x = 1;', undefined);
  const replaceChildren = { tag: 'replace-children' };
  assert.throws(
    () => model.assign('x', { tag: 'value', val: { tag: 'number', val: 2 } }, { tag: 'first-child' }, undefined),
    capabilityError('execution'),
  );
  assert.deepEqual(model.evaluateValue('x', undefined), { tag: 'number', val: 1 });
  model.assign('x', { tag: 'value', val: { tag: 'number', val: 3 } }, replaceChildren, undefined);
  assert.deepEqual(model.evaluateValue('x', undefined), { tag: 'number', val: 3 });
  model.assign('x', { tag: 'expression', val: 'x + 1' }, replaceChildren, undefined);
  assert.deepEqual(model.evaluateValue('x', undefined), { tag: 'number', val: 4 });
  // SCXML 5.4 and 4.6: only a `<foreach>` value declares its variable.
  assert.throws(
    () => model.assign('fresh', { tag: 'value', val: { tag: 'number', val: 1 } }, replaceChildren, undefined),
    capabilityError('execution'),
  );
  model.assign('item', { tag: 'foreach-value', val: { tag: 'number', val: 5 } }, replaceChildren, undefined);
  assert.deepEqual(model.evaluateValue('item', undefined), { tag: 'number', val: 5 });
});

test('dist embeds none of the vendored W3C schema bytes, so the license needs no W3C terms', async () => {
  // The compiler component resolves schemas through a MemorySchemaResolver;
  // only BundledSchemaResolver includes the vendored XSD, and the linker
  // drops it. Should a build ever embed it, the package license and files
  // must carry the W3C notice.
  const schemaDir = join(workspace, 'crates/scxml-validator-xsd/schemas/w3c-scxml-1.0');
  const schemaFiles = (await readdir(schemaDir)).sort();
  assert.deepEqual(schemaFiles, [
    'NOTICE.md',
    'scxml-attribs.xsd',
    'scxml-contentmodels.xsd',
    'scxml-copyright.xsd',
    'scxml-datatypes.xsd',
    'scxml-module-core.xsd',
    'scxml-module-data.xsd',
    'scxml-module-external.xsd',
    'scxml.xsd',
  ]);
  const distFiles = (await readdir(join(packageRoot, 'dist'), { recursive: true, withFileTypes: true }))
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name));
  const dist = await Promise.all(distFiles.map((path) => readFile(path)));
  assert.ok(dist.some((bytes) => bytes.includes('http://www.w3.org/2005/07/scxml')), 'the scan sees embedded text');
  let probes = 0;
  for (const file of schemaFiles) {
    const text = await readFile(join(schemaDir, file), 'utf8');
    for (const line of text.split('\n').map((l) => l.trim()).filter((l) => l.length >= 40)) {
      probes += 1;
      const embedded = distFiles.filter((_, index) => dist[index].includes(line));
      assert.deepEqual(embedded, [], `${file} line ${JSON.stringify(line.slice(0, 40))} is not in dist`);
    }
  }
  assert.ok(probes > 400, `probed ${probes} schema lines`);
  const packageJson = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
  assert.equal(packageJson.license, 'Apache-2.0 OR MIT');
  // The packed tarball carries both license texts: `prepack` copies them
  // from the repository root and `files` admits them.
  assert.equal(packageJson.scripts.prepack, 'node scripts/copy-licenses.mjs');
  for (const file of ['LICENSE-APACHE', 'LICENSE-MIT']) {
    assert.ok(packageJson.files.includes(file), `files lists ${file}`);
    await readFile(join(workspace, file));
  }
  assert.equal(packageJson.repository.directory, 'hosts/scxml-host-browser');
});

test('sha256Hex matches the FIPS 180-4 known answers across padding boundaries', () => {
  const ascii = (text) => new TextEncoder().encode(text);
  // FIPS 180-4 / NIST CSHA examples.
  assert.equal(sha256Hex(new Uint8Array()), 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
  assert.equal(sha256Hex(ascii('abc')), 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
  assert.equal(
    sha256Hex(ascii('abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq')),
    '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1',
  );
  const reference = (bytes) => createHash('sha256').update(bytes).digest('hex');
  // One and two padding blocks (55/56 bytes), block multiples, and 1 MiB.
  for (const length of [1, 55, 56, 63, 64, 65, 119, 120, 128, 1000, 4097, 1 << 20]) {
    const bytes = Uint8Array.from({ length }, (_, index) => (index * 31 + 7) & 0xff);
    assert.equal(sha256Hex(bytes), reference(bytes), `length ${length}`);
  }
  // A view into a larger buffer hashes only its own bytes.
  const backing = Uint8Array.from({ length: 300 }, (_, index) => index & 0xff);
  const view = backing.subarray(17, 17 + 200);
  assert.equal(sha256Hex(view), reference(Buffer.from(view)));
});

test('index.d.ts declares exactly the runtime exports', async () => {
  assert.deepEqual(await declaredValueExports(), Object.keys(host).sort());
});

test('closed vocabularies match their canonical WIT and Rust sources', async () => {
  const scxmlWit = await read('crates/scxml-wit/wit/scxml/scxml.wit');
  const compilerDir = 'crates/scxml-wit/wit/compiler';
  const compilerFiles = (await readdir(join(workspace, compilerDir))).filter((file) => file.endsWith('.wit'));
  const compilerWit = (await Promise.all(compilerFiles.map((file) => read(`${compilerDir}/${file}`)))).join('\n');
  assert.deepEqual([...ERROR_CODES], witEnum(scxmlWit, 'error-code'));
  assert.deepEqual([...STATUSES], witEnum(scxmlWit, 'status'));
  assert.deepEqual([...COMPILER_ERROR_CODES], witEnum(compilerWit, 'error-code'));
  assert.deepEqual([...SCHEMA_POLICIES], witEnum(compilerWit, 'schema-policy'));

  const reporting = await read('crates/scxml-reporting/src/lib.rs');
  const compilerStatus = reporting.match(/pub enum CompilerStatus \{([^}]*)\}/);
  assert.ok(compilerStatus, 'CompilerStatus exists');
  const variants = [...compilerStatus[1].matchAll(/^\s*([A-Z]\w*),/gm)].map((m) => m[1].toLowerCase());
  assert.deepEqual([...COMPILE_STATUSES].sort(), variants.sort());

  const ir = await read('crates/scxml-compiler-core/src/ir.rs');
  // The browser runs the wasm machine, so it mirrors the wasm-build bound.
  const bound = ir.match(/pub const MAX_IR_BYTES: usize = wasm_bound\(([0-9_ *]+)\);/);
  assert.ok(bound, 'MAX_IR_BYTES exists as a wasm-build bound');
  const bytes = bound[1]
    .split('*')
    .map((factor) => Number(factor.trim().replaceAll('_', '')))
    .reduce((product, factor) => product * factor, 1);
  assert.equal(MAX_MACHINE_IR_BYTES, bytes);

  const meter = await read('crates/scxml-wasm-meter/src/lib.rs');
  assert.ok(meter.includes(`pub const FUEL_EXPORT: &str = "${FUEL_EXPORT}";`));
  const manifest = JSON.parse(await readFile(join(packageRoot, 'dist/build-manifest.json'), 'utf8'));
  assert.deepEqual(manifest.meter, {
    tool: 'scxml-wasm-meter',
    fuel_export: FUEL_EXPORT,
    initial_fuel: DEFAULT_FUEL,
  });
});

// --- OpenTelemetry -----------------------------------------------------------

test('telemetry names and labels match the Wasmtime host and the Rust facade', async () => {
  const component = await read('hosts/scxml-host-wasmtime/src/component.rs');
  const dataModel = await read('hosts/scxml-host-wasmtime/src/data_model.rs');
  const wasmtimeLib = await read('hosts/scxml-host-wasmtime/src/lib.rs');
  const facade = await read('crates/scxml-telemetry/src/lib.rs');
  const scxmlWit = await read('crates/scxml-wit/wit/scxml/scxml.wit');
  const packageJson = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));

  // Guest spans use the Wasmtime host's tracer and `scxml.module` values.
  assert.ok(component.includes(`"${hostTelemetry.GUEST_SCOPE}"`));
  const capabilityGraph = await read('hosts/scxml-host-wasmtime/src/capability_graph.rs');
  assert.ok(capabilityGraph.includes(`"${hostTelemetry.CAPABILITY_SCOPE}"`));
  assert.ok(capabilityGraph.includes(`"${hostTelemetry.CAPABILITY_MODULE}"`));
  assert.ok(component.includes(`"${hostTelemetry.GUEST_MODULES.machine}"`));
  assert.ok(dataModel.includes(`"${hostTelemetry.GUEST_MODULES.dataModel}"`));
  // The compiler label is the Rust facade's `Module::CompilerComponent`.
  const modules = [...facade.matchAll(/Self::(\w+) => "([a-z-]+)",/g)];
  const moduleValue = (variant) => modules.find((m) => m[1] === variant)?.[2];
  assert.equal(moduleValue('MachineComponent'), hostTelemetry.GUEST_MODULES.machine);
  assert.equal(moduleValue('CompilerComponent'), hostTelemetry.GUEST_MODULES.compiler);
  // The browser host has its own identity, parallel to `host-wasmtime`.
  assert.equal(moduleValue('HostWasmtime'), 'host-wasmtime');
  assert.equal(hostTelemetry.HOST_MODULE, 'host-browser');
  assert.ok(!modules.some((m) => m[2] === hostTelemetry.HOST_MODULE), 'no collision with a Rust module');
  // Outcomes and measurement instruments are the WIT enums and the
  // Wasmtime host's instruments.
  assert.deepEqual([...hostTelemetry.OUTCOMES], witEnum(scxmlWit, 'outcome'));
  assert.deepEqual(Object.keys(hostTelemetry.MEASUREMENT_INSTRUMENTS), witEnum(scxmlWit, 'measurement-kind'));
  for (const [name, unit] of Object.values(hostTelemetry.MEASUREMENT_INSTRUMENTS)) {
    assert.ok(wasmtimeLib.includes(`("${name}", "${unit}")`), `${name} ${unit}`);
  }
  assert.equal(hostTelemetry.SCOPE_VERSION, packageJson.version);
  assert.equal(packageJson.dependencies['@opentelemetry/api'], '1.9.1', 'the API is pinned exactly');
  // The carrier bounds are the Rust facade's.
  assert.ok(facade.includes(`pub const MAX_TRACEPARENT_BYTES: usize = ${hostTelemetry.MAX_TRACEPARENT_BYTES};`));
  assert.ok(facade.includes(`pub const MAX_TRACESTATE_BYTES: usize = ${hostTelemetry.MAX_TRACESTATE_BYTES};`));
});

test('guest trace context is validated, parents the guest span, and propagates', () => {
  const { telemetry, spans } = recordingTelemetry();
  const guest = hostTelemetry.scxmlTelemetryImport(
    new hostTelemetry.Telemetry(telemetry),
    hostTelemetry.GUEST_MODULES.machine,
  );
  const traceId = '4bf92f3577b34da6a3ce929d0e0e4736';
  const parent = {
    traceparent: `00-${traceId}-00f067aa0ba902b7-01`,
    tracestate: 'vendor=value',
    baggage: undefined,
    baggagePolicy: 'drop',
  };
  const [handle, carrier] = guest.start('evaluate-condition', parent);
  assert.notEqual(handle, 0n);
  assert.match(carrier.traceparent, new RegExp(`^00-${traceId}-[0-9a-f]{16}-01$`));
  assert.notEqual(carrier.traceparent, parent.traceparent, 'the child carrier names the new span');
  assert.equal(carrier.baggagePolicy, 'drop');
  const span = spans.at(-1);
  assert.equal(span.parentSpanId, '00f067aa0ba902b7');
  assert.equal(span.spanContext.traceId, traceId);
  guest.end(handle, 'rejected');
  assert.equal(span.attributes['scxml.outcome'], 'rejected');
  assert.equal(span.status.code, 2, 'a non-ok outcome is an error status');
  assert.ok(span.ended);
  guest.end(handle, 'ok'); // an unknown id is ignored
  assert.equal(span.attributes['scxml.outcome'], 'rejected');

  // Invalid carriers (Rust facade rules) open nothing: id 0, no carrier.
  for (const traceparent of [
    'garbage',
    `01-${traceId}-00f067aa0ba902b7-01`,
    `00-${'0'.repeat(32)}-00f067aa0ba902b7-01`,
    `00-${traceId}-${'0'.repeat(16)}-01`,
    `00-${traceId}-00f067aa0ba902b7-02`,
    `00-${traceId.toUpperCase()}-00f067aa0ba902b7-01`,
  ]) {
    const before = spans.length;
    assert.deepEqual(guest.start('start', { ...parent, traceparent }), [0n, undefined], traceparent);
    assert.equal(spans.length, before);
  }
  assert.deepEqual(guest.start('start', { ...parent, tracestate: 'k=v,'.repeat(200) }), [0n, undefined]);

  // Point signals carry the closed event as a span event.
  guest.event('send', 'send-accepted', parent);
  assert.deepEqual(spans.at(-1).events, ['send-accepted']);
  assert.equal(spans.at(-1).parentSpanId, '00f067aa0ba902b7');
});

test('open guest spans are bounded per adapter', () => {
  const { telemetry } = recordingTelemetry();
  const guest = hostTelemetry.scxmlTelemetryImport(
    new hostTelemetry.Telemetry(telemetry),
    hostTelemetry.GUEST_MODULES.dataModel,
  );
  const handles = [];
  for (let i = 0; i < hostTelemetry.MAX_OPEN_SPANS; i += 1) handles.push(guest.start('assign', undefined)[0]);
  assert.ok(handles.every((handle) => handle !== 0n));
  assert.deepEqual(guest.start('assign', undefined), [0n, undefined]);
  guest.end(handles[0], 'ok');
  assert.notEqual(guest.start('assign', undefined)[0], 0n, 'closing a span frees capacity');
});

test('failing telemetry providers never change host behavior', async () => {
  const ir = await compileIr(scxml('<state id="a"><transition event="go" target="b"/></state><state id="b"/>'));
  const boom = () => {
    throw new Error('provider failure');
  };
  const telemetry = {
    tracerProvider: { getTracer: () => ({ startSpan: boom }) },
    meterProvider: { getMeter: () => ({ createCounter: boom, createHistogram: boom }) },
  };
  const session = await createSession({ ir, telemetry, loadCoreModule });
  assert.equal(session.start().status, 'running');
  assert.deepEqual(session.send('go').activeConfiguration, ['b']);
  session.dispose();
});

test('a malformed telemetry option is invalid-argument', async () => {
  const ir = await compileIr(scxml('<state id="a"/>'));
  for (const telemetry of [null, 42, { tracerProvider: {} }, { meterProvider: { getMeter: 1 } }]) {
    await assert.rejects(createSession({ ir, telemetry, loadCoreModule }), hostError('invalid-argument'));
    await assert.rejects(
      createEcmaScriptDataModel({ telemetry, loadCoreModule }),
      hostError('invalid-argument'),
    );
    await assert.rejects(createCompiler({ telemetry, loadCoreModule }), hostError('invalid-argument'));
    assert.throws(() => cachedCoreModules(() => new Uint8Array(), { telemetry }), TypeError);
  }
});

// Telemetry lives at boundaries (CONTRIBUTING.md): the pure validators,
// converters, and fuel helpers emit nothing themselves; the observed host
// operation that calls each one records its rejection or failure.
test('helper rejections are observed by the calling host operation', async () => {
  const ir = await compileIr(scxml('<state id="a"><transition event="go" target="b"/></state><state id="b"/>'));
  const { telemetry, spans } = recordingTelemetry();
  const outcomes = (operation) =>
    spans.filter((s) => s.name === operation).map((s) => s.attributes['scxml.outcome']);

  // isFuel -> create-session rejected
  await assert.rejects(createSession({ ir, fuel: 0, telemetry, loadCoreModule }), hostError('invalid-argument'));
  assert.deepEqual(outcomes('create-session'), ['rejected']);

  // A capability provider failure (toComponentError) -> capability failed;
  // a malformed provider result -> capability failed.
  let throwNext = false;
  const session = await createSession({
    ir,
    telemetry,
    loadCoreModule,
    dataModel: {
      initialize() {},
      evaluateCondition: () => {
        if (throwNext) throw new Error('page bug');
        return true;
      },
    },
  });
  session.start();
  // toWitEvent / isWitEvent -> command rejected
  assert.throws(() => session.send({ name: 7 }), hostError('invalid-argument'));
  assert.deepEqual(outcomes('command').filter((o) => o === 'rejected').length, 1);
  // exportPayload (a closed session failure) -> command failed
  const failing = await createSession({
    ir: await compileIr(scxml('<state id="a"><transition event="go" cond="x" target="a"/></state>')),
    telemetry,
    loadCoreModule,
    dataModel: {
      initialize() {},
      evaluateCondition: () => {
        throwNext = true;
        throw new Error('page bug');
      },
    },
  });
  failing.start();
  assert.throws(() => failing.send('go'), ScxmlSessionError);
  assert.ok(outcomes('capability').includes('failed'), 'the provider failure is observed');
  assert.ok(outcomes('command').includes('failed'), 'the failed command is observed');
  session.dispose();
  failing.dispose();

  // cachedCoreModules name validation -> load-core-module rejected;
  // FuelMeter.instantiateCore on an unmetered module -> create-session failed.
  const unmetered = cachedCoreModules(() => new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]), { telemetry });
  await assert.rejects(unmetered('bogus', 'x.core.wasm'), TypeError);
  assert.deepEqual(outcomes('load-core-module'), ['rejected']);
  await assert.rejects(createSession({ ir, telemetry, loadCoreModule: unmetered }), hostError('instance-lost'));
  assert.ok(outcomes('create-session').includes('failed'));
});

/** `field` / `field?` names of one `export interface Name { ... }` block. */
function interfaceFields(source, name) {
  const match = source.match(new RegExp(`export interface ${name} \\{([^}]*)\\}`));
  assert.ok(match, `interface ${name} exists`);
  return match[1]
    .split('\n')
    .map((line) => line.replace(/\/\*\*.*?\*\/|\/\/.*$/g, '').trim())
    .filter((line) => /^[A-Za-z]\w*\??:/.test(line))
    .map((line) => line.slice(0, line.indexOf(':')))
    .sort();
}

test('index.d.ts capability records match the jco shapes of their WIT records', async () => {
  const declared = await readFile(join(packageRoot, 'src/index.d.ts'), 'utf8');
  const generated = await readFile(
    join(packageRoot, 'dist/machine/interfaces/stateforward-scxml-types.d.ts'),
    'utf8',
  );
  for (const [ours, jco] of [
    ['WitEvent', 'Event'],
    ['TraceContext', 'TraceContext'],
    ['NamedValue', 'NamedValue'],
    ['QualifiedName', 'QualifiedName'],
    ['ExtensionAttribute', 'ExtensionAttribute'],
    ['ExecutableContentExtension', 'ExecutableContentExtension'],
    ['PinnedScript', 'PinnedScript'],
    ['ScriptReference', 'ScriptReference'],
  ]) {
    assert.deepEqual(interfaceFields(declared, ours), interfaceFields(generated, jco), ours);
  }
  const dataModelApi = await readFile(
    join(packageRoot, 'dist/data-model-ecmascript/interfaces/stateforward-scxml-data-model-api.d.ts'),
    'utf8',
  );
  assert.deepEqual(interfaceFields(declared, 'SystemValues'), interfaceFields(dataModelApi, 'SystemValues'));
  // Every bundled data model exports the same data-model-api the provider
  // contract mirrors.
  for (const component of ['data-model-xpath', 'data-model-null']) {
    const other = await readFile(
      join(packageRoot, `dist/${component}/interfaces/stateforward-scxml-data-model-api.d.ts`),
      'utf8',
    );
    assert.equal(other, dataModelApi, component);
  }
});

// --- W3C trace propagation through events -------------------------------------
//
// The machine opens one `execute` span per effect batch (one per engine
// command in these documents: no `<if>` or `<foreach>`). Each event carries
// the carrier of the batch span that emitted it, so the batch that processes
// it is that span's child, whatever route it took: the same contract the
// Wasmtime host's tests/trace_propagation.rs proves.

const REMOTE_TRACE_ID = '0af7651916cd43dd8448eb211c80319c';
const REMOTE_SPAN_ID = 'b7ad6b7169203331';
/** A null data-model document (the propagation tests need no expressions). */
const nullScxml = (body) =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="a">${body}</scxml>`;

const remoteCarrier = () => ({
  traceparent: `00-${REMOTE_TRACE_ID}-${REMOTE_SPAN_ID}-01`,
  tracestate: 'vendor=value',
  baggage: 'tenant=a',
  baggagePolicy: 'propagate',
});

/**
 * The machine's batch spans in start order, each as `session<-parent`
 * (`root` or `child`; parent `#k` for batch k, `remote` for the page's
 * carrier, `host` for a host span), paired with the trace's `command`
 * observations. In end order every batch span is followed by its outcome
 * point signal and, for a completed batch, its event point signal.
 */
function lineage(spans, trace) {
  const executes = spans.filter(
    (span) => span.name === 'execute' && span.attributes['scxml.module'] === hostTelemetry.GUEST_MODULES.machine,
  );
  const byEnd = [...executes].sort((a, b) => a.endOrder - b.endOrder);
  const batches = [];
  for (let index = 0; index < byEnd.length; ) {
    const batch = byEnd[index];
    const signal = byEnd[index + 1];
    assert.ok(signal !== undefined && signal.events.length === 0, 'an outcome signal follows every batch');
    assert.equal(signal.parentSpanId, batch.parentSpanId, 'the signal shares the batch parent');
    index += 2;
    if (byEnd[index] !== undefined && byEnd[index].events.length > 0) index += 1;
    batches.push(batch);
  }
  batches.sort((a, b) => spans.indexOf(a) - spans.indexOf(b));
  const commands = trace.entries.filter((entry) => entry.observation === 'command');
  assert.equal(batches.length, commands.length, 'one batch span per engine command');
  return batches.map((batch, index) => {
    const owner = commands[index].parent === undefined ? 'root' : 'child';
    const parentBatch = batches.findIndex((other) => other.spanContext.spanId === batch.parentSpanId);
    let parent;
    if (parentBatch >= 0) parent = `#${parentBatch}`;
    else if (batch.parentSpanId === REMOTE_SPAN_ID) parent = 'remote';
    else if (spans.some((span) => span.scope === hostTelemetry.HOST_SCOPE && span.spanContext.spanId === batch.parentSpanId))
      parent = 'host';
    else parent = 'other';
    return `${owner}<-${parent}`;
  });
}

test('raised, self-sent, delayed, and send-error events are processed under the emitting batch', async () => {
  const ir = await compileIr(
    nullScxml(
      `<state id="a">
         <onentry><raise event="r"/><send event="now"/><send event="later" delay="5ms"/></onentry>
         <onentry><send event="lost" target="#_scxml_nobody"/></onentry>
         <transition event="r now later error.communication"/>
       </state>`,
    ),
  );
  const { telemetry, spans } = recordingTelemetry();
  const session = await createSession({
    ir,
    dataModel: await createNullDataModel({ loadCoreModule }),
    telemetry,
    trace: true,
    timers: 'manual',
    loadCoreModule,
  });
  session.start();
  assert.equal(session.advanceTime(5).complete, true);
  // start; then r, error.communication, and now (all under the start
  // batch); later, routed when due on the clock, under the start batch.
  assert.deepEqual(lineage(spans, session.trace()), [
    'root<-host',
    'root<-#0',
    'root<-#0',
    'root<-#0',
    'root<-#0',
  ]);
  const traceIds = new Set(
    spans.filter((span) => span.name === 'execute').map((span) => span.spanContext.traceId),
  );
  assert.equal(traceIds.size, 1, 'one trace');
  session.dispose();
});

test('#_parent, #_<invokeid>, and done.invoke keep their lineage across page-supplied child IR', async () => {
  const child = nullScxml(
    `<state id="a">
       <onentry><send event="hello" target="#_parent"/></onentry>
       <transition event="stop" target="end"/>
     </state>
     <final id="end"/>`,
  );
  const childIr = await compileIr(child);
  const ir = await compileIr(
    nullScxml(
      `<state id="a" initial="waiting">
         <invoke id="kid" src="file:child.scxml"/>
         <state id="waiting"><transition event="hello" target="stopping"/></state>
         <state id="stopping"><onentry><send event="stop" target="#_kid"/></onentry></state>
         <transition event="done.invoke.kid" target="pass"/>
       </state>
       <final id="pass"/>`,
    ),
  );
  const { telemetry, spans } = recordingTelemetry();
  const session = await createSession({
    ir,
    dataModel: await createNullDataModel({ loadCoreModule }),
    telemetry,
    trace: true,
    timers: 'manual',
    loadCoreModule,
    capabilities: { loadDocument: () => child, compileInvocation: () => childIr },
  });
  session.start();
  assert.equal(session.status(), 'finished');
  // Root start and its invoking step; the child's start under the invoking
  // batch (invoked-session.trace-context); `hello` (#_parent) under the
  // child's start; `stop` (#_kid) under the root's `hello` batch;
  // `done.invoke.kid` under the child's finishing batch: the Wasmtime
  // host's lineage exactly.
  assert.deepEqual(lineage(spans, session.trace()), [
    'root<-host',
    'root<-host',
    'child<-#1',
    'root<-#2',
    'child<-#3',
    'root<-#4',
  ]);
  session.dispose();
});

test('a failed transition condition raises its error under the preceding batch', async () => {
  const ir = await compileIr(
    scxml(
      `<state id="a">
         <transition cond="missing.member" target="never"/>
         <transition event="error.execution" target="pass"/>
       </state>
       <state id="never"/><final id="pass"/>`,
    ),
  );
  const { telemetry, spans } = recordingTelemetry();
  const session = await createSession({
    ir,
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    telemetry,
    trace: true,
    loadCoreModule,
  });
  session.start();
  assert.equal(session.status(), 'finished');
  // Selection runs outside any batch: the error names the start batch,
  // whose effects the condition observed (the Wasmtime lineage).
  assert.deepEqual(lineage(spans, session.trace()), ['root<-host', 'root<-#0']);
  session.dispose();
});

test('a page-supplied carrier parents the batch that processes its event', async () => {
  const ir = await compileIr(
    scxml(
      `<state id="a"><transition event="go" target="b"><raise event="next"/></transition></state>
       <state id="b"><transition event="next" target="c"/></state>
       <final id="c"/>`,
    ),
  );
  const { telemetry, spans } = recordingTelemetry();
  const session = await createSession({ ir, telemetry, trace: true, loadCoreModule });
  session.start();
  assert.equal(session.send({ name: 'go', traceContext: remoteCarrier() }).status, 'finished');
  assert.deepEqual(lineage(spans, session.trace()), ['root<-host', 'root<-remote', 'root<-#1']);
  const executes = spans.filter((span) => span.name === 'execute' && span.parentSpanId !== undefined);
  assert.ok(
    executes.filter((span) => span.parentSpanId === REMOTE_SPAN_ID).every((span) => span.spanContext.traceId === REMOTE_TRACE_ID),
    "the page's trace continues",
  );
  // A malformed carrier is a typed rejection, not a silent drop.
  assert.throws(
    () => session.send({ name: 'go', traceContext: { traceparent: 'garbage', baggagePolicy: 'drop' } }),
    (error) => error instanceof ScxmlSessionError || error instanceof ScxmlHostError,
  );
  session.dispose();
});

// Regression: without an SDK the OpenTelemetry API's no-op tracer answers a
// remote parent with that same span, so the batch span "echoed" the page's
// carrier and the machine failed the batch ("effect trace context is
// invalid"): installing no telemetry changed machine behavior.
test('a page carrier under the no-op API changes nothing', async () => {
  const ir = await compileIr(
    scxml(
      `<state id="a"><transition event="go" target="b"><raise event="next"/></transition></state>
       <state id="b"><transition event="next" target="c"/></state>
       <final id="c"/>`,
    ),
  );
  const session = await createSession({ ir, loadCoreModule });
  session.start();
  assert.deepEqual(session.send({ name: 'go', traceContext: remoteCarrier() }).activeConfiguration, ['c']);
  assert.equal(session.status(), 'finished');
  session.dispose();
});
