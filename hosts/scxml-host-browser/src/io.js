// The bundled SCXML Event I/O Processor linked to its own project clock
// (the capability-graph pattern of the Wasmtime session host): one pair per
// machine session.
//
// Both components are transpiled once by jco. The processor imports
// `stateforward:scxml-clock/clock-api`, which this module satisfies with
// the clock instance's own export, so every schedule, cancel, and drain the
// processor performs runs inside the clock component. The host never
// classifies a target, stamps an event, or decides a delivery: it forwards
// the machine's requests with the session's sender facts and executes the
// processor's decision.
//
// Bounds: every call runs on the session's deterministic fuel budget
// (src/fuel.js), capped per call by the session's `fuel` and shared by the
// processor's and the clock's core instances: a call made inside a machine
// command spends that command's budget. A call that
// traps (fuel exhaustion included) discards both instances: that call and
// every later one throw an `IoFault`, which the session turns into
// ScxmlHostError('fuel-exhausted' | 'instance-lost').

import { instantiate as instantiateClock } from '../dist/clock/clock.js';
import { instantiate as instantiateIo } from '../dist/ioprocessor-scxml/ioprocessor-scxml.js';
import { exportPayload } from './errors.js';
import { FuelMeter } from './fuel.js';
import { CAPABILITY_MODULE, clockTelemetryImport, scxmlTelemetryImport } from './telemetry.js';

/** Largest due batch one `drain-due` removes (the WIT maximum). */
export const MAX_DUE_BATCH_ITEMS = 1024;

/** A trap inside the processor or its clock; both instances are discarded. */
export class IoFault {
  /** @param {'fuel-exhausted' | 'instance-lost'} code @param {unknown} [cause] */
  constructor(code, cause) {
    this.code = code;
    this.cause = cause;
  }
}

function clockImports(telemetry) {
  const imports = {};
  for (const [key, value] of [
    ['types', {}],
    ['telemetry', clockTelemetryImport(telemetry)],
  ]) {
    imports[`stateforward:scxml-clock/${key}`] = value;
    imports[`stateforward:scxml-clock/${key}@0.2.0`] = value;
  }
  return imports;
}

function ioImports(telemetry, clockApi) {
  const imports = {};
  for (const [key, value] of [
    ['scxml/types', {}],
    ['scxml/telemetry', scxmlTelemetryImport(telemetry, CAPABILITY_MODULE)],
    ['scxml-clock/types', {}],
    ['scxml-clock/clock-api', clockApi],
  ]) {
    imports[`stateforward:${key}`] = value;
    imports[`stateforward:${key}@0.2.0`] = value;
  }
  return imports;
}

/**
 * Instantiates one clock and one SCXML I/O processor linked to it.
 * @param {{telemetry: import('./telemetry.js').Telemetry, loader: Function, fuel: number, budget: import('./fuel.js').FuelBudget}} options
 *   `fuel` is the per-call cap; calls run on the session's `budget`
 * @returns {Promise<IoLink>}
 * @throws {IoFault} when instantiation traps or fails
 */
export async function createIoLink({ telemetry, loader, fuel, budget }) {
  const meter = new FuelMeter(fuel);
  try {
    const clockApi = (
      await instantiateClock((file) => loader('clock', file), clockImports(telemetry), meter.instantiateCore)
    ).clockApi;
    const ioApi = (
      await instantiateIo(
        (file) => loader('ioprocessor-scxml', file),
        ioImports(telemetry, clockApi),
        meter.instantiateCore,
      )
    ).ioProcessorApi;
    return new IoLink(ioApi, clockApi, meter, budget);
  } catch (cause) {
    throw new IoFault(meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', cause);
  }
}

/**
 * Instantiates one clock and processor pair synchronously from compiled
 * core modules (an invoked session's, built inside its parent's
 * `start-invocation` call).
 * @param {{telemetry: import('./telemetry.js').Telemetry, module: (component: string, file: string) => WebAssembly.Module, fuel: number, budget: import('./fuel.js').FuelBudget}} options
 *   `fuel` is the per-call cap; instantiation and calls run on `budget`
 * @returns {IoLink}
 * @throws {IoFault} when instantiation traps or fails
 */
export function createIoLinkSync({ telemetry, module, fuel, budget }) {
  const meter = new FuelMeter(fuel);
  try {
    return budget.run(meter, () => {
      const clockApi = instantiateClock(
        (file) => module('clock', file),
        clockImports(telemetry),
        meter.instantiateCoreSync,
      ).clockApi;
      const ioApi = instantiateIo(
        (file) => module('ioprocessor-scxml', file),
        ioImports(telemetry, clockApi),
        meter.instantiateCoreSync,
      ).ioProcessorApi;
      return new IoLink(ioApi, clockApi, meter, budget);
    });
  } catch (cause) {
    throw new IoFault(meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', cause);
  }
}

/**
 * Synchronous calls into one processor/clock pair. Component-error results
 * are thrown unchanged (read them with `exportPayload`); traps throw
 * `IoFault`.
 */
export class IoLink {
  #io;
  #clock;
  #meter;
  #budget;
  #fault;

  /** @internal use createIoLink */
  constructor(io, clock, meter, budget) {
    this.#io = io;
    this.#clock = clock;
    this.#meter = meter;
    this.#budget = budget;
  }

  #call(body) {
    if (this.#fault !== undefined) throw this.#fault;
    try {
      return this.#budget.run(this.#meter, body);
    } catch (thrown) {
      if (exportPayload(thrown) !== undefined) throw thrown;
      this.#fault = new IoFault(this.#meter.exhausted() ? 'fuel-exhausted' : 'instance-lost', thrown);
      throw this.#fault;
    }
  }

  typeUri() {
    return this.#call(() => this.#io.typeUri());
  }
  aliases() {
    return this.#call(() => this.#io.aliases());
  }
  sessionLocation(sessionId) {
    return this.#call(() => this.#io.sessionLocation(sessionId));
  }
  classifyTarget(target, sender) {
    return this.#call(() => this.#io.classifyTarget(target, sender));
  }
  dispatch(request, sender) {
    return this.#call(() => this.#io.dispatch(request, sender));
  }
  routeDue(request, sender) {
    return this.#call(() => this.#io.routeDue(request, sender));
  }
  /** Holds one checked delayed BasicHTTP send on the clock (`schedule-basichttp`). */
  scheduleBasichttp(request) {
    this.#call(() => this.#io.scheduleBasichttp(request));
  }
  cancel(sendId, context) {
    this.#call(() => this.#io.cancel(sendId, context));
  }
  drainDue() {
    return this.#call(() => this.#io.drainDue(MAX_DUE_BATCH_ITEMS));
  }
  /** Logical time in milliseconds. */
  now() {
    return Number(this.#call(() => this.#clock.now()));
  }
  /** Earliest pending delayed-send deadline in logical milliseconds, if any. */
  nextDeadline() {
    const deadline = this.#call(() => this.#clock.nextDeadline());
    return deadline === undefined ? undefined : Number(deadline);
  }
  /**
   * Pending delayed sends in clock order (`io-processor-api.snapshot`,
   * read-only). A component error is thrown unchanged.
   */
  snapshot() {
    return this.#call(() => this.#io.snapshot());
  }
  /** Logical time from the clock's read-only `snapshot` (emits no telemetry). */
  snapshotNow() {
    return Number(this.#call(() => this.#clock.snapshot()).nowMs);
  }
  /** Advances logical time by `delta` (> 0) milliseconds. */
  advance(delta) {
    this.#call(() => this.#clock.advance(BigInt(delta)));
  }
}
