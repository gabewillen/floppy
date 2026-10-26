// Core Wasm module loading for the transpiled components.
//
// jco emits each component as ES module bindings plus several core Wasm
// modules (`<name>.core*.wasm`). A loader maps `(component, file)` to a
// compiled `WebAssembly.Module`. Modules are compiled once per loader and
// reused by every session; each session still gets its own instances,
// memories, and imports.

import { Telemetry, isTelemetryOption } from './telemetry.js';

/**
 * Components shipped in `dist/`. `snapshot-codec` is optional: it is loaded
 * only by the first session snapshot or snapshot diff; `validator` only by
 * createValidator; and the extension components (`extension-example`, the
 * example namespace, and `scx`, the `scx` namespace) only when a page binds
 * one. A build may ship more extension components (`--extensions`, listed
 * in `dist/extensions.js`) and ships invoked-service components
 * (`service-example` and those `--services` adds, listed
 * in `dist/services.js`); the loader accepts those names too.
 */
export const COMPONENTS = Object.freeze([
  'machine',
  'data-model-ecmascript',
  'data-model-xpath',
  'data-model-null',
  'ioprocessor-scxml',
  'ioprocessor-basichttp',
  'clock',
  'compiler',
  'snapshot-codec',
  'validator',
  'extension-example',
  'package-validator-example',
  'scx',
]);

const CORE_FILE = /^[a-z-]+\.core[0-9]*\.wasm$/;

/**
 * The extension components this build ships (`dist/extensions.js`, written
 * by scripts/build.mjs): `{name: () => import(bindings)}`, frozen; empty
 * when `dist/` is not built.
 * @returns {Promise<Readonly<Record<string, () => Promise<object>>>>}
 */
export function shippedExtensions() {
  return import('../dist/extensions.js').then(
    (module) => module.EXTENSIONS,
    () => Object.freeze({}),
  );
}

/**
 * The invoked-service components this build ships (`dist/services.js`,
 * written by scripts/build.mjs): `{name: () => import(bindings)}`, frozen;
 * empty when `dist/` is not built.
 * @returns {Promise<Readonly<Record<string, () => Promise<object>>>>}
 */
export function shippedServices() {
  return import('../dist/services.js').then(
    (module) => module.SERVICES,
    () => Object.freeze({}),
  );
}

/** The extension and service components named at build time. */
const EXTENSION_COMPONENTS = [...Object.keys(await shippedExtensions()), ...Object.keys(await shippedServices())];

/**
 * Loader that fetches `<baseUrl>/<component>/<file>` and compiles it with
 * `WebAssembly.compileStreaming` (browsers serve `.wasm` as
 * `application/wasm`). The default base is this package's `dist/`.
 * @param {URL | string} [baseUrl]
 * @param {{telemetry?: object}} [options] OpenTelemetry providers, as for
 *   createSession
 * @returns {(component: string, file: string) => Promise<WebAssembly.Module>}
 * @throws {TypeError} for a malformed `baseUrl` or `options.telemetry`
 */
export function fetchCoreModules(baseUrl = new URL('../dist/', import.meta.url), options) {
  const base = new URL(baseUrl);
  return cachedCoreModules(
    (component, file) => WebAssembly.compileStreaming(fetch(new URL(`${component}/${file}`, base))),
    options,
  );
}

/**
 * Wraps a `(component, file) => Module | Promise<Module> | BufferSource`
 * source with validation of the requested name and a per-loader cache.
 * Every request is one `load-core-module` host operation: `rejected` for an
 * unknown name, `failed` when the source or compilation fails, else `ok`.
 * @param {Function} source
 * @param {{telemetry?: object}} [options] OpenTelemetry providers, as for
 *   createSession
 * @throws {TypeError} for a malformed `options.telemetry`
 */
export function cachedCoreModules(source, options) {
  const telemetryOption = options?.telemetry;
  const telemetry = new Telemetry(telemetryOption);
  if (!isTelemetryOption(telemetryOption)) {
    telemetry.observe('load-core-module', 'rejected');
    throw new TypeError('telemetry option must be {tracerProvider?, meterProvider?}');
  }
  const cache = new Map();
  return (component, file) => {
    if (!(COMPONENTS.includes(component) || EXTENSION_COMPONENTS.includes(component)) || !CORE_FILE.test(file)) {
      telemetry.observe('load-core-module', 'rejected');
      return Promise.reject(new TypeError('unknown core module request'));
    }
    const key = `${component}/${file}`;
    let compiled = cache.get(key);
    if (compiled === undefined) {
      const span = telemetry.start('load-core-module');
      compiled = Promise.resolve()
        .then(() => source(component, file))
        .then((value) => (value instanceof WebAssembly.Module ? value : WebAssembly.compile(value)));
      compiled.then(
        () => telemetry.end(span, 'ok'),
        () => {
          // A failed compilation is not cached, so a later call can retry.
          cache.delete(key);
          telemetry.end(span, 'failed');
        },
      );
      cache.set(key, compiled);
    } else {
      telemetry.observe('load-core-module', 'ok');
    }
    return compiled;
  };
}
