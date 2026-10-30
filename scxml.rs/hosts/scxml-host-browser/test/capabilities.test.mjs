// Contract tests of the browser host's machine capabilities: data binding
// and every forwarded data-model function on the bundled ECMAScript, XPath,
// and null data models; the linked SCXML Event I/O Processor and clock
// (immediate, delayed, and cancelled sends, unreachable targets); the
// deliberately unsupported capabilities (invocation, unsupplied documents),
// each a typed error; and the command budget and trace. BasicHTTP has its
// own suite (test/basichttp.test.mjs).
//
// Runs in Node on the same ES modules and metered core Wasm a browser loads.
// Documents are compiled in-page with the bundled compiler.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  CapabilityError,
  ScxmlHostError,
  ScxmlSessionError,
  UNSUPPORTED,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createNullDataModel,
  createSession,
  createXPathDataModel,
} from '../src/index.js';
import {
  InvocationCompiler,
  MAX_CACHED_INVOCATIONS,
  staticInvocations,
} from '../src/invoke.js';
import { InvokedSessions, InvokeViolation, ROOT, byCodePoint } from '../src/invoked-sessions.js';
import { IoLink } from '../src/io.js';
import { callDeferred, requestCancel, requestStart } from '../src/machine.js';
import { Telemetry } from '../src/telemetry.js';
import { recordingTelemetry } from './recording-telemetry.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Counts the calls of `prototype[name]` until the returned `stop` restores
 * it and returns the count: a deterministic measure of work.
 */
function countCalls(prototype, name) {
  const original = prototype[name];
  let count = 0;
  prototype[name] = function counted(...args) {
    count += 1;
    return original.apply(this, args);
  };
  return () => {
    prototype[name] = original;
    return count;
  };
}

const loadCoreModule = cachedCoreModules((component, file) =>
  readFile(join(packageRoot, 'dist', component, file)),
);
const compiler = await createCompiler({ loadCoreModule });

async function compile(source) {
  const result = await compiler.compile(source, { sourceName: 'capability.scxml' });
  assert.equal(result.status, 'emitted', new TextDecoder().decode(result.report));
  return result.ir;
}

const document = (datamodel, body, attributes = '') =>
  `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="${datamodel}"${attributes}>${body}</scxml>`;

const FACTORIES = {
  ecmascript: createEcmaScriptDataModel,
  xpath: createXPathDataModel,
  null: createNullDataModel,
};

/**
 * Opens a manual-timer session over `source` on the bundled data model it
 * names, recording `<log>` lines.
 */
async function open(datamodel, source, options = {}) {
  const logs = [];
  const { capabilities, ...rest } = options;
  const session = await createSession({
    ir: await compile(source),
    dataModel: datamodel === undefined ? undefined : await FACTORIES[datamodel]({ loadCoreModule }),
    capabilities: { log: ({ label, text }) => logs.push(label === undefined ? text : `${label}: ${text}`), ...capabilities },
    timers: 'manual',
    loadCoreModule,
    ...rest,
  });
  return { session, logs };
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

// --- data models ---------------------------------------------------------------

test('an ECMAScript data-model document binds data and runs every forwarded evaluation', async () => {
  const { session, logs } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<datamodel>
         <data id="count" expr="1"/>
         <data id="items" expr="[1, 2, 3]"/>
         <data id="inline">{"a": 7}</data>
         <data id="broken" expr="undefined_name.x"/>
       </datamodel>
       <state id="a">
         <onentry>
           <foreach array="items" item="item" index="i"><assign location="count" expr="count + item"/></foreach>
           <send event="go" namelist="count"><param name="extra" expr="inline.a"/></send>
         </onentry>
         <transition event="error.execution" target="failed"/>
         <transition event="go" cond="_event.data.count === 7 &amp;&amp; _event.data.extra === 7" target="b"/>
       </state>
       <state id="b">
         <onentry><log label="Outcome" expr="typeof broken === 'undefined' ? 'pass' : 'fail'"/></onentry>
       </state>
       <final id="failed"/>`,
      ' initial="a"',
    ),
  );
  const started = session.start();
  // `broken`'s initializer queued error.execution at binding (SCXML 5.3),
  // ahead of every other event.
  assert.deepEqual(started.activeConfiguration, ['failed']);
  session.dispose();

  const { session: clean, logs: cleanLogs } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<datamodel>
         <data id="count" expr="1"/>
         <data id="items" expr="[1, 2, 3]"/>
         <data id="inline">{"a": 7}</data>
       </datamodel>
       <state id="a">
         <onentry>
           <foreach array="items" item="item" index="i"><assign location="count" expr="count + item"/></foreach>
           <send event="go" namelist="count"><param name="extra" expr="inline.a"/></send>
         </onentry>
         <transition event="go" cond="_event.data.count === 7 &amp;&amp; _event.data.extra === 7" target="b"/>
         <transition event="*" target="failed"/>
       </state>
       <state id="b"><onentry><log label="Outcome" expr="'pass'"/></onentry></state>
       <final id="failed"/>`,
      ' initial="a"',
    ),
  );
  assert.deepEqual(clean.start().activeConfiguration, ['b']);
  assert.deepEqual(cleanLogs, ['Outcome: pass']);
  assert.deepEqual(logs, []);
  clean.dispose();
});

