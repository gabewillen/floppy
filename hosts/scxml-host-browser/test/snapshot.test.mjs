// Contract tests of the read-only session snapshot API
// (docs/session-snapshot.md; the browser README, "Snapshots"): argument
// validation, `codec-unavailable`, checkpoint gating (`not-stable`,
// `status-invalid`), the lazily loaded and cached codec, data-model names
// and roles, telemetry labels, and drift of the closed vocabularies against
// the WIT and the `scxml-snapshot` crate. Any snapshot error fails a test
// that expects a document.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import * as hostTelemetry from '../src/telemetry.js';
import { recordingTelemetry } from './recording-telemetry.js';
import {
  DEFAULT_SNAPSHOT_FUEL,
  MAX_SNAPSHOT_BYTES,
  SNAPSHOT_ERROR_CODES,
  SNAPSHOT_MODES,
  SNAPSHOT_SECTIONS,
  ScxmlHostError,
  ScxmlSnapshotError,
  TRACE_CONTEXT_POLICIES,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createNullDataModel,
  createSession,
  diffSnapshots,
} from '../src/index.js';
import { isFuel as isFuelBudget } from '../src/fuel.js';
import { MAX_SNAPSHOT_INVOKED_SESSIONS, MAX_SNAPSHOT_INVOKE_DEPTH, isDataModelSnapshot } from '../src/snapshot.js';
import { InvokedSessions, ROOT } from '../src/invoked-sessions.js';
import { withinDocumentBounds } from '../src/machine.js';
import { isDataValue } from '../src/values.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const read = (path) => readFile(join(workspace, path), 'utf8');
const readDist = (component, file) => readFile(join(packageRoot, 'dist', component, file));
const loadCoreModule = cachedCoreModules(readDist);
const compiler = await createCompiler({ loadCoreModule });

