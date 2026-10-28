# Architecture

How an SCXML document becomes a running session: the compiler pipeline, the shipped Wasm components and their WIT contracts, the data models, validation and extension packages, and what the hosts do. The [README](../README.md) has the overview; resource bounds and implementation-defined choices are in [limits.md](limits.md).

## Wasm-first pipeline

```text
SCXML XML + extension package manifests + supplied XSD bytes
             │
             ▼
 stateforward:scxml-validator/validator
   XML well-formedness + XSD 1.0 + bounded semantic rules
             │
             ▼
 stateforward:scxml-compiler/compiler
   parse → validate → deterministic Machine IR → package recipe
             │
             ▼
 lowered-ir (Machine IR JSON: portable data, not Wasm)
             │
             ├── runtime mode: host serves the bytes through ir-source
             │   (Wasmtime host and hosts/scxml-host-browser; browsers
             │   ship the generic machine once)
             │
             └── packaged mode (scxml-packager, non-browser AOT hosts):
                 data-only ir-source provider (IR in one data segment)
                 composed with the pinned prebuilt generic machine
                 → validated component-wasm
             │
             ▼
 stateforward:scxml/machine
   generic engine decodes, bounds, version-checks, and validates the IR,
   then runs it inside the Wasm component; imports narrow capabilities
             │
             ▼
 stateforward:scxml-clock/clock
   injected logical time + bounded deterministic delayed-send queue
             │
             ▼
 thin native/browser host links data model, I/O processor, extension
 component, telemetry, clock/storage/network, and invoked-session
 capabilities as needed
```

The compiler never invokes `rustc` or any native process and never generates
code, so it runs unchanged inside Wasm (including a browser). It emits one of
two phases:

- `lowered-ir`: canonical Machine IR JSON (portable data, never described as
  Wasm) plus a deterministic recipe naming the generic machine template world
  and version, the IR SHA-256, the `ir-source` interface, linked extension
  component artifacts, and the next phase `component-wasm`.
- `component-wasm`: produced by `scxml-packager` from that IR and a
  caller-supplied, pinned, prebuilt generic machine template (bytes plus
  SHA-256; the compiler never reads the filesystem, so template bytes arrive in
  the request). The packager checks the template's size, digest, component
  validity, and exact machine world, encodes a data-only provider component
  (one core module whose non-growable memory holds the IR in one active data
  segment and whose only function returns a constant pointer) with
  `wasm-encoder`, and composes it with the template using `wac-graph`. The
  result is validated, imports only `types`, `telemetry`, and
  `machine-capability-types`, exports `machine-api`, and is byte-deterministic
  for identical inputs. Its recipe records the template SHA-256 and IR SHA-256
  and has no next phase.

Browsers never use packaged components: they ship the generic machine
component once and feed each machine's IR as data through `ir-source`.

## Components and declarative resources

