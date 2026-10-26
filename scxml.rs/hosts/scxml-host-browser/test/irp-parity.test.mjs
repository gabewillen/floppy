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
import { dirname, join, posix, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { cachedCoreModules, createCompiler } from '../src/index.js';
import { readPackage } from './packages.js';
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

// Every repository namespace's own suite (namespaces/<name>/irp) against its
// canonical baseline, through the same pipeline as `scx conformance run`:
// the compiler (schema policy off) with the extension packages the suite
// binds (the `env` suite binds the example namespace; the `event` suite
// none) and each case's dependencies as its `event:schema` files, keyed by
// their relative path from its document's directory (`..` included). A
// document-rejection case passes when the compiler rejects it with exactly
// the listed error codes (the oracle compares the set of codes); every
// other case runs like an official one. Every case must reach the
// disposition (and, for a run, the final configuration) the baseline
// records.
const NAMESPACE_SUITES = {
  env: ['components/scxml-extension-example/package/example.extension.json'],
  event: [],
};
const NAMESPACE_TALLIES = {
  env: { rejections: 8, runs: 0 },
  event: { rejections: 16, runs: 15 },
};

for (const [name, manifests] of Object.entries(NAMESPACE_SUITES)) {
  for (const variant of ['ecmascript', 'xpath']) {
    test(`browser and Wasmtime agree on the ${name} namespace suite (${variant})`, async () => {
      const suite = join(workspace, 'namespaces', name, 'irp');
      const dir = join(suite, 'prepared', variant);
      const readText = (path) => readFile(join(dir, path), 'utf8');
      const corpus = JSON.parse(await readText('corpus.json'));
      const baseline = JSON.parse(await readFile(join(suite, 'baselines', `${variant}.scxml-report.json`), 'utf8'));
      const expected = new Map(baseline.conformance.cases.map((entry) => [entry.id, entry]));
      const packages = await Promise.all(manifests.map(readPackage));
      const tally = { rejections: 0, runs: 0 };
      const mismatches = [];
      for (const testCase of corpus.cases) {
        const recorded = expected.get(testCase.id);
        assert.ok(recorded, `baseline records case ${testCase.id}`);
        const directory = testCase.start_uri.includes('/') ? testCase.start_uri.slice(0, testCase.start_uri.lastIndexOf('/') + 1) : '';
        const eventSchemas = await Promise.all(
          Object.entries(testCase.dependency_source_artifacts ?? {})
            .map(async ([uri, path]) => ({
              uri: posix.relative(directory || '.', uri),
              bytes: new Uint8Array(await readFile(join(dir, path))),
            })),
        );
        if (testCase.expected.kind === 'document-rejection') {
          tally.rejections += 1;
          const source = await readText(testCase.source);
          const compiled = await compiler.compile(source, {
            sourceName: testCase.source.slice(testCase.source.lastIndexOf('/') + 1),
            packages,
            eventSchemas,
          });
          const codes = new Set(
            JSON.parse(new TextDecoder().decode(compiled.report))
              .compiler.diagnostics.filter((diagnostic) => diagnostic.severity === 'error')
              .map((diagnostic) => diagnostic.code),
          );
          const disposition =
            compiled.status === 'rejected' &&
            codes.size === new Set(testCase.expected.codes).size &&
            testCase.expected.codes.every((code) => codes.has(code))
              ? 'passed'
              : 'failed';
          if (disposition !== recorded.disposition) mismatches.push({ id: testCase.id, codes: [...codes], recorded: recorded.disposition });
          continue;
        }
        tally.runs += 1;
        const result = await runIrpCase({
          testCase,
          commandBudget: corpus.default_command_budget,
          readText,
          compiler,
          loadCoreModule,
          eventSchemas,
        });
        if (
          result.disposition !== recorded.disposition ||
          JSON.stringify(result.configuration) !== JSON.stringify(recorded.configuration)
        ) {
          mismatches.push({ id: testCase.id, expected: [recorded.disposition, recorded.configuration], result });
        }
      }
      assert.deepEqual(mismatches, []);
      assert.deepEqual(tally, NAMESPACE_TALLIES[name]);
    });
  }
}
