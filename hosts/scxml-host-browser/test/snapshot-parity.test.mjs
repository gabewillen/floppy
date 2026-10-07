// Golden parity of the browser host's read-only session snapshot
// (docs/session-snapshot.md): every
// `tests/fixtures/session-snapshot/<name>.ops.json` in the directory is
// replayed through the same shipped components the Wasmtime golden harness
// (`hosts/scxml-host-wasmtime/tests/session_snapshot.rs`) runs, and each
// requested checkpoint's document must equal the harness's
// `golden/<name>/NNNN.snapshot.json`.
//
// Every fixture runs twice: once with the BasicHTTP processor configured
// the way the Wasmtime session host links it (`http.location` is each
// session's loopback location), and once without `http`. Every shared
// checkpoint is compared byte for byte, after normalising the component
// digests and removing the documented host differences
// (docs/session-snapshot.md, "Parity fixtures") from both documents:
//
// - `snapshot.components[*].sha256`, always: each is set to a fixed
//   all-zero placeholder. Component digests identify the build that
//   produced the components, and release component bytes differ between
//   build hosts. The goldens already store the placeholder. Before it is
//   replaced, each produced digest must equal the release digest
//   `dist/build-manifest.json` records for the component loaded for that
//   role, so a swapped role-to-digest mapping fails.
// - `system._ioprocessors`, only in the run without `http`: the Wasmtime
//   host always links the BasicHTTP Event I/O Processor, and the browser
//   host advertises it only when the page configures `http` (its README,
//   "BasicHTTP"). The `basichttp` and
//   `http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor` entries are then
//   removed from every session's `system.ioprocessors` value. With `http`
//   configured the value must match byte for byte.
// - Event trace carriers, only when `options.trace_context` is `preserve`:
//   each embedder's telemetry provider mints them (the Wasmtime harness
//   pins sequential IDs, the browser runs on the page's global provider).
//   The `trace_context` of each event in `queues` and `pending_sends` is
//   removed; `options.trace_context` is never touched. Under `strip` there
//   is no carrier exception.
//
// The envelope `sha256` (the digest of the canonical `snapshot` bytes) is
// then re-derived. The test asserts that normalisation changed no other
// path, and that each document re-serialises to its exact bytes, then
// compares the normalised bytes exactly.
//
// Ops vocabulary (the Wasmtime harness's): `options` `{exclude (snake_case,
// sorted), trace_context}` and `ops`, with `start` exactly first, no `send`
// after an `advance`, and strictly ascending `snapshot` checkpoints. A
// `snapshot` op marked `"hosts": "wasmtime"` is a checkpoint only the
// Wasmtime scheduler reaches (mid-drain: a browser `send` settles fully)
// and is the only kind skipped here; every other checkpoint must match.
//
// Coverage fails closed: there must be fixtures; every fixture must compare
// at least one shared checkpoint; the checkpoints its ops request (marked or
// not) must be exactly the files in `golden/<name>/`; every `golden/` and
// `diff/` directory must belong to a fixture; and the codec's diff of each
// pair of consecutive goldens must equal `diff/<name>/NNNN.diff.json`. Any
// snapshot error fails the test.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  CapabilityError,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createNullDataModel,
  createSession,
  createXPathDataModel,
  diffSnapshots,
} from '../src/index.js';
import { loopbackLocation } from './irp-harness.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const fixtures = join(workspace, 'tests', 'fixtures', 'session-snapshot');
const readDist = (component, file) => readFile(join(packageRoot, 'dist', component, file));
const loadCoreModule = cachedCoreModules(readDist);

const DATA_MODELS = {
  ecmascript: createEcmaScriptDataModel,
  xpath: createXPathDataModel,
  null: createNullDataModel,
};

/** `readFile`, synchronous: a `loadDocument` provider returns text directly. */
const readFixture = (path) => readFileSync(path, 'utf8');

/** `pending_sends` -> `pending-sends`: sections are snake_case in ops, kebab in options. */
const kebab = (sections) => sections.map((section) => section.replaceAll('_', '-'));

/** One parsed `<name>.ops.json`. */
async function loadFixtures() {
  const names = (await readdir(fixtures))
    .filter((name) => name.endsWith('.ops.json'))
    .map((name) => name.slice(0, -'.ops.json'.length))
    .sort();
  return Promise.all(
    names.map(async (name) => ({
      name,
      ops: JSON.parse(await readFile(join(fixtures, `${name}.ops.json`), 'utf8')),
      source: await readFile(join(fixtures, `${name}.scx`), 'utf8'),
    })),
  );
}