test('an XPath data-model document binds data and runs its evaluations', async () => {
  const { session, logs } = await open(
    'xpath',
    document(
      'xpath',
      `<datamodel>
         <data id="count" expr="1"/>
         <data id="nodes"><node xmlns="">1</node><node xmlns="">2</node></data>
       </datamodel>
       <state id="a">
         <onentry>
           <foreach array="$nodes/*" item="node"><assign location="$count" expr="$count/text() + $node/text()"/></foreach>
           <send event="go" namelist="$count"/>
         </onentry>
         <transition event="go" cond="$_event/data/data[@id='count'] = 4 and $node/text() = 2" target="b"/>
         <transition event="*" target="failed"/>
       </state>
       <state id="b"><onentry><log label="Outcome" expr="'pass'"/></onentry></state>
       <final id="failed"/>`,
      ' initial="a"',
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['b']);
  assert.deepEqual(logs, ['Outcome: pass']);
  session.dispose();
});

test('a null data-model document answers In() through the data model', async () => {
  const { session } = await open(
    'null',
    document(
      'null',
      `<parallel id="p">
         <state id="left"><transition event="go" cond="In('right')" target="done"/></state>
         <state id="right"/>
       </parallel>
       <final id="done"/>`,
      ' initial="p"',
    ),
  );
  session.start();
  assert.equal(session.send('go').status, 'finished');
  session.dispose();
});

test('a data-model document without a linked data model fails at start with unavailable', async () => {
  const { session } = await open(undefined, document('ecmascript', '<datamodel><data id="x"/></datamodel><state id="a"/>'));
  assert.throws(() => session.start(), sessionError('unavailable'));
  assert.equal(session.status(), 'failed');
  session.dispose();
});

// --- SCXML Event I/O Processor --------------------------------------------------

const DELAYED = document(
  'ecmascript',
  `<state id="a">
     <onentry>
       <send event="late" delay="1s" id="late"/>
       <send event="cancelled" delayexpr="'500ms'" id="doomed"/>
       <cancel sendid="doomed"/>
     </onentry>
     <transition event="late" target="b"/>
     <transition event="cancelled" target="failed"/>
   </state>
   <state id="b"><onentry><log label="Outcome" expr="'pass'"/></onentry></state>
   <final id="failed"/>`,
  ' initial="a"',
);

test('delayed sends are delivered at their deadline and cancelled sends never are', async () => {
  const { session, logs } = await open('ecmascript', DELAYED);
  assert.deepEqual(session.start().activeConfiguration, ['a']);
  assert.equal(session.now(), 0);
  assert.equal(session.nextDeadline(), 1000, 'the cancelled send left the clock');
  assert.deepEqual(session.advanceTime(999), { results: [], complete: true });
  assert.equal(session.now(), 999);
  const { results, complete } = session.advanceTime(1);
  assert.equal(complete, true);
  assert.deepEqual(
    results.map((step) => step.activeConfiguration),
    [['b']],
  );
  assert.deepEqual(logs, ['Outcome: pass']);
  assert.equal(session.nextDeadline(), undefined);
  session.dispose();
});

test('the event loop delivers delayed sends on its own timer', async () => {
  const delivered = [];
  let wake;
  const settled = new Promise((resolveWake) => {
    wake = resolveWake;
  });
  const { session, logs } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <onentry><send event="late" delay="30ms"/></onentry>
         <transition event="late" target="b"/>
       </state>
       <state id="b"><onentry><log label="Outcome" expr="'pass'"/></onentry></state>`,
      ' initial="a"',
    ),
    {
      timers: 'event-loop',
      onTimer: (outcome) => {
        delivered.push(outcome);
        wake();
      },
    },
  );
  const started = performance.now();
  session.start();
  assert.throws(() => session.advanceTime(1), hostError('invalid-argument'));
  await settled;
  assert.ok(performance.now() - started >= 25, 'the timer honored the delay');
  assert.equal(delivered.length, 1);
  assert.equal(delivered[0].complete, true);
  assert.deepEqual(delivered[0].results.map((step) => step.activeConfiguration), [['b']]);
  assert.deepEqual(logs, ['Outcome: pass']);
  session.dispose();
});

test('an unreachable target raises error.communication and an invalid one error.execution', async () => {
  const { session, logs } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <onentry><send event="lost" id="lost" target="#_scxml_nobody"/></onentry>
         <transition event="error.communication" target="b"/>
         <transition event="*" target="failed"/>
       </state>
       <state id="b">
         <onentry>
           <log label="sendid" expr="_event.sendid"/>
           <send event="bad" target="not a target"/>
         </onentry>
         <transition event="error.execution" target="pass"/>
         <transition event="*" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.deepEqual(logs, ['sendid: lost']);
  session.dispose();
});

test('sends to the session itself are stamped by the processor and enqueued externally', async () => {
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <onentry><send event="self" target="#_internal"/><send event="loop"/></onentry>
         <transition event="self" cond="_event.type === 'internal'" target="b"/>
       </state>
       <state id="b">
         <transition event="loop"
           cond="_event.type === 'external' &amp;&amp; _event.origintype === 'http://www.w3.org/TR/scxml/#SCXMLEventProcessor' &amp;&amp; _event.origin === '#_scxml_' + _sessionid"
           target="pass"/>
         <transition event="*" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test('an SCXML message whose data the data model cannot handle raises error.communication and is ignored', async () => {
  // SCXML C.1: "If the receiving Processor cannot handle the data format
  // contained in the message, the receiving Processor MUST place the error
  // error.communication in internal queue of the session for which the
  // message was intended and MUST ignore the message." Every bundled data
  // model handles every portable format (the XPath model through SCXML 1.0
  // CR 2014-03-13 §C.3.7.1's space-normalized string literal), so this
  // page-supplied provider declines JSON arrays through `acceptsEventData`:
  // the array its ECMAScript child sends is ignored and `after` still
  // arrives.
  const base = await createEcmaScriptDataModel({ loadCoreModule });
  const declined = [];
  const dataModel = Object.create(base, {
    acceptsEventData: {
      value(data) {
        const accepted = data.tag !== 'json-array';
        if (!accepted) declined.push(data.val);
        return accepted;
      },
    },
  });
  const { session, logs } = await open(
    undefined,
    document(
      'ecmascript',
      `<state id="a">
         <invoke id="kid"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="k">` +
        `<state id="k"><onentry><send id="arr" event="list" target="#_parent"><content expr="[1, 2]"/></send><send event="after" target="#_parent"/></onentry></state>` +
        `</scxml></content></invoke>
         <transition event="error.communication" target="b"><log label="error" expr="_event.sendid + ':' + _event.type"/></transition>
         <transition event="*" target="failed"><log label="unexpected" expr="_event.name"/></transition>
       </state>
       <state id="b">
         <transition event="after" target="pass"/>
         <transition event="*" target="failed"><log label="unexpected" expr="_event.name"/></transition>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
    { dataModel },
  );
  const started = session.start();
  assert.deepEqual(logs, ['error: arr:platform']);
  assert.deepEqual(declined, ['[1,2]']);
  assert.deepEqual(started.activeConfiguration, ['pass']);
  session.dispose();
});

// --- invocation with pre-lowered child IR ------------------------------------

const PARENT = document(
  'ecmascript',
  `<state id="outer" initial="waiting">
     <invoke id="kid" src="file:child.scxml"><param name="greeting" expr="'hi'"/></invoke>
     <state id="waiting">
       <transition event="fromChild" cond="_event.data.echo === 'hi' &amp;&amp; _event.invokeid === 'kid'" target="childDone"/>
     </state>
     <state id="childDone">
       <transition event="done.invoke.kid" cond="_event.data.result === 42" target="pass"/>
     </state>
     <transition event="*" target="failed"><log label="unexpected" expr="_event.name"/></transition>
   </state>
   <final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
   <final id="failed"/>`,
  ' initial="outer"',
);
const CHILD = document(
  'ecmascript',
  `<datamodel><data id="greeting"/></datamodel>
   <state id="c">
     <onentry><send event="later" delay="100ms"/><log label="child" expr="greeting"/></onentry>
     <transition event="later" target="reply"/>
   </state>
   <state id="reply">
     <onentry><send target="#_parent" event="fromChild"><param name="echo" expr="greeting"/></send></onentry>
     <transition target="end"/>
   </state>
   <final id="end"><donedata><param name="result" expr="42"/></donedata></final>`,
  ' initial="c"',
);

test('<invoke> runs a child from the page\'s pre-lowered IR, through external event queues and the shared clock', async () => {
  const childIr = await compile(CHILD);
  const requests = [];
  const { session, logs } = await open('ecmascript', PARENT, {
    trace: true,
    capabilities: {
      loadDocument: (kind, uri) => {
        requests.push([kind, uri]);
        return CHILD;
      },
      compileInvocation: (sourceName, source) => {
        requests.push(['compile', sourceName]);
        if (source !== CHILD) throw new CapabilityError('invalid-input');
        return childIr;
      },
    },
  });
  assert.deepEqual(session.start().activeConfiguration, ['waiting']);
  assert.deepEqual(requests, [
    ['invocation', 'file:child.scxml'],
    ['compile', 'child.scxml'],
  ]);
  // The child's delayed send is on its own clock, advanced with the root's.
  assert.equal(session.nextDeadline(), 100);
  const { results, complete } = session.advanceTime(100);
  assert.equal(complete, true);
  // `fromChild`, then `done.invoke.kid` after the child's other events.
  assert.deepEqual(
    results.map((step) => step.activeConfiguration),
    [['childDone'], ['pass']],
  );
  assert.equal(session.status(), 'finished');
  assert.deepEqual(logs, ['Outcome: pass'], "only the root's <log> reaches the page");
  const child = session.trace().entries.filter((entry) => entry.parent === 'machine-session');
  assert.ok(child.length > 0, 'child observations are attributed to the child');
  assert.ok(child.every((entry) => entry.session === 'machine-session.kid'));
  assert.deepEqual(
    child.filter((entry) => entry.observation === 'log').map(({ label, text }) => `${label}: ${text}`),
    ['child: hi'],
  );
  session.dispose();
});

