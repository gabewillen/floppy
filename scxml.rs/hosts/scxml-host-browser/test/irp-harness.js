// Runs one prepared official W3C IRP case through the browser host, the way
// the Wasmtime session host runs it for `scx conformance run`
// (crates/scxml-conformance/src/wasm.rs): compile the prepared document,
// link the data model the document names, start, deliver the case's
// scripted events, then deliver every delayed send and cross-session
// message in clock order until the session finishes or nothing is pending.
// Documents resolve from the case's prepared dependencies exactly as the
// conformance embedder resolves them, through the host's `resources`
// option: every static `src` URL is allowlisted only when it names a
// prepared dependency and is prefetched before the run. The synchronous
// `loadDocument` provider answers only what the prefetch cannot know (a
// runtime `srcexpr`); the result counts those calls as `dynamicLoads`. The pass oracle is the official one:
// exactly one root `<log>` labeled `Outcome` whose text is `pass`, in a
// session finished in exactly the `pass` final state the W3C templates
// generate for `conf:pass` (templates/ecmascript.xsl:18-30).
//
// `<invoke>` children compile inside the host, exactly as a page that does
// nothing gets them: the session's default automatic compiler
// (`invoke.compile: 'auto'`, src/invoke.js) answers `compile-invocation`
// with the Wasmtime host's request (lowered IR, schema policy off, runtime
// script resolution). The harness supplies no `compileInvocation`.
//
// With `basichttp: true` the harness is also the BasicHTTP network: the
// Wasmtime session host's in-process loopback
// (hosts/scxml-host-wasmtime/src/session_host/loopback.rs). Every session's
// location is `http://basichttp.loopback.invalid/sessions/<id>/basichttp`;
// `http.allow` accepts only that address space (anything else is no route:
// error.communication), and `http.send` hands the request synchronously to
// `session.receiveHttp`, which delivers it as the loopback does: to the
// sending session, its parent, or a live child, by location in its
// canonical form, and
// answers 404 for a location no live session answers.
//
// With `checkpoint`, the harness awaits `checkpoint(session)` at every
// checkpoint of the run (after start, after each scripted event,
// and after each delivery pass): the session snapshot non-perturbation
// proof takes a snapshot there. With `trace`, the result carries the
// observation trace, the command count, and the final clock, so two runs
// can be compared exactly.
//
// Browser-compatible ES module: the caller supplies file reading.

import {
  CapabilityError,
  DEFAULT_SESSION_ID,
  ScxmlSessionError,
  createEcmaScriptDataModel,
  createNullDataModel,
  createSession,
  createXPathDataModel,
} from '../src/index.js';

const FACTORIES = {
  ecmascript: createEcmaScriptDataModel,
  xpath: createXPathDataModel,
  null: createNullDataModel,
};
/** Scheme and authority of every loopback BasicHTTP location (the Wasmtime host's LOOPBACK_ORIGIN). */
export const LOOPBACK_ORIGIN = 'http://basichttp.loopback.invalid';
/** The loopback BasicHTTP location of session `sessionId` (the Wasmtime host's `loopback_location`). */
export const loopbackLocation = (sessionId) => `${LOOPBACK_ORIGIN}/sessions/${sessionId}/basichttp`;

/**
 * The loopback `http` capability of one run: `session()` returns the root
 * session once created.
 */
export function loopbackHttp(session) {
  return {
    location: loopbackLocation,
    allow: (url) => url.startsWith(`${LOOPBACK_ORIGIN}/`),
    send: (request) => session().receiveHttp(request),
  };
}

const fileName = (key) => key.slice(key.lastIndexOf('/') + 1);
const render = ({ label, text }) => (label === undefined ? text : `${label}: ${text}`);

/**
 * The conformance embedder's document lookup over prepared sources: the
 * exact key, else the file a `file:` URI (any kind) or a bare name (an
 * invocation) names; `undefined` when none.
 */
function preparedDocument(sources, kind, uri) {
  if (sources.has(uri)) return sources.get(uri);
  const name = uri.startsWith('file:') ? uri.slice(5) : kind === 'invocation' ? uri : undefined;
  if (name !== undefined) {
    for (const [key, text] of sources) if (fileName(key) === name) return text;
  }
  return undefined;
}

/**
 * The `resources` option over prepared sources: a URL is allowed only when
 * it names a prepared dependency (for any kind: `resources` has none).
 */
