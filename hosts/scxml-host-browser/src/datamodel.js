// Links the bundled data-model components (ECMAScript, XPath, null;
// each transpiled once by jco) as machine-session `dataModel` providers.
//
// The component evaluates every expression; this adapter only forwards
// every `data-model-api` function one-to-one, answers the component's
// `In()` import from the configuration the machine updates, and maps
// component errors to closed codes. The machine initializes the data model
// itself (`initialize-data-model`, from `start`), with the host's
// `_sessionid` and `_ioprocessors` and the document's `_name`; a provider is
// therefore created uninitialized and belongs to exactly one session.
//
// `event` is the WIT event, or `undefined` where SCXML defines no current
// event (for example `<onentry>` executed by `start`). It is forwarded as the
// WIT `option<event>` unchanged: the component leaves `_event` unbound for
// `none` and never sees a fabricated event.
//
// Bounds: every call into the component runs on a deterministic fuel budget
// (src/fuel.js): capped per call by the provider's `fuel`, and, once a
// session links the provider (joinFuelBudget), charged to the budget of the
// session's host call that made it, so a machine command's data-model work
// spends that command's budget. Used without a session, each call is its
// own host call. A call that traps (fuel exhaustion or any other trap)
// discards the instance: that call and every later fallible one throw
// `CapabilityError('platform')`, which fails the calling machine session,
// and every later pure one answers `undefined` (WIT `none`), except
// `acceptsEventData`, which answers `true`, exactly as the Wasmtime host
// poisons a trapped data-model store.
//
// The `execution-host` import: the component calls only `in-state` and
// `now-ms` (the linked session's clock time; 0 unlinked). Effects
// (`<raise>`, `<send>`, `<cancel>`, `<log>`) are machine capabilities, never
// data-model imports, so `raise`/`send`/`cancel` fail closed with
// `unavailable` and `log` (infallible by WIT contract) records nothing; each
// such call is observed as a rejected `capability` operation.

import { instantiate as instantiateEcmaScript } from '../dist/data-model-ecmascript/data-model-ecmascript.js';
import { instantiate as instantiateNull } from '../dist/data-model-null/data-model-null.js';
import { instantiate as instantiateXPath } from '../dist/data-model-xpath/data-model-xpath.js';
import { CapabilityError, ImportFailure, ScxmlHostError, exportPayload } from './errors.js';
import { DEFAULT_FUEL, FuelBudget, FuelMeter, isFuel } from './fuel.js';
import { fetchCoreModules } from './loader.js';
import { GUEST_MODULES, Telemetry, isTelemetryOption, scxmlTelemetryImport } from './telemetry.js';
import {
  isAssignPlacement,
  isAssignSource,
  isDataInitializer,
  isDataValue,
  isInlineBody,
  isNamedValues,
  isWitEvent,
} from './values.js';

let defaultLoader;

/** Bundled data models: `data-model-api.name` -> transpiled component. */
const BUNDLED = Object.freeze({
  ecmascript: { component: 'data-model-ecmascript', instantiate: instantiateEcmaScript },
  xpath: { component: 'data-model-xpath', instantiate: instantiateXPath },
  null: { component: 'data-model-null', instantiate: instantiateNull },
});

/** A bundled provider's hook that charges its calls to a session's budget. */
const JOIN_BUDGET = Symbol('scxml.data-model.join-budget');
/** A bundled provider's hook that answers `now-ms` from a session's clock. */
const JOIN_CLOCK = Symbol('scxml.data-model.join-clock');

/**
 * Charges every later call of `provider` (when it is a bundled data model)
 * to `budget`, the fuel budget of the session that links it; a page
 * provider is unaffected.
 */
export function joinFuelBudget(provider, budget) {
  provider?.[JOIN_BUDGET]?.(budget);
}

/**
 * Answers every later `execution-host.now-ms` of `provider` (when it is a
 * bundled data model) with `now()`, the clock time in ms of the session
 * that links it; unlinked, a provider's clock reads 0.
 */
export function joinSessionClock(provider, now) {
  provider?.[JOIN_CLOCK]?.(now);
}

/** Names of the bundled data models (`data-model-api.name`). */
export const DATA_MODELS = Object.freeze(Object.keys(BUNDLED));

function unavailable() {
  return new ImportFailure({
    version: 'v1',
    code: 'unavailable',
    detail: 'capability is not provided by this host',
  });
}

