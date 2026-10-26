// Contract tests of the `resources` option of createSession (src/resources.js):
// deny-all by default, the page's allowlist gating every resolution, the
// prefetch of statically known `<data src>`, `<script src>`, and
// `<invoke src>` URLs before the machine runs, the prefetch bounds and the
// Wasmtime host's error mapping, runtime-computed URLs, and the bounds
// pinned against their Rust sources.
//
// Runs in Node on the same ES modules and metered core Wasm a browser loads.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  CapabilityError,
  RESOURCE_DETAILS,
  ScxmlHostError,
  ScxmlSessionError,
  UNSUPPORTED,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';
import {
  MAX_SCRIPT_URI_BYTES,
  ResourceCache,
  ScriptLoads,
  collectUrls,
  prefetchResources,
  resolveReference,
  resolvedDocument,
} from '../src/resources.js';
import { Telemetry } from '../src/telemetry.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const loadCoreModule = cachedCoreModules((component, file) =>
  readFile(join(packageRoot, 'dist', component, file)),
);
const compiler = await createCompiler({ loadCoreModule });

async function compile(source, sourceName = 'resources.scxml') {
  const result = await compiler.compile(source, { sourceName });
  assert.equal(result.status, 'emitted', new TextDecoder().decode(result.report));
  return result.ir;
}

const document = (body, attributes = '') =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"${attributes}>${body}</scxml>`;

/** Opens a manual-timer ECMAScript session over `source`, recording `<log>` lines. */
async function open(source, options = {}) {
  const logs = [];
  const { capabilities, ...rest } = options;
  const session = await createSession({
    ir: await compile(source),
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    capabilities: { log: ({ label, text }) => logs.push(label === undefined ? text : `${label}: ${text}`), ...capabilities },
    timers: 'manual',
    loadCoreModule,
    ...rest,
  });
  return { session, logs };
}

/** A resolver over `documents` that records every `allow` and `resolve` call. */
function recorder(documents, { allow = (url) => Object.hasOwn(documents, url), delay = 1 } = {}) {
  const calls = [];
  return {
    calls,
    resources: {
      allow(url) {
        calls.push(['allow', url]);
        return allow(url);
      },
      async resolve(url) {
        calls.push(['resolve', url]);
        await new Promise((done) => setTimeout(done, delay));
        if (!Object.hasOwn(documents, url)) throw new CapabilityError('unavailable');
        return documents[url];
      },
    },
  };
}