The default data model is ECMAScript (SCXML 1.0 §3.2.1: the `datamodel`
attribute's default is "platform-specific"; §3.2.2: "Platforms SHOULD
document their default data model"): a document without a `datamodel` attribute is lowered
with `ecmascript` recorded in its Machine IR, every host links the ECMAScript
data model for it, and the validator checks it exactly as
`datamodel="ecmascript"`. The one official IRP document that declares none,
test 301, is by its own comment "valid only for datamodels that support
scripting". Name `null`, `ecmascript`, or `xpath` to select another.

Executable product semantics are Wasm components:

- `components/scxml-machine`: generic machine guest containing `scxml-engine`;
  it embeds no machine and loads Machine IR through `ir-source`;
- `components/scxml-clock`: deterministic logical clock and bounded scheduler guest;
- `components/scxml-data-model-ecmascript`: bounded Boa 0.22 ECMAScript data-model guest;
- `components/scxml-data-model-null`: null data-model guest (SCXML B.1): the `In()` condition through the imported execution host, and no value, location, or script evaluation (B.1.3-B.1.5);
- `components/scxml-data-model-xpath`: bounded XPath data-model guest (SCXML 1.0 Candidate Recommendation of 2014-03-13, Appendix C.3; the final Recommendation omits the XPath data model) over the pure `scxml-xpath` XPath 1.0 evaluator;
- `components/scxml-ioprocessor-basichttp`: `BasicHTTP` Event I/O Processor
  guest (SCXML C.2); it reaches a network only through the embedder-supplied
  `stateforward:scxml-basichttp/http-transport` import and imports no WASI
  socket or HTTP capability;
- `components/scxml-ioprocessor-websocket`: the WebSocket Event I/O
  Processor, a component of the `stateforward:scxml/event-processor` world
  that a package binds to its type URI
  ([event-io-processors.md](event-io-processors.md)); it reaches a network
  only through host-admitted `outbound-websocket` connections;
- `components/scxml-validator`: validation guest; its parse-only
  `expression-syntax` import is answered by a data-model component (the
  Wasmtime host's `ComponentValidator`), and its build composes the fixed
  `wasi:random` provider `components/scxml-validator/wasi-random.wat` that
  seeds the XML Schema backend's hash tables;
- `components/scxml-compiler`: validation, lowering, and codegen-free
  `component-wasm` packaging guest;
- `components/scxml-script-example`: example Wasm script component for
  `<script src>` (reads `input`, assigns `doubled` through `data-model-host`);
- `components/scxml-extension-example`: example extension component
  (`https://stateforward.org/scxml/example`, with its XSD and package
  manifest in `components/scxml-extension-example/package`) with one element per extension
  component capability (`raise`, `send`, `fail`, `spin`);
- `namespaces/scx`: the `scx` extension namespace
  (`https://github.com/stateforward/scxml.rs/namespaces/scx`, tagless and
  versioned with the toolchain): `<scx:validate>` runs the validator as
  executable content; `scx` embeds its package and component (see
  [namespaces/scx/README.md](../namespaces/scx/README.md));
- `namespaces/env`: the host's `env` attribute vocabulary
  (`https://github.com/stateforward/scxml.rs/namespaces/env`, unversioned and
  never fetched), documented with a schema for tools; not a package (see
  [namespaces/env/README.md](../namespaces/env/README.md));
- `namespaces/event`: the host's `event` vocabulary
  (`https://github.com/stateforward/scxml.rs/namespaces/event`, unversioned
  and never fetched): `event:schema` (a JSON Schema, inline or from a file
  relative to the document, with an optional JSON Pointer) or
  `event:schemaexpr` on `<transition>`, `<send>`, `<raise>`, and
  `<donedata>`, describing the event data each handles or creates, and
  `<event:defer/>`, which the engine executes itself (Machine IR `defer`):
  it holds a transition's event until the transition's source state is
  exited or a state inside it that handles the event is entered, then
  queues it on the internal queue again. The pure
  `scxml-event-schema` crate (behind the validator and compiler, with the
  `boon` crate's metaschema checks and compilation, with a URL loader that
  refuses every URL) resolves each
  schema; the compiler embeds it, as canonical JSON with its SHA-256, in the
  Machine IR on the owning element, and the engine copies a transition's
  schema into session snapshots. The engine checks the data an executed
  `<send>` or `<raise>` creates against its own literal schema and, for an
  event the session delivers to itself, those of the transitions handling
  it (`crates/scxml-engine/src/contracts.rs`, compiling each with the same
  crate); a rejected event is not dispatched and raises `error.execution`.
  For an event bound for another session, or the host's own input, the
  host asks the receiving session (`session.accepts-event-data`, answered
  by the same engine code) before it routes or delivers it, so no host
  judges an event itself. Transition selection never reads it. Not a package (see
  [namespaces/event/README.md](../namespaces/event/README.md));
- data models, I/O processors, invoked services, and extension components.

Declarative resources are data, not executable substitutions for components:
SCXML/XML, XSD, extension package manifests, configuration, and lowered Machine
IR. Rust libraries have native builds only for fast tests and CLI composition.

## WIT packages

The canonical WIT (under [`crates/scxml-wit/wit`](../crates/scxml-wit/wit)) is split by independent versioning boundary; the
version of every contract is in the [README](../README.md#versions):

- `stateforward:scxml@0.2.0`: runtime/capability and machine lifecycle,
  and the worlds of invoked services and Event I/O Processor components
  (`event-processor`, with `outbound-http` and `outbound-websocket`);
- `stateforward:scxml-clock@0.2.0`: logical time, scheduling, cancellation, and due draining;
- `stateforward:scxml-basichttp@0.2.0`: the `BasicHTTP` Event I/O Processor
  (`basichttp-api`) and the narrow `http-transport` capability it imports;
- `stateforward:scxml-validator@0.2.0`: structured validation input and canonical artifact bytes;
- `stateforward:scxml-reporter@0.2.0`: generic versioned artifact bytes to rendered bytes;
- `stateforward:scxml-compiler@0.2.0`: compile request and artifact phases.

## Reports and reporters

The validator emits exactly one `scxml.report/v3` JSON artifact. It preserves the
complete source name and UTF-8 source text so separately deployed rustc and SARIF
reporters can produce precise snippets without rerunning validation. Canonical
artifacts are therefore privacy-sensitive and must receive the same access,
retention, and redaction treatment as their source documents.

Rustc and SARIF reporters consume only those canonical producer bytes. Their
deployable Wasm artifacts are deny-by-default virtualized and import only the
versioned reporter telemetry and types interfaces; they receive no filesystem,
network, clock, random, environment, CLI, or stream capability.

## The machine component

The machine world imports `ir-source` (`machine-ir: func() -> list<u8>`) and
exports a `session` resource with initialize-by-constructor, `start`,
`start-invoked`, external `send`, `deliver-from-invocation`, `resume`, `cancel`
(W3C `exitInterpreter`: runs `<onexit>` of the active configuration and cancels
its invocations without `done.invoke`; precondition status `running`),
`configuration`, `status`, `failure` (the cause of a `failed` session), and
`shutdown`. Construction fetches the IR once and rejects more than 16 MiB
(`payload-too-large`), another Machine IR version (`unsupported-version`),
malformed or structurally/semantically invalid IR (`invalid-input`), and valid
IR that uses constructs the engine does not implement (`unavailable`), each as a `failed` session
through the existing session-failure contract. Construction also receives an
owned narrow `machine-capabilities` resource for event-aware condition/value
evaluation, assignment (an `assign-source` the data model evaluates and assigns
in one call, a materialized value for an existing location, or a `<foreach>`
item or index value whose variable the data model declares, with its
`assign-placement` (XPath `<assign type>`/`attr`, SCXML 1.0 CR 2014-03-13
§C.3.6); the bundled ECMAScript and null data models answer every placement
other than `replace-children` with `execution`, so `error.execution`), inline
execution, external script load/run, send/cancel, logging, executable-content
extensions, data binding and document loading, and child invocations
(`start-invocation`, `cancel-invocation`, `forward-to-invocation`), with
`start-invoked` and `deliver-from-invocation` on `machine-api` for invoked
sessions. Internal `<raise>` events remain engine-owned queue work.
`session.send` accepts only `external` events without an invoke id
(`invalid-input` otherwise), so invocation events enter only through
`deliver-from-invocation`. There is no save or restore of session state: SCXML
defines no persistence.

## Session snapshot exports

Session snapshots are read-only introspection for tools and LLMs
([session-snapshot.md](session-snapshot.md)). The WIT
contract is frozen: `session.snapshot(options)` returns the machine's image
(status, atomic and full configuration, internal queue, history values,
bound late-binding states, live invocations, the invoking parent, id
counters, and the transitions an event could trigger with their exit and
entry previews and `event` vocabulary schemas, conditions and
`event:schemaexpr` never evaluated); `data-model-api.snapshot`
dumps the system variables and each declared `<data>` id's value (a value
with no portable form by its type and string form); `clock-api.snapshot` returns logical time
and live timers; and `io-processor-api.snapshot` decodes the pending delayed
sends. Each export applies no effect, evaluates nothing, calls no
capability, charges no command, and reports no telemetry observation. Hosts
gather these records at stable checkpoints and encode one canonical
`scxml.session-snapshot/v2` JSON document per checkpoint through the pure
`scxml-snapshot` crate, whose JSON Schemas live in
`crates/scxml-snapshot/schema/`; the optional capability-free
`scxml-snapshot-codec` component exports `encode` and `diff` for the browser
host. All four component exports, the codec, and the Wasmtime host's
gathering are implemented: the Wasmtime host takes read-only snapshots at
scheduler checkpoints (`SessionRun.snapshots`), the CLI's `snapshot run`
executes a document and writes its checkpoint documents, and the browser host
loads the codec lazily. `conformance run --snapshot-every-checkpoint` proves
over both prepared IRP corpora that collecting one read-only document per
checkpoint never perturbs the judged run, that every document passes the
codec checks, and that two runs produce identical documents
(`target/snapshot-proof/<datamodel>.json` under the current directory
summarizes the counts and digests); the plain report is then judged like
any run, baselines and gates included.

## Component authentication and compilation

The capability proof builds release guests, virtualizes each one
twice with pinned `wasi-virt`, rejects byte drift, validates and import-allowlists
the result, and records the exact byte count and SHA-256 in a versioned artifact
manifest. The Wasmtime execution test reads and rehashes the machine entry from
that manifest, then instantiates those exact virtualized bytes without a WASI
linker. Production host execution requires a caller-trusted SHA-256 identity for
the complete deterministic manifest. Callers acquire manifest and artifact
bytes through their own explicit capability boundary; the host authenticator
performs no I/O. It accepts only the manifest's closed component identities and
canonical relative filenames, bounds the manifest to 64 KiB and each component
to 2 GiB − 1 bytes (`MAX_AUTHENTICATED_COMPONENT_BYTES`, GitHub's per-file
release-asset limit: "each file included in a release must be under 2 GiB",
<https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases>),
and verifies exact size and SHA-256 before any in-process Wasmtime compilation.
The bound is a ceiling, not a reservation: every reader checks a file's
metadata or a download's `Content-Length` against it before reading and grows
its buffer with the bytes actually read.
Wasmtime store fuel and memory limits govern instantiation and execution; they
are not claimed to bound compilation. Compilation is in-process Cranelift over
the authenticated bytes (parallel across functions); the session host compiles a
component only when a session first needs it (the compiler for a child
document, a data model when a document names it) and shares compiled components
process-wide by authenticated SHA-256 while any host holds them. No compiled
code is cached on or loaded from disk.

## The clock

The clock world exports one component-owned logical timeline. It accepts
caller-owned send IDs plus bounded canonical request bytes, orders equal
deadlines by insertion sequence, and bounds queue capacity, retained payload
bytes, ID bytes, drain batches, and arithmetic. The SCXML I/O processor imports this project contract
for delayed sends, cancellation, and bounded due draining; cancellation removes
timer and payload together, and due drains recover complete requests without
host-side scheduling decisions. A due send is routed again when it falls due
(`route-due`): a target that no longer answers (an invoked session that
finished or was cancelled) is `undeliverable`, and the sender receives the
platform `error.communication` with the send's id on its internal event queue,
processed at once by the macrostep the machine's `communication-error` runs
(SCXML 6.2: "It MUST dispatch the message only when the delay interval
elapses"; 6.2.4: "If it is unable to dispatch the message, the Processor MUST
place the error error.communication on the internal event queue"; the
Appendix D mapping is in [live-session.md](live-session.md)). A sender that terminated has no due
sends (6.2, official case 187). A `<send>` to `#_internal` with `delay` or
`delayexpr` is non-conformant (6.2.1) and raises `error.execution` (6.2.4;
validator `SCXML070` for a literal target). Neither component makes timer decisions through
direct WASI clock calls. The deployable clock artifact is deny-by-default
virtualized after the pinned `wasm32-wasip2` build so unused Rust adapter
capabilities are not host imports. A future realtime provider remains a separate
adapter.

## The ECMAScript data model

The ECMAScript data-model world's portable value contract keeps absent,
`undefined`, `null`, booleans, finite numbers, strings, canonical JSON arrays,
canonical JSON objects, and XML payloads remain distinct. It exports bounded
initialize, condition/value evaluation, assignment, script execution, and system
context operations. Evaluation takes the
current event as `option<event>`: `none` means SCXML defines no current event
(for example `<onentry>` run by `start`), and `_event` is then unbound (SCXML
1.0 section 5.10); the null and XPath providers accept the same `none`.
`_event`, `_sessionid`, `_name`, and `_ioprocessors` are read-only
accessors of the global object whose setters throw, so a write from any
path fails with `error.execution` (SCXML 5.10), and the objects they hold
(an XML payload's DOM included) are frozen and each sits behind a proxy whose
traps turn every attempted change (a member assignment, `delete`, a property
definition, a prototype change) into a `TypeError`, so a member write fails
with `error.execution` in non-strict code as in strict code. `delete` of a
system variable itself, in non-strict code, returns `false` without an error:
the global object is an ordinary object in Boa, whose embedding API offers no
way to make it exotic. `In()` is a native function of the realm that asks the
imported execution-host query (`in-state`) while the operation runs.
Evaluation reads no ambient time or randomness: Boa's wall clock is the
session clock (`execution-host.now-ms`), its monotonic clock is fixed,
`Math.random` is a deterministic per-session sequence, and the build
composes a declarative constant seed for the one
`wasi:random/insecure-seed` read that keys Boa's hash tables. The Wasmtime
host links a data-model component as a session's data model through
`ComponentDataModel` (fresh fuel per call; a trap fails the session). Each
instance creates one Boa realm at `initialize` and keeps it for the whole
session (the released component is pre-initialized with Wizer at build
time, so an instance's first session takes the realm its initial memory
already holds, built exactly as `initialize` would build it, and only adds
the system variables), as one ECMAScript program would run: values keep their identity,
closures their scopes, and objects their prototypes, and a script's changes
to the global object or to built-ins persist; a script that fails keeps
what it did before the failure, as ECMAScript defines
(`components/scxml-data-model-ecmascript/src/session.rs`). The adapter's own
work (event binding, value conversion, log rendering, snapshots) uses
built-ins captured before any user code ran, and reads values without
running user code. The realm has explicit loop, recursion, stack, source,
expression, value, and host-query limits. The deployable artifact is
deny-by-default virtualized so its only imports are the versioned SCXML
types, telemetry, and execution-host interfaces.

### `_name` without a `name` attribute

SCXML 1.0 §5.10: "The
SCXML processor MUST bind the variable _name at load time to the value of
the 'name' attribute of the <scxml> element", and the attribute table gives
`name` no default. Such a document supplies no value, so no name is
invented: `_name` is declared and read-only in every data model but holds
no value (ECMAScript `undefined`, an empty XPath `<_name/>`), and session
snapshots omit `name`.

The ECMAScript data model provides no browser DOM objects, ECMAScript
modules, async jobs or promises, or network or filesystem access. Scripts run
in global scope, and their declarations persist in the session's realm.

## The null and XPath data models

The null and XPath guests implement the same deployment boundary. The null
guest supports only its explicitly documented expression-free subset. The XPath
guest keeps all state in one bounded XML data tree whose `<datamodel>` element
is the context node of every expression, evaluates complete XPath 1.0
(`crates/scxml-xpath`, pure and fuel-bounded, with string work charged in
proportion to the bytes it materializes, scans, or produces) with `$name` variables and the
`In()` extension, and applies every `<assign type>` placement (SCXML 1.0 CR 2014-03-13 §C.3.6);
corpus XSLT preparation supplies no
XPath processor. All three deployable data-model artifacts are
deny-by-default virtualized and retain only the project types, telemetry, and
execution-host imports (the last answers `In()`).

## Validation and extension packages

### XML parsing and schemas

`scxml-syntax` centralizes `quick-xml` and exposes loss-conscious source types:
UTF-8 source text, byte spans and line/column mapping, expanded names, namespace
declarations, ordered attributes, text, and unknown/executable-content
extensions. `quick-xml` types do not cross crate boundaries.

Schema resolution is exact namespace URI lookup. Schemas are caller-supplied
bytes in Wasm; native adapters provide in-memory and explicit filesystem maps.
There is no network resolver. Explicit filesystem mappings reject symbolic
links and non-regular paths, reject metadata-declared oversize files before
allocation, and read through a `limit + 1` cap so growth or stale metadata also
fails closed. Policies are:

- `strict`: every used non-built-in namespace requires a schema;
- `lax`: validate registered namespaces and skip missing ones;
- `off`: XML and semantic validation only.

Schema policy controls XSD execution only; it never disables compiler behavior
package binding. Before emitting lowered IR or a package recipe, the canonical
compiler verifies every supplied namespace manifest (including unused ones)
against one exact caller-supplied schema artifact and requires every used
extension namespace to have exactly one verified manifest. The core SCXML and
built-in XML namespaces are exempt. Recipe extension artifact identifiers
correspond only to used extension namespaces, are distinct, and are sorted
canonically. The CLI binds packages with `--extension-package MANIFEST.json`
(`validate`, `compile`, and `snapshot run`; see [cli.md](cli.md)); `compile` rejects
a document that uses an extension namespace without one under `strict`, `lax`,
and `off` (`COMP002`, at the first element in the namespace, with its
declaration labeled); `--schema` alone is not a behavior binding.

`scxml-validator-xsd` isolates `xsd-schema` 0.2.x with default features disabled
(no `xsd11`, `compose`, or `async`). The backend uses XSD 1.0 `SchemaSet`, parses
all supplied schemas, processes them together, and drives the instance through
the pure-Rust streaming validator. It compiles for `wasm32-wasip2`; therefore
validation is not a native-only escape hatch. Before calling `xsd-schema`, the
adapter deterministically bounds schema nodes, nested compositor and occurrence
expansion, projected schema-compilation work, and projected document-validation
work. Inputs that exceed a budget, overflow its checked arithmetic, or use
model-group references whose resolved expansion cannot be soundly preflighted
are rejected before backend compilation.

### Extension packages

An extension package manifest binds exact URI, package version, extension
component artifact, schema artifact and SHA-256 digest, WIT world, and WIT
version, and nothing else: an extension element does no I/O, so it declares
no outbound origins and no `env` slots (only an invoked-service package
does). Every string field is bounded to 4096 UTF-8 bytes. The authoritative
supported extension component identity is exactly
`stateforward:scxml/executable-content-extension` at WIT version `0.2.0`;
missing, oversized, or mismatched identity metadata is rejected before schema
use. Validator/compiler consumers verify this metadata and schema integrity
without reading or instantiating the extension artifact. The CLI reads manifests
as `stateforward.scxml.extension-package/v2` JSON (every field required, unknown
fields rejected, at most 64 KiB) and resolves `schema-artifact` and
`extension-artifact` as plain relative paths inside the manifest's directory; a
package's verified schema is the schema of its namespace, so `--schema` may not
name the same URI.

Executing executable-content extensions is the Wasmtime session host's job
(`SessionHost::with_extension_components`,
`hosts/scxml-host-wasmtime/src/extension.rs`): it binds exact namespace URIs to
`executable-content-extension` components (release artifacts, or caller
components authenticated by SHA-256 through `authenticate_extension_component`,
compiled lazily through the process-wide cache), gives every session (root and
each invoked child) its own instance, checks the instance's `namespace-uri`
against the binding and calls `initialize` at session construction, routes
`machine-capabilities.execute-extension` by the element's exact namespace URI,
and calls `shutdown` once when the session ends, however it ends. For the
`execute` call it lends the extension component the session's data model, links,
and active configuration, so `data-model-host` and `execution-host` (`evaluate`,
`assign`, `raise`, `send`, `cancel`, `log`, `in-state`) act on the executing
session with the machine's own semantics and bounds; raised and self-addressed
events are returned to the machine, which queues them. An unbound namespace
answers `unavailable` and fails the session closed; `not-handled` and
`execution`/`invalid-input` failures raise `error.execution`. Each extension
component call is capped (`DEFAULT_EXTENSION_CALL_FUEL`) and charged to the
executing call's shared fuel budget, and a trap retires the instance for the
session. Invoked child documents compile against the host's bound packages.
A namespace a session's document uses that no binding names is loaded when the
session starts (`machine-capabilities.load-extensions`) through the embedder's
`ExtensionResolver`, or the start fails closed. The CLI binds every repository
namespace of the document and of each literal `<invoke src>` document it
loads before the session starts, so its runs never need the resolver. Extension components do no
I/O: they import no WASI interface and no `outbound-http` (the host links
neither into them; WASI is linked into invoked-service components only, and
`outbound-http` and `outbound-websocket` into invoked-service and Event I/O
Processor components; WASI as each component's own `wasi:*` imports with the
runtime resources the run grants, through `wasmtime-wasi` inside the host
adapter; see [invoked-services.md](invoked-services.md)), and they receive
no `env` value or reference: only an invoked service's `<invoke>` binds
slots in the host's `env` vocabulary
(`https://github.com/stateforward/scxml.rs/namespaces/env`).
[executable-content-extensions.md](executable-content-extensions.md)
maps agentml-go's Go `Namespace` interface onto the WIT and walks through writing
one.

These checks emit the closed OTEL events `scxml.input.accepted` or
`scxml.input.rejected`; the telemetry facade records each event in the
`scxml.events` counter using only the bounded event-name dimension. Namespace
URIs, artifact names, digests, WIT strings, and diagnostic text are never metric
labels or span attributes. Compatibility failure diagnostics name the rejected
field and the exact supported constant, but never echo the supplied value.

### Event data schemas

The validator checks the `event` vocabulary
([namespaces/event](../namespaces/event/README.md), `EVENT001`-`EVENT008`)
from the document and the bytes of the schema files it references, which the
caller supplies (`Validator::with_event_schema_resources`,
`CompileInput.event_schemas`, and the WIT `event-schemas` request field);
neither the validator nor the compiler performs I/O. The CLI reads each
referenced file relative to the document (`scxml.cli.read-event-schema`, a
closed span with no attributes). Nothing is fetched: references to other
resources are rejected, and those inside a file are bundled at compile time.
The W3C schema admits the attributes through its lax `##other` wildcards, and
the strict schema policy needs no registered schema for the namespace.

### The bundled W3C schema

The unmodified official W3C SCXML 1.0 XML Schema set
(<https://www.w3.org/2011/04/SCXML/scxml.xsd> and its modules) is bundled in
[`crates/scxml-validator-xsd/schemas/w3c-scxml-1.0`](../crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/NOTICE.md)
under the W3C schema license notice recorded there, with per-file SHA-256
provenance re-verified by tests. It is the default schema for the exact SCXML
namespace in the CLI and the validator component; `--schema
http://www.w3.org/2005/07/scxml=PATH` replaces it. The included
`tests/fixtures/scxml-subset.xsd` is a small demo schema for exercising that
override. XSD is structural validation; XSLT is
not used by the product pipeline and belongs only in external conformance corpus
transformation where required.

### Expression checking

The validator component checks expressions only through its parse-only
`expression-syntax` import. The Wasmtime host's `ComponentValidator`
(`hosts/scxml-host-wasmtime/src/validator.rs`) is the adapter that links it:
it compiles the authenticated validator once with one authenticated data-model
component (one data model per validator; each request runs in fresh stores)
and forwards every fragment unchanged to that component's
`data-model-api.check-syntax`. Only documents whose `datamodel` names the
linked data model are checked; with no data model or the null data model
linked it answers `none`, and other recognized data models get the `LINT010`
note. Data-model failures and traps become closed declines, reported as
`LIMIT003`. The host never parses SCXML or expressions itself.
`hosts/scxml-host-wasmtime/tests/validator_component.rs` runs every
single-data-model `tests/ui` case through the component and requires each
canonical report to equal the CLI's snapshot byte for byte. The virtualized
validator denies ambient randomness, so its build composes the fixed
`wasi:random` provider
[`components/scxml-validator/wasi-random.wat`](../components/scxml-validator/wasi-random.wat)
(beside the ECMAScript data model's constant-seed provider); its values only
key in-memory hash tables and never reach a diagnostic or report.

## Embedding the Wasmtime host

Embedders get live sessions, as `scx run` uses them, from the Wasmtime
host ([live-session.md](live-session.md)): `SessionHost::open` returns a
`LiveSession`. Its calls are `send`, `advance` (driven by
`next_deadline` and a pluggable `WallClock`, `ManualClock`, or logical
clock), `snapshot`, `take_outputs`, and `close`. `record` and `apply` log
and replay a session. `SessionOpen::http` (`LiveHttp`) gives it a
BasicHTTP network: a `PublicUrl` base for every location, an
`OutboundHttp` transport (the `http` feature's allowlisted `NetworkClient`,
or the embedder's own), and the `on_communication_error` and
`on_http_response` callbacks; `receive_http` delivers one inbound message
from any server (the `http` feature's `BasicHttpListener` is a minimal
built-in one), `deliver_communication_errors` delivers the
`error.communication` owed for sends that failed after acceptance, and
`deliver_http_responses` delivers the HTTP responses of invoked services'
`outbound-http.fetch` requests and the parts of their `outbound-http.send`
responses, one per call.
`SessionHost::run` (batch) drives the same engine through the same stepwise
scheduler calls with the logical clock unbounded, and always uses the
deterministic in-process loopback.