/**
 * The embedder the Wasmtime harness mirrors: `loadDocument` resolves an
 * exact `src` to a file beside the fixture; `compileInvocation` answers
 * from IR precompiled for every fixture document, keyed by source text.
 */
async function embedder(compiler) {
  const lowered = new Map();
  for (const name of await readdir(fixtures)) {
    if (name.endsWith('.scx')) {
      const source = await readFile(join(fixtures, name), 'utf8');
      const compiled = await compiler.compile(source, { sourceName: name });
      lowered.set(source, compiled.status === 'emitted' ? compiled.ir : undefined);
    }
  }
  return {
    // The Wasmtime harness's embedder: an exact `src` resolves to a file
    // beside the fixture; a plain name only, no traversal. Synchronous: a
    // provider that returns a malformed value is `platform`.
    loadDocument: (kind, uri) => {
      void kind;
      if (uri.length === 0 || uri.includes('/') || uri.startsWith('.')) {
        throw new CapabilityError('unavailable');
      }
      return readFixture(join(fixtures, uri));
    },
    compileInvocation: (sourceName, source) => {
      void sourceName;
      const ir = lowered.get(source);
      if (ir === undefined) throw new CapabilityError('invalid-input');
      return ir;
    },
  };
}

/** The BasicHTTP Event I/O Processor keys of `_ioprocessors`, which the browser host advertises only with `http`. */
const BASICHTTP_KEYS = ['basichttp', 'http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor'];

/** Every session of a snapshot (root first, then invoked children, depth first) with its JSON path. */
function sessions(snapshot) {
  const found = [];
  const visit = (session, path) => {
    found.push({ session, path });
    for (const [index, invocation] of (session.invocations ?? []).entries()) {
      if (invocation.session !== undefined) visit(invocation.session, `${path}/invocations/${index}/session`);
    }
  };
  if (snapshot.root !== undefined) visit(snapshot.root, '/snapshot/root');
  return found;
}

/** The JSON paths at which two parsed values differ. */
function differingPaths(left, right, path = '') {
  if (left === right) return [];
  const bothObjects =
    left !== null &&
    right !== null &&
    typeof left === 'object' &&
    typeof right === 'object' &&
    Array.isArray(left) === Array.isArray(right);
  if (!bothObjects) return [path];
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  return [...keys].flatMap((key) => differingPaths(left[key], right[key], `${path}/${key}`));
}

/**
 * The value every `snapshot.components[*].sha256` is normalised to, on both
 * sides: component digests identify the build that produced the components,
 * not the snapshot content. The goldens store it (the Wasmtime harness
 * writes them normalised), so they are identical on every build host.
 */
const COMPONENT_DIGEST_PLACEHOLDER = '0'.repeat(64);

/** The `dist/` component (build-manifest name) the browser host loads for each snapshot role. */
const ROLE_COMPONENTS = {
  clock: 'clock',
  'data-model-ecmascript': 'data-model-ecmascript',
  'data-model-null': 'data-model-null',
  'data-model-xpath': 'data-model-xpath',
  'io-processor-scxml': 'ioprocessor-scxml',
  machine: 'machine',
};

/**
 * The release SHA-256 of the component loaded for each snapshot role, read
 * from `dist/build-manifest.json` (the source digest `scripts/build.mjs`
 * authenticated for each transpiled component). A produced document's
 * component digests must equal these before normalisation replaces them.
 */
function releaseDigests() {
  const manifest = JSON.parse(readFileSync(join(packageRoot, 'dist', 'build-manifest.json'), 'utf8'));
  const sources = new Map(manifest.components.map((component) => [component.name, component.source_sha256]));
  return Object.fromEntries(
    Object.entries(ROLE_COMPONENTS).map(([role, name]) => {
      const digest = sources.get(name);
      assert.match(digest ?? '', /^[0-9a-f]{64}$/, `build-manifest.json records ${name}`);
      return [role, digest];
    }),
  );
}

/** Where an event trace carrier may sit: an event in a queue or a pending send, in any session. */
const CARRIER_PATH =
  /^\/snapshot\/root(\/invocations\/\d+\/session)?\/(queues\/[a-z_]+\/\d+(\/event)?|pending_sends\/\d+\/event)\/trace_context$/;

/** Removes every `trace_context` key under `value` (the `snapshot` object), returning the removed paths. */
function removeCarriers(value, path, removed = []) {
  if (Array.isArray(value)) {
    value.forEach((item, index) => removeCarriers(item, `${path}/${index}`, removed));
  } else if (value !== null && typeof value === 'object') {
    if (Object.hasOwn(value, 'trace_context')) {
      delete value.trace_context;
      removed.push(`${path}/trace_context`);
    }
    for (const [key, child] of Object.entries(value)) removeCarriers(child, `${path}/${key}`, removed);
  }
  return removed;
}