test('inline content past the machine 1 MiB bound fails the invocation with error.execution, as under Wasmtime', async () => {
  // Each 600,000-byte text fits the Machine IR string bound; the child
  // document they serialize into does not fit the machine's 1 MiB bound.
  const text = 'x'.repeat(600_000);
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c"><datamodel><data id="d1">${text}</data><data id="d2">${text}</data></datamodel><state id="c"/></scxml></content></invoke>
         <transition event="error.execution" target="pass"/>
         <transition event="*" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test('a child the page does not pre-lower fails the invocation with error.execution', async () => {
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml></content></invoke>
         <transition event="error.execution" target="pass"/>
         <transition event="*" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
    {
      capabilities: {
        compileInvocation: () => {
          throw new CapabilityError('invalid-input');
        },
      },
    },
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test('page child IR that fails Machine IR validation fails the <invoke> with error.execution, as under Wasmtime', async () => {
  // SCXML 3.12.2: the machine decodes and validates the page's IR; an
  // empty <raise> event name fails that invocation only.
  const named = await compile(
    document('ecmascript', '<state id="c"><onentry><raise event="NAME"/></onentry></state>', ' initial="c"'),
  );
  const invalid = new TextEncoder().encode(new TextDecoder().decode(named).replace('"NAME"', '""'));
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <invoke src="file:child.scxml"/>
         <transition event="error.execution" target="pass"/>
         <transition event="*" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
    {
      capabilities: {
        loadDocument: () => 'unused: the page compiles every child',
        compileInvocation: () => invalid,
      },
    },
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  session.dispose();
});

test("a invoked session's work is charged to the invoking command; exhaustion fails the run, as under Wasmtime", async () => {
  const hostileChild = document(
    'ecmascript',
    `<state id="c"><onentry><script>function f(){for(var i=0;i&lt;99999;i++){}} for(var j=0;j&lt;99999;j++) f();</script></onentry></state>`,
    ' initial="c"',
  );
  const childIr = await compile(hostileChild);
  const { session } = await open(
    'ecmascript',
    document('ecmascript', `<state id="a"><invoke src="file:child.scxml"/></state>`, ' initial="a"'),
    {
      fuel: 20_000_000,
      capabilities: { loadDocument: () => hostileChild, compileInvocation: () => childIr },
    },
  );
  // The child's data model spends what is left of the root's `start`
  // budget (the child runs inside it): the child machine then runs out of
  // fuel, and so does the run, as under Wasmtime.
  assert.throws(() => session.start(), hostError('fuel-exhausted'));
  assert.throws(() => session.status(), hostError('instance-lost'));
  session.dispose();
});

test('one host call spends one fuel budget across every child start it carries out, as under Wasmtime', async () => {
  // A child whose start runs a script costing about 0.6 of the budget
  // below: with the root's own start, one child costs about 7 million
  // charges of 11 million.
  const burning = document(
    'ecmascript',
    `<state id="c0"><onentry><script>function burn(){var x=0;for(var i=0;i&lt;50000;i++){x+=i}return x}var y=0;for(var j=0;j&lt;7;j++){y+=burn()}</script></onentry></state>`,
    ' initial="c0"',
  );
  const startWith = async (invokes) => {
    const { session } = await open(
      'ecmascript',
      document('ecmascript', `<state id="r0">${'<invoke src="file:burn.scxml"/>'.repeat(invokes)}</state>`, ' initial="r0"'),
      { fuel: 11_000_000, capabilities: { loadDocument: () => burning } },
    );
    try {
      return session.start().activeConfiguration;
    } finally {
      session.dispose();
    }
  };
  assert.deepEqual(await startWith(1), ['r0']);
  // Each child start is carried out after the root's start suspends, yet
  // both are charged to the root's one `start` budget.
  await assert.rejects(startWith(2), hostError('fuel-exhausted'));
});

// --- automatic child compilation (invoke.compile 'auto', src/invoke.js) --------

const SCXML_NS = 'http://www.w3.org/2005/07/scxml';
/** Host `compile` spans: one per compile the compiler actually ran. */
const compiles = (spans) =>
  spans.filter((span) => span.scope === 'scxml-host-browser' && span.attributes['scxml.operation'] === 'compile').length;
/** A parent whose outcome is `pass` exactly when `done.invoke` arrives before any error. */
const invoking = (invoke) =>
  document(
    'ecmascript',
    `<state id="a">
       ${invoke}
       <transition event="done.invoke" target="pass"/>
       <transition event="*" target="failed"><log label="unexpected" expr="_event.name"/></transition>
     </state>
     <final id="pass"/>
     <final id="failed"/>`,
    ' initial="a"',
  );
/** A parent whose outcome is `pass` exactly when the invocation fails with error.execution. */
const failing = (invoke) =>
  document(
    'ecmascript',
    `<state id="a">
       ${invoke}
       <transition event="error.execution" target="pass"/>
       <transition event="*" target="failed"/>
     </state>
     <final id="pass"/>
     <final id="failed"/>`,
    ' initial="a"',
  );

test("releaseForeach records no telemetry, as the Wasmtime host's untraced forward", async () => {
  const { telemetry, spans, samples } = recordingTelemetry();
  const model = await createEcmaScriptDataModel({ loadCoreModule, telemetry });
  const before = [spans.length, samples.length];
  model.releaseForeach();
  assert.deepEqual([spans.length, samples.length], before);
  // Another pure call is observed, so the recorder sees the data model.
  model.namelistKey('x');
  assert.ok(spans.length > before[0]);
});

test('a session forwards releaseForeach to its data model untraced, as the Wasmtime host does', async () => {
  const source = document(
    'ecmascript',
    `<state id="a"><transition event="go" target="b"><foreach array="[1, 2]" item="i"><log expr="i"/></foreach></transition></state><state id="b"/>`,
    ' initial="a"',
  );
  const ir = await compile(source);
  // The same session twice: once with a provider that counts its
  // releaseForeach calls, once with one that has none to call.
  const capabilitySpans = async (provider) => {
    const { telemetry, spans } = recordingTelemetry();
    const session = await createSession({ ir, dataModel: provider, telemetry, timers: 'manual', loadCoreModule });
    session.start();
    session.send('go');
    session.dispose();
    return spans.filter((span) => span.attributes['scxml.operation'] === 'capability').length;
  };
  const counted = await FACTORIES.ecmascript({ loadCoreModule });
  let releases = 0;
  const withRelease = await capabilitySpans({
    ...counted,
    releaseForeach() {
      releases += 1;
      counted.releaseForeach();
    },
  });
  const withoutRelease = await capabilitySpans({ ...(await FACTORIES.ecmascript({ loadCoreModule })), releaseForeach: undefined });
  assert.ok(releases > 0, 'the machine releases its <foreach> state each command');
  assert.equal(withRelease, withoutRelease, 'no capability span for releaseForeach');
});

test('by default the host compiles an inline <invoke> child itself, pre-warmed before start', async () => {
  const { telemetry, spans } = recordingTelemetry();
  const { session, logs } = await open(
    'ecmascript',
    invoking(
      `<invoke><content><scxml xmlns="${SCXML_NS}" version="1.0" datamodel="ecmascript" name="kid &amp; co">` +
        `<final id="f"><onentry><log label="child" expr="_name"/></onentry></final></scxml></content></invoke>`,
    ),
    { telemetry, trace: true },
  );
  // The pre-warm compiled the child during createSession ...
  assert.equal(compiles(spans), 1);
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  // ... with the exact request the machine makes, so start compiled nothing.
  assert.equal(compiles(spans), 1);
  assert.deepEqual(logs, []);
  const child = session.trace().entries.filter((entry) => entry.parent === 'machine-session' && entry.observation === 'log');
  assert.deepEqual(child.map(({ label, text }) => `${label}: ${text}`), ['child: kid & co']);
  session.dispose();
});

test('the pre-warm reads only statically known SCXML children from the root IR', async () => {
  const ir = await compile(
    document(
      'ecmascript',
      `<state id="a">
         <invoke><content><scxml xmlns="${SCXML_NS}" version="1.0" name="a&quot;b&lt;c"><final id="x">t&amp;u</final></scxml></content></invoke>
         <invoke><content><scxml xmlns="${SCXML_NS}" version="1.0" name="a&quot;b&lt;c"><final id="x">t&amp;u</final></scxml></content></invoke>
         <invoke src="file:child.scxml"/>
         <invoke><content expr="'&lt;scxml/&gt;'"/></invoke>
         <invoke typeexpr="'scxml'"><content><scxml xmlns="${SCXML_NS}" version="1.0"><final id="t"/></scxml></content></invoke>
         <invoke type="http://example.com/other"><content><scxml xmlns="${SCXML_NS}" version="1.0"><final id="o"/></scxml></content></invoke>
       </state>`,
      ' initial="a"',
    ),
  );
  // Duplicates collapse; `src`, `<content expr>`, `typeexpr`, and foreign
  // types are decided at run time only. The text mirrors the machine's
  // serializer (raw text, escaped attributes, restated root namespace).
  assert.deepEqual(staticInvocations(ir), {
    invokes: true,
    documents: [
      ['invoke-content.scxml', `<scxml xmlns="${SCXML_NS}" version="1.0" name="a&quot;b&lt;c"><final id="x">t&u</final></scxml>`],
    ],
  });
  assert.deepEqual(staticInvocations(await compile(document('null', '<final id="f"/>'))), { invokes: false, documents: [] });
  assert.equal(staticInvocations(new TextEncoder().encode('not ir')), undefined);
});

test('the automatic compiler caches outcomes by (sourceName, source)', async () => {
  const { telemetry, spans } = recordingTelemetry();
  const child = document('ecmascript', '<final id="f"/>');
  const loads = [];
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <invoke id="one" src="file:child.scxml"/>
         <transition event="done.invoke.one" target="b"/>
       </state>
       <state id="b">
         <invoke id="two" src="file:child.scxml"/>
         <transition event="done.invoke.two" target="pass"/>
       </state>
       <final id="pass"/>`,
      ' initial="a"',
    ),
    { telemetry, capabilities: { loadDocument: (kind, uri) => (loads.push([kind, uri]), child) } },
  );
  assert.equal(compiles(spans), 0, 'a src child is not known before start');
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.equal(loads.length, 2, 'the machine loads the document for each invocation');
  assert.equal(compiles(spans), 1, 'the second invocation is a cache hit');
  session.dispose();

  // Direct: a hit returns the cached outcome; the cache keeps at most
  // MAX_CACHED_INVOCATIONS entries, evicting the oldest.
  const invocations = await InvocationCompiler.open({ loadCoreModule, hostTelemetry: new Telemetry() });
  const first = invocations.compile('child.scxml', child);
  assert.ok(first.ir instanceof Uint8Array);
  assert.equal(invocations.compile('child.scxml', child), first);
  assert.notEqual(invocations.compile('other.scxml', child), first, 'the source name is part of the key');
  for (let n = 0; n < MAX_CACHED_INVOCATIONS; n += 1) invocations.compile(`c${n}.scxml`, child);
  assert.equal(invocations.size, MAX_CACHED_INVOCATIONS);
  assert.notEqual(invocations.compile('child.scxml', child), first, 'the oldest entry was evicted');
});

test('compile diagnostics and the compiler\'s own source bound fail the invocation with error.execution, as under Wasmtime', async () => {
  // A child the compiler rejects (an unknown initial target). The parent's
  // compile already validates inline children, so it is loaded at run time.
  const rejected = `<scxml xmlns="${SCXML_NS}" version="1.0" initial="nowhere"><final id="f"/></scxml>`;
  const loaded = await open('ecmascript', failing('<invoke src="file:child.scxml"/>'), {
    capabilities: { loadDocument: () => rejected },
  });
  assert.deepEqual(loaded.session.start().activeConfiguration, ['pass']);
  loaded.session.dispose();

  const invocations = await InvocationCompiler.open({ loadCoreModule, hostTelemetry: new Telemetry() });
  assert.deepEqual(invocations.compile('child.scxml', rejected), { rejected: true });
  // The host caps no source size: a child of more than 4 MiB compiles, and
  // only the compiler component's own bound (8 MiB of XML) rejects one.
  const large = `<!--${'x'.repeat(4 * 1024 * 1024)}-->${document('null', '<final id="f"/>')}`;
  assert.ok(invocations.compile('large.scxml', large).ir instanceof Uint8Array);
  const oversized = `<!--${'x'.repeat(8 * 1024 * 1024)}-->${document('null', '<final id="f"/>')}`;
  assert.deepEqual(invocations.compile('big.scxml', oversized), { rejected: true });
});

test('each automatic compile runs on its fuel budget; exhaustion fails the run, as under Wasmtime', async () => {
  const child = document('ecmascript', '<final id="f"/>');
  const { session } = await open('ecmascript', failing('<invoke src="file:child.scxml"/>'), {
    invoke: { fuel: 1_000 },
    capabilities: { loadDocument: () => child },
  });
  assert.throws(() => session.start(), hostError('fuel-exhausted'));
  assert.throws(() => session.status(), hostError('instance-lost'));
  session.dispose();
  // An exhausted pre-warm is cached and fails the run the same way at start.
  const inline = await open('ecmascript', failing(`<invoke><content>${child}</content></invoke>`), {
    invoke: { fuel: 1_000 },
  });
  assert.throws(() => inline.session.start(), hostError('fuel-exhausted'));
  inline.session.dispose();
});

test('a page compileInvocation overrides the automatic compiler', async () => {
  const { telemetry, spans } = recordingTelemetry();
  const child = `<scxml xmlns="${SCXML_NS}" version="1.0"><final id="f"/></scxml>`;
  const childIr = await compile(child);
  const asked = [];
  const { session } = await open('ecmascript', invoking(`<invoke><content>${child}</content></invoke>`), {
    telemetry,
    capabilities: {
      compileInvocation: (sourceName, source) => {
        asked.push([sourceName, source]);
        return childIr;
      },
    },
  });
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.deepEqual(asked, [['invoke-content.scxml', `<scxml xmlns="${SCXML_NS}" version="1.0"><final id="f"/></scxml>`]]);
  assert.equal(compiles(spans), 0, 'the host compiled nothing itself');
  session.dispose();
});

test('an invoked session invokes in turn, as under Wasmtime: done.invoke reaches each level', async () => {
  const grandchild = `<scxml xmlns="${SCXML_NS}" version="1.0"><final id="g"/></scxml>`;
  const child = document(
    'ecmascript',
    `<state id="c"><invoke><content>${grandchild}</content></invoke><transition event="done.invoke" target="cdone"/></state><final id="cdone"/>`,
    ' initial="c"',
  );
  const { session } = await open('ecmascript', invoking('<invoke id="kid" src="file:child.scxml"/>'), {
    trace: true,
    capabilities: { loadDocument: () => child },
  });
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  const sessions = new Set(session.trace().entries.map((entry) => entry.session));
  assert.equal(sessions.size, 3, 'the root, its child, and the grandchild each ran');
  session.dispose();
});

test('an invocation beyond the invoked bounds fails with error.execution, as under Wasmtime', async () => {
  const child = `<scxml xmlns="${SCXML_NS}" version="1.0"><state id="idle"/></scxml>`;
  const regions = (count) =>
    Array.from({ length: count }, (_, n) => `<state id="r${n}"><invoke id="k${n}"><content>${child}</content></invoke></state>`).join('');
  const bounded = { maxInvokeDepth: 4, maxInvokedSessions: 2 };
  const within = await open('ecmascript', document('ecmascript', `<parallel id="p">${regions(2)}</parallel>`), { invokeLimits: bounded });
  assert.equal(within.session.start().status, 'running');
  within.session.dispose();
  const beyond = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a"><parallel id="p">${regions(3)}</parallel><transition event="error.execution" target="pass"/></state><final id="pass"/>`,
      ' initial="a"',
    ),
    { invokeLimits: bounded },
  );
  assert.deepEqual(beyond.session.start().activeConfiguration, ['pass']);
  beyond.session.dispose();
  const ir = await compile(document('null', '<final id="f"/>'));
  // A limit is the page's own choice: any non-negative integer, and an
  // absent field sets no limit.
  for (const invokeLimits of [{}, { maxInvokeDepth: 33 }, { maxInvokedSessions: 256 }, { maxInvokeDepth: 0, maxInvokedSessions: Number.MAX_SAFE_INTEGER }]) {
    const accepted = await createSession({ ir, invokeLimits, timers: 'manual', loadCoreModule });
    accepted.dispose();
  }
  // A plain object with only the two known keys: a misspelled key, an
  // array, or another kind of object would silently set no limit.
  const typo = { maxInvokeDeph: 1 };
  const inherited = Object.create({ maxInvokeDepth: 1 });
  for (const invokeLimits of [
    null,
    8,
    { maxInvokeDepth: -1 },
    { maxInvokedSessions: 1.5 },
    { maxInvokeDepth: '3' },
    { maxInvokedSessions: null },
    typo,
    { maxInvokeDepth: 1, extra: true },
    [],
    new Date(0),
    inherited,
    new Map(),
  ]) {
    await assert.rejects(createSession({ ir, invokeLimits, timers: 'manual', loadCoreModule }), hostError('invalid-argument'));
  }
  // An accessor property could answer each read differently: it is
  // rejected without being called.
  let reads = 0;
  const getter = Object.defineProperty({}, 'maxInvokeDepth', {
    get: () => {
      reads += 1;
      return reads === 1 ? 1 : -1;
    },
    enumerable: true,
  });
  await assert.rejects(createSession({ ir, invokeLimits: getter, timers: 'manual', loadCoreModule }), hostError('invalid-argument'));
  assert.equal(reads, 0, 'the getter was never called');
  // Each own property is read once, and the session keeps its own frozen
  // copy: a later change of the page's object changes nothing.
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
  const refusing = await open(
    'ecmascript',
    document('ecmascript', '<state id="a"><invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml></content></invoke><transition event="error.execution" target="refused"/></state><final id="refused"/>', ' initial="a"'),
    { invokeLimits: proxied },
  );
  assert.deepEqual(traps, ['ownKeys', 'descriptor:maxInvokeDepth'], 'one read of each own property, no getter');
  target.maxInvokeDepth = 5;
  assert.deepEqual(refusing.session.start().activeConfiguration, ['refused'], 'the copy taken at creation applies');
  assert.deepEqual(traps, ['ownKeys', 'descriptor:maxInvokeDepth']);
  refusing.session.dispose();
});

/**
 * Invocations nested `depth` levels deep: the root invokes `file:level-1.scxml`,
 * which invokes `file:level-2.scxml`, and so on; the deepest level is final
 * and each level finishes once the one below it is done.
 */
function nestedInvocations(depth) {
  const level = (n) =>
    n === depth
      ? `<scxml xmlns="${SCXML_NS}" version="1.0"><final id="f"/></scxml>`
      : `<scxml xmlns="${SCXML_NS}" version="1.0" initial="w"><state id="w"><invoke src="file:level-${n + 1}.scxml"/><transition event="done.invoke" target="d"/></state><final id="d"/></scxml>`;
  return {
    source: invoking('<invoke src="file:level-1.scxml"/>'),
    loadDocument: (kind, uri) => level(Number(uri.match(/^file:level-(\d+)\.scxml$/)[1])),
  };
}

test('without invokeLimits no invoke bound applies: 9 nested levels and 65 invoked sessions run', async () => {
  // Nine nested levels run.
  const nested = nestedInvocations(9);
  const deep = await open('ecmascript', nested.source, { trace: true, capabilities: { loadDocument: nested.loadDocument } });
  assert.deepEqual(deep.session.start().activeConfiguration, ['pass']);
  assert.equal(new Set(deep.session.trace().entries.map((entry) => entry.session)).size, 10, 'the root and nine nested sessions ran');
  deep.session.dispose();
  // An explicit depth limit still applies: the invocation at depth 9 fails
  // inside the session at depth 8, so no done.invoke reaches the root.
  const limited = await open('ecmascript', nested.source, {
    invokeLimits: { maxInvokeDepth: 8 },
    capabilities: { loadDocument: nested.loadDocument },
  });
  assert.deepEqual(limited.session.start().activeConfiguration, ['a']);
  limited.session.dispose();
  // 65 live invoked sessions run.
  const child = `<scxml xmlns="${SCXML_NS}" version="1.0"><state id="idle"/></scxml>`;
  const regions = Array.from({ length: 65 }, (_, n) => `<state id="r${n}"><invoke id="k${n}"><content>${child}</content></invoke></state>`).join('');
  const wide = await open(
    'ecmascript',
    document('ecmascript', `<state id="a"><parallel id="p">${regions}</parallel><transition event="error.execution" target="failed"/></state><final id="failed"/>`, ' initial="a"'),
    { trace: true },
  );
  const started = wide.session.start();
  assert.equal(started.status, 'running');
  assert.ok(!started.activeConfiguration.includes('failed'), 'no invocation failed');
  wide.session.dispose();
});

// Invocations nested inside their invoking calls overflowed this host's
// stack between 100 and 300 levels; these chains run past that.
test('a chain of invocations runs from the host loop, not nested calls: 300 levels, and self-invocation with no depth limit, as under Wasmtime', async () => {
  // 300 nested levels without invokeLimits (the null data model, so the
  // one host call's fuel budget holds them all): each child starts after
  // its invoking session's call suspended, so the depth takes no stack.
  const depth = 300;
  const level = (n) =>
    n === depth
      ? `<scxml xmlns="${SCXML_NS}" version="1.0" datamodel="null"><final id="f"/></scxml>`
      : `<scxml xmlns="${SCXML_NS}" version="1.0" datamodel="null" initial="w"><state id="w"><invoke id="k" src="file:level-${n + 1}.scxml"/><transition event="done.invoke" target="d"/></state><final id="d"/></scxml>`;
  const deep = await open(
    'null',
    document(
      'null',
      '<state id="a"><invoke id="k" src="file:level-1.scxml"/><transition event="done.invoke" target="pass"/></state><final id="pass"/>',
      ' initial="a"',
    ),
    { trace: true, capabilities: { loadDocument: (kind, uri) => level(Number(uri.match(/^file:level-(\d+)\.scxml$/)[1])) } },
  );
  assert.deepEqual(deep.session.start().activeConfiguration, ['pass']);
  const sessions = new Set(deep.session.trace().entries.map((entry) => entry.session));
  assert.equal(sessions.size, depth + 1, 'the root and every nested session ran');
  // A child is `<parent _sessionid>.<invokeid>` (the same ids Wasmtime
  // gives): the deepest is the root's id and `.k` per level.
  assert.ok(sessions.has(`machine-session${'.k'.repeat(depth)}`), [...sessions].slice(0, 3).join());
  deep.session.dispose();
  // A document that invokes itself until its own session id is 1,020 bytes
  // or longer, with no invokeLimits: SCXML 6.4 sets no depth and the host
  // refuses no <invoke>; each level adds its generated invoke id (`.c.0`)
  // to the session id, never a stack overflow that loses the instance:
  // `machine-session.a.0` (19 bytes), then 251 levels.
  const self = `<scxml xmlns="${SCXML_NS}" version="1.0" datamodel="ecmascript"><state id="c"><transition cond="_sessionid.length &gt; 1019" target="f"/><invoke src="file:child.scxml"/></state><final id="f"/></scxml>`;
  const unbounded = await open('ecmascript', document('ecmascript', '<state id="a"><invoke src="file:child.scxml"/></state>'), {
    trace: true,
    maxCommands: 4000,
    capabilities: { loadDocument: () => self },
  });
  assert.equal(unbounded.session.start().status, 'running');
  const chain = new Set(unbounded.session.trace().entries.map((entry) => entry.session));
  const deepest = `machine-session.a.0${'.c.0'.repeat(251)}`;
  assert.equal(deepest.length, 1023);
  assert.equal(chain.size, 1 + 1 + 251, 'the root and every level');
  assert.ok(chain.has(deepest));
  unbounded.session.dispose();
});

test('a chain with every level queued at once drains in invocation order and cancels, as under Wasmtime', async () => {
  // Every level sends `up` to its invoking session on entry, so the whole
  // chain's sends are queued when the root's start returns from the chain;
  // each is delivered (one command each) and the root's exit cancels the
  // chain. Each level here costs about 70 ms of component instantiation
  // (the same chain 1,000 deep takes about 71 s); the ordering's own cost
  // at 10,000 levels is the InvokedSessions test above.
  const depth = 300;
  const child = `<scxml xmlns="${SCXML_NS}" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="up" target="#_parent"/></onentry><invoke id="k" src="file:child.scxml"/><transition event="up"><log label="up"/></transition></state></scxml>`;
  const { session } = await open(
    'null',
    document(
      'null',
      '<state id="r0"><invoke id="k" src="file:child.scxml"/><transition event="up"><log label="up"/></transition><transition event="stop" target="done"/></state><final id="done"/>',
      ' initial="r0"',
    ),
    {
      trace: true,
      maxCommands: 100 * depth,
      invokeLimits: { maxInvokeDepth: depth },
      capabilities: { loadDocument: () => child },
    },
  );
  assert.deepEqual(session.start().activeConfiguration, ['r0']);
  const ups = session.trace().entries.filter((entry) => entry.observation === 'log' && entry.label === 'up');
  assert.equal(ups.length, depth, "every level's up reached its invoking session");
  session.send({ name: 'stop' });
  assert.equal(session.status(), 'finished');
  session.dispose();
});

test('createSession rejects a malformed invoke option', async () => {
  const ir = await compile(document('null', '<final id="f"/>'));
  for (const invoke of [null, 'auto', { compile: 'eager' }, { fuel: 0 }, { fuel: 1.5 }, { other: true }]) {
    await assert.rejects(createSession({ ir, invoke, timers: 'manual', loadCoreModule }), hostError('invalid-argument'));
  }
});

// --- deliberately unsupported ----------------------------------------------------

test("<invoke> under invoke.compile 'provided' without compileInvocation is a typed unsupported capability: error.execution", async () => {
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="a">
         <invoke type="http://www.w3.org/TR/scxml/">
           <content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="f"/></scxml></content>
         </invoke>
         <transition event="error.execution" target="pass"/>
         <transition event="*" target="failed"/>
       </state>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="a"',
    ),
    { invoke: { compile: 'provided' } },
  );
  assert.deepEqual(session.start().activeConfiguration, ['pass']);
  assert.equal(UNSUPPORTED.invoke, 'unsupported-invoke');
  session.dispose();
});

