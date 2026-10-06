// Contract tests of the browser host's BasicHTTP Event I/O Processor
// (src/basichttp.js, SCXML C.2): deny-by-default, the `http.allow`
// allowlist, the component's request encoding, `error.communication` for
// every undeliverable send (synchronous, and later for a fire-and-forget
// send), inbound decoding through `session.receiveHttp` (to the root or a
// live invoked session at any depth), the in-process loopback between the root and its invoked
// children, and `_ioprocessors`.
//
// Runs in Node on the same ES modules and metered core Wasm a browser loads.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  ScxmlHostError,
  ScxmlSessionError,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createSession,
} from '../src/index.js';
import { inboundTraceContext } from '../src/basichttp.js';
import { recordingTelemetry } from './recording-telemetry.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const compiler = await createCompiler({ loadCoreModule });

const BASICHTTP = 'http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor';
const FORM = 'application/x-www-form-urlencoded';
const LOCATION = 'https://example.test/scxml/session';

const scxml = (body, attributes = '') =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="a"${attributes}>${body}</scxml>`;

async function open(source, options = {}) {
  const compiled = await compiler.compile(source, { sourceName: 'basichttp.scxml' });
  assert.equal(compiled.status, 'emitted', new TextDecoder().decode(compiled.report));
  const logs = [];
  const { capabilities, ...rest } = options;
  const session = await createSession({
    ir: compiled.ir,
    dataModel: await createEcmaScriptDataModel({ loadCoreModule }),
    capabilities: { log: ({ label, text }) => logs.push(label === undefined ? text : `${label}: ${text}`), ...capabilities },
    timers: 'manual',
    loadCoreModule,
    ...rest,
  });
  return { session, logs };
}

/** A document whose onentry performs `send`, then logs how the platform answered. */
const sender = (send, after = '') =>
  scxml(`<state id="a">
     <onentry>${send}<log label="after" expr="'sent'"/>${after}</onentry>
     <transition event="error.communication" target="communication">
       <log label="error" expr="_event.name + ' ' + _event.type + ' ' + String(_event.sendid) + ' ' + String(_event.origintype)"/>
     </transition>
     <transition event="error.execution" target="execution"/>
   </state>
   <state id="communication"/>
   <state id="execution"/>`);

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

/** A recording `http.send` answering `response` (a value or a function of the request). */
function recorder(response = { status: 200 }) {
  const requests = [];
  const send = (request) => {
    requests.push(request);
    return typeof response === 'function' ? response(request) : response;
  };
  return { requests, send };
}

test('deny by default: without http, a BasicHTTP send raises the platform error.communication and nothing is sent', async () => {
  const original = globalThis.fetch;
  let fetched = 0;
  globalThis.fetch = () => {
    fetched += 1;
    return Promise.resolve(new Response(''));
  };
  try {
    const { session, logs } = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://example.test/"/>`));
    assert.deepEqual(session.start().activeConfiguration, ['communication']);
    // SCXML 5.10: the error is raised on the internal queue and the rest of
    // the block is skipped, exactly as under Wasmtime.
    assert.deepEqual(logs, ['error: error.communication platform sendid_0 undefined']);
    assert.equal(fetched, 0);
    session.dispose();
  } finally {
    globalThis.fetch = original;
  }
});

test('the allowlist: only a URL http.allow accepts with exactly true reaches http.send', async () => {
  for (const allow of [() => false, () => 'yes', () => 1, () => { throw new Error('policy'); }]) {
    const { requests, send } = recorder();
    const { session, logs } = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://blocked.test/"/>`), {
      http: { allow, send },
    });
    assert.deepEqual(session.start().activeConfiguration, ['communication']);
    assert.deepEqual(logs, ['error: error.communication platform sendid_0 undefined']);
    assert.deepEqual(requests, []);
    session.dispose();
  }
  const seen = [];
  const { requests, send } = recorder();
  const { session, logs } = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://allowed.test/in"/>`), {
    http: {
      allow: (url) => {
        seen.push(url);
        return url === 'https://allowed.test/in';
      },
      send,
    },
  });
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  assert.deepEqual(logs, ['after: sent']);
  assert.deepEqual(seen, ['https://allowed.test/in']);
  assert.equal(requests.length, 1);
  session.dispose();
});

// Dot segments a raw prefix check would admit under `/scxml/`, and what the
// WHATWG URL parser (and so fetch) makes of them.
const ESCAPES = [
  ['https://api.example/scxml/../admin', 'https://api.example/admin'],
  ['https://api.example/scxml/%2e%2e/admin', 'https://api.example/admin'],
  ['https://api.example/scxml/%2E%2E/%2e%2e/other', 'https://api.example/other'],
  ['https://api.example/scxml/..\\admin', 'https://api.example/admin'],
];

test('http.allow judges the canonical target, so dot segments cannot leave an allowed path', async () => {
  for (const [target, canonical] of ESCAPES) {
    const seen = [];
    const { requests, send } = recorder();
    const { session, logs } = await open(sender(`<send event="x" type="${BASICHTTP}" target="${target}"/>`), {
      http: {
        // A raw prefix policy: it sees the canonical form, not `target`.
        allow: (url) => {
          seen.push(url);
          return url.startsWith('https://api.example/scxml/');
        },
        send,
      },
    });
    assert.deepEqual(session.start().activeConfiguration, ['communication'], target);
    assert.deepEqual(logs, ['error: error.communication platform sendid_0 undefined']);
    assert.deepEqual(seen, [canonical], target);
    assert.deepEqual(requests, [], target);
    session.dispose();
  }
});

test('http.send and the default fetch get the canonical target that http.allow accepted', async () => {
  const allow = (url) => {
    const { origin, pathname } = new URL(url);
    return origin === 'https://api.example' && pathname.startsWith('/scxml/');
  };
  const { requests, send } = recorder();
  const { session } = await open(
    sender(`<send event="x" type="${BASICHTTP}" target="https://API.example/scxml/sub/%2e%2e/in"/>`),
    { http: { allow, send } },
  );
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  assert.deepEqual(requests.map((request) => request.url), ['https://api.example/scxml/in']);
  session.dispose();

  const original = globalThis.fetch;
  const fetched = [];
  globalThis.fetch = (url) => {
    fetched.push(url);
    return Promise.resolve(new Response('', { status: 200 }));
  };
  try {
    const opened = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://api.example/scxml/a/../in"/>`), {
      http: { allow },
    });
    opened.session.start();
    await new Promise((later) => setTimeout(later, 10));
    assert.deepEqual(fetched, ['https://api.example/scxml/in']);
    opened.session.dispose();
  } finally {
    globalThis.fetch = original;
  }
});

