// Runs the shared fixture trace in real headless Chromium (pinned
// playwright-core; its matching chromium-headless-shell must be installed,
// see README). The page loads src/ and dist/ over HTTP with the default
// fetch + WebAssembly.compileStreaming loader, exactly as a site would.
//
// It also runs a hostile nested-loop script on the bundled ECMAScript data
// model and requires the fuel budget to stop it with a typed failure, runs
// one heavy command whose nested data-model work must be charged to that
// command's one budget (`fuel-exhausted`), and compiles an XPath document in the page whose delayed send the page's own
// event loop must deliver through the linked SCXML I/O processor, and an
// `<invoke>` whose child the host builds synchronously on the main thread
// from IR the page pre-lowered, the same child inline with nothing from
// the page (the host's automatic compiler), a `<data src>` the page
// allowlists through `resources` and fetches over HTTP before the machine
// runs (a second, denied URL is never fetched), and BasicHTTP: sends through
// the default `fetch` to allowlisted endpoints of this server (one answers
// 500, so its `error.communication` arrives later), a denied URL the server
// must never see, and an inbound request through `receiveHttp`.
//
// The async bridge page (test/browser/async.html) runs twice: served with
// cross-origin isolation headers (COOP same-origin, COEP require-corp) it
// gets the `worker` mechanism: a `<data src>` document prefetched in the
// page and a runtime-computed `<invoke srcexpr>` child load through the
// page's asynchronous `resources.resolve`; served without them it gets the
// `none` fallback, where the prefetched document still loads and the
// runtime URL is the typed `dynamic-url-unavailable` error.
//
// Inputs: SCXML_BROWSER_IR (CLI-compiled lowered IR of the representative
// fixture). Serves only this package's src/, dist/, test/, the pinned
// `@opentelemetry/api` ESM build, the trace fixture, and that IR, plus the
// recording BasicHTTP endpoints under /basichttp/, on 127.0.0.1 with an
// ephemeral port.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, extname, join, normalize, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright-core';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const irPath = process.env.SCXML_BROWSER_IR;
assert.ok(
  irPath,
  'SCXML_BROWSER_IR must name the CLI-compiled lowered IR of tests/fixtures/representative-machine.scx; ' +
    'run `sh scripts/verify-browser-host.sh` from the repository root (README.md, "Tests")',
);
const tracePath = join(workspace, 'tests/fixtures/representative-machine.trace.json');
const fixture = JSON.parse(await readFile(tracePath, 'utf8'));

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
  '.scxml': 'application/xml',
};

// The pinned `@opentelemetry/api` ESM build uses extensionless relative
// specifiers (written for bundlers); the server resolves them to `.js`.
const OTEL_API_ESM = 'node_modules/@opentelemetry/api/build/esm';

function resolveRequest(pathname) {
  if (pathname === '/fixture/trace.json') return tracePath;
  if (pathname === '/fixture/ir.json') return irPath;
  const path = normalize(join(packageRoot, decodeURIComponent(pathname)));
  const inside = (dir) => path.startsWith(join(packageRoot, dir) + sep);
  if (inside(OTEL_API_ESM)) return extname(path) === '' ? `${path}.js` : path;
  return inside('src') || inside('dist') || inside('test') ? path : undefined;
}

// Cross-origin isolation headers (the async bridge's `worker` mechanism
// needs a crossOriginIsolated page and worker).
const ISOLATION = {
  'cross-origin-opener-policy': 'same-origin',
  'cross-origin-embedder-policy': 'require-corp',
};

/**
 * Serves the allowed files on 127.0.0.1 (ephemeral port), optionally
 * isolated, and records every request that reaches a /basichttp/ endpoint
 * into `posts`, in arrival order.
 */
