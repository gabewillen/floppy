// Cross-host parity on the official W3C IRP corpus: every prepared case the
// browser host supports runs through the browser host and must reach the
// disposition and final configuration the checked-in baseline records for
// the Wasmtime session host (tests/conformance/w3c-irp/baselines, produced by
// `scx conformance run` through the shipped components).
//
// Scope, per data-model variant (ECMAScript and XPath):
// * compared: every case decided by the `Outcome` log oracle, `<invoke>`
//   and BasicHTTP cases included: the host compiles child documents itself
//   (the default `invoke.compile: 'auto'`; the harness supplies no
//   `compileInvocation`), and the harness links the BasicHTTP processor
//   over the Wasmtime host's in-process loopback (`http.send` routed to `session.receiveHttp`; see
//   test/irp-harness.js). The BasicHTTP cases are counted separately
//   (`basichttp`) and are compared like every other case. Documents
//   (`<data src>`, `<invoke src>`) load through the host's `resources`
//   option, allowlisted to the case's prepared dependencies and prefetched
//   before the run; only a runtime `srcexpr` reaches the synchronous
//   `loadDocument` fallback, and exactly the `srcexpr` cases may use it;
// * snapshot non-perturbation (docs/session-snapshot.md, "Non-perturbation
//   proof over the IRP"): every case that runs, runs twice, once plain and
//   once with a read-only snapshot at every checkpoint (after start, after
//   each scripted event, after each delivery pass). Both runs must agree
//   exactly: disposition, logs, final configuration and status, command
//   count, clock, and the full observation trace. Every snapshot must be a
//   document: any snapshot error fails the case;
// * not run: the W3C manual cases (decided by the manual-equivalent oracle
//   over the Wasmtime run's trace) and cases the baseline marks
//   not applicable.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { cachedCoreModules, createCompiler } from '../src/index.js';
import { runIrpCase } from './irp-harness.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const irp = join(workspace, 'tests/conformance/w3c-irp');
const loadCoreModule = cachedCoreModules((component, file) =>
  readFile(join(packageRoot, 'dist', component, file)),
);
const compiler = await createCompiler({ loadCoreModule });

/**
 * The exact scope per variant, as docs/conformance.md and this package's
 * README publish it: a case that moves between the tallies (for
 * example a compared case misclassified as outside the slice) fails here.
 */
const EXPECTED_TALLIES = {
  ecmascript: { compared: 194, basichttp: 12, notRun: 8 },
  xpath: { compared: 172, basichttp: 12, notRun: 30 },
};

for (const variant of ['ecmascript', 'xpath']) {
  test(`browser and Wasmtime agree on the supported official ${variant} IRP cases`, async () => {
    const dir = join(irp, 'prepared', variant);
    const readText = (path) => readFile(join(dir, path), 'utf8');
    const corpus = JSON.parse(await readText('corpus.json'));
    const baseline = JSON.parse(await readFile(join(irp, 'baselines', `${variant}.scxml-report.json`), 'utf8'));
    const expected = new Map(baseline.conformance.cases.map((entry) => [entry.id, entry]));
    const tally = { compared: 0, basichttp: 0, notRun: 0, snapshots: 0, resourceCases: 0 };
    const dynamicCases = [];
    const srcexprCases = [];
    const mismatches = [];
    for (const testCase of corpus.cases) {
      const recorded = expected.get(testCase.id);
      assert.ok(recorded, `baseline records case ${testCase.id}`);
      if (recorded.oracle !== 'outcome-log' || recorded.disposition === 'not-applicable') {
        tally.notRun += 1;
        continue;
      }
      const documents = await Promise.all([testCase.source, ...testCase.dependencies].map(readText));
      if (documents.some((text) => /BasicHTTP|basichttp/.test(text))) tally.basichttp += 1;
      const run = (checkpoint) =>
        runIrpCase({
          testCase,
          commandBudget: corpus.default_command_budget,
          readText,
          compiler,
          basichttp: true,
          loadCoreModule,
          trace: true,
          checkpoint,
        });
      const result = await run();
      const snapshots = [];
      const snapshotted = await run(async (session) => {
        try {
          snapshots.push({ bytes: await session.snapshot() });
        } catch (error) {
          snapshots.push({ error });
        }
      });
      assert.deepEqual(snapshotted, result, `case ${testCase.id}: snapshots perturbed the run`);
      for (const snapshot of snapshots) {
        assert.equal(snapshot.error, undefined, `case ${testCase.id}: ${snapshot.error}`);
        assert.ok(snapshot.bytes instanceof Uint8Array && snapshot.bytes.byteLength > 0);
      }
      tally.snapshots += snapshots.length;
      tally.compared += 1;
      if (documents.some((text) => /<(?:data|invoke|script)\b[^>]*\ssrc="/.test(text))) tally.resourceCases += 1;
      if (documents.some((text) => /\ssrcexpr="/.test(text))) srcexprCases.push(testCase.id);
      if (result.dynamicLoads > 0) dynamicCases.push(testCase.id);
      if (
        result.disposition !== recorded.disposition ||
        JSON.stringify(result.configuration) !== JSON.stringify(recorded.configuration)
      ) {
        mismatches.push({ id: testCase.id, expected: [recorded.disposition, recorded.configuration], result });
      }
    }
    assert.deepEqual(mismatches, []);
    console.log(`irp-parity ${variant}: ${JSON.stringify({ ...tally, dynamicCases })}`);
    assert.ok(tally.resourceCases > 0, 'static src cases load through resources');
    assert.deepEqual(
      dynamicCases.filter((id) => !srcexprCases.includes(id)),
      [],
      'only srcexpr cases use the synchronous loadDocument fallback',
    );
    const { snapshots, resourceCases, ...tallies } = tally;
    assert.deepEqual(tallies, EXPECTED_TALLIES[variant]);
    assert.ok(snapshots > tally.compared, 'a snapshot at every checkpoint of every case');
  });
}