const sessionError = (code, detail) => (error) => {
  assert.ok(error instanceof ScxmlSessionError, `expected ScxmlSessionError(${code}), got ${error}`);
  assert.equal(error.code, code);
  if (detail !== undefined) assert.equal(error.detail, detail);
  return true;
};
const hostError = (code) => (error) => {
  assert.ok(error instanceof ScxmlHostError, `expected ScxmlHostError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};

const DATA_URL = 'https://example.test/data.json';
const OTHER_URL = 'https://example.test/other.json';
/** Two `<data src>` loads: `loaded` when both bind, else the first error. */
const TWO_DATA = document(
  `<datamodel><data id="doc" src="${DATA_URL}"/><data id="other" src="${OTHER_URL}"/></datamodel>
   <state id="a">
     <transition event="error.execution" target="failed"/>
     <transition cond="doc.a === 1 &amp;&amp; other.b === 2" target="loaded"/>
     <transition cond="doc.a === 1" target="partial"/>
   </state>
   <final id="failed"/>
   <final id="partial"/>
   <final id="loaded"/>`,
  ' initial="a"',
);

// --- deny by default -----------------------------------------------------------

test('without the resources option every URL keeps today\'s typed error (deny-all)', async () => {
  const { session } = await open(TWO_DATA);
  assert.deepEqual(session.start().activeConfiguration, ['failed']);
  session.dispose();
  // A `<script src>` keeps rejecting the document at load with `unavailable`.
  await assert.rejects(
    open(document(`<script src="https://example.test/a.js"/><state id="a"/>`, ' initial="a"')),
    sessionError('unavailable', 'capability is not provided by this host'),
  );
});

test('an allow that denies everything never resolves and matches the default', async () => {
  const { calls, resources } = recorder({ [DATA_URL]: '{"a": 1}', [OTHER_URL]: '{"b": 2}' }, { allow: () => false });
  const { session } = await open(TWO_DATA, { resources });
  assert.deepEqual(calls, [
    ['allow', DATA_URL],
    ['allow', OTHER_URL],
  ]);
  assert.deepEqual(session.start().activeConfiguration, ['failed'], 'error.execution, as without resources');
  session.dispose();
});

test('only an allow result of exactly true admits a URL; a throwing allow denies', async () => {
  for (const verdict of [1, 'yes', {}, undefined, null]) {
    const { calls, resources } = recorder({ [DATA_URL]: '{"a": 1}' }, { allow: () => verdict });
    const { session } = await open(TWO_DATA, { resources });
    assert.ok(calls.every(([kind]) => kind === 'allow'), `verdict ${String(verdict)} resolved`);
    assert.deepEqual(session.start().activeConfiguration, ['failed']);
    session.dispose();
  }
  const { calls, resources } = recorder({}, {
    allow: () => {
      throw new Error('policy failure');
    },
  });
  const { session } = await open(TWO_DATA, { resources });
  assert.ok(calls.every(([kind]) => kind === 'allow'));
  assert.deepEqual(session.start().activeConfiguration, ['failed']);
  session.dispose();
});

// --- media types (SCXML B.2.1) ---------------------------------------------------

test('a resolve answer may carry the media type its source indicated', async () => {
  // B.2.1: "if an indication of the type of the content is available (e.g.,
  // via a Content-Type header), then the Processor SHOULD try to interpret
  // the content according to that indication". A JSON label is read as
  // JSON; `text/plain` indicates nothing, so JSON text is still read by
  // content (the official cases ship JSON in `.txt` files).
  const { resources } = recorder({
    [DATA_URL]: { text: '{"a": 1}', mediaType: 'application/json; charset=utf-8' },
    [OTHER_URL]: { text: '{"b": 2}', mediaType: 'text/plain' },
  });
  const { session } = await open(TWO_DATA, { resources });
  assert.deepEqual(session.start().activeConfiguration, ['loaded']);
  session.dispose();
});

test('a document answer is a string or {text, mediaType?, baseUri?, eventSchemas?} with a visible-ASCII media type', () => {
  const plain = { mediaType: undefined, baseUri: undefined, eventSchemas: undefined };
  assert.deepEqual({ ...resolvedDocument('x') }, { ...plain, text: 'x' });
  assert.deepEqual({ ...resolvedDocument({ text: 'x', mediaType: 'application/xml' }) }, { ...plain, text: 'x', mediaType: 'application/xml' });
  assert.deepEqual({ ...resolvedDocument({ text: 'x' }) }, { ...plain, text: 'x' });
  // An `<invoke src>` document's base URI and schema files (the Wasmtime
  // host's EmbeddedDocument::base_uri and event_schemas), copied.
  const bytes = new Uint8Array([123, 125]);
  const answered = resolvedDocument({ text: 'x', baseUri: 'https://example.test/a/child.scxml', eventSchemas: [{ uri: 's.json', bytes }] });
  assert.equal(answered.baseUri, 'https://example.test/a/child.scxml');
  assert.deepEqual(answered.eventSchemas.map(({ uri, bytes: copy }) => [uri, [...copy]]), [['s.json', [123, 125]]]);
  assert.notEqual(answered.eventSchemas[0].bytes, bytes);
  // A media type of any length is kept (257 bytes here). One not visible
  // ASCII is dropped, as the Wasmtime host drops it.
  const long = `text/${'a'.repeat(252)}`;
  assert.equal(long.length, 257);
  assert.equal(resolvedDocument({ text: 'x', mediaType: long }).mediaType, long);
  assert.equal(resolvedDocument({ text: 'x', mediaType: 'text/é' }).mediaType, undefined);
  for (const malformed of [
    1,
    null,
    {},
    { text: 1 },
    { text: 'x', mediaType: 1 },
    { text: 'x', baseUri: 'child.scxml' },
    { text: 'x', baseUri: 1 },
    { text: 'x', eventSchemas: {} },
    { text: 'x', eventSchemas: [{ uri: 's.json', bytes: [1] }] },
  ]) {
    assert.equal(resolvedDocument(malformed), undefined, JSON.stringify(malformed));
  }
});

test('capabilities.loadDocument may answer {text, mediaType}', async () => {
  const source = document(
    `<datamodel><data id="doc" expr="'x'"/></datamodel>
     <state id="a"><onentry><assign location="doc" expr="1"/></onentry>
       <invoke type="scxml" srcexpr="'child.scxml'"/>
       <transition event="error.execution" target="failed"/>
       <transition event="done.invoke.*" target="done"/>
     </state>
     <final id="failed"/><final id="done"/>`,
    ' initial="a"',
  );
  const child = document('<final id="f"/>');
  const loads = [];
  const { session } = await open(source, {
    capabilities: {
      loadDocument(kind, uri) {
        loads.push([kind, uri]);
        return { text: child, mediaType: 'application/scxml+xml' };
      },
    },
  });
  session.start();
  assert.deepEqual(loads, [['invocation', 'child.scxml']]);
  session.dispose();
});

// --- allowlist and prefetch -------------------------------------------------------

test('the allowlist gates every resolve: an allowed URL binds, a denied one is error.execution', async () => {
  const { calls, resources } = recorder(
    { [DATA_URL]: '{"a": 1}', [OTHER_URL]: '{"b": 2}' },
    { allow: (url) => url === DATA_URL },
  );
  const { session } = await open(TWO_DATA, { resources });
  // Every allow verdict comes first, synchronously; only allowed URLs resolve.
  assert.deepEqual(calls, [
    ['allow', DATA_URL],
    ['allow', OTHER_URL],
    ['resolve', DATA_URL],
  ]);
  // `doc` bound; `other` raised error.execution, queued behind the eventless
  // transition that saw `doc`.
  assert.deepEqual(session.start().activeConfiguration, ['partial']);
  session.dispose();
});

/** One `<data src="url">`: `loaded` when it binds, `failed` on error.execution. */
const ONE_DATA = (url) =>
  document(
    `<datamodel><data id="doc" src="${url}"/></datamodel>
     <state id="a">
       <transition event="error.execution" target="failed"/>
       <transition cond="doc.a === 1" target="loaded"/>
     </state>
     <final id="failed"/>
     <final id="loaded"/>`,
    ' initial="a"',
  );

test('allow and resolve get the canonical URL, so dot segments cannot leave an allowed path', async () => {
  for (const [url, canonical] of [
    ['https://example.test/docs/../api/me', 'https://example.test/api/me'],
    ['https://example.test/docs/%2e%2e/api/me', 'https://example.test/api/me'],
    ['https://example.test/docs/%2E%2E/%2e%2e/other', 'https://example.test/other'],
    ['https://example.test/docs/..\\api/me', 'https://example.test/api/me'],
  ]) {
    // A raw prefix policy over documents keyed by the URL as written: it
    // sees the canonical form, which it does not admit.
    const { calls, resources } = recorder(
      { [url]: '{"a": 1}', [canonical]: '{"a": 1}' },
      { allow: (candidate) => candidate.startsWith('https://example.test/docs/') },
    );
    const { session } = await open(ONE_DATA(url), { resources });
    assert.deepEqual(calls, [['allow', canonical]], url);
    assert.deepEqual(session.start().activeConfiguration, ['failed'], url);
    session.dispose();
  }
  // An admitted URL is resolved in the canonical form allow judged, and the
  // machine's load of the URL as written is served from it.
  const allow = (candidate) => {
    const { origin, pathname } = new URL(candidate);
    return origin === 'https://example.test' && pathname.startsWith('/docs/');
  };
  const { calls, resources } = recorder({ 'https://example.test/docs/d.json': '{"a": 1}' }, { allow });
  const { session } = await open(ONE_DATA('https://EXAMPLE.test/docs/sub/../d.json'), { resources });
  assert.deepEqual(calls, [
    ['allow', 'https://example.test/docs/d.json'],
    ['resolve', 'https://example.test/docs/d.json'],
  ]);
  assert.deepEqual(session.start().activeConfiguration, ['loaded']);
  session.dispose();
});

test('an http(s) URL with userinfo never reaches allow; any other URL reaches it as written', async () => {
  for (const url of ['https://example.test@evil.test/docs/d.json', 'https://user:pass@example.test/docs/d.json']) {
    const { calls, resources } = recorder({ [url]: '{"a": 1}' }, { allow: () => true });
    const { session } = await open(ONE_DATA(url), { resources });
    assert.deepEqual(calls, [], url);
    assert.deepEqual(session.start().activeConfiguration, ['failed'], url);
    session.dispose();
  }
  // The host never fetches: a relative name or another scheme is the page's
  // to map, and the policy sees exactly what the document wrote.
  for (const url of ['docs/../d.json', 'file:docs/../d.json']) {
    const { calls, resources } = recorder({ [url]: '{"a": 1}' });
    const { session } = await open(ONE_DATA(url), { resources });
    assert.deepEqual(calls, [['allow', url], ['resolve', url]], url);
    assert.deepEqual(session.start().activeConfiguration, ['loaded'], url);
    session.dispose();
  }
});

test('every static URL is resolved asynchronously before start and served synchronously', async () => {
  const { calls, resources } = recorder({ [DATA_URL]: '{"a": 1}', [OTHER_URL]: '{"b": 2}' }, { delay: 20 });
  const { session } = await open(TWO_DATA, { resources });
  const before = calls.length;
  assert.deepEqual(calls, [
    ['allow', DATA_URL],
    ['allow', OTHER_URL],
    ['resolve', DATA_URL],
    ['resolve', OTHER_URL],
  ]);
  assert.deepEqual(session.start().activeConfiguration, ['loaded']);
  assert.equal(calls.length, before, 'the machine is served from the cache: no call during start');
  session.dispose();
});

test('collectUrls finds data, unpinned script, and invoke src URLs once, deterministically', async () => {
  const ir = await compile(
    document(
      `<datamodel><data id="d" src="https://x.test/d.json"/></datamodel>
       <script src="https://x.test/a.js"/>
       <state id="a">
         <datamodel><data id="e" src="https://x.test/e.json"/><data id="f" src="https://x.test/d.json"/></datamodel>
         <invoke src="https://x.test/child.scxml"/>
         <invoke srcexpr="'https://x.test/dynamic.scxml'"/>
         <onentry><script src="https://x.test/b.js"/></onentry>
       </state>`,
      ' initial="a"',
    ),
  );
  const urls = collectUrls(ir);
  assert.deepEqual([...urls].sort(), [
    'https://x.test/a.js',
    'https://x.test/b.js',
    'https://x.test/child.scxml',
    'https://x.test/d.json',
    'https://x.test/e.json',
  ]);
  assert.deepEqual(collectUrls(ir), urls, 'the same IR yields the same order');
  // Hostile or malformed IR yields no URL (the machine rejects it).
  assert.deepEqual(collectUrls(new Uint8Array([0xff, 0xfe])), []);
  assert.deepEqual(collectUrls(new TextEncoder().encode('{"data": [')), []);
  assert.deepEqual(collectUrls(new TextEncoder().encode(`${'['.repeat(100_000)}${']'.repeat(100_000)}`)), []);
  // A pinned script is a Wasm component: never prefetched.
  const pinned = { kind: 'script', src: { uri: 'https://x.test/p.wasm', pinned: { media_type: 'application/wasm' } } };
  assert.deepEqual(collectUrls(new TextEncoder().encode(JSON.stringify({ actions: [pinned] }))), []);
});

test('every prefetched URL reaches allow and resolves: 65 <data src> URLs, as under Wasmtime', async () => {
  // Neither host bounds the URLs one session loads: all 65 load.
  const urls = Array.from({ length: 65 }, (_, index) => `https://x.test/${index}`);
  const ir = new TextEncoder().encode(JSON.stringify({ data: urls.map((src) => ({ src })) }));
  const allowed = [];
  const cache = await prefetchResources(
    ir,
    { allow: (url) => allowed.push(url) > 0, resolve: async (url) => url },
    new Telemetry(),
  );
  assert.equal(allowed.length, urls.length);
  for (const url of urls) assert.deepEqual(cache.document(url), { text: url, mediaType: undefined, baseUri: undefined, eventSchemas: undefined });
});

