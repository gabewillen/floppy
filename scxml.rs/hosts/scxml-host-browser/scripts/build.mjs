// One-time build: authenticate the release guest components produced by
// `scripts/verify-wasm-capabilities.sh` against their artifact manifest and
// transpile each exactly once with the pinned jco transpiler
// (`@bytecodealliance/jco-transpile`, the library behind `jco transpile`)
// into `dist/<name>/` (ES module bindings plus core Wasm modules), then meter
// every core module with the in-workspace `scxml-wasm-meter` tool so the
// browser host can bound each call with deterministic fuel (src/fuel.js).
//
// Usage: node scripts/build.mjs --artifacts <dir-with-manifest.tsv>
//          --meter <scxml-wasm-meter binary> [--out dist]
//
// Contract:
// * every transpiled component is listed in `manifest.tsv` and its bytes hash
//   to the recorded SHA-256 and byte length; anything else fails closed;
// * the transpiled output imports no WASI shim (the components are
//   wasi-virt virtualized, so their only imports are stateforward:* WIT);
// * the transpiler version is exactly the one pinned in package.json;
// * every emitted core module is metered with DEFAULT_FUEL as its
//   instantiation budget and exports the FUEL_EXPORT global (checked by
//   compiling the metered bytes), so the host never loads an unmetered module;
// * `dist/build-manifest.json` records the transpiler version, each source digest,
//   and the digest of every emitted file (sorted), so a rebuild can be
//   compared byte-for-byte;
// * `dist/identity.js` exports each component's release-artifact SHA-256
//   (the manifest digest the build authenticated), which the session
//   snapshot lists in `snapshot.components` (docs/session-snapshot.md,
//   "`snapshot`");
// * the validator (`scxml_validator`) and the extension components the
//   package ships (`scxml_extension_example`, `scx`) are built like every
//   other component; `identity.js` also lists each component's imported
//   interfaces;
// * the optional session snapshot codec (`scxml_snapshot_codec`) is built
//   into `dist/snapshot-codec/` like every other component and loaded by the
//   host only on first use (src/snapshot.js); a deployment may omit that
//   directory, and snapshots then fail with `codec-unavailable`.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { transpile } from '@bytecodealliance/jco-transpile';

import { DEFAULT_FUEL, FUEL_EXPORT } from '../src/fuel.js';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** Components transpiled for the browser host, keyed by output name. */
const COMPONENTS = [
  { name: 'machine', artifact: 'scxml_machine', file: 'scxml_machine.virtualized.wasm' },
  {
    name: 'data-model-ecmascript',
    artifact: 'scxml_data_model_ecmascript',
    file: 'scxml_data_model_ecmascript.virtualized.wasm',
  },
  {
    name: 'data-model-xpath',
    artifact: 'scxml_data_model_xpath',
    file: 'scxml_data_model_xpath.virtualized.wasm',
  },
  {
    name: 'data-model-null',
    artifact: 'scxml_data_model_null',
    file: 'scxml_data_model_null.virtualized.wasm',
  },
  {
    name: 'ioprocessor-scxml',
    artifact: 'scxml_ioprocessor_scxml',
    file: 'scxml_ioprocessor_scxml.virtualized.wasm',
  },
  {
    name: 'ioprocessor-basichttp',
    artifact: 'scxml_ioprocessor_basichttp',
    file: 'scxml_ioprocessor_basichttp.virtualized.wasm',
  },
  { name: 'clock', artifact: 'scxml_clock', file: 'scxml_clock.virtualized.wasm' },
  { name: 'compiler', artifact: 'scxml_compiler', file: 'scxml_compiler.virtualized.wasm' },
  {
    name: 'snapshot-codec',
    artifact: 'scxml_snapshot_codec',
    file: 'scxml_snapshot_codec.virtualized.wasm',
  },
  { name: 'validator', artifact: 'scxml_validator', file: 'scxml_validator.virtualized.wasm' },
  {
    name: 'extension-example',
    artifact: 'scxml_extension_example',
    file: 'scxml_extension_example.virtualized.wasm',
  },
  { name: 'scx', artifact: 'scx', file: 'scx.virtualized.wasm' },
];
// 2 GiB - 1: components ship as GitHub release assets, and "each file
// included in a release must be under 2 GiB"
// (https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases);
// the Wasmtime host's MAX_AUTHENTICATED_COMPONENT_BYTES is the same bound.
// Checked against the file's size before it is read.
const MAX_COMPONENT_BYTES = 2 ** 31 - 1;
const MAX_MANIFEST_BYTES = 64 * 1024;

