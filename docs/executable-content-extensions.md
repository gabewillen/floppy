# Writing an extension component

An extension namespace (SCXML 1.0 §4.10) adds executable elements to SCXML,
for example `<example:fetch>` from the example namespace
([`components/scxml-extension-example`](../components/scxml-extension-example)). In this project a namespace
is a *package* of two independently deployed artifacts:

- an **XSD** that governs the elements' XML structure (the validator and
  compiler use it; it never executes anything), and
- a **extension component**: a Wasm component implementing the WIT world
  `stateforward:scxml/executable-content-extension@0.2.0`, which the host runs
  when the machine reaches one of the namespace's elements.

A **manifest** binds them: the exact namespace URI, the package version, the
extension artifact, the schema artifact and its SHA-256, the WIT world and
version, the outbound HTTP origins its extension component may reach
(`http-origins`), and the `env` slots its elements accept (`env-slots`).
It declares no WASI and no events ("WASI" below). A package is bound
explicitly (`--extension-package`), or, when its namespace
URI is a repository path, loaded from that repository's release ("Loading an
element from a repository" below). The example namespace
([`components/scxml-extension-example`](../components/scxml-extension-example))
exercises every host capability with one element each and is the model for
an extension component. Repository namespaces live under the top-level
[`namespaces/`](../namespaces) directory, one per folder; today it holds only
the toolchain's vocabularies: [`namespaces/env`](../namespaces/env), which is
not a package (it documents the host's `env` attribute vocabulary, "Values
are references" below), and [`namespaces/event`](../namespaces/event). A
network service, such as a model call, is not an extension element: an
element is executable content and runs synchronously inside a microstep, so
slow I/O belongs in an invoked service
([invoked-services.md](invoked-services.md); the echo example
[`components/scxml-service-example`](../components/scxml-service-example) is
its model).

## From agentml-go's Go `Namespace` to the WIT

agentml-go defines a namespace as a Go interface the interpreter calls
in-process. Here the same roles are a component's exports and imports:

| agentml-go (`agentml.Namespace`) | WIT (`stateforward:scxml@0.2.0`) |
|---|---|
| `NamespaceLoader(ctx, itp, doc)` builds the value | the host instantiates the component once per session (root and every invoked child) and calls `executable-content-extension-api.initialize` |
| `URI() string` | `executable-content-extension-api.namespace-uri` (the host checks it against the manifest's URI before the session runs) |
| `Handle(ctx, el) (bool, error)` | `executable-content-extension-api.execute(element, context) -> result<extension-outcome, component-error>` |
| `true, nil` / `false, nil` | `handled` / `not-handled` (the machine raises `error.execution` for `not-handled`; it never ignores an element) |
| returned `error` / `*PlatformError{EventName: "error.execution"}` | `component-error` with code `execution` or `invalid-input` (both become `error.execution`); any other code fails the session |
| `Unload(ctx)` | `executable-content-extension-api.shutdown` (called once when the session ends, however it ends) |
| `itp.DataModel().EvaluateValue(ctx, expr)` | `data-model-host.evaluate(expression)` |
| `itp.DataModel().SetVariable(ctx, loc, v)` | `data-model-host.assign(location, value)` |
| `itp.Raise(ctx, event)` | `execution-host.raise(event)` (queued on the session's internal queue) |
| `itp.Send(ctx, event)` | `execution-host.send(request)` (routed exactly like `<send>`) |
| `itp.Cancel(ctx, sendId)` | `execution-host.cancel(send-id)` |
| `itp.In(ctx, stateId)` | `execution-host.in-state(state-id)` |
| `itp.Log(ctx, label, message)` | `execution-host.log(level, message)` (hosts never put the message in telemetry) |
| OpenTelemetry spans (`otel.Tracer`) | the `telemetry` import (closed operation labels; the host owns the SDK) |
| `el xmldom.Element` | `executable-content-extension`: the exact expanded name, attributes, and flattened content |
| `itp.Snapshot(ctx, SnapshotConfig{ExcludeData})` | `session-snapshot-host.snapshot(exclude)`: the canonical `scxml.session-snapshot/v2` document of the executing session (read-only; see below) |
| `net/http` clients | `outbound-http.fetch` for a quick request (deny by default; admitted to the origins the package declares or the run supplies; see below); a slow request belongs in an invoked service |
| `os.LookupEnv` for keys and endpoints | an `env` slot: the element names the variable (`env:token="API_TOKEN"`), and the component receives an opaque reference, never the value (see below) |
| other ambient OS access | nothing ambient: an extension component reaches the outside world only through its world's imports; it gets no WASI interface (see "WASI") |

Differences that matter when porting:

- **State is per session.** agentml keeps one `Namespace` per interpreter;
  here every session, including every invoked child, gets its own instance,
  so state never crosses sessions. Keep what you need in the instance
  between `initialize` and `shutdown`.
- **Capabilities work only during `execute`.** Inside `initialize` and
  `shutdown` the session capabilities answer `unavailable` (and `in-state`
  answers false): no executable content is executing.
- **Ordering follows the document.** Events you raise or send to
  `#_internal` go on the internal queue at this element's place in the
  executable content, and
  self-addressed sends on the external queue, exactly as `<raise>` and
  `<send>` would; they are applied even when `execute` then fails, like the
  data-model assignments you already made.
- **Errors never echo input.** `component-error.detail` is at most 160 bytes
  of static text; the host replaces it with its own text before it reaches
  the session.
- **Every call is metered.** Each extension component call has a fuel cap (100
  million instructions by default) and is charged to the calling machine call's
  budget; a trap, including running out of fuel, is an `execution` failure that
  retires the instance for the rest of the session.

## Writing the component

Generate bindings for the `executable-content-extension` world and export the
API. A minimal skeleton:

```rust
wit_bindgen::generate!({
    world: "executable-content-extension",
    path: "../../crates/scxml-wit/wit/scxml",
});

use exports::stateforward::scxml::executable_content_extension_api::Guest;
use stateforward::scxml::{data_model_host, execution_host};
use stateforward::scxml::types::{
    ComponentError, ExecutableContentExtension, ExtensionOutcome, SlotReference, TraceContext,
};

struct Component;

impl Guest for Component {
    fn namespace_uri() -> String {
        "https://example.org/my-namespace".into()
    }
    fn initialize() -> Result<(), ComponentError> {
        Ok(())
    }
    fn execute(
        element: ExecutableContentExtension,
        _context: Option<TraceContext>,
        _references: Vec<SlotReference>,
    ) -> Result<ExtensionOutcome, ComponentError> {
        match element.name.local_name.as_str() {
            "hello" => {
                let greeting = data_model_host::evaluate("'hello'")?;
                data_model_host::assign("greeting", &greeting)?;
                Ok(ExtensionOutcome::Handled)
            }
            _ => Ok(ExtensionOutcome::NotHandled),
        }
    }
    fn shutdown() -> Result<(), ComponentError> {
        Ok(())
    }
}

export!(Component);
```

Build it for `wasm32-wasip2` and virtualize it deny-by-default with the pinned
`wasi-virt` (as `scripts/verify-wasm-capabilities.sh` does for the shipped
extension components), so the deployable component imports only the project
interfaces it uses. Keep the decision logic in plain Rust behind a small trait
so native tests exercise it without a host (the example component's
`ExampleNamespace::execute` is the model).

## WASI

Extension components get no WASI interface, by rule. An extension element
is executable content: it runs synchronously inside a microstep and must
stay quick. Anything asynchronous, I/O-bound, or long-running belongs in an
invoked service ([invoked-services.md](invoked-services.md)), which runs
beside the session and whose `<invoke>` is cancelled when its state exits.
The Wasmtime host links no WASI interface into an extension component, so
one that imports any fails to instantiate. An extension package manifest
has no WASI field: an invoked service's WASI interfaces are its
component's own imports, and the run grants their resources with
wasmtime's option names
([invoked-services.md](invoked-services.md#wasi-imports-and-resources)).
[`components/scxml-service-files`](../components/scxml-service-files) is
the example of a service component that imports `wasi:filesystem`.

An extension component's `outbound-http.fetch` is the same WIT `async
func`, but `execute` is synchronous: the host drives the call to
completion, so `fetch` blocks `execute`, and the session, for the
request's duration, and is meant for quick requests. A slow one (a model
call, for example) belongs in a service, whose `fetch` suspends only its
own task.

## Values are references: `env` slots

(An invoked service's `<invoke>` binds slots the same way, with references
that live for the whole invocation:
[invoked-services.md](invoked-services.md#values-are-references-env-slots).)

An extension component never receives a secret, and imports no WASI interface
in this release (one that imports any, `wasi:cli/environment`, `wasi:http`,
and `wasi:sockets` included, or anything outside its world, fails to
instantiate). Instead:

- **The package declares slots.** The manifest's `env-slots` lists the slots
  its elements accept (`[a-z][a-z0-9-]*`, at most 32; the example package
  declares `url` and `token`).
- **The document names a variable per slot.** An element binds a slot with an
  attribute in the host's `env` vocabulary, `https://github.com/stateforward/scxml.rs/namespaces/env`:
  `env:<slot>="NAME"`, or `env:<slot>expr="EXPR"` for an expression that
  evaluates to the name (SCXML's attribute/`expr` convention). Names are not
  secrets; values never appear in a document. The namespace has no elements,
  no package, and no component, and it is never fetched (it names no release;
  [`namespaces/env`](../namespaces/env/README.md) documents it, with a schema
  for tools).
  An extension schema admits it with
  `<xs:anyAttribute namespace="https://github.com/stateforward/scxml.rs/namespaces/env" processContents="lax"/>`
  (or `##other`); the validator checks every slot against the element's
  package (`SCXML083`: a misspelled slot, with the fix, a slot bound twice, or
  a name that is not a variable name) and rejects the namespace anywhere
  else (`SCXML084`; on an SCXML element it is `SCXML068`).
- **The run holds the values.** The embedder supplies them
  (`ExtensionComponents::with_values`), each variable granted to exact package
  URIs (`EnvVariable`); a slot naming a variable not granted to the element's
  namespace is absent, exactly as if no value were held. The CLI derives the
  grants from the document: a variable goes to the packages of the elements
  (and `<invoke>`s) whose `env` attributes name it, and to the package of
  every `env:<slot>expr` slot. Values come from `--env NAME=VALUE`, else the
  last `--env-file PATH` entry (Docker's format: `NAME=VALUE` with the value
  taken literally, a bare `NAME` from the shell, `#` comments, blank lines
  ignored), else, for a bare `--env NAME`, the calling shell's; a variable no
  slot can name is dropped. Nothing by default; there is no whole-environment
  form. Values are masked (`[masked]`) in the text the CLI writes; canonical
  snapshot documents are written exactly (their `sha256` covers their bytes).
- **The component holds references.** For each `execute` call the host binds
  every slot the element names, whose variable the run holds, to a fresh
  opaque `value-reference` and passes the list (`slot-reference`: slot and
  reference) with the call. A reference is valid only during that call and
  only for that element: another element, of the same or another namespace,
  never receives it, and an element that names no slot receives nothing. A
  name the run holds no value for leaves the slot absent (the component
  decides: `<example:fetch>` sends no `authorization` without a token, and
  uses its `url` attribute without a `url` slot), and the CLI names the
  `--env` flag that would add it before the run starts.

## Outbound HTTP: declared origins, run-supplied origins

An extension component that calls a network service imports
`stateforward:scxml/outbound-http` from its world and calls `fetch` with one
`GET` or `POST` whose URL, header values, and body are lists of parts: literal
text (or bytes) and references. The capability is deny by default:

- **No transport, no network.** Unless the embedder supplies a transport
  (`ExtensionComponents::with_outbound_http`), every request fails with
  `unavailable`. The CLI supplies one to `snapshot run` and `run`, except a
  `run` with `--record` or `--replay` (a replay log does not record extension
  component responses, so both refuse every request the same way).
- **Substitute, then admit.** The host substitutes every reference with its
  value, checks the request against the WIT bounds (method `GET` or `POST`;
  an absolute `http`/`https` URL without userinfo, at most 16384 bytes; at
  most 64 headers, none of `host`, `content-length`, `transfer-encoding`, or
  `connection`, no CR, LF, or NUL in a value; a body of at most 4 MiB, empty
  for `GET`; at most 64 parts per field), canonicalizes the URL with the
  WHATWG parser, and admits it only to:
  - an origin or URL prefix the element's package declares
    (`http-origins`; importing the element consents to them), compared in
    canonical form so dot segments cannot escape a prefix; or
  - an origin the run supplied: the URL's first part is a reference whose
    value is itself an absolute `http`/`https` URL without userinfo, and the
    rest of the URL, if any, begins its path, query, or fragment. The value
    then decides the whole scheme, host, and port (a local service's base
    URL, for example); a literal that precedes or extends the authority
    (`https://` + value, or value + `:8080/`) supplies no origin, and a value
    that is not such a URL creates none.

  Every other request is refused before the transport sees it, so a value is
  never substituted into a request that leaves for anywhere else; the CLI
  notes the refused origin after the run. Nothing about the request enters
  telemetry.
- **The transport sends.** The `http` feature's `ExtensionHttpClient` follows
  no redirect (a 3xx is returned as the response) and uses no cookie,
  credential, or environment proxy; TLS is rustls against the operating
  system's trust store.
- **Bounded on every build.** These are network ingress and egress bounds,
  so they apply to native builds too: a 10 s connect and 120 s total timeout
  per request (`ExtensionHttpConfig`), a response head of at most 16 KiB, and a
  response body of at most 16 MiB (`payload-too-large` beyond it).
- **The `execute` call blocks.** `fetch` runs on the session thread inside
  `execute`; fuel meters instructions, not wall time, so the transport's
  timeouts are what bound the wait. That is why `fetch` is for quick
  requests only: a slow one (a model call, for example) belongs in an
  invoked service, whose `fetch` suspends only its own task, never the
  session ([invoked-services.md](invoked-services.md#outbound-http)).
- **Failures are the extension component's to map.** A refused request, a
  transport failure, or an oversized response is a `component-error`;
  `<example:fetch>` maps every one, and every non-2xx status, to
  `execution`, so the session raises `error.execution` and keeps running.

`<example:fetch>` sends the `token` slot's reference as a bearer token
(`authorization: Bearer <reference>`) and builds its URL from the `url`
slot's reference followed by its `path` attribute, or from its `url`
attribute without that slot. Each request is one
`scxml.host-wasmtime.extension-http-fetch` span carrying only its closed outcome
event and error category.

```console
API_TOKEN=... scx snapshot run DOC.scx \
  --extension-package components/scxml-extension-example/package/example.extension.json \
  --extension-component-sha256 "https://stateforward.org/scxml/example=$EXAMPLE_SHA256" \
  --env API_TOKEN
```

The WIT import is project-specific rather than `wasi:http`, as the
`BasicHTTP` processor's `http-transport` is: extension components import no
WASI interface (and no host links `wasi:http`), the reference
substitution needs the host to see every part, and a
bounded request-and-response record lets the host reuse its audited
allowlist, canonicalization, and TLS policy without adding the
`wasi:io` stream and resource machinery or a second HTTP client.

## Reading the session snapshot

An extension component reads the executing session's state through
`stateforward:scxml/session-snapshot-host` (agentml-go's `Snapshot`): `snapshot(exclude)` returns the
executing session's canonical `scxml.session-snapshot/v2` document
([session-snapshot.md](session-snapshot.md), "Snapshots during executable
content") as compact JSON text, with any of the `system`, `data_model`, `queues`,
`pending-sends`, and `transitions` sections excluded.

- **Read-only, never restore.** Nothing restores, resumes, or changes a
  session from a snapshot. It shows the extension component nothing it could not
  already read through `data-model-host` and `in-state`, plus the queues,
  the pending delayed sends, and the transitions; SCXML documents never see
  it, so it adds nothing to SCXML.
- **Taken at the executing element.** The machine cannot be called while it
  executes the extension, so it hands its *executable-content image* to the host
  with the element: the configuration this element observes (the one `In()`
  answers), the internal queue, history, bound states, and counters, and the
  transitions walked from the innermost states of that configuration. The host adds
  the data model, pending sends, and clock read at the call (so they include
  what the extension component changed so far) and the events the extension
  queued so far. `invocations` and `previews` are always excluded, trace carriers
  are stripped, and the document says so in
  `options.point: "executable-content"`.
- **Free unless used.** The machine computes its executable-content image only
  for an extension component whose component imports `session-snapshot-host`
  (`machine-capabilities.extension-snapshot-request`), so extension
  components that do not read it cost nothing.
- **Bounded.** It works only during `execute` (`unavailable` in
  `initialize` and `shutdown`), at most 64 times per `execute` call
  (`capacity-exhausted` after; each call costs host work linear in the
  session's state, so the count is bounded on every build), and a document
  over a bound of the format (32 MiB, or a list bound) is
  `payload-too-large`. The machine's part runs on the executing machine
  call's fuel, the data-model and I/O-processor reads on their own per-call
  fuel, and the guest's copy in its own memory limit.
- **Telemetry at the boundary.** Each call is one
  `scxml.host-wasmtime.session-snapshot` span (the checkpoint snapshot's
  operation) with only its closed signal event and error category, plus
  the existing `snapshot-bytes` measurement; the document never becomes an
  attribute.

The example's `<example:snapshot location="L" exclude="S..."/>` assigns the
document, with the sections `S` excluded, to `L`.

## Packaging

Write the XSD for the namespace (it governs structure only, and admits the
`env` attributes laxly; see
[`components/scxml-extension-example/package/example.xsd`](../components/scxml-extension-example/package/example.xsd)),
then the manifest next to it:

```json
{
  "format": "stateforward.scxml.extension-package/v2",
  "namespace-uri": "https://example.org/scxml/weather@v1.0.0",
  "package-version": "1.0.0",
  "extension-artifact": "weather.wasm",
  "schema-artifact": "weather.xsd",
  "schema-sha256": "<lowercase SHA-256 of the exact weather.xsd bytes>",
  "wit-world": "stateforward:scxml/executable-content-extension",
  "wit-version": "0.2.0",
  "http-origins": ["https://api.example"],
  "env-slots": ["token"]
}
```

The CLI resolves `schema-artifact` and `extension-artifact` relative to the
manifest's directory (plain relative paths only). Every field is required,
unknown fields are rejected, and `wit-world` and `wit-version` must be exactly
the supported identity. The string fields are bounded to 4096 bytes.
`http-origins` (absolute `http`/`https` origins or URL prefixes without
userinfo, query, or fragment, at most 2048 bytes each, canonicalized by the
host) and `env-slots` (`[a-z][a-z0-9-]*`, at most 64 bytes, never ending in
`expr`) list at most 32 distinct entries each. `http-origins` is the
component's whole outbound allowlist: binding or loading the package consents
to it, and the CLI lists it before every run. Version 1 manifests are rejected.

## Using it

```console
# Structure: the package's schema validates its namespace, and its env-slots
# every env attribute.
scx validate machine.scx --extension-package weather/weather.extension.json

# Binding: compile requires a verified package for every used namespace.
scx compile machine.scx --output machine.scxml-ir.json \
  --extension-package weather/weather.extension.json

# Execution: the extension component, authenticated by its SHA-256, with the
# value its token slot names.
scx snapshot run machine.scx \
  --extension-package weather/weather.extension.json \
  --extension-component https://example.org/scxml/weather@v1.0.0="weather.wasm" \
  --extension-component-sha256 https://example.org/scxml/weather@v1.0.0="$WEATHER_SHA256" \
  --env-file .env
```

A document that uses a namespace no package binds is rejected by `compile`
(`COMP002`, at the first element in the namespace) under every schema policy,
and a session that reaches one at run time fails closed; executable-content
extensions never degrade into core SCXML.

Embedders of the Wasmtime host bind extension components with
`SessionHost::with_extension_components`:

```rust
let extensions = ExtensionComponents::new()
    // The values env slots may reference (components receive references).
    .with_values(ExtensionEnvironment::new([
        ("WEATHER_TOKEN".to_owned(), token),
    ])?)
    // The transport admitted requests go through (none: every request refused).
    .with_outbound_http(Arc::new(ExtensionHttpClient::new(ExtensionHttpConfig::new())?))
    .bind(weather_package, &authenticate_extension_component(&bytes, trusted_digest)?)?
    // Optional: load namespaces no binding names when a session starts.
    .with_resolver(Arc::new(my_resolver))
    // Optional: hear about refused requests and missing values.
    .with_observer(Arc::new(my_observer));
let host = SessionHost::new(&components, &config, &providers)?
    .with_extension_components(extensions)?;
```

`ExtensionPackage` carries the manifest fields and the schema bytes so the
host's compiler component can compile invoked child documents that use the
same namespaces. An `ExtensionResolver` answers `resolve(namespace_uri)` with
`Ok(None)` for a URI it does not serve, or a `ResolvedExtension` (the package
and a component it authenticated); the host checks it like a binding, binds it
for every later session, and fails the session start closed on any error. The
host performs no acquisition itself.

## Loading an element from a repository

(An invoked service's type URI can name its package the same way, under a
`services/` folder: [invoked-services.md](invoked-services.md#loading-a-service-from-a-repository).)

A namespace URI can name an element's package in a GitHub repository at a
release tag, the way agentml-go's namespaces were Go import paths, so a
document needs no flag to bind it:

```xml
<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:weather="https://github.com/example-org/scxml-weather/namespaces/weather@v1.0.0"
       version="1.0" datamodel="ecmascript" initial="ask">
```

- **The URI form.** Exactly `https://github.com/<owner>/<repo>/namespaces/<name>@<tag>`:
  the literal `namespaces` folder and one name segment (`[A-Za-z0-9._-]`;
  any other path, such as a bare or nested package path, is an ordinary
  namespace URI), no trailing slash, and the release tag after the last `@`. Binding
  stays byte-exact, so a new version is a new namespace. The package's
  manifest `namespace-uri` and its schema's `targetNamespace` must equal the
  URI byte for byte, tag included (`scripts/set-namespace-version.sh vX.Y.Z`
  moves this repository's namespaces to a new tag before it is tagged, and the
  release job checks it with `scripts/check-namespace-versions.sh`). To SCXML it
  is an ordinary namespace URI: nothing in a document names a version, ref,
  or digest.
- **The layout.** The manifest is `namespaces/<name>/package/<name>.extension.json`
  and its schema the manifest's relative `schema-artifact`; several
  namespaces live in one repository, one folder each under `namespaces/`. The extension component is the
  release asset the manifest's `extension-artifact` names, in the GitHub
  release of `<tag>`, which must also carry a `SHA256SUMS` asset
  (`sha256sum` format) listing it. This repository's tag-triggered release job
  publishes exactly that.
- **When it loads.** `validate` and `compile` (and the compile step of
  `snapshot run` and `run`) resolve the manifest and schema of every
  repository namespace the document uses that no `--extension-package` or
  `--schema` binds; they never need the component. When a session starts,
  root or invoked, the machine asks its host for every extension namespace its
  document uses (`machine-capabilities.load-extensions`, before data binding
  and the first macrostep), and the CLI's resolver supplies the component.
  Any failure fails closed: validation or compilation stops, or the session
  start fails and nothing runs. An invoked child whose document is inline in
  its parent's uses the namespaces the parent loaded.
- **The trust model.** The tag resolves to a commit through the GitHub API
  (`/repos/<owner>/<repo>/git/ref/tags/<tag>`, annotated tags dereferenced);
  the manifest and schema come from `raw.githubusercontent.com` at that
  commit, never at the tag, and must verify (exact URI, schema SHA-256,
  `targetNamespace`, WIT identity); the component must match the SHA-256 the
  release's `SHA256SUMS` lists for it (an absent, duplicate, or mismatched
  entry fails closed) and is then authenticated by that digest like any
  caller component. The first resolution pins the URI: the cache records the
  commit and the manifest, schema, and component digests, and every later use
  re-verifies against them, so a moved tag or a re-uploaded asset is never
  picked up silently.
- **The cache.** Exact verified bytes, content-addressed:
  `<cache>/sha256/<hex>`, plus `<cache>/index/<sha256 of the URI>.json`
  (`stateforward.scxml.extension-cache/v1`: the URI, commit, and digests). The
  cache is `${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions` (the same on
  macOS) unless `--cache-dir DIR` names another. A cached object that does not
  match its digest fails closed ("it was modified"); delete it and the next
  online run fetches and verifies it again.
- **Offline.** A pinned URI whose bytes are cached uses no network at all.
  `--offline` forbids every fetch: a URI that is not in the cache fails
  closed, before validation, or at session start for a component that was
  never fetched.
- **Network bounds.** HTTPS only; a redirect is followed only to an origin of
  the configured bases or GitHub's asset hosts (`objects.githubusercontent.com`,
  `release-assets.githubusercontent.com`), at most 5 times; a 10 s connect and
  60 s total timeout per request and no retry; bodies bounded (64 KiB for API
  answers, the manifest, and `SHA256SUMS`, 4 MiB for the schema, and for the
  component 2 GiB − 1 bytes, GitHub's per-file release-asset limit and the
  Wasmtime host's authentication bound; a declared `Content-Length` over a
  bound fails before the body is read, and a body of exactly its bound is
  admitted); no cookies, no proxy from the environment, and no credentials,
  except a GitHub token read from the variable `--github-token-env NAME`
  names, sent only to the API origin. `--github-api-base`,
  `--github-raw-base`, and `--github-download-base` point at a mirror (or the
  CLI's own tests); plain `http` is accepted only for a loopback host.
- **Importing is consent.** Loading an element consents to the origins its
  manifest declares, exactly as binding its package does; the run's values
  still reach it only as references, substituted into requests to those
  origins or to an origin a value supplies. Every command that reads a
  document (`validate` in its human format) lists each element's namespace, release tag and pinned commit,
  declared origins, and the slots the document binds, before anything runs.
- **Telemetry.** `scxml.cli.resolve-extension-package`,
  `scxml.cli.resolve-extension-component`, and the host's
  `scxml.host-wasmtime.extension-load` spans carry only their closed outcome;
  no URI, URL, tag, commit, digest, or path is ever an attribute.

```console
# First run: resolves, verifies, caches, and pins the element.
scx snapshot run weather.scx

# Later runs: the same command with --offline uses the cache only.
```

## Current gaps

- The browser host still hands executable-content extensions to the page's
  `executeExtension` callback (answering `handled` or `not-handled`, with no
  session capabilities); it does not instantiate extension components or load
  repository namespaces, and without that callback a document that uses an
  extension namespace fails its session start.
- A child document an `<invoke src>` loads at run time is compiled with the
  namespaces already bound; a repository namespace only such a child uses is
  not resolved for it (its compilation fails closed, raising
  `error.execution` in the invoking session). Inline `<content>` children use
  the namespaces their parent loads.
- Only GitHub repositories are repository namespaces.
- Extension component instance state is not part of session snapshots (agentml-go's
  optional `Snapshotter` has no WIT counterpart yet).
- The browser host offers no `session-snapshot-host`: it runs no extension
  component components, its machine capabilities answer
  `extension-snapshot-request` with false, and a page's `executeExtension`
  callback receives no snapshot.
- An executable-content snapshot holds the executing session only (no invoked
  sessions) and no transition previews.
- `outbound-http` is not streamed: a component receives the whole response
  body once.
- WASI is linked into invoked-service components only, by rule
  (extension elements are synchronous executable content; I/O-bound work
  belongs in services): an extension component that imports any WASI
  interface fails to instantiate. The browser host links no WASI interface
  at all.
