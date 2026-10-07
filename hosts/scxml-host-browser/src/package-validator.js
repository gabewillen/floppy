// Package validator components (`stateforward:scxml-validator/package-validator`)
// in the browser host: the counterpart of the Wasmtime host's
// `PackageValidators` (hosts/scxml-host-wasmtime/src/package_validator.rs),
// with the same names, routing, and failure semantics.
//
// * Binding: `PackageValidators.bind(uri, component)` binds an exact package
//   URI (an extension namespace or a service type URI) to a package
//   validator the page authenticated with
//   `authenticatePackageValidatorComponent(bytes, sha256)`: the bytes must
//   hash to the trusted SHA-256, and that digest must be the release artifact
//   of a package validator this package ships transpiled (jco) and
//   fuel-metered (scripts/build.mjs, `dist/<component>/`). A URI binds once.
// * Routing: `createValidator({packageValidators})` answers the validator
//   component's `package-validator-api` import. A call goes to the validator
//   bound to its first target's exact namespace URI, or, for an SCXML
//   `<invoke>`, to its literal `type`; a package with no bound validator
//   answers an empty list. The validator component builds the document
//   context, chooses the targets, and checks every returned span and edit.
// * Bounds: every call instantiates the component afresh (its core modules
//   compiled once, when the validator is created) on its own fuel budget
//   (`withCallFuel`, default DEFAULT_FUEL), as the Wasmtime host runs each
//   call in its own store, so it never spends the validation's fuel. The
//   world imports nothing (its record types arrive through the types-only
//   `types` and `document` interfaces), so a package validator gets no I/O.
//   A trap (fuel exhaustion included) or an `err` answers the validator an
//   `err`, which it reports as `PKG002`.
//
// Telemetry: one `package-validator-validate` operation per call that
// reaches a bound component, with its closed outcome. Package URIs, the
// document, and diagnostics never become attributes.

import { ImportFailure, ScxmlHostError } from './errors.js';
import { DEFAULT_FUEL, FuelBudget, FuelMeter, isFuel } from './fuel.js';
import { sha256Hex } from './sha256.js';

/**
 * The package validators this package ships, by `dist/` component name:
 * their jco bindings are imported only when a page binds one.
 */
const SHIPPED = Object.freeze({
  'package-validator-example': () => import('../dist/package-validator-example/package-validator-example.js'),
});

const SCXML_NAMESPACE = 'http://www.w3.org/2005/07/scxml';
const VALIDATOR_PACKAGE = 'stateforward:scxml-validator';
const FAILED = Object.freeze({ version: 'v1', code: 'invalid-request' });

let identities;

/** `dist/identity.js`: each shipped component's release digest. */
function loadIdentities() {
  identities ??= import('../dist/identity.js').then(
    (module) => module,
    () => ({ RELEASE_DIGESTS: {} }),
  );
  return identities;
}

/**
 * Authenticates one package validator component: `bytes` must hash to
 * `sha256` (64 lowercase hex characters, the trust anchor), and that digest
 * must be the release artifact of a package validator this package ships.
 * @param {Uint8Array} bytes the exact component bytes
 * @param {string} sha256 the trusted SHA-256
 * @returns {Promise<{kind: 'package-validator-component', sha256: string, component: string}>}
 * @throws {ScxmlHostError} `invalid-argument` for malformed arguments,
 *   `artifact-digest-mismatch` when the bytes do not hash to `sha256`,
 *   `artifact-kind-mismatch` when no shipped package validator has it
 */
export async function authenticatePackageValidatorComponent(bytes, sha256) {
  if (!(bytes instanceof Uint8Array) || typeof sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(sha256)) {
    throw new ScxmlHostError('invalid-argument');
  }
  if (sha256Hex(bytes) !== sha256) throw new ScxmlHostError('artifact-digest-mismatch');
  const { RELEASE_DIGESTS } = await loadIdentities();
  const component = Object.keys(SHIPPED).find((name) => RELEASE_DIGESTS[name] === sha256);
  if (component === undefined) throw new ScxmlHostError('artifact-kind-mismatch');
  return Object.freeze({ kind: 'package-validator-component', sha256, component });
}

/** Package validator components keyed by exact package URI. */
export class PackageValidators {
  #bindings = new Map();
  #callFuel = DEFAULT_FUEL;