// --- size bounds ---------------------------------------------------------------------

/** 4 MiB of UTF-8: the documents built from it exceed 4 MiB, and load on every host. */
const DOCUMENT_BYTES_OVER_4_MIB = 4 * 1024 * 1024;

test('a document of any size loads, from resources or capabilities.loadDocument, as under Wasmtime', async () => {
  // Over 4 MiB; the machine reads it as whitespace-normalized
  // text (SCXML B.2.1), so the data model holds 'x y'.
  const big = `x${' '.repeat(DOCUMENT_BYTES_OVER_4_MIB)}y`;
  const source = document(
    `<datamodel><data id="d" src="${DATA_URL}"/></datamodel>
     <state id="a">
       <transition cond="d === 'x y'" target="loaded"/>
       <transition event="error.execution" target="failed"/>
     </state>
     <final id="loaded"/>
     <final id="failed"/>`,
    ' initial="a"',
  );
  const { resources } = recorder({ [DATA_URL]: big });
  const prefetched = await open(source, { resources });
  assert.deepEqual(prefetched.session.start().activeConfiguration, ['loaded']);
  prefetched.session.dispose();

  const supplied = await open(source, { capabilities: { loadDocument: () => big } });
  assert.deepEqual(supplied.session.start().activeConfiguration, ['loaded']);
  supplied.session.dispose();

  const ir = new TextEncoder().encode(JSON.stringify({ data: [{ src: DATA_URL }] }));
  const cache = await prefetchResources(ir, { allow: () => true, resolve: async () => big }, new Telemetry());
  assert.equal(cache.document(DATA_URL).text, big);
});

