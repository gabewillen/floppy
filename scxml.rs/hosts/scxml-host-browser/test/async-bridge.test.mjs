// Contract tests of the async bridge (src/async-bridge.js): `createSession`
// with `async: 'auto'`.
// * Mechanism detection, and the option contract that is the same for
//   every mechanism.
// * The blocking channel between real threads (Atomics.wait in a
//   worker_threads thread), including a reply that grows the buffer.
// * The `worker` mechanism end to end in Node. test/node-worker.js stands in
//   for a dedicated Worker: the unchanged synchronous session runs in a
//   thread, a static `<data src>` URL is prefetched in the page (as for a
//   synchronous session) and its answers are handed to the worker, and a
//   runtime-computed `<invoke srcexpr>` URL loads through the page's
//   asynchronous `resources.resolve` while the worker blocks. Prefetched
//   answers, including `platform` and `payload-too-large`, are the
//   synchronous session's.
// * The `none` fallback: without an async mechanism, static URLs are still
//   prefetched, and a runtime URL the page does not supply is the typed
//   `unavailable` error with detail `dynamic-url-unavailable`, and the
//   machine turns it into error.execution.
// * `async: 'off'` (the default) is the synchronous MachineSession.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { Worker as ThreadWorker } from 'node:worker_threads';