  /**
   * Binds `component` to the exact package URI `uri`.
   * @param {string} uri an extension namespace URI or a service type URI
   * @param {object} component an authenticatePackageValidatorComponent result
   * @returns {PackageValidators} this
   * @throws {ScxmlHostError} `artifact-kind-mismatch` for anything but an
   *   authenticated package validator; `namespace-binding` for a URI that is
   *   not a non-empty string or is already bound
   */
  bind(uri, component) {
    if (component?.kind !== 'package-validator-component' || !Object.hasOwn(SHIPPED, component.component)) {
      throw new ScxmlHostError('artifact-kind-mismatch');
    }
    if (typeof uri !== 'string' || uri.length === 0 || this.#bindings.has(uri)) {
      throw new ScxmlHostError('namespace-binding');
    }
    this.#bindings.set(uri, component);
    return this;
  }

  /**
   * Sets the fuel cap of one package validator call.
   * @param {number} fuel a positive safe integer
   * @returns {PackageValidators} this
   * @throws {ScxmlHostError} `invalid-argument`
   */
  withCallFuel(fuel) {
    if (!isFuel(fuel)) throw new ScxmlHostError('invalid-argument');
    this.#callFuel = fuel;
    return this;
  }

  /** The bindings and call fuel the validator links (internal). */
  parts() {
    return { bindings: new Map(this.#bindings), callFuel: this.#callFuel };
  }
}

/** Whether `value` is a PackageValidators (createValidator's option). */
export function isPackageValidators(value) {
  return value === undefined || value instanceof PackageValidators;
}

/** The empty types-only imports of a package validator instance. */
function typeImports() {
  const imports = {};
  for (const name of ['types', 'document']) {
    imports[`${VALIDATOR_PACKAGE}/${name}`] = {};
    imports[`${VALIDATOR_PACKAGE}/${name}@0.2.0`] = {};
  }
  return imports;
}

/** One bound validator, its core modules compiled so calls instantiate synchronously. */
async function prepare(component, loader, callFuel) {
  const bindings = await SHIPPED[component.component]();
  const compiled = new Map();
  const recording = (file) =>
    Promise.resolve(loader(component.component, file)).then((module) => {
      compiled.set(file, module);
      return module;
    });
  const meter = new FuelMeter(callFuel);
  await bindings.instantiate(recording, typeImports(), meter.instantiateCore);
  return {
    bindings,
    module: (file) => {
      const found = compiled.get(file);
      if (found === undefined) throw new TypeError('core module was not preloaded');
      return found;
    },
  };
}

/** The package a call with `targets` belongs to (the Wasmtime host's `route`). */
function route(document, targets) {
  const element = document?.elements?.[targets?.[0]];
  const namespace = element?.name?.namespaceUri;
  if (namespace === undefined) return undefined;
  if (namespace === SCXML_NAMESPACE && element.name.localName === 'invoke') {
    return element.attributes.find(
      (attribute) => attribute.name.namespaceUri === undefined && attribute.name.localName === 'type',
    )?.value;
  }
  return namespace;
}

/**
 * The validator component's `package-validator-api` import over
 * `validators` (a PackageValidators, or undefined: every package answers an
 * empty list).
 */
export async function packageValidatorImport(validators, loader, telemetry) {
  const prepared = new Map();
  let callFuel = DEFAULT_FUEL;
  if (validators !== undefined) {
    const parts = validators.parts();
    callFuel = parts.callFuel;
    for (const [uri, component] of parts.bindings) prepared.set(uri, await prepare(component, loader, callFuel));
  }
  return {
    validate(document, targets) {
      const bound = prepared.get(route(document, targets));
      if (bound === undefined) return [];
      const span = telemetry.start('package-validator-validate');
      const meter = new FuelMeter(callFuel);
      try {
        const diagnostics = new FuelBudget(callFuel).run(meter, () =>
          bound.bindings
            .instantiate(bound.module, typeImports(), meter.instantiateCoreSync)
            .packageValidatorApi.validate(document, targets),
        );
        telemetry.end(span, 'ok');
        return diagnostics;
      } catch {
        telemetry.end(span, 'failed');
        throw new ImportFailure(FAILED);
      }
    },
  };
}
