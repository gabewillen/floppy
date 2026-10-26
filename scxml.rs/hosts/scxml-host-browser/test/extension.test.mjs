// Extension components in the browser host (src/extension.js), the
// counterpart of hosts/scxml-host-wasmtime/tests/executable_content_extensions.rs:
// authentication and binding checks, an unbound namespace failing the
// session start closed, per-call fuel and retirement after a trap, one
// instance per session (an invoked child gets its own, and the bound
// packages reach the `<invoke>` compiler), and the options an async session
// does not take. The session capabilities, ordering, and the
// executable-content snapshot are held to the Wasmtime host by
// test/host-parity.test.mjs.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  ExtensionComponents,
  ScxmlHostError,
  ScxmlSessionError,
  authenticateExtensionComponent,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';
import { readPackage } from './packages.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const artifacts = process.env.SCXML_BROWSER_ARTIFACTS ?? join(workspace, 'target/wasm-artifacts/browser-host');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const EXAMPLE = 'https://stateforward.org/scxml/example';
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

const exampleBytes = new Uint8Array(await readFile(join(artifacts, 'scxml_extension_example.virtualized.wasm')));
const examplePackage = await readPackage('components/scxml-extension-example/package/example.extension.json');
const example = await authenticateExtensionComponent(exampleBytes, sha256(exampleBytes));
const compiler = await createCompiler({ loadCoreModule });