test('resolved text of any total is held: a 20 MiB document and 20 MiB over five URLs', async () => {
  const twenty = 'x'.repeat(20 * 1024 * 1024);
  const single = new TextEncoder().encode(JSON.stringify({ data: [{ src: DATA_URL }] }));
  const one = await prefetchResources(single, { allow: () => true, resolve: async () => twenty }, new Telemetry());
  assert.equal(one.document(DATA_URL).text.length, twenty.length);

  const chunk = 'x'.repeat(DOCUMENT_BYTES_OVER_4_MIB);
  const count = 5;
  const urls = Array.from({ length: count }, (_, index) => `https://x.test/${index}`);
  const ir = new TextEncoder().encode(JSON.stringify({ data: urls.map((src) => ({ src })) }));
  const cache = await prefetchResources(
    ir,
    {
      allow: () => true,
      resolve: (url) => new Promise((done) => setTimeout(() => done(chunk), count - urls.indexOf(url))),
    },
    new Telemetry(),
  );
  for (const url of urls) assert.equal(cache.document(url).text.length, chunk.length);
});

// --- error mapping ---------------------------------------------------------------------

test('a failed or malformed resolution is unavailable: error.execution, as a Wasmtime unresolvable document', async () => {
  for (const resolveFn of [
    async () => {
      throw new Error('network down');
    },
    async () => {
      throw new CapabilityError('unavailable');
    },
    async () => 42,
    () => {
      throw new Error('synchronous failure');
    },
  ]) {
    const { session } = await open(TWO_DATA, { resources: { allow: () => true, resolve: resolveFn } });
    assert.deepEqual(session.start().activeConfiguration, ['failed']);
    session.dispose();
  }
});