test('documents resolve only through the page: none supplied is error.execution', async () => {
  const source = document(
    'ecmascript',
    `<datamodel><data id="doc" src="file:data.json"/></datamodel>
     <state id="a">
       <transition event="error.execution" target="denied"/>
       <transition cond="doc.a === 1" target="loaded"/>
     </state>
     <final id="denied"/>
     <final id="loaded"/>`,
    ' initial="a"',
  );
  const denied = await open('ecmascript', source);
  assert.deepEqual(denied.session.start().activeConfiguration, ['denied']);
  denied.session.dispose();

  const requests = [];
  const supplied = await open('ecmascript', source, {
    capabilities: {
      loadDocument(kind, uri) {
        requests.push([kind, uri]);
        if (uri !== 'file:data.json') throw new CapabilityError('unavailable');
        return '{"a": 1}';
      },
    },
  });
  assert.deepEqual(supplied.session.start().activeConfiguration, ['loaded']);
  assert.deepEqual(requests, [['data', 'file:data.json']]);
  supplied.session.dispose();
});

// --- budget, trace, and options -------------------------------------------------

test('the command budget and trace follow the Wasmtime session host', async () => {
  const source = document(
    'ecmascript',
    `<state id="a">
       <onentry><raise event="r"/><log label="entered" expr="1"/></onentry>
       <transition event="r" target="b"/>
     </state>
     <state id="b"><transition event="go" target="a"/></state>`,
    ' initial="a"',
  );
  const { session } = await open('ecmascript', source, { trace: true });
  session.start();
  const { entries } = session.trace();
  assert.deepEqual(
    entries.map(({ session: id, ...rest }) => (assert.equal(id, 'machine-session'), rest)),
    [
      { observation: 'command' },
      { observation: 'raised', name: 'r' },
      { observation: 'log', label: 'entered', text: '1' },
      { observation: 'command' },
      { observation: 'processed', name: 'r' },
    ],
  );
  assert.equal(session.commands(), 2);
  session.dispose();

  const budgeted = await open('ecmascript', source, { maxCommands: 2 });
  budgeted.session.start();
  assert.throws(() => budgeted.session.send('go'), sessionError('capacity-exhausted'));
  budgeted.session.dispose();
});