const isOptionalEvent = (event) => event === undefined || isWitEvent(event);
const isStringList = (value) => Array.isArray(value) && value.every((item) => typeof item === 'string');
const isSystemValues = (value) =>
  value !== null &&
  typeof value === 'object' &&
  typeof value.sessionId === 'string' &&
  // `_name`: the document's `name`, or undefined when it has none (SCXML 5.10).
  (value.machineName === undefined || typeof value.machineName === 'string') &&
  isDataValue(value.ioProcessors);

/**
 * Instantiates one bundled data model, uninitialized, for one session.
 *
 * @param {string} name one of DATA_MODELS
 * @param {object} [options]
 * @param {object} [options.telemetry] OpenTelemetry providers
 *   `{tracerProvider?, meterProvider?}`; omitted ones are the global API
 *   providers
 * @param {Function} [options.loadCoreModule] `(component, file) => Module`
 * @param {number} [options.fuel] per-call fuel cap (default DEFAULT_FUEL);
 *   inside a session each call is also bounded by what is left of the
 *   session's host-call budget
 * @returns {Promise<object>} a `dataModel` provider for createSession
 * @throws {ScxmlHostError} `invalid-argument` for an unknown name or a
 *   malformed option, `fuel-exhausted` or `instance-lost` when
 *   instantiation traps
 */
export async function createDataModel(name, options) {
  const { telemetry: telemetryOption, loadCoreModule, fuel = DEFAULT_FUEL } = options ?? {};
  const telemetry = new Telemetry(telemetryOption);
  const bundled = typeof name === 'string' && Object.hasOwn(BUNDLED, name) ? BUNDLED[name] : undefined;
  if (bundled === undefined || !isFuel(fuel) || !isTelemetryOption(telemetryOption)) {
    telemetry.observe('create-session', 'rejected');
    throw new ScxmlHostError('invalid-argument');
  }
  const linked = link(bundled, telemetry);
  const loader = loadCoreModule ?? (defaultLoader ??= fetchCoreModules());
  const meter = new FuelMeter(fuel);
  const span = telemetry.start('create-session');
  let api;
  try {
    api = (await linked.instantiate((file) => loader(bundled.component, file), meter.instantiateCore))
      .dataModelApi;
  } catch (cause) {
    telemetry.end(span, 'failed');
    if (meter.exhausted()) throw new ScxmlHostError('fuel-exhausted');
    throw new ScxmlHostError('instance-lost', { cause });
  }
  telemetry.end(span, 'ok');
  return provider(name, api, telemetry, meter, linked.state, new FuelBudget(fuel));
}

/**
 * Instantiates one bundled data model synchronously from compiled core
 * modules (an invoked session's data model, constructed inside the
 * parent's `start-invocation` call).
 *
 * @param {string} name the child document's data model, as its Machine IR
 *   records it (its `datamodel`, or the default `ecmascript`)
 * @param {{telemetry: Telemetry, module: (component: string, file: string) => WebAssembly.Module, fuel: number, budget: FuelBudget}} options
 *   `fuel` is the per-call cap; instantiation and every call are charged to
 *   the session's `budget`
 * @returns {object | undefined} a provider, or `undefined` for a name this
 *   host does not bundle (the child then runs with no data model)
 * @throws {unknown} whatever instantiation throws (a trap, a missing module)
 */
export function instantiateDataModelSync(name, { telemetry, module, fuel, budget }) {
  const bundled = typeof name === 'string' && Object.hasOwn(BUNDLED, name) ? BUNDLED[name] : undefined;
  if (bundled === undefined) return undefined;
  const linked = link(bundled, telemetry);
  const meter = new FuelMeter(fuel);
  const root = budget.run(meter, () =>
    linked.instantiate((file) => module(bundled.component, file), meter.instantiateCoreSync),
  );
  return provider(name, root.dataModelApi, telemetry, meter, linked.state, budget);
}

/**
 * The component's imports: telemetry, and the `execution-host` whose only
 * answered functions are `in-state`, over the configuration the machine
 * updates (`state.configuration`, a set: only membership is observable),
 * and `now-ms`, the clock time of the session that links the provider
 * (`state.now`, set by joinSessionClock).
 */
