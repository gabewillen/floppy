// The bundled validator component in the browser host (src/validator.js).
//
// Parity with `scx validate`: every `tests/ui` case whose machines use
// exactly one checked data model (`ecmascript` or `xpath`) and no extra CLI
// arguments is validated with that data model linked, and the canonical
// report bytes must equal the case's `.json` snapshot, which `scx validate
// --format json` produced (the Wasmtime host's validator_component test
// holds its ComponentValidator to the same snapshots). The schema files the
// cases reference are supplied relative to `tests/ui`, as the CLI reads them.
//
// Then the request options (packages, event schema files, schema
// policies), argument validation, and fuel exhaustion.
import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { createHash } from 'node:crypto';

import {
  PackageValidators,
  ScxmlHostError,
  authenticatePackageValidatorComponent,
  cachedCoreModules,
  createEcmaScriptDataModel,
  createValidator,
  createXPathDataModel,
} from '../src/index.js';
import { readPackage } from './packages.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const ui = join(workspace, 'tests/ui');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const decode = (bytes) => new TextDecoder().decode(bytes);
const codes = (report) => JSON.parse(decode(report)).validator.diagnostics.map((diagnostic) => diagnostic.code);

const ecmascript = await createValidator({ loadCoreModule, dataModel: await createEcmaScriptDataModel({ loadCoreModule }) });
const xpath = await createValidator({ loadCoreModule, dataModel: await createXPathDataModel({ loadCoreModule }) });

/** The one checked data model every machine of `source` declares, if any (the Wasmtime test's rule). */
function singleCheckedDataModel(source) {
  const declares = (name) => source.includes(`datamodel="${name}"`);
  if (declares('ecmascript') && !declares('xpath')) return 'ecmascript';
  if (declares('xpath') && !declares('ecmascript')) return 'xpath';
  return undefined;
}

test('validator reports equal the scx validate snapshots byte for byte', async () => {
  const eventSchemas = await Promise.all(
    (await readdir(join(ui, 'event-schemas'))).sort().map(async (name) => ({
      uri: `event-schemas/${name}`,
      bytes: new Uint8Array(await readFile(join(ui, 'event-schemas', name))),
    })),
  );
  const cases = (await readdir(ui)).filter((name) => name.endsWith('.scx')).map((name) => name.slice(0, -4)).sort();
  const mismatches = [];
  const compared = [];
  for (const name of cases) {
    const source = await readFile(join(ui, `${name}.scx`), 'utf8');
    if (source.trimStart().startsWith('<?ui-args')) continue;
    const dataModel = singleCheckedDataModel(source);
    if (dataModel === undefined) continue;
    const validator = dataModel === 'xpath' ? xpath : ecmascript;
    const expected = (await readFile(join(ui, `${name}.json`), 'utf8')).trimEnd();
    // The documents under tests/ui/invoked/ its literal `src` attributes name,
    // as `scx validate` reads them (their schemas are inline).
    const invokedDocuments = [];
    for (const [, src] of source.matchAll(/src="(invoked\/[^"]*)"/g)) {
      if (invokedDocuments.some((document) => document.src === src)) continue;
      invokedDocuments.push({ src, text: await readFile(join(ui, src), 'utf8') });
    }
    const { report } = await validator.validate(source, { sourceName: `tests/ui/${name}.scx`, eventSchemas, invokedDocuments });
    const actual = decode(report).trimEnd();
    if (actual !== expected) mismatches.push(name);
    compared.push(name);
  }
  assert.deepEqual(mismatches, []);
  assert.ok(compared.length > 20, `compared ${compared.length} cases`);
  for (const required of ['scxml038-expression-syntax-error', 'scxml038-xpath-mixed-habits', 'scxml074-log-side-effects', 'event011-incompatible-invoked-event-data']) {
    assert.ok(compared.includes(required), required);
  }
});