/**
 * One snapshot document with every component digest replaced by
 * `COMPONENT_DIGEST_PLACEHOLDER` and the documented host differences
 * removed: the BasicHTTP entries of each session's `system.ioprocessors`
 * value (only when `basichttp` is false: the browser run had no `http`
 * option), and,
 * under `preserve` only, each event's trace carrier; `sha256` is then
 * re-derived over the canonical `snapshot` bytes. Fails unless the text
 * round-trips through `JSON.stringify` exactly (so the re-serialisation is
 * canonical) and unless only those paths changed. With `digests` (a
 * produced document), each component digest must first equal the release
 * digest of its role; without it (a golden), each must be the placeholder.
 */
function normalize(text, label, basichttp, digests) {
  const original = JSON.parse(text);
  assert.equal(JSON.stringify(original), text, `${label}: the document re-serialises to its exact bytes`);
  const document = structuredClone(original);
  const allowed = new Set();
  for (const { session, path } of sessions(document.snapshot)) {
    const processors = session.system?.ioprocessors;
    if (processors === undefined || basichttp) continue;
    assert.equal(processors.type, 'json-object', `${label}: ${path}/system/ioprocessors is a JSON object`);
    const map = JSON.parse(processors.value);
    assert.equal(JSON.stringify(map), processors.value, `${label}: ${path}/system/ioprocessors re-serialises`);
    if (!BASICHTTP_KEYS.some((key) => Object.hasOwn(map, key))) continue;
    for (const key of BASICHTTP_KEYS) delete map[key];
    processors.value = JSON.stringify(map);
    allowed.add(`${path}/system/ioprocessors/value`);
  }
  for (const [index, component] of (document.snapshot.components ?? []).entries()) {
    assert.equal(
      component.sha256,
      digests === undefined ? COMPONENT_DIGEST_PLACEHOLDER : digests[component.role],
      `${label}: the ${component.role} digest is ${digests === undefined ? 'the placeholder' : 'that of the component loaded for the role'}`,
    );
    if (component.sha256 === COMPONENT_DIGEST_PLACEHOLDER) continue;
    component.sha256 = COMPONENT_DIGEST_PLACEHOLDER;
    allowed.add(`/snapshot/components/${index}/sha256`);
  }
  if (document.options.trace_context === 'preserve') {
    for (const path of removeCarriers(document.snapshot, '/snapshot')) {
      assert.match(path, CARRIER_PATH, `${label}: ${path} is an event trace carrier`);
      allowed.add(path);
    }
  }
  if (allowed.size > 0) {
    document.sha256 = createHash('sha256').update(JSON.stringify(document.snapshot)).digest('hex');
    allowed.add('/sha256');
  }
  assert.deepEqual(
    differingPaths(original, document).filter((path) => !allowed.has(path)),
    [],
    `${label}: normalisation changes only component digests, system.ioprocessors, preserve-mode event carriers, and sha256`,
  );
  return Buffer.from(JSON.stringify(document));
}

/** `NNNN` of one checkpoint. */
const padded = (checkpoint) => String(checkpoint).padStart(4, '0');