function preparedResources(sources) {
  const lookup = (url) => preparedDocument(sources, 'invocation', url);
  return {
    allow: (url) => lookup(url) !== undefined,
    resolve: async (url) => lookup(url),
  };
}

/**
 * @param {object} options
 * @param {object} options.testCase one `corpus.json` case
 * @param {number} options.commandBudget the corpus `default_command_budget`
 * @param {(path: string) => Promise<string>} options.readText reads a
 *   corpus-relative path
 * @param {object} options.compiler a createCompiler() instance
 * @param {boolean} [options.basichttp] link the in-process BasicHTTP loopback
 * @param {Function} [options.loadCoreModule]
 * @param {Function} [options.checkpoint] awaited as `checkpoint(session)`
 *   at every checkpoint
 * @param {boolean} [options.trace] record the trace, command count, and clock
 * @returns {Promise<{disposition: string, logs: string[], configuration?: string[], status?: string,
 *   error?: string, commands?: number, now?: number, trace?: object, dynamicLoads: number}>}
 *   (`dynamicLoads`: calls of the synchronous `loadDocument` fallback in the
 *   final run)
 */
export async function runIrpCase({
  testCase,
  commandBudget,
  readText,
  compiler,
  basichttp = false,
  loadCoreModule,
  checkpoint,
  trace = false,
}) {
  const source = await readText(testCase.source);
  const sources = new Map();
  for (const dependency of testCase.dependencies) sources.set(dependency, await readText(dependency));
  const compiled = await compiler.compile(source, { sourceName: fileName(testCase.source) });
  if (compiled.status !== 'emitted') return { disposition: 'invalid', logs: [] };
  // Like the conformance embedder, link the data model the document itself
  // names (the IR's `data_model`), not the corpus variant: some XPath-corpus
  // cases are ECMAScript documents. The embedder may read its own IR; the
  // host never does.
  const { data_model: dataModelName } = JSON.parse(new TextDecoder().decode(compiled.ir));
  const factory = Object.hasOwn(FACTORIES, dataModelName ?? '') ? FACTORIES[dataModelName] : undefined;
  let dynamicLoads = 0;
  const resources = preparedResources(sources);
  const loadDocument = (kind, uri) => {
    dynamicLoads += 1;
    const text = preparedDocument(sources, kind, uri);
    if (text === undefined) throw new CapabilityError('unavailable');
    return text;
  };
  const logs = [];
  let session;
  void DEFAULT_SESSION_ID;
  try {
    session = await createSession({
      ir: compiled.ir,
      dataModel: factory === undefined ? undefined : await factory({ loadCoreModule }),
      capabilities: {
        log: ({ label, text }) => logs.push({ label, text }),
        loadDocument,
      },
      resources,
      ...(basichttp ? { http: loopbackHttp(() => session) } : {}),
      timers: 'manual',
      maxCommands: commandBudget,
      trace,
      loadCoreModule,
    });
    session.start();
    await checkpoint?.(session);
    for (const event of testCase.events) {
      session.send(event);
      await checkpoint?.(session);
    }
    for (let turn = 0; turn < 10_000; turn += 1) {
      if (session.status() !== 'running') break;
      const deadline = session.nextDeadline();
      if (deadline === undefined) break;
      const { complete } = session.advanceTime(deadline - session.now());
      if (!complete) return { disposition: 'harness-error', logs: logs.map(render), error: 'incomplete', dynamicLoads };
      await checkpoint?.(session);
    }
    const outcomes = logs.filter((log) => log.label === 'Outcome');
    return {
      disposition:
        outcomes.length === 1 &&
        outcomes[0].text === 'pass' &&
        session.status() === 'finished' &&
        JSON.stringify(session.configuration()) === JSON.stringify(['pass'])
          ? 'passed'
          : 'failed',
      logs: logs.map(render),
      configuration: session.configuration(),
      status: session.status(),
      ...(trace ? { commands: session.commands(), now: session.now(), trace: session.trace() } : {}),
      dynamicLoads,
    };
  } catch (error) {
    return {
      disposition: error instanceof ScxmlSessionError && error.code === 'unavailable' ? 'unsupported' : 'failed',
      logs: logs.map(render),
      error: `${error.name}: ${error.code} ${error.detail ?? ''}`.trim(),
      ...(trace && session !== undefined ? { commands: session.commands(), trace: session.trace() } : {}),
      dynamicLoads,
    };
  } finally {
    session?.dispose();
  }
}