/**
 * `body` run `outer` × `inner` times, as two nested `<foreach>` loops.
 */
const repeated = (outer, inner, body) =>
  `<foreach array="Array.from({length: ${outer}}, (_, n) => n)" item="i"><foreach array="Array.from({length: ${inner}}, (_, n) => n)" item="j">${body}</foreach></foreach>`;

test('the trace records every observation: all 66,000 logs', async () => {
  const perSend = 2_000;
  const sends = 33;
  const { session } = await open(
    'ecmascript',
    document('ecmascript', `<state id="a"><transition event="go">${repeated(20, 100, '<log expr="i * 100 + j"/>')}</transition></state>`),
    { trace: true },
  );
  session.start();
  for (let sent = 0; sent < sends; sent += 1) session.send('go');
  const trace = session.trace();
  assert.deepEqual(Object.keys(trace), ['entries'], 'the trace has no truncation flag');
  assert.equal(trace.entries.filter((entry) => entry.observation === 'log').length, perSend * sends);
  const { session: id, observation, text } = trace.entries.at(-1);
  assert.deepEqual({ id, observation, text }, { id: 'machine-session', observation: 'log', text: String(perSend - 1) });
  session.dispose();
});

test('an external queue holds any number of events: 10,250 queued for one invoked session all arrive', async () => {
  // `source` asks the root for five bursts at once; the root handles all five
  // (it is first in invocation order) before `sink` runs, so `sink`'s queue
  // holds 5 × 2,050 = 10,250 events.
  const bursts = 5;
  const count = bursts * 2_050;
  const source = `<scxml xmlns="${SCXML_NS}" version="1.0" datamodel="ecmascript"><state id="s"><onentry>${'<send target="#_parent" event="burst"/>'.repeat(bursts)}</onentry></state></scxml>`;
  const sink = `<scxml xmlns="${SCXML_NS}" version="1.0" datamodel="ecmascript"><datamodel><data id="received" expr="0"/></datamodel><state id="s"><transition event="e" cond="received + 1 === ${count}" target="f"/><transition event="e"><assign location="received" expr="received + 1"/></transition></state><final id="f"/></scxml>`;
  const { session } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<parallel id="p">
         <state id="sinking"><invoke id="sink"><content>${sink}</content></invoke></state>
         <state id="sourcing">
           <invoke id="source"><content>${source}</content></invoke>
           <transition event="burst">${repeated(41, 50, '<send target="#_sink" event="e"/>')}</transition>
         </state>
         <transition event="done.invoke.sink" target="pass"/>
         <transition event="error.*" target="failed"/>
       </parallel>
       <final id="pass"/>
       <final id="failed"/>`,
      ' initial="p"',
    ),
  );
  // At most MAX_DUE_DELIVERIES deliveries per call: the rest wait for the next.
  let last = session.start();
  for (let turn = 0; session.status() === 'running' && turn < 10; turn += 1) last = session.advanceTime(0).results.at(-1) ?? last;
  assert.deepEqual(last.activeConfiguration, ['pass']);
  session.dispose();
});

test('malformed scheduling and capability options are invalid-argument', async () => {
  const ir = await compile(document('ecmascript', '<state id="a"/>'));
  for (const options of [
    { timers: 'wall' },
    { onTimer: 1 },
    { now: 0 },
    { maxCommands: 0 },
    { maxCommands: 1.5 },
    { trace: 'yes' },
    { dataModel: null },
    // `send` and `cancel` belong to the linked I/O processor.
    { capabilities: { send: () => ({ tag: 'routed' }) } },
    { capabilities: { cancel: () => {} } },
    { capabilities: { log: 'console' } },
  ]) {
    await assert.rejects(createSession({ ir, loadCoreModule, ...options }), hostError('invalid-argument'));
  }
  const { session } = await open('ecmascript', document('ecmascript', '<state id="a"/>'));
  session.start();
  for (const ms of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1, '5']) {
    assert.throws(() => session.advanceTime(ms), hostError('invalid-argument'));
  }
  session.dispose();
  assert.throws(() => session.now(), hostError('instance-lost'));
});

// --- invoked sessions -----------------------------------------------------------

test('invoked sessions keep the executor order and invoke limits', () => {
  const invoked = new InvokedSessions({ maxInvokeDepth: 2, maxInvokedSessions: 3 });
  const b = invoked.register(ROOT, 'b');
  const a = invoked.register(ROOT, 'a');
  const x = invoked.register(a, 'x');
  assert.deepEqual(invoked.keys(), [ROOT, a, x, b], 'sessions iterate in invocation order');
  assert.deepEqual(invoked.path(x), ['a', 'x']);
  assert.equal(invoked.child(a, 'x'), x);
  // Sends go straight onto the target's external queue; nothing reaches an
  // absent session.
  assert.equal(invoked.pushExternal(invoked.child(ROOT, 'gone'), 'x'), false);
  invoked.pushExternal(b, 'to-b');
  invoked.pushToParent(x, { tag: 'event', val: 'up' });
  assert.deepEqual(invoked.popNext(() => true), [a, { tag: 'from-invocation', invokeId: 'x', message: { tag: 'event', val: 'up' } }]);
  assert.deepEqual(invoked.popNext(() => true), [b, { tag: 'external', val: 'to-b' }]);
  assert.equal(invoked.popNext(() => true), undefined);
  // done.invoke is reported once, after the session's events.
  invoked.pushToParent(b, { tag: 'event', val: 'last' });
  invoked.markFinished(b);
  assert.deepEqual(invoked.takeDoneDue(), [b]);
  assert.deepEqual(invoked.takeDoneDue(), [], 'done is reported once');
  // A session being cancelled sends nothing more (official case 252).
  invoked.beginCancel(a);
  assert.equal(invoked.pushToParent(x, { tag: 'event', val: 'ignored' }), false);
  assert.deepEqual(invoked.remove(a), [x, a], 'deepest first');
  assert.deepEqual(invoked.keys(), [ROOT, b]);
  // Bounds fail closed with the executor's violation codes.
  const violation = (code) => (error) => error instanceof InvokeViolation && error.code === code;
  const y = invoked.register(b, 'y');
  assert.equal(invoked.canInvoke(y), false, 'depth 3 is beyond the bound');
  assert.throws(() => invoked.register(y, 'z'), violation('invoke-limit-exceeded'));
  assert.throws(() => invoked.register(999, 'c'), violation('session-missing'));
  assert.throws(() => invoked.register(b, 'y'), violation('duplicate-invocation'));
  invoked.register(ROOT, 'c');
  assert.throws(() => invoked.register(ROOT, 'd'), violation('invoke-limit-exceeded'));
});

test("a command's step counts the effects of every root step it runs, the deliveries of its children's events to the root included", async () => {
  // The child sends `hello` to its parent while it starts, so the root's
  // start also delivers it: the start's step counts the root's start and
  // that delivery, each measured on its own below.
  const root = (child) =>
    document(
      'ecmascript',
      `<state id="a"><invoke><content><scxml xmlns="${SCXML_NS}" version="1.0"><state id="c">${child}</state></scxml></content></invoke>
       <transition event="hello"><log expr="1"/><log expr="2"/><log expr="3"/></transition></state>`,
      ' initial="a"',
    );
  const silent = await open('ecmascript', root(''));
  const started = silent.session.start().effectCount;
  const delivered = silent.session.send({ name: 'hello' }).effectCount;
  silent.session.dispose();
  const greeting = await open('ecmascript', root('<onentry><send target="#_parent" event="hello"/></onentry>'));
  const step = greeting.session.start();
  greeting.session.dispose();
  assert.ok(delivered > 0);
  assert.equal(step.effectCount, started + delivered);
});

test('a command that carries out a deferred <invoke> reports one step: every effect of its pieces counted, nothing suspended', async () => {
  const logs = '<log expr="1"/>'.repeat(5);
  const child = `<invoke><content><scxml xmlns="${SCXML_NS}" version="1.0"><state id="c"/></scxml></content></invoke>`;
  const results = [];
  for (const invoke of ['', child]) {
    const { session } = await open('ecmascript', document('ecmascript', `<state id="a"><onentry>${logs}</onentry>${invoke}</state>`, ' initial="a"'));
    results.push(session.start());
    session.dispose();
  }
  const [plain, invoking] = results;
  assert.equal('suspended' in invoking, false, 'the public step has no suspended field');
  // The invoking command runs every effect the plain one runs, and its
  // <invoke> start besides; the continuation alone ran none of the logs.
  assert.ok(invoking.effectCount > plain.effectCount, `${invoking.effectCount} > ${plain.effectCount}`);
});

test('idle invoked sessions cost a clock tick nothing: 100 idle children make the same I/O calls per tick as 10, as under Wasmtime', async () => {
  // The root ticks every logical millisecond; its children are idle (no
  // timers, no events). A tick reads the earliest deadline from an index
  // and moves only the clocks of sessions about to run, so the I/O and
  // clock calls of 300 ticks (every `IoLink` method) are the same with 100
  // idle children as with 10. (Each child instance costs about 70 ms and
  // 5 MB to create here, which bounds the count.)
  const tickCalls = async (children) => {
    const invokes = Array.from(
      { length: children },
      (_, n) => `<invoke id="k${n}"><content><scxml xmlns="${SCXML_NS}" version="1.0" datamodel="null"><state id="idle"/></scxml></content></invoke>`,
    ).join('');
    const { session } = await open(
      'ecmascript',
      document(
        'ecmascript',
        `<state id="a">${invokes}<onentry><send event="tick" delay="1ms"/></onentry><transition event="tick" type="internal"><send event="tick" delay="1ms"/></transition></state>`,
        ' initial="a"',
      ),
    );
    session.start();
    session.advanceTime(20);
    const methods = Object.getOwnPropertyNames(IoLink.prototype).filter(
      (name) => name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(IoLink.prototype, name).value === 'function',
    );
    const stops = methods.map((name) => [name, countCalls(IoLink.prototype, name)]);
    let calls;
    try {
      session.advanceTime(300);
    } finally {
      calls = Object.fromEntries(stops.map(([name, stop]) => [name, stop()]));
    }
    session.dispose();
    return calls;
  };
  const few = await tickCalls(10);
  const many = await tickCalls(100);
  assert.ok(few.advance >= 300, `the root's clock moved every tick: ${JSON.stringify(few)}`);
  assert.deepEqual(many, few);
});

