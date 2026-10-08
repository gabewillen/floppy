// In-browser SCXML validation through the bundled validator component
// (transpiled once by jco, authenticated and fuel-metered like the
// compiler). The component parses and validates; this adapter builds the
// request and returns the canonical `scxml.report/v3` report bytes it
// produces, unchanged, as the sole decision.
//
// The validator's `expression-syntax` import is linked, as the Wasmtime
// host's ComponentValidator links it, to one bundled data model's
// `check-syntax` and `check-effects` (the page chooses it with
// `dataModel`; the host never chooses a data model for a document). Only
// documents whose `datamodel` equals that data model's name exactly have
// their expressions checked; without one (or with the null data model, which
// has no expression syntax) every check is declined `unavailable` and the
// validator reports other data models' expressions with its `LINT010` note.
// A data-model `payload-too-large` declines `too-large`, `invalid-input`
// `too-complex`, and anything else `unavailable`.
//
// Bounds: every validation runs on its own deterministic fuel budget
// (src/fuel.js, `fuel`, default DEFAULT_COMPILE_FUEL); a validation that
// traps (fuel exhaustion included) discards the instance: it throws
// `fuel-exhausted` or `instance-lost`, and every later one `instance-lost`.

import { instantiate } from '../dist/validator/validator.js';
import { ImportFailure, ScxmlCompileError, ScxmlHostError, exportPayload } from './errors.js';
import { compileRequestParts, isCompileRequestOptions } from './compiler.js';
import { DEFAULT_COMPILE_FUEL, FuelBudget, FuelMeter, isFuel } from './fuel.js';
import { fetchCoreModules } from './loader.js';
import { isPackageValidators, packageValidatorImport } from './package-validator.js';
import { GUEST_MODULES, Telemetry, compilerTelemetryImport, isTelemetryOption } from './telemetry.js';

let defaultLoader;

/** A data-model failure as the validator's closed `declined` value. */
function declined(thrown) {
  switch (thrown?.code) {
    case 'payload-too-large':
      return new ImportFailure('too-large');
    case 'invalid-input':
      return new ImportFailure('too-complex');
    default:
      return new ImportFailure('unavailable');
  }
}

/** The `expression-syntax` import over one bundled data model, or none. */
function expressionSyntax(dataModel) {
  const name = typeof dataModel?.name === 'string' && dataModel.name !== 'null' ? dataModel.name : undefined;
  const forward = (method, args) => {
    if (name === undefined) throw new ImportFailure('unavailable');
    try {
      return dataModel[method](...args);
    } catch (thrown) {
      throw declined(thrown);
    }
  };
  return {
    supportedDataModel: () => name,
    checkSyntax: (kind, source) => forward('checkSyntax', [kind, source]),
    checkEffects: (fragments, scripts) => forward('checkEffects', [fragments, scripts]),
  };
}

/**
 * Instantiates one validator.
 * @param {object} [options]
 * @param {object} [options.dataModel] a bundled data model
 *   (createEcmaScriptDataModel, createXPathDataModel) whose expression
 *   syntax the validator checks; absent: no expression is checked
 * @param {object} [options.telemetry] OpenTelemetry providers
 * @param {Function} [options.loadCoreModule] `(component, file) => Module`
 * @param {number} [options.fuel] per-validation fuel budget (default
 *   DEFAULT_COMPILE_FUEL)
 * @param {object} [options.packageValidators] a PackageValidators: the
 *   package validator components that answer the validator's
 *   `package-validator-api` import; absent, every package answers none
 * @throws {ScxmlHostError} `invalid-argument` for a malformed option,
 *   `fuel-exhausted` or `instance-lost` when instantiation traps
 */
export async function createValidator(options) {
  const {
    dataModel,
    telemetry: telemetryOption,
    loadCoreModule,
    fuel = DEFAULT_COMPILE_FUEL,
    packageValidators,
  } = options ?? {};
  const telemetry = new Telemetry(telemetryOption);
  if (
    !isFuel(fuel) ||
    !isTelemetryOption(telemetryOption) ||
    (dataModel !== undefined &&
      (dataModel === null || typeof dataModel.checkSyntax !== 'function' || typeof dataModel.checkEffects !== 'function')) ||
    (loadCoreModule !== undefined && typeof loadCoreModule !== 'function') ||
    !isPackageValidators(packageValidators)
  ) {
    telemetry.observe('validate', 'rejected');
    throw new ScxmlHostError('invalid-argument');
  }
  const loader = loadCoreModule ?? (defaultLoader ??= fetchCoreModules());
  const imports = {};
  for (const [name, value] of [
    ['types', {}],
    ['document', {}],
    ['telemetry', compilerTelemetryImport(telemetry, GUEST_MODULES.validator)],
    ['expression-syntax', expressionSyntax(dataModel)],
    ['package-validator-api', await packageValidatorImport(packageValidators, loader, telemetry)],
  ]) {
    imports[`stateforward:scxml-validator/${name}`] = value;
    imports[`stateforward:scxml-validator/${name}@0.2.0`] = value;
  }
  const meter = new FuelMeter(fuel);
  const budget = new FuelBudget(fuel);
  let api;
  try {
    api = (await instantiate((file) => loader('validator', file), imports, meter.instantiateCore)).validatorApi;
  } catch (cause) {
    telemetry.observe('validate', 'failed');
    if (meter.exhausted()) throw new ScxmlHostError('fuel-exhausted');
    throw new ScxmlHostError('instance-lost', { cause });
  }
  let lost = false;
  return {
    /**
     * Validates one SCXML document; returns `{report}`, the canonical
     * `scxml.report/v3` validator report.
     * @param {string} source SCXML text
     * @param {object} [validateOptions] `sourceName` (default
     *   'document.scxml'), `schemaPolicy` (default 'strict', as `scx
     *   validate`), `packages` (ExtensionPackage[]), `eventSchemas`
     *   (`{uri, bytes}[]`)
     */
    async validate(source, validateOptions) {
      if (lost) {
        telemetry.observe('validate', 'rejected');
        throw new ScxmlHostError('instance-lost');
      }
      const { sourceName = 'document.scxml', schemaPolicy = 'strict' } = validateOptions ?? {};
      if (
        typeof source !== 'string' ||
        typeof sourceName !== 'string' ||
        !isCompileRequestOptions(validateOptions, schemaPolicy) ||
        !(validateOptions?.packages ?? []).every((pkg) =>
          ['validatorArtifact', 'validatorSha256'].every(
            (field) => pkg[field] === undefined || typeof pkg[field] === 'string',
          ),
        )
      ) {
        telemetry.observe('validate', 'rejected');
        throw new ScxmlHostError('invalid-argument');
      }
      const { schemas, packages: records, eventSchemas } = compileRequestParts(validateOptions);
      // The validator's package record also carries the optional package
      // validator fields.
      const packages = records.map((pkg, index) => ({
        ...pkg,
        validatorArtifact: validateOptions.packages[index].validatorArtifact,
        validatorSha256: validateOptions.packages[index].validatorSha256,
      }));
      const span = telemetry.start('validate');
      let report;
      try {
        report = budget.run(meter, () =>
          telemetry.within(span, () =>
            api.validate({
              sourceName,
              source,
              policy: schemaPolicy,
              schemas,
              packages,
              eventSchemas,
              traceContext: undefined,
            }),
          ),
        );
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
      telemetry.end(span, 'ok');
      return { report };
    },
  };
}