test('a target with userinfo is denied without reaching http.allow', async () => {
  for (const target of ['https://api.example@evil.test/scxml/', 'https://user:pass@api.example/scxml/in']) {
    let asked = 0;
    const { requests, send } = recorder();
    const { session } = await open(sender(`<send event="x" type="${BASICHTTP}" target="${target}"/>`), {
      http: { allow: () => (asked += 1) > 0, send },
    });
    assert.deepEqual(session.start().activeConfiguration, ['communication'], target);
    assert.equal(asked, 0, target);
    assert.deepEqual(requests, [], target);
    session.dispose();
  }
});

test('the request is the component encoding: POST, form media type, _scxmleventname first, then namelist and params', async () => {
  const { requests, send } = recorder();
  const { session } = await open(
    scxml(`<datamodel><data id="Var1" expr="2"/><data id="text" expr="'a b&amp;c=d/é'"/></datamodel>
       <state id="a"><onentry>
         <send event="test.event" type="${BASICHTTP}" target="https://example.test/p?q=1" namelist="Var1">
           <param name="param1" expr="text"/>
           <param name="n" expr="2.5"/>
           <param name="flag" expr="true"/>
         </send>
         <send type="basichttp" target="https://example.test/c"><content>this is some content</content></send>
       </onentry></state>`),
    { http: { allow: () => true, send } },
  );
  session.start();
  assert.deepEqual(requests, [
    {
      method: 'POST',
      url: 'https://example.test/p?q=1',
      headers: { 'Content-Type': FORM },
      body: '_scxmleventname=test.event&Var1=2&param1=a%20b%26c%3Dd%2F%C3%A9&n=2.5&flag=true',
    },
    {
      method: 'POST',
      url: 'https://example.test/c',
      headers: { 'Content-Type': FORM },
      body: 'this%20is%20some%20content',
    },
  ]);
  session.dispose();
});

test('an undeliverable send raises error.communication; an invalid one, delayed or not, error.execution', async () => {
  // A synchronous transport answer is interpreted at once, as under Wasmtime.
  for (const response of [{ status: 500 }, { status: 404 }, { status: 99 }, { status: 'ok' }, null]) {
    const { session, logs } = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://example.test/"/>`), {
      http: { allow: () => true, send: () => response },
    });
    assert.deepEqual(session.start().activeConfiguration, ['communication'], JSON.stringify(response));
    assert.deepEqual(logs, ['error: error.communication platform sendid_0 undefined']);
    session.dispose();
  }
  const thrower = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://example.test/"/>`), {
    http: { allow: () => true, send: () => { throw new Error('offline'); } },
  });
  assert.deepEqual(thrower.session.start().activeConfiguration, ['communication']);
  thrower.session.dispose();
  // No target (SCXML C.2, official 577): error.communication.
  const untargeted = await open(sender(`<send event="x" type="${BASICHTTP}"/>`), { http: { allow: () => true, send: () => ({ status: 200 }) } });
  assert.deepEqual(untargeted.session.start().activeConfiguration, ['communication']);
  untargeted.session.dispose();
  // Not an absolute http(s) URI: the component rejects it before sending.
  const { requests, send } = recorder();
  const invalid = await open(sender(`<send event="x" type="${BASICHTTP}" target="ftp://example.test/"/>`), {
    http: { allow: () => true, send },
  });
  assert.deepEqual(invalid.session.start().activeConfiguration, ['execution']);
  assert.deepEqual(requests, []);
  invalid.session.dispose();
  // A delayed send is checked at the <send> (SCXML 6.2: "The Processor MUST
  // evaluate all arguments to <send> when the <send> element is evaluated,
  // and not when the message is actually dispatched.").
  const delayedInvalid = await open(sender(`<send event="x" type="${BASICHTTP}" target="ftp://example.test/" delay="1s"/>`), {
    http: { allow: () => true, send },
  });
  assert.deepEqual(delayedInvalid.session.start().activeConfiguration, ['execution']);
  assert.deepEqual(requests, []);
  delayedInvalid.session.dispose();
});

// --- delayed sends (SCXML 6.2, 6.3) -------------------------------------------

/** A root that sends `notify` with `n` to `target` after 50ms and `first` after 10ms. */
const delayedSender = (target, afterFirst = '') =>
  scxml(`<datamodel><data id="n" expr="7"/></datamodel>
   <state id="a">
     <onentry>
       <send id="post" event="notify" type="${BASICHTTP}" target="${target}" delay="50ms" namelist="n"/>
       <assign location="n" expr="8"/>
       <send event="first" delay="10ms"/>
     </onentry>
     <transition event="first" target="b"><log label="first" expr="'at 10'"/>${afterFirst}</transition>
   </state>
   <state id="b">
     <onentry><send event="end" delay="100ms"/></onentry>
     <transition event="error.communication" target="communication">
       <log label="error" expr="_event.sendid + ' ' + _event.type"/>
     </transition>
     <transition event="end" target="ended"/>
   </state>
   <state id="communication"/>
   <state id="ended"/>`);

test('a delayed BasicHTTP send is posted only when its delay elapses, as evaluated at the <send>', async () => {
  // SCXML 6.2: "It MUST dispatch the message only when the delay interval
  // elapses."
  const { requests, send } = recorder();
  const { session, logs } = await open(delayedSender('https://example.test/in'), { http: { allow: () => true, send } });
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  session.advanceTime(49);
  assert.deepEqual(requests, [], 'nothing is posted before the delay elapses');
  assert.deepEqual(session.configuration(), ['b']);
  session.advanceTime(1);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].url, 'https://example.test/in');
  assert.equal(requests[0].body, '_scxmleventname=notify&n=7');
  session.advanceTime(100);
  assert.deepEqual(session.configuration(), ['ended']);
  assert.deepEqual(logs, ['first: at 10']);
  session.dispose();
});