function fail(message) {
  console.error(`scxml-host-browser build: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const options = { artifacts: undefined, meter: undefined, out: join(packageRoot, 'dist') };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = argv[index + 1];
    if ((flag === '--artifacts' || flag === '--out' || flag === '--meter') && typeof value === 'string') {
      options[flag.slice(2)] = resolve(value);
      index += 1;
    } else {
      fail(`unknown or incomplete argument: ${flag}`);
    }
  }
  if (options.artifacts === undefined) fail('--artifacts <dir> is required');
  if (options.meter === undefined) fail('--meter <scxml-wasm-meter binary> is required');
  return options;
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

/** Parses the `stateforward.wasm-artifacts/v1` manifest artifact rows. */
function readManifest(path) {
  const size = statSync(path).size;
  if (size > MAX_MANIFEST_BYTES) fail('artifact manifest exceeds its byte bound');
  const lines = readFileSync(path, 'utf8').split('\n').filter((line) => line.length > 0);
  if (lines[0] !== 'schema\tstateforward.wasm-artifacts/v1') fail('unsupported artifact manifest schema');
  const artifacts = new Map();
  for (const line of lines) {
    const fields = line.split('\t');
    if (fields[0] !== 'artifact') continue;
    if (fields.length !== 5) fail('malformed artifact manifest row');
    const [, name, file, bytes, digest] = fields;
    if (!/^[0-9]+$/.test(bytes) || !/^[0-9a-f]{64}$/.test(digest)) fail('malformed artifact manifest row');
    if (artifacts.has(name)) fail('duplicate artifact manifest row');
    artifacts.set(name, { file, bytes: Number(bytes), digest });
  }
  return artifacts;
}

const TRANSPILER = '@bytecodealliance/jco-transpile';

function pinnedTranspilerVersion() {
  const manifest = JSON.parse(readFileSync(join(packageRoot, 'package.json'), 'utf8'));
  const pinned = manifest.devDependencies?.[TRANSPILER];
  if (typeof pinned !== 'string' || !/^[0-9]+\.[0-9]+\.[0-9]+$/.test(pinned)) {
    fail(`package.json must pin ${TRANSPILER} to an exact version`);
  }
  const installed = JSON.parse(
    readFileSync(join(packageRoot, 'node_modules', TRANSPILER, 'package.json'), 'utf8'),
  ).version;
  if (installed !== pinned) fail(`installed ${TRANSPILER} ${installed} does not match pinned ${pinned}`);
  return pinned;
}

const MAX_CORE_MODULE_BYTES = 64 * 1024 * 1024;

/**
 * Meters one core module with the pinned tool and checks that the result
 * compiles and exports the fuel global.
 */
function meterCoreModule(meter, scratch, name, bytes) {
  const input = join(scratch, 'input.wasm');
  const output = join(scratch, 'output.wasm');
  writeFileSync(input, bytes);
  rmSync(output, { force: true });
  try {
    execFileSync(meter, ['--initial-fuel', String(DEFAULT_FUEL), input, output], {
      stdio: ['ignore', 'ignore', 'inherit'],
    });
  } catch {
    fail(`${name}: scxml-wasm-meter failed`);
  }
  if (statSync(output).size > MAX_CORE_MODULE_BYTES) fail(`${name}: metered module exceeds its bound`);
  const metered = readFileSync(output);
  let exports;
  try {
    exports = WebAssembly.Module.exports(new WebAssembly.Module(metered));
  } catch {
    fail(`${name}: metered module does not compile`);
  }
  if (!exports.some((entry) => entry.name === FUEL_EXPORT && entry.kind === 'global')) {
    fail(`${name}: metered module does not export ${FUEL_EXPORT}`);
  }
  return metered;
}

/** Output paths are relative, forward-slash, and confined to the component directory. */
function safeOutputPath(path) {
  return (
    typeof path === 'string' &&
    /^[A-Za-z0-9._/-]+$/.test(path) &&
    !path.startsWith('/') &&
    !path.split('/').some((segment) => segment === '' || segment === '.' || segment === '..')
  );
}

const options = parseArgs(process.argv.slice(2));
const transpilerVersion = pinnedTranspilerVersion();
const manifest = readManifest(join(options.artifacts, 'manifest.tsv'));

rmSync(options.out, { recursive: true, force: true });
mkdirSync(options.out, { recursive: true });
const scratch = join(options.out, '.meter');
mkdirSync(scratch, { recursive: true });
const record = {
  schema: 'stateforward.browser-host-build/v2',
  transpiler: `${TRANSPILER}@${transpilerVersion}`,
  meter: { tool: 'scxml-wasm-meter', fuel_export: FUEL_EXPORT, initial_fuel: DEFAULT_FUEL },
  components: [],
};

for (const component of COMPONENTS) {
  const entry = manifest.get(component.artifact);
  if (entry === undefined || entry.file !== component.file) {
    fail(`artifact manifest does not list ${component.file}`);
  }
  const source = join(options.artifacts, component.file);
  const size = statSync(source).size;
  if (size === 0 || size > MAX_COMPONENT_BYTES || size !== entry.bytes) {
    fail(`${component.file} does not match its manifest byte length`);
  }
  const bytes = readFileSync(source);
  if (sha256(bytes) !== entry.digest) fail(`${component.file} does not match its manifest SHA-256`);

  const outDir = join(options.out, component.name);
  let result;
  try {
    result = await transpile(source, {
      name: component.name,
      // One component instance per session with host-supplied imports.
      instantiation: 'async',
      // Browser-first output: no Node.js `fetch` fallbacks.
      nodejsCompat: false,
      // Virtualized components have no WASI imports; never rewrite to a shim.
      wasiShim: false,
      // Export failures throw an Error whose own `payload` is the WIT error
      // (the `jco transpile` CLI default; the host relies on it).
      noComponentErrorWrapping: false,
      emitTypescriptDeclarations: true,
    });
  } catch (cause) {
    fail(`transpile failed for ${component.name}: ${cause}`);
  }
  const files = Object.entries(result.files)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([path, transpiled]) => {
      let content = transpiled;
      if (!safeOutputPath(path)) fail(`${component.name} produced an unsafe output path`);
      if (path.endsWith('.js') && Buffer.from(content).includes('@bytecodealliance/preview2-shim')) {
        fail(`${component.name} bindings import a WASI shim`);
      }
      if (path.endsWith('.wasm')) {
        content = meterCoreModule(options.meter, scratch, `${component.name}/${path}`, content);
      }
      const target = join(outDir, path);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, content);
      return { path: `${component.name}/${path}`, sha256: sha256(content) };
    });
  record.components.push({
    name: component.name,
    source: component.file,
    source_sha256: entry.digest,
    // The component's imported interfaces (an extension component that
    // imports `session-snapshot-host` reads the session snapshot).
    imports: [...result.imports].sort(),
    files,
  });
  console.log(`transpiled ${component.name} source-sha256=${entry.digest} files=${files.length}`);
}

rmSync(scratch, { recursive: true, force: true });
writeFileSync(join(options.out, 'build-manifest.json'), `${JSON.stringify(record, null, 2)}\n`);
// Release identities for the snapshot's component list, keyed by dist name.
const identities = Object.fromEntries(record.components.map((component) => [component.name, component.source_sha256]));
const imports = Object.fromEntries(record.components.map((component) => [component.name, component.imports]));
writeFileSync(
  join(options.out, 'identity.js'),
  '// Generated by scripts/build.mjs: release-artifact SHA-256 and imported interfaces of each component.\n' +
    `export const RELEASE_DIGESTS = Object.freeze(${JSON.stringify(identities, null, 2)});\n` +
    `export const COMPONENT_IMPORTS = Object.freeze(${JSON.stringify(imports, null, 2)});\n`,
);