function link(bundled, telemetry) {
  const state = { configuration: new Set(), now: () => 0 };
  const rejectEffect = () => {
    telemetry.observe('capability', 'rejected');
    throw unavailable();
  };
  const executionHost = {
    inState: (stateId) => state.configuration.has(stateId),
    nowMs: () => BigInt(state.now()),
    raise: rejectEffect,
    send: rejectEffect,
    cancel: rejectEffect,
    log: () => telemetry.observe('capability', 'rejected'),
  };
  const imports = {};
  for (const [key, value] of [
    ['types', {}],
    ['telemetry', scxmlTelemetryImport(telemetry, GUEST_MODULES.dataModel)],
    ['execution-host', executionHost],
  ]) {
    imports[`stateforward:scxml/${key}`] = value;
    imports[`stateforward:scxml/${key}@0.2.0`] = value;
  }
  return {
    state,
    instantiate: (getCoreModule, instantiateCore) => bundled.instantiate(getCoreModule, imports, instantiateCore),
  };
}

/**
 * The provider object over one instantiated `data-model-api`, whose calls
 * run on `budget` (replaced by joinFuelBudget).
 */
function provider(name, api, telemetry, meter, state, budget) {
  let lost = false;
  /** Runs one fallible component call on the budget; `valid` guards the arguments. */
  const call = (valid, body) => {
    if (lost) {
      telemetry.observe('data-model', 'failed');
      throw new CapabilityError('platform');
    }
    if (!valid) {
      telemetry.observe('data-model', 'rejected');
      throw new CapabilityError('invalid-input');
    }
    const callSpan = telemetry.start('data-model');
    try {
      const result = budget.run(meter, () => telemetry.within(callSpan, body));
      telemetry.end(callSpan, 'ok');
      return result;
    } catch (thrown) {
      const payload = exportPayload(thrown);
      if (payload === undefined) {
        // A trap (fuel exhaustion included): never enter the instance again.
        lost = true;
        telemetry.end(callSpan, 'failed');
        throw new CapabilityError('platform');
      }
      telemetry.end(callSpan, 'rejected');
      throw new CapabilityError(payload.code ?? 'platform');
    }
  };
  /**
   * Runs one pure component call (no error channel): `undefined` (WIT
   * `none`) for malformed arguments, after a trap, and on a lost instance.
   */
  const pure = (valid, body) => {
    if (lost || !valid) {
      telemetry.observe('data-model', lost ? 'failed' : 'rejected');
      return undefined;
    }
    const callSpan = telemetry.start('data-model');
    try {
      const result = budget.run(meter, () => telemetry.within(callSpan, body));
      telemetry.end(callSpan, 'ok');
      return result;
    } catch {
      // A pure export cannot fail except by trapping: poison the instance.
      lost = true;
      telemetry.end(callSpan, 'failed');
      return undefined;
    }
  };

  return {
    /** Stable data-model name (`data-model-api.name`). */
    name,
    [JOIN_BUDGET]: (joined) => {
      budget = joined;
    },
    [JOIN_CLOCK]: (now) => {
      state.now = now;
    },
    /**
     * `data-model-api.initialize`: declares `declarations` with the
     * provider's placeholder and binds the system variables. The machine
     * calls it once from `start` (through the host, with the host's
     * `_sessionid` and `_ioprocessors`); call it directly only when using
     * the provider without a session.
     */
    initialize(declarations, systemValues) {
      call(isStringList(declarations) && isSystemValues(systemValues), () =>
        api.initialize(declarations, systemValues),
      );
    },
    declare(id) {
      call(typeof id === 'string', () => api.declare(id));
    },
    initializeData(id, initializer) {
      call(typeof id === 'string' && isDataInitializer(initializer), () => api.initializeData(id, initializer));
    },
    setParameter(parameter, value) {
      return call(typeof parameter === 'string' && isDataValue(value), () => api.setParameter(parameter, value));
    },
    /**
     * `update-active-configuration`: removes every `exited` state, then
     * adds every `entered` state.
     */
    updateActiveConfiguration(exited, entered) {
      for (const stateId of exited) state.configuration.delete(stateId);
      for (const stateId of entered) state.configuration.add(stateId);
    },
    evaluateCondition(expression, event) {
      return call(typeof expression === 'string' && isOptionalEvent(event), () =>
        api.evaluateCondition(expression, event),
      );
    },
    evaluateValue(expression, event) {
      return call(typeof expression === 'string' && isOptionalEvent(event), () =>
        api.evaluateValue(expression, event),
      );
    },
    evaluateLocation(location, event) {
      return call(typeof location === 'string' && isOptionalEvent(event), () =>
        api.evaluateLocation(location, event),
      );
    },
    evaluateText(expression, event) {
      return call(typeof expression === 'string' && isOptionalEvent(event), () =>
        api.evaluateText(expression, event),
      );
    },
    /** Renders one `<log expr>` to its log text (SCXML 4.7). */
    evaluateLog(expression, event) {
      return call(typeof expression === 'string' && isOptionalEvent(event), () =>
        api.evaluateLog(expression, event),
      );
    },
    evaluateFormValue(source, location, event) {
      return call(
        typeof source === 'string' && typeof location === 'boolean' && isOptionalEvent(event),
        () => api.evaluateFormValue(source, location, event),
      );
    },
    foreachItems(expression, event) {
      return call(typeof expression === 'string' && isOptionalEvent(event), () =>
        api.foreachItems(expression, event),
      );
    },
    /**
     * Forgets every `<foreach>` in progress (`data-model-api.release-foreach`),
     * as the Wasmtime host forwards it: on the fuel budget, untraced (no span
     * or counter), and a lost instance has nothing left to forget.
     */
    releaseForeach() {
      if (lost) return;
      try {
        budget.run(meter, () => api.releaseForeach());
      } catch {
        // It cannot fail except by trapping: poison the instance.
        lost = true;
      }
    },
    namelistKey(location) {
      return pure(typeof location === 'string', () => api.namelistKey(location));
    },
    eventPayload(entries) {
      return pure(isNamedValues(entries), () => api.eventPayload(entries));
    },
    inlineContent(content) {
      return pure(isInlineBody(content), () => api.inlineContent(content));
    },
    externalData(text) {
      return pure(typeof text === 'string', () => api.externalData(text));
    },
    eventDataField(data, name) {
      return pure(isDataValue(data) && typeof name === 'string', () => api.eventDataField(data, name));
    },
    /**
     * WIT `accepts-event-data` answers a `bool`: `true` for a malformed
     * value, after a trap, and on a lost instance, as the Wasmtime host
     * reads a call that answers nothing.
     */
    acceptsEventData(data) {
      return pure(isDataValue(data), () => api.acceptsEventData(data)) ?? true;
    },
    /**
     * `source` is the WIT `assign-source` (an expression the component
     * evaluates with `_event` bound to `event`, a materialized value for an
     * existing location, or a `<foreach>` value that declares its variable);
     * `placement` is the WIT `assign-placement`. The ECMAScript and null
     * data models define only `{ tag: 'replace-children' }` and answer every
     * other placement with `execution`.
     */
    assign(location, source, placement, event) {
      call(
        typeof location === 'string' &&
          isAssignSource(source) &&
          isAssignPlacement(placement) &&
          isOptionalEvent(event),
        () => api.assign(location, source, placement, event, undefined),
      );
    },
    execute(script, event) {
      call(typeof script === 'string' && isOptionalEvent(event), () => api.execute(script, event));
    },
    /**
     * `data-model-api.snapshot`: read-only dump of the system values and
     * user variables (docs/session-snapshot.md, "Session object");
     * evaluates nothing.
     * A component error is a CapabilityError whose `detail` is the
     * component's static detail.
     */
    snapshot() {
      if (lost) {
        telemetry.observe('data-model', 'failed');
        throw new CapabilityError('platform');
      }
      const callSpan = telemetry.start('data-model');
      try {
        const result = budget.run(meter, () => telemetry.within(callSpan, () => api.snapshot()));
        telemetry.end(callSpan, 'ok');
        return result;
      } catch (thrown) {
        const payload = exportPayload(thrown);
        if (payload === undefined) {
          lost = true;
          telemetry.end(callSpan, 'failed');
          throw new CapabilityError('platform');
        }
        telemetry.end(callSpan, 'rejected');
        const error = new CapabilityError(payload.code ?? 'platform');
        if (typeof payload.detail === 'string') error.detail = payload.detail;
        throw error;
      }
    },
  };
}

/** The bundled ECMAScript data model (`createDataModel('ecmascript', options)`). */
export function createEcmaScriptDataModel(options) {
  return createDataModel('ecmascript', options);
}

/** The bundled XPath data model (`createDataModel('xpath', options)`). */
export function createXPathDataModel(options) {
  return createDataModel('xpath', options);
}

/** The bundled null data model (`createDataModel('null', options)`). */
export function createNullDataModel(options) {
  return createDataModel('null', options);
}
