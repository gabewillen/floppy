# scxml-host-browser

Thin browser host for the SCXML generic machine component. It runs machines
in **runtime-IR mode only**. The page ships the generic machine component,
transpiled once at build time with the pinned jco, and passes each machine's
lowered Machine IR as data. Browsers never load per-machine packaged
components.

The host parses no SCXML, interprets no IR (it reads the root IR only to
pre-warm `<invoke>` children and to prefetch allowlisted URLs; see
"Invocations" and "URL resources"), selects no transitions, and implements
no microstep or macrostep semantics. Those all run inside the components: the same
virtualized `scxml_machine`, data-model, SCXML and BasicHTTP Event I/O
Processor, clock, and extension component release artifacts the Wasmtime
session host runs; the page validates and compiles with the same validator
and compiler components. The
host only links capabilities,
bounds inputs and per-call work, maps failures to closed codes, delivers due
delayed sends, and calls the lifecycle exports in order.

The canonical API contract (preconditions, failures, bounds) is
[`src/index.d.ts`](src/index.d.ts); this README summarizes it.

## Build

Requires the pinned Node.js `v24.11.1`, the release guests from
`scripts/verify-wasm-capabilities.sh`, and the in-workspace fuel-metering
tool:

```console
sh scripts/verify-wasm-capabilities.sh scxml_clock scxml_data_model_ecmascript \
  scxml_data_model_null scxml_data_model_xpath scxml_ioprocessor_scxml scxml_ioprocessor_basichttp \
  scxml_compiler scxml_machine scxml_snapshot_codec scxml_validator \
  scxml_extension_example scx
cargo build --release --locked -p scxml-wasm-meter
cd hosts/scxml-host-browser
npm ci --ignore-scripts
npm run build        # node scripts/build.mjs --artifacts ../../target/wasm-artifacts/release \
                     #   --meter ../../target/release/scxml-wasm-meter
```