const machine = (body, initial = 's') =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="${EXAMPLE}" version="1.0" datamodel="ecmascript" initial="${initial}">${body}</scxml>`;

async function compile(source) {
  const compiled = await compiler.compile(source, { packages: [examplePackage] });
  assert.equal(compiled.status, 'emitted', new TextDecoder().decode(compiled.report));
  return compiled.ir;
}

async function open(source, extensions) {
  const logs = [];
  const session = await createSession({
    ir: await compile(source),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    capabilities: { log: ({ label, text }) => logs.push(`${label}: ${text}`) },
    extensionComponents: extensions,
    timers: 'manual',
    loadCoreModule,
  });
  return { session, logs };
}

test('components are authenticated by their release SHA-256', async () => {
  assert.equal(example.kind, 'extension-component');
  assert.equal(example.component, 'extension-example');
  await assert.rejects(authenticateExtensionComponent(exampleBytes, '0'.repeat(64)), (error) => error.code === 'artifact-digest-mismatch');
  const other = new Uint8Array([0, 97, 115, 109]);
  await assert.rejects(authenticateExtensionComponent(other, sha256(other)), (error) => error.code === 'artifact-kind-mismatch');
  for (const [bytes, digest] of [[[1], sha256(exampleBytes)], [exampleBytes, 'ABC'], [exampleBytes, undefined]]) {
    await assert.rejects(authenticateExtensionComponent(bytes, digest), (error) => error.code === 'invalid-argument');
  }
});

test('a build ships the extension components its --extensions name, each under its own digest', async () => {
  // scripts/verify-browser-host.sh builds dist with --extensions naming
  // extensions/manifest.tsv: the example component under another name and
  // another digest (a name section of its own).
  const extraBytes = new Uint8Array(await readFile(join(artifacts, 'extensions', 'scxml_extension_extra.virtualized.wasm')));
  const extra = await authenticateExtensionComponent(extraBytes, sha256(extraBytes));
  assert.equal(extra.component, 'scxml-extension-extra');
  const { EXTENSIONS } = await import('../dist/extensions.js');
  assert.deepEqual(Object.keys(EXTENSIONS).sort(), ['extension-example', 'scx', 'scxml-extension-extra']);
  const { session } = await open(
    machine('<state id="s"><onentry><example:raise event="after"/></onentry><transition event="after" target="f"/></state><final id="f"/>'),
    new ExtensionComponents().bind(examplePackage, extra),
  );
  session.start();
  assert.deepEqual(session.configuration(), ['f'], 'the added component ran the element');
  session.dispose();
});

test('a build refuses an added extension component another component already is', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'scxml-extensions-'));
  try {
    await writeFile(
      join(dir, 'manifest.tsv'),
      `schema\tstateforward.wasm-artifacts/v1\nartifact\tscxml_extension_again\tscxml_extension_example.virtualized.wasm\t${exampleBytes.byteLength}\t${sha256(exampleBytes)}\n`,
    );
    const build = () =>
      execFileSync(process.execPath, [join(packageRoot, 'scripts/build.mjs'), '--artifacts', artifacts, '--meter', join(dir, 'unused'), '--extensions', dir, '--out', join(dir, 'dist')], { stdio: 'pipe' });
    assert.throws(build, (error) => /has the release digest of another component/.test(String(error.stderr)));
    await writeFile(join(dir, 'manifest.tsv'), `schema\tstateforward.wasm-artifacts/v1\nartifact\tscx\tscx.virtualized.wasm\t1\t${'0'.repeat(64)}\n`);
    assert.throws(build, (error) => /extension component scx is already built/.test(String(error.stderr)));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('bindings are checked as the Wasmtime host checks them', () => {
  const refused = (pkg, component = example, extensions = new ExtensionComponents()) =>
    assert.throws(() => extensions.bind(pkg, component), (error) => error instanceof ScxmlHostError && error.code === 'namespace-binding');
  refused({ ...examplePackage, namespaceUri: 'http://www.w3.org/2005/07/scxml' });
  refused({ ...examplePackage, namespaceUri: 'http://www.w3.org/XML/1998/namespace' });
  refused({ ...examplePackage, namespaceUri: 'not absolute' });
  refused({ ...examplePackage, schemaSha256: '0'.repeat(64) });
  refused({ ...examplePackage, schema: undefined });
  refused(examplePackage, example, new ExtensionComponents().bind(examplePackage, example));
  assert.throws(() => new ExtensionComponents().bind(examplePackage, { kind: 'other' }), (error) => error.code === 'artifact-kind-mismatch');
  assert.throws(() => new ExtensionComponents().withCallFuel(0), (error) => error.code === 'invalid-argument');
  assert.ok(new ExtensionComponents().isEmpty());
});

test('a namespace no binding names fails the session start closed', async () => {
  const { session } = await open(machine('<state id="s"><onentry><example:raise event="e"/></onentry></state>'));
  assert.throws(() => session.start(), (error) => error instanceof ScxmlSessionError && error.code === 'unavailable');
  session.dispose();
});

test('a trap retires the instance for its session only; each session has its own', async () => {
  const extensions = new ExtensionComponents().bind(examplePackage, example).withCallFuel(100_000);
  const source = machine(
    `<datamodel><data id="errors" expr="0"/></datamodel>
     <state id="s"><onentry><example:spin/></onentry><onentry><example:raise event="after"/></onentry>
       <transition event="error.execution"><assign location="errors" expr="errors + 1"/></transition>
       <transition event="after" target="raised"/>
       <transition event="check"><log label="errors" expr="errors"/></transition></state>
     <final id="raised"/>`,
  );
  const { session, logs } = await open(source, extensions);
  session.start();
  session.send({ name: 'check' });
  assert.deepEqual(logs, ['errors: 2'], 'the spin trapped, and the retired instance refused the raise');
  assert.deepEqual(session.configuration(), ['s']);
  session.dispose();
  // A fresh session gets a fresh instance.
  const fresh = await open(machine('<state id="s"><onentry><example:raise event="after"/></onentry><transition event="after" target="f"/></state><final id="f"/>'), extensions);
  fresh.session.start();
  assert.deepEqual(fresh.session.configuration(), ['f']);
  fresh.session.dispose();
});

test('an invoked child gets its own instance, compiled against the bound packages', async () => {
  const extensions = new ExtensionComponents().bind(examplePackage, example);
  const child = `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="${EXAMPLE}" version="1.0" datamodel="ecmascript" initial="c"><state id="c"><onentry><example:raise event="ran"/></onentry><transition event="ran" target="f"/></state><final id="f"><donedata><content expr="'child ran'"/></donedata></final></scxml>`;
  const { session, logs } = await open(
    machine(
      `<state id="s"><onentry><example:raise event="parent.ran"/></onentry><invoke type="scxml"><content>${child}</content></invoke>
         <transition event="done.invoke"><log label="child" expr="_event.data"/></transition>
         <transition event="parent.ran"><log label="parent" expr="'ran'"/></transition>
         <transition event="error.execution" target="failed"/></state><final id="failed"/>`,
    ),
    extensions,
  );
  session.start();
  assert.deepEqual(logs, ['parent: ran', 'child: child ran']);
  session.dispose();
});

test('async sessions do not take extension components', async () => {
  await assert.rejects(
    createSession({ ir: new Uint8Array(1), async: 'auto', extensionComponents: new ExtensionComponents() }),
    (error) => error.code === 'invalid-argument',
  );
  await assert.rejects(createSession({ ir: new Uint8Array(1), extensionComponents: {} }), (error) => error.code === 'invalid-argument');
});
