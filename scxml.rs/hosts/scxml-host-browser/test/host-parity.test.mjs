// Cross-host parity corpus (tests/fixtures/host-parity/cases.json): every
// case runs through the browser host (test/parity-harness.js) and must reach
// exactly the outcome the checked-in golden records for the Wasmtime host
// (`scx run`, crates/scxml-cli/tests/host_parity.rs): the root session's
// `<log>` records in order, its final configuration and status, and its
// `<donedata>` value. test/browser.test.mjs runs the same corpus in
// headless Chromium.
//
// `SCXML_BROWSER_ARTIFACTS` names the release directory the browser host's
// dist was built from (scripts/verify-browser-host.sh sets it); the
// extension component bytes come from it.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { cachedCoreModules, createCompiler, createEcmaScriptDataModel, createValidator } from '../src/index.js';
import { runParityCase } from './parity-harness.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const fixtures = join(workspace, 'tests/fixtures/host-parity');
const artifacts = process.env.SCXML_BROWSER_ARTIFACTS ?? join(workspace, 'target/wasm-artifacts/browser-host');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const read = async (path) => new Uint8Array(await readFile(join(workspace, path)));
const component = async (file) => new Uint8Array(await readFile(join(artifacts, file)));
const sha256 = async (bytes) => createHash('sha256').update(bytes).digest('hex');

const compiler = await createCompiler({ loadCoreModule });
const validator = await createValidator({ loadCoreModule, dataModel: await createEcmaScriptDataModel({ loadCoreModule }) });
const { cases, version } = JSON.parse(await readFile(join(fixtures, 'cases.json'), 'utf8'));

test('the parity corpus covers extension elements, scx, and an event:schema file', () => {
  assert.equal(version, 1);
  assert.ok(cases.some((testCase) => testCase.extensions.includes('https://stateforward.org/scxml/example')));
  assert.ok(cases.some((testCase) => testCase.extensions.includes('https://github.com/stateforward/scxml.rs/namespaces/scx')));
  assert.ok(cases.some((testCase) => (testCase.eventSchemas ?? []).length > 0));
});

for (const testCase of cases) {
  test(`browser host matches the Wasmtime golden: ${testCase.name}`, async () => {
    const result = await runParityCase({ testCase, read, component, sha256, compiler, validator, loadCoreModule });
    assert.equal(result.validation, undefined, JSON.stringify(result.validation));
    const golden = JSON.parse(await readFile(join(fixtures, `${testCase.name}.golden.json`), 'utf8'));
    assert.deepEqual(result.outcome, golden);
  });
}