test('packages and event schema files reach the validator', async () => {
  const pkg = await readPackage('components/scxml-extension-example/package/example.extension.json');
  const extension = `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="${pkg.namespaceUri}" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><onentry><example:raise event="e"/></onentry></state></scxml>`;
  const unbound = codes((await ecmascript.validate(extension)).report);
  assert.ok(unbound.includes('XSD001'), `strict: an unbound namespace has no schema: ${unbound}`);
  assert.ok(!codes((await ecmascript.validate(extension, { packages: [pkg] })).report).includes('XSD001'));
  assert.ok(!codes((await ecmascript.validate(extension, { schemaPolicy: 'lax' })).report).includes('XSD001'));

  const referencing = `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><transition event="go" target="f" event:schema="schemas/go.json"/></state><final id="f"/></scxml>`;
  assert.ok(codes((await ecmascript.validate(referencing)).report).includes('EVENT004'));
  const schema = { uri: 'schemas/go.json', bytes: new TextEncoder().encode('{"type": "object"}') };
  assert.deepEqual(codes((await ecmascript.validate(referencing, { eventSchemas: [schema] })).report), []);

  // The documents its literal `<invoke src>` elements load reach EVENT011,
  // each with its own schema files.
  const parent = `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><invoke id="kid" src="child.scx"/><onentry><send event="go" target="#_kid"><content>{"n": 1}</content></send></onentry></state></scxml>`;
  const child = { src: 'child.scx', text: referencing, eventSchemas: [{ uri: 'schemas/go.json', bytes: new TextEncoder().encode('{"type": "array"}') }] };
  assert.deepEqual(codes((await ecmascript.validate(parent)).report), []);
  assert.deepEqual(codes((await ecmascript.validate(parent, { invokedDocuments: [child] })).report), ['EVENT011']);
  assert.deepEqual(codes((await ecmascript.validate(parent, { invokedDocuments: [{ ...child, eventSchemas: [schema] }] })).report), []);
});

test('malformed arguments are invalid-argument and the instance stays usable', async () => {
  for (const [source, options] of [
    [1, undefined],
    ['<scxml/>', { schemaPolicy: 'none' }],
    ['<scxml/>', { packages: [{}] }],
    ['<scxml/>', { eventSchemas: [{ uri: 'x' }] }],
    ['<scxml/>', { invokedDocuments: [{ src: 'x' }] }],
    ['<scxml/>', { invokedDocuments: [{ src: 'x', text: '', eventSchemas: [{ uri: 'x' }] }] }],
  ]) {
    await assert.rejects(ecmascript.validate(source, options), (error) => error instanceof ScxmlHostError && error.code === 'invalid-argument');
  }
  await assert.rejects(createValidator({ dataModel: {} }), (error) => error.code === 'invalid-argument');
  await assert.rejects(createValidator({ fuel: 0 }), (error) => error.code === 'invalid-argument');
  assert.equal(JSON.parse(decode((await ecmascript.validate('<scxml/>')).report)).kind, 'validator');
});

test('a validation that runs out of fuel loses the instance', async () => {
  const starved = await createValidator({ loadCoreModule, fuel: 1_000_000 });
  await assert.rejects(starved.validate('<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"/>'), (error) => error.code === 'fuel-exhausted');
  await assert.rejects(starved.validate('<scxml/>'), (error) => error.code === 'instance-lost');
});

test('package validators report with the document prefix and fail closed (Wasmtime parity)', async () => {
  const artifacts = process.env.SCXML_BROWSER_ARTIFACTS ?? join(workspace, 'target/wasm-artifacts/browser-host');
  const bytes = new Uint8Array(await readFile(join(artifacts, 'scxml_package_validator_example.virtualized.wasm')));
  const digest = createHash('sha256').update(bytes).digest('hex');
  await assert.rejects(authenticatePackageValidatorComponent(bytes, '0'.repeat(64)), (error) => error.code === 'artifact-digest-mismatch');
  const component = await authenticatePackageValidatorComponent(bytes, digest);
  const pkg = await readPackage('components/scxml-extension-example/package/example.extension.json');
  const packageValidators = new PackageValidators().bind(pkg.namespaceUri, component);
  assert.throws(() => packageValidators.bind(pkg.namespaceUri, component), (error) => error.code === 'namespace-binding');
  const validator = await createValidator({ loadCoreModule, packageValidators });
  const document = (raise) =>
    `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:ex="${pkg.namespaceUri}" version="1.0" initial="busy"><final id="busy"><onentry>${raise}</onentry></final></scxml>`;
  const typo = JSON.parse(decode((await validator.validate(document('<ex:raise event="e" in="bsy"/>'), { packages: [pkg] })).report));
  const finding = typo.validator.diagnostics.find((diagnostic) => diagnostic.code === 'ex::unknown_state');
  assert.ok(finding, JSON.stringify(typo));
  assert.equal(finding.fixes[0].applicability, 'machine-applicable');
  assert.equal(finding.fixes[0].edits[0].replacement, 'in="busy"');
  assert.deepEqual(codes((await validator.validate(document('<ex:raise event="e" in="busy"/>'), { packages: [pkg] })).report), []);
  for (const event of ['example.validator.trap', 'example.validator.spin']) {
    const report = (await validator.validate(document(`<ex:raise event="${event}"/>`), { packages: [pkg] })).report;
    assert.deepEqual(codes(report), ['PKG002'], event);
  }
  // Unbound: no package validator answers.
  const unlinked = await createValidator({ loadCoreModule });
  assert.deepEqual(codes((await unlinked.validate(document('<ex:raise event="e" in="bsy"/>'), { packages: [pkg] })).report), []);
});