test('a delayed BasicHTTP send beyond the clock capacity raises error.execution, as under Wasmtime', async () => {
  // SCXML 3.12.2: "it MUST signal any errors that occur by raising SCXML
  // events whose names begin with 'error.'". The clock holds 1024 pending
  // sends; the 1025th raises error.execution and the session goes on.
  const { requests, send } = recorder();
  const { session, logs } = await open(
    scxml(`<datamodel><data id="items" expr="Array.from({ length: 1025 }, function (_, i) { return i; })"/><data id="sent" expr="0"/></datamodel>
     <state id="a">
       <onentry><foreach array="items" item="i"><send type="${BASICHTTP}" event="later" target="https://example.test/x" delay="3600s"/><assign location="sent" expr="sent + 1"/></foreach><log label="unreached"/></onentry>
       <transition event="error.execution" target="done"><log label="clock-full" expr="sent"/></transition>
     </state>
     <final id="done"/>`),
    { http: { allow: () => true, send } },
  );
  assert.deepEqual(session.start().activeConfiguration, ['done']);
  assert.deepEqual(logs, ['clock-full: 1024']);
  assert.deepEqual(requests, []);
  session.dispose();
});

test('a cancelled delayed BasicHTTP send is never posted', async () => {
  // SCXML 6.3: "The <cancel> element is used to cancel a delayed <send>
  // event."
  const { requests, send } = recorder();
  const { session } = await open(delayedSender('https://example.test/in', '<cancel sendid="post"/>'), {
    http: { allow: () => true, send },
  });
  session.start();
  session.advanceTime(200);
  assert.deepEqual(requests, []);
  assert.deepEqual(session.configuration(), ['ended']);
  session.dispose();
});

test('a due BasicHTTP send that cannot be dispatched raises error.communication with its send id when due', async () => {
  // SCXML 6.2.4: "If it is unable to dispatch the message, the Processor
  // MUST place the error error.communication on the internal event queue";
  // 5.10.1: the platform error carries the failed send's id. Refused when
  // due (denied or non-2xx), or failing later (a rejected fetch).
  for (const http of [
    { allow: () => false },
    { allow: () => true, send: () => ({ status: 503 }) },
  ]) {
    const { session, logs } = await open(delayedSender('https://example.test/in'), { http });
    session.start();
    session.advanceTime(50);
    assert.deepEqual(session.configuration(), ['communication']);
    assert.deepEqual(logs, ['first: at 10', 'error: post platform']);
    session.dispose();
  }
  const { session, logs } = await open(delayedSender('https://example.test/in'), {
    http: { allow: () => true, send: () => Promise.reject(new Error('network')) },
  });
  session.start();
  session.advanceTime(50);
  assert.deepEqual(session.configuration(), ['b'], 'accepted when due');
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(session.configuration(), ['communication']);
  assert.deepEqual(logs, ['first: at 10', 'error: post platform']);
  session.dispose();
});

test('a terminated session discards its delayed BasicHTTP send', async () => {
  // SCXML 6.2: "If the SCXML session terminates before the delay interval
  // has elapsed, the SCXML Processor MUST discard the message without
  // attempting to deliver it."
  const { requests, send } = recorder();
  const { session } = await open(
    scxml(`<state id="a">
       <onentry><send event="notify" type="${BASICHTTP}" target="https://example.test/in" delay="50ms"/></onentry>
       <transition target="done"/>
     </state>
     <final id="done"/>`),
    { http: { allow: () => true, send } },
  );
  assert.equal(session.start().status, 'finished');
  session.advanceTime(100);
  assert.deepEqual(requests, []);
  session.dispose();
});

test('a delayed BasicHTTP send to the session itself arrives through the loopback when due', async () => {
  const seen = [];
  let session;
  const opened = await open(
    scxml(`<state id="a">
       <onentry><send event="posted" type="${BASICHTTP}" targetexpr="_ioprocessors['basichttp'].location" delay="20ms"/></onentry>
       <transition event="posted" target="done"><log label="posted" expr="_event.origintype"/></transition>
     </state>
     <final id="done"/>`),
    {
      http: {
        allow: (url) => url === LOCATION,
        location: LOCATION,
        send: (request) => {
          seen.push(request.url);
          return session.receiveHttp(request);
        },
      },
    },
  );
  session = opened.session;
  session.start();
  assert.deepEqual(seen, []);
  session.advanceTime(20);
  assert.deepEqual(seen, [LOCATION]);
  assert.deepEqual(session.configuration(), ['done']);
  assert.deepEqual(opened.logs, [`posted: ${BASICHTTP}`]);
  session.dispose();
});

test('a fire-and-forget send that fails later delivers error.communication then; one that succeeds delivers nothing', async () => {
  const outcomes = [];
  const settle = () => new Promise((resolve) => setTimeout(resolve, 10));
  for (const result of [
    () => Promise.reject(new Error('network')),
    () => Promise.resolve({ status: 503 }),
    () => Promise.resolve({ status: 204 }),
  ]) {
    const { session, logs } = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://example.test/"/>`), {
      http: { allow: () => true, send: result },
      onTimer: (outcome) => outcomes.push(outcome),
    });
    assert.deepEqual(session.start().activeConfiguration, ['a']);
    assert.deepEqual(logs, ['after: sent']);
    await settle();
    logs.push(`config ${session.configuration().join(',')}`);
    session.dispose();
    const succeeded = result.toString().includes('204');
    const expected = succeeded
      ? ['after: sent', 'config a']
      : ['after: sent', 'error: error.communication platform sendid_0 undefined', 'config communication'];
    assert.deepEqual(logs, expected);
    // The page hears each late delivery through onTimer.
    assert.equal(outcomes.length, succeeded ? 0 : 1);
    if (!succeeded) {
      assert.equal(outcomes[0].complete, true);
      assert.deepEqual(outcomes[0].results.at(-1).activeConfiguration, ['communication']);
    }
    outcomes.length = 0;
  }
});

test('every error.communication of a failed BasicHTTP send carries its send id (SCXML 5.10.1)', async () => {
  const settle = () => new Promise((resolve) => setTimeout(resolve, 10));
  // The authored id, the id stored at idlocation, and the id the platform
  // generated for a send with neither (SCXML 3.14).
  const identities = [
    ['id="note"', 'note'],
    ['idlocation="sid"', 'sendid_0'],
    ['', 'sendid_0'],
  ];
  for (const [identity, expected] of identities) {
    const send = `<send ${identity} event="x" type="${BASICHTTP}" target="https://example.test/"/>`;
    const withLocation = (doc) => doc.replace('<state id="a">', '<datamodel><data id="sid"/></datamodel><state id="a">');
    // Refused at once: raised inside the <send>.
    const refused = await open(withLocation(sender(send, '<log label="sid" expr="String(sid)"/>')), {
      http: { allow: () => false },
    });
    assert.deepEqual(refused.session.start().activeConfiguration, ['communication'], identity);
    assert.deepEqual(refused.logs.at(-1), `error: error.communication platform ${expected} undefined`, identity);
    refused.session.dispose();
    // Accepted, then failed: delivered later, still naming its send.
    const late = await open(withLocation(sender(send, '<log label="sid" expr="String(sid)"/>')), {
      http: { allow: () => true, send: () => Promise.resolve({ status: 503 }) },
    });
    assert.deepEqual(late.session.start().activeConfiguration, ['a'], identity);
    await settle();
    assert.deepEqual(late.session.configuration(), ['communication'], identity);
    assert.deepEqual(late.logs.at(-1), `error: error.communication platform ${expected} undefined`, identity);
    if (identity.startsWith('idlocation')) assert.ok(late.logs.includes(`sid: ${expected}`), late.logs.join('|'));
    late.session.dispose();
  }
});

