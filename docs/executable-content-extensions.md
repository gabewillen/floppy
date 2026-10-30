# Writing an extension component

An extension namespace (SCXML 1.0 §4.10) adds executable elements to SCXML,
for example `<example:raise>` from the example namespace
([`components/scxml-extension-example`](../components/scxml-extension-example)). In this project a namespace
is a *package* of two independently deployed artifacts:

- an **XSD** that governs the elements' XML structure (the validator and
  compiler use it; it never executes anything), and
- a **extension component**: a Wasm component implementing the WIT world
  `stateforward:scxml/executable-content-extension@0.2.0`, which the host runs
  when the machine reaches one of the namespace's elements.

A **manifest** binds them: the exact namespace URI, the package version, the
extension artifact, the schema artifact and its SHA-256, and the WIT world
and version. It declares nothing else: no outbound origins, no `env` slots,
no WASI, and no events, because an extension element does no I/O ("No I/O"
below). A package is bound
explicitly (`--extension-package`), or, when its namespace
URI is a repository path, loaded from that repository's release ("Loading an
element from a repository" below). The example namespace
([`components/scxml-extension-example`](../components/scxml-extension-example))
exercises every host capability with one element each and is the model for
an extension component. Repository namespaces live under the top-level
[`namespaces/`](../namespaces) directory, one per folder; today it holds only
the toolchain's vocabularies: [`namespaces/env`](../namespaces/env), which is
not a package (it documents the host's `env` attribute vocabulary, which
binds invoked services' slots), and [`namespaces/event`](../namespaces/event). A
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
| `itp.Snapshot(ctx, SnapshotConfig{ExcludeData})` | `session-snapshot-host.snapshot(exclude, target)`: the canonical `scxml.session-snapshot/v2` document of the executing session, or of another session of its run that `target` names (read-only; see below) |
| `net/http` clients, `os.LookupEnv` for keys and endpoints | an invoked service ([invoked-services.md](invoked-services.md)): its `outbound-http.fetch` suspends only its own task, and its `<invoke>` binds `env` slots to opaque references; an extension element does no I/O (see "No I/O") |
| other ambient OS access | nothing ambient: an extension component reaches the outside world only through its world's imports, which carry no I/O (see "No I/O") |

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

## No I/O

An extension element is executable content: it runs synchronously inside a
microstep and must stay quick, so its component does no I/O, by rule. The
`executable-content-extension` world imports no `outbound-http` and no WASI
interface, the Wasmtime host links neither into an extension component (one
that imports either, `wasi:cli/environment`, `wasi:http`, and
`wasi:sockets` included, fails to instantiate), and an element has no `env`
slots: the validator rejects an `env` attribute on it (`ENV002`), and the
component receives no value and no reference. Anything asynchronous,
I/O-bound, or long-running (a model call, a request to a network service,
a file) belongs in an invoked service
([invoked-services.md](invoked-services.md)), which runs beside the session,
whose `<invoke>` binds `env` slots to opaque references and is cancelled
when its state exits, and whose `outbound-http.fetch` suspends only its own
task. [`components/scxml-service-example`](../components/scxml-service-example)
is the model of a service that calls the network, and
[`components/scxml-service-files`](../components/scxml-service-files) of one
that imports `wasi:filesystem`.

## Reading the session snapshot

An extension component reads the executing session's state through
`stateforward:scxml/session-snapshot-host` (agentml-go's `Snapshot`): `snapshot(exclude, target)` returns the
executing session's canonical `scxml.session-snapshot/v2` document
([session-snapshot.md](session-snapshot.md), "Snapshots during executable
content") as compact JSON text, with any of the `system`, `data_model`, `queues`,
`pending-sends`, and `transitions` sections excluded.

With a `target`, it reads another session of the same run instead, named as
a `<send>` target names it (`#_parent`, `#_<invokeid>` of a live invoked
session, or `#_scxml_<sessionid>` of any running session of the run): that
session's canonical document at its last checkpoint
([session-snapshot.md](session-snapshot.md), "Snapshots of another
session"). A target that is malformed, names no running session of the run
(an invoked service, a finished, `failed`, or `shut-down` session, another
run's) is `unavailable`. Nothing of another run, of the host, or of a
finished session is readable, and the document still never sees a snapshot
itself, only what an extension component writes from it.

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
  over a bound of the format (a string or integer bound) is
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

Write the XSD for the namespace (it governs structure only; see
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
  "wit-version": "0.2.0"
}
```

The CLI resolves `schema-artifact` and `extension-artifact` relative to the
manifest's directory (plain relative paths only). Every field is required,
unknown fields are rejected, and `wit-world` and `wit-version` must be exactly
the supported identity. The string fields are bounded to 4096 bytes.
Optionally, a package ships a package validator component that checks what
its XSD cannot: `validator-artifact` (relative to the manifest; a release
asset for a repository namespace) and `validator-sha256` (its lowercase
SHA-256), both or neither. `validate`, `compile`, `snapshot run`, and `run`
call it once per document with the namespace's elements and report its
diagnostics as `<prefix>::<code>`
([validator-rules.md](validator-rules.md#package-validators)); a repository
namespace's validator is verified against the release's `SHA256SUMS` and the
manifest's digest, and cached like its component.
`http-origins` and `env-slots` are unknown fields here (only a service
package declares them). Version 1 manifests are rejected.

## Using it

```console
# Structure: the package's schema validates its namespace.
scx validate machine.scx --extension-package weather/weather.extension.json

# Binding: compile requires a verified package for every used namespace.
scx compile machine.scx --output machine.scxml-ir.json \
  --extension-package weather/weather.extension.json

# Execution: the extension component, authenticated by its SHA-256.
scx snapshot run machine.scx \
  --extension-package weather/weather.extension.json \
  --extension-component https://example.org/scxml/weather@v1.0.0="weather.wasm" \
  --extension-component-sha256 https://example.org/scxml/weather@v1.0.0="$WEATHER_SHA256"
```

A document that uses a namespace no package binds is rejected by `compile`
(`COMP002`, at the first element in the namespace) under every schema policy,
and a session that reaches one at run time fails closed; executable-content
extensions never degrade into core SCXML.

Embedders of the Wasmtime host bind extension components with
`SessionHost::with_extension_components`:

```rust
let extensions = ExtensionComponents::new()
    .bind(weather_package, &authenticate_extension_component(&bytes, trusted_digest)?)?
    // Optional: load namespaces no binding names when a session starts.
    .with_resolver(Arc::new(my_resolver));
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

## The embedded `scx` namespace

`scx` ships one extension namespace of its own,
[`scx`](../namespaces/scx/README.md)
(`https://github.com/stateforward/scxml.rs/namespaces/scx`): `<scx:validate>`
runs the validator on a document held as text. The binary embeds its
manifest, schema, and component (authenticated by the SHA-256 its release
manifest lists, checked when `scx` is built), so `validate`, `compile`,
`snapshot run`, and `run` bind it whenever a document uses the namespace and
no `--extension-package` binds the URI; nothing is fetched. Its URI carries
no tag: it is versioned with the toolchain, like the `env` and `event`
vocabularies. The consent listing names it as `package <version> built into
scx`.

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

- **The URI form.** Exactly `https://github.com/<owner>/<repo>/namespaces/<name>`:
  the literal `namespaces` folder and one name segment (`[A-Za-z0-9._-]`;
  any other path, such as a bare or nested package path, is an ordinary
  namespace URI), no trailing slash. That tagless URI is the package's
  identity and names the repository's latest GitHub release, resolved once
  per root session. A document may pin a release by writing one `@<tag>`
  after it (`[A-Za-z0-9._+-]`, starting with a letter or digit, without
  `..`).
- **The tag is not part of the namespace.** The `@<tag>` is split off once,
  when the document is parsed (`scxml_types::strip_tag`, for namespace
  declarations and literal `<invoke type>`s only), and goes to the CLI's
  resolver alone: the validator, the compiler, the Machine IR, the engine,
  the data models, and the host see only the tagless URI, exactly, so a
  pinned and a tagless use of one release validate against the same schema
  and bind the same element. The package's manifest `namespace-uri` and its
  schema's `targetNamespace` name the tagless URI byte for byte (a manifest
  naming a pinned URI is refused; the release job checks this repository's
  packages with `scripts/check-namespace-versions.sh`). A malformed pin (an
  `@` not followed by exactly one tag, such as `...@` or `...@v1/x`), one
  document pinning a package to two releases, and a pin of the `env`,
  `event`, or `scx` vocabularies (`scx` provides them: tagless, matched
  first, never fetched) fail closed. Any other URI keeps its `@`
  (`https://example.org/a@b/ns`, a `urn:` with `@`) byte for byte.
- **The layout.** The manifest is `namespaces/<name>/package/<name>.extension.json`
  and its schema the manifest's relative `schema-artifact`; several
  namespaces live in one repository, one folder each under `namespaces/`. The extension component is the
  release asset the manifest's `extension-artifact` names, in the GitHub
  release of `<tag>` (or the latest release), which must also carry a `SHA256SUMS` asset
  (`sha256sum` format) listing it. This repository's tag-triggered release job
  publishes exactly that.
- **When it loads.** `validate` and `compile` (and the compile step of
  `snapshot run` and `run`) resolve the manifest and schema of every
  repository namespace the document uses that no `--extension-package` or
  `--schema` binds; they never need the component. `snapshot run` and `run`
  also resolve the namespaces of every literal `<invoke src>` document the
  run loads, and bind every component, authenticated, before the session
  starts. Each resolution (the tagless URI, or it with a pin) happens once
  per root session. The run binds each package to the root document's
  resolution (its pin, else the latest release), or, for a package only
  `<invoke src>` documents use, the first such document's; every tagless use
  in the tree shares it, and the root and every invoked session (inline
  `<content>` or `<invoke src>`) share its compiled component, one instance
  per session. An `<invoke src>` document that pins another release gets
  that release for its own sessions and their inline `<content>` sessions,
  alongside the run's (the CLI hands it to the host with the document,
  `EmbeddedDocument::extensions`), and the consent listing names that
  document. Any failure fails closed: validation or compilation stops, or the
  run fails before the first macrostep.
- **Who fetches.** The GitHub resolver component `scx` embeds
  (`components/scxml-resolver-github`, the `stateforward:scxml/resolver`
  world, authenticated by its SHA-256 in the embedded release) holds the URI
  pattern's GitHub meaning: it resolves a URI to a release and fetches that
  release's files through WASI 0.3 `wasi:http/client`, which the CLI's
  Wasmtime host admits only to the origins below. It decides no trust: the
  CLI verifies everything it returns and owns the cache, the pins,
  `--offline`, and resolving each URI once per root session.
- **The trust model.** A tagless URI's tag is its repository's latest
  release (`/repos/<owner>/<repo>/releases/latest`, its `tag_name`), and the
  consent listing shows the tag and commit it resolved to. The tag resolves
  to a commit through the GitHub API
  (`/repos/<owner>/<repo>/git/ref/tags/<tag>`, annotated tags dereferenced);
  the manifest and schema come from `raw.githubusercontent.com` at that
  commit, never at the tag, and must verify (exact URI, schema SHA-256,
  `targetNamespace`, WIT identity); the component must match the SHA-256 the
  release's `SHA256SUMS` lists for it (an absent, duplicate, or mismatched
  entry fails closed) and is then authenticated by that digest like any
  caller component. The first resolution pins the URI: the cache records the
  commit (and a tagless URI's release tag) and the manifest, schema, and
  component digests, and every later use re-verifies against them, so a
  moved tag or a re-uploaded asset is never picked up silently; a tagless
  URI is re-pinned only when its latest release names another commit.
- **The cache.** Exact verified bytes, content-addressed:
  `<cache>/sha256/<hex>`, plus `<cache>/index/<sha256 of the URI>.json`
  (`stateforward.scxml.extension-cache/v1`: the URI, a tagless URI's `tag`, the commit, and digests). The
  cache is `${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions` (the same on
  macOS) unless `--cache-dir DIR` names another. A cached object that does not
  match its digest fails closed ("it was modified"); delete it and the next
  online run fetches and verifies it again.
- **Offline.** A pinned URI whose bytes are cached uses no network at all.
  `--offline` forbids every fetch: a URI that is not in the cache fails
  closed, before validation, or before the session for a component that was
  never fetched; a tagless URI uses the release the cache pinned.
- **Network bounds.** HTTPS only; the host admits a request (each redirect
  the component follows included) only to an origin of the configured bases
  or GitHub's asset hosts (`objects.githubusercontent.com`,
  `release-assets.githubusercontent.com`), and the component follows at most
  5 redirects; a 10 s connect and
  60 s total timeout per request and no retry; bodies bounded (64 KiB for API
  answers, the manifest, and `SHA256SUMS`, 4 MiB for the schema, and for the
  component 2 GiB − 1 bytes, GitHub's per-file release-asset limit and the
  Wasmtime host's authentication bound; a declared `Content-Length` over a
  bound fails before the body is read, and a body of exactly its bound is
  admitted); no cookies, no proxy from the environment, and no credentials,
  except a GitHub token read from the variable `--github-token-env NAME`
  names, which the host (never the component) adds to requests for the API
  origin only. `--github-api-base`, `--github-raw-base`, and
  `--github-download-base` point at a mirror (or the CLI's own tests) and
  reach the component as its configuration; plain `http` is accepted only
  for a loopback host.
- **Importing is consent.** Loading an element consents to the origins its
  manifest declares, exactly as binding its package does; the run's values
  still reach it only as references, substituted into requests to those
  origins or to an origin a value supplies. Every command that reads a
  document (`validate` in its human format) lists each element's namespace, release tag and pinned commit,
  declared origins, and the slots the document binds, before anything runs.
- **Telemetry.** `scxml.cli.resolve-extension-package`,
  `scxml.cli.resolve-extension-component` (which cover the resolver
  component's calls; it records nothing itself), and the host's
  `scxml.host-wasmtime.extension-load` spans carry only their closed outcome;
  no URI, URL, tag, commit, digest, or path is ever an attribute.

```console
# First run: resolves, verifies, caches, and pins the element.
scx snapshot run weather.scx

# Later runs: the same command with --offline uses the cache only.
```

## Current gaps

- The browser host runs extension components with the Wasmtime host's
  semantics (`ExtensionComponents`, the same names), but only the ones it
  ships transpiled and fuel-metered at build time (the example namespace and
  `scx`); it loads no repository namespace, since a component resolved at run
  time would need a browser build of its own.
- A child document an `<invoke srcexpr>` loads at run time, or a
  `<content expr>` child, is compiled with the namespaces already bound; a
  repository namespace only such a child uses is not resolved for it (its
  compilation fails closed, raising `error.execution` in the invoking
  session). Literal `<invoke src>` documents and inline `<content>` children
  have their namespaces resolved with the root's before the session starts.
- Only GitHub repositories are repository namespaces.
- Extension component instance state is not part of session snapshots (agentml-go's
  optional `Snapshotter` has no WIT counterpart yet).
- An executable-content snapshot holds the executing session only (no invoked
  sessions) and no transition previews.
- By rule, not as a gap, an extension component does no I/O (extension
  elements are synchronous executable content; I/O-bound work belongs in
  services): one that imports any WASI interface or `outbound-http` fails
  to instantiate.