test('a call that defers a start but does not suspend for it drops the child it built, as under Wasmtime', () => {
  for (const ending of ['returned', 'threw']) {
    const removed = [];
    let disposed = 0;
    const child = { instance: { links: { key: 7, children: new Map() }, dispose: () => (disposed += 1) } };
    const run = { deferred: undefined, budget: { hold: (body) => body() }, remove: (key) => removed.push(key) };
    const instance = {
      links: { run },
      rawCall: (body) => {
        run.deferred = { kind: 'start', child };
        return ending === 'returned' ? { value: body() } : { thrown: new Error('trap') };
      },
    };
    const outcome = callDeferred(instance, () => ({ suspended: false }));
    assert.deepEqual(Object.keys(outcome), [ending === 'returned' ? 'value' : 'thrown'], ending);
    assert.equal(run.deferred, undefined, ending);
    assert.deepEqual(removed, [7], ending);
    assert.equal(disposed, 1, ending);
  }
});

test('an invocation request while one is pending is a host invariant, as under Wasmtime', () => {
  const pending = { kind: 'cancel', child: undefined };
  let fault;
  const links = { run: { deferred: pending }, children: new Map(), invariant: () => ((fault = 'host-invariant'), new Error('host')) };
  assert.throws(() => requestCancel(links, 'k'));
  assert.equal(fault, 'host-invariant');
  assert.equal(links.run.deferred, pending, 'the pending request is kept');
  fault = undefined;
  assert.throws(() => requestStart(links, { invokeId: 'k', machineIr: new Uint8Array() }));
  assert.equal(fault, 'host-invariant');
});