`scripts/build.mjs` authenticates each component (the machine, the
ECMAScript, XPath, and null data models, the SCXML and BasicHTTP Event I/O
Processors, the clock, the compiler, the validator, the session snapshot
codec, and the extension components this package ships: the example
namespace and `scx`) against the artifact
manifest's SHA-256 and byte length. It then transpiles each component exactly once into
`dist/<component>/`, using `@bytecodealliance/jco-transpile`
with async instantiation, no Node.js compat, no WASI shim, and
component-error wrapping. It then meters every emitted core module with
`scxml-wasm-meter` (see [Fuel](#fuel)) and checks that each metered module
compiles and exports the fuel global. It records the metering parameters and
every output digest in `dist/build-manifest.json`, and each component's
release digest and imported interfaces in `dist/identity.js`.

`--extensions <dir>` (repeatable) ships more extension components: every
artifact `<dir>/manifest.tsv` lists (the same `stateforward.wasm-artifacts/v1`
rows, virtualized components with no WASI import) is authenticated and
transpiled like the others into `dist/<name>/`, `<name>` its artifact name
with `_` as `-` (lowercase ASCII letters and `-` only). A name or release
digest another component already has fails the build, so a digest
authenticates exactly one component. `dist/extensions.js` lists every
extension component the build ships.

The validator and `scx` components validate with the bundled, unmodified W3C
SCXML 1.0 schema set, so the package carries its W3C notice (`NOTICE.md`,
copied by `npm pack`).

`--services <dir>` (repeatable) ships invoked-service components the same
way, listed in `dist/services.js`; the build always ships the example echo
and timer service, `components/scxml-service-example`. A service component
may import WASI 0.2 (`wasi:filesystem`, `wasi:io`, `wasi:clocks`,
`wasi:random`): the build copies the browser modules of the pinned
`@bytecodealliance/preview2-shim` `0.25.0` (and its license) into
`dist/wasi/`, and the host links them in itself. The bindings jco emits
never import the shim; the build fails if they do.

The transpiler is `@bytecodealliance/jco-transpile` `0.14.0`: the library
that `@bytecodealliance/jco` `1.34.0`'s `jco transpile` pins. With these
options it emits byte-identical output. It is used directly instead of the
full jco CLI package, because that package also pulls in
componentize-js/weval, whose `decompress` dependency carries a critical npm
advisory. The installed tree audits clean with `npm audit`.

Pinned tools: `@bytecodealliance/jco-transpile` `0.14.0`, `playwright-core`
`1.62.0` (dev only, for the Chromium e2e), `typescript` `5.9.3` (dev only:
`npm run typecheck` checks `src/index.d.ts` strictly, and
`scripts/verify-browser-host.sh` runs it), and every transitive package
through `package-lock.json`. The one runtime dependency is the OpenTelemetry
JS API, `@opentelemetry/api` `1.9.1` (API only, no SDK or exporter). Pages
resolve its bare specifier with their bundler or an import map, as
`test/browser/index.html` does.

## API

```js
import {
  createSession, createCompiler, createValidator, createEcmaScriptDataModel,
  ExtensionComponents, authenticateExtensionComponent, CapabilityError,
  ScxmlSessionError,
} from '@stateforward/scxml-host-browser';

// Optional: validate and compile in the page (or ship IR compiled by the CLI).
// `packages` are the ExtensionPackages of the namespaces the document uses;
// `eventSchemas` the files its `event:schema` attributes reference.
const validator = await createValidator({ dataModel: await createEcmaScriptDataModel() });
const { report: validation } = await validator.validate(scxmlText, { packages, eventSchemas });
// `validation` is the canonical scxml.report/v3 validator report, byte for
// byte the report `scx validate --format json` writes.
const compiler = await createCompiler();
const { status, report, ir } = await compiler.compile(scxmlText, { packages, eventSchemas });
// `report` (canonical scxml.report/v3) is the decision; `ir` is present only
// when status is 'emitted' and matches the report's length and SHA-256.

// Optional: the extension components of the namespaces the document uses.
const extensions = new ExtensionComponents()
  .bind(examplePackage, await authenticateExtensionComponent(componentBytes, componentSha256));

const session = await createSession({
  ir,                                   // Uint8Array, at most 16 MiB
  dataModel: await createEcmaScriptDataModel(), // the data model the document
                                        // names; see "Data model" below
  capabilities: {
    log({ label, text }) { /* text is the data model's rendered log text */ },
    loadDocument(kind, uri) { /* 'data' | 'invocation'; return the text, or
                                 {text, mediaType} (see "URL resources") */ },
    // compileInvocation(sourceName, source) { /* optional override; see "Invocations" */ },
    // raise, loadScript, runScript
  },
  extensionComponents: extensions,      // optional; see "Extension components"
  resources: {                          // optional; absent = no URL is ever
                                        // resolved (see "URL resources")
    // `url` is already canonical (dot segments removed); compare its parts.
    allow: (url) => new URL(url).origin === 'https://docs.example',
    resolve: async (url) => {
      const response = await fetch(url, { credentials: 'omit', redirect: 'error' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return response.text();
    },
  },
  http: {                               // optional BasicHTTP (see "BasicHTTP")
    allow: (url) => {                   // deny-all otherwise
      const { origin, pathname } = new URL(url);
      return origin === 'https://api.example' && pathname.startsWith('/scxml/');
    },
    // send: (request) => fetch(...),   // default: fetch, no credentials, no redirects
    location: 'https://app.example/scxml/inbox', // advertised in _ioprocessors
  },
  timers: 'event-loop',                 // or 'manual' (see "Delayed sends")
  onTimer(outcome) { /* {results, complete} or {error} per asynchronous delivery */ },
  telemetry: { tracerProvider, meterProvider }, // optional OpenTelemetry
                                        // providers; default: the global ones
  fuel: 75_000_000,                     // optional budget of one host call,
                                        // shared by every component it reaches
                                        // (DEFAULT_FUEL; see "Fuel")
  invoke: { compile: 'auto' },          // default: the host compiles <invoke>
                                        // children; see "Invocations"
});
const started = session.start();          // runs to a stable configuration in the machine
const result = session.send('begin');     // result.stable is true unless the
                                          // per-call bound stopped it: resume()
session.receiveHttp({ method: 'POST', headers, body }); // inbound BasicHTTP
session.nextDeadline(); session.now();    // the session clock (logical ms)
session.configuration(); session.status(); session.failure();
session.shutdown(); session.dispose();
```

- `createSession` instantiates a fresh machine instance per session. Core
  modules are compiled once per loader and cached. The default loader fetches
  `dist/` relative to this module with `WebAssembly.compileStreaming`, so
  serve `.wasm` as `application/wasm`. Pass `loadCoreModule` to load them
  another way.
- IR over 16 MiB throws `ScxmlHostError('machine-ir-too-large')` before
  instantiation. IR the machine rejects makes `createSession` throw
  `ScxmlSessionError` with the machine's code (`unsupported-version`,
  `invalid-input`, `unavailable`) and `sessionFailed: true`.
- Command failures throw `ScxmlSessionError` carrying the WIT
  `session-failure` (`code`, `committedEffects`, `sessionFailed`). A trap or
  foreign exception throws `ScxmlHostError('instance-lost')`, a command that
  runs out of fuel throws `ScxmlHostError('fuel-exhausted')`, and in both
  cases the instance is never entered again.
- Malformed arguments throw `ScxmlHostError('invalid-argument')` before any
  component is entered, and the instance stays usable: a malformed `send`
  event, a `compile` `schemaPolicy` outside `strict`/`lax`/`off`,
  a non-positive or non-integer `fuel`, or a `sessionId` that differs from
  the data model's. Error messages and details are static; caller values are
  never reflected or attached.
- `send(event)` takes an external event: its `type` is `external` (the
  default) and it has no `invokeId`. The machine rejects any other type,
  or an invoke id, with `ScxmlSessionError('invalid-input')` before
  processing it: only the machine raises `internal` and `platform` events,
  and only an invoked child's own messages carry an invoke id and run its
  `<finalize>` (SCXML 5.10.1, 6.4).
- `sessionId` (default `DEFAULT_SESSION_ID`) is the one source of
  `_sessionid`: the machine initializes the data model from `start` with it.
- Providers throw `CapabilityError(code)` to return a closed WIT error code.
  Any other exception becomes `platform`. A missing provider method is
  `unavailable`. Malformed return values are `platform`. A `capabilities`
  function the host does not take from the page (for example `send` or
  `cancel`, which belong to the linked I/O processor) is
  `invalid-argument`, so no provider is ever silently ignored.
- Sessions run to completion, one operation at a time. Providers and a
  page-supplied data model run inside a session operation and must not call
  back into the session: any `MachineSession` method called while another is
  in progress (a timer-driven delivery included) throws
  `ScxmlHostError('reentrant-call')` without entering any instance, and the
  operation in progress continues unaffected. Queue the work and run it
  after the operation returns (`onTimer` runs after its delivery and may
  call the session). A method never reports success for a lost session: it
  throws the fault that lost it, or `instance-lost`.

### Capabilities

Every `machine-capabilities` function of the WIT contract, and what this host
does with it. "As Wasmtime" means the same forwarding and the same component
decides, so results are identical to the Wasmtime session host
(`hosts/scxml-host-wasmtime/src/session_host/`); the IRP parity test holds
the browser to the checked-in Wasmtime baselines.

| Capability | Browser host | Wasmtime session host |
| --- | --- | --- |
| `initialize-data-model` | forwarded to `dataModel.initialize` with the host's `_sessionid`, `_ioprocessors` (the SCXML processor, plus BasicHTTP only with the `http` option), and the machine's `_name`; `unavailable` with no data model | as browser, BasicHTTP always in `_ioprocessors` |
| `declare`, `initialize-data`, `set-parameter`, `evaluate-condition`, `evaluate-value`, `evaluate-location`, `evaluate-text`, `evaluate-log`, `evaluate-form-value`, `foreach-items`, `assign`, `execute`, `update-active-configuration`, `data-model-name` | forwarded one-to-one; `unavailable` with no data model | as browser |
| `namelist-key`, `event-payload`, `inline-content`, `external-data`, `event-data-field`, `accepts-event-data` (pure) | forwarded one-to-one; `none` with no data model, a throwing provider, or a trapped component | as browser |
| `classify-target` | SCXML I/O processor `classify-target` with the session's sender facts | as browser |
| `send` | SCXML I/O processor `dispatch`; `scheduled`, `local` (enqueued externally), `unavailable` -> `undeliverable` (`error.communication`), other errors unchanged | as browser, plus parent and child routing |
| delayed delivery | clock `next-deadline`, `drain-due`, and `route-due` at delivery time, in clock order; driven by a browser timer (`event-loop`) or `advanceTime` (`manual`) | the same order, driven by the run scheduler on virtual time |
| `cancel` | SCXML I/O processor `cancel` | as browser |
| `send-basichttp` | BasicHTTP processor component; its `http-transport` is the page's `http.send`, only for URLs `http.allow` accepts (deny-all by default); `unavailable` -> `undeliverable` (`error.communication`); a delayed send is `check`ed, scheduled through the SCXML processor's `schedule-basichttp`, and posted when due (a failure then raises `error.communication` with its send id) | BasicHTTP processor over the in-process loopback |
| `load-document` | a static URL `resources` prefetched (allowlisted by the page), else `capabilities.loadDocument(kind, uri)`; either answers the text or `{text, mediaType}`, the media type becoming the WIT `loaded-document` indication (kept when visible ASCII, of any length, else dropped); a document of any size; `unavailable` (error.execution) without either, detail `dynamic-url-unavailable` for a runtime URL when `resources` is set; in an `async: 'auto'` session, that runtime URL waits on `resources` (see [Async sessions](#async-sessions)) | the embedder's documents |
| `compile-invocation` | default (`invoke.compile: 'auto'`): the session's own compiler component instance, pre-warmed with the root IR's static inline children and the `<invoke src>` documents `resources` prefetched, and cached by `(sourceName, source, eventSchemas)`; rejected documents (the compiler component's own bounds included; the host caps no source size) are `invalid-input` (error.execution), compiler faults fail the session. A page `capabilities.compileInvocation` overrides it; under `'provided'` without one, **unsupported** (`execution`, detail `unsupported-invoke`: error.execution, the invocation never starts) | compiler component, same request and outcome mapping |
| `start-invocation` (`session`) | invoked machine, data model, I/O processor, and clock built synchronously from preloaded modules, then `start-invoked`; an invoked session invokes in turn; beyond the limits the page set in `invokeLimits` `invalid-input` (error.execution) | as browser (sessions built from compiled components) |
| `start-invocation` (`service`) | the service component the page bound to the exact type URI (`serviceComponents`; see [Invoked services](#invoked-services)): one instance per invocation, its `invoke` export driven asynchronously through JSPI; an unbound type, or a page without JSPI, gets `unavailable`, and the `<invoke>` fails when it starts with `error.execution` (SCXML 6.4) | the service component bound to the exact type URI (`SessionHost::with_service_components`): one instance per invocation, driven through its `invoke` export; an unbound type fails its `<invoke>` when it starts |
| `cancel-invocation`, `forward-to-invocation` | the invoked session's cancellation begins (its later sends are ignored), its `cancel` runs, and it is dropped with every session below it; autoforward onto a live invoked session's external queue; when none is live, the case the WIT defines as a no-op | as browser |
| cross-session delivery | the SCXML Event I/O Processor pushes onto the target session's external queue; the Wasmtime executor's invocation order (src/invoked-sessions.js keeps the order `scxml-scheduler`'s `InvokedSessions` keeps, with its own index structures), after every command and in every delivery pass | `scxml-scheduler` |
| `admit-command`, `observe` | optional session budget (`maxCommands`, `capacity-exhausted`); command count and optional trace (`trace: true`) | run-wide budget and trace |
| `log` | always accepted (any length), recorded in the trace, passed to `capabilities.log` | trace and root logs |
| `load-script`, `run-script` | an unpinned `<script src>` `resources` prefetched: ECMAScript text, the Wasmtime host's bounds and codes, run through the data model's `execute`; otherwise the `capabilities` providers, `unavailable` when missing | script loader under a digest allowlist |
| `raise` | the `capabilities` provider; `unavailable` when missing | `unavailable` |
| `load-extensions` | each namespace the document uses gets this session's instance of the component `extensionComponents` binds to it (namespace URI checked, `initialize` called); a namespace nothing binds fails the session start closed (`unavailable`); see [Extension components](#extension-components) | as browser, plus the embedder's resolver |
| `execute-extension` | the session's instance of the bound component, with its `data-model-host`, `execution-host`, and `session-snapshot-host` acting on this session during the call; the events it queues go back with the outcome | as browser |
| `extension-snapshot-request` | true exactly for a bound component that imports `session-snapshot-host` | as browser |

Why the unsupported ones stay unsupported:

- **`<invoke>` under `invoke: { compile: 'provided' }` without
  `compileInvocation`.** A page that opts out of the host's compiler and
  supplies none gets the typed `unsupported-invoke`, which the machine
  turns into `error.execution` exactly as for a child the platform rejects
  (SCXML 6.4), so documents that handle a failed invocation still run.
- **URLs.** The host never fetches a URI by itself. Pages decide which URLs
  resolve: through the allowlisted `resources` option (static URLs,
  prefetched) or `capabilities.loadDocument` (synchronous).
- **Repository packages.** A repository namespace or service
  (`https://github.com/<owner>/<repo>/namespaces/<name>` or `.../services/<name>`)
  is resolved by nothing here, and fails closed as any unbound one does.
  `scx` resolves them with its GitHub resolver component
  (`stateforward:scxml/resolver`), and jco-transpile 0.14.0 does transpile
  it, but this host does not run it: (1) a resolved component arrives at
  run time, while this host runs only the components it ships transpiled
  and fuel-metered at build time (see [Extension components](#extension-components)
  and [Invoked services](#invoked-services)); (2) its only import, WASI 0.3
  `wasi:http/client.send`, has no browser implementation (preview3-shim
  0.6.1's browser `send` throws `Todo`), and a page's `fetch` cannot stand
  in for it with the same grant: `redirect: 'manual'` hides the `Location`
  the component must follow so each hop is admitted again, and `follow`
  lets the browser reach any origin; (3) `github.com` release downloads
  (`SHA256SUMS` and the component asset) send no CORS headers, so a page
  cannot read them.

### Validator

`createValidator({dataModel})` instantiates the bundled validator component
(the `scx validate` validator: XML well-formedness, the bundled W3C SCXML
1.0 schema set, the semantic rules, the `event:schema` checks, and the
packages' schemas), authenticated, transpiled, and fuel-metered like the
compiler. `validate(source, {sourceName, schemaPolicy, packages,
eventSchemas})` returns `{report}`, the canonical
`scxml.report/v3` validator report: for a document whose expressions use the
linked data model, byte for byte what `scx validate --format json` writes
(the parity test compares every `tests/ui` snapshot). `schemaPolicy`
defaults to `strict`, as `scx validate`. Its `expression-syntax` import is linked to the
`dataModel` the page passes (a bundled ECMAScript or XPath data model), as
the Wasmtime host's `ComponentValidator` links one: only documents whose
`datamodel` is exactly that name have their expressions checked, the others
get the `LINT010` note, and without one no expression is checked. Each
validation runs on its own fuel budget (`fuel`, default
`DEFAULT_COMPILE_FUEL`); a trap discards the instance.

The page supplies what `scx` reads from disk: `packages`, the
ExtensionPackages (manifest fields and schema bytes) of the namespaces the
document uses, and `eventSchemas`, the bytes of each schema file an
`event:schema` attribute references, keyed by the reference as authored
(the part before `#`), read relative to the document. The compiler takes
the same two options. The compiler component bundles no W3C schema, so a
page validates first and compiles with `schemaPolicy: 'off'`, as the
Wasmtime host compiles child documents.

A package may ship a package validator component (manifest
`validator-artifact` and `validator-sha256`) that checks what its schema
cannot. `createValidator({packageValidators})` links them, as the Wasmtime
host's `ComponentValidator::with_package_validators` does:

```js
const component = await authenticatePackageValidatorComponent(bytes, sha256);
const packageValidators = new PackageValidators().bind(pkg.namespaceUri, component); // .withCallFuel(n)
const validator = await createValidator({ dataModel, packageValidators });
```

The validator calls a bound package validator once per document with the
package's elements (an extension namespace's elements, or the `<invoke>`s
whose literal `type` is a service URI) and reports its diagnostics as
`<prefix>::<code>`; a trap (fuel exhaustion included) or an error is
`PKG002`. Each call instantiates the component afresh on its own fuel
budget (`withCallFuel`, default `DEFAULT_FUEL`). Only package validators
this package ships transpiled authenticate: the example package validator
(`scxml_package_validator_example`).

### Extension components

An extension namespace (SCXML 1.0 section 4.10) runs its elements in its
extension component, exactly as under the Wasmtime host's
`ExtensionComponents` (the same names, checks, and semantics):

```js
const pkg = { namespaceUri, packageVersion, extensionArtifact, schemaArtifact,
              schemaSha256, witWorld, witVersion, schema };  // the manifest + schema bytes
const component = await authenticateExtensionComponent(bytes, sha256);
const extensions = new ExtensionComponents().bind(pkg, component); // .withCallFuel(n)
const session = await createSession({ ir, dataModel, extensionComponents: extensions });
```

- **Authentication.** `authenticateExtensionComponent(bytes, sha256)`
  checks that the component bytes hash to the trusted SHA-256, and that the
  digest is the release artifact of an extension component this package
  ships transpiled and fuel-metered at build time (`dist/`: the example
  namespace, `components/scxml-extension-example`, and `scx`,
  `namespaces/scx`, plus any the build's `--extensions` added; see
  [Build](#build)). A component this build does not ship is
  `artifact-kind-mismatch`: browsers run only components built with the
  package (a component resolved at run time needs a build of its own).
- **Binding.** `bind(pkg, component)` checks what the Wasmtime host checks:
  an absolute namespace URI of at most `MAX_NAMESPACE_URI_BYTES` that is not
  the SCXML or XML namespace and is bound once, a schema of at most
  `MAX_NAMESPACE_SCHEMA_BYTES` whose SHA-256 is `schemaSha256`, and at most
  `MAX_EXTENSION_COMPONENTS` bindings (`namespace-binding` otherwise).
  Every bound package also reaches the session's `<invoke>` compiler, so a
  child may use its parent's namespaces.
- **Lifecycle.** `createSession` compiles every bound component's core
  modules. Each session (the root and every invoked child) instantiates its
  own instance of each namespace its document uses, before data binding:
  the `namespace-uri` must be the bound URI and `initialize` must succeed
  (`extension-component-uri-mismatch`, `extension-component-initialize-failed`).
  A namespace nothing binds fails the session start closed. `shutdown` runs
  once when the session is dropped.
- **Execution.** The component's `data-model-host` (`evaluate`, `assign`)
  and `execution-host` act on the executing session, and only during
  `execute` (`unavailable` outside it; `in-state` false and `now-ms` 0):
  `raise` queues an internal event, `send` routes through the session's
  SCXML Event I/O Processor exactly as `<send>` (another processor type is
  `invalid-input`; `#_internal` cannot be delayed), and `cancel` cancels a
  delayed send. The events it queues are handed back with the element's
  outcome, and the machine queues them in document order. An error other
  than the contract's codes becomes `protocol`, with static details.
- **Session snapshot.** A component that imports `session-snapshot-host`
  makes the machine pass its executable-content image with each of its
  elements, and `snapshot(exclude)` returns the executing session's
  canonical `executable-content` document (encoded by the bundled codec
  component, which `createSession` then loads), at most
  `MAX_EXTENSION_SNAPSHOTS` times per call. The parity test holds it to the
  Wasmtime host's document.
- **Bounds.** Each call runs with the per-call cap (`withCallFuel`, default
  `DEFAULT_EXTENSION_CALL_FUEL`) on the session's host-call budget (see
  [Fuel](#fuel)), so its work is charged to the machine call that made it.
  A trap (fuel exhaustion included) retires the instance: later elements of
  its namespace in that session raise `error.execution`.
- **No I/O.** An extension element is executable content: its component
  gets no `outbound-http`, no WASI, and no `env` value. Async sessions
  (`async: 'auto'`) do not take `extensionComponents`.

### Invoked services

A service `<invoke>` (SCXML 6.4) runs in the invoked-service component the
page binds to its exact type URI, as under the Wasmtime host's
`ServiceComponents` (the same names, checks, bounds, and semantics; see
[docs/invoked-services.md](../../docs/invoked-services.md)):

```js
const pkg = { serviceUri, packageVersion, serviceArtifact, witWorld, witVersion,
              httpOrigins, envSlots };                        // the service package manifest
const component = await authenticateServiceComponent(bytes, sha256);
const services = new ServiceComponents()
  .bind(pkg, component)
  .withValues([{ name: 'API_KEY', value, packages: [pkg.serviceUri] }])
  .withOutboundHttp({ fetch: (url, init) => fetch(url, init) })
  .withWasi(pkg.serviceUri, { dirs: [[tree, '/data']] });  // only for wasi:filesystem
const session = await createSession({ ir, dataModel, serviceComponents: services });
```

- **JSPI.** A service's `invoke` is an `async func`, and its imports
  (`outbound-http`, the monotonic clock's waits) suspend it. jco runs these
  through JavaScript Promise Integration, so services need
  `WebAssembly.Suspending` (Chromium 137+; Node.js 24 with
  `--experimental-wasm-jspi`). Without it every service start is
  `unavailable` (`error.execution`); the rest of the session is unaffected.
- **Authentication.** `authenticateServiceComponent(bytes, sha256)` checks
  the bytes against the trusted SHA-256 and that the digest is the release
  artifact of a service component this build ships (see [Build](#build));
  any other is `artifact-kind-mismatch`.
- **Binding.** `bind(pkg, component)` checks a service type URI of at most
  `MAX_SERVICE_TYPE_URI_BYTES` bound once, at most `MAX_BOUND_SERVICES`
  bindings, and at most `MAX_DECLARED_ITEMS` valid `httpOrigins` and
  `envSlots` (`service-binding` otherwise). Binding a service consents to
  the origins its package declares.
- **Values and references.** The page holds the values (`withValues`), each
  granted to the type URIs it lists. A service never receives a value:
  `env:<slot>="NAME"` on its `<invoke>` gives it a reference for a slot its
  package declares, valid only inside its own admitted requests, where the
  host substitutes the value. An undeclared slot or a variable not granted
  to the type fails the `<invoke>` with `error.execution`.
- **Outbound HTTP.** `outbound-http.fetch` and `send` admit a request as the
  Wasmtime host does (references substituted, the WIT bounds, then the
  package's origins or an origin a reference's value supplies entirely) and
  call the page's transport like the WHATWG `fetch`, with
  `redirect: 'manual'` and `credentials: 'omit'`. Without a transport every
  request is `unavailable`. At most `MAX_REQUESTS_IN_FLIGHT` are in flight
  per invocation.
- **WASI.** Clocks, randomness, and `wasi:io` need no grant.
  `wasi:filesystem` needs at least one directory (`withWasi`), each an
  in-memory tree the page owns and can read back. `wasi:sockets` cannot be
  granted in a browser (`wasi-resource-ungranted`), another `wasi:*`
  interface is `unknown-import`, and a grant to an unbound type or for an
  interface the component does not import is `wasi-grant-undeclared`; all
  are checked when the session is created.
- **Lifecycle.** Each invocation instantiates its own instance inside
  `start-invocation`. Events sent to `#_<invokeid>` (and autoforwarded)
  feed its event stream; the events it returns reach the invoking session's
  external queue stamped with the invoke id; its result `ok` becomes
  `done.invoke.<id>`, while `err`, a trap (fuel exhaustion included), or an
  invalid returned event becomes `error.communication.<id>`. Cancelling the
  invocation ends its stream, aborts its requests, clears its waits, and
  drops whatever it returns afterwards.
- **Bounds.** Between two inputs (its start, an event it reads, a
  response, a wait falling due) an invocation runs at most the call fuel
  (`withCallFuel`, default `DEFAULT_SERVICE_CALL_FUEL`) and returns at most
  `MAX_RETURNED_EVENTS_PER_INPUT` events of at most
  `MAX_RETURNED_BYTES_PER_INPUT` bytes in all.
- **Browser differences.** A service runs on the event loop, so what it
  returns reaches the session in a later turn (the page hears it through
  `onTimer`), not at a deterministic point of the run; an `invoke` that
  rejects its start does so after `start-invocation` completed, so it
  becomes `error.communication.<id>` rather than `error.execution`; and the
  monotonic clock reads the session's logical clock while its waits fall
  due in wall-clock time. Snapshots of a run holding a live service are
  refused, as under the Wasmtime host. Async sessions (`async: 'auto'`) do
  not take `serviceComponents`.

### URL resources

A `resolve(url)` or `capabilities.loadDocument` answer is the text, or
`{text, mediaType, baseUri, eventSchemas}` (each field but `text`
optional) with the media type the source indicated (for example the
`Content-Type` of the page's `fetch`). SCXML B.2.1 says "if an
indication of the type of the content is available (e.g., via a
Content-Type header), then the Processor SHOULD try to interpret the
content according to that indication": for `<data src>` the ECMAScript data
model tries a JSON (`application/json`, `+json`) or XML (`application/xml`,
`text/xml`, `+xml`) indication first and otherwise reads the content by
B.2.1's own rules. Any other type, `text/plain` included, indicates nothing:
it is the label resources get by default (RFC 2045 §5.2), and the official
W3C cases ship JSON and XML in `.txt` files that they expect read by
content. A script's media type is ignored.

For an `<invoke src>` document, `baseUri` and `eventSchemas` are what the
Wasmtime host's embedder answers in `EmbeddedDocument`: `baseUri` is the
absolute URI the child's own relative URIs resolve against (RFC 3986
§5.1: its `<data src>`, `<script src>`, and `<invoke src>`, so a child's
children resolve against their own base in turn), and `eventSchemas`
(`{uri, bytes}[]`, as `createCompiler().compile` takes them) are the
`event:schema` files the child is compiled with. A `baseUri` that is not
an absolute URL fails the answer. The root session has no base URI: its
URIs reach `allow` as written, and an inline `<content>` child shares
its invoking session's base.

**Security: the default is deny.** Without `resources`, no URL is ever
resolved, and every `<data src>`, `<script src>`, and `<invoke src>` keeps
its typed error. Agent-written SCXML reaches the network only through URLs
the page's own `allow` accepts: allowlist exact origins or paths, never
everything. `allow` and `resolve` get an `http:` or `https:` URL in its
canonical form (`new URL(url).href`, the form `fetch` reaches), so `..`,
`%2e%2e`, and backslash segments are already resolved when `allow` judges
it; one that carries userinfo is denied without reaching `allow`. Any other
URL (a relative name, or another scheme such as `file:`) reaches `allow`
exactly as the document wrote it: the host never fetches, and a page may
map such names to its own documents. Compare the parsed origin and
pathname, not a raw string prefix; `new URL(url)` also rejects relative
names, which `fetch` would resolve against the page. The page's origin and cookies stand behind a
`fetch` in `resolve`: omit credentials and refuse redirects
(`{credentials: 'omit', redirect: 'error'}`) so a redirect on an allowed
origin cannot take a load outside the allowlist.

`createSession({ resources: { allow, resolve } })` (see
`ResourceProviders` in `src/index.d.ts`):

- Machine calls are synchronous and a page's fetch is not, so
  `createSession` resolves URLs before the machine runs. It collects every
  statically known URL of the IR: `<data src>`, unpinned `<script src>`, and
  `<invoke src>`. With the host's automatic compiler (see
  [Invocations](#invocations)), each `<invoke src>` document it resolved
  is then compiled, with its `eventSchemas`, and the static URLs of the
  child's IR, resolved against its `baseUri`, are prefetched the same way,
  breadth first, each distinct URL once, for at most
  `MAX_PREFETCHED_CHILDREN` (64) children. This scan only chooses what to
  prefetch; the machine still validates the IR and decides every load.
- `allow(url)` is asked once per distinct URL (an http(s) URL in its
  canonical form), synchronously, before any `resolve`. Only a result of exactly `true`
  admits the URL; anything else, or a throw, denies it. `resolve(url)` (a
  promise of the text) is called with the same form, only for
  admitted URLs, all in parallel, and `createSession` settles after they do.
- The machine's loads are then served from that cache, with the Wasmtime
  session host's bounds and codes:
  - `<data src>`, `<invoke src>`: a denied, unresolved (rejected or not a
    string), or over-limit URL is `unavailable`, so the machine raises
    `error.execution` (an `<invoke>` fails). A document of any size is
    accepted. An `<invoke src>` document goes unchanged to the child
    compiler (the host's automatic compiler, or the page's
    `compileInvocation(sourceName, source)`).
  - `<script src>`: text in the ECMAScript data model's language, of any
    size (any number of distinct URIs per session), run through the data
    model's `execute`, whose own script bound (64 KiB in the shipped
    component) raises `error.execution` when the script runs, as under
    Wasmtime. A denial or `resolve` rejecting with
    `CapabilityError('unavailable')` is `unavailable`, any other rejection
    `platform`, empty text `payload-too-large`, and a session
    without the ECMAScript data model `invalid-input`; each rejects the
    document, so `createSession` throws `ScxmlSessionError`. A pinned
    script (a Wasm component) is not resolved here.
- Every URL is prefetched, of any number and resolving to text of any
  total, as under Wasmtime; a URL longer than `MAX_RESOURCE_URL_BYTES`
  (16384 bytes, the script URI bound) answers `unavailable`.
  `RESOURCE_DETAILS` names each failure's detail.
- A URL computed at run time (`srcexpr`) cannot be prefetched.
  `capabilities.loadDocument` answers it when present. In an
  `async: 'auto'` session with the `worker` mechanism, `resources` answers
  it (see [Async sessions](#async-sessions)). Otherwise it is
  `unavailable` with detail `UNSUPPORTED.dynamicUrl`
  (`dynamic-url-unavailable`), which the machine raises as
  `error.execution`. URLs of an invoked child beyond the prefetch (one the
  page's `compileInvocation` compiles, one named at run time, or one past
  `MAX_PREFETCHED_CHILDREN`) are known only once the child is compiled, so
  they are treated the same way.
- Each resolution is one `capability` host operation. URLs are never
  telemetry attributes.

### Invocations

`<invoke>` runs without the page doing anything. By default
(`invoke: { compile: 'auto' }`, see `InvokeOptions` in `src/index.d.ts`),
when the root IR declares `<invoke>`, `createSession` opens a dedicated
compiler component instance for the session (`src/invoke.js`), and the
machine's synchronous `compile-invocation(sourceName, source)` is answered
from it inside the call, exactly as the Wasmtime host answers it from its
compiler component:

- the request is the Wasmtime host's: lowered IR, schema policy off,
  runtime script resolution, and the `eventSchemas` of the page's answer
  for an `<invoke src>` document (none for inline `<content>`; see
  [URL resources](#url-resources));
- a child the compiler rejects (diagnostics, or `invalid-request`) is
  `invalid-input`, which fails the invocation with error.execution in the
  parent (SCXML 6.4); any other compiler failure fails the session, with
  `ScxmlCompileError(code)` or, for a compiler trap or exhausted fuel,
  `ScxmlHostError('instance-lost' | 'fuel-exhausted')`;
- bounds: each compile is charged to the budget of the host call that
  makes it (the invoking command, or `createSession` for a pre-warm; see
  [Fuel](#fuel)) and capped by `invoke.fuel` (default
  `DEFAULT_COMPILE_FUEL`); the host caps no source size, so only the
  compiler component's own bounds reject a large child;
- outcomes are cached per session by the exact `(sourceName, source,
  eventSchemas)` request (at most 64 entries and 32 MiB, oldest evicted
  first), so invoking the same child again compiles nothing;
- pre-warm: before `start`, `createSession` compiles every child the root
  IR fixes statically (an SCXML-typed `<invoke>` with inline `<content>`
  and no `src`, `srcexpr`, `typeexpr`, or content `expr`), rebuilding the
  exact source the machine will request, so the first invocation is a
  cache hit; with `resources`, the `<invoke src>` documents the prefetch
  resolved are compiled to the same cache. This is the only IR the host
  reads, and only as a hint: the machine still validates the IR and
  decides every invocation.

A page may still pass `capabilities.compileInvocation`, a synchronous
provider of pre-lowered child IR (see `CapabilityProviders`); it overrides
the host's compiler. Lower each child beforehand with
`createCompiler().compile(text, { sourceName, schemaPolicy: 'off' })` or the
CLI, return its IR for the exact `(sourceName, source)` the machine asks
for, and throw `CapabilityError('invalid-input')` for any other document
(the invocation then fails with error.execution). IR the machine cannot
decode or validate (a `<raise>` event over the 1024-byte name bound, for
one) fails the invocation the same way. `invoke: { compile:
'provided' }` turns the host's compiler off.

With either compiler, `createSession` first compiles every core module a
child needs (machine, I/O processor, clock, and the three data models), so
`start-invocation` instantiates the child synchronously inside the parent's
call, as the Wasmtime host builds a child store there. The parent's call
then suspends right after the `<invoke>` (WIT
`invocation-progress.deferred`); the host starts the child from its own
loop and then continues the parent, so a chain of any depth takes no stack
per level (the same for a cancel):

- each child gets its own machine instance, the data model its document
  names, and its own SCXML I/O processor and clock, advanced with the
  root's (one shared logical clock); `_sessionid` is the invoking
  session's `_sessionid`, `.`, and the child's invoke id
  (`<parent _sessionid>.<invokeid>`, as under Wasmtime), and an
  `<invoke>` whose child's id would exceed the 1024-byte name bound raises
  `error.execution`;
- every session owns an internal and an external event queue; the SCXML
  Event I/O Processor pushes an event for another session straight onto
  that session's external queue (`#_parent`, `#_<invokeid>`, autoforward,
  and `#_scxml_<sessionid>` of any running session of the run, as under
  Wasmtime), and a finished session's `done.invoke` follows every event it sent; the
  run processes one queued entry at a time, the first session in
  invocation order with one, in the Wasmtime executor's order
  (src/invoked-sessions.js keeps the order `scxml-scheduler`'s
  `InvokedSessions` keeps, with its own index structures), after every
  command and in every delivery pass;
- a session whose cancellation began sends nothing more to its invoking
  session, and cancelling a session cancels the sessions it invoked;
- an invoked session whose call stops at its per-call command bound is
  ended as its invoking session's cancel would end it (SCXML 6.4): nothing
  it sends reaches its invoking session from then on, its `cancel` runs
  (cancelling the sessions it invoked), and no `done.invoke` follows; a
  start that stopped there fails its `<invoke>` (`error.execution` in the
  invoking session), and a later call queues
  `error.communication.<invokeid>` for it, as under Wasmtime. Any other
  failure or trap of any session, and a failure of that cancel, fails the
  whole session, as it fails the Wasmtime run; the root's own stop at the
  bound leaves it unstable until `resume`;
- depth and breadth are the Wasmtime host's: an invoked session invokes in
  turn, with no limit unless the page sets one in
  `invokeLimits: { maxInvokeDepth?, maxInvokedSessions? }` (non-negative
  integers; an absent field sets no limit); an invocation beyond them fails
  with error.execution. An external event queue holds any number of
  events.

### BasicHTTP

The bundled BasicHTTP Event I/O Processor (`scxml_ioprocessor_basichttp`,
SCXML 1.0 appendix C.2) runs in the browser exactly as under Wasmtime: the
component validates and encodes every `<send
type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor">` (or
`type="basichttp"`), interprets the response, decodes inbound messages, and
chooses every status code. The host only moves bytes (`src/basichttp.js`).
One sender and one receiver instance serve the root and its invoked
children.

- **Outgoing.** The component encodes a `POST` with `Content-Type:
  application/x-www-form-urlencoded`: `_scxmleventname=<event>` first, then
  every `namelist` entry and `<param>` in document order as `name=value`, or
  the `<content>` as one bare segment; every byte outside RFC 3986
  unreserved is percent-encoded in uppercase hex (a space is `%20`). The
  request `{method, url, headers, body}` reaches `http.send` only when
  `http.allow(url)` returns exactly `true`. `send` defaults to `fetch` with
  `credentials: 'omit'` and `redirect: 'error'`. A cross-origin endpoint
  must answer CORS for the default send to see its status; otherwise
  `fetch` rejects and the send is reported failed.
- **Fire-and-forget.** A `send` that returns a promise (such as `fetch`)
  does not block the machine: the send completes at once. When the promise
  rejects, or settles with a non-2xx or malformed response, the session
  receives `error.communication` later, reported through `onTimer`. The
  platform raised it, so its `_event.type` is `platform` (SCXML 5.10.1),
  and it goes on the sender's internal event queue (SCXML 3.12.2, 6.2.4)
  through the machine's `communicationError`, which runs the macrostep that
  processes it at once without dequeuing an external event (so it is never
  autoforwarded). Its `_event.sendid` is the
  failed `<send>`'s id (SCXML 5.10.1: the authored `id`, else the one the
  machine generated for that execution), and it matches the Wasmtime one
  exactly (no `origin`, `origintype`, or data). A `send` that
  returns `{status, headers?, body?}` directly is the WIT's synchronous
  transport, and the component interprets the response inside the `<send>`.
- **`error.communication`.** A denied URL, a missing target, a throwing
  `send`, and a synchronous non-2xx or malformed response make the
  component report `unavailable`, and the machine raises the platform
  `error.communication`, carrying the send's `_event.sendid`, on the
  internal queue and skips the rest of the block, exactly as under
  Wasmtime. Without the `http` option every
  BasicHTTP send is denied. An invalid target (not an absolute `http` or
  `https` URI) or value raises `error.execution`. A delayed BasicHTTP send
  is checked at the `<send>`, held on the session clock by the SCXML Event
  I/O Processor, and posted only when its delay elapses (SCXML 6.2), driven
  by the timer mode like every delayed send; `<cancel>` cancels it, and a
  session that terminates first discards it. A due send that cannot be
  dispatched raises `error.communication` with its send id, as under
  Wasmtime.
- **Incoming.** `session.receiveHttp({method, headers, body, url?})`
  injects one inbound request addressed, by location in its canonical form
  (src/url.js), to the root session or to any live, unfinished invoked
  session at any depth, as the Wasmtime session host routes one. The
  component
  decodes it: the event name is the first `_scxmleventname` form parameter,
  else the `_scxmleventname` header, else `HTTP.POST`; `_event.data` holds
  the other form parameters (a repeated name becomes an array) or the body
  text; `_event.raw` is the message as received; `_event.origintype` is the
  BasicHTTP type URI. A single valid `traceparent` header (and a valid
  `tracestate`) becomes the event's carrier. The event is processed before
  `receiveHttp` returns the component's response `{status, headers, body}`:
  200 once enqueued, 400, 405 (`Allow: POST`), or 413 for a rejected
  message, 404 when `url` is neither the root's location nor a live
  invoked session's, or the session is not running, and
  503 while the last call stopped at its command bound.
  Without `http` it throws `invalid-argument`.
- **Loopback.** While a session's BasicHTTP send (the root's or an
  invoked child's) runs `http.send`, the first request the page routes
  straight back to `receiveHttp` is delivered as the Wasmtime in-process
  loopback delivers it, by location in its canonical form (src/url.js): to
  the sending session itself (enqueued before the `<send>` returns), to its
  parent, or to one of its live children (each queued and delivered once
  the sending call ends); any other location answers 404. Any other `receiveHttp` call during a
  session operation throws `reentrant-call`. The IRP parity run uses this
  loopback.
- **`_ioprocessors`.** The processor's type URI and its `basichttp` alias
  are listed only with the `http` option, and every entry has a `location`
  (SCXML C.2.3: the processor "MUST maintain a 'location' field inside this
  entry"): `http.location` for the root when it is a string, or
  `location(sessionId)` when it is a function; a session for which neither
  gives one (an invoked child under a string `location`, a function that
  returns `undefined`, or no `location` at all) gets its loopback address
  `http://basichttp.loopback.invalid/sessions/<id>/basichttp`, the
  Wasmtime host's default location. A send to a URL under
  `http://basichttp.loopback.invalid` is delivered in-process, as under
  Wasmtime, to the live session at that location (the sender itself, its
  parent, or a live child), without reaching `allow` or `send`; one that no
  live session answers raises `error.communication`.

**Security.** SCXML documents, including agent-written ones, reach the
network only through `http.allow`. Without the option, and for every URL
`allow` does not accept with exactly `true`, nothing is sent. A throwing
`allow` also denies. `allow`, `send`, and the default fetch get the
target's canonical form (`new URL(target).href`), so `..`, `%2e%2e`, and
backslash segments are already resolved when `allow` judges it; a target
that carries userinfo is denied without reaching `allow`. Keep the
allowlist to exact origins and paths, and compare the parsed origin and
pathname, not a raw string prefix.
Remember that the page's origin and cookies stand behind `fetch`: the
default send omits credentials and refuses redirects, so a redirect cannot
leave the allowlist. A custom `send` takes on those duties. `receiveHttp`
is an injection point that trusts its caller: authenticate inbound requests
before passing them in, because the document sees their parameters, and
their raw text as `_event.raw`.

### Delayed sends

Each session links its own SCXML Event I/O Processor and project clock. A
`<send delay>` is scheduled on that clock under its send id (logical
milliseconds); `<cancel>` removes it. When a deadline is reached the host
drains the due batch, routes each request through the processor again
(`route-due`), and delivers local events as external events; a target that
no longer answers (an invoked session that finished or was cancelled) cannot
be dispatched, so the sender's `communicationError` places the platform
`error.communication`, carrying the send's id, on its internal event queue
and processes it at once (SCXML 3.12.2, 6.2.4, C.1), in (deadline, schedule)
order, until the
session is no longer running. At most `MAX_DUE_DELIVERIES` (10 000) sends are
delivered per turn; nothing is delivered while the last call did not
end stable (`resume` first).

- `timers: 'event-loop'` (default): after every command the host arms one
  `setTimeout` for the earliest deadline. Logical time follows the `now`
  option (default `performance.now`) from session creation, but never passes
  an undelivered deadline. Each timer delivery is reported to `onTimer` as
  `{results, complete}`, or `{error}` when a delivered event failed the
  session; `onTimer` exceptions are contained. `onTimer` also reports each
  late `error.communication` delivery (see [BasicHTTP](#basichttp)), in
  either mode.
- `timers: 'manual'`: time moves only through `advanceTime(ms)`, which
  returns `{results, complete}`. This mode is deterministic and is what the
  tests and the IRP parity run use.
- `now()` and `nextDeadline()` read the session clock.

### Async sessions

The machine's capability calls are synchronous. Static URLs are
prefetched before the machine runs (see "URL resources"), but a URL
computed at run time (`<invoke srcexpr>`) that the page does not supply
through `loadDocument` can only be resolved by asynchronous page work.
`createSession({ async: 'auto', resources: { allow, resolve } })` returns
an `AsyncMachineSession` whose capability calls can wait on it:
the same methods, results, and errors as `MachineSession`, each answered
through a Promise, with `mechanism` naming how it waits. `async: 'off'` is
the default and is the synchronous session. It is unchanged: an `off`
session never loads the bridge, so its results, costs, and IRP parity are
exactly those of a session without the option.

| `mechanism` | When | A runtime-computed URL |
| --- | --- | --- |
| `worker` | module Workers, growable `SharedArrayBuffer`, and `Atomics.wait` exist, and the page is cross-origin isolated | the unchanged synchronous session runs in a dedicated module Worker (`src/async-worker.js`), with the page's prefetched static URLs; on a cache miss the capability call posts to the page and blocks the worker with `Atomics.wait` until `allow(url)` returned true and `await resolve(url)` settled (both with the URL in the form static URLs use); the text is the document. Runtime URLs have the prefetch bound: a URL over 16384 bytes is `unavailable`; neither the runtime URLs one session resolves nor their total text are bounded |
| `none` | otherwise | the synchronous session runs in the page, static URLs prefetched as usual; a runtime URL is the typed `unavailable` error with detail `dynamic-url-unavailable` (`UNSUPPORTED.dynamicUrl`), which the machine turns into `error.execution` (SCXML 5.3); `resolve` is never called for it |

Requirements of the `worker` mechanism:

- **Cross-origin isolation.** Serve the page with
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp`, so
  `crossOriginIsolated` is true. Serve the worker script with the same
  headers.
- **Browser support.** Module Workers, `Atomics.wait` in workers, and
  growable `SharedArrayBuffer`: Chrome and Edge 111+, Firefox 128+, and
  Safari 16.4+ (MDN browser-compat-data: module workers need Chrome 80,
  Firefox 114, and Safari 15; `SharedArrayBuffer.prototype.grow` needs
  Chrome 111, Firefox 128, and Safari 16.4). `asyncMechanism()` reports
  what the current page gets.
- **The worker entry.** The default entry is
  `new URL('./async-worker.js', import.meta.url)`, which bundlers resolve.
  Import maps do not apply inside workers, so an unbundled page passes
  `workerUrl`: an entry whose `@opentelemetry/api` import is resolved, such
  as a bundled `src/async-worker.js`.

The contract:

- `resources.allow(url)` must return `true` before `resolve(url)` is
  called, and `resolve` answers the document's text or `{text, mediaType}`
  (the media type its source indicated). A denied URL, a rejected
  `resolve`, or an answer of any other shape is `unavailable`
  (error.execution), for a URL prefetched before the worker started and a
  runtime URL alike. A document of any size is accepted.
- The options are those of `createSession`, and they are validated the
  same way under both mechanisms. The exceptions: `dataModel` names a
  bundled data model (`'ecmascript'`, `'xpath'`, `'null'`), and `now` and
  `http` are not accepted (the BasicHTTP providers cannot be called from a
  worker). Page `capabilities` stay in the page and are called through
  the same blocking channel. Under `worker` they may also answer with a
  Promise; a page that must run under both mechanisms answers
  synchronously. Core modules come from the page's `loadCoreModule`.
- One operation at a time. A call made before the previous one settled, or
  from a provider the session is waiting on, rejects with
  `ScxmlHostError('reentrant-call')`. `dispose()` also terminates the
  worker. A `resolve` that never settles keeps the worker blocked until
  `dispose()`, which then terminates it at once; a worker that does not
  release its engine within 1000 ms is terminated
  anyway.
- Telemetry. The page records `create-session` and one `capability` span
  per call it serves, with the existing closed labels. The worker's own
  spans go to the worker's global OpenTelemetry API, which stays a no-op
  unless the worker entry registers an SDK.
- Integration. `run.bridge.resolveDynamic(uri)` in `load-document` is the
  single hook: after a miss in the prefetched `resources` cache, when the
  page supplies no `loadDocument`, and before the typed
  `dynamic-url-unavailable` error. Under `worker` it blocks on the page's
  `resources`; under `none`, and for `async: 'off'` sessions, the URL fails
  with the typed error. Static URLs are prefetched in the page under both
  mechanisms, and a worker session receives the prefetched answers in its
  `init` message, so its loads, including `platform` and
  `payload-too-large`, are the synchronous session's.

Why not JSPI (WebAssembly JavaScript Promise Integration) in this release:

- **Support is there.** JSPI ships in Chrome 137, Firefox 153, and
  Safari 27 (MDN browser-compat-data `webassembly.api.Suspending`,
  September 2026). Node 24.11.1 (V8 13.6) has it only behind
  `--experimental-wasm-jspi`. The pinned `@bytecodealliance/jco-transpile`
  0.14.0 supports `asyncMode: 'jspi'` with `asyncImports`/`asyncExports`.
- **The spike.** Transpiling `scxml_machine` with `load-document` as the
  async import leaves the eight core modules byte-identical. However, the
  bindings turn every `machine-api` export into an `async` function over
  `WebAssembly.promising`: the constructor, `start`, `send`, `resume`,
  `status`, `configuration`, `failure`, `snapshot`, `shutdown`,
  `start-invoked`, and `deliver-from-invocation`. Each awaits `task.enter()`,
  so every call on the hot path pays a Promise and microtasks.
- **The cost.** JSPI suspends only Wasm frames, and it cannot suspend
  through the host's JS capability layer. A JSPI session would need a
  second, async copy of the session scheduler (commands, cross-session
  settling, delayed delivery, timers, and the snapshot gather), plus an
  async `start-invocation` for children. The worker mechanism reuses the
  synchronous session unchanged.
- **Follow-up.** A JSPI mechanism for pages that cannot be cross-origin
  isolated, once one scheduler can serve both a synchronous and a
  suspending machine without a hot-path cost.

### Session snapshots

A session exposes a read-only snapshot of the root session and every
session it invoked, taken only
at a stable checkpoint, for tools and LLMs to inspect
(`docs/session-snapshot.md`). It is never restored; there is no save, resume,
or rollback.

```js
const bytes = await session.snapshot({          // canonical
                                        // scxml.session-snapshot/v2 JSON
  exclude: ['previews', 'invocations'],    // sections to omit (SNAPSHOT_SECTIONS)
  traceContext: 'strip',                // or 'preserve'
});
const diff = await diffSnapshots(before, after, { includeTraceContext: false });
```

- `session.snapshot()` gathers the machine, data-model, I/O-processor, and
  clock snapshot exports plus every session's external event queue, then
  encodes one
  canonical document through the `scxml-snapshot-codec` component. The codec
  is loaded lazily on first use through the session's core-module loader and
  runs on its own deterministic fuel budget (`DEFAULT_SNAPSHOT_FUEL`);
  `createSession({ snapshot: 'none' })` never loads it and every snapshot
  answers `codec-unavailable`. The virtualized codec is about 0.9 MB.
- A document is at most `MAX_SNAPSHOT_BYTES` (32 MiB); a larger one fails
  with `too-large` and is never truncated.
- For a failed or shut-down session, `snapshot()` throws
  `ScxmlSnapshotError` with code `status-invalid`. While deliveries are
  pending, the session is busy, or the last call did not end stable, it
  throws `not-stable`. Nothing is collected in either case.
- Errors throw `ScxmlSnapshotError { code }`; the codes are the closed
  `SNAPSHOT_ERROR_CODES` (`too-large`, `malformed-json`, `non-canonical`,
  `not-stable`, `status-invalid`, `codec-unavailable`, ...). The codec
  reports only the code: no pointer, name, or value crosses its boundary.
- The `snapshot` and `snapshot-diff` host operations are observed like every
  other host operation, with the output size as the `snapshot-bytes`
  measurement. Documents can hold data-model values and trace carriers, so
  they are never logged or indexed.
- `diffSnapshots(before, after, options)` encodes the canonical
  `scxml.session-snapshot-diff/v2` document of two valid documents; its
  output is byte-identical to `scxml snapshot diff --format json`.
- A page data model without a `snapshot()` function is rejected only when
  `system` or `data-model` is requested.
- A page data model's `name` is reported exactly like a bundled one's:
  `datamodel_name` and the `data-model-<name>` component role (with that
  role's release digest) whenever the machine initialized it, even when
  `system` and `data-model` are both excluded.

### Data model

`dataModel` implements the WIT `data-model-api` functions the host forwards
(see `DataModelProvider` in `src/index.d.ts`): `initialize(declarations,
systemValues)` (called once by the machine from `start`), `declare`,
`initializeData`, `setParameter`, `evaluateCondition(expression, event)`,
`evaluateValue(expression, event)`, `evaluateLocation`, `evaluateText`, `evaluateLog(expression, event)` (the
`<log expr>` text, SCXML 4.7), `evaluateFormValue`, `foreachItems`,
`assign(location, source, placement, event)`, `execute(script, event)`, the
pure `namelistKey`, `eventPayload`, `inlineContent`, `externalData`,
`eventDataField`, and `acceptsEventData`, and
may expose its stable `name` (`ecmascript`, `null`, or `xpath`; any other
value is `invalid-argument`, read once by `createSession`). It also implements
`updateActiveConfiguration(exited, entered)`, which the host forwards from the
machine's WIT `machine-capabilities` call `update-active-configuration` (not a
`data-model-api` function). A provider serves one session.
`event` is the WIT event, or `undefined` where SCXML defines no current event
(for example `<onentry>` run by `start`). `source` is the WIT `assign-source`:
`{ tag: 'expression', val }` is an `expr` the data model evaluates and assigns
in one call (with `_event` bound to `event`), `{ tag: 'value', val }` a value
the machine materialized for an existing location (inline `<assign>` children,
an `xml` value when they contain an element; `absent` for `delete`), and
`{ tag: 'foreach-value', val }` a `<foreach>` item or index, whose variable the
data model declares when it is not declared yet.
`placement` is the WIT `assign-placement` (XPath `<assign type>`/`attr`, SCXML 1.0 CR
2014-03-13 §C.3.6, in jco shape, for example `{ tag: 'replace-children' }` or
`{ tag: 'add-attribute', val: 'name' }`). A data model throws
`CapabilityError('execution')` for a placement it does not define. `In()` must answer from the configuration the
`updateActiveConfiguration` calls built: each removes its `exited` states, then
adds its `entered` states, starting from an empty configuration.

`createEcmaScriptDataModel()`, `createXPathDataModel()`, and
`createNullDataModel()` link the bundled `scxml_data_model_ecmascript`,
`scxml_data_model_xpath`, and `scxml_data_model_null` components (each
transpiled once) as complete, uninitialized providers. Pass the one the
document's Machine IR records (its `datamodel`, or the default
`ecmascript` when it names none); the host does not choose. Their behavior:

- `event` is forwarded as the WIT `option<event>`. `undefined` is `none`,
  which leaves `_event` unbound (SCXML 1.0 section 5.10); the adapter never
  fabricates an event.
- The component reads no ambient clock or randomness. `Date` reads the
  session clock through `execution-host.now-ms`: the session's start epoch
  (the wall-clock time an `event-loop`-timer session was created, and
  `BATCH_START_EPOCH_MS`, 2015-09-01T00:00:00Z, for a `manual`-timer
  session) plus its logical time; a provider joined to no session reads 0.
  `Math.random` is a deterministic sequence seeded from `_sessionid`, and
  Boa's hash seed is pinned. The component keeps one realm for the whole
  session (the root README lists the implementation-defined choices).
- A trap while instantiating throws `ScxmlHostError('instance-lost')`
  (`'fuel-exhausted'` when the budget ran out). Before `initialize` the
  component answers evaluations with `unavailable`. Evaluation failures throw
  `CapabilityError` with the component's closed code; malformed arguments
  throw `CapabilityError('invalid-input')` (pure calls answer `undefined`,
  except `acceptsEventData`, which answers `true`).
- Each call is capped by the factory's `fuel` (default `DEFAULT_FUEL`).
  Used without a session, each call is a host call with a fresh budget;
  linked into a session, each call is also charged to the budget of the
  session's host call that made it (see [Fuel](#fuel)). A call that traps,
  including fuel exhaustion, discards the instance: it and every later fallible call throw
  `CapabilityError('platform')`, which fails the calling machine session
  (`ScxmlSessionError`, `sessionFailed: true`), and every later pure call
  answers `undefined`, except `acceptsEventData`, which answers `true`,
  exactly as the Wasmtime host poisons a trapped store.
- The component's `execution-host` import is answered for `In()` only.
  Effects (`<raise>`, `<send>`, `<cancel>`, `<log>`) are machine
  capabilities, not data-model imports, so the component's `raise`, `send`,
  and `cancel` imports fail closed with `unavailable` and `log` records
  nothing; each such call is observed as a rejected `capability` operation.

The smoke test replays every recorded call of the shared fixture through the
component and runs the whole representative session on it. Pages may still
supply their own provider with the contract above.

A data-model package (`<scxml datamodel="NAME" datamodel:src="URI">`,
[namespaces/datamodel](../../namespaces/datamodel/README.md)) runs here as
extension components do: only from a component this package ships
transpiled and fuel-metered at build time. The page passes the provider of
that component as `dataModel` (the example `datamodels/example`
repackages the null data model, so `createNullDataModel()` runs it); the
host resolves no package at run time, and an invoked child whose
`datamodel` names a package gets no data model of its own (only the bundled
`ecmascript`, `null`, and `xpath` are instantiated for children). Run-time
package resolution stays with the CLI and the Wasmtime host
(`SessionHost::with_data_model_components`).

### Fuel

Browsers expose no Wasm fuel, epoch interruption, or deadline, and Boa's
loop limit (100 000 iterations) restarts in every call frame, so a loop that
calls a looping function would run about 1e10 iterations and hang the page.
The build therefore meters every transpiled core module with the
in-workspace `scxml-wasm-meter` tool: each module gets an exported `i64`
counter (`FUEL_EXPORT`) that it decrements at every function entry and every
loop iteration, trapping when it reaches zero.

- One host call, one budget. Every host call (a session command or query,
  a timer or `advanceTime` delivery, `receiveHttp`, a pre-warm compile, and
  each standalone data-model or `createCompiler` call) runs on one budget:
  `fuel` on `createSession` (default `DEFAULT_FUEL`, 75 000 000 charges),
  shared by every component the call reaches. The data-model, I/O processor,
  clock, BasicHTTP (including the loopback delivery a send causes),
  `<invoke>` compiler, and child-machine calls a machine command makes are
  charged to that command's budget (including every child start, cancel,
  and continuation the host carries out for an `<invoke>` request the
  command deferred), exactly as the Wasmtime session host's
  call budget charges its stores: entering a component grants it the
  smaller of its own per-call cap and what is left; calling out of it into
  a nested component, and leaving it, charges what it ran; returning into
  it grants it the smaller of its own remainder and what is left. A nested
  component that exhausts the shared budget leaves its caller nothing, so
  the command fails with `ScxmlHostError('fuel-exhausted')` and the session
  is lost.
- Per-call caps: every instance the session creates is capped by `fuel`,
  a bundled data model by its factory's `fuel` (default `DEFAULT_FUEL`),
  and the `<invoke>` compiler by `invoke.fuel` (default
  `DEFAULT_COMPILE_FUEL`, 4 000 000 000). A data model that exhausts only
  its own cap is poisoned and fails the machine session
  (`ScxmlSessionError('platform')`); the session's other components keep
  running. `createCompiler` compiles on `fuel` (default
  `DEFAULT_COMPILE_FUEL`) per compile, and the snapshot codec on
  `DEFAULT_SNAPSHOT_FUEL` per call (it reaches no session component, as
  under Wasmtime). Page providers run as page JavaScript and are not
  metered.
- Units. A charge is one function entry or loop iteration of a metered core
  module, a coarser unit than Wasmtime's instruction fuel. One component
  is several core instances (the guest's code plus jco's small adapter and
  trampoline modules), each with its own counter; a grant sets them all,
  and the component is charged the largest count one of them spent. So
  within one host call the components' charges sum to at most `fuel`, and
  no single core instance executes more than `fuel` charges.
- Default sizing, in Node 24 with the shipped components, as the largest
  single host call of each workload (the smallest budget it runs under;
  `node scripts/fuel-sizing.mjs` bisects and prints them, as
  hosts/scxml-host-wasmtime/tests/fuel_sizing.rs does under Wasmtime): the
  ECMAScript data model creates its Boa realm once, at `initialize`, and
  keeps it for the whole session, so an ECMAScript session's start takes
  about 2.3 million charges, which also covers eight non-assignment
  `<script>`s in one transition and the representative fixture's start; a
  `<foreach>` with an `<assign>` over 60, 90, and 1 000 items in the start
  takes about 2.5, 2.5, and 4.3 million; and a 64-region parallel state
  whose regions all transition 0.32 million (its start and one event).
  75 000 000 admits all of them with at least 17 times headroom (the
  1 000-item `<foreach>`; the others 29 times or more), and the start of an
  ECMAScript root around a 4 000-state inline `<invoke>` child, 22.4
  million, with 3.35 times. It is the Wasmtime default divided by 40, the
  ratio the two defaults had before the Wasmtime default rose to
  3 000 000 000. One charge is about 31 to 49 Wasmtime fuel
  units on the ECMAScript and parallel workloads and about 77 on the child,
  so the two defaults do not convert exactly; each host admits every sizing
  workload (the Wasmtime default is 3 000 000 000). The
  Wasmtime host test's heavy document (ten `<foreach>` loops over 200 items,
  each running a 500 000-iteration `<script>`, about 2.3e10 charges) stops
  with `fuel-exhausted` after about 0.3 s of `start` in Node 24 on an
  Apple M3 Max, and a 64-region parallel machine whose regions toggle on a
  `<raise>` loop after about 0.21 s (a 64-region eventless one, which the
  Wasmtime host's command bound stops first, after about 0.19 s). Charges
  run at about 4 ns each, so the default bounds one runaway host call to
  roughly 0.3 s of Wasm.
- The count depends only on the executed path, so exhaustion is
  deterministic for identical inputs, budgets, and builds. The budget bounds
  work, not wall-clock time.
- Instantiation outside a call (`createSession`, the factories) runs on the
  budget the build bakes into every module (`DEFAULT_FUEL`); an invoked
  child's instances are built inside the invoking command and charged to it.
- A module without the fuel export fails instantiation (`instance-lost`), so
  an unmetered build never runs.

### Telemetry

The host emits OpenTelemetry through the OpenTelemetry JS API
(`@opentelemetry/api`). It installs no SDK, provider, propagator, or
exporter: the page owns those, just as native hosts own SDK installation in
Rust. `createSession`, the data-model factories, `createCompiler`,
`fetchCoreModules`, and `cachedCoreModules` take an optional
`telemetry: {tracerProvider?, meterProvider?}`. Omitted providers are the
global API providers (`trace.getTracerProvider()`,
`metrics.getMeterProvider()`), which stay no-op until the page registers an
SDK. A malformed `telemetry` option is `invalid-argument` (a `TypeError` for
the loaders). Provider exceptions are contained and never change results,
errors, or machine behavior.

Trace context propagates in both directions. A guest's W3C carrier is
validated with the Rust facade's rules: a canonical version-00
`traceparent`, non-zero ids, flags `00` or `01`, and a `tracestate` of at most
512 bytes and 32 members. The carrier parents the guest span, and `start`
returns the new span's own carrier to the guest. An invalid carrier opens
nothing: `start` returns id `0` and no carrier, and a point signal is dropped.
A guest span without a carrier parents to the innermost open host span, so
`command` → `capability` → `data-model` → guest spans form one trace. The
outermost host span parents to the page's active context.

Trace context also propagates through events, as under the Wasmtime host
([CONTRIBUTING.md](../../CONTRIBUTING.md#trace-propagation-through-events)):
each event carries the carrier of the machine batch span that emitted it,
and the batch that processes it is that span's child, for `<raise>`, self
sends (immediate and delayed), `#_<invokeid>`, `#_parent`, autoforward,
`done.invoke` (named by the child's finishing batch through
`callResult.doneTraceContext`), and send errors. A page passes its own carrier
as `send({name, traceContext})`; the machine validates it (a malformed one
is `invalid-input`). Without an SDK the API's no-op tracer answers a
parent with that same span; the machine then passes the page's carrier
through unchanged, so installing no telemetry changes nothing.

### Telemetry cardinality

Every indexed value is a closed label. The names match the Wasmtime host's
for the same facts, and `test/contract.test.mjs` pins them against the Rust
sources.

| Signal | Name | Attribute or field | Values | Cardinality |
| --- | --- | --- | --- | --- |
| host spans | tracer `scxml-host-browser`; span name = `scxml.operation` | `scxml.module` | `host-browser` | 1 |
| host spans and counter | | `scxml.operation` | `create-session`, `ir-source`, `command`, `capability`, `data-model`, `compile`, `load-core-module`, `delayed-send`, `invocation`, `snapshot`, `snapshot-diff`, `validate`, `extension-load`, `extension-instantiate`, `extension-execute`, `extension-shutdown`, `package-validator-validate`, `service-start`, `service-send`, `service-cancel`, `service-http-fetch`, `service-http-send` | 22 |
| host spans and counter | | `scxml.outcome` | `ok`, `rejected`, `failed` | 3 |
| host counter | `scxml.host.operations` (`{operation}`) | the three attributes above | 1 × 22 × 3 | ≤ 66 series |
| guest spans and point signals | tracer `scxml-component-guest`; span name = `scxml.operation` | `scxml.module` | `machine-component`, `data-model-component`, `compiler-component`, `snapshot-codec-component`, `validator-component`, `executable-content-extension`, `invoked-service` | 7 |
| guest spans | | `scxml.operation` | WIT telemetry `operation` enum (16), compiler `operation` enum (`compile`), validator `operation` enum (`validate`) | 18 |
| guest spans | | `scxml.outcome` | WIT `outcome` enum (`ok`, `rejected`, `failed`) | 3 |
| compiler guest spans | | `error.type` | compiler WIT `error-category` enum | 5 |
| validator guest spans | | `error.type` | validator WIT `error-category` enum (`invalid-input`, `validation`, `report`) | 3 |
| guest span events | event name | | WIT `signal-event` enum (11; the compiler's 2 events are among them) | 11 |
| capability guest spans and point signals | tracer `scxml-capability-guest` (the Wasmtime host's); span name = `scxml.operation` | `scxml.module` | `capability-component` | 1 |
| capability guest spans | | `scxml.operation` | WIT telemetry `operation` enum (the SCXML I/O processor), clock `operation` enum (7), BasicHTTP `operation` enum (4) | ≤ 27 |
| capability guest spans | | `scxml.outcome` | WIT `outcome` enum; clock `outcome` enum (`ok`, `rejected`) | 3 |
| capability guest span events | event name | | WIT `signal-event` enum, clock `signal-event` enum (6), BasicHTTP `signal-event` enum (6) | ≤ 23 |
| guest measurements | `scxml.component.effects`, `.input.size`, `.snapshot.size`, `.queue.depth`, `.fuel` histograms | none | numeric value only | 0 labels |

A non-`ok` outcome also sets the span status to error. The span status
message is the same closed outcome or error category. Span ids, trace ids,
and handles are identifiers and are never attributes. Payloads, expressions,
state ids, IR, messages, and caller values are never recorded. At most 1024
guest spans are open per adapter; beyond that, `start` returns id `0` and
records nothing.

## Tests

```console
sh scripts/verify-browser-host.sh              # from the repository root
sh scripts/verify-browser-host.sh --skip-e2e   # skip Chromium (prints SKIPPED)
```

The script is the supported entry point: it builds the release guests,
`dist/`, and the representative fixture's lowered IR before it runs the
tests. `npm test` alone runs the same Node suites but needs both inputs
already built, and `SCXML_BROWSER_IR` naming the lowered IR (the smoke and
browser tests fail with that message without it):

```console
cd hosts/scxml-host-browser
SCXML_BROWSER_IR=../../target/wasm-artifacts/release/packaged/representative-machine.scxml-ir.json npm test
```

- `test/capabilities.test.mjs` runs in Node on the same modules. It checks
  data binding and the forwarded evaluations on documents for each bundled
  data model (ECMAScript, XPath, null), a data-model document without a data
  model (`unavailable` at start), delayed delivery at the deadline and
  cancellation (`manual` timers), delivery by the event loop's own timer,
  `error.communication` for an unreachable target and `error.execution` for
  an invalid one, processor-stamped self sends, an invoked child from
  pre-lowered IR (parameters, its delayed send on the shared clock, its
  event to the parent, `done.invoke` with `<donedata>`, attributed trace),
  a child the page does not pre-lower (error.execution), a failing child
  failing the run, the automatic child compiler (an inline child
  pre-warmed so `start` compiles nothing, the pre-warm's exact sources, the
  `(sourceName, source)` cache and its eviction, compile diagnostics and the
  source bound as error.execution, per-compile fuel exhaustion failing the
  run, a page `compileInvocation` overriding it, a grandchild `<invoke>`
  reporting `done.invoke` at each level, and the invoke limits as
  error.execution), the invocation order and invoke limits, the typed `unsupported-invoke` (error.execution
  under `compile: 'provided'`) path,
  documents supplied or denied through `loadDocument`, the command budget
  and trace, and option validation.
- `test/resources.test.mjs` runs in Node on the same modules. It checks the
  `resources` option: deny-all without it; the allowlist gating every
  `resolve` (only an exact `true` admits a URL); `allow` and `resolve`
  getting an http(s) URL's canonical form (`..`, `%2e%2e`, and backslash
  segments cannot leave an allowed path; userinfo never reaches `allow`)
  and any other URL as written; the asynchronous prefetch
  before `start`, with synchronous service from the cache; the URL scan over
  hostile IR; the per-document, per-script, URL-count, and total size
  bounds; the Wasmtime error mapping for data, script, and invoke loads;
  `<invoke src>` text handed to the child compiler; runtime `srcexpr` URLs
  (`dynamic-url-unavailable`, or the page's `loadDocument`); option
  validation; and the bounds against their Rust sources.
- `test/basichttp.test.mjs` checks the BasicHTTP processor: deny-by-default
  (no `http`: the platform `error.communication`, nothing fetched), the
  allowlist (only an exact `true` from `allow`), the canonical target that
  `allow`, `send`, and the default `fetch` get (dot segments cannot leave an
  allowed path; userinfo never reaches `allow`), the component's request
  encoding, `error.communication` for every undeliverable send (and later
  for a failed fire-and-forget send), `error.execution` for an invalid
  target, the delayed-send failure, the default `fetch` options, inbound
  decoding and every response status of `receiveHttp`, the loopback order,
  the loopback across invoked sessions (parent to child, child to parent,
  and a child to itself) and an external request to a live child and to
  a grandchild,
  re-entrancy, and `_ioprocessors`.
- `test/async-bridge.test.mjs` runs in Node. It checks mechanism detection
  and the `async` option contract, and the blocking channel between real
  threads (`Atomics.wait` in a `worker_threads` thread while the page
  answers asynchronously, with a reply that grows the buffer). Through
  `test/node-worker.js`, a Worker stand-in on `worker_threads`, it runs the
  `worker` mechanism end to end: a `<data src>` document prefetched in the
  page and a runtime-computed `<invoke srcexpr>` child load through an
  asynchronous `resources.resolve`, denied and failing URLs are
  `error.execution`, as are a `resolve` rejection of any code and a
  malformed runtime answer (`unavailable`), and a call back into the
  session is `reentrant-call`. A runtime http(s) URL reaches `allow` and
  `resolve` in its canonical form, and runtime URLs have the prefetch's
  URL byte bound (no bound on their count or total text). The page's prefetch reaches the
  worker with the synchronous session's answers (a loaded script, a failed
  script fetch as `platform`, an empty script as `payload-too-large`, a
  script over 64 KiB loaded (its run then `error.execution` under the data
  model's bound), and a document over 4 MiB loaded),
  each static URL resolved once. It also covers the `none` fallback (static
  URLs still prefetched, the typed `dynamic-url-unavailable` error for a
  runtime one), and checks that `async: 'off'` is the synchronous session.
- `test/service.test.mjs` runs in Node with `--experimental-wasm-jspi`
  (in one process, `--test-isolation=none`, since test workers do not
  inherit the flag) on `scxml_service_example` and `scxml_service_files`.
  It checks authentication and binding, an unbound type
  (`error.execution`), events both ways with the service's timer and
  `done.invoke`, cancellation when the invoking state exits, outbound HTTP
  admission (references substituted, a supplied origin, no transport),
  undeclared env slots, WASI planning against the grants, an in-memory
  directory the service reads and writes, a trap after the start
  (`error.communication`), and the snapshot and async-session refusals.
- `test/snapshot.test.mjs` runs in Node. It checks the read-only session
  snapshot API (see [Session snapshots](#session-snapshots)): argument
  validation, `codec-unavailable`, checkpoint gating (`not-stable`,
  `status-invalid`), the lazily loaded and cached codec, data-model names
  and roles, telemetry labels, and the closed vocabularies against the WIT
  and the `scxml-snapshot` crate.
- `test/snapshot-parity.test.mjs` is the golden parity gate. It replays
  every `tests/fixtures/session-snapshot/<name>.ops.json` through this host,
  once with BasicHTTP configured as the Wasmtime session host links it and
  once without `http`, and requires each checkpoint's document to equal the
  Wasmtime harness's golden byte for byte after removing only the
  documented host differences (component digests, the BasicHTTP
  `_ioprocessors` entries in the run without `http`, and preserved trace
  carriers); it asserts that nothing else changed and that each fixture
  compares at least one checkpoint.
- `test/irp-parity.test.mjs` runs every official W3C IRP case of both
  prepared corpora (ECMAScript and XPath) that the `Outcome` oracle decides
  (194 ECMAScript and 172 XPath cases: the 35 `<invoke>` cases of each included, with child
  documents compiled by the host's automatic compiler (the harness supplies
  no `compileInvocation`), and the 12 BasicHTTP cases
  of each, over the Wasmtime host's in-process loopback: `http.send` routed
  to `session.receiveHttp`) through this host with `manual` timers, and
  requires the disposition and final configuration the checked-in Wasmtime
  baselines record. Static `src` documents (8 ECMAScript and 5 XPath cases) load through
  the `resources` option, allowlisted to each case's prepared dependencies;
  only runtime `srcexpr` URLs (case 216) use the synchronous `loadDocument`.
- `test/smoke.test.mjs` runs in Node, which executes the same ES modules and
  core Wasm a browser loads. It checks:
  - the CLI-compiled IR digest equals the fixture's;
  - the runtime-IR session reproduces
    `tests/fixtures/representative-machine.trace.json` exactly, including
    every data-model call's expression, `_event`, and `In()` configuration;
  - OpenTelemetry spans and metrics carry only closed labels, and guest
    spans join the host trace;
  - in-page compilation emits the CLI's exact IR bytes;
  - hostile IR (oversized, wrong version, malformed, dangling, unknown field)
    fails with typed codes;
  - lifecycle rejections and shutdown behave as specified;
  - the bundled ECMAScript component answers every recorded data-model call
    (including the no-event `<onentry>` call during `start`) identically and
    runs the whole representative session to the fixture trace.
- `test/contract.test.mjs` runs in Node on the same modules. It checks:
  - a hostile nested-loop script stops on the data-model fuel budget with
    `CapabilityError('platform')`; in a session it exhausts the command's
    budget (`fuel-exhausted`), or, under a smaller data-model cap, only its
    own call, which fails the machine session closed;
  - exhaustion verdicts are identical on fresh instances, and machine,
    data-model, and compiler budgets are each enforced per call;
  - one budget per host call: nested data-model work is charged to the
    machine command that made it (a command whose data-model calls each fit
    the budget but together do not fails deterministically), the Wasmtime
    host test's heavy document stops inside `start` at the default, and the
    default runs 90 `<foreach>` iterations with an `<assign>` in one command
    and a 64-region parallel toggle;
  - invalid `fuel`, `send`, `compile`, and data-model arguments
    are rejected without losing the instance, and unmetered core modules
    never run;
  - `send` rejects an `invokeId` and any non-`external` type with
    `invalid-input`, so a page cannot forge an invoked child's event or run
    its `<finalize>`;
  - `src/index.d.ts` declares exactly the runtime exports, and the closed
    vocabularies (`ERROR_CODES`, `STATUSES`, `COMPILER_ERROR_CODES`,
    `SCHEMA_POLICIES`, `COMPILE_STATUSES`), `MAX_MACHINE_IR_BYTES`, and
    `FUEL_EXPORT` match their WIT and Rust sources;
  - the telemetry scope, `scxml.module`, outcome, and instrument names match
    the Wasmtime host and the Rust facade; the capability record types in
    `src/index.d.ts` match the jco shapes of their WIT records;
  - events keep their W3C lineage: `<raise>`, immediate and delayed self
    sends, send errors, `#_parent`, `#_<invokeid>`, `done.invoke`, and a
    page-supplied carrier each parent the batch that processes them, and a
    page carrier under the no-op API changes nothing;
  - guest trace context is validated, parents the guest span, and
    propagates; open guest spans are bounded; failing providers change
    nothing; and rejections by the pure helpers, which emit no telemetry of
    their own, are observed by the calling host operation.
- `test/browser.test.mjs` serves `src/`, `dist/`, the fixture, and the IR on
  `127.0.0.1` and runs the same trace, a hostile-IR check, the hostile
  nested loop (which must stop on its fuel budget), the Wasmtime host
  test's heavy document (whose nested data-model work must exhaust one
  `start`'s budget), an in-page compiled
  XPath document whose delayed send the page's event loop delivers, an
  `<invoke>` whose child the host instantiates synchronously on the page's
  main thread from pre-lowered IR, an inline `<invoke>` the page does
  nothing for (the host's pre-warmed automatic compiler), a `<data src>`
  the page allowlists through `resources` and fetches over HTTP before the
  machine runs (a denied URL is never fetched), and BasicHTTP (the default `fetch` to
  allowlisted endpoints of the test server, without the page's cookie, a
  500 answered with a later `error.communication`, a denied URL the server
  never sees, and one `receiveHttp`), in headless Chromium through
  `playwright-core` with the default fetch loader. It then serves
  `test/browser/async.html` twice. The first run is cross-origin isolated
  and gets the `worker` mechanism: a `<data src>` document prefetched in the
  page and a runtime-computed `<invoke srcexpr>` child load through a
  fetching `resources.resolve`, and the child's delayed send reaches
  `onTimer`. The second run is not isolated and gets the `none` fallback:
  the prefetched document still loads, and the runtime URL is the typed
  error. For worker requests the server resolves the one bare specifier,
  as a bundler would. Install
  the pinned shell once with
  `npx playwright-core install --only-shell chromium`.

The shared fixture comes from the Wasmtime host's `machine_ir_modes` test,
which requires the machine component to reproduce it in both packaged and
runtime IR modes. Regenerate it only
after an intended semantic change, with
`sh scripts/bless-fixtures.sh --trace-fixture` (run from the repository root).

## License

The package is `Apache-2.0 OR MIT`; `npm pack` copies the repository's
`LICENSE-APACHE` and `LICENSE-MIT` into the tarball (the `prepack` script).
The package is not published to npm. It ships no third-party schema. The
vendored W3C SCXML 1.0 XSD (`crates/scxml-validator-xsd/schemas/w3c-scxml-1.0`,
under its own W3C notice) is compiled only into `BundledSchemaResolver`,
which the compiler component does not use: it resolves schemas through a
`MemorySchemaResolver` of caller-supplied documents, so the linker drops the
schema bytes and `dist/` contains none of them. `test/contract.test.mjs`
pins that absence against every line of the vendored schema set and its
NOTICE; a build that embedded them would need the W3C terms in this
package's license and its `files`.
