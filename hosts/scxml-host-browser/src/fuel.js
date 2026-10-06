// Deterministic fuel for the jco-transpiled components.
//
// Browsers expose no Wasm fuel, epoch, or interrupt, so `scripts/build.mjs`
// runs every transpiled core module through the pinned in-workspace
// `scxml-wasm-meter` tool. Each metered module exports one mutable i64 global
// (FUEL_EXPORT) and spends one unit of it at every function entry and every
// loop iteration; a charge that finds it at zero traps (`unreachable`).
//
// One component is several core instances, each with its own counter (the
// guest's code, plus jco's small adapter and trampoline modules). A
// FuelMeter holds one component's counters; a FuelBudget is the budget of
// one host call, shared by every component that call reaches (the browser
// counterpart of the Wasmtime host's `call_budget`).
//
// Contract:
// * Every core instance of a component is created through one FuelMeter's
//   `instantiateCore` (or `instantiateCoreSync`); a module without the fuel
//   export fails instantiation closed, so an unmetered build can never run.
// * Every call into a component runs inside `budget.run(meter, body)`. The
//   outermost run (no component call of that budget below it on the stack)
//   starts the host call's budget at its configured fuel. Entering a
//   component grants it the smaller of its meter's own per-call cap and what
//   is left of the budget; calling out of it into a nested component, and
//   leaving it, charges what it ran; returning into it grants it the smaller
//   of its own remaining cap and what is left. So a nested data-model, I/O
//   processor, clock, BasicHTTP, compiler, or child-machine call spends the
//   budget of the machine command that made it, never a fresh one.
// * A grant sets every counter of the component to it; a component is
//   charged the largest count any one of its core instances spent since the
//   grant. Hence, within one host call, the charges of the components sum to
//   at most the configured fuel, and no single core instance executes more
//   than that many charges. The counts depend only on the executed path:
//   exhaustion is deterministic for identical inputs, budgets, and builds.
// * After a trap, `exhausted()` is true exactly when some counter of that
//   component reached zero; the host then reports `fuel-exhausted` and
//   never enters the instance again. A nested component that exhausts the
//   shared budget leaves its caller a zero grant, so the caller traps at its
//   next charge and the host call fails with `fuel-exhausted`.
// * Code between charges is straight-line code bounded by module size, and
//   bulk memory operations are bounded by memory size, so a budget bounds
//   the call's work, not its wall-clock time.

/** Export name of the injected fuel global (matches scxml-wasm-meter). */
export const FUEL_EXPORT = 'scxml:fuel';

/**
 * Default fuel of one host call, in charges (see the module comment), shared
 * by every component the call reaches; also the default per-call cap of the
 * bundled data models and the instantiation budget `scripts/build.mjs` bakes
 * into every metered module.
 */
export const DEFAULT_FUEL = 75_000_000;

/**
 * Default per-compile cap. Compilation is linear-ish in document size
 * (about 24 million charges for a 160 KB, 2 000-state document), so the
 * default admits documents of several MiB. Inside a session a compile is
 * also bounded by what is left of the host call's budget.
 */
export const DEFAULT_COMPILE_FUEL = 4_000_000_000;

/** Largest accepted budget (the global is a signed 64-bit integer). */
export const MAX_FUEL = Number.MAX_SAFE_INTEGER;

/** True when `value` is an accepted fuel budget. */
export function isFuel(value) {
  return Number.isSafeInteger(value) && value > 0 && value <= MAX_FUEL;
}

/** Fuel counters of one component's core instances, with its per-call cap. */
export class FuelMeter {
  #counters = [];
  /** The fuel of the active grant, or undefined outside a call. */
  #grant;

  /** @param {number} cap per-call charges (see isFuel) */
  constructor(cap) {
    /** The component's own per-call cap (BigInt). */
    this.cap = BigInt(cap);
    /** What was left of the cap when the last call into it returned. */
    this.lastLeft = this.cap;
    /**
     * The cap a suspended call left (set by the host): its continuation
     * runs on it, so a call and its continuations share one call's cap.
     */
    this.suspendedLeft = undefined;
  }

  /** `instantiateCore` for jco's `instantiate`: records each fuel counter. */
  instantiateCore = async (module, imports) => this.#record(await WebAssembly.instantiate(module, imports));

  /**
   * Synchronous `instantiateCore` over an already compiled module (an
   * invoked session built inside its parent's capability call).
   */
  instantiateCoreSync = (module, imports) => this.#record(new WebAssembly.Instance(module, imports));

  #record(instance) {
    const counter = instance.exports[FUEL_EXPORT];
    if (!(counter instanceof WebAssembly.Global)) {
      throw new TypeError('core module is not fuel-metered');
    }
    // Instantiated inside a call (synchronously, under a grant): the new
    // instance runs on that grant, like the component's other instances.
    if (this.#grant !== undefined) counter.value = this.#grant;
    this.#counters.push(counter);
    return instance;
  }

  /** Sets every counter to `grant` (BigInt). */
  grant(grant) {
    this.#grant = grant;
    for (const counter of this.#counters) counter.value = grant;
  }

  /** The largest count one core instance spent since the last grant. */
  spent() {
    let spent = 0n;
    for (const counter of this.#counters) {
      const used = this.#grant - counter.value;
      if (used > spent) spent = used;
    }
    return spent;
  }

  /** Ends the active grant (the counters keep their values). */
  release() {
    this.#grant = undefined;
  }

  /** True when some counter ran out (the last trap was fuel exhaustion). */
  exhausted() {
    return this.#counters.some((counter) => counter.value === 0n);
  }
}

/**
 * The fuel budget of one host call, shared by every component the call
 * reaches (see the module comment). Single-threaded; `run` nests.
 */
export class FuelBudget {
  #fuel;
  #remaining = 0n;
  /** Active component calls, innermost last: `{meter, left}` (`left`: its remaining cap). */
  #frames = [];
  /** Active `hold` scopes. */
  #held = 0;

  /** @param {number} fuel charges of one host call (see isFuel) */
  constructor(fuel) {
    this.#fuel = BigInt(fuel);
  }

  /**
   * Runs `body` as one host call: the component calls it makes, one after
   * another or nested, share one budget (a machine call that suspends at a
   * deferred invocation request, the calls the host makes to carry the
   * request out, and the call's continuation spend what the uninterrupted
   * call would have).
   */
  hold(body) {
    if (this.#held === 0 && this.#frames.length === 0) this.#remaining = this.#fuel;
    this.#held += 1;
    try {
      return body();
    } finally {
      this.#held -= 1;
    }
  }

  /** Runs `body` as one call into `meter`'s component on this budget. */
  run(meter, body) {
    const outer = this.#frames.at(-1);
    if (outer === undefined) {
      if (this.#held === 0) this.#remaining = this.#fuel;
    } else this.#charge(outer);
    const frame = { meter, left: meter.suspendedLeft ?? meter.cap };
    meter.suspendedLeft = undefined;
    this.#grant(frame);
    this.#frames.push(frame);
    try {
      return body();
    } finally {
      this.#frames.pop();
      this.#charge(frame);
      meter.lastLeft = frame.left;
      meter.release();
      if (outer !== undefined) this.#grant(outer);
    }
  }

  /** Charges what `frame`'s component ran since its grant. */
  #charge(frame) {
    const spent = frame.meter.spent();
    frame.left -= spent;
    this.#remaining = this.#remaining > spent ? this.#remaining - spent : 0n;
  }

  /** Grants `frame` the smaller of its own remaining cap and the budget's. */
  #grant(frame) {
    frame.meter.grant(frame.left < this.#remaining ? frame.left : this.#remaining);
  }
}
