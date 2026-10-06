// Module Worker entry of an `async: 'auto'` session with the `worker`
// mechanism (src/async-bridge.js has the protocol). It runs the unchanged
// synchronous MachineSession. Every page provider (capabilities and
// `resources`) is called through the blocking channel, so the machine's
// synchronous capability calls can wait on the page's Promises. Core
// modules come from the page's loader, as compiled WebAssembly.Module
// values.
//
// Telemetry: the worker's spans go to the worker's global OpenTelemetry
// API, which stays a no-op unless the worker registers an SDK. The page
// records its own `create-session` and `capability` spans at the boundary.

import { BRIDGE, encodeError, waitOnChannel, workerBridge } from './async-bridge.js';
import { createDataModel } from './datamodel.js';
import { CapabilityError, ScxmlHostError } from './errors.js';
import { createSession } from './machine.js';
import { PREFETCHED, ResourceCache } from './resources.js';

const post = (message) => self.postMessage(message);
const modules = new Map();
let nextModule = 0;
let session;
let channel;

/** Asks the page's loader for one compiled core module. */
function loadCoreModule(component, file) {
  const id = (nextModule += 1);
  return new Promise((resolve, reject) => {
    modules.set(id, { resolve, reject });
    post({ type: 'module', id, component, file });
  });
}

/** Blocks on one page operation (the bridge's waitSync). */
function wait(operation, args) {
  return waitOnChannel(channel, post, operation, args);
}

/** A capability provider that calls the page's provider synchronously. */
function pageCapabilities(keys) {
  const capabilities = {};
  for (const key of keys) capabilities[key] = (...args) => wait(key, args);
  return capabilities;
}

async function init(options) {
  const { capabilityKeys, resources, prefetched, onTimer, dataModel, fuel, ...rest } = options;
  const bridge = workerBridge(wait);
  const model = dataModel === undefined ? undefined : await createDataModel(dataModel, { loadCoreModule, fuel });
  session = await createSession({
    ...rest,
    dataModel: model,
    fuel,
    loadCoreModule,
    capabilities: pageCapabilities(capabilityKeys),
    onTimer: onTimer
      ? (outcome) => post({ type: 'timer', outcome: outcome.error === undefined ? outcome : { error: encodeError(outcome.error) } })
      : undefined,
    [BRIDGE]: resources ? bridge : undefined,
    // The page's prefetch of the static URLs (src/resources.js).
    [PREFETCHED]: prefetched === undefined ? undefined : new ResourceCache(new Map(prefetched)),
  });
}

const METHODS = new Set([
  'start',
  'send',
  'resume',
  'configuration',
  'status',
  'failure',
  'now',
  'nextDeadline',
  'advanceTime',
  'commands',
  'trace',
  'shutdown',
  'snapshot',
  'dispose',
]);

async function operate({ id, name, args }) {
  try {
    if (!METHODS.has(name) || session === undefined) throw new ScxmlHostError('invalid-argument');
    post({ type: 'result', id, value: await session[name](...args) });
  } catch (error) {
    post({ type: 'result', id, error: encodeError(error) });
  }
}

self.addEventListener('message', async ({ data }) => {
  switch (data?.type) {
    case 'init':
      channel = data.channel;
      try {
        await init(data.options);
        post({ type: 'ready' });
      } catch (error) {
        post({ type: 'init-failed', error: encodeError(error) });
      }
      break;
    case 'op':
      await operate(data);
      break;
    case 'module': {
      const pending = modules.get(data.id);
      modules.delete(data.id);
      if (data.failed) pending?.reject(new CapabilityError('unavailable'));
      else pending?.resolve(data.module);
      break;
    }
    default:
      break;
  }
});