async function serve({ isolated = false, posts = [] } = {}) {
  const server = createServer(async (request, response) => {
    const { pathname } = new URL(request.url, 'http://127.0.0.1');
    if (pathname.startsWith('/basichttp/')) {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      posts.push({
        method: request.method,
        path: pathname,
        contentType: request.headers['content-type'],
        cookie: request.headers.cookie,
        body: Buffer.concat(chunks).toString('utf8'),
      });
      response.writeHead(pathname === '/basichttp/fail' ? 500 : 200).end();
      return;
    }
    const path = resolveRequest(pathname);
    try {
      if (path === undefined) throw new Error('not served');
      let body = await readFile(path);
      // Import maps do not apply inside workers: for a worker's module
      // graph, resolve the one bare specifier as a bundler would.
      if (request.headers['sec-fetch-dest'] === 'worker' && extname(path) === '.js') {
        body = String(body).replaceAll("from '@opentelemetry/api'", `from '/${OTEL_API_ESM}/index.js'`);
      }
      response.writeHead(200, {
        'content-type': TYPES[extname(path)] ?? 'application/octet-stream',
        ...(isolated ? ISOLATION : {}),
      });
      response.end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  return server;
}

/** Loads one test page in headless Chromium and returns its window.__result. */
async function runPage(browser, port, page) {
  const tab = await browser.newPage();
  const errors = [];
  tab.on('pageerror', (error) => errors.push(String(error)));
  await tab.goto(`http://127.0.0.1:${port}/test/browser/${page}`);
  await tab.waitForFunction(() => window.__result !== undefined, null, { timeout: 60_000 });
  const result = await tab.evaluate(() => window.__result);
  await tab.close();
  assert.deepEqual(errors, []);
  assert.ok(result.ok, result.error);
  return result.value;
}

test('headless Chromium reproduces the shared Wasmtime trace', async () => {
  // Every request that reached a /basichttp/ endpoint, in arrival order.
  const posts = [];
  const server = await serve({ posts });
  const { port } = server.address();
  const browser = await chromium.launch({ headless: true });
  try {
    const { trace, hostile, nestedLoop, sharedBudget, eventLoop, invocation, autoInvocation, resources, basichttp, userAgent } =
      await runPage(browser, port, 'index.html');
    assert.match(userAgent, /HeadlessChrome/);
    assert.deepEqual(trace.mismatches, []);
    assert.equal(trace.consumed, fixture['data-model-calls'].length);
    assert.deepEqual(trace.results, fixture.results);
    assert.equal(trace.finalStatus, 'shut-down');
    assert.equal(hostile, 'unsupported-version');
    // Fuel exhaustion fails the data-model call closed instead of hanging
    // the page (without fuel this runs about 1e10 Boa loop iterations).
    assert.equal(nestedLoop.code, 'platform');
    assert.ok(nestedLoop.elapsed < 60_000, `exhaustion took ${nestedLoop.elapsed} ms`);
    // One budget per host call: the nested data-model work of one heavy
    // `start` is charged to it, so the command runs out of fuel.
    assert.equal(sharedBudget.code, 'fuel-exhausted');
    assert.ok(sharedBudget.elapsed < 30_000, `exhaustion took ${sharedBudget.elapsed} ms`);
    // The page's event loop delivered an XPath document's delayed send.
    assert.deepEqual(eventLoop.initial, ['a']);
    assert.equal(eventLoop.error, undefined);
    assert.deepEqual(eventLoop.delivered, [['b']]);
    assert.deepEqual(eventLoop.logs, ['Outcome: pass']);
    assert.ok(eventLoop.elapsed >= 15, `delivered after ${eventLoop.elapsed} ms`);
    // A child built synchronously on the page's main thread from pre-lowered
    // IR: its delayed send, its event to the parent, and done.invoke.
    assert.equal(invocation.error, undefined);
    assert.deepEqual(invocation.initial, ['waiting']);
    assert.deepEqual(invocation.delivered, [['heard'], ['done']]);
    assert.equal(invocation.status, 'finished');
    // The same child inline, with no page compileInvocation: the host's
    // automatic compiler builds it.
    assert.equal(autoInvocation.error, undefined);
    assert.deepEqual(autoInvocation.initial, ['waiting']);
    assert.deepEqual(autoInvocation.delivered, [['heard'], ['done']]);
    assert.equal(autoInvocation.status, 'finished');
    // URL resources: only the allowlisted document was fetched, and bound.
    assert.deepEqual(resources.fetched, [`http://127.0.0.1:${port}/test/browser/resource-data.json`]);
    assert.deepEqual(resources.initial, ['loaded']);
    // BasicHTTP: the component's encoding over the default fetch, only to
    // allowlisted URLs and without the page's cookie; the 500 arrives later
    // as error.communication, the denied URL raises it at once and is never
    // requested, and an inbound request is decoded by the component.
    assert.equal(basichttp.error, undefined);
    assert.deepEqual(basichttp.initial, ['a']);
    assert.deepEqual(basichttp.late, [['c']]);
    assert.deepEqual(basichttp.inbound, { status: 200, configuration: ['d'] });
    assert.deepEqual(basichttp.logs, ['late: platform', 'denied: platform', 'inbound: {"x":"1"}']);
    const form = 'application/x-www-form-urlencoded';
    assert.deepEqual(posts, [
      { method: 'POST', path: '/basichttp/inbox', contentType: form, cookie: undefined, body: '_scxmleventname=hello&k=v%20w' },
      { method: 'POST', path: '/basichttp/fail', contentType: form, cookie: undefined, body: '_scxmleventname=bad' },
    ]);
    console.log(`browser: nested-loop fuel exhaustion after ${Math.round(nestedLoop.elapsed)} ms`);
    console.log(`browser: heavy-command fuel exhaustion after ${Math.round(sharedBudget.elapsed)} ms`);
    console.log(`browser: ${userAgent}`);
  } finally {
    await browser.close();
    await new Promise((done) => server.close(done));
  }
});

test('async bridge: runtime-computed URLs load through resources in a worker, else the typed error', async () => {
  const isolated = await serve({ isolated: true });
  const plain = await serve();
  const browser = await chromium.launch({ headless: true });
  try {
    // Cross-origin isolated: the `worker` mechanism. The static `<data src>`
    // document is prefetched in the page and handed to the worker; the
    // `<invoke srcexpr>` child (a URL only the data model computes) loads
    // through the page's asynchronous `resources.resolve` while the worker
    // blocks; the child's delayed send runs on the
    // worker's event loop, and its outcome reaches the page's `onTimer`.
    const worker = await runPage(browser, isolated.address().port, 'async.html');
    assert.equal(worker.crossOriginIsolated, true);
    assert.deepEqual(worker.data, { mechanism: 'worker', configuration: ['loaded'] });
    assert.deepEqual(worker.invocation, {
      mechanism: 'worker',
      initial: ['outer'],
      delivered: [['pass']],
      status: 'finished',
    });
    assert.deepEqual(worker.resolved, ['async-data.json', 'async-child.scx']);

    // Not isolated: the `none` fallback. The static `<data src>` is still
    // prefetched through `resources` before the machine runs; the runtime
    // `<invoke srcexpr>` URL fails with `unavailable` /
    // `dynamic-url-unavailable` (error.execution), and the page's resolver
    // is never called for it.
    const fallback = await runPage(browser, plain.address().port, 'async.html');
    assert.equal(fallback.crossOriginIsolated, false);
    assert.deepEqual(fallback.data, { mechanism: 'none', configuration: ['loaded'] });
    assert.equal(fallback.invocation.mechanism, 'none');
    assert.deepEqual(fallback.invocation.initial, ['failed']);
    assert.equal(fallback.invocation.status, 'finished');
    assert.deepEqual(fallback.resolved, ['async-data.json']);
  } finally {
    await browser.close();
    await new Promise((done) => isolated.close(done));
    await new Promise((done) => plain.close(done));
  }
});