test('a session finds its place among its siblings by binary search: 20,000 siblings in scrambled order register in pre-order', () => {
  const invoked = new InvokedSessions();
  const id = (n) => `k${String(n).padStart(8, '0')}`;
  // The ids 1..20,000 in the order 1 + 7,919 * i mod 20,000 (7,919 is
  // coprime to 20,000): each new id lands among its siblings far from
  // either end, so a scan of the siblings from either end would compare it
  // with thousands of them (about 10^8 comparisons in all); the binary
  // search compares it with at most 15 (ceil(log2(20,000)) + 1), each
  // reading at most the 9 code units of both ids.
  const stop = countCalls(String.prototype, 'charCodeAt');
  let codeUnits;
  try {
    for (let i = 0; i < 20_000; i += 1) invoked.register(ROOT, id(1 + ((7_919 * i) % 20_000)));
  } finally {
    codeUnits = stop();
  }
  assert.ok(codeUnits <= 20_000 * 15 * 2 * 9, `${codeUnits} code units compared`);
  const key = (n) => invoked.child(ROOT, id(n));
  // A removed id leaves the index: it registers again in its place.
  invoked.remove(key(10_000));
  const again = invoked.register(ROOT, id(10_000));
  assert.equal(again, key(10_000));
  // Queued work drains in invocation order: ascending invoke id.
  for (const n of [20_000, 10_001, 10_000, 9_999, 1]) invoked.pushExternal(key(n), n);
  const drained = [];
  for (let next = invoked.popNext(() => true); next !== undefined; next = invoked.popNext(() => true)) {
    drained.push(next[1].val);
  }
  assert.deepEqual(drained, [1, 9_999, 10_000, 10_001, 20_000]);
});

