// Invoked services in the browser host (src/service.js), the counterpart of
// hosts/scxml-host-wasmtime/tests/invoked_services.rs: authentication and
// binding checks, an unbound type failing its `<invoke>` (error.execution),
// events both ways and `done.invoke` with the service's data, cancellation
// when the invoking state exits, outbound HTTP admission (references,
// supplied origins, no transport), WASI planning against the page's grants,
// and an in-memory directory a service reads and writes. The example
// components are scxml_service_example (echo, timers, outbound HTTP) and
// scxml_service_files (wasi:filesystem), built with --services.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  ENV_NAMESPACE_URI,
  ScxmlHostError,
  ServiceComponents,
  authenticateServiceComponent,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const artifacts = process.env.SCXML_BROWSER_ARTIFACTS ?? join(workspace, 'target/wasm-artifacts/browser-host');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const ECHO = 'https://stateforward.org/scxml/services/echo';
const FILES = 'https://stateforward.org/scxml/services/files';

async function readServicePackage(manifestPath) {
  const manifest = JSON.parse(await readFile(join(workspace, manifestPath), 'utf8'));
  return {
    serviceUri: manifest['service-uri'],
    packageVersion: manifest['package-version'],
    serviceArtifact: manifest['service-artifact'],
    witWorld: manifest['wit-world'],
    witVersion: manifest['wit-version'],
    httpOrigins: manifest['http-origins'],
    envSlots: manifest['env-slots'],
  };
}

const echoBytes = new Uint8Array(await readFile(join(artifacts, 'scxml_service_example.virtualized.wasm')));
const filesBytes = new Uint8Array(await readFile(join(artifacts, 'scxml_service_files.virtualized.wasm')));
const echo = await authenticateServiceComponent(echoBytes, sha256(echoBytes));
const files = await authenticateServiceComponent(filesBytes, sha256(filesBytes));
const echoPackage = await readServicePackage('components/scxml-service-example/package/echo.service.json');
const filesPackage = await readServicePackage('components/scxml-service-files/package/files.service.json');
const compiler = await createCompiler({ loadCoreModule });