test("an invoked session's late error.communication names its own send", async () => {
  const { session } = await open(
    scxml(`<state id="a">
       <invoke id="kid">
         <content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c">
           <state id="c">
             <onentry><send id="child-note" event="x" type="${BASICHTTP}" target="https://example.test/child"/></onentry>
             <transition event="error.communication" cond="_event.sendid === 'child-note' &amp;&amp; _event.type === 'platform'" target="named"/>
             <transition event="error.communication" target="unnamed"/>
           </state>
           <final id="named"><onentry><send target="#_parent" event="named"/></onentry></final>
           <final id="unnamed"/>
         </scxml></content>
       </invoke>
       <transition event="named" target="pass"/>
       <transition event="error.*" target="leaked"/>
     </state>
     <final id="pass"/>
     <final id="leaked"/>`),
    { http: { allow: () => true, send: () => Promise.reject(new Error('offline')) } },
  );
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(session.configuration(), ['pass']);
  session.dispose();
});

test('a late error.communication runs at the current logical time (the clock syncs first)', async () => {
  let wall = 0;
  const outcomes = [];
  const { session } = await open(
    scxml(`<state id="a">
       <onentry><send event="x" type="${BASICHTTP}" target="https://example.test/late"/></onentry>
       <transition event="error.communication" target="b"><send event="later" delay="100ms"/></transition>
     </state>
     <state id="b"/>`),
    {
      http: { allow: () => true, send: () => Promise.reject(new Error('offline')) },
      timers: 'event-loop',
      now: () => wall,
      onTimer: (outcome) => outcomes.push(outcome),
    },
  );
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  wall = 1000;
  await new Promise((later) => setTimeout(later, 20));
  assert.equal(outcomes.length, 1);
  assert.deepEqual(session.configuration(), ['b']);
  // Scheduled from the late handler at logical 1000, not at the stale 0.
  assert.equal(session.nextDeadline(), 1100);
  session.dispose();
});

test('the default send is fetch without credentials or redirects, only for allowed URLs', async () => {
  const original = globalThis.fetch;
  const calls = [];
  globalThis.fetch = (url, init) => {
    calls.push({ url, init });
    return Promise.resolve(new Response('ignored', { status: 200 }));
  };
  try {
    const { session } = await open(sender(`<send event="x" type="${BASICHTTP}" target="https://example.test/ok"><param name="k" expr="'v'"/></send>`), {
      http: { allow: (url) => url === 'https://example.test/ok' },
    });
    session.start();
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.deepEqual(session.configuration(), ['a']);
    assert.deepEqual(calls, [
      {
        url: 'https://example.test/ok',
        init: {
          method: 'POST',
          headers: { 'Content-Type': FORM },
          body: '_scxmleventname=x&k=v',
          credentials: 'omit',
          redirect: 'error',
        },
      },
    ]);
    session.dispose();
  } finally {
    globalThis.fetch = original;
  }
});

/** A document that logs every field of the one event it receives. */
const receiver = scxml(`<state id="a">
     <transition event="*" target="got">
       <log label="name" expr="_event.name"/>
       <log label="type" expr="_event.type"/>
       <log label="origintype" expr="String(_event.origintype)"/>
       <log label="data" expr="JSON.stringify(_event.data)"/>
       <log label="raw" expr="_event.raw"/>
     </transition>
   </state>
   <state id="got"/>`);

test('receiveHttp decodes an inbound request as the component decodes it and answers 200', async () => {
  const { session, logs } = await open(receiver, { http: { allow: () => false, location: LOCATION } });
  session.start();
  const response = session.receiveHttp({
    method: 'POST',
    headers: { 'Content-Type': `${FORM}; charset=utf-8` },
    body: 'a=1&_scxmleventname=hello.world&a=2&b=x+y%21&_scxmleventname=ignored',
  });
  assert.deepEqual(response, { status: 200, headers: {}, body: '' });
  assert.deepEqual(session.configuration(), ['got']);
  assert.deepEqual(logs, [
    'name: hello.world',
    'type: external',
    `origintype: ${BASICHTTP}`,
    'data: {"a":["1","2"],"b":"x y!"}',
    `raw: POST ${LOCATION} HTTP/1.1\r\nContent-Type: ${FORM}; charset=utf-8\r\n\r\na=1&_scxmleventname=hello.world&a=2&b=x+y%21&_scxmleventname=ignored`,
  ]);
  session.dispose();

  // No form: the header names the event and the body is text; nothing names
  // it: `HTTP.POST`.
  for (const [headers, name, data] of [
    [[['_scxmleventname', 'from.header']], 'from.header', '"plain text"'],
    [{}, 'HTTP.POST', '"plain text"'],
  ]) {
    const opened = await open(receiver, { http: { allow: () => false, location: LOCATION } });
    opened.session.start();
    assert.equal(opened.session.receiveHttp({ method: 'POST', headers, body: 'plain text' }).status, 200);
    assert.deepEqual(opened.logs.slice(0, 1).concat(opened.logs.slice(3, 4)), [`name: ${name}`, `data: ${data}`]);
    opened.session.dispose();
  }
});