test('invoked sessions without limits: any depth, any count, any queue length', () => {
  const invoked = new InvokedSessions();
  // 40 nested levels.
  let parent = ROOT;
  for (let level = 0; level < 40; level += 1) {
    assert.equal(invoked.canInvoke(parent), true);
    parent = invoked.register(parent, `l${level}`);
  }
  // 300 invoked sessions in all.
  const s0 = invoked.register(ROOT, 's0');
  for (let n = 1; n < 260; n += 1) invoked.register(ROOT, `s${n}`);
  assert.equal(invoked.size, 301);
  // 10,001 queued external events.
  for (let n = 0; n <= 10_000; n += 1) assert.equal(invoked.pushExternal(s0, n), true);
  assert.equal(invoked.external(s0).length, 10_001);
  for (let n = 0; n <= 10_000; n += 1) assert.deepEqual(invoked.popNext((at) => at === s0), [s0, { tag: 'external', val: n }]);
  assert.equal(invoked.popNext((at) => at === s0), undefined);
  // A single explicit limit applies alone; the absent one sets no limit.
  const deepOnly = new InvokedSessions({ maxInvokeDepth: 1 });
  const first = deepOnly.register(ROOT, 's0');
  for (let n = 1; n < 300; n += 1) deepOnly.register(ROOT, `s${n}`);
  assert.equal(deepOnly.canInvoke(first), false);
  const fewOnly = new InvokedSessions({ maxInvokedSessions: 2 });
  fewOnly.register(fewOnly.register(ROOT, 'a'), 'b');
  assert.equal(fewOnly.canInvoke(ROOT), false);
  assert.equal(new InvokedSessions({ maxInvokeDepth: 0 }).canInvoke(ROOT), false);
});

test('sessions iterate in the Rust executor\'s code point order across the BMP boundary', () => {
  // The Wasmtime executor orders sibling invoke ids by UTF-8 bytes, that is
  // by Unicode code point. UTF-16 code units put U+10000 (surrogates D800
  // DC00) before U+FF21; code points do not.
  const ids = ['\u{10000}', 'Ａ', '', '\u{1F600}', 'a\u{10000}', 'a￿', 'é', 'a'];
  const invoked = new InvokedSessions({ maxInvokeDepth: 1, maxInvokedSessions: ids.length });
  const keys = new Map(ids.map((id) => [invoked.register(ROOT, id), id]));
  const utf8 = new TextEncoder();
  const byUtf8 = (x, y) => Buffer.compare(Buffer.from(utf8.encode(x)), Buffer.from(utf8.encode(y)));
  const expected = ['a', 'a￿', 'a\u{10000}', 'é', '', 'Ａ', '\u{10000}', '\u{1F600}'];
  assert.deepEqual([...ids].sort(byUtf8), expected, 'expected order is UTF-8 byte order');
  assert.deepEqual(
    invoked.keys().slice(1).map((key) => keys.get(key)),
    expected,
  );
  for (const [key, id] of keys) invoked.pushExternal(key, `to-${id}`);
  const delivered = [];
  for (let next = invoked.popNext(() => true); next !== undefined; next = invoked.popNext(() => true)) delivered.push(keys.get(next[0]));
  assert.deepEqual(delivered, expected);
});

test('invocation order is the path order under any registration order, as in the Rust scheduler', () => {
  const invoked = new InvokedSessions();
  const live = [ROOT];
  // A spine 200 levels deep whose every level registers `b` before `a`:
  // labels run out of room and must be spread again, and the registration
  // order must not leak into the result.
  let spine = ROOT;
  for (let level = 0; level < 200; level += 1) {
    const later = invoked.register(spine, 'b');
    live.push(later, invoked.register(spine, 'a'));
    spine = later;
  }
  let seed = 0x2545f491;
  const next = (bound) => {
    seed = (Math.imul(seed, 1_103_515_245) + 12_345) >>> 0;
    return (seed >>> 8) % bound;
  };
  const byPath = (keys) =>
    [...keys].sort((x, y) => {
      const [p, q] = [invoked.path(x), invoked.path(y)];
      for (let i = 0; i < Math.min(p.length, q.length); i += 1) {
        const order = byCodePoint(p[i], q[i]);
        if (order !== 0) return order;
      }
      return p.length - q.length;
    });
  for (let round = 0; round < 4_000; round += 1) {
    if (round % 7 === 6 && live.length > 1) {
      const removed = new Set(invoked.remove(live[1 + next(live.length - 1)]));
      for (let i = live.length - 1; i >= 0; i -= 1) if (removed.has(live[i])) live.splice(i, 1);
    } else {
      const parent = live[next(live.length)];
      const id = String(next(40));
      if (invoked.child(parent, id) === undefined) live.push(invoked.register(parent, id));
    }
    if (round % 250 === 0 || round === 3_999) assert.deepEqual(invoked.keys(), byPath(live), `round ${round}`);
  }
  const expected = byPath(live);
  for (const key of [...expected].reverse()) invoked.pushExternal(key, 'e');
  const taken = [];
  for (let entry = invoked.popNext(() => true); entry !== undefined; entry = invoked.popNext(() => true)) taken.push(entry[0]);
  assert.deepEqual(taken, expected);
});

test('ordering queued work costs no more than the queue and the tree: 10,000 levels each queued at once', () => {
  // Every session lookup is one `Map.get`: registering, queueing, taking
  // each level's work from a heap of at most 10,000 sessions (at most
  // 2 * ceil(log2(10,000)) comparisons per push or pop, two lookups each),
  // and the cancel cost at most 8 * ceil(log2(10,000)) lookups a level
  // (about 59 here); work that scanned the queue or the chain would cost
  // about 10,000 a level.
  const stop = countCalls(Map.prototype, 'get');
  const invoked = new InvokedSessions();
  const chain = [ROOT];
  for (let level = 0; level < 10_000; level += 1) {
    const key = invoked.register(chain.at(-1), `k${level}`);
    assert.equal(invoked.pushToParent(key, { tag: 'event', val: 'up' }), true);
    chain.push(key);
  }
  const taken = [];
  for (let entry = invoked.popNext(() => true); entry !== undefined; entry = invoked.popNext(() => true)) taken.push(entry[0]);
  assert.deepEqual(taken, chain.slice(0, -1), 'each invoking session, root first');
  invoked.beginCancel(chain[1]);
  assert.equal(invoked.remove(chain[1]).length, 10_000);
  const lookups = stop();
  assert.ok(lookups <= 10_000 * 8 * 14, `${lookups} session lookups`);
});

// --- delayed sends that cannot be dispatched (SCXML 6.2, 6.2.4, C.1) ----------

test('a delayed send whose invoked target finished raises error.communication with its send id', async () => {
  // SCXML 6.2: the message is dispatched "only when the delay interval
  // elapses"; 6.2.4: "If it is unable to dispatch the message, the Processor
  // MUST place the error error.communication". The child `c` finished before
  // the send to `#_c` fell due: the platform error carries the send's id.
  const { session, logs } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="s0">
         <invoke id="c"><content><scxml xmlns="${SCXML_NS}" version="1.0" datamodel="null" initial="c0">` +
        `<state id="c0"><onentry><send event="ready" target="#_parent"/></onentry><transition event="stop" target="cf"/></state>` +
        `<final id="cf"/></scxml></content></invoke>
         <transition event="ready"><send id="late" event="late" target="#_c" delay="10ms"/><send event="stop" target="#_c"/></transition>
         <transition event="done.invoke.c"><log label="child-done"/></transition>
         <transition event="error.communication" target="done"><log label="error" expr="_event.sendid + ':' + _event.type"/></transition>
         <transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition>
       </state>
       <final id="done"/>
       <final id="fail"/>`,
      ' initial="s0"',
    ),
  );
  session.start();
  assert.equal(session.advanceTime(10).complete, true);
  assert.deepEqual(session.configuration(), ['done']);
  assert.deepEqual(logs, ['child-done: ', 'error: late:platform']);
  session.dispose();
});

test('a delayed #_internal send raises error.execution and delivers nothing', async () => {
  // SCXML 6.2.1: 'delay'/'delayexpr' "Must not occur ... when the attribute
  // 'target' has the value "_internal""; 6.2.4: a target that is "not
  // supported or invalid" raises error.execution.
  const { session, logs } = await open(
    'ecmascript',
    document(
      'ecmascript',
      `<state id="s0">
         <onentry><send id="soon" event="soon" targetexpr="'#_internal'" delayexpr="'10ms'"/></onentry>
         <transition event="error.execution" target="s1"><log label="error" expr="_event.sendid"/></transition>
         <transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition>
       </state>
       <state id="s1">
         <onentry><send event="settled" delay="20ms"/></onentry>
         <transition event="settled" target="done"/>
         <transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition>
       </state>
       <final id="done"/>
       <final id="fail"/>`,
      ' initial="s0"',
    ),
  );
  session.start();
  assert.equal(session.advanceTime(20).complete, true);
  assert.deepEqual(session.configuration(), ['done']);
  assert.deepEqual(logs, ['error: soon']);
  session.dispose();
});