/** Sorted entries of `fixtures/<kind>`, or of `fixtures/<kind>/<name>`; `[]` when absent. */
async function listing(...parts) {
  try {
    return (await readdir(join(fixtures, ...parts))).sort();
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
}

/** The `snapshot` ops of one fixture, each `{checkpoint, shared}`; malformed markers fail. */
function snapshotOps(fixture) {
  return fixture.ops.ops
    .filter((op) => op.op === 'snapshot')
    .map((op) => {
      assert.ok(
        op.hosts === undefined || op.hosts === 'wasmtime',
        `${fixture.name}: checkpoint ${op.checkpoint}: hosts is absent or "wasmtime"`,
      );
      assert.ok(Number.isSafeInteger(op.checkpoint) && op.checkpoint >= 0, `${fixture.name}: checkpoint index`);
      return { checkpoint: op.checkpoint, shared: op.hosts === undefined };
    });
}

test('every golden and diff directory belongs to a fixture', async () => {
  const names = (await loadFixtures()).map((fixture) => fixture.name);
  assert.ok(names.length > 0, 'session snapshot fixtures exist');
  assert.deepEqual(await listing('golden'), names, 'golden/ holds exactly one directory per fixture');
  for (const name of await listing('diff')) assert.ok(names.includes(name), `diff/${name} belongs to a fixture`);
});

for (const basichttp of [true, false]) {
  test(`browser snapshots equal the Wasmtime goldens (${basichttp ? 'BasicHTTP configured' : 'no http option'})`, async () => {
    const fixtures_ = await loadFixtures();
    assert.ok(fixtures_.length > 0, 'session snapshot fixtures exist');
    const digests = releaseDigests();
    const compiler = await createCompiler({ loadCoreModule });
    const capabilities = await embedder(compiler);
    for (const fixture of fixtures_) {
      await test(`${fixture.name}`, async () => {
        const requested = snapshotOps(fixture);
        assert.deepEqual(
          await listing('golden', fixture.name),
          requested.map(({ checkpoint }) => `${padded(checkpoint)}.snapshot.json`),
          `${fixture.name}: golden/ holds exactly the requested checkpoints`,
        );
        const compiled = await compiler.compile(fixture.source, { sourceName: `${fixture.name}.scx` });
        assert.equal(compiled.status, 'emitted', `${fixture.name}: the fixture compiles`);
        const { data_model: dataModelName } = JSON.parse(new TextDecoder().decode(compiled.ir));
        const factory = DATA_MODELS[dataModelName ?? ''];
        const dataModel = factory === undefined ? undefined : await factory({ loadCoreModule });
        const session = await createSession({
          ir: compiled.ir,
          dataModel,
          sessionId: 'session',
          timers: 'manual',
          maxCommands: 4_096,
          loadCoreModule,
          capabilities,
          // Wasmtime's links: each session's loopback location, nothing sent.
          ...(basichttp ? { http: { allow: () => false, location: loopbackLocation } } : {}),
        });
        let compared = 0;
        try {
          const { exclude, trace_context: traceContext } = fixture.ops.options;
          const options = { exclude: kebab(exclude), traceContext };
          for (const [index, op] of fixture.ops.ops.entries()) {
            if (op.op === 'start') {
              assert.equal(index, 0, `${fixture.name}: start is the first op`);
              session.start();
            } else if (op.op === 'send') {
              session.send(op.event);
            } else if (op.op === 'advance') {
              const { complete } = session.advanceTime(op.ms);
              assert.equal(complete, true, `${fixture.name}: op ${index}: the advance completes`);
            } else if (op.op === 'snapshot') {
              // Only the Wasmtime scheduler reaches a marked (mid-drain) checkpoint.
              if (op.hosts === 'wasmtime') continue;
              const bytes = await session.snapshot(options);
              const golden = await readFile(
                join(fixtures, 'golden', fixture.name, `${padded(op.checkpoint)}.snapshot.json`),
              );
              const label = `${fixture.name}: checkpoint ${op.checkpoint}`;
              const expected = normalize(new TextDecoder().decode(golden), `${label} golden`, basichttp);
              const produced = normalize(new TextDecoder().decode(bytes), `${label} browser`, basichttp, digests);
              if (!produced.equals(expected)) {
                const paths = differingPaths(
                  JSON.parse(produced.toString('utf8')),
                  JSON.parse(expected.toString('utf8')),
                );
                assert.fail(`${label}: the bytes differ from the normalised golden at ${JSON.stringify(paths)}`);
              }
              compared += 1;
            } else {
              assert.fail(`${fixture.name}: op ${index}: unknown op ${op.op}`);
            }
          }
        } finally {
          session.dispose();
        }
        assert.ok(compared > 0, `${fixture.name}: at least one checkpoint both hosts reach is compared`);
      });
    }
  });
}

test('the codec diff of consecutive goldens equals the diff goldens', async () => {
  const fixtures_ = await loadFixtures();
  assert.ok(fixtures_.length > 0, 'session snapshot fixtures exist');
  for (const fixture of fixtures_) {
    const checkpoints = snapshotOps(fixture).map(({ checkpoint }) => checkpoint);
    const read = (checkpoint) =>
      readFile(join(fixtures, 'golden', fixture.name, `${padded(checkpoint)}.snapshot.json`));
    const expected = [];
    for (let index = 1; index < checkpoints.length; index += 1) {
      const name = `${padded(checkpoints[index])}.diff.json`;
      expected.push(name, `${padded(checkpoints[index])}.diff.txt`);
      const produced = await diffSnapshots(
        new Uint8Array(await read(checkpoints[index - 1])),
        new Uint8Array(await read(checkpoints[index])),
        { loadCoreModule },
      );
      const golden = await readFile(join(fixtures, 'diff', fixture.name, name));
      assert.deepEqual(produced, new Uint8Array(golden), `${fixture.name}: ${name}`);
    }
    assert.deepEqual(await listing('diff', fixture.name), expected.sort(), `${fixture.name}: diff/ holds exactly the pairs`);
  }
});