const machine = (body) =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="${ENV_NAMESPACE_URI}" version="1.0" datamodel="ecmascript" initial="s">${body}</scxml>`;

/** Every event the session processes is logged as `name data`. */
const LOG_ALL = '<transition event="*"><log label="event" expr="_event.name + \' \' + JSON.stringify(_event.data)"/></transition>';

async function open(source, services) {
  const compiled = await compiler.compile(source, {});
  assert.equal(compiled.status, 'emitted', new TextDecoder().decode(compiled.report));
  const logs = [];
  const turns = [];
  const session = await createSession({
    ir: compiled.ir,
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    capabilities: { log: ({ text }) => logs.push(text) },
    serviceComponents: services,
    loadCoreModule,
    onTimer: (outcome) => turns.push(outcome),
  });
  return { session, logs, turns };
}

/** Waits (event-loop turns) until `predicate()` holds. */
async function until(predicate, what) {
  for (let turn = 0; turn < 400; turn += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(`timed out waiting for ${what}`);
}

const echoServices = () => new ServiceComponents().bind(echoPackage, echo);

test('components are authenticated by their release SHA-256', async () => {
  assert.equal(echo.kind, 'service-component');
  assert.equal(echo.component, 'service-example');
  await assert.rejects(authenticateServiceComponent(echoBytes, '0'.repeat(64)), (error) => error.code === 'artifact-digest-mismatch');
  const other = new Uint8Array([0, 97, 115, 109]);
  await assert.rejects(authenticateServiceComponent(other, sha256(other)), (error) => error.code === 'artifact-kind-mismatch');
  await assert.rejects(authenticateServiceComponent(echoBytes, 'ABC'), (error) => error.code === 'invalid-argument');
});

test('binding refuses non-service types, rebinding, and invalid declared items', () => {
  assert.throws(() => new ServiceComponents().bind(echoPackage, { kind: 'extension-component', component: 'scx' }), (error) => error.code === 'artifact-kind-mismatch');
  for (const changed of [
    { serviceUri: 'scxml' },
    { serviceUri: 'http://www.w3.org/TR/scxml/' },
    { serviceUri: 'not a uri' },
    { httpOrigins: ['ftp://example.test'] },
    { httpOrigins: ['https://example.test/?q'] },
    { httpOrigins: ['https://user@example.test'] },
    { envSlots: ['Token'] },
    { envSlots: ['tokenexpr'] },
    { envSlots: Array.from({ length: 33 }, (_, index) => `s${index}`) },
  ]) {
    assert.throws(() => new ServiceComponents().bind({ ...echoPackage, ...changed }, echo), (error) => error.code === 'namespace-binding', JSON.stringify(changed));
  }
  assert.throws(() => echoServices().bind(echoPackage, echo), (error) => error.code === 'namespace-binding');
  assert.throws(() => echoServices().withValues([{ name: 'A', value: 'x', packages: [] }]), (error) => error.code === 'invalid-argument');
  assert.throws(() => echoServices().withValues([{ name: 'A=B', value: 'x', packages: [ECHO] }]), (error) => error.code === 'invalid-argument');
  assert.throws(() => echoServices().withOutboundHttp({}), (error) => error.code === 'invalid-argument');
  assert.throws(() => echoServices().withWasi(ECHO, { dirs: [[{ dir: {} }, 'relative']] }), (error) => error.code === 'invalid-argument');
  assert.throws(() => echoServices().withWasi(ECHO, { network: true }), (error) => error.code === 'invalid-argument');
  assert.throws(() => echoServices().withCallFuel(0), (error) => error.code === 'invalid-argument');
});

test('an invoke of an unbound type raises error.execution', async () => {
  const { session, logs } = await open(machine(`<state id="s"><invoke id="e" type="urn:nothing"/>${LOG_ALL}</state>`), echoServices());
  session.start();
  assert.deepEqual(logs, ['error.execution undefined']);
  session.dispose();
});

test('events reach the service and its events, timer, and done reach the session', async () => {
  const { session, logs, turns } = await open(
    machine(`<state id="s">
      <invoke id="e" type="${ECHO}"><param name="delay" expr="1"/></invoke>
      <transition event="go"><send target="#_e" event="hi"><param name="x" expr="1"/></send></transition>
      <transition event="stop"><send target="#_e" event="stop"/></transition>
      <transition event="echo.*"><log expr="_event.name + ' ' + JSON.stringify(_event.data) + ' ' + _event.invokeid + ' ' + _event.origintype"/></transition>
      <transition event="done.invoke.e" target="done"><log expr="'done ' + JSON.stringify(_event.data)"/></transition>
    </state><final id="done"/>`),
    echoServices(),
  );
  session.start();
  await until(() => logs.length === 1, 'the tick');
  assert.deepEqual(logs, [`echo.tick 1 e ${ECHO}`]);
  session.send('go');
  await until(() => logs.length === 2, 'the echo');
  assert.equal(logs[1], `echo.hi {"x":1} e ${ECHO}`);
  session.send('stop');
  await until(() => session.status() === 'finished', 'done.invoke');
  assert.equal(logs[2], 'done {"echoed":1,"ticks":1}');
  assert.ok(turns.every((turn) => turn.error === undefined));
});

test('leaving the invoking state cancels the service: its later events never arrive', async () => {
  let aborted = false;
  let release;
  const services = echoServices()
    .withValues([{ name: 'BASE', value: 'https://api.example.test', packages: [ECHO] }])
    .withOutboundHttp({
      fetch: (url, init) =>
        new Promise((resolve, reject) => {
          release = () => resolve(new Response('late', { status: 200 }));
          init.signal.addEventListener('abort', () => {
            aborted = true;
            reject(new DOMException('aborted', 'AbortError'));
          });
        }),
    });
  const { session, logs } = await open(
    machine(`<state id="s">
      <state id="busy">
        <invoke id="e" type="${ECHO}" env:endpoint="BASE"/>
        <transition event="get"><send target="#_e" event="fetch"/></transition>
        <transition event="leave" target="idle"/>
      </state>
      <state id="idle"/>
      ${LOG_ALL}
    </state>`),
    services,
  );
  session.start();
  session.send('get');
  await until(() => release !== undefined, 'the request');
  session.send('leave');
  assert.equal(aborted, true, 'the request in flight was aborted');
  release();
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(logs, [], 'no echo.fetched, no error.communication');
  session.dispose();
});

test('outbound HTTP is admitted only to a supplied origin, with references substituted', async () => {
  const requests = [];
  const services = echoServices()
    .withValues([
      { name: 'BASE', value: 'https://api.example.test/v1', packages: [ECHO] },
      { name: 'TOKEN', value: 'secret-token', packages: [ECHO] },
      { name: 'OTHER', value: 'https://other.example.test', packages: ['urn:other'] },
    ])
    .withOutboundHttp({
      fetch: async (url, init) => {
        requests.push({ url, method: init.method, headers: init.headers, redirect: init.redirect, credentials: init.credentials });
        return new Response('hello', { status: 201 });
      },
    });
  const { session, logs } = await open(
    machine(`<state id="s">
      <invoke id="e" type="${ECHO}" env:endpoint="BASE" env:token="TOKEN"/>
      <transition event="get"><send target="#_e" event="fetch"><content expr="'/items?q=1'"/></send></transition>
      ${LOG_ALL}
    </state>`),
    services,
  );
  session.start();
  session.send('get');
  await until(() => logs.length === 1, 'echo.fetched');
  assert.deepEqual(logs, ['echo.fetched 201']);
  session.dispose();
  assert.deepEqual(requests, [
    {
      url: 'https://api.example.test/v1/items?q=1',
      method: 'GET',
      headers: [['authorization', 'Bearer secret-token']],
      redirect: 'manual',
      credentials: 'omit',
    },
  ]);

  // A variable granted only to another type is absent: no endpoint, so the
  // service's future resolves with an error.
  const { session: unbound, logs: unboundLogs } = await open(
    machine(`<state id="s"><invoke id="e" type="${ECHO}" env:endpoint="OTHER"/>
      <transition event="get"><send target="#_e" event="fetch"/></transition>${LOG_ALL}</state>`),
    services,
  );
  unbound.start();
  unbound.send('get');
  await until(() => unboundLogs.length === 1, 'error.communication');
  assert.deepEqual(unboundLogs, ['error.communication.e undefined']);
  unbound.dispose();
});

test('without a transport, or to an origin no value supplies, a request is refused', async () => {
  for (const [services, why] of [
    [echoServices().withValues([{ name: 'BASE', value: 'https://api.example.test', packages: [ECHO] }]), 'no transport'],
    [
      echoServices()
        .withValues([{ name: 'BASE', value: 'not a url', packages: [ECHO] }])
        .withOutboundHttp({ fetch: async () => assert.fail('nothing is admitted') }),
      'no supplied origin',
    ],
  ]) {
    const { session, logs } = await open(
      machine(`<state id="s"><invoke id="e" type="${ECHO}" env:endpoint="BASE"/>
        <transition event="get"><send target="#_e" event="fetch"><content expr="'https://elsewhere.test/'"/></send></transition>${LOG_ALL}</state>`),
      services,
    );
    session.start();
    session.send('get');
    await until(() => logs.length === 1, why);
    assert.deepEqual(logs, ['error.communication.e undefined'], why);
    session.dispose();
  }
});

test('an env slot the package does not declare fails the invoke', async () => {
  const { session, logs } = await open(
    machine(`<state id="s"><invoke id="e" type="${ECHO}" env:other="BASE"/>${LOG_ALL}</state>`),
    echoServices(),
  );
  session.start();
  assert.deepEqual(logs, ['error.execution undefined']);
  session.dispose();
});

test('WASI imports are planned against the grants when the session is created', async () => {
  const source = machine(`<state id="s"><invoke id="f" type="${FILES}"/></state>`);
  const tree = { dir: {} };
  const cases = [
    [new ServiceComponents().bind(filesPackage, files), 'wasi-resource-ungranted'],
    [new ServiceComponents().bind(filesPackage, files).withWasi('urn:unbound', { dirs: [[tree, '/workspace']] }), 'wasi-grant-undeclared'],
    [echoServices().withWasi(ECHO, { dirs: [[tree, '/workspace']] }), 'wasi-grant-undeclared'],
  ];
  for (const [services, code] of cases) {
    await assert.rejects(open(source, services), (error) => error instanceof ScxmlHostError && error.code === code, code);
  }
});

test('a service reads and writes the in-memory directory the page grants', async () => {
  const tree = { dir: { 'seed.txt': { source: 'from the page' } } };
  const services = new ServiceComponents().bind(filesPackage, files).withWasi(FILES, { dirs: [[tree, '/workspace']] });
  const { session, logs } = await open(
    machine(`<state id="s">
      <invoke id="f" type="${FILES}"><param name="seed" expr="'seed.txt'"/><param name="name" expr="'out.txt'"/><param name="text" expr="'written'"/></invoke>
      <transition event="done.invoke.f" target="done"><log expr="JSON.stringify(_event.data)"/></transition>
    </state><final id="done"/>`),
    services,
  );
  session.start();
  await until(() => session.status() === 'finished', 'done.invoke');
  assert.deepEqual(logs, ['{"seed":"from the page","wrote":7}']);
  assert.equal(new TextDecoder().decode(tree.dir['out.txt'].source), 'written');
});

test('a trap after the start (fuel exhaustion) raises error.communication', async () => {
  const { session, logs } = await open(
    machine(`<state id="s"><invoke id="e" type="${ECHO}"/>
      <transition event="error.communication.e" target="failed"/>${LOG_ALL}</state><final id="failed"/>`),
    echoServices().withCallFuel(50),
  );
  session.start();
  await until(() => session.status() === 'finished', 'the trap');
  assert.deepEqual(logs, []);
  session.dispose();
});

test('a run holding a service, live or finished, refuses snapshots, and async sessions refuse services', async () => {
  const { session, logs } = await open(
    machine(`<state id="s"><invoke id="e" type="${ECHO}"/>
      <transition event="stop"><send target="#_e" event="stop"/></transition>
      <transition event="done.invoke.e"><log expr="'done'"/></transition>
      <transition event="leave" target="t"/></state><state id="t"/>`),
    echoServices(),
  );
  const refused = (error) => error instanceof ScxmlHostError && error.code === 'unavailable';
  session.start();
  await assert.rejects(session.snapshot(), refused);
  session.send({ name: 'stop' });
  await until(() => logs.includes('done'), 'done.invoke');
  await assert.rejects(session.snapshot(), refused);
  session.send({ name: 'leave' });
  assert.ok((await session.snapshot()).byteLength > 0);
  session.dispose();
  const compiled = await compiler.compile(machine('<state id="s"/>'), {});
  await assert.rejects(
    createSession({ async: 'auto', ir: compiled.ir, dataModel: 'ecmascript', serviceComponents: echoServices() }),
    (error) => error.code === 'invalid-argument',
  );
});