test('receiveHttp answers the component status for rejected, misaddressed, and early requests', async () => {
  const { session, logs } = await open(receiver, { http: { allow: () => false, location: LOCATION } });
  // Not running yet: no live session answers.
  assert.equal(session.receiveHttp({ method: 'POST', body: '' }).status, 404);
  session.start();
  assert.deepEqual(session.receiveHttp({ method: 'GET' }), { status: 405, headers: { Allow: 'POST' }, body: '' });
  assert.equal(session.receiveHttp({ method: 'POST', headers: { 'content-type': FORM }, body: 'a=%zz' }).status, 400);
  assert.equal(session.receiveHttp({ method: 'POST', body: 'x'.repeat(65_537) }).status, 413);
  assert.equal(session.receiveHttp({ method: 'POST', url: 'https://example.test/other', body: '' }).status, 404);
  assert.deepEqual(logs, []);
  assert.deepEqual(session.configuration(), ['a']);
  for (const malformed of [undefined, {}, { method: 1 }, { method: 'POST', body: 1 }, { method: 'POST', headers: 'x' }]) {
    assert.throws(() => session.receiveHttp(malformed), hostError('invalid-argument'));
  }
  session.dispose();
  const unconfigured = await open(receiver);
  unconfigured.session.start();
  assert.throws(() => unconfigured.session.receiveHttp({ method: 'POST' }), hostError('invalid-argument'));
  unconfigured.session.dispose();
});

test("a location that throws fails the call that fills the session's _ioprocessors with platform", async () => {
  const { session } = await open(receiver, {
    http: {
      allow: () => false,
      location: () => {
        throw new Error('location unavailable');
      },
    },
  });
  assert.throws(() => session.start(), sessionError('platform'));
  session.dispose();
});

test('a location that throws makes receiveHttp throw invalid-argument for that request only', async () => {
  let broken = false;
  const { session, logs } = await open(receiver, {
    http: {
      allow: () => false,
      location: () => {
        if (broken) throw new Error('location unavailable');
        return LOCATION;
      },
    },
  });
  session.start();
  const request = { method: 'POST', headers: { _scxmleventname: 'hello' }, body: '' };
  broken = true;
  assert.throws(() => session.receiveHttp(request), hostError('invalid-argument'));
  assert.deepEqual(session.configuration(), ['a'], 'the request delivered nothing');
  broken = false;
  assert.equal(session.receiveHttp(request).status, 200);
  assert.deepEqual(session.configuration(), ['got']);
  assert.deepEqual(logs.slice(0, 1), ['name: hello']);
  session.dispose();
});

test('the in-process loopback: a send to the session itself is enqueued before later sends, as under Wasmtime', async () => {
  let session;
  const opened = await open(
    scxml(`<state id="a">
       <onentry>
         <send type="${BASICHTTP}" targetexpr="_ioprocessors['basichttp']['location']" event="event1"/>
         <send event="timeout"/>
       </onentry>
       <transition event="event1" target="pass"><log label="raw" expr="_event.raw"/></transition>
       <transition event="*" target="fail"/>
     </state>
     <final id="pass"/><final id="fail"/>`),
    { http: { allow: (url) => url === LOCATION, location: LOCATION, send: (request) => session.receiveHttp(request) } },
  );
  session = opened.session;
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.deepEqual(opened.logs, [`raw: POST ${LOCATION} HTTP/1.1\r\nContent-Type: ${FORM}\r\n\r\n_scxmleventname=event1`]);
  session.dispose();
});

