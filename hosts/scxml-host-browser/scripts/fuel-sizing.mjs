// The workloads that size DEFAULT_FUEL in this host's charges: the smallest
// per-call fuel each runs under, found by bisection over the session's
// `fuel` (and the data model's), and printed with the headroom
// DEFAULT_FUEL leaves it. Run after `npm run build`:
//
//   node scripts/fuel-sizing.mjs
//
// The Wasmtime host's counterpart is
// hosts/scxml-host-wasmtime/tests/fuel_sizing.rs.
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  DEFAULT_FUEL,
  ScxmlHostError,
  cachedCoreModules,
  createCompiler,
  createEcmaScriptDataModel,
  createNullDataModel,
  createSession,
} from '../src/index.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');
const loadCoreModule = cachedCoreModules((component, file) => readFile(join(packageRoot, 'dist', component, file)));
const compiler = await createCompiler({ loadCoreModule });

/** The lower end of the bisection, which no workload runs under. */
const FUEL_FLOOR = 1_000;
/** The upper end of the bisection, which every workload runs under. */
const FUEL_CEILING = 2 * DEFAULT_FUEL;
/** The bisection stops within 1 / PRECISION of the fuel that runs (0.5 %). */
const PRECISION = 200;

/**
 * Whether the workload starts and takes `events` on `fuel` per host call
 * without leaving its first state for `f` (where an invocation failure goes).
 */
async function runs(ir, dataModelName, events, fuel) {
  let session;
  try {
    const dataModel =
      dataModelName === 'null'
        ? await createNullDataModel({ loadCoreModule, fuel })
        : await createEcmaScriptDataModel({ loadCoreModule, fuel });
    session = await createSession({ ir, dataModel, loadCoreModule, fuel, timers: 'manual' });
    let step = session.start();
    for (const event of events) step = session.send(event);
    return !step.activeConfiguration.includes('f');
  } catch (error) {
    if (error instanceof ScxmlHostError && error.code === 'fuel-exhausted') return false;
    throw error;
  } finally {
    session?.dispose();
  }
}

/** The smallest per-call fuel the workload runs under, within 0.5 %. */
async function smallestFuel(label, source, dataModelName, events) {
  const compiled = await compiler.compile(source, { sourceName: 'sizing.scxml' });
  if (compiled.status !== 'emitted') throw new Error(new TextDecoder().decode(compiled.report));
  if (!(await runs(compiled.ir, dataModelName, events, FUEL_CEILING))) {
    throw new Error(`${label}: does not run on twice the default fuel`);
  }
  let [fails, passes] = [FUEL_FLOOR, FUEL_CEILING];
  while (passes - fails > passes / PRECISION) {
    const middle = fails + Math.floor((passes - fails) / 2);
    if (await runs(compiled.ir, dataModelName, events, middle)) passes = middle;
    else fails = middle;
  }
  return passes;
}

/** An ECMAScript `<foreach>` with an `<assign>` over `items` items in the start. */
const foreach = (items) =>
  '<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0">' +
  `<datamodel><data id="arr" expr="(function(){var r=[];for(var i=0;i&lt;${items};i++)r.push(i);return r})()"/><data id="acc" expr="0"/></datamodel>` +
  '<state id="s0"><onentry><foreach array="arr" item="it"><assign location="acc" expr="acc + it"/></foreach><log label="sum" expr="acc"/></onentry></state></scxml>';
/** Eight non-assignment `<script>`s in one transition. */
const eightScripts =
  '<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><transition event="go" target="s1">' +
  '<script>[1, 2, 3].map(function (v) { return v * 2; });</script>'.repeat(8) +
  '</transition></state><state id="s1"/></scxml>';
/** A `count`-region parallel state whose regions all transition on `t`. */
function wideParallel(count) {
  let regions = '';
  for (let region = 0; region < count; region += 1) {
    regions +=
      `<state id="r${region}" initial="r${region}a"><state id="r${region}a"><transition event="t" target="r${region}b"/></state>` +
      `<state id="r${region}b"><transition event="t" target="r${region}a"/></state></state>`;
  }
  return `<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="wide"><parallel id="wide">${regions}</parallel></scxml>`;
}
/** An ECMAScript root invoking an inline child of `count` states. */
function largeInlineChild(count) {
  let states = '';
  for (let state = 0; state < count; state += 1) {
    states += `<state id="c${state}"><transition event="e${state}" target="c${(state + 1) % count}"/></state>`;
  }
  return (
    '<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0">' +
    `<invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c0">${states}</scxml></content></invoke>` +
    '<transition event="error.*" target="f"/></state><final id="f"/></scxml>'
  );
}
/** Items of the `<foreach>` workloads. */
const FOREACH_ITEMS = [60, 90, 1_000];
/** Regions of the wide parallel workload. */
const WIDE_REGIONS = 64;
/** States of the large inline child workload. */
const CHILD_STATES = 4_000;
const representative = await readFile(join(workspace, 'tests/fixtures/representative-machine.scx'), 'utf8');

const workloads = [
  ...FOREACH_ITEMS.map((items) => [`<foreach> over ${items} items`, foreach(items), 'ecmascript', []]),
  ['eight <script>s in one transition', eightScripts, 'ecmascript', ['go']],
  ['representative machine start', representative, 'ecmascript', []],
  [`${WIDE_REGIONS}-region parallel start and event`, wideParallel(WIDE_REGIONS), 'null', ['t']],
  [`${CHILD_STATES}-state inline child start`, largeInlineChild(CHILD_STATES), 'ecmascript', []],
];
for (const [label, source, dataModelName, events] of workloads) {
  const fuel = await smallestFuel(label, source, dataModelName, events);
  console.log(`${label}: ${fuel} charges, ${(DEFAULT_FUEL / fuel).toFixed(2)} times headroom on DEFAULT_FUEL`);
}
