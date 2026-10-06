// In-browser SCXML -> Machine IR through the bundled compiler component
// (transpiled once by jco). The component parses, validates, and lowers;
// this adapter builds the request, returns the canonical report bytes as
// the sole decision, and checks that any returned IR matches the byte
// length and SHA-256 the report records before handing it out.
//
// Every compile runs on a deterministic fuel budget (src/fuel.js): its own
// per-compile budget, or, for a session's automatic `<invoke>` compiler, the
// session's host-call budget capped per compile (src/invoke.js). A compile
// that traps (fuel exhaustion included) discards the instance: it throws
// `fuel-exhausted` or `instance-lost`, and every later compile throws
// `instance-lost`.

import { instantiate } from '../dist/compiler/compiler.js';
import { ScxmlCompileError, ScxmlHostError, exportPayload } from './errors.js';
import { DEFAULT_COMPILE_FUEL, FuelBudget, FuelMeter, isFuel } from './fuel.js';
import { fetchCoreModules } from './loader.js';
import { sha256Hex } from './sha256.js';
import { Telemetry, compilerTelemetryImport, isTelemetryOption } from './telemetry.js';

/** Largest canonical report this adapter decodes for its integrity check. */
export const MAX_REPORT_BYTES = 64 * 1024 * 1024;
/** Compiler report statuses (`scxml.report/v3` `compiler.status`). */
export const COMPILE_STATUSES = Object.freeze(['emitted', 'rejected']);
/** WIT `schema-policy` values (stateforward:scxml-compiler/types@0.2.0). */
export const SCHEMA_POLICIES = Object.freeze(['strict', 'lax', 'off']);

let defaultLoader;

/** The integrity fields of a canonical `scxml.report/v3` compiler report. */
function reportOutput(report) {
  if (report.byteLength > MAX_REPORT_BYTES) throw new ScxmlHostError('report-mismatch');
  let envelope;
  try {
    envelope = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(report));
  } catch (cause) {
    throw new ScxmlHostError('report-mismatch', { cause });
  }
  if (
    envelope?.version !== 'scxml.report/v3' ||
    envelope.kind !== 'compiler' ||
    !COMPILE_STATUSES.includes(envelope.compiler?.status)
  ) {
    throw new ScxmlHostError('report-mismatch');
  }
  return { status: envelope.compiler?.status, output: envelope.compiler?.output };
}

/**
 * Instantiates one compiler.
 * @param {object} [options]
 * @param {object} [options.telemetry] OpenTelemetry providers
 *   `{tracerProvider?, meterProvider?}`; omitted ones are the global API
 *   providers
 * @param {Function} [options.loadCoreModule] `(component, file) => Module`
 * @param {number} [options.fuel] per-compile fuel budget (default
 *   DEFAULT_COMPILE_FUEL)
 * @throws {ScxmlHostError} `invalid-argument` for a malformed option,
 *   `fuel-exhausted` or `instance-lost` when instantiation traps
 */
export async function createCompiler(options) {
  const compiler = await openCompiler(options);
  return {
    /**
     * Compiles one SCXML document to lowered Machine IR.
     * @param {string} source SCXML text
     * @param {{sourceName?: string, schemaPolicy?: 'strict' | 'lax' | 'off'}} [compileOptions]
     * @returns {Promise<{status: 'emitted' | 'rejected', report: Uint8Array, ir: Uint8Array | undefined}>}
     *   `report` is the canonical compiler report (render it with a
     *   reporter); `ir` is present only when the report says `emitted`.
     * @throws {ScxmlHostError} `invalid-argument` (non-string `source` or
     *   `sourceName`, or a `schemaPolicy` outside SCHEMA_POLICIES; the
     *   instance stays usable), `report-mismatch`, `fuel-exhausted`, or
     *   `instance-lost` (a trap; this and every later compile)
     * @throws {ScxmlCompileError} the compiler's closed error for a rejected request
     */
    async compile(source, compileOptions) {
      return compiler.compile(source, compileOptions);
    },
  };
}

