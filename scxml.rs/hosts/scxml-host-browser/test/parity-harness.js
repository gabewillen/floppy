// Runs one case of the cross-host parity corpus
// (tests/fixtures/host-parity/cases.json) through the browser host, the way
// `scx run` runs it (crates/scxml-cli/tests/host_parity.rs), and returns the
// outcome the corpus golden records: the root session's `<log>` records in
// order, its final configuration and status, and its `<donedata>` value.
//
// The page does what `scx run` does: it validates the document with the
// bundled validator (schema policy `strict`, the bundled W3C SCXML schema,
// the packages of the namespaces it binds, and the event schema files it
// references, read relative to the document), compiles it with the bundled
// compiler (the same packages and files; the compiler component bundles no
// W3C schema, so it compiles with schema policy `off` after validation),
// binds each extension package (manifest fields, schema bytes, and the
// component bytes authenticated by their release SHA-256), and runs the
// session on the logical clock until nothing is pending.
//
// Browser-compatible ES module: the caller supplies `read(path)` (bytes of a
// workspace-relative path), `component(file)` (bytes of a release artifact),
// and `sha256(bytes)` (lowercase hex).

import {
  ExtensionComponents,
  authenticateExtensionComponent,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';

/** Each extension namespace the corpus binds: its manifest and component. */
export const PACKAGES = {
  'https://stateforward.org/scxml/example': {
    manifest: 'components/scxml-extension-example/package/example.extension.json',
    component: 'scxml_extension_example.virtualized.wasm',
  },
  'https://github.com/stateforward/scxml.rs/namespaces/scx': {
    manifest: 'namespaces/scx/package/scx.extension.json',
    component: 'scx.virtualized.wasm',
  },
};

const decode = (bytes) => new TextDecoder().decode(bytes);
const directoryOf = (path) => path.slice(0, path.lastIndexOf('/') + 1);

/** The ExtensionPackage of one workspace-relative manifest, as `scx` reads it. */
export async function packageOf(read, manifestPath) {
  const manifest = JSON.parse(decode(await read(manifestPath)));
  return {
    namespaceUri: manifest['namespace-uri'],
    packageVersion: manifest['package-version'],
    extensionArtifact: manifest['extension-artifact'],
    schemaArtifact: manifest['schema-artifact'],
    schemaSha256: manifest['schema-sha256'],
    witWorld: manifest['wit-world'],
    witVersion: manifest['wit-version'],
    schema: await read(directoryOf(manifestPath) + manifest['schema-artifact']),
  };
}

/** The JSON value of a WIT `data-value`, as `scx run` writes `done.data`. */
function jsonValue(value) {
  switch (value?.tag) {
    case 'json-object':
    case 'json-array':
      return JSON.parse(value.val);
    case 'text':
    case 'number':
    case 'boolean':
      return value.val;
    default:
      return null;
  }
}

/** An `{name, data}` corpus event as the session's external event (data as JSON). */
function sessionEvent(event) {
  if (typeof event === 'string') return { name: event };
  const { name, data } = event;
  if (data === undefined) return { name };
  const val = JSON.stringify(data);
  if (Array.isArray(data)) return { name, data: { tag: 'json-array', val } };
  if (data !== null && typeof data === 'object') return { name, data: { tag: 'json-object', val } };
  return { name, data: { tag: 'text', val: String(data) } };
}

/**
 * @param {object} options
 * @param {object} options.testCase one corpus case
 * @param {(path: string) => Promise<Uint8Array>} options.read
 * @param {(file: string) => Promise<Uint8Array>} options.component
 * @param {(bytes: Uint8Array) => Promise<string>} options.sha256
 * @param {object} options.compiler a createCompiler() instance
 * @param {object} options.validator a createValidator() instance (ECMAScript linked)
 * @param {Function} [options.loadCoreModule]
 * @returns {Promise<{validation?: object[], outcome?: object}>} the
 *   validator's error diagnostics when the document is invalid, else the
 *   run's outcome
 */
export async function runParityCase({ testCase, read, component, sha256, compiler, validator, loadCoreModule }) {
  const source = decode(await read(testCase.document));
  const packages = [];
  const extensions = new ExtensionComponents();
  for (const uri of testCase.extensions) {
    const spec = PACKAGES[uri];
    const pkg = await packageOf(read, spec.manifest);
    packages.push(pkg);
    const bytes = await component(spec.component);
    extensions.bind(pkg, await authenticateExtensionComponent(bytes, await sha256(bytes)));
  }
  const eventSchemas = await Promise.all(
    (testCase.eventSchemas ?? []).map(async (uri) => ({ uri, bytes: await read(directoryOf(testCase.document) + uri) })),
  );
  const { report } = await validator.validate(source, { sourceName: testCase.document, packages, eventSchemas });
  const errors = JSON.parse(decode(report)).validator.diagnostics.filter((diagnostic) => diagnostic.severity === 'error');
  if (errors.length > 0) return { validation: errors };
  const compiled = await compiler.compile(source, { sourceName: testCase.document, schemaPolicy: 'off', packages, eventSchemas });
  if (compiled.status !== 'emitted') return { validation: JSON.parse(decode(compiled.report)).compiler.diagnostics };
  const logs = [];
  const session = await createSession({
    ir: compiled.ir,
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    capabilities: { log: ({ label, text }) => logs.push([label ?? null, text]) },
    extensionComponents: extensions,
    // `scx run`'s default `--session-id`.
    sessionId: 'session',
    timers: 'manual',
    loadCoreModule,
  });
  try {
    const results = [session.start()];
    for (const event of testCase.events) {
      if (session.status() !== 'running') break;
      results.push(session.send(sessionEvent(event)));
    }
    while (session.status() === 'running' && session.nextDeadline() !== undefined) {
      results.push(...session.advanceTime(session.nextDeadline() - session.now()).results);
    }
    let done = null;
    for (const result of results) if (result.returnData !== undefined) done = jsonValue(result.returnData);
    return { outcome: { logs, configuration: session.configuration(), status: session.status(), done } };
  } finally {
    session.dispose();
  }
}