import {
  ASYNC_MECHANISMS,
  ASYNC_MODES,
  AsyncMachineSession,
  CapabilityError,
  MachineSession,
  ScxmlHostError,
  UNSUPPORTED,
  asyncMechanism,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';
import { MAX_RESOURCE_URL_BYTES } from '../src/resources.js';
import {
  MAX_REPLY_BYTES,
  UNAVAILABLE_BRIDGE,
  createChannel,
  decodeReply,
  encodeReply,
  replyOnChannel,
} from '../src/async-bridge.js';
import { NodeWorker } from './node-worker.js';
import { recordingTelemetry } from './recording-telemetry.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const compiler = await createCompiler({ loadCoreModule });

async function compile(source) {
  const result = await compiler.compile(source, { sourceName: 'async.scxml' });
  assert.equal(result.status, 'emitted', new TextDecoder().decode(result.report));
  return result.ir;
}

const scxml = (body, attributes = '') =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"${attributes}>${body}</scxml>`;

// `<data src>` whose document the page resolves asynchronously (prefetched).
const DATA_DOCUMENT = scxml(
  `<datamodel><data id="doc" src="https://example.test/data.json"/></datamodel>
   <state id="a">
     <transition event="error.execution" target="denied"/>
     <transition cond="doc.a === 1" target="loaded"/>
   </state>
   <final id="denied"/>
   <final id="loaded"/>`,
  ' initial="a"',
);
// `<invoke srcexpr>`: the child URL exists only at runtime.
const CHILD = scxml(
  `<datamodel><data id="answer" expr="42"/></datamodel>
   <final id="done"><donedata><param name="answer" expr="answer"/></donedata></final>`,
);
const PARENT = scxml(
  `<datamodel><data id="base" expr="'https://example.test/'"/></datamodel>
   <state id="outer">
     <invoke id="kid" srcexpr="base + 'child-' + (20 + 22) + '.scxml'"/>
     <transition event="done.invoke.kid" cond="_event.data.answer === 42" target="pass"/>
     <transition event="error.execution" target="failed"/>
   </state>
   <final id="pass"/>
   <final id="failed"/>`,
  ' initial="outer"',
);

const hostError = (code) => (error) => {
  assert.ok(error instanceof ScxmlHostError, `expected ScxmlHostError(${code}), got ${error}`);
  assert.equal(error.code, code);
  return true;
};

/** Installs the Node Worker stand-in as a cross-origin-isolated page for `body`. */
async function withWorkerScope(body) {
  const saved = { Worker: globalThis.Worker, crossOriginIsolated: globalThis.crossOriginIsolated };
  globalThis.Worker = NodeWorker;
  globalThis.crossOriginIsolated = true;
  try {
    return await body();
  } finally {
    globalThis.Worker = saved.Worker;
    globalThis.crossOriginIsolated = saved.crossOriginIsolated;
  }
}

// --- detection and options ------------------------------------------------------

test('asyncMechanism is `worker` only with module workers, growable shared memory, and isolation', () => {
  assert.deepEqual(ASYNC_MODES, ['off', 'auto']);
  assert.deepEqual(ASYNC_MECHANISMS, ['worker', 'none']);
  const isolated = { Worker: class {}, SharedArrayBuffer, Atomics, crossOriginIsolated: true };
  assert.equal(asyncMechanism(isolated), 'worker');
  assert.equal(asyncMechanism({ ...isolated, crossOriginIsolated: false }), 'none');
  assert.equal(asyncMechanism({ ...isolated, Worker: undefined }), 'none');
  assert.equal(asyncMechanism({ ...isolated, SharedArrayBuffer: undefined }), 'none');
  assert.equal(asyncMechanism({ ...isolated, Atomics: undefined }), 'none');
  // Node's main thread has no Web Worker and is not cross-origin isolated.
  assert.equal(asyncMechanism(), 'none');
});

test('async options are validated the same way for every mechanism', async () => {
  const ir = await compile(DATA_DOCUMENT);
  for (const options of [
    { ir, async: 'on' },
    { ir, async: true },
    { ir: 'not bytes', async: 'auto' },
    { ir, async: 'auto', dataModel: { name: 'ecmascript' } },
    { ir, async: 'auto', dataModel: 'python' },
    { ir, async: 'auto', now: () => 0 },
    { ir, async: 'auto', resources: { allow: () => true } },
    { ir, async: 'auto', resources: { allow: true, resolve: async () => '' } },
    { ir, async: 'auto', resources: { allow: () => true, resolve: async () => '', cache: true } },
    // `http` providers cannot cross into a worker: refused for every mechanism.
    { ir, async: 'auto', http: { allow: () => false } },
    { ir, async: 'auto', capabilities: null },
    { ir, async: 'auto', onTimer: 'no' },
    { ir, async: 'auto', workerUrl: 42 },
  ]) {
    await assert.rejects(createSession({ loadCoreModule, ...options }), hostError('invalid-argument'));
  }
});

test("`async: 'off'` (and no `async`) is the synchronous MachineSession", async () => {
  const ir = await compile(DATA_DOCUMENT);
  for (const options of [{}, { async: 'off' }]) {
    const session = await createSession({ ir, loadCoreModule, timers: 'manual', ...options });
    assert.ok(session instanceof MachineSession);
    session.dispose();
  }
});

// --- blocking channel -----------------------------------------------------------

test('replies round-trip through the channel encoding', () => {
  for (const value of [undefined, '', 'text ✓', new Uint8Array([0, 1, 255]), { tag: 'handled' }, true]) {
    assert.deepEqual(decodeReply(encodeReply({ value })), { value });
  }
  assert.deepEqual(decodeReply(encodeReply({ error: 'unavailable' })), { error: 'unavailable' });
  // An unknown code never crosses: it is `platform`.
  assert.deepEqual(decodeReply(encodeReply({ error: 'not-a-code' })), { error: 'platform' });
});

test('a worker thread blocks in Atomics.wait until the page answers asynchronously', async () => {
  const channel = createChannel();
  // A 33 MiB reply: the host sets no reply bound.
  const large = 'x'.repeat(33 * 1024 * 1024);
  const thread = new ThreadWorker(
    `import { parentPort, workerData } from 'node:worker_threads';
     const { waitOnChannel } = await import(workerData.bridge);
     const post = (message) => parentPort.postMessage(message);
     const results = [];
     for (const operation of ['small', 'large', 'fails']) {
       try {
         const value = waitOnChannel(workerData.channel, post, operation, [operation]);
         results.push(typeof value === 'string' ? value.length : value);
       } catch (error) {
         results.push(error.name + ':' + error.code);
       }
     }
     post({ type: 'done', results });`,
    { eval: true, workerData: { channel, bridge: new URL('../src/async-bridge.js', import.meta.url).href } },
  );
  const done = new Promise((resolveDone, reject) => {
    thread.on('error', reject);
    thread.on('message', async (message) => {
      if (message.type === 'done') {
        resolveDone(message.results);
        return;
      }
      // Answer after a real asynchronous delay: the thread stays blocked.
      await new Promise((later) => setTimeout(later, 20));
      if (message.operation === 'small') replyOnChannel(channel, { value: 'ok' });
      if (message.operation === 'large') replyOnChannel(channel, { value: large });
      if (message.operation === 'fails') replyOnChannel(channel, { error: 'unavailable' });
    });
  });
  assert.deepEqual(await done, [2, large.length, 'CapabilityError:unavailable']);
  // The large reply grew the buffer.
  assert.ok(channel.byteLength > large.length);
  await thread.terminate();
});

test('a reply the channel cannot carry is payload-too-large and one that cannot be encoded platform; the worker never stays blocked', async () => {
  // A channel that can never grow past 128 KiB.
  const channel = new SharedArrayBuffer(64 * 1024, { maxByteLength: 128 * 1024 });
  const thread = new ThreadWorker(
    `import { parentPort, workerData } from 'node:worker_threads';
     const { waitOnChannel } = await import(workerData.bridge);
     const post = (message) => parentPort.postMessage(message);
     const results = [];
     for (const operation of ['oversized', 'unencodable', 'small']) {
       try {
         results.push(waitOnChannel(workerData.channel, post, operation, []));
       } catch (error) {
         results.push(error.name + ':' + error.code);
       }
     }
     post({ type: 'done', results });`,
    { eval: true, workerData: { channel, bridge: new URL('../src/async-bridge.js', import.meta.url).href } },
  );
  const done = new Promise((resolveDone, reject) => {
    thread.on('error', reject);
    thread.on('message', (message) => {
      if (message.type === 'done') {
        resolveDone(message.results);
        return;
      }
      if (message.operation === 'oversized') replyOnChannel(channel, { value: 'x'.repeat(200 * 1024) });
      if (message.operation === 'unencodable') replyOnChannel(channel, { value: { big: 1n } });
      if (message.operation === 'small') replyOnChannel(channel, { value: 'ok' });
    });
  });
  assert.deepEqual(await done, ['CapabilityError:payload-too-large', 'CapabilityError:platform', 'ok']);
  await thread.terminate();
  // The reply length travels in one Int32 word: no reply may exceed it.
  assert.ok(MAX_REPLY_BYTES + 8 <= 2 ** 31 - 1);
  assert.ok(createChannel().maxByteLength <= 2 ** 31);
});

// --- worker mechanism -------------------------------------------------------------

test('worker: invokeLimits are checked in the page, and a valid limit crosses intact', async () => {
  const ir = await compile(scxml('<final id="f"/>'));
  class Limits {
    constructor() {
      this.maxInvokeDepth = 1;
    }
  }
  const hiddenInvalid = Object.defineProperty({}, 'maxInvokeDepth', { value: -1, enumerable: false });
  await withWorkerScope(async () => {
    // Each would reach the worker as a plain object it accepts: a symbol
    // key, a non-enumerable key, and a class are all lost in the clone.
    for (const invokeLimits of [
      { [Symbol('maxInvokeDepth')]: 1 },
      hiddenInvalid,
      new Limits(),
      new Map([['maxInvokeDepth', 1]]),
      [],
      { maxInvokeDeph: 1 },
      Object.defineProperty({}, 'maxInvokeDepth', { get: () => 1, enumerable: true }),
    ]) {
      await assert.rejects(
        createSession({ ir, async: 'auto', timers: 'manual', loadCoreModule, invokeLimits }),
        hostError('invalid-argument'),
      );
    }
    // A valid limit set as a non-enumerable property still applies in the
    // worker: no session may invoke, so the `<invoke>` fails.
    const invoking = await compile(
      scxml(
        `<state id="a">
           <invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml></content></invoke>
           <transition event="error.execution" target="refused"/>
         </state>
         <final id="refused"/>`,
        ' initial="a"',
      ),
    );
    const childIr = await compile('<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml>');
    const invokeLimits = Object.defineProperty({}, 'maxInvokeDepth', { value: 0, enumerable: false });
    const session = await createSession({
      ir: invoking,
      async: 'auto',
      dataModel: 'ecmascript',
      timers: 'manual',
      loadCoreModule,
      invokeLimits,
      capabilities: { compileInvocation: () => childIr },
    });
    try {
      assert.equal(session.mechanism, 'worker');
      assert.deepEqual((await session.start()).activeConfiguration, ['refused']);
    } finally {
      await session.dispose();
    }
  });
});

test('worker: a prefetched <data src> and a runtime-computed <invoke srcexpr> load through resources.resolve', async () => {
  const [dataIr, parentIr, childIr] = await Promise.all([compile(DATA_DOCUMENT), compile(PARENT), compile(CHILD)]);
  await withWorkerScope(async () => {
    const { telemetry, spans } = recordingTelemetry();
    const resolved = [];
    const resources = {
      allow: (url) => url.startsWith('https://example.test/'),
      async resolve(url) {
        resolved.push(url);
        await new Promise((later) => setTimeout(later, 10));
        if (url.endsWith('/data.json')) return '{"a": 1}';
        if (url.endsWith('/child-42.scxml')) return CHILD;
        throw new Error('not found');
      },
    };
    const data = await createSession({
      ir: dataIr,
      async: 'auto',
      dataModel: 'ecmascript',
      timers: 'manual',
      loadCoreModule,
      resources,
      telemetry,
    });
    assert.ok(data instanceof AsyncMachineSession);
    assert.equal(data.mechanism, 'worker');
    // The static URL was prefetched in the page before the worker started.
    assert.deepEqual(resolved, ['https://example.test/data.json']);
    assert.deepEqual((await data.start()).activeConfiguration, ['loaded']);
    assert.deepEqual(resolved, ['https://example.test/data.json']);
    // The page resolved the URL inside a `capability` span.
    assert.ok(spans.some((span) => span.name === 'capability' && span.ended));
    await data.dispose();
    await assert.rejects(data.status(), hostError('instance-lost'));

    const compiled = [];
    const parent = await createSession({
      ir: parentIr,
      async: 'auto',
      dataModel: 'ecmascript',
      timers: 'manual',
      loadCoreModule,
      resources,
      capabilities: {
        // Page providers are called synchronously from the worker, and may
        // themselves answer with a Promise.
        compileInvocation: async (sourceName, source) => {
          compiled.push(sourceName);
          if (source !== CHILD) throw new CapabilityError('invalid-input');
          return childIr;
        },
      },
    });
    const started = await parent.start();
    assert.deepEqual(started.activeConfiguration, ['pass']);
    assert.equal(await parent.status(), 'finished');
    assert.deepEqual(resolved.slice(1), ['https://example.test/child-42.scxml']);
    assert.deepEqual(compiled, ['https://example.test/child-42.scxml']);
    await parent.dispose();
  });
});

test('worker: a denied or failing URL, a resolve rejection of any code, or a malformed answer, is error.execution, for a static and a runtime URL alike', async () => {
  const [ir, parentIr] = await Promise.all([compile(DATA_DOCUMENT), compile(PARENT)]);
  await withWorkerScope(async () => {
    for (const resources of [
      { allow: () => false, resolve: async () => '{"a": 1}' },
      { allow: () => true, resolve: async () => Promise.reject(new Error('offline')) },
      { allow: () => true, resolve: async () => Promise.reject(new CapabilityError('platform')) },
      { allow: () => true, resolve: async () => 42 },
      { allow: () => true, resolve: async () => ({ text: 7 }) },
    ]) {
      const session = await createSession({
        ir,
        async: 'auto',
        dataModel: 'ecmascript',
        timers: 'manual',
        loadCoreModule,
        resources,
      });
      try {
        assert.deepEqual((await session.start()).activeConfiguration, ['denied']);
      } finally {
        await session.dispose();
      }
    }
    // A runtime URL whose answer is neither text nor `{text, mediaType?}`,
    // or whose `resolve` rejects with any error or code, is `unavailable`
    // exactly as a static one: the <invoke> fails with error.execution and
    // the session goes on.
    for (const resolve of [
      async () => 42,
      async () => ({ text: 7 }),
      async () => Promise.reject(new Error('offline')),
      async () => Promise.reject(new CapabilityError('platform')),
      async () => Promise.reject(new CapabilityError('invalid-input')),
    ]) {
      const malformed = await createSession({
        ir: parentIr,
        async: 'auto',
        dataModel: 'ecmascript',
        timers: 'manual',
        loadCoreModule,
        resources: { allow: () => true, resolve },
      });
      try {
        assert.deepEqual((await malformed.start()).activeConfiguration, ['failed']);
        assert.equal(await malformed.status(), 'finished');
      } finally {
        await malformed.dispose();
      }
    }
  });
});

/** `<invoke srcexpr>` of the literal `url`: `pass` when the child ran, else `failed`. */
const RUNTIME_URL = (url) =>
  scxml(
    `<datamodel><data id="where" expr="${JSON.stringify(url).replaceAll('"', '&quot;')}"/></datamodel>
     <state id="outer">
       <invoke id="kid" srcexpr="where"/>
       <transition event="done.invoke.kid" cond="_event.data.answer === 42" target="pass"/>
       <transition event="error.execution" target="failed"/>
     </state>
     <final id="pass"/>
     <final id="failed"/>`,
    ' initial="outer"',
  );

test('worker: allow and resolve get the canonical runtime URL, so dot segments cannot leave an allowed path', async () => {
  const childIr = await compile(CHILD);
  const compileInvocation = async () => childIr;
  await withWorkerScope(async () => {
    for (const [url, canonical] of [
      ['https://example.test/scxml/../admin.scxml', 'https://example.test/admin.scxml'],
      ['https://example.test/scxml/%2e%2e/admin.scxml', 'https://example.test/admin.scxml'],
      ['https://example.test/scxml/..\\admin.scxml', 'https://example.test/admin.scxml'],
    ]) {
      const seen = [];
      const resolved = [];
      const session = await createSession({
        ir: await compile(RUNTIME_URL(url)),
        async: 'auto',
        dataModel: 'ecmascript',
        timers: 'manual',
        loadCoreModule,
        capabilities: { compileInvocation },
        resources: {
          // A raw prefix policy: it sees the canonical form.
          allow: (candidate) => seen.push(candidate) > 0 && candidate.startsWith('https://example.test/scxml/'),
          resolve: async (candidate) => resolved.push(candidate) && CHILD,
        },
      });
      try {
        assert.deepEqual((await session.start()).activeConfiguration, ['failed'], url);
        assert.deepEqual(seen, [canonical], url);
        assert.deepEqual(resolved, [], url);
      } finally {
        await session.dispose();
      }
    }
    // Userinfo never reaches allow; an admitted URL is resolved in the
    // canonical form allow judged.
    for (const [url, expected] of [
      ['https://example.test@evil.test/scxml/child.scxml', { configuration: ['failed'], seen: [] }],
      [
        'https://EXAMPLE.test/scxml/sub/../child.scxml',
        { configuration: ['pass'], seen: ['https://example.test/scxml/child.scxml'] },
      ],
    ]) {
      const seen = [];
      const resolved = [];
      const session = await createSession({
        ir: await compile(RUNTIME_URL(url)),
        async: 'auto',
        dataModel: 'ecmascript',
        timers: 'manual',
        loadCoreModule,
        capabilities: { compileInvocation },
        resources: {
          allow: (candidate) => {
            seen.push(candidate);
            const { origin, pathname } = new URL(candidate);
            return origin === 'https://example.test' && pathname.startsWith('/scxml/');
          },
          resolve: async (candidate) => resolved.push(candidate) && CHILD,
        },
      });
      try {
        assert.deepEqual((await session.start()).activeConfiguration, expected.configuration, url);
        assert.deepEqual(seen, expected.seen, url);
        assert.deepEqual(resolved, expected.seen, url);
      } finally {
        await session.dispose();
      }
    }
  });
});

/** `<invoke srcexpr>` of a fresh URL `count` times; each failure re-enters. */
const RUNTIME_LOOP = (count) =>
  scxml(
    `<datamodel><data id="n" expr="0"/></datamodel>
     <state id="loop">
       <onentry><assign location="n" expr="n + 1"/></onentry>
       <invoke srcexpr="'https://example.test/c' + n + '.scxml'"/>
       <transition event="error.execution" cond="n &lt; ${count}" target="loop"/>
       <transition event="error.execution" target="end"/>
     </state>
     <final id="end"/>`,
    ' initial="loop"',
  );

test('worker: runtime URLs have the prefetch URL-length bound and no count or total bound', async () => {
  await withWorkerScope(async () => {
    const open = (ir, resources, capabilities) =>
      createSession({ ir, async: 'auto', dataModel: 'ecmascript', timers: 'manual', loadCoreModule, resources, capabilities });

    // A URL over MAX_RESOURCE_URL_BYTES never reaches allow.
    let asked = 0;
    const long = await open(await compile(RUNTIME_URL(`https://example.test/${'a'.repeat(MAX_RESOURCE_URL_BYTES)}`)), {
      allow: () => (asked += 1) > 0,
      resolve: async () => CHILD,
    });
    try {
      assert.deepEqual((await long.start()).activeConfiguration, ['failed']);
      assert.equal(asked, 0);
    } finally {
      await long.dispose();
    }

    // Every runtime URL reaches allow: all 70.
    const seen = [];
    const counted = await open(await compile(RUNTIME_LOOP(70)), {
      allow: (url) => seen.push(url) > 0,
      resolve: async () => Promise.reject(new Error('not found')),
    });
    try {
      assert.deepEqual((await counted.start()).activeConfiguration, ['end']);
      assert.equal(seen.length, 70);
    } finally {
      await counted.dispose();
    }

    // Resolved text of any total is handed to the worker: six 3.5 MiB
    // documents, 21 MiB in all.
    const document = `<!--${'x'.repeat(3.5 * 1024 * 1024)}-->${CHILD}`;
    const compiled = [];
    const heavy = await open(
      await compile(RUNTIME_LOOP(6)),
      { allow: () => true, resolve: async () => document },
      {
        compileInvocation: async (sourceName) => {
          compiled.push(sourceName);
          throw new CapabilityError('invalid-input');
        },
      },
    );
    try {
      assert.deepEqual((await heavy.start()).activeConfiguration, ['end']);
      assert.equal(compiled.length, 6);
    } finally {
      await heavy.dispose();
    }
  });
});

test('worker: one operation at a time, and typed failures cross the boundary', async () => {
  const ir = await compile(PARENT);
  await withWorkerScope(async () => {
    let session;
    let reentrant;
    session = await createSession({
      ir,
      async: 'auto',
      dataModel: 'ecmascript',
      timers: 'manual',
      loadCoreModule,
      resources: {
        allow: () => true,
        async resolve() {
          // A provider the session is waiting on (the runtime child URL)
          // cannot call back into it.
          reentrant = await session.status().catch((error) => error);
          return CHILD;
        },
      },
    });
    assert.ok(reentrant === undefined);
    const start = session.start();
    await assert.rejects(session.status(), hostError('reentrant-call'));
    assert.deepEqual((await start).activeConfiguration, ['pass']);
    assert.ok(reentrant instanceof ScxmlHostError && reentrant.code === 'reentrant-call');
    // Session errors keep their family and code.
    await assert.rejects(session.advanceTime(-1), hostError('invalid-argument'));
    await assert.rejects(session.send(() => 0), hostError('invalid-argument'));
    await session.dispose();
  });
});

test('worker: dispose does not wait on a page promise a timer-driven call is blocked on', async () => {
  const ir = await compile(
    scxml(
      `<state id="a">
         <onentry><send event="go" delay="10ms"/></onentry>
         <transition event="go" target="b"/>
       </state>
       <state id="b"><invoke srcexpr="'https://example.test/never.scxml'"/></state>`,
      ' initial="a"',
    ),
  );
  await withWorkerScope(async () => {
    let asked;
    const reached = new Promise((resolve) => {
      asked = resolve;
    });
    const session = await createSession({
      ir,
      async: 'auto',
      dataModel: 'ecmascript',
      timers: 'event-loop',
      loadCoreModule,
      resources: {
        allow: () => true,
        // Never settles: the worker stays blocked in the timer's delivery.
        resolve: () => {
          asked();
          return new Promise(() => {});
        },
      },
    });
    assert.deepEqual((await session.start()).activeConfiguration, ['a']);
    await reached;
    const bound = new Promise((resolve) => setTimeout(() => resolve('hung'), 5000));
    assert.equal(await Promise.race([session.dispose().then(() => 'disposed'), bound]), 'disposed');
    await assert.rejects(session.status(), hostError('instance-lost'));
  });
});

// --- none fallback ------------------------------------------------------------------

test('none: invokeLimits are read once, and the session keeps the frozen copy', async () => {
  const invoking = await compile(
    scxml(
      `<state id="a">
         <invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml></content></invoke>
         <transition event="error.execution" target="refused"/>
       </state>
       <final id="refused"/>`,
      ' initial="a"',
    ),
  );
  const traps = [];
  const target = { maxInvokeDepth: 0 };
  const proxied = new Proxy(target, {
    ownKeys: (object) => {
      traps.push('ownKeys');
      return Reflect.ownKeys(object);
    },
    getOwnPropertyDescriptor: (object, key) => {
      traps.push(`descriptor:${String(key)}`);
      return Reflect.getOwnPropertyDescriptor(object, key);
    },
    get: (object, key) => {
      traps.push(`get:${String(key)}`);
      return Reflect.get(object, key);
    },
  });
  const session = await createSession({
    ir: invoking,
    async: 'auto',
    dataModel: 'ecmascript',
    timers: 'manual',
    loadCoreModule,
    invokeLimits: proxied,
  });
  try {
    assert.equal(session.mechanism, 'none');
    assert.deepEqual(traps, ['ownKeys', 'descriptor:maxInvokeDepth'], 'one read of each own property');
    target.maxInvokeDepth = 5;
    assert.deepEqual((await session.start()).activeConfiguration, ['refused'], 'the copy taken at creation applies');
    assert.deepEqual(traps, ['ownKeys', 'descriptor:maxInvokeDepth']);
  } finally {
    await session.dispose();
  }
});

test('none: a runtime URL the page does not supply is the typed dynamic-url-unavailable error', async () => {
  const [ir, dataIr] = await Promise.all([compile(PARENT), compile(DATA_DOCUMENT)]);
  assert.equal(UNSUPPORTED.dynamicUrl, 'dynamic-url-unavailable');
  assert.throws(
    () => UNAVAILABLE_BRIDGE.resolveDynamic('https://example.test/data.json'),
    (failure) => failure.payload.code === 'unavailable' && failure.payload.detail === UNSUPPORTED.dynamicUrl,
  );
  const { telemetry, spans } = recordingTelemetry();
  let resolved = 0;
  const session = await createSession({
    ir,
    async: 'auto',
    dataModel: 'ecmascript',
    timers: 'manual',
    loadCoreModule,
    telemetry,
    resources: {
      allow: () => true,
      resolve: async () => {
        resolved += 1;
        return '{"a": 1}';
      },
    },
  });
  assert.ok(session instanceof AsyncMachineSession);
  assert.equal(session.mechanism, 'none');
  // The machine raised error.execution; the page's resolver never ran.
  assert.deepEqual((await session.start()).activeConfiguration, ['failed']);
  assert.equal(resolved, 0);
  const rejected = spans.filter(
    (span) => span.name === 'capability' && span.attributes['scxml.outcome'] === 'rejected',
  );
  assert.ok(rejected.length >= 1);
  // One operation at a time here too.
  const status = session.status();
  await assert.rejects(session.configuration(), hostError('reentrant-call'));
  assert.equal(await status, 'finished');
  await session.dispose();

  // A static URL is still prefetched through `resources` under `none`.
  const data = await createSession({
    ir: dataIr,
    async: 'auto',
    dataModel: 'ecmascript',
    timers: 'manual',
    loadCoreModule,
    resources: {
      allow: () => true,
      resolve: async () => {
        resolved += 1;
        return '{"a": 1}';
      },
    },
  });
  assert.equal(data.mechanism, 'none');
  assert.equal(resolved, 1);
  assert.deepEqual((await data.start()).activeConfiguration, ['loaded']);
  await data.dispose();
});

test('none: a page loadDocument provider still answers runtime URLs exactly as before', async () => {
  const ir = await compile(PARENT);
  const session = await createSession({
    ir,
    async: 'auto',
    dataModel: 'ecmascript',
    timers: 'manual',
    loadCoreModule,
    resources: { allow: () => true, resolve: async () => '{}' },
    capabilities: { loadDocument: () => CHILD },
  });
  assert.deepEqual((await session.start()).activeConfiguration, ['pass']);
  await session.dispose();
});

// --- prefetched resources in a worker session ---------------------------------------

/** The outcome of creating and starting one session: a configuration or a typed failure. */
async function startOutcome(options) {
  let session;
  try {
    session = await createSession({ timers: 'manual', loadCoreModule, ...options });
    const step = await session.start();
    return { configuration: step.activeConfiguration, status: await session.status() };
  } catch (error) {
    return {
      error: `${error.name}:${error.code}`,
      ...(error.sessionFailed === undefined ? {} : { sessionFailed: error.sessionFailed }),
    };
  } finally {
    await session?.dispose();
  }
}

test('worker: the page prefetch reaches the worker, with the synchronous platform and payload-too-large answers and no script text bound', async () => {
  const scripted = await compile(
    scxml(
      `<script src="https://example.test/lib.js"/>
       <state id="a"><transition cond="typeof libValue === 'number'" target="ok"/></state>
       <final id="ok"/>`,
      ' initial="a"',
    ),
  );
  const data = await compile(DATA_DOCUMENT);
  // Reads a document as whitespace-normalized text (SCXML B.2.1).
  const text = await compile(
    scxml(
      `<datamodel><data id="doc" src="https://example.test/data.txt"/></datamodel>
       <state id="a">
         <transition event="error.execution" target="denied"/>
         <transition cond="doc === 'x y'" target="loaded"/>
       </state>
       <final id="denied"/>
       <final id="loaded"/>`,
      ' initial="a"',
    ),
  );
  const cases = [
    // A prefetched script loads in the worker from the page's answer.
    { ir: scripted, resolve: async () => 'var libValue = 7;' },
    // A failed fetch of a static script is `platform`: the document is rejected.
    { ir: scripted, resolve: async () => Promise.reject(new Error('offline')) },
    // An empty static script is `payload-too-large`: the document is rejected.
    { ir: scripted, resolve: async () => '' },
    // A static script over 64 KiB loads: the host bounds no script text, as
    // under Wasmtime; the data model's own bound refuses to run it
    // (error.execution), so `libValue` stays undefined.
    { ir: scripted, resolve: async () => `var libValue = 7; //${'x'.repeat(65 * 1024)}` },
    // A static document over 4 MiB loads.
    { ir: text, resolve: async () => `x${' '.repeat(4 * 1024 * 1024)}y` },
    // A static document loads.
    { ir: data, resolve: async () => '{"a": 1}' },
  ];
  const expected = [
    { configuration: ['ok'], status: 'finished' },
    { error: 'ScxmlSessionError:platform', sessionFailed: true },
    { error: 'ScxmlSessionError:payload-too-large', sessionFailed: true },
    { configuration: ['a'], status: 'running' },
    { configuration: ['loaded'], status: 'finished' },
    { configuration: ['loaded'], status: 'finished' },
  ];
  const run = async (mode, { ir, resolve: answer }) => {
    const calls = [];
    const resources = {
      allow: (url) => url.startsWith('https://example.test/'),
      resolve: (url) => {
        calls.push(url);
        return answer(url);
      },
    };
    // A synchronous session takes a data-model instance; an async one, its name.
    const dataModel = mode === 'off' ? await createEcmaScriptDataModel({ loadCoreModule }) : 'ecmascript';
    const outcome = await startOutcome({ ir, dataModel, resources, async: mode });
    return { outcome, calls };
  };
  const synchronous = [];
  for (const entry of cases) synchronous.push(await run('off', entry));
  assert.deepEqual(
    synchronous.map(({ outcome }) => outcome),
    expected,
  );
  await withWorkerScope(async () => {
    for (const [index, entry] of cases.entries()) {
      const { outcome, calls } = await run('auto', entry);
      // The worker's answers are the synchronous session's, and the page
      // resolved each static URL exactly once (during the prefetch).
      assert.deepEqual(outcome, synchronous[index].outcome, `case ${index}`);
      assert.deepEqual(calls, synchronous[index].calls, `case ${index}`);
      assert.equal(calls.length, 1);
    }
  });
});