/**
 * Instantiates one compiler whose `compile` returns synchronously, with the
 * `createCompiler` contract otherwise: the automatic `<invoke>` compiler
 * (src/invoke.js) calls it inside the machine's synchronous
 * `compile-invocation`. Internal; not a package export. `budget` (a
 * session's FuelBudget) charges each compile to the session's host call,
 * capped by `fuel`; without it each compile is its own host call.
 */
export async function openCompiler(options) {
  const { telemetry: telemetryOption, loadCoreModule, fuel = DEFAULT_COMPILE_FUEL, budget: shared } = options ?? {};
  const telemetry = new Telemetry(telemetryOption);
  if (!isFuel(fuel) || !isTelemetryOption(telemetryOption)) {
    telemetry.observe('compile', 'rejected');
    throw new ScxmlHostError('invalid-argument');
  }
  const imports = {};
  for (const [name, value] of [
    ['types', {}],
    ['telemetry', compilerTelemetryImport(telemetry)],
  ]) {
    imports[`stateforward:scxml-compiler/${name}`] = value;
    imports[`stateforward:scxml-compiler/${name}@0.2.0`] = value;
  }
  const loader = loadCoreModule ?? (defaultLoader ??= fetchCoreModules());
  const meter = new FuelMeter(fuel);
  const budget = shared ?? new FuelBudget(fuel);
  let api;
  try {
    api = (await instantiate((file) => loader('compiler', file), imports, meter.instantiateCore))
      .compilerApi;
  } catch (cause) {
    telemetry.observe('compile', 'failed');
    if (meter.exhausted()) throw new ScxmlHostError('fuel-exhausted');
    throw new ScxmlHostError('instance-lost', { cause });
  }
  let lost = false;

  return {
    /** `createCompiler().compile`, returning its result synchronously. */
    compile(source, compileOptions) {
      if (lost) {
        telemetry.observe('compile', 'rejected');
        throw new ScxmlHostError('instance-lost');
      }
      const { sourceName = 'document.scxml', schemaPolicy = 'off' } = compileOptions ?? {};
      if (
        typeof source !== 'string' ||
        typeof sourceName !== 'string' ||
        !SCHEMA_POLICIES.includes(schemaPolicy)
      ) {
        telemetry.observe('compile', 'rejected');
        throw new ScxmlHostError('invalid-argument');
      }
      const span = telemetry.start('compile');
      let result;
      try {
        result = budget.run(meter, () => telemetry.within(span, () => api.compile({
          sourceName,
          source,
          schemas: [],
          packages: [],
          // Browser hosts resolve `<script src>` at document load through
          // the page's `loadScript` capability, so nothing is pinned here.
          scripts: [],
          // The page supplies no schema files: a document compiled here
          // describes its event data with inline `event:schema` JSON or
          // `event:schemaexpr` (a schema-file reference is `SCXML088`).
          eventSchemas: [],
          options: { schemaPolicy, requestedArtifact: 'lowered-ir', scriptResolution: 'runtime' },
          machineTemplate: undefined,
          traceContext: undefined,
        })));
      } catch (thrown) {
        const payload = exportPayload(thrown);
        telemetry.end(span, payload === undefined ? 'failed' : 'rejected');
        if (payload === undefined) {
          lost = true;
          if (meter.exhausted()) throw new ScxmlHostError('fuel-exhausted');
          throw new ScxmlHostError('instance-lost', { cause: thrown });
        }
        throw new ScxmlCompileError(payload.code);
      }
      const report = result.reportArtifact;
      let decoded;
      try {
        decoded = reportOutput(report);
      } catch (error) {
        telemetry.end(span, 'failed');
        throw error;
      }
      const { status, output } = decoded;
      const ir = result.artifactBytes;
      if (ir === undefined) {
        if (status === 'emitted') {
          telemetry.end(span, 'failed');
          throw new ScxmlHostError('report-mismatch');
        }
        telemetry.end(span, 'rejected');
        return { status, report, ir: undefined };
      }
      const digest = sha256Hex(ir);
      if (
        status !== 'emitted' ||
        output?.artifact_kind !== 'lowered-ir' ||
        output.byte_length !== ir.byteLength ||
        output.sha256 !== digest
      ) {
        telemetry.end(span, 'failed');
        throw new ScxmlHostError('report-mismatch');
      }
      telemetry.end(span, 'ok');
      return { status, report, ir };
    },
  };
}