const SCRIPT_URL = 'https://example.test/lib.js';
const SCRIPTED = document(
  `<datamodel><data id="loaded" expr="0"/></datamodel>
   <script src="${SCRIPT_URL}"/>
   <state id="a">
     <onentry><script src="${SCRIPT_URL}"/></onentry>
     <transition cond="loaded === 2" target="pass"/>
     <transition event="error.execution" target="failed"/>
   </state>
   <final id="pass"/>
   <final id="failed"/>`,
  ' initial="a"',
);

test('<script src> loads once from the cache and runs as ECMAScript text', async () => {
  const { calls, resources } = recorder({ [SCRIPT_URL]: 'loaded = loaded + 1;' });
  const { session } = await open(SCRIPTED, { resources });
  assert.deepEqual(calls, [
    ['allow', SCRIPT_URL],
    ['resolve', SCRIPT_URL],
  ]);
  // Run twice: the document-level script and the `<onentry>` one.
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test('a <script src> the page does not provide rejects the document with the Wasmtime codes', async () => {
  const cases = [
    [{ allow: () => false, resolve: async () => 'loaded = 2;' }, 'unavailable', 'script loader did not provide the script'],
    [{ allow: () => true, resolve: async () => { throw new CapabilityError('unavailable'); } }, 'unavailable', undefined],
    [{ allow: () => true, resolve: async () => { throw new Error('fetch failed'); } }, 'platform', 'script loader failed'],
    [{ allow: () => true, resolve: async () => '' }, 'payload-too-large', undefined],
  ];
  for (const [resources, code, detail] of cases) {
    await assert.rejects(open(SCRIPTED, { resources }), sessionError(code, detail));
  }
  // Script text is the data model's language: a session without the
  // ECMAScript data model rejects it.
  await assert.rejects(
    createSession({
      ir: await compile(SCRIPTED),
      resources: { allow: () => true, resolve: async () => 'loaded = 2;' },
      timers: 'manual',
      loadCoreModule,
    }),
    sessionError('invalid-input'),
  );
  // The host bounds no script text; the session's ECMAScript data model
  // decides, exactly as under the Wasmtime session host
  // (hosts/scxml-host-wasmtime/tests/session_host.rs,
  // script_src_text_is_bounded_only_by_the_data_model): text within its
  // 64 KiB script bound runs, and 65 KiB loads and raises error.execution
  // when it runs.
  const outcome = async (padding) => {
    const text = `loaded = 2;${' '.repeat(padding)}`;
    const { session } = await open(
      document(
        `<datamodel><data id="loaded" expr="0"/></datamodel><script src="${SCRIPT_URL}"/>
         <state id="a"><transition event="error.execution" target="refused"/><transition cond="loaded === 2" target="pass"/></state><final id="pass"/><final id="refused"/>`,
        ' initial="a"',
      ),
      { resources: { allow: () => true, resolve: async () => text } },
    );
    try {
      return session.start().activeConfiguration;
    } finally {
      session.dispose();
    }
  };
  assert.deepEqual(await outcome(64 * 1024 - 11), ['pass']);
  assert.deepEqual(await outcome(65 * 1024), ['refused']);
});

// --- invoke src and dynamic URLs -------------------------------------------------------

const CHILD = document(`<final id="done"/>`, ' initial="done"');

test('<invoke src> hands the resolved document to compileInvocation', async () => {
  const childUrl = 'https://example.test/child.scxml';
  const childIr = await compile(CHILD);
  const { calls, resources } = recorder({ [childUrl]: CHILD });
  const compiled = [];
  const { session } = await open(
    document(
      `<state id="a">
         <invoke src="${childUrl}"/>
         <transition event="done.invoke" target="pass"/>
         <transition event="error.execution" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
    {
      resources,
      capabilities: {
        compileInvocation(sourceName, source) {
          compiled.push([sourceName, source]);
          return childIr;
        },
      },
    },
  );
  assert.deepEqual(calls, [
    ['allow', childUrl],
    ['resolve', childUrl],
  ]);
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.deepEqual(compiled, [[childUrl, CHILD]]);
  session.dispose();
});

// An `<invoke src>` child that runs its own `<script src>`, loads its own
// `<data src>`, is compiled with its own `event:schema` file, and invokes
// a grandchild, every one of them named relative to the child's base URI.
const EVENT_NAMESPACE = 'https://github.com/stateforward/scxml.rs/namespaces/event';
const SCHEMA = new TextEncoder().encode(
  JSON.stringify({ $schema: 'https://json-schema.org/draft/2020-12/schema', type: 'object', required: ['n'] }),
);
const GRANDCHILD = document(`<final id="g"/>`, ' initial="g"');
const RELATIVE_CHILD = `<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="${EVENT_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="c">
  <datamodel><data id="loaded" expr="0"/><data id="doc" src="data.json"/></datamodel>
  <script src="lib/child.js"/>
  <state id="c">
    <invoke src="../grand/grandchild.scxml"/>
    <transition event="done.invoke" cond="loaded === 1 &amp;&amp; doc.a === 1" target="w"/>
    <transition event="error.*" target="bad"/>
  </state>
  <state id="w">
    <onentry><send target="#_parent" event="ready"/></onentry>
    <transition event="go" event:schema="schemas/go.json" cond="_event.data.n === 2" target="done"/>
  </state>
  <final id="done"/>
  <final id="bad"><onentry><send target="#_parent" event="childfailed"/></onentry></final>
</scxml>`;
const CHILD_BASE = 'https://example.test/bots/child/child.scxml';
const RELATIVE_PARENT = document(
  `<state id="p" initial="a">
     <invoke id="kid" src="child/child.scxml"/>
     <transition event="done.invoke.kid" target="pass"/>
     <transition event="childfailed" target="failed"/>
     <transition event="error.*" target="failed"/>
     <state id="a"><transition event="ready" target="b"/></state>
     <state id="b"><onentry><send target="#_kid" event="go"><param name="n" expr="2"/></send></onentry></state>
   </state>
   <final id="pass"/>
   <final id="failed"/>`,
  ' initial="p"',
);
/** The site the relative documents live on, answering as a page would. */
const RELATIVE_SITE = {
  'child/child.scxml': { text: RELATIVE_CHILD, baseUri: CHILD_BASE, eventSchemas: [{ uri: 'schemas/go.json', bytes: SCHEMA }] },
  'https://example.test/bots/child/lib/child.js': 'loaded = loaded + 1;',
  'https://example.test/bots/child/data.json': '{"a": 1}',
  'https://example.test/bots/grand/grandchild.scxml': { text: GRANDCHILD, baseUri: 'https://example.test/bots/grand/grandchild.scxml' },
};

test('resolveReference resolves against a base URI, and leaves a URI as written without one', () => {
  assert.equal(resolveReference(CHILD_BASE, 'lib/child.js'), 'https://example.test/bots/child/lib/child.js');
  assert.equal(resolveReference(CHILD_BASE, '../grand/g.scxml'), 'https://example.test/bots/grand/g.scxml');
  assert.equal(resolveReference(CHILD_BASE, 'https://other.test/x'), 'https://other.test/x');
  assert.equal(resolveReference(undefined, '../x.scxml'), '../x.scxml');
});

test("an <invoke src> child resolves its own scripts, data, schemas, and children against its base URI", async () => {
  const { calls, resources } = recorder(RELATIVE_SITE);
  const { session } = await open(RELATIVE_PARENT, { resources });
  // The root's URL as written; the child's and grandchild's resolved
  // against the child's base URI, each once, all before start.
  assert.deepEqual(
    calls.filter(([call]) => call === 'resolve').map(([, url]) => url).sort(),
    Object.keys(RELATIVE_SITE).sort(),
  );
  // The child loads and runs its script and data, completes its
  // grandchild, and takes the parent's `go` under its own event schema.
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test("an <invoke src> child without its schema files is rejected at compile, as under Wasmtime", async () => {
  const site = { ...RELATIVE_SITE, 'child/child.scxml': { text: RELATIVE_CHILD, baseUri: CHILD_BASE } };
  const { session } = await open(RELATIVE_PARENT, { resources: recorder(site).resources });
  assert.deepEqual(session.start().activeConfiguration, ['failed']);
  session.dispose();
});

test('without a base URI a child names its documents as written', async () => {
  const site = {
    'child/child.scxml': { text: RELATIVE_CHILD, eventSchemas: [{ uri: 'schemas/go.json', bytes: SCHEMA }] },
    'lib/child.js': 'loaded = loaded + 1;',
    'data.json': '{"a": 1}',
    '../grand/grandchild.scxml': GRANDCHILD,
  };
  const { calls, resources } = recorder(site);
  const { session } = await open(RELATIVE_PARENT, { resources });
  assert.deepEqual(
    calls.filter(([call]) => call === 'resolve').map(([, url]) => url).sort(),
    Object.keys(site).sort(),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

const DYNAMIC = document(
  `<datamodel><data id="where" expr="'https://example.test/child.scxml'"/></datamodel>
   <state id="a">
     <invoke srcexpr="where"/>
     <transition event="done.invoke" target="pass"/>
     <transition event="error.execution" target="failed"/>
   </state>
   <final id="pass"/>
   <final id="failed"/>`,
  ' initial="a"',
);

test('a runtime-computed URL is typed dynamic-url-unavailable (error.execution), never resolved', async () => {
  assert.equal(UNSUPPORTED.dynamicUrl, 'dynamic-url-unavailable');
  const childIr = await compile(CHILD);
  const { calls, resources } = recorder({ 'https://example.test/child.scxml': CHILD });
  const { session } = await open(DYNAMIC, { resources, capabilities: { compileInvocation: () => childIr } });
  assert.deepEqual(session.start().activeConfiguration, ['failed']);
  assert.deepEqual(calls, [], 'the dynamic URL never reached allow or resolve');
  session.dispose();

  // The page's synchronous loadDocument still answers what the cache cannot.
  const loads = [];
  const supplied = await open(DYNAMIC, {
    resources,
    capabilities: {
      loadDocument(kind, uri) {
        loads.push([kind, uri]);
        return CHILD;
      },
      compileInvocation: () => childIr,
    },
  });
  assert.deepEqual(supplied.session.start().activeConfiguration, ['pass']);
  assert.deepEqual(loads, [['invocation', 'https://example.test/child.scxml']]);
  supplied.session.dispose();
});

// --- options and bounds --------------------------------------------------------------------

test('a malformed resources option is invalid-argument', async () => {
  const ir = await compile(document(`<state id="a"/>`, ' initial="a"'));
  const resolveFn = async () => '';
  for (const resources of [
    null,
    42,
    {},
    { allow: () => true },
    { resolve: resolveFn },
    { allow: true, resolve: resolveFn },
    { allow: () => true, resolve: resolveFn, fetch: () => {} },
  ]) {
    await assert.rejects(createSession({ ir, resources, loadCoreModule }), hostError('invalid-argument'));
  }
});

test('resource bounds match the scxml-script sources', async () => {
  const read = (path) => readFile(join(workspace, path), 'utf8');
  const constant = (source, name) => {
    const match = source.match(new RegExp(`pub const ${name}: usize = ([^;]+);`));
    assert.ok(match, `${name} exists`);
    return match[1].trim();
  };
  // The browser runs the wasm components, so a bound that applies to wasm
  // builds only (`scxml_types::wasm_bound(n)`) is `n` here.
  const evaluate = (expression) =>
    expression
      .replace(/^scxml_types::wasm_bound\((.*)\)$/, '$1')
      .replaceAll('_', '')
      .split('*')
      .reduce((product, factor) => product * Number(factor.trim()), 1);
  const script = await read('crates/scxml-script/src/lib.rs');
  assert.equal(MAX_SCRIPT_URI_BYTES, evaluate(constant(script, 'MAX_SCRIPT_URI_BYTES')));
});

test('a session loads any number of distinct scripts, as under Wasmtime', () => {
  // Neither host bounds the distinct URIs one session loads: all 65 load.
  const urls = Array.from({ length: 65 }, (_, index) => `https://example.test/s${index}.js`);
  const loads = new ScriptLoads(new ResourceCache(new Map(urls.map((url) => [url, { text: 'var x = 1;' }]))));
  for (const uri of urls) loads.load({ uri }, 'ecmascript');
  assert.equal(loads.text({ uri: urls[64] }), 'var x = 1;');
});

test('a script src is blank only in XML white space, as under Wasmtime', () => {
  // scxml_script::validate_uri: U+1680, U+00A0, and U+FEFF are URI text.
  const uris = ['\u1680', '\u00a0', '\ufeff'];
  const loads = new ScriptLoads(new ResourceCache(new Map(uris.map((uri) => [uri, { text: 'var x = 1;' }]))));
  for (const uri of uris) {
    loads.load({ uri }, 'ecmascript');
    assert.equal(loads.text({ uri }), 'var x = 1;', JSON.stringify(uri));
  }
  for (const uri of ['', ' \t\r\n']) {
    assert.throws(() => loads.load({ uri }, 'ecmascript'), (error) => error.payload?.code === 'invalid-input', JSON.stringify(uri));
  }
});