test('the default loopback location is delivered in-process, never through http.allow or http.send, as under Wasmtime', async () => {
  const { requests, send } = recorder();
  const asked = [];
  const { session, logs } = await open(
    scxml(`<state id="a">
       <onentry>
         <send type="${BASICHTTP}" targetexpr="_ioprocessors['basichttp']['location']" event="event1"/>
         <send event="timeout"/>
       </onentry>
       <transition event="event1" target="b"><log label="origintype" expr="_event.origintype"/></transition>
       <transition event="*" target="fail"/>
     </state>
     <state id="b">
       <onentry><send type="${BASICHTTP}" target="http://basichttp.loopback.invalid/sessions/nobody/basichttp" event="lost"/></onentry>
       <transition event="error.communication" target="pass"/>
       <transition event="*" target="fail"/>
     </state>
     <final id="pass"/><final id="fail"/>`),
    { http: { allow: (url) => asked.push(url) > 0 && false, send } },
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.deepEqual(logs, [`origintype: ${BASICHTTP}`]);
  assert.deepEqual(asked, []);
  assert.deepEqual(requests, []);
  session.dispose();
});

test('a loopback target matches a location in its canonical form, as under Wasmtime', async () => {
  // The default port, letter case in the scheme and host, and a `.`
  // segment all address the session itself (the Wasmtime host compares the
  // same WHATWG canonical form).
  for (const target of [
    'http://basichttp.loopback.invalid:80/sessions/machine-session/basichttp',
    'http://basichttp.loopback.invalid/sessions/./machine-session/basichttp',
    'HTTP://BASICHTTP.LOOPBACK.INVALID/sessions/machine-session/basichttp',
  ]) {
    const { requests, send } = recorder();
    const { session } = await open(
      scxml(`<state id="a">
         <onentry><send type="${BASICHTTP}" target="${target}" event="self"/></onentry>
         <transition event="self" target="pass"/>
         <transition event="*" target="fail"/>
       </state>
       <final id="pass"/><final id="fail"/>`),
      { http: { allow: () => false, send } },
    );
    assert.deepEqual(session.start().activeConfiguration, ['pass'], target);
    assert.deepEqual(requests, []);
    session.dispose();
  }
});

test('loopback at the default locations: a parent reaches a live child and the child its parent', async () => {
  const { requests, send } = recorder();
  const { session } = await open(
    invoking(
      `<state id="c">
         <onentry><send target="#_parent" event="ready"><param name="loc" expr="_ioprocessors['basichttp'].location"/></send></onentry>
         <transition event="ping" target="got"><send type="${BASICHTTP}" event="pong" targetexpr="parentLoc"/></transition>
         <transition event="error.communication"><send target="#_parent" event="childfailed"/></transition>
       </state>
       <final id="got"/>`,
      `<transition event="ready"><send type="${BASICHTTP}" event="ping" targetexpr="_event.data.loc"/></transition>
       <transition event="pong" target="pass"/>`,
    ),
    { http: { allow: () => false, send } },
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.deepEqual(requests, []);
  session.dispose();
});

// --- the loopback across invoked sessions (as the Wasmtime loopback: the
// sending session, its parent, or one of its live children) ------------------

/** Every session's location under `https://example.test/s/<session id>`. */
const sessionLocation = (id) => `https://example.test/s/${encodeURIComponent(id)}`;

/** Opens `source` with the page routing every BasicHTTP send back into the session. */
async function openLoopback(source) {
  let session;
  const opened = await open(source, {
    http: {
      allow: (url) => url.startsWith('https://example.test/s/'),
      location: sessionLocation,
      send: (request) => session.receiveHttp(request),
    },
  });
  session = opened.session;
  return opened;
}

/** An invoked child with `body`; it reports `error.communication` to its parent as `childfailed`. */
const invoking = (child, extra = '') =>
  scxml(`<datamodel><data id="childLoc"/></datamodel>
   <state id="a">
     <invoke id="kid">
       <param name="parentLoc" expr="_ioprocessors['basichttp'].location"/>
       <content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c">
         <datamodel><data id="parentLoc"/></datamodel>
         ${child}
       </scxml></content>
     </invoke>
     ${extra}
     <transition event="childfailed" target="fail"/>
     <transition event="error.*" target="fail"/>
   </state>
   <final id="pass"/>
   <final id="fail"/>`);

test('loopback: a parent reaches a live child at its advertised location', async () => {
  const { session } = await openLoopback(
    invoking(
      `<state id="c">
         <onentry><send target="#_parent" event="ready"><param name="loc" expr="_ioprocessors['basichttp'].location"/></send></onentry>
         <transition event="ping" target="got"/>
         <transition event="error.communication"><send target="#_parent" event="childfailed"/></transition>
       </state>
       <final id="got"/>`,
      `<transition event="ready"><send type="${BASICHTTP}" event="ping" targetexpr="_event.data.loc"/></transition>
       <transition event="done.invoke.kid" target="pass"/>`,
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test('loopback: a child reaches its parent at the parent location', async () => {
  const { session } = await openLoopback(
    invoking(
      `<state id="c">
         <onentry><send type="${BASICHTTP}" event="hello" targetexpr="parentLoc"/></onentry>
         <transition event="error.communication"><send target="#_parent" event="childfailed"/></transition>
       </state>`,
      `<transition event="hello" target="pass"/>`,
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test("loopback: a child's delayed send reaches its parent when due", async () => {
  // SCXML 6.2: "It MUST dispatch the message only when the delay interval
  // elapses."
  const { session } = await openLoopback(
    invoking(
      `<state id="c">
         <onentry><send type="${BASICHTTP}" event="hello" targetexpr="parentLoc" delay="20ms"/></onentry>
         <transition event="error.communication"><send target="#_parent" event="childfailed"/></transition>
       </state>`,
      `<transition event="hello" target="pass"/>`,
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  session.advanceTime(19);
  assert.deepEqual(session.configuration(), ['a']);
  session.advanceTime(1);
  assert.deepEqual(session.configuration(), ['pass']);
  session.dispose();
});

test('loopback: a child reaches itself at its own location', async () => {
  const { session } = await openLoopback(
    invoking(
      `<state id="c">
         <onentry><send type="${BASICHTTP}" event="self" targetexpr="_ioprocessors['basichttp'].location"/></onentry>
         <transition event="self" target="got"/>
         <transition event="error.communication"><send target="#_parent" event="childfailed"/></transition>
       </state>
       <final id="got"/>`,
      `<transition event="done.invoke.kid" target="pass"/>`,
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test('receiveHttp routes an external request to a live child at its location', async () => {
  const { session, logs } = await openLoopback(
    invoking(
      `<state id="c">
         <onentry><send target="#_parent" event="ready"><param name="loc" expr="_ioprocessors['basichttp'].location"/></send></onentry>
         <transition event="poke" target="got"/>
       </state>
       <final id="got"/>`,
      `<transition event="ready"><log label="child" expr="_event.data.loc"/></transition>
       <transition event="done.invoke.kid" target="pass"/>`,
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  const childLocation = logs.find((line) => line.startsWith('child: '))?.slice('child: '.length);
  assert.ok(childLocation?.startsWith('https://example.test/s/'), String(childLocation));
  const response = session.receiveHttp({ method: 'POST', url: childLocation, headers: { 'Content-Type': FORM }, body: '_scxmleventname=poke' });
  assert.equal(response.status, 200);
  assert.deepEqual(session.configuration(), ['pass']);
  // The child finished: its location no longer answers.
  assert.equal(session.receiveHttp({ method: 'POST', url: childLocation, headers: { 'Content-Type': FORM }, body: '_scxmleventname=poke' }).status, 404);
  session.dispose();
});

// SCXML C.2.3: the BasicHTTP `_ioprocessors` entry of every session, an
// invoked child's included, "MUST maintain a 'location' field".
test("an invoked session's BasicHTTP entry has a location under every location form", async () => {
  const doc = scxml(`<state id="a">
     <onentry><log label="root" expr="_ioprocessors['basichttp'].location"/></onentry>
     <invoke id="kid">
       <content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript">
         <final id="c"><onentry><send target="#_parent" event="where">
           <param name="text" expr="_sessionid + ' ' + _ioprocessors['basichttp'].location"/>
         </send></onentry></final>
       </scxml></content>
     </invoke>
     <transition event="where"><log label="child" expr="_event.data.text"/></transition>
   </state>`);
  for (const location of [LOCATION, (id) => (id.includes('.') ? undefined : LOCATION), undefined]) {
    const { session, logs } = await open(doc, { http: { allow: () => false, location } });
    session.start();
    const root = logs.find((line) => line.startsWith('root: '));
    assert.equal(root, `root: ${location === undefined ? 'http://basichttp.loopback.invalid/sessions/machine-session/basichttp' : LOCATION}`);
    const child = logs.find((line) => line.startsWith('child: '));
    assert.ok(child !== undefined, JSON.stringify(logs));
    const [id, advertised] = child.slice('child: '.length).split(' ');
    assert.equal(advertised, `http://basichttp.loopback.invalid/sessions/${id}/basichttp`);
    session.dispose();
  }
});

test('receiveHttp routes an external request to a live session at any depth, as under Wasmtime', async () => {
  // The grandchild reports its location through the child to the root;
  // an external request to that location reaches the grandchild, whose
  // completion finishes the child and then the root.
  const { session, logs } = await openLoopback(
    invoking(
      `<state id="c">
         <invoke id="grand">
           <content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="g">
             <state id="g">
               <onentry><send target="#_parent" event="ready"><param name="loc" expr="_ioprocessors['basichttp'].location"/></send></onentry>
               <transition event="poke" target="poked"/>
             </state>
             <final id="poked"/>
           </scxml></content>
         </invoke>
         <transition event="ready"><send target="#_parent" event="ready"><param name="loc" expr="_event.data.loc"/></send></transition>
         <transition event="done.invoke.grand" target="got"/>
       </state>
       <final id="got"/>`,
      `<transition event="ready"><log label="grandchild" expr="_event.data.loc"/></transition>
       <transition event="done.invoke.kid" target="pass"/>`,
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  const location = logs.find((line) => line.startsWith('grandchild: '))?.slice('grandchild: '.length);
  assert.ok(location?.startsWith('https://example.test/s/'), String(location));
  const poke = { method: 'POST', url: location, headers: { 'Content-Type': FORM }, body: '_scxmleventname=poke' };
  assert.equal(session.receiveHttp(poke).status, 200);
  assert.deepEqual(session.configuration(), ['pass']);
  // The grandchild finished: its location no longer answers.
  assert.equal(session.receiveHttp(poke).status, 404);
  session.dispose();
});

test('receiveHttp finds an invoked session by its location without asking http.location again for every request', async () => {
  let invokedReads = 0;
  let session;
  const opened = await open(
    invoking(
      `<state id="c"><transition event="poke" target="got"/></state>
       <final id="got"/>`,
      `<transition event="done.invoke.kid" target="pass"/>`,
    ),
    {
      http: {
        allow: () => false,
        location: (id) => {
          if (id.includes('.')) invokedReads += 1;
          return sessionLocation(id);
        },
        send: (request) => session.receiveHttp(request),
      },
    },
  );
  session = opened.session;
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  const reads = invokedReads;
  const elsewhere = { method: 'POST', url: 'https://example.test/s/nobody', headers: { 'Content-Type': FORM }, body: '_scxmleventname=poke' };
  for (let n = 0; n < 10; n += 1) assert.equal(session.receiveHttp(elsewhere).status, 404);
  assert.equal(invokedReads, reads, 'no request reads an invoked session location again');
  const kid = { ...elsewhere, url: sessionLocation('machine-session.1') };
  assert.equal(session.receiveHttp(kid).status, 200);
  assert.deepEqual(session.configuration(), ['pass']);
  assert.equal(invokedReads, reads);
  session.dispose();
});

test('an invoked session whose location read failed when it started is found once a later read succeeds', async () => {
  // The host reads a started invoked session's location to route external
  // requests to it. The page's http.location throws for that read once:
  // the failure is recorded, and the next request reads it again.
  const { telemetry, spans } = recordingTelemetry();
  let kidReads = 0;
  let session;
  const opened = await open(
    invoking(
      `<state id="c"><transition event="poke" target="got"/></state>
       <final id="got"/>`,
      `<transition event="done.invoke.kid" target="pass"/>`,
    ),
    {
      telemetry,
      http: {
        allow: () => false,
        location: (id) => {
          if (id === 'machine-session.1') {
            kidReads += 1;
            // The child's own `_ioprocessors` read is the first; the host's
            // read for routing, when the child has started, the second.
            if (kidReads === 2) throw new Error('location unavailable');
          }
          return sessionLocation(id);
        },
        send: (request) => session.receiveHttp(request),
      },
    },
  );
  session = opened.session;
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  const failed = spans.filter(
    (span) => span.attributes['scxml.operation'] === 'capability' && span.attributes['scxml.outcome'] === 'failed',
  );
  assert.equal(failed.length, 1, 'the failed read is recorded');
  const poke = {
    method: 'POST',
    url: sessionLocation('machine-session.1'),
    headers: { 'Content-Type': FORM },
    body: '_scxmleventname=poke',
  };
  assert.equal(session.receiveHttp(poke).status, 200);
  assert.deepEqual(session.configuration(), ['pass']);
  session.dispose();
});

test('a child location read that keeps failing is retried at most once a turn and recorded once', async () => {
  // 100 waiting children whose location the page cannot answer (every
  // read throws; they never read their own `_ioprocessors`). Each `go` is
  // one turn that raises 100 events, each of which sends a BasicHTTP event,
  // which the page routes back into the session, to a location no session
  // has (a failed send ends only its own block): every send looks the
  // target up (Run.sessionAt), but each failed child location is read
  // again once that turn, not once per lookup, and each child's failure is
  // one failed `capability` span, not one per retry.
  const CHILDREN = 100;
  const { telemetry, spans } = recordingTelemetry();
  const reads = new Map();
  const invokes = Array.from(
    { length: CHILDREN },
    (_, index) => `<invoke id="k${index}"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="w"><state id="w"/></scxml></content></invoke>`,
  ).join('');
  const raises = '<raise event="r"/>'.repeat(CHILDREN);
  const send = `<send type="${BASICHTTP}" event="x" target="https://example.test/s/nobody"/>`;
  let session;
  let lookups = 0;
  const opened = await open(
    scxml(`<state id="a">${invokes}<transition event="go">${raises}</transition><transition event="r">${send}</transition></state>`),
    {
      telemetry,
      http: {
        allow: (url) => url.startsWith('https://example.test/s/'),
        send: (request) => {
          lookups += 1;
          return session.receiveHttp(request);
        },
        location: (id) => {
          reads.set(id, (reads.get(id) ?? 0) + 1);
          if (id !== 'machine-session') throw new Error('location unavailable');
          return sessionLocation(id);
        },
      },
    },
  );
  session = opened.session;
  const childReads = () => [...reads].filter(([id]) => id !== 'machine-session').reduce((sum, [, count]) => sum + count, 0);
  session.start();
  assert.equal(childReads(), CHILDREN, "the host's routing read when each child starts");
  assert.equal(failedCapabilitySpans(spans), CHILDREN);
  for (const turn of [1, 2]) {
    session.send({ name: 'go' });
    assert.equal(lookups, turn * CHILDREN, `turn ${turn}: every send looked its target up`);
    assert.equal(childReads(), (1 + turn) * CHILDREN, `turn ${turn}: one more read per child`);
    assert.equal(failedCapabilitySpans(spans), CHILDREN, `turn ${turn}: each child recorded once`);
  }
  assert.deepEqual(session.configuration(), ['a']);
  session.dispose();
});

test("a loopback read of the invoking session's location that throws is recorded once and made once a turn", async () => {
  // The child routes its BasicHTTP sends through the page back into the
  // session. Once `go` arrives, the second read of the root's location in
  // each inbound request throws: receiveHttp's own read (to route the
  // request) succeeds, and the loopback's read of the invoking session's
  // location (to see whether the request addresses it) throws. The child's
  // three sends (one for each poke) find no session; after the loopback
  // read fails it is not made again that turn, and the root's failure is
  // one failed `capability` span however often it fails.
  const { telemetry, spans } = recordingTelemetry();
  let rootBroken = false;
  let rootReads = 0;
  let readsInRequest = 0;
  let session;
  const opened = await open(
    invoking(
      `<state id="c"><transition event="poke"><send type="${BASICHTTP}" event="x" target="https://example.test/s/nobody"/></transition></state>`,
      `<transition event="go">${'<send target="#_kid" event="poke"/>'.repeat(3)}</transition>`,
    ),
    {
      telemetry,
      http: {
        allow: (url) => url.startsWith('https://example.test/s/'),
        send: (request) => {
          readsInRequest = 0;
          return session.receiveHttp(request);
        },
        location: (id) => {
          if (id === 'machine-session' && rootBroken) {
            rootReads += 1;
            readsInRequest += 1;
            if (readsInRequest === 2) throw new Error('location unavailable');
          }
          return sessionLocation(id);
        },
      },
    },
  );
  session = opened.session;
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  assert.equal(failedCapabilitySpans(spans), 0);
  rootBroken = true;
  session.send('go');
  assert.equal(rootReads, 4, "each request's routing read, and one loopback read this turn");
  assert.equal(failedCapabilitySpans(spans), 1);
  session.send('go');
  assert.equal(rootReads, 8, 'the loopback reads again once in the next turn');
  assert.equal(failedCapabilitySpans(spans), 1, 'recorded once per session');
  assert.deepEqual(session.configuration(), ['a']);
  session.dispose();
});

test("an invoked session whose location read throws is recorded once and read once a turn by every path", async () => {
  // The child's location read throws after its `_ioprocessors` is filled
  // in: when it starts (the host's routing index), and when the loopback
  // looks up the sender of each of its three BasicHTTP sends a turn. Its failure is one failed
  // `capability` span for the whole session, and its location is read at
  // most once a turn, whichever path reads it.
  const { telemetry, spans } = recordingTelemetry();
  let childReads = 0;
  let session;
  const opened = await open(
    invoking(
      `<state id="c"><transition event="poke"><send type="${BASICHTTP}" event="x" target="https://example.test/s/nobody"/></transition></state>`,
      `<transition event="go">${'<send target="#_kid" event="poke"/>'.repeat(3)}</transition>`,
    ),
    {
      telemetry,
      http: {
        allow: (url) => url.startsWith('https://example.test/s/'),
        send: (request) => session.receiveHttp(request),
        location: (id) => {
          if (id === 'machine-session') return sessionLocation(id);
          childReads += 1;
          if (childReads === 1) return sessionLocation(id);
          throw new Error('location unavailable');
        },
      },
    },
  );
  session = opened.session;
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  assert.equal(childReads, 2, 'its `_ioprocessors`, then the routing index');
  assert.equal(failedCapabilitySpans(spans), 1);
  for (const turn of [1, 2]) {
    session.send('go');
    assert.equal(childReads, 2 + turn, `turn ${turn}: one more read`);
    assert.equal(failedCapabilitySpans(spans), 1, `turn ${turn}: recorded once per session`);
  }
  assert.deepEqual(session.configuration(), ['a']);
  session.dispose();
});

/** The failed `capability` spans recorded so far. */
const failedCapabilitySpans = (spans) =>
  spans.filter((span) => span.attributes['scxml.operation'] === 'capability' && span.attributes['scxml.outcome'] === 'failed')
    .length;

test('receiveHttp during another session operation is a reentrant call', async () => {
  let session;
  let thrown;
  const opened = await open(scxml(`<state id="a"><onentry><log expr="'go'"/></onentry></state>`), {
    http: { allow: () => false, location: LOCATION },
    capabilities: {
      log: () => {
        try {
          session.receiveHttp({ method: 'POST' });
        } catch (error) {
          thrown = error;
        }
      },
    },
  });
  session = opened.session;
  session.start();
  assert.ok(hostError('reentrant-call')(thrown));
  session.dispose();
});

test('_ioprocessors advertises BasicHTTP only with http, always with a location (SCXML C.2.3)', async () => {
  const doc = scxml(`<state id="a"><onentry><log label="io" expr="JSON.stringify(_ioprocessors)"/></onentry></state>`);
  const cases = [
    [undefined, undefined],
    [{ allow: () => false }, { location: 'http://basichttp.loopback.invalid/sessions/machine-session/basichttp' }],
    [{ allow: () => false, location: LOCATION }, { location: LOCATION }],
    [{ allow: () => false, location: (id) => `https://example.test/${id}` }, { location: 'https://example.test/machine-session' }],
  ];
  for (const [http, entry] of cases) {
    const { session, logs } = await open(doc, http === undefined ? {} : { http });
    session.start();
    const processors = JSON.parse(logs[0].slice('io: '.length));
    const scxmlKey = 'http://www.w3.org/TR/scxml/#SCXMLEventProcessor';
    assert.ok(Object.hasOwn(processors, scxmlKey));
    if (entry === undefined) {
      assert.deepEqual(Object.keys(processors).sort(), ['http://www.w3.org/TR/scxml/#SCXMLEventProcessor', 'scxml']);
    } else {
      assert.deepEqual(processors[BASICHTTP], entry);
      assert.deepEqual(processors.basichttp, entry);
      assert.deepEqual(Object.keys(processors), Object.keys(processors).sort());
    }
    session.dispose();
  }
  for (const http of [null, {}, { allow: true }, { allow: () => true, send: 'fetch' }, { allow: () => true, location: 1 }]) {
    await assert.rejects(open(doc, { http }), hostError('invalid-argument'));
  }
});


// Only HTTP optional white space (SP and HTAB, RFC 9110 section 5.5) pads a
// trace header, as the Wasmtime host reads it (loopback.rs
// `a_trace_header_is_padded_by_http_white_space_only`): U+FEFF, U+0085, and
// U+00A0 are part of the value, which is then invalid.
test('a trace header is padded by HTTP white space only', () => {
  const traceparent = '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01';
  const tracestate = 'vendor=value';
  const read = (parent, state) => {
    const context = inboundTraceContext([
      { name: 'traceparent', value: parent },
      { name: 'tracestate', value: state },
    ]);
    return context === undefined ? undefined : [context.traceparent, context.tracestate];
  };
  assert.deepEqual(read(` \t${traceparent}\t `, `\t${tracestate} `), [traceparent, tracestate]);
  for (const padding of ['\uFEFF', '\u0085', '\u00A0']) {
    assert.equal(read(`${padding}${traceparent}`, tracestate), undefined, JSON.stringify(padding));
    assert.equal(read(`${traceparent}${padding}`, tracestate), undefined, JSON.stringify(padding));
    assert.deepEqual(read(traceparent, `${padding}${tracestate}`), [traceparent, undefined], JSON.stringify(padding));
  }
});