const hostError = (code) => (error) => {
  assert.ok(error instanceof ScxmlHostError, `expected ScxmlHostError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};
const snapshotError = (code) => (error) => {
  assert.ok(error instanceof ScxmlSnapshotError, `expected ScxmlSnapshotError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};

async function compileIr(source) {
  const result = await compiler.compile(source, { sourceName: 'snapshot.scxml' });
  assert.equal(result.status, 'emitted');
  return result.ir;
}

/** A document with a pending delayed send and one user variable. */
const DOCUMENT = `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="a">
  <datamodel><data id="x" expr="1"/></datamodel>
  <state id="a">
    <onentry><log label="entered" expr="'in-a'"/><send event="later" delay="50ms"/></onentry>
    <transition event="go" target="b"><log label="went" expr="'in-go'"/></transition>
  </state>
  <state id="b"><transition event="later" target="done"/></state>
  <final id="done"/>
</scxml>`;

async function session(options = {}) {
  return createSession({
    ir: await compileIr(DOCUMENT),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    timers: 'manual',
    loadCoreModule,
    ...options,
  });
}

// --- closed vocabularies -----------------------------------------------------

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

test('snapshot vocabularies and bounds match the WIT and the scxml-snapshot crate', async () => {
  const wit = await read('crates/scxml-wit/wit/scxml/scxml.wit');
  const crate = await read('crates/scxml-snapshot/src/lib.rs');
  // `session-snapshot-host` declares its own, narrower `section` enum
  // earlier in the file, so the match starts at `session-snapshot-api`.
  const api = wit.slice(wit.indexOf('interface session-snapshot-api {'));
  assert.deepEqual([...SNAPSHOT_SECTIONS], witEnum(api, 'section'));
  assert.deepEqual([...TRACE_CONTEXT_POLICIES].sort(), witEnum(wit, 'trace-context-policy').sort());
  const codes = crate.match(/pub const fn as_str\(self\) -> &'static str \{\s*match self \{([^}]*)\}/);
  assert.ok(codes, 'SnapshotErrorCode::as_str exists');
  assert.deepEqual([...SNAPSHOT_ERROR_CODES], [...codes[1].matchAll(/=> "([a-z-]+)"/g)].map((m) => m[1]));
  assert.ok(crate.includes(`pub const MAX_DOCUMENT_BYTES: usize = ${MAX_SNAPSHOT_BYTES.toLocaleString('en-US').replaceAll(',', '_')};`));
  assert.ok(crate.includes(`pub const MAX_INVOKED_SESSIONS: usize = ${MAX_SNAPSHOT_INVOKED_SESSIONS};`));
  assert.ok(crate.includes(`pub const MAX_INVOKE_DEPTH: usize = ${MAX_SNAPSHOT_INVOKE_DEPTH};`));
  assert.deepEqual([...SNAPSHOT_MODES], ['lazy', 'none']);
  assert.ok(isFuelBudget(DEFAULT_SNAPSHOT_FUEL));
  assert.ok(hostTelemetry.HOST_OPERATIONS.includes('snapshot'));
  assert.ok(hostTelemetry.HOST_OPERATIONS.includes('snapshot-diff'));
  const readme = await readFile(join(packageRoot, 'README.md'), 'utf8');
  const row = readme.split('\n').find((line) => line.startsWith('| host spans and counter |'));
  assert.ok(row, 'README cardinality row exists');
  for (const operation of hostTelemetry.HOST_OPERATIONS) assert.ok(row.includes(`\`${operation}\``), operation);
  assert.ok(row.trim().endsWith(`| ${hostTelemetry.HOST_OPERATIONS.length} |`));
});

test('the document bounds are checked before any session is read, at any depth or count, as under Wasmtime', () => {
  // A run's sessions as the capture sees them: no instance is ever read,
  // so a session whose read would throw proves the check reads none.
  const runOf = (invokedSessions) => ({
    invokedSessions,
    sessions: () =>
      invokedSessions
        .keys()
        .slice(1)
        .map((key) => [key, new Proxy({}, { get: () => assert.fail(`session ${key} was read`) })]),
  });
  // A chain 10,000 sessions deep (no invokeLimits): the codec's count
  // bound, at once, as the Wasmtime host reports it.
  const deep = new InvokedSessions();
  for (let level = 0, parent = ROOT; level < 10_000; level += 1) parent = deep.register(parent, `k${level}`);
  assert.throws(() => withinDocumentBounds(runOf(deep)), snapshotError('bounds-exceeded'));
  // One invoked session past the count bound, none deep.
  const wide = new InvokedSessions();
  for (let n = 0; n <= MAX_SNAPSHOT_INVOKED_SESSIONS; n += 1) wide.register(ROOT, `s${n}`);
  assert.throws(() => withinDocumentBounds(runOf(wide)), snapshotError('bounds-exceeded'));
  // Within the count, one level past the depth bound.
  const tall = new InvokedSessions();
  for (let level = 0, parent = ROOT; level <= MAX_SNAPSHOT_INVOKE_DEPTH; level += 1) parent = tall.register(parent, `k${level}`);
  assert.throws(() => withinDocumentBounds(runOf(tall)), snapshotError('depth-exceeded'));
  // Within both bounds, the sessions come back in invocation order.
  const fits = new InvokedSessions();
  for (let level = 0, parent = ROOT; level < MAX_SNAPSHOT_INVOKE_DEPTH; level += 1) parent = fits.register(parent, `k${level}`);
  assert.equal(withinDocumentBounds({ invokedSessions: fits, sessions: () => fits.keys().slice(1).map((key) => [key, {}]) }).length, MAX_SNAPSHOT_INVOKE_DEPTH);
});

test('a snapshot of a chain deeper than the document holds fails with depth-exceeded and the run goes on', async () => {
  const NS = 'http://www.w3.org/2005/07/scxml';
  const child = `<scxml xmlns="${NS}" version="1.0" datamodel="null" initial="c0"><state id="c0"><invoke src="file:child.scxml"/></state></scxml>`;
  const root = `<scxml xmlns="${NS}" version="1.0" datamodel="null" initial="r0"><state id="r0"><invoke src="file:child.scxml"/><transition event="stop" target="done"/></state><final id="done"/></scxml>`;
  const depth = MAX_SNAPSHOT_INVOKE_DEPTH + 8;
  const deep = await createSession({
    ir: await compileIr(root),
    dataModel: await createNullDataModel({ loadCoreModule }),
    timers: 'manual',
    loadCoreModule,
    invokeLimits: { maxInvokeDepth: depth },
    capabilities: { loadDocument: () => child },
  });
  deep.start();
  await assert.rejects(deep.snapshot(), snapshotError('depth-exceeded'));
  // The read-only snapshot changed nothing: the run continues.
  assert.equal(deep.status(), 'running');
  deep.send({ name: 'stop' });
  assert.equal(deep.status(), 'finished');
  deep.dispose();
});

test('the build records the codec release identity', async () => {
  const manifest = JSON.parse(await readFile(join(packageRoot, 'dist/build-manifest.json'), 'utf8'));
  const { RELEASE_DIGESTS } = await import('../dist/identity.js');
  assert.deepEqual(
    RELEASE_DIGESTS,
    Object.fromEntries(manifest.components.map((component) => [component.name, component.source_sha256])),
  );
  assert.match(RELEASE_DIGESTS['snapshot-codec'], /^[0-9a-f]{64}$/);
});

// --- arguments ---------------------------------------------------------------

test('malformed snapshot and diff arguments are invalid-argument', async () => {
  const machine = await session();
  machine.start();
  for (const options of [
    null,
    'strip',
    { exclude: 'data-model' },
    { exclude: ['pending_sends'] },
    { exclude: ['bogus'] },
    { traceContext: 'keep' },
  ]) {
    await assert.rejects(machine.snapshot(options), hostError('invalid-argument'), JSON.stringify(options));
  }
  await assert.rejects(createSession({ ir: await compileIr(DOCUMENT), snapshot: 'eager' }), hostError('invalid-argument'));
  const bytes = new Uint8Array([123, 125]);
  for (const [before, after, options] of [
    ['{}', bytes],
    [bytes, [123, 125]],
    [bytes, bytes, null],
    [bytes, bytes, { includeTraceContext: 'yes' }],
    [bytes, bytes, { loadCoreModule: 'dist' }],
    [bytes, bytes, { telemetry: {} , includeTraceContext: 1 }],
  ]) {
    await assert.rejects(diffSnapshots(before, after, options), hostError('invalid-argument'));
  }
  await assert.rejects(
    diffSnapshots(new Uint8Array(MAX_SNAPSHOT_BYTES + 1), bytes, { loadCoreModule }),
    snapshotError('too-large'),
  );
  machine.dispose();
});

// --- codec availability ------------------------------------------------------

/** A loader over `dist/` that has no snapshot codec, counting codec requests. */
function loaderWithoutCodec() {
  const requests = [];
  const loader = cachedCoreModules((component, file) => {
    if (component === 'snapshot-codec') {
      requests.push(file);
      throw new Error('not shipped');
    }
    return readDist(component, file);
  });
  return { loader, requests };
}

test("snapshot: 'none' and a missing codec are codec-unavailable and change nothing", async () => {
  const none = await session({ snapshot: 'none' });
  none.start();
  await assert.rejects(none.snapshot(), snapshotError('codec-unavailable'));
  assert.equal(none.send('go').status, 'running');
  none.dispose();

  const { loader, requests } = loaderWithoutCodec();
  const missing = await session({ loadCoreModule: loader });
  missing.start();
  await assert.rejects(missing.snapshot(), snapshotError('codec-unavailable'));
  assert.ok(requests.length > 0, 'the codec is requested on first use');
  await assert.rejects(diffSnapshots(new Uint8Array(), new Uint8Array(), { loadCoreModule: loader }), snapshotError('codec-unavailable'));
  assert.deepEqual(missing.send('go').activeConfiguration, ['b']);
  missing.dispose();
});

test('the codec loads on first use and is cached per loader', async () => {
  const requests = [];
  const loader = cachedCoreModules((component, file) => {
    if (component === 'snapshot-codec') requests.push(file);
    return readDist(component, file);
  });
  const machine = await session({ loadCoreModule: loader });
  machine.start();
  assert.equal(requests.length, 0, 'creating and running a session loads no codec');
  await machine.snapshot();
  const loaded = requests.length;
  assert.ok(loaded > 0);
  await machine.snapshot();
  const other = await session({ loadCoreModule: loader });
  other.start();
  await other.snapshot();
  await diffSnapshots(new Uint8Array(), new Uint8Array(), { loadCoreModule: loader }).catch(() => {});
  assert.equal(requests.length, loaded, 'the instance is reused');
  machine.dispose();
  other.dispose();
});

// --- checkpoints -------------------------------------------------------------

test('a snapshot during another operation is not-stable and the operation completes', async () => {
  const pending = [];
  let machine;
  machine = await createSession({
    ir: await compileIr(DOCUMENT),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    timers: 'manual',
    loadCoreModule,
    capabilities: {
      log: ({ text }) => {
        if (text === 'in-go') pending.push(machine.snapshot());
      },
    },
  });
  machine.start();
  assert.deepEqual(machine.send('go').activeConfiguration, ['b']);
  assert.equal(pending.length, 1);
  await assert.rejects(pending[0], snapshotError('not-stable'));
  assert.equal(machine.commands() > 0, true);
  machine.dispose();
});

test('a not stable step is not-stable; a failed or shut-down session is status-invalid', async () => {
  // Each `tick` re-raises itself: the machine stops at its per-call command
  // bound with work left (step.stable false).
  const busy = await createSession({
    ir: await compileIr(
      `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="a">
         <state id="a"><onentry><raise event="tick"/></onentry><transition event="tick" target="a"/></state>
       </scxml>`,
    ),
    timers: 'manual',
    loadCoreModule,
  });
  assert.equal(busy.start().stable, false);
  await assert.rejects(busy.snapshot(), snapshotError('not-stable'));
  busy.dispose();

  const failing = await createSession({
    ir: await compileIr(DOCUMENT),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    timers: 'manual',
    maxCommands: 1,
    loadCoreModule,
  });
  assert.throws(() => {
    failing.start();
    failing.send('go');
  });
  assert.equal(failing.status(), 'failed');
  await assert.rejects(failing.snapshot(), snapshotError('status-invalid'));
  failing.dispose();

  const done = await session();
  done.start();
  done.shutdown();
  await assert.rejects(done.snapshot(), snapshotError('status-invalid'));
  done.dispose();
  await assert.rejects(done.snapshot(), snapshotError('status-invalid'));
});

test('rejected snapshots are observed on the snapshot operation', async () => {
  const { telemetry, samples } = recordingTelemetry();
  const machine = await session({ telemetry, snapshot: 'none' });
  machine.start();
  await assert.rejects(machine.snapshot({ exclude: ['bogus'] }), hostError('invalid-argument'));
  await assert.rejects(machine.snapshot(), snapshotError('codec-unavailable'));
  const outcomes = samples
    .filter((sample) => sample.attributes?.['scxml.operation'] === 'snapshot')
    .map((sample) => sample.attributes['scxml.outcome']);
  assert.deepEqual(outcomes, ['rejected', 'rejected']);
  machine.dispose();
});

// --- documents ---------------------------------------------------------------

test('snapshots at every checkpoint are documents that change nothing', async () => {
  const run = async (snapshots) => {
    const machine = await session({ trace: true });
    const documents = [];
    const take = async (options) => {
      if (!snapshots) return;
      documents.push(JSON.parse(new TextDecoder().decode(await machine.snapshot(options))));
    };
    await take();
    machine.start();
    await take();
    await take({ exclude: ['system', 'data-model', 'queues', 'pending-sends', 'transitions', 'previews', 'invocations'] });
    await take({ traceContext: 'preserve' });
    machine.send('go');
    await take();
    machine.advanceTime(50);
    await take();
    const result = {
      trace: machine.trace(),
      commands: machine.commands(),
      now: machine.now(),
      status: machine.status(),
      configuration: machine.configuration(),
    };
    machine.dispose();
    return { result, documents };
  };
  const plain = await run(false);
  const snapshotted = await run(true);
  assert.deepEqual(snapshotted.result, plain.result);
  assert.equal(snapshotted.documents.length, 6);
  const [ready, started, excluded] = snapshotted.documents;
  for (const document of snapshotted.documents) {
    assert.equal(document.version, 'scxml.session-snapshot/v2');
    assert.match(document.sha256, /^[0-9a-f]{64}$/);
    assert.equal(document.snapshot.root.session_id, 'machine-session');
  }
  // A linked data model that has never been initialized is reported as not
  // linked (docs/session-snapshot.md, "Session object"): the `ready`
  // snapshot precedes `start`, so it carries neither the data-model role
  // nor `datamodel_name`. Once initialized, the name and role stay even when
  // `system` and `datamodel` are both excluded (only the sections go), as
  // on the Wasmtime host.
  assert.deepEqual(
    ready.snapshot.components.map((component) => component.role),
    ['clock', 'io-processor-scxml', 'machine'],
  );
  assert.equal(ready.snapshot.root.datamodel_name, undefined);
  assert.deepEqual(
    excluded.snapshot.components.map((component) => component.role),
    ['clock', 'data-model-ecmascript', 'io-processor-scxml', 'machine'],
  );
  assert.equal(excluded.snapshot.root.datamodel_name, 'ecmascript');
  for (const document of snapshotted.documents.slice(3)) {
    assert.deepEqual(
      document.snapshot.components.map((component) => component.role),
      ['clock', 'data-model-ecmascript', 'io-processor-scxml', 'machine'],
    );
  }
  assert.deepEqual(
    started.snapshot.components.map((component) => component.role),
    ['clock', 'data-model-ecmascript', 'io-processor-scxml', 'machine'],
  );
  assert.equal(ready.snapshot.root.status, 'ready');
  assert.equal(started.snapshot.root.status, 'running');
  assert.deepEqual(started.options, { exclude: [], trace_context: 'strip' });
  // `options.exclude` is canonicalized to the WIT `section` declaration
  // order (docs/session-snapshot.md, "`options`": sorted and unique; the
  // Rust `Section` `Ord`), which is not alphabetical.
  assert.deepEqual(excluded.options.exclude, [
    'system',
    'data_model',
    'queues',
    'pending_sends',
    'transitions',
    'previews',
    'invocations',
  ]);
  for (const section of ['system', 'data_model', 'queues', 'pending_sends', 'transitions', 'invocations']) {
    assert.equal(excluded.snapshot.root[section], undefined, section);
  }
});

test('diffSnapshots compares two documents', async () => {
  const machine = await session();
  machine.start();
  const before = await machine.snapshot();
  machine.send('go');
  const after = await machine.snapshot();
  machine.dispose();
  const diff = JSON.parse(new TextDecoder().decode(await diffSnapshots(before, after, { loadCoreModule })));
  assert.equal(diff.version, 'scxml.session-snapshot-diff/v2');
  assert.equal(diff.identical, false);
  const same = JSON.parse(new TextDecoder().decode(await diffSnapshots(before, before, { loadCoreModule })));
  assert.equal(same.identical, true);
  const flipped = before.slice();
  flipped[flipped.length - 2] ^= 1;
  await assert.rejects(diffSnapshots(flipped, after, { loadCoreModule }), (error) => error instanceof ScxmlSnapshotError);
});

// --- page-supplied data models -------------------------------------------------

test('a named page data model snapshots like the bundled one', async () => {
  const take = async (dataModel) => {
    const machine = await session({ dataModel });
    machine.start();
    const documents = [];
    for (const options of [undefined, { exclude: ['system', 'data-model'] }]) {
      documents.push(new TextDecoder().decode(await machine.snapshot(options)));
    }
    machine.dispose();
    return documents;
  };
  const expected = await take(await createEcmaScriptDataModel({ loadCoreModule }));
  // A page provider: a bundled provider's members copied onto a plain object.
  const page = Object.fromEntries(Object.entries(await createEcmaScriptDataModel({ loadCoreModule })));
  assert.equal(page.name, 'ecmascript');
  const produced = await take(page);
  assert.deepEqual(produced, expected);
  for (const text of produced) {
    const { snapshot } = JSON.parse(text);
    assert.equal(snapshot.root.datamodel_name, 'ecmascript');
    assert.ok(snapshot.components.some((component) => component.role === 'data-model-ecmascript'));
  }
});

test('a page data model name outside the closed set is invalid-argument; the name is read once', async () => {
  const ir = await compileIr(DOCUMENT);
  for (const name of ['custom', 'ECMAScript', '', 7, null]) {
    await assert.rejects(
      createSession({ ir, dataModel: { name, initialize() {} }, timers: 'manual', loadCoreModule }),
      hostError('invalid-argument'),
      JSON.stringify(name),
    );
  }
  let reads = 0;
  const provider = await createEcmaScriptDataModel({ loadCoreModule });
  const counted = Object.fromEntries(Object.entries(provider).filter(([key]) => key !== 'name'));
  Object.defineProperty(counted, 'name', {
    enumerable: true,
    get: () => (reads++ === 0 ? 'ecmascript' : 'custom'),
  });
  const machine = await createSession({ ir, dataModel: counted, timers: 'manual', loadCoreModule });
  machine.start();
  const { snapshot } = JSON.parse(new TextDecoder().decode(await machine.snapshot()));
  assert.equal(snapshot.root.datamodel_name, 'ecmascript');
  assert.equal(reads, 1);
  machine.dispose();
});

test('every WIT variable-value kind passes the data-model snapshot check', () => {
  const dump = (value) => ({
    system: { sessionId: 's', machineName: 'm', ioProcessors: { tag: 'absent' } },
    variables: [{ name: 'x', value }],
  });
  for (const value of [
    { tag: 'value', val: { tag: 'number', val: 1 } },
    { tag: 'text', val: { kind: 'function', text: 'function f() {}' } },
    { tag: 'text', val: { kind: 'object', text: '[object Map]' } },
    { tag: 'oversized', val: 65_537n },
    { tag: 'oversized', val: (1n << 64n) - 1n },
  ]) {
    assert.equal(isDataModelSnapshot(dump(value), isDataValue), true, value.tag);
  }
  for (const value of [
    { tag: 'oversized', val: 65_537 },
    { tag: 'oversized', val: -1n },
    { tag: 'oversized', val: 1n << 64n },
    { tag: 'text', val: 'function f() {}' },
    { tag: 'text', val: { kind: 'function', text: 1 } },
    { tag: 'value', val: 1 },
    { tag: 'bogus', val: 'x' },
    null,
  ]) {
    assert.equal(isDataModelSnapshot(dump(value), isDataValue), false, JSON.stringify(value, (_, v) => (typeof v === 'bigint' ? `${v}n` : v)));
  }
});

// --- error types ---------------------------------------------------------------

test('index.d.ts closes SnapshotErrorCode, and ScxmlSnapshotError carries only a code', async () => {
  const declarations = await readFile(join(packageRoot, 'src/index.d.ts'), 'utf8');
  const union = declarations.match(/export type SnapshotErrorCode =([^;]*);/);
  assert.ok(union, 'SnapshotErrorCode is declared');
  assert.deepEqual([...union[1].matchAll(/'([a-z-]+)'/g)].map((match) => match[1]), [...SNAPSHOT_ERROR_CODES]);
  assert.match(declarations, /export const SNAPSHOT_ERROR_CODES: readonly SnapshotErrorCode\[\];/);
  const errorClass = declarations.match(/export class ScxmlSnapshotError extends Error \{([^}]*)\}/);
  assert.ok(errorClass, 'ScxmlSnapshotError is declared');
  assert.deepEqual([...errorClass[1].matchAll(/readonly (\w+)/g)].map((match) => match[1]), ['code']);
  assert.deepEqual(Object.keys(new ScxmlSnapshotError('inconsistent')).sort(), ['code', 'name']);
  const snapshotDoc = declarations.match(/\* Read-only canonical snapshot[\s\S]*?snapshot\(options\?: SnapshotOptions\)/);
  assert.ok(snapshotDoc, 'MachineSession.snapshot is documented');
  for (const thrown of ['ScxmlSnapshotError', 'ScxmlSessionError', 'ScxmlHostError', "'status-invalid'"]) {
    assert.ok(snapshotDoc[0].includes(thrown), `snapshot() documents ${thrown}`);
  }
});
