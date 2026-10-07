# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project is pre-1.0. From the `0.1.0` release on, breaking changes bump the
affected WIT package's minor version or the affected Machine IR, report,
snapshot, or wire-format version (and the workspace and npm package minor
version); additive WIT changes and compatible fixes bump the patch version;
replacements are hard cutovers with no deprecation period. The complete policy,
owned by the repository maintainers, is the "Versioning policy" section of
[crates/scxml-wit/src/lib.rs](crates/scxml-wit/src/lib.rs).

## [Unreleased]

Changes since `0.1.0`, to be released as `0.2.0`. Every change below is a
hard cutover.

### Summary

- Versions: the workspace, every crate, and the browser package are
  `0.2.0`, and every contract whose shape changed since `0.1.0` has a new
  version: the six WIT packages are `@0.2.0`, Machine IR is `5`, the report
  envelope is `scxml.report/v3` (conformance payload `9`), session
  snapshots are `scxml.session-snapshot/v2` and their diffs
  `scxml.session-snapshot-diff/v2`, the delayed-send wire record is `8`,
  and the prepared corpus manifest is `6`. The live-session log
  (`scxml.live-log/v1`) and the extension package manifest
  (`stateforward.scxml.extension-package/v2`) and the extension and service
  cache index entries (`stateforward.scxml.extension-cache/v1`,
  `stateforward.scxml.service-cache/v1`) are new. The README's
  versioning table lists each contract at `0.1.0` and `0.2.0`.
- Invoked sessions run at any depth on one shared logical clock, and
  live sessions (`scx run`, `SessionHost::open`) take events as they
  arrive, with record and replay.
- BasicHTTP runs over the network in live sessions, and delayed BasicHTTP
  sends are held on the session clock.
- Extension components execute extension-namespace elements end to end
  under the Wasmtime host, with host-admitted outbound HTTP and read-only
  session snapshots. A document loads an element straight from a repository
  by naming it
  (`https://github.com/<owner>/<repo>/namespaces/<name>`, the latest
  release, or `...@<tag>` to pin one): verified against the release's
  `SHA256SUMS`, cached, pinned, resolved once per run, and bound before the
  session starts. The tag is never part of the namespace: it is split off
  when the document is parsed (`scxml_types::strip_tag`), so release
  manifests and schemas name the tagless URI and everything after the
  parser sees it exactly. Elements receive the run's values only
  as references (`env:<slot>="NAME"`), and importing an element consents to
  the origins its manifest declares.
- Both official W3C IRP corpora still pass every applicable case through
  the shipped components (ECMAScript 201 / 201, XPath 178 / 178); cases the
  W3C evidence shows never exercise a corpus's data model are now
  `NotApplicable` to it rather than counted.
- Every public surface uses SCXML 1.0 vocabulary; the renames are listed
  under [Renames](#renames). Resource bounds apply to wasm builds only.
- Many fixes to white-space handling, error events, hostile-input bounds,
  and browser-host scheduling; see [Fixed](#fixed).

### Added

- `snapshot run` and `run` load `<invoke src>`, `<script src>`, and
  `<data src>` by SCXML's defaults (`https:`, `file:`, and relative URIs
  against the naming document's base URI): every literal URI of the document
  tree once before the first macrostep, listed before running, through the
  repository resolver's bounded, credential-free HTTPS fetcher
  (`scxml.cli.read-document`); a `<script src>` that cannot be fetched
  rejects the document (SCXML 5.8), and an `<invoke srcexpr>` URI is fetched
  when the invoke runs (`error.execution` on failure). This replaces
  `--resource URI=PATH` and the at-or-below-the-document-directory rule for
  `<invoke src>`, both removed.
- The validator's draft profile (`scxml_validator_core::Profile::Draft`,
  `Validator::with_profile`; `<scx:validate profile="draft">`): for a
  document a machine writes and runs as its own invoked child, `SCXML097`
  (a namespace other than SCXML's, `event`, and `xml`), `SCXML098`
  (`<invoke>`, `<script>`), `SCXML099` (`src`, `srcexpr`), `SCXML100`
  (`typeexpr`, a `<send type>` other than the SCXML Event I/O Processor),
  and `SCXML101` (a DTD), documented in `docs/validator-rules.md` and
  `scx --explain`.
- The `scx` extension namespace (`namespaces/scx`,
  `https://github.com/stateforward/scxml.rs/namespaces/scx`, tagless and
  versioned with the toolchain): `<scx:validate>` runs the validator on a
  document held as text and assigns the canonical report, optionally with
  the machine-applicable fixes applied. It moved from nemobot's `scxml`
  namespace (`<sx:validate>`). `scx` embeds its package and its component
  (`ArtifactKind::Scx`, listed in the release manifest), so a document uses
  it with no flag or fetch; `scripts/verify-wasm-capabilities.sh` builds and
  checks it, and the namespace version scripts skip it.
- `scx` embeds the authenticated release session components: its build
  (`crates/scxml-cli/build.rs`, the default `wasmtime` feature) reads the
  `scripts/verify-wasm-capabilities.sh` release (`target/wasm-artifacts/release`
  or `SCXML_WASM_ARTIFACTS_DIR`), checks every component's SHA-256 and size
  against `manifest.tsv`, and fails with a message naming what is missing;
  `run`, `snapshot run`, and `conformance run` use them by default, and
  `--components DIR --components-manifest-sha256 HEX` overrides them
  (`WasmExecutor::from_release_bytes`,
  `WasmExecutor::from_release_bytes_collecting_snapshots`).
- `DEFAULT_EXTENSION_CALL_FUEL` is one billion instructions (it was 100
  million): one call of an element that links the whole validator compiles
  the bundled W3C SCXML 1.0 XSD set (measured 166 to 214 million
  instructions per `<scx:validate>` call, about 85% of it the
  schema compilation, which `xsd-schema` cannot cache across documents).
- Invoked services (SCXML 6.4): an `<invoke>` whose type is neither absent
  nor the SCXML type runs in a `stateforward:scxml/invoked-service` Wasm
  component the Wasmtime session host binds to that exact type URI
  (`SessionHost::with_service_components`, `ServiceComponents`,
  `ServicePackage`, `authenticate_service_component`), one instance per
  invocation in a store of its own, on a thread of its own, driven by
  wasmtime 49's Component Model async event loop (`run_concurrent`,
  `call_concurrent`). The world imports
  `wasi:clocks/monotonic-clock@0.3.0` (served from the session's logical
  clock: logical milliseconds in nanoseconds, 1 ms resolution, so a
  `wait-for` or `wait-until` falls due in order with the session's delayed
  sends), `outbound-http`, and `telemetry`, and exports
  `invoked-service-api` with one WASI 0.3 async function, `invoke: async
  func(request: invoke-request, events: stream<event>) ->
  result<tuple<stream<event>, future<result<option<data-value>,
  component-error>>>, component-error>`. `events` carries the session's
  `<send target="#_<invokeid>">` and autoforwarded events; the events the
  service writes to its returned stream reach the invoking session stamped
  with the invoke id (through `<finalize>`); the future resolving `ok` is
  `done.invoke.<id>` with its data, and `err` or a trap is
  `error.communication` (`invocation-message.communication-error`);
  rejecting `invoke` fails the `<invoke>` with `error.execution`. Leaving
  the invoking state cancels the invocation (SCXML 6.4): the host drops its
  store and closes its HTTP connections at once. A type no service binds,
  literal or `typeexpr`, is judged only when its `<invoke>` runs, raising
  `error.execution` (invocation is a runtime action). The service package
  manifest `stateforward.scxml.service-package/v1` (`http-origins` and
  `env-slots`; no WASI and no events) and the CLI's `--service-package`,
  `--service-component`, and `--service-component-sha256`; validator
  warnings `SCXML093` (a literal type that is a platform-specific value,
  not a URI; SCXML 6.4.1 allows it) and `SCXML094` (no package binds it;
  the `<invoke>` raises `error.execution` when it runs). The example echo
  and timer service ships as `components/scxml-service-example`. The
  browser host binds no service, so every service `<invoke>` raises
  `error.execution` when it runs; snapshots of a run holding a live service
  are refused. See [docs/invoked-services.md](docs/invoked-services.md).
- Asynchronous outbound HTTP for invoked services:
  `outbound-http.fetch` is an `async func`, so a service's call suspends
  only the calling task, never the session. A batch run performs it through
  the embedder's transport (`ServiceComponents::with_outbound_http`); a live
  session performs it on a transport thread per request (at most 16 in
  flight per invocation) and delivers the response as an external input
  (`LiveSession::deliver_http_responses`, signaled through
  `LiveHttp::on_http_response`; `LiveSession::awaiting_http_responses`
  keeps a logical clock from moving past a request in flight). The
  responses are recorded inputs (`InputRecord::HttpResponse`, the
  `http-response` record of `scxml.live-log/v1`), so `--record` and
  `--replay` work with services; `scx run` delivers each in a call
  with cause `http-response`. An extension component's `fetch` is the same
  WIT function, driven to completion inside its synchronous `execute`.
  Telemetry operations `service-wait-until`, `service-http-fetch`,
  `service-http-request`, and `session-http-response`.
- WASI for invoked services, without a manifest: the WASI interfaces a
  service may use are its component's own `wasi:*` imports. Only runtime
  resources need a grant, named after wasmtime CLI's options and scoped to
  the service's exact type URI (`ServiceComponents::with_wasi(uri,
  WasiOptions { dirs, inherit_network, allow_ip_name_lookup })`; the CLI's
  `--dir URI=HOST::GUEST`, `--wasi URI=inherit-network`, and
  `--wasi URI=allow-ip-name-lookup`). An import without its grant, a grant
  without its import, and a `wasi:*` import the host never links
  (`wasi:cli`, `wasi:http`) fail closed when the service is bound, before
  anything runs (`ArtifactError::WasiResourceUngranted`,
  `WasiGrantUndeclared`, `UnknownImport`, `WasiGrantInvalid`). The
  CLI lists each service's `= WASI imports:` and `= WASI resources:` before
  running. Extension components get no WASI interface, by rule: extension
  elements are synchronous executable content, and I/O-bound work belongs
  in invoked services. The example `components/scxml-service-files`
  imports `wasi:filesystem` and runs only with its directory granted.
- `env` slots for invoked services: an invoked service's `<invoke>` binds
  the slots its service package declares with `env:<slot>="NAME"` or
  `env:<slot>expr` (evaluated when the `<invoke>` runs). Machine IR 5
  (unreleased) carries them on the invoke (`attributes`, sorted by local
  name, omitted when empty, so existing IR bytes are unchanged); WIT
  (`@0.2.0`, unreleased) passes them in `invoke-request.attributes`
  (`list<extension-attribute>`) and the host's references in
  `invoke-request.references` (`list<slot-reference>`). The Wasmtime host
  binds each to an opaque reference for the whole invocation
  (`ServiceComponents::with_values`), substituted only into that
  invocation's admitted `outbound-http` requests; an undeclared or repeated
  slot rejects the start (`error.execution`). The validator checks them
  (`SCXML083` against the bound package, `SCXML084` on an SCXML session's
  `<invoke>`); the CLI passes `--env`/`--env-file` values and the outbound
  transport to services. The echo example declares an `endpoint` slot and
  GETs it on `fetch`.
- Env values scoped per package: `ExtensionEnvironment::new` takes
  `EnvVariable`s, each granted to exact package URIs (extension namespace or
  invoked-service type URIs); a slot naming a variable not granted to its
  element's namespace or its `<invoke>`'s type URI is absent, exactly like
  an unheld one. The CLI derives the grants from the document: a variable
  goes only to the packages of the elements and `<invoke>`s whose `env`
  attributes name it (and to the package of every `env:<slot>expr` slot,
  whose name is known only when it executes). `--env NAME[=VALUE]` and
  `--env-file` supply values: a variable's value is its flag's, else its
  last `--env-file` entry, else, only for a bare `--env NAME`, the calling
  shell's; a variable no slot can name is dropped. The pre-run listing shows
  each variable and the elements and `<invoke>`s it goes to (names only).
- Per-input bounds for invoked services: between two inputs (its start,
  an event, its due timers, an HTTP response) an invocation runs at most the
  configured fuel (`ServiceComponents::with_call_fuel`) and returns at most
  1024 events of at most 4 MiB in all, on every build (an amplification
  guard); past either bound the invocation fails (`error.communication`).
- Repository services: a literal `<invoke type>` of exactly
  `https://github.com/<owner>/<repo>/services/<name>[@<tag>]` that no
  `--service-package` binds is resolved by the CLI exactly like a repository
  namespace: the tag to a commit, the manifest
  (`services/<name>/package/<name>.service.json`, naming its own URI) and
  its optional schema at that commit, the component from
  the release authenticated by its `SHA256SUMS` when `snapshot run` or
  `run` binds it, all cached content-addressed and pinned
  (`stateforward.scxml.service-cache/v1`), with `--offline` and fail-closed
  failures. New CLI spans `scxml.cli.resolve-service-package` and
  `scxml.cli.resolve-service-component`.
- WASI 0.3 / Component Model async cutover of invoked services (wasmtime
  49.0.2, `wasm-tools` 1.244.0 validating with `--features
  component-model,cm-async`; every name below is unreleased and replaced
  with no alias). Renamed: `service-invocation` is `invoke-request`
  (`invoke-id`, `type-uri`, `source`, `content`, `parameters`,
  `attributes: list<extension-attribute>`, `references:
  list<slot-reference>`, `trace-context`), `env-binding` is
  `extension-attribute`, `machine-capabilities.start-service` is folded
  into `start-invocation: func(request: invocation)` with `variant
  invocation { session(invoked-session), service(invoke-request) }`, and
  `invocation-message.failed` is `communication-error`. Deleted: the
  `invoked-service-host` interface; the `type-uri`, `start`, `send`,
  `wake`, and `cancel` exports (now the one `invoke`);
  `outbound-http.begin` and `take`; `machine-capabilities.load-services`
  and Machine IR `invoke_types`; the threaded service transport
  (`ServiceHttp`, `ThreadedServiceHttp`,
  `ServiceComponents::with_deferred_http`, `ServiceHttpOutcome`,
  `LiveSession::deliver_service_responses`, `LiveHttp::on_service_response`,
  `MAX_SERVICE_PENDING_REQUESTS`); the per-call event bounds
  (`MAX_SERVICE_EMITS_PER_CALL`, `MAX_SERVICE_EMITTED_BYTES_PER_CALL`); the
  telemetry operation `wake`, the call cause `service-response`, and the
  operations `service-load`, `service-wake`, `service-http-begin`, and
  `session-service-response`; the manifest fields `wasi-permissions` and
  `events` of service and extension package manifests, with event
  contracts and their schema files (`PackageEvents`, `DeclaredEvents`,
  `ServiceEvents`); the `scxml_types::wasi` module, `WasiGrants`,
  `WasiGrant`, and `ServiceComponents::with_wasi_grants`;
  `ArtifactError::EventContractInvalid`, `WasiPermissionsInvalid`,
  `WasiPermissionUngranted`, and `WasiExtensionUnsupported`; validator codes
  `SCXML095` and `SCXML096`, compiler code `COMP004` (`compile` no longer
  rejects an unbound type), and the CLI's `--grant` flag and
  `[record-wasi]` refusal. The schemas of the events a service sends live
  in the document (`event:schema` on the transitions that handle them).
- Invoked sessions invoke in turn, at any depth (SCXML 6.4). Every session
  owns its machine, data model, SCXML Event I/O Processor, and clock, an
  internal and an external event queue, and runs the Appendix D loop; the
  processor pushes cross-session events (`#_parent`, `#_<invokeid>`,
  autoforward, `done.invoke.<id>`) onto the target's external queue, and a
  session whose cancellation began sends nothing more. One cooperative
  executor (`scxml-scheduler`) runs the root session and every session it
  invoked over one logical clock for batch, IRP, and live runs.
  Embedder-chosen `InvokeLimits` (`SessionHost::with_invoke_limits`, browser
  `invokeLimits`; none by default) bound the nested invoke depth and the
  invoked sessions under one root session; an invocation beyond them fails
  as that `<invoke>`'s `error.execution`. Cancelling a session cancels the
  sessions it invoked. Session snapshots carry `internal` and `external`
  queues and `invocations` nested up to the format's 32 levels.
- Live sessions: `SessionHost::open` returns a `LiveSession` (`send`,
  `advance`, `advance_to`, `run_until_idle`, `snapshot`, `close`) on a
  pluggable clock (`WallClock`, `ManualClock`, or logical), with `<log>`
  records and the root's completion as `SessionOutput`s and byte-exact
  record and replay; see [docs/live-session.md](docs/live-session.md).
  `scx run DOC` keeps one live session open over newline-delimited
  JSON (`--clock`, `--snapshot-on`, `--on-eof`, `--record`, `--replay`).
- `BasicHTTP` over the network for live sessions (SCXML C.2): the `http`
  feature of `scxml-host-wasmtime` (`NetworkClient`, a deny-by-default
  allowlist; `BasicHttpListener`), `scx run --http-allow`,
  `--http-listen`, and `--http-public-url`. Record and replay cover the
  network. The `Sent` output and the CLI's `sent` record are removed.
- Delayed `BasicHTTP` sends, held on the session clock by the SCXML Event
  I/O Processor (`schedule-basichttp`) and posted when their delay elapses
  (SCXML 6.2); the delayed-send wire records are version `8` (a kind byte,
  then an SCXML send with its message or a `BasicHTTP` send).
- Extension namespace execution end to end (SCXML 4.10;
  [docs/executable-content-extensions.md](docs/executable-content-extensions.md)):
  `execute-extension` takes the effect's event and returns the events the
  extension component queued; the Wasmtime session host binds authenticated
  `executable-content-extension` components to namespace URIs, per session. CLI
  `--extension-package`, `--extension-component`,
  `--extension-component-sha256`, `--env NAME[=VALUE]`, and `--env-file PATH`
  (Docker's format; files in order, then `--env`).
- `machine-capabilities.load-extensions(namespace-uris)`: every session, root
  or invoked, asks its host for the extension namespaces its document uses
  before data binding and the first macrostep; any failure fails the start
  closed. The Wasmtime host answers from its bindings or through an
  embedder-supplied `ExtensionResolver` (`ExtensionComponents::with_resolver`),
  binding what it loads for later sessions; the standalone proof host answers
  `unavailable`, and the browser host answers only through the page's
  `executeExtension` callback. New span `scxml.host-wasmtime.extension-load`.
- Repository namespaces: a namespace URI of exactly
  `https://github.com/<owner>/<repo>/namespaces/<name>` names a package at
  `namespaces/<name>/package/<name>.extension.json` in that repository, at
  the repository's latest release (`releases/latest`), asked once per run;
  a document pins a release by writing `@<tag>` after it. The tag is split
  off when the document is parsed (`scxml_types::strip_tag`,
  `NamespaceDeclaration::tag`, `Attribute::tag` for a literal
  `<invoke type>`) and read only by the resolver: release manifests and
  schemas name the tagless URI (a manifest naming a pinned URI is refused),
  and the validator (which validates such a document as its infoset),
  compiler, IR, engine, data models, and hosts see only the tagless URI. A
  malformed pin, one document pinning a package to two releases, and a
  pinned built-in fail closed. The run binds the root's resolution of each
  package, shared by every tagless use; an `<invoke src>` document that pins
  another release binds it for its own sessions
  (`EmbeddedDocument::extensions`, `EmbeddedDocument::services`). The
  `env`, `event`, and `scx` vocabularies `scx` provides are matched first and
  never fetched. The
  CLI (`crates/scxml-cli/src/repository.rs`) resolves every one the document,
  or a literal `<invoke src>` document it loads, uses that no flag binds: the
  tag to a commit through the GitHub API, the manifest and schema from that
  commit (exact URI, schema SHA-256, `targetNamespace`), and, before the
  session starts, the release asset the manifest names, verified against the
  release's `SHA256SUMS`, bound once and shared by every invoked session.
  Verified bytes are cached content-addressed
  (`${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions`, `--cache-dir`), the URI
  is pinned to its resolution's commit and digests (a tagless URI with its
  `tag`), a pinned URI's later runs use no network, and `--offline` forbids
  any fetch; the consent listing shows each release tag and commit.
  HTTPS only, redirects only to GitHub or configured origins (at most 5),
  explicit timeouts, no retry, bounded bodies, a token only from
  `--github-token-env NAME` and only to the API; `--github-api-base`,
  `--github-raw-base`, and `--github-download-base` name a mirror. New spans
  `scxml.cli.resolve-extension-package` and
  `scxml.cli.resolve-extension-component`.
- `env` slots: the host's attribute vocabulary `https://github.com/stateforward/scxml.rs/namespaces/env`
  (`env:<slot>="NAME"`, `env:<slot>expr="EXPR"`) names, on an extension
  element, the variable whose value the host binds to one of the slots its
  package declares (`env-slots`). The component receives an opaque
  `value-reference` per slot with `execute` (valid for that call and that
  element only) and never the value; `outbound-http` URLs, header values, and
  bodies are lists of literal and reference parts the host substitutes, then
  admits only to an origin the package declares (`http-origins`) or one a
  reference's value supplies entirely. Validator codes `SCXML083` (a slot the
  package does not declare, with a fix; a slot bound twice; a name that is no
  variable name) and `SCXML084` (the `env` namespace outside an extension
  element's attributes). The CLI lists each element's namespace, version,
  declared origins, and bound slots before running, names the `--env` flag
  for a missing value, notes refused origins, and masks the run's values in
  the text it writes.
- A `v*` tag publishes a GitHub release with every release component,
  `manifest.tsv`, and `SHA256SUMS` (CI job `release`, after every other job);
  `scripts/check-namespace-versions.sh` checks that every namespace package
  names its tagless URI and the tag's package version.
- `data-model-api` `event-data-field` (the automatic update of an empty
  `<finalize>`, SCXML 6.5.2) and `accepts-event-data` (an SCXML message
  whose data has no representation raises `error.communication` and is
  ignored, SCXML C.1). A call that answers nothing (a malformed value, a
  trap, a lost instance) accepts the data, in both hosts and from the
  browser's bundled data models.
- Validator: the internal DTD subset is processed as XML 1.0 §5.1 requires
  of a non-validating processor: documents with entity and attribute-list
  declarations are accepted, `XML003` now means a reference to no
  predefined or declared entity, the new `XML006` an external entity the
  parser does not read, and XSD validation runs over the infoset the
  declarations build; the data model's
  parse-only side-effect analysis reports `<log expr>` side effects
  (`SCXML074`, an error), conditions with them (`SCXML076`, a warning, per
  IRP 444), what it cannot decide (`SCXML075`), and writes of a system
  variable (`SCXML077`, a warning; SCXML 5.10 defines the run-time
  `error.execution`); `SCXML078` (a `<parallel>` with no regions), `SCXML079`
  (an expression under the null data model, which raises `error.execution`),
  and `SCXML080` (a target set that pairs a history with a state it
  restores), `SCXML081` (an error on every build: a state ID longer than
  the 1,024-byte name bound, which no configuration could report; such a
  document compiled and then failed at run time with a broken invariant; or
  the ID of a state that can complete, longer than 1,013 bytes, whose
  `done.state.<ID>` event exceeds the bound), and `SCXML082` (an error on
  every build: a literal `<raise event>`, `<send event>`, `<send id>`, or
  `<invoke id>` longer than the bound, or an `<invoke id>` longer than
  1,012 bytes, whose `done.invoke.<ID>` event exceeds it; such a document
  compiled and then failed the session when the element ran).
- An invoke id longer than the 1,024-byte name bound, literal or generated
  (`<state ID>.<_sessionid>.<n>` over a long state ID), or one whose
  `done.invoke.<ID>` event the bound cannot hold, fails that `<invoke>`
  with `error.execution` (SCXML 3.12.2) instead of failing the session, as
  does inline `<invoke><content>` that serializes past the machine's 1 MiB
  bound (wasm builds).
- Validator codes: `SCXML070` (an error: `delay` or `delayexpr` on a
  `<send>` whose literal `target` is `#_internal`, SCXML 6.2), `SCXML071` (an
  error: an `<invoke>` with both `namelist` and `<param>` children, 6.4,
  with a fix that rewrites the namelist as `<param location>`), `SCXML072`
  (a lint warning: a target list that names one state twice; such a list
  now lowers as the set and the state is entered once, 3.11), `SCXML073`
  (an error: a `<data id>` beginning with `_` under every data model but
  null, 5.10), and `XML006` (a reference to an external entity the parser
  does not read).
- The ECMAScript data model has `eval` and `Function` again, within the
  parse bounds (a catchable `TypeError` beyond them); `Math.random` as a
  deterministic sequence per session, seeded from `_sessionid`; and `Date`,
  which reads the session clock through the new WIT
  `execution-host.now-ms`: every session clock has a start epoch (batch,
  IRP, and logical sessions start at 2015-09-01T00:00:00Z, timed live
  sessions at the wall time), recorded as the new `InputRecord::Start` and
  CLI `"start"` record.
- A top-level `<donedata>` is returned for every session: WIT
  `call-result.return-data`, `SessionOutput::Done { data }` (the unit variant is
  gone), and the CLI `done` record's `data`/`xml`.
- `<data src>` and BasicHTTP bodies honour a JSON or XML media type: WIT
  `load-document` returns a `loaded-document` `{text, media-type}`;
  `EmbeddedDocument` carries the media type (the CLI passes a fetched
  document's `Content-Type`, else maps `.json` and `.xml` URLs), and the browser `resolve`/`loadDocument` may answer
  `{text, mediaType}`.
- WIT `data-model-api.check-effects`, which the validator's
  `expression-syntax` import forwards (a third-party data model implements
  it), and its `check-effects` telemetry operation. Whether the document
  loads a `<script src>` is the enum `external-scripts` (`absent` or
  `present`; `scxml_types::ExternalScripts` in Rust), not a boolean.
- The machine WIT observation `processed(string)` (`Observation::Processed`,
  `EffectExecutor::event_processed`): Wasmtime, browser, and conformance
  traces record each event the engine dequeues.
- Batch `ScriptedEvent` carries `data`; live sessions record the spans
  `session-open`, `session-send`, `session-advance`, and `session-close`;
  the BasicHTTP listener records `basichttp-listener-accept` for a failed
  `accept` and `basichttp-listener-connection` for a connection it
  accepted but dropped unanswered or served on a thread it could not keep
  (recorded as a failed accept before).
- `InboundHttp::claim`: an embedder that serves the built-in listener's
  messages claims each before handling it, and a message the listener
  already answered `503` (the session did not take it in time) cannot be
  claimed and is dropped, never an event.
- The browser host delivers a BasicHTTP send to a URL under
  `http://basichttp.loopback.invalid` in-process to the live session at
  that location, as the Wasmtime host does, without reaching `http.allow`
  or `http.send`; it exports `MAX_RESOURCE_URL_BYTES`.
- `scripts/build-canonical-components.sh` (a Docker build that reproduces
  the canonical release components) and `scripts/test-wasm32.sh` with the
  `wasm32-tests` CI job, which runs the tests of the wasm-only bounds on
  `wasm32-wasip2` under Wasmtime.
- [docs/spec-errata.md](docs/spec-errata.md) records every place the
  implementation follows the IRP instead of the specification text, where
  the normative prose and Appendix D differ, and the known limitations
  pending an ECMAScript engine change. docs/limits.md lists every
  implementation-defined choice and resource limit with its governing text.
- Browser host: `BATCH_START_EPOCH_MS`, the Unix epoch ms of logical time
  0 of a `manual`-timer session's clock (2015-09-01T00:00:00Z, the
  Wasmtime host's batch epoch); ECMAScript `Date` reads it plus the
  session's logical time.
- `stateforward:scxml/outbound-http`, a host-admitted, deny-by-default
  outbound HTTP import of the `executable-content-extension` world (`fetch` of
  one bounded `GET` or `POST`; no redirect followed; no ambient credentials).
  The Wasmtime host refuses every request unless the embedder supplies an
  `ExtensionHttp` transport (`ExtensionComponents::with_outbound_http`), and
  admits each one only to an origin the element's package declares or a
  reference's value supplies; the `http` feature's `ExtensionHttpClient`
  bounds each request by a 10 s connect and 120 s total timeout and a 16 MiB
  response body. `scx snapshot run` and `run` supply it (not with
  `--record` or `--replay`). New host span
  `scxml.host-wasmtime.extension-http-fetch`.
- `namespaces/`, the top-level directory for repository namespaces, one
  folder each.
- `stateforward:scxml/session-snapshot-host`, a read-only import of the
  `executable-content-extension` world: during `execute`, `snapshot(exclude)`
  returns the executing session's canonical `scxml.session-snapshot/v2` document
  (one session, `invocations` and `previews` excluded, carriers stripped, marked
  `options.point: "executable-content"`), `unavailable` outside `execute`, at
  most 64 per `execute` call. The machine hands the host its executable-content
  image with `execute-extension` (new `snapshot` parameter): the configuration
  the element observes (the one `In()` answers), the internal queue, and the
  transitions walked from that configuration's innermost states
  (`Engine::effect_snapshot`), computed only when the new
  `machine-capabilities.extension-snapshot-request` reports that the bound
  extension component imports the interface. The machine-snapshot records moved
  from `machine-api` to `machine-capability-types` (`machine-api` re-exports
  them). The format's decoder accepts a non-empty internal queue and an invoked
  root only in an executable-content document. Each call is one
  `scxml.host-wasmtime.session-snapshot` span. The browser host offers no
  executable-content snapshot (its `extension-snapshot-request` is always
  false).
- The host's `event` attribute vocabulary
  (`https://github.com/stateforward/scxml.rs/namespaces/event`, unversioned
  and never fetched; [namespaces/event](namespaces/event/README.md)):
  `event:schema` (a JSON Schema, inline or from a file relative to the
  document with an optional JSON Pointer) or `event:schemaexpr` on
  `<transition>`, `<send>`, `<raise>`, and `<donedata>` describes the event
  data each handles or creates. The compiler checks each schema against its
  dialect's metaschema (2020-12, 2019-09, or draft-07), bundles the
  same-file references a pointer needs, rejects references to other
  resources, and embeds it as canonical JSON with its SHA-256 in Machine IR
  (`event_schema` on transitions, `raise` and `send` actions, and
  `donedata`; absent otherwise, so existing IR bytes are unchanged); session
  snapshot transitions carry it (`event_schema`), and `event:schemaexpr` is
  carried unevaluated. Metadata only: execution and the IRP results are
  unchanged. New validator codes `SCXML085`-`SCXML092` with explanations;
  the CLI reads the referenced files relative to the document
  (`scxml.cli.read-event-schema`), the validator and compiler WIT requests
  take them as `event-schemas` (`event-schema-resource`), and the machine
  WIT `transition-snapshot` gains `event-schema`. New crate
  `scxml-event-schema`, which checks schemas with the `boon` crate.
- Each repository namespace has its own implementation report plan,
  `namespaces/{env,event}/irp`: W3C IRP-format assertions over
  numbered statements added to each namespace's README, `.txml` documents
  in the W3C `conf:` vocabulary prepared by the W3C templates for
  ECMAScript and XPath, and checked-in canonical baselines, run through
  the same component pipeline as the W3C IRP and reported separately
  (env 20, event 29 tests; every one passes in both data
  models). The conformance harness gains the `namespace-irp` corpus kind
  (`conformance prepare --namespace NAME`), the `document-rejection`
  oracle (a manifest `<reject code="..."/>`: the compiler must reject the
  document with exactly those codes), extension bindings
  (`conformance run --extension-package MANIFEST.json
  --extension-component-sha256 URI=HEX`; the report identity lists every
  bound package as `extensions`: namespace URI, package version, and
  the SHA-256 of its manifest, schema, and component), and a deterministic in-process
  stand-in, with fake keys, for the server the example extension's
  `<example:fetch>` reaches (the `env` suite binds the example package). The baseline
  generator is `generate_irp_baselines` (formerly
  `generate_w3c_irp_baselines`) and regenerates every suite; the canonical
  component build and the `irp-components` CI job also build the example
  extension component. `ExtensionPackageManifest::from_json` (in
  `scxml-validator-core`) is now the one extension package manifest parser
  (`EXTENSION_PACKAGE_FORMAT`).

### Changed

- Repository packages are resolved by a Wasm component (hard cutover):
  `scx` embeds the GitHub resolver component
  (`components/scxml-resolver-github`, the new `stateforward:scxml/resolver`
  world and `resolver-api` interface, with `resolve` and `fetch`), which
  knows what a repository URI and its `@<tag>` mean and fetches through WASI
  0.3 `wasi:http/client` (`wasi:http@0.3.0` and the packages its worlds name
  are vendored under `crates/scxml-wit/wit/scxml/deps`). The CLI's native
  GitHub client is deleted; the CLI keeps trust (digests, exact URI and WIT
  identity, `targetNamespace`, the cache, pins, `--offline`, resolving once
  per root session), passes `--github-*-base` to the component as its
  configuration, and runs it through `scxml-host-wasmtime`'s
  `ResolverComponent` (feature `http`), which admits its requests only to
  the bases' origins and GitHub's asset hosts and adds the
  `--github-token-env` token itself. The URI pattern has one definition,
  `scxml_types::RepositoryUri`, shared by the parser's `strip_tag`, the CLI,
  and the component. The browser host still resolves no repository package.

- Every repository namespace lives in its own folder under the top-level
  `namespaces/` directory, and a repository namespace URI is exactly
  `https://github.com/<owner>/<repo>/namespaces/<name>` (pinned with
  `@<tag>`), and the host's `env` vocabulary is
  `https://github.com/stateforward/scxml.rs/namespaces/env`. Any other
  repository path is an ordinary namespace URI the CLI never fetches. The
  release script is `scripts/check-namespace-versions.sh`
  (`scripts/set-namespace-version.sh`, which rewrote URIs to a tag, is
  deleted). Hard cutover from the folder used during development; no alias
  remains.
- Component artifacts are bounded by GitHub's per-file release-asset limit
  (each file in a release must be under 2 GiB), since components ship as
  release assets. `MAX_AUTHENTICATED_COMPONENT_BYTES` and
  `DEFAULT_COMPONENT_BYTES` in `scxml-host-wasmtime` (was 8 MiB) are
  2,147,483,647 bytes, and so are the bounds that follow them: the CLI's
  repository component download and `--extension-component` read, the
  conformance executor's release read, and the browser host build's
  component check (was 64 MiB). Every reader checks a file's metadata or a
  download's `Content-Length` before reading and grows its buffer with the
  bytes read; nothing reserves the bound. An empty release artifact is now
  `ArtifactTooLarge`, as an empty packaged or extension component already
  was.
- The Wasmtime host's `DEFAULT_FUEL` is 3,000,000,000 instructions (was
  2,000,000,000), sized on documents `compile` accepts: the start of an
  ECMAScript root around a 4,000-state inline `<invoke>` child takes 1.73
  billion, 1.72 times headroom, where the null-model root the default was
  sized on is `SCXML037` and never reaches a host
  (`default_fuel_sizing_workloads` now validates every workload and
  requires 1.5 times headroom of each). One runaway host call runs about
  0.22 to 0.25 s of Wasm on an Apple M3 Max
  (`default_fuel_runaway_call_time`), its memory-growth copy allowance is
  3,000,000,000 bytes, and a 64-region eventless runaway reaches the
  machine's 1,024-command bound before the fuel (a 128-region one traps
  out of fuel). The CLI's `run` and `snapshot run` use it.
- The browser host's `DEFAULT_FUEL` is 75,000,000 charges (was
  50,000,000), the Wasmtime default divided by 40 as before. It is also
  the bundled data models' default per-call cap and the instantiation
  budget the build bakes into every metered module. It admits the
  4,000-state inline child's start (22.4 million charges) with 3.35 times
  headroom and every other sizing workload with at least 17 times
  (`node scripts/fuel-sizing.mjs`); one runaway host call runs about
  0.3 s of Wasm in Node 24 on an Apple M3 Max (the heavy nested start;
  a 64-region `<raise>` toggle loop about 0.21 s), and both still stop
  with `fuel-exhausted`.
- Generated invoke ids (`<state>.<session>.<n>`, SCXML 6.4.1) start at
  `.0`, as generated send ids start at `sendid_0`, and an `<invoke>` with a
  literal `id` no longer takes an ordinal: only a generated id does. The
  session snapshot's `counters.invoke` is the next generated ordinal, so a
  session that has generated no invoke id reports `invoke: 0` (it reported
  `1`).
- The browser package's public declarations (`src/index.d.ts`) are
  typechecked strictly with the pinned `typescript` `5.9.3`
  (`npm run typecheck`, run by `scripts/verify-browser-host.sh` and so by
  CI).
- Resource limits apply to wasm builds only. Every resource bound is
  declared through `scxml_types::wasm_bound(n)`, which is `n` on `wasm32`
  and `usize::MAX` natively (and compared through
  `scxml_types::exceeds_wasm_bound`), so the shipped components keep their
  bounds and native builds have none of their own:
  - validation and compilation: the native `scx` validates and
    compiles documents of any size and never reports `SCXML027`,
    `SCXML041`, `SCXML069`, or `COMP001`, nor `SCXML028`, `SCXML040`,
    `XML002`, or `XSD006` for a size bound;
  - native-only code loses its caps outright. The Wasmtime session host
    and `scxml-scheduler` lose external events per session, the 64 KiB
    `<log>` text bound, log and trace records
    (`SessionOutcome::trace_truncated` is removed), loaded-document size,
    media-type length, the queue bounds, the namespace execution bound, the
    loaded-script bound, the 64 KiB bound on a `<script src>` of
    data-model text (a Wasm script component stays at most 4 MiB on every
    build), the 256-variable bound of a `ExtensionEnvironment`
    (`MAX_ENVIRONMENT_VARIABLES` is removed), and the invoke-limit defaults
    and ceilings;
    `InvokeLimits` fields are `Option`, `None` by default;
    `ArtifactError::MachineIrTooLarge` is removed (oversized IR is the wasm
    machine's `payload-too-large`); `execution-host.raise` no longer
    answers `capacity-exhausted` after 1024 events. The CLI loses its 8 MiB
    source, 1 MiB input line, 256 MiB replay log, and default command
    budget. The browser host loses its mirrors of these caps, its
    `mailbox-limit` error, its 4 MiB bound on a child document's source
    (the compiler component's own bound decides), the async bridge's 32
    MiB reply bound (a reply is
    at most what the bridge's length word describes, 2 GiB), and its
    resource bounds of 64 URLs and 16 MiB of text per session (the
    16,384-byte URL bound stays), and its 64 KiB bound on `<script src>`
    text (the data model's own script bound decides, raising
    `error.execution` when the script runs, as under Wasmtime).
  - component bounds that had applied natively too: the ECMAScript data
    model's 128 distinct state IDs one operation may ask `In()` about.

  What stays on every build is not a resource bound: native stack guards
  (the XML depth of 256 and the ECMAScript and XPath parse nesting), the work and
  amplification guards (DTD entity expansion, now counting the entity
  references attribute defaults expand and charging every supplied default
  its name and value; XSD content-model expansion and compilation work; the
  ECMAScript side-effect analysis's 200,000 steps per fragment; the graph
  analysis work budget of 8,000,000 units per machine or 1,024 per element
  and target reference, past which even the native `scx` reports
  `SCXML028`; and the rendered and SARIF report size), the format and
  interchange contracts (WIT value bounds, the `data-model-api`
  `initialize` bounds, and the session snapshot format), network ingress
  (the BasicHTTP listener now reads each request within one deadline and
  rejects zero timeouts), and artifact authentication. A checkpoint
  snapshot that the snapshot format cannot represent now fails only that
  snapshot (`SessionOutcome::snapshot_failures`), and
  `conformance run --snapshot-every-checkpoint` fails closed on one.
- The default data model is ECMAScript (SCXML 3.2.1: "platform-specific";
  3.2.2: "Platforms SHOULD document their default data model").
  A document without `datamodel` is lowered with `ecmascript` in its Machine
  IR and validated as `datamodel="ecmascript"`; it no longer fails at start
  with `unavailable` when it uses data or expressions. `SCXML045` is only
  the note that a run links no checker for the machine's data model.
- `_name` of a document without a `name` attribute has no value (SCXML
  5.10): ECMAScript `undefined`, an empty XPath `<_name/>`; snapshots omit
  `name`. It was `"scxml"`. WIT `machine-name` and snapshot `name` are
  optional.
- Validator findings whose specification-defined outcome is a run-time
  error event are warnings, not errors: `SCXML038` (syntax), `SCXML039`
  (location), `SCXML056` (undeclared XPath variable), and `SCXML066` (empty
  expression); every applicable official IRP document compiles through
  `scx` with no error. Accepted as conforming: `type` on `<initial>`
  and history transitions (`SCXML010`, `SCXML020`), a `<parallel>` with no
  regions, `<assign type="delete">` with an `expr` (which is ignored, CR
  C.3.6), every `<data id>` ECMAScript can declare (non-ASCII, hyphenated,
  or reserved words; `SCXML061` warns when no expression can name one),
  and `:` in event descriptors (`SCXML013`).
- Transition content runs in document order (SCXML 3.13), and default
  history content runs when the history's parent stays active (3.10.2);
  both differ from the informative Appendix D and are recorded in
  docs/spec-errata.md.
- `<send>` evaluates every argument before classifying its target (6.2),
  so an argument error is `error.execution`, not a delivery failure; a
  `<cancel sendidexpr>` value no send id can equal raises `error.execution`
  instead of failing the session; a delayed send the clock has no room for
  raises `error.execution` (3.12.2) instead of failing the session.
- An XPath `<foreach>` item variable denotes the collection's own node, and
  the declared `<data id>` element stays in place (SCXML 4.6, CR C.3.1).
- The side-effect analysis reports effects of functions a `<log expr>` or
  condition creates (callbacks, getters, conversion methods).
- Every browser `BasicHTTP` `_ioprocessors` entry has a `location` (C.2.3);
  a session the page gives none is listed at its loopback address.
- The conformance `Outcome` oracle requires the one `pass` log of a root
  session finished in the templates' `pass` final state; cases the W3C
  declares not applicable to a data model are listed with their reasons.
- CI runs the Wasmtime host tests with the `http` feature.
- Validator: `SCXML004` reports every `<transition>` outside `<initial>`
  and `<history>` with no `event`, `cond`, or `target`, executable content
  or not (SCXML 3.5.2), with a placeholder-event fix; `SCXML026` and
  `SCXML036` (eventless loops) are lint warnings, since Appendix D allows a
  macrostep that does not terminate; under `datamodel="null"`, `SCXML037`
  reports each outermost element B.1.7 rules out (`<datamodel>`, `<data>`,
  `<assign>`, `<donedata>`, `<content>`, `<param>`, `<script>`,
  `<foreach>`) with remove and switch-to-ecmascript fixes, and every
  expression there is the `SCXML079` warning; `SCXML057` no longer exempts
  machines with a `<foreach>`; `COMP002` for an unbound extension namespace
  points at its first element or attribute, names the namespace, and
  suggests `--extension-package`.
- `<send>` targets are the SCXML 1.0 C.1 forms only: `#_scxml_<sessionid>`
  of a running invoked child now reaches it, an unknown `#_scxml_` or `#_`
  address raises `error.communication`, and any other form
  `error.execution`; the WIT sender record lists each live invocation's
  invoke id and session id (`invocation-address`).
- An asynchronous error (a delayed send whose target no longer answers
  when it falls due, a BasicHTTP request that fails after acceptance) is a
  platform `error.communication` on the sender's internal event queue
  (SCXML 6.2.4), processed at once and never autoforwarded, through the
  machine WIT `session.communication-error(send-id)`; `session.send`
  accepts external events only, and `route-due` `undeliverable` carries the
  send id. Every `<send>` failure (`typeexpr`, `eventexpr`, `targetexpr`,
  `delayexpr`, `<param>`, `namelist`, an immediate BasicHTTP refusal) sets
  `_event.sendid`, generated when the author gave none; WIT
  `basichttp-send` carries `send-id`, and CLI `communication-error`
  records carry `sendid` (replay rejects one without it).
- The ECMAScript data model keeps one realm per session: declarations,
  closures, classes, and changes to built-ins persist, and a failing
  script keeps what it did before the failure; `<assign>` assigns
  strictly, so an undeclared or read-only target raises `error.execution`.
  `In()` is a real, non-writable global function. `<script>` globals may
  have any name but the system variables; `<foreach item>` and `index`
  must each be one identifier (`_item` and non-ASCII names are legal), and
  `<assign location>` accepts any ECMAScript left-hand-side expression. A
  location's leading identifier is read as ECMAScript reads an
  `IdentifierName` (Unicode `ID_Start` and `ID_Continue`). The data-model
  snapshot lists only declared `<data>` ids, each a portable value or
  `{"type":"text","typeof":K,"text":S}` (WIT `variable-value`
  `text(variable-text)` replaces the function and opaque forms).
- XPath data model: a `<foreach index>` counts from 1 (CR C.3.10; the
  ECMAScript index still counts from 0); `<data>` string content, inline or
  `src`, is space-normalized (C.3.1); `_event.data` follows C.3.7.1
  (key-value pairs, else a valid XML document, else a space-normalized
  string), so a JSON array or unparseable markup becomes a string, and
  `accepts-event-data` accepts every portable value. Space normalization
  everywhere strips only XML white space (space, tab, CR, LF).
- An inbound BasicHTTP body that is not a form (or a form's bare
  `<content>`) is read by its JSON or XML `Content-Type` (`+json` and `+xml`
  included), else tried as JSON, then as an XML document, else kept as
  space-normalized text.
- The browser host's `invokeLimits` must be a plain object with no key but
  `maxInvokeDepth` and `maxInvokedSessions`, so a misspelled key is
  `invalid-argument` instead of no limit. Each own property is read once,
  by its descriptor: an accessor property is `invalid-argument` without its
  getter being called, and the session (in the page or in a worker) keeps
  that frozen copy, so a later change of the page's object changes
  nothing. An extension component's
  `execution-host.send` routes only SCXML Event I/O Processor sends; any
  other `processor-type` is `invalid-input`.
- The BasicHTTP listener's `ListenerConfig::read_timeout` is the time
  allowed for one whole request, head and body; `bind` rejects a zero
  timeout, and a failed `accept` backs off (10 ms, doubling to 1 s) when
  the listener itself failed. Its accept loop never blocks (it checks for
  `stop` every 10 ms, `ACCEPT_POLL`), so `ListenerHandle::stop` returns
  promptly on any bound address, including `0.0.0.0` and `::`, and
  dropping a `ListenerHandle` stops it. `scx run --http-listen`
  keeps its listener for the session, fails the run with
  `http-listener-failed` when a listener thread panicked, and stops it on
  close.
- A prepared IRP corpus carries its errata file next to `corpus.json`, and
  loading rehashes and checks it.
- `scxml-xpath`: `Store::children` and `Store::attributes` return a
  `Siblings` iterator instead of a slice, and the crate is `#![no_std]`
  (alloc only).
- Wasmtime is 49.0.2 (RUSTSEC-2026-0315 and RUSTSEC-2026-0316, fuel
  accounting; RUSTSEC-2026-0325, RUSTSEC-2026-0326 and
  RUSTSEC-2026-0327), and the wasm32 tests run under `wasmtime-cli`
  49.0.2.
- Invocation never nests calls (SCXML 6.4 sets no depth). The WIT
  `machine-capabilities.start-invocation` and `cancel-invocation` answer
  `invocation-progress` (`completed` or `deferred`); a deferred request
  suspends the machine call right after the `<invoke>` effect
  (`call-result.suspended`), the host starts or cancels the child from its own
  loop, and `session.invocation-completed` continues the call with the
  request's result, so every session runs exactly what it ran when the
  child started inside the call. The engine gains `Command::Continue`,
  `EffectExecutor::suspend_requested`/`resume_effect`,
  `RunReport::suspended`, and `interpret::InvocationProgress`. Both hosts
  start, cancel, and drop invoke chains iteratively, and the Wasmtime
  session host's nested-start stack guard is removed: a Wasmtime chain
  10,000 sessions deep runs, and a document that invokes itself without
  end runs until the caller's fuel or command budget ends it. A command and every
  start, cancel, and continuation the host carries out for it are one host
  call of the fuel budget in both hosts, as one uninterrupted call would be
  (by default 3,000,000,000 instructions under Wasmtime and 75,000,000
  metered charges in the browser), so a finite command that starts a deep
  or heavy chain of invocations can exhaust it.
- Invoked sessions' `_sessionid`: the host names each one
  `<root _sessionid>.<n>` in start order across the run (formerly
  `<parent id>.<n>`, which grew with the nesting depth and stopped a chain
  at about 510 levels); `invoked-session.session-id` is removed.
  Host-side records name the parent beside the session: the live replay
  log's `http` record gains the session's invoke-id `path`.
- `scxml-scheduler` addresses sessions by `SessionKey` (`InvokedSessions`
  no longer keys or returns invoke-id paths, except `path` on request),
  `Sessions::advance_clock` advances the whole run (a session's clock may
  move when it is next used), and one drain iteration reads only the
  sessions a command ran on (`InvokedSessions::touch`). Each session takes
  its place in invocation order when it registers (an order-maintenance
  label), so ordering queued work no longer compares ancestor chains:
  taking the next queued entry is logarithmic and cancelling or removing a
  session costs its subtree, in the scheduler and in the browser host
  (whose `InvokedSessions` now addresses sessions by integer key too).
  10,000 levels each queued at once order, drain, and cancel in at most
  12 n log2 n counted scheduler work steps (1,238,945; the test-only count
  of `a_deep_chain_with_every_level_queued_is_taken_in_order_in_n_log_n_steps`
  replaces its former 2 s wall-clock bound) and about 25 ms in the browser,
  where 2,000 levels took 74 s in the browser's former path-keyed queues.
- A browser `resources.resolve` answer that is neither text nor
  `{text, mediaType}` is `unavailable` (error.execution) for a runtime URL,
  as it is for a prefetched one; it was `platform` and failed the session.
  So is a runtime resolve that rejects with a `CapabilityError` of any code
  (`platform` included), as a prefetched URL's rejection is.
- The machine releases every `<foreach>` in progress when one of its calls
  returns (WIT `machine-capabilities.release-foreach`, forwarded to the new
  `data-model-api.release-foreach`): a loop an error ended early no longer
  stays recorded in the ECMAScript or XPath data model for the rest of the
  session (natively, where no count bounds them).
- Repository tooling: the browser package declares its repository,
  homepage, and issue tracker, and `npm pack` copies the license texts into
  it; the scripts digest with `sha256sum` or, without it, `shasum -a 256`;
  CI runs pushes to `main` only (pull requests as before) and cancels a
  superseded run; and the `release-components` artifact carries the license
  texts.

### Renames

- Every public surface uses the SCXML 1.0 vocabulary, and host concepts
  use plain host words; no alias remains.
  - Session snapshots: the `actions` section is `transitions`, a list of
    the transitions an event could trigger with no derived `action` field
    (a transition's `type` says whether it exits its source, not whether an
    event is internal) and no `cancel` list (the cancelable send ids are the
    `pending_sends` ids); the `datamodel` section is `data_model` and the
    `children` section is `invocations`; the `not-quiescent` code is
    `not-stable`. WIT `transition-snapshot.kind` is `type`
    (`transition-type`).
  - "Quiescent" is "stable", after SCXML 6.4's "stable configuration":
    WIT `step.quiescent` and engine `is_quiescent` are `stable` and
    `is_stable`. The result of one host call is a call result, not a step:
    WIT `record step` is `call-result`, engine `Step` is `CommandResult`
    (`RunReport.result`, `MAX_EFFECTS_PER_COMMAND`), `LiveStep` is
    `LiveCallResult`, the browser `Step` is `CallResult` and due deliveries
    report `{results, complete}`, and `scx run` writes `call` records
    and takes `--snapshot-on call`. The engine-command budget is a command
    budget: `--max-steps` is `--max-commands`, the corpus manifest's
    `default_step_budget` and a case's `max_steps` are
    `default_command_budget` and `max_commands`, a conformance case reports
    `commands`, and the failure codes are `command-budget-*`.
  - Extension packages: "namespace package" is "extension package" and
    "behavior component" is "extension component": WIT `namespace-package`
    is `extension-package` and its `behavior-artifact` is
    `extension-artifact`; the manifest format is
    `stateforward.scxml.extension-package/v2` in `*.extension.json`; the CLI
    takes `--extension-package`, `--extension-component`, and
    `--extension-component-sha256`; the crate is
    `scxml-extension-example`. The process that
    runs the CLI is the calling shell.
  - "Child session" is "invoked session" (SCXML 6.4): WIT
    `child-invocation` is `invoked-session` and the host spans are
    `scxml.host-wasmtime.invoked-session-start` and `-cancel`. WIT
    `invoke-request.invoker-type` is `type-uri`, `dispatch-action` is
    `dispatch-decision`, and `send-message.fields` and
    `basichttp-send.fields` are `params`.
  - Machine IR keys use the SCXML attribute and element names: `cond`,
    `eventexpr`, `targetexpr`, `type`, `typeexpr`, `idlocation`,
    `delayexpr`, `namelist`, `srcexpr`, `autoforward`, `sendid`,
    `sendidexpr`, `onentry`, `onexit`, `donedata`, and `elseif`.
    `<donedata>` is only a `<final>` child, never executable content: the
    IR has no `donedata` executable content, and the engine's
    `Effect::Donedata` and `ReturnDonedata` are `DoneEvent` and
    `ReturnDoneEvent`, after the done event `done.state.<id>`.
  - "Data model" names the concept and `datamodel` only the element and the
    attribute: the data-model crates are `scxml-data-model-ecmascript`,
    `scxml-data-model-null`, and `scxml-data-model-xpath`, and their
    telemetry labels are `data-model-*`. "Effect" is defined once as host
    vocabulary: one request the machine makes of its host.
- Extensions to executable content (SCXML 1.0 section 4.10) are named in
  the specification's words; "dispatch" now means only sending a message
  (section 6.2), and "handler" only the specification's `<onentry>`,
  `<onexit>`, and `<finalize>` handlers. In the WIT,
  `machine-capabilities.dispatch-custom` is `execute-extension`,
  `dispatch-snapshot-wanted` is `extension-snapshot-request`, the
  `custom-element` record is `executable-content-extension`,
  `custom-attribute` and `custom-content` are `extension-attribute` and
  `extension-content`, the `dispatch-result` variant is
  `extension-outcome`, the `custom-dispatch` record is
  `extension-execution`, the `namespace-handler` world and
  `namespace-handler-api` are `executable-content-extension` and
  `executable-content-extension-api`, and its `dispatch` is `execute`.
  The snapshot point `options.point: "dispatch"` is
  `"executable-content"`. The browser host's capability provider
  `dispatchCustom` is `executeExtension`. In the Wasmtime host,
  `NamespaceHandlers`, `HandlerEnvironment`, the `HandlerHttp*` types,
  `authenticate_namespace_handler`, `SessionHost::with_namespace_handlers`,
  and `DEFAULT_NAMESPACE_CALL_FUEL` are `ExtensionComponents`,
  `ExtensionEnvironment`, `ExtensionHttp*`,
  `authenticate_extension_component`,
  `SessionHost::with_extension_components`, and
  `DEFAULT_EXTENSION_CALL_FUEL`; the spans
  `scxml.host-wasmtime.namespace-{instantiate,dispatch,shutdown,http-fetch}`
  are `extension-{instantiate,execute,shutdown,http-fetch}`, the guest
  `scxml.module` value `namespace-handler` is
  `executable-content-extension`, and its log event `namespace-handler-log`
  is `extension-log`. `docs/namespace-handlers.md` is
  `docs/executable-content-extensions.md`, the Wasmtime host test suite
  `namespace_handlers` is `executable_content_extensions`, and the package
  manifests' `wit-world` is
  `stateforward:scxml/executable-content-extension`. In Machine IR,
  `ExecutableContentIr::Custom` is `ExecutableContentIr::Extension` and
  `MachineIr::custom_namespaces` is `extension_namespaces`; the conformance
  capability code `unsupported-custom-namespace` is
  `unsupported-extension-namespace`; and diagnostics, docs, and WIT comments
  say "extension namespace" for "custom namespace". "Custom action" stays
  where it names an element from another namespace in executable content
  (SCXML 1.0 Appendix G.7, Custom Action Elements). No alias remains.
- The Wasmtime host's extension module is `extension.rs`, and the example
  extension schema fixture is
  `tests/fixtures/extension-example.xsd`.

### Removed

- The bounded logical `scxml-invoked-service` component and its 0.1.0
  `invoke-request` WIT record (`invoker-type`, text `content`; the name is
  now the invoked-service request), `ArtifactKind::InvokedService`, the
  capability graph's invoked-service step
  (`CapabilityGraphComponents::invoked_service`,
  `CapabilityGraphExecution::invoked_lifecycle_completed`), and its
  telemetry labels (`invoked-service-component`,
  `scxml.invoked-service.*`): invoked services are real components now.
- The `env` extension component (`env:get`, `env:set`, the
  `scxml-extension-env` crate and package, and the host's `ExtensionEnv`
  artifact identity): the `env` namespace is now only the host's slot
  attribute vocabulary. `--inherit-env` and per-element HTTP grants
  (`--extension-http-allow`) are gone: a package declares its origins, and a
  run supplies values only by name. Extension components import no WASI
  interface (`wasi:cli/environment` is no longer linked).
- Validator codes `SCXML035` (its findings are `SCXML004` errors),
  `SCXML043` (a value expression under the null data model, now the
  `SCXML079` warning), and `SCXML054` (an ECMAScript `=` in a condition,
  now `SCXML074`/`SCXML076` with a `===` fix).
- The null data model's literal extension: `null` and quoted strings are
  no longer values and an empty `<script>` no longer runs; every value,
  location, `<assign>`, `<foreach>`, and script raises `error.execution`
  (a location or `<foreach>` used to end the session), and the telemetry
  operation `scxml.datamodel-null.evaluate-value` is gone.
- `<send>` target forms SCXML 1.0 does not define: `#_self`, a bare invoke
  id, `#_scxml_<invokeid>`, and `#_<own or parent sessionid>`.
- The ECMAScript adapter globals `__scxml_max_value_bytes`,
  `__scxml_max_nesting_depth`, `__scxml_dom`, and `__scxml_xml_text`.
- The conformance `--disposition` flag and `Filters.dispositions`, and
  `ExecutionTrace.truncated` with the `manual-trace-truncated` oracle
  precondition.
- The browser exports `MAX_CHILD_SESSIONS`, `MAX_LOADED_DOCUMENT_BYTES`,
  `MAX_LOG_TEXT_BYTES`, `MAX_TRACE_RECORDS`, `MAX_RESOURCE_URLS`, and
  `MAX_TOTAL_RESOURCE_BYTES`, and the browser error code `mailbox-limit`.
- The WIT `foreach-result.unsupported` case (and the browser
  `ForeachResult` `{ tag: 'unsupported' }`): a value that is not a legal
  `<foreach>` collection is `illegal`.

### Fixed

- A repository namespace download whose body is exactly its byte bound
  resolves: the network client rejected it as over the bound when it read
  past the last byte.
- In the browser host, routing a loopback send reads the BasicHTTP
  location of a session whose `http.location` throws at most once a
  turn, and records it as one failed `capability` span for the whole
  session: the loopback's read of a sending or invoking session's
  location was retried on every send, and an invoked session whose read
  failed when it started was recorded again by the loopback.
  `receiveHttp` reads the root's location for each request and throws
  `invalid-argument` when it throws; `HttpOptions.location` documents
  each outcome.
- Both hosts read an inbound BasicHTTP `traceparent` or `tracestate`
  header without its leading and trailing HTTP optional white space (SP
  and HTAB, RFC 9110 §5.5) only. The browser host also stripped U+FEFF
  and the Wasmtime host U+0085, so the same header continued the
  caller's trace in one host and not the other, and both stripped
  U+00A0. Trace context never changes semantics.
- `compile` accepts `<scxml exmode>` (`lax` or `strict`), which the W3C
  schema declares and the Recommendation gives no semantics, and lowers it
  to nothing; it failed with `COMP001` (unsupported attribute) on a
  document `validate` accepted.
- Decoding Machine IR rejects an event descriptor or name list item that
  holds XML white space (` e `); it checked only that the item was one
  token, so a padded descriptor that could never match was accepted.
- Only XML white space (#x20, #x9, #xD, #xA) separates, pads, or blanks
  an SCXML value, in `validate`, `compile`, and Machine IR. The bundled
  schema engine splits list types (`IDREFS`, `NMTOKENS`) on any Unicode
  white space, but its findings agree: a lexical error in `initial` or
  `target` is reported as `SCXML005`/`SCXML006`, which split on XML
  white space only, and `<invoke autoforward>` holding U+00A0 or U+1680
  is an XSD error. A name containing U+1680 OGHAM SPACE MARK (an XML
  name character and Unicode white space) validated and then failed to
  compile with `InvalidName`, `event="&#x1680;"` was reported as an
  empty descriptor list (`SCXML013`), and text of U+1680 in element-only
  content passed `validate` and failed to compile; it is now
  `XSD-cvc-complex-type.2.3`, as text of `x` is, in every SCXML element
  the schema assesses, also one under the lax wildcards of `<data>`,
  `<assign>`, `<content>`, and extension elements, such as an inline
  child `<scxml>`. An SCXML-namespace name the schema does not declare
  is not assessed, so its text draws no finding of either kind. The null data model's `In('id')` names
  such a state, in the component and in the validator's `SCXML079`
  check. An XPath `<script>` is empty only when it holds nothing but XML
  white space, in the component and in `validate`: `<script>&#x1680;</script>`
  did nothing and passed `validate`; it raises `error.execution` and is
  `SCXML038`.
- A `<script src>` is blank only when it holds nothing but XML white
  space, in `validate`, `compile`, and both hosts: `src="&#x1680;"` was
  `SCXML051` and refused, and a `src` of U+FEFF loaded under Wasmtime
  but was `invalid-input` in the browser host.
- `<data>` text, inline or from `src`, padded with white space that is
  not JSON or XML white space (such as U+1680 OGHAM SPACE MARK) is a
  space-normalized string (B.2.1), not JSON: `&#x1680;1&#x1680;` was the
  number 1, and text of only U+1680 was undefined. Under the ECMAScript
  data model, such text around one element is not an XML document: from
  `<data src>` it was a DOM `Document` and is a string, and as an XML
  value its DOM is a fragment (`nodeType` 11). A BasicHTTP body padded
  with it was JSON or an XML document in `_event.data` and is a
  space-normalized string.
- An ECMAScript expression, location, or `<log expr>` is blank only when
  it holds nothing but ECMAScript white space and line terminators
  (ECMA-262 §12.2, §12.3), in the data model and in `validate`'s syntax
  and side-effect checks. `<log expr="&#x85;"/>` logged nothing and
  `expr="x&#x85;;"` evaluated `x`, although U+0085 is not ECMAScript white
  space; both raise `error.execution`. An expression of U+FEFF, which is
  ECMAScript white space, was a syntax error and is blank: an empty
  `<log>` logs nothing, and an empty `cond` or `expr` is reported as
  "expected an expression, found nothing".
- `validate` and `compile` bound an attribute value by the value the
  compiler stores, on wasm builds: a white-space-collapsing SCXML
  attribute by its collapsed value (an `id` padded past 1 MiB compiled
  with `LimitExceeded` although `SCXML069` did not fire) and an attribute
  of inline XML, which the compiler keeps byte for byte, by its exact
  value even on an element in the SCXML namespace (`SCXML069` measured
  its collapsed value).
- An ECMAScript `<foreach>` over an XML (DOM) value raises
  `error.execution` and ends only its block (SCXML 4.6: not "a legal
  iterable collection"); it ended the session with `unavailable`.
- Inline `<invoke><content>` reaches the child document with its text
  and attribute values escaped as the compiler escapes inline XML
  (`scxml_compiler_core::escape_markup`): text containing `&` or `<`
  made the child fail to parse, and a tab, line feed, or carriage return
  written as a character reference in an attribute value became a space.
- Lowering reads every attribute whose XSD type collapses white space
  (`xsd:token` and its derived types, `xsd:anyURI`, `xsd:decimal`) as its
  collapsed value (XML Schema Part 2 `whiteSpace="collapse"`), as the
  validator's schema does: `<raise event=" go ">`, `<send event>`,
  `<transition type>`, `<scxml version>`/`binding`, `<assign type>`, and
  `<invoke autoforward>` with surrounding white space validated but failed
  to compile, and `<scxml name>`, `<param name>`, `<assign attr>`, `<send
  target>`, `<cancel sendid>`, and `src` values kept the white space.
- The validator reads every attribute whose type in the bundled SCXML
  schema collapses white space (XML Schema 1.1 Part 2 §4.3.6; listed in
  `scxml_validator_core::COLLAPSED_ATTRIBUTES` and derived from the
  schema by a test) as its collapsed value, as the schema and lowering
  do: a padded `id` (`<state id=" s ">`) was `SCXML003` and every
  reference to it `SCXML005`/`SCXML006`, a padded `datamodel` was
  `SCXML044`, and `SCXML081`/`SCXML082`/`SCXML069` measured the padded
  length. A padded `id` now declares its collapsed value for duplicate
  detection (`SCXML002`), target and `initial` resolution, `In()`, the
  bounds, the engine, and snapshots, and `datamodel=" xpath "` selects
  XPath in the validator, the compiler's script-package check, and the
  conformance harness. List values (`IDREFS`, event descriptors,
  `namelist`) split on XML white space only (space, tab, CR, LF) in the
  validator, lowering, and Machine IR validation; another Unicode space
  such as U+00A0 no longer separates items. `<script src>` with a tab
  written as `&#x9;` is a space (`xsd:anyURI`), not a control character
  (`SCXML051`).
- Child Machine IR from `compile-invocation` that does not decode or
  validate (a browser page's `compileInvocation` answer, for one) fails
  that `<invoke>` with `error.execution` (SCXML 3.12.2) in both hosts; it
  ended the invoking session with `invoke-ir-invalid`.
- A chain of invoked sessions thousands deep under the Wasmtime host no
  longer fails on Linux with "Cannot allocate memory": every linear memory
  is one read-write mapping of exactly its size (no guard region, growth
  reservation, or copy-on-write image), so live sessions add no memory
  mapping instead of three or four for each memory (`vm.max_map_count` is
  65,530 by default) and cost about 8.6 MiB of address space each, and compiled
  code checks memory bounds (measured in Docker, linux/amd64, by
  `a_live_waiting_session_adds_few_memory_mappings`, which prints them).
  A memory that grows is copied to its new place by the host, and one
  host call may copy at most its fuel's worth of bytes
  (`MEMORY_GROWTH_COPY_BYTES_PER_FUEL`), an allowance kept apart from its
  Wasm fuel, on every build; a copy past it ends the call as fuel
  exhaustion does. An expression growing its memory 60 KB at a time copied
  about 34 GB (about 3.3 s) before it hit the 64 MiB store bound and now
  stops after 0.99 GB on a billion fuel, while a 40,000-object script and
  a 4,000-state inline `<invoke>` child run as before
  (`memory_growth_copies_are_bounded_by_the_calls_copy_allowance`,
  `a_large_inline_child_starts_on_the_default_fuel`, release, Apple M3
  Max). The run's compiler store for `<invoke>` children takes its copies
  from the allowance too. The store `component_store` returns now carries
  a call hook and a limiter that cap each outermost call at the configured
  fuel and its copy allowance; an embedder must not replace either. `ComponentDataModel::memory_growth_copied_bytes`
  reports the copies, and `ComponentDataModel::last_call_fuel` reports the
  Wasm fuel and the copied bytes of the last call apart (`CallFuel`).
- `SCXML081` and Machine IR validation bound the `done.state.<ID>` event
  of exactly the states the engine raises it for (SCXML 1.0 Appendix D): a
  `<parallel>` completes only through a `<state>` region with a `<final>`
  child, so one whose regions are all `<parallel>` no longer has its ID
  limited to 1,013 bytes.
- The browser host's `CallResult` of a command that starts or cancels an
  invoked session reports the whole command: `effectCount` counts the effects
  of every root call the command made, in every piece it ran in, including
  queued root calls it reached whether its own work queued them (an event
  an invoked session sent the session while starting, for one) or an
  earlier command left them at the delivery bound; it counted only the
  last. The host-internal
  `suspended` flag is no longer exposed.
- Decoding untrusted Machine IR (`MachineIr::from_json`, `validate`)
  checks every field the engine turns into a name on every build: state
  IDs, `<raise>`, `<send>`, and `<donedata>` event names, `<send>` and
  `<invoke>` IDs, and the `done.state.<ID>` and `done.invoke.<ID>` events
  a state that can complete or an invocation raises. One that is empty,
  contains white space, or exceeds 1,024 bytes is refused with the new
  `LowerError::InvalidName`, instead of failing the session when the
  machine reaches it.
- A delayed send to `#_internal` raises `error.execution` and delivers
  nothing (it was delivered at once); one whose target session finished
  before its delay elapsed raises `error.communication` (it was dropped);
  a finished invoked session is no longer listed as a live invocation
  (WIT `route-due` `drop` is `undeliverable`).
- `<invoke>` parameters set only the child's top-level `<datamodel>`
  `<data>`, whose own `expr`, `src`, and children are then not evaluated,
  and a child with no data model no longer fails the invocation.
- `_event.sendid` is set only when the author gave `id` or `idlocation`
  (an immediate send with `idlocation` now carries it, and a delayed send
  with neither no longer leaks its clock id); `_event.data` is `undefined`
  for an event without a payload.
- These raise `error.execution` instead of ending the session: a `<data>`
  value the data model cannot hold (the data stays empty), a non-string
  `typeexpr`, and inline `<send>` or `<donedata>` content the data model
  cannot represent; BasicHTTP `<content>` with element children is sent as
  its serialized XML.
- A null data-model condition over its 16 KiB byte bound is false and
  raises `error.execution` (SCXML 5.9), as in the ECMAScript and XPath data
  models; it failed the session with `payload-too-large`. So does every
  other null data-model evaluation over its bound (a `<log>`, `<send>`,
  `<param>`, `<foreach>`, or `<assign>` expression or location over 16 KiB,
  a `<script>` over 64 KiB): the null data model answers `invalid-input`
  (WIT `evaluate-condition`, `assign`), and the block ends with
  `error.execution`.
- A failing `<donedata>` `<param>` raises `error.execution` and only its
  name and value are ignored (SCXML 5.7.1); the other params still make
  the done event's data, and with none left the event has no data.
- Writing a system variable, or any member reachable from `_event` or
  `_ioprocessors`, raises `error.execution` in every mode (it was a silent
  no-op), and `var` or `function` redeclarations of one fail.
- A `<history>` without an `id` loads (lowering generates `$history<n>`); an
  empty XPath node-set is a `<foreach>` collection that iterates zero times;
  inline `<invoke><content>` keeps extension namespaces, so an
  executable-content extension inside an invoked child runs.
- Hostile input no longer exhausts a native build: attribute defaults
  built from nested empty entities, or empty defaults supplied to many
  elements, are bounded; exponential local-function call chains end the
  side-effect analysis with one undecidable `SCXML075`; quadratic graph
  checks end with `SCXML028`; and a chain of invocations of any depth no
  longer nests native or JavaScript calls (see Changed), so a document
  that invokes itself without end runs until the caller's fuel or command
  budget ends it instead of overflowing the stack.
- The browser async bridge always wakes the worker: a reply that cannot be
  encoded is `platform`, one too large for the channel `payload-too-large`,
  and a reply that cannot be written loses the session instead of blocking
  it. An inbound BasicHTTP message the listener already answered `503` is
  no longer delivered as an event by `scx run`.
- The browser host forwards `release-foreach` to its data model untraced,
  as the Wasmtime host does, instead of opening a capability span for it
  after every command.
- The browser host carries out each command's work without scanning every
  invoked session: an invoked session's next deadline is read only after a
  command ran on it, its clock is moved to the shared clock only before it
  is used, a new session finds its place among its siblings by binary
  search, and an external `receiveHttp` request finds an invoked session by
  its location, read when the session starts (and again before a lookup,
  at most once a turn, when the page's `http.location` threw; each
  session's failure is one failed `capability` span, and a loopback read
  of the sending or invoking session's location that throws is recorded
  the same way instead of being ignored). An invocation request a machine call made without
  suspending for it is discarded with the child it built, and a second one
  while one is pending is `host-invariant`, as under Wasmtime.
- `scx run` with `--http-listen` checks the listener at least every
  250 ms while a timer is pending, so a panicked listener thread fails the
  run promptly. A BasicHTTP listener backing off after an `accept` failure
  stops within one accept poll, and every connection it drops after
  accepting it records an `input-rejected` / `platform` span; a connection
  it serves on a thread it could not keep (a poisoned thread list) records
  a `platform` span with no event.
- The Wasmtime loopback matches a BasicHTTP target to a session's location
  in their canonical form (the WHATWG URL serialization, as the browser
  host does and as an HTTP request to the target reaches it): a default
  port, a letter-case difference in the scheme or host, or a dot segment
  reached the session in the browser but raised `error.communication`
  under Wasmtime.

### Known limitations

- The checked-in IRP baselines pass 201 of 201 applicable cases for the
  ECMAScript data model (436 is not applicable to it) and 178 of 178 for the
  XPath data model (5 under documented upstream errata; 24 cases that never
  exercise the XPath data model are not applicable to it).
- Extension components run only under the Wasmtime session
  host; the browser host answers executable-content extensions from the
  page's `executeExtension` callback.
- `event:schema` references only files relative to the document: no
  repository-namespace resource or other absolute URI. Child documents the
  Wasmtime host compiles when invoked, and documents the browser host
  compiles, get no schema files (inline schemas and `event:schemaexpr`
  only). A session snapshot never evaluates `event:schemaexpr`, and lists
  schemas on transitions only.
- See "Known limitations (pending)" in docs/spec-errata.md: the ECMAScript
  engine gaps (run-time `<log>` side effects, reflective deletion of a
  system variable), a BasicHTTP location no external entity can use
  without an embedder-provided address, the wasm resource bounds that end
  a session or change how an ECMAScript program that reaches them runs,
  and (L.6) every document and `<invoke>` a bound refuses.

## [0.1.0] - 2026-09-28

First public source release of the `0.1.0` workspace. This entry describes the
initial release as a whole, so it lists no changes against earlier, unreleased
development states. No crate is published to
crates.io; the project is distributed as source and as Wasm components built
from this workspace. This release is experimental and is **not** a complete
SCXML implementation.

### Added

- Versioned WIT contracts (`stateforward:scxml`, `scxml-clock`,
  `scxml-basichttp`, `scxml-validator`, `scxml-reporter`, and
  `scxml-compiler`, all `@0.1.0`) and Wasm component guests for the machine,
  logical clock, ECMAScript/null/XPath data models, SCXML and BasicHTTP event
  I/O processors (BasicHTTP behind an embedder-supplied `http-transport`
  import), invoked service, validator, compiler,
  rustc-style and SARIF reporters, and an example extension component.
  `data-model-api` `evaluate-condition`, `evaluate-value`, and `execute` take
  `option<event>`; `none` leaves `_event` unbound (SCXML 1.0 section 5.10), so
  `<onentry>` executed by `start` evaluates on the component data model.
  `data-model-api.assign` and `machine-capabilities.assign` take an
  `assign-source` (an `expr` the data model evaluates and assigns in one
  call, a materialized value for an existing location, or a `<foreach>`
  `foreach-value` whose variable the data model declares), an
  `assign-placement` (XPath data model `<assign type>` and `attr`, SCXML 1.0
  CR 2014-03-13 §C.3.6), and the current `option<event>`; the
  ECMAScript and null data models answer every placement but
  `replace-children` with `execution`, and the XPath data model applies all
  of them.
- The component contracts for running SCXML sessions entirely through
  components, all at `@0.1.0`:
  - `data-model-api`: `initialize` takes the document's `<data>`
    declarations and the system values (the provider binds placeholders, and
    the XPath provider binds the W3C XPath template processor name in
    `_ioprocessors`), plus `declare`, `initialize-data`, `set-parameter`,
    `evaluate-location`, `evaluate-text`, `evaluate-log`,
    `evaluate-form-value`, `foreach-items`, `namelist-key`, `event-payload`,
    `inline-content`, and `external-data`.
  - `machine-capabilities`: `admit-command` and `observe`,
    `update-active-configuration` (the configuration `In()` observes, sent
    as the states exited and entered since the last update, so one
    microstep costs work linear in its effects plus the configuration
    size), forwarding of every data-model function, `load-document`, `classify-target`, `send`
    (answering a `send-disposition`), `send-basichttp`, `log` (the rendered
    text), `compile-invocation`, `start-invocation`, `cancel-invocation`, and
    `forward-to-invocation`. `initialize-data-model` takes the declarations
    and the machine-supplied `_name` (`machine-name`: the document's `name`,
    or `"scxml"`), because only the machine holds its IR.
  - `machine-api` sessions: `start`, `start-invoked`, `send`,
    `deliver-from-invocation`, `resume`, `cancel`, `configuration`, `status`,
    `failure`, and `shutdown`; the step that finishes
    an invoked session carries its `<donedata>` as `return-data` and its
    finishing batch's W3C carrier as `done-trace-context`, which the host
    passes back on `invocation-message.done` (`invocation-done`) so
    `done.invoke` names the child's operation. Every event carries the
    carrier of the batch span that emitted it on every route (see
    CONTRIBUTING.md, "Trace propagation through events"). `session.send`
    rejects an invoke id or a non-`external` event type with `invalid-input`.
  - `io-processor-api` (the SCXML Event I/O Processor): `dispatch`
    (validation, `origin`/`origin-type`/`raw` stamping, target
    classification, and scheduling), `aliases`, `session-location`,
    `classify-target`, `route-due`, `cancel`, and `drain-due`. A
    `send-request` carries its `send-message`.
  - The clock provides `next-deadline`, and the BasicHTTP processor provides
    `aliases`.

  Session snapshots are read-only introspection (there is no save, restore,
  or resume of session state; SCXML defines none): `machine-api`
  `session.snapshot`, `data-model-api.snapshot`, `clock-api.snapshot`, and
  `io-processor-api.snapshot` return structured records, and the
  `session-snapshot` world exports `encode` and `diff` of the canonical
  document `scxml.session-snapshot/v1` (diff `scxml.session-snapshot-diff/v1`,
  JSON Schemas in `crates/scxml-snapshot/schema/`). All four exports, the
  codec, and both hosts' gathering are implemented: read-only snapshots are
  taken at quiescent checkpoints only, and nothing is ever restored.
  The delayed-send wire records are version `7` (with the message). A
  function a component or host cannot serve answers a typed `unavailable`;
  see the README for the current capability set.
- Source-preserving SCXML syntax layer, bounded XSD 1.0 validation, and a
  semantic validator with stable diagnostic codes `SCXML001`–`SCXML045` and
  `SCXML050`–`SCXML069`. The rules cover structure and references; bounded
  state-graph analysis such as unreachable states, shadowed transitions, and
  eventless loops; data-model expression and location checks; `<script>`
  source rules; event-less SCXML `<send>`; conflicting `<data>`/`<content>`
  value sources; ECMAScript assignment-in-condition lint; `<raise>` or
  `<send>` inside `<finalize>` (`SCXML055`); and `In()` arguments that name
  no state, name a history, or are unquoted (`SCXML058`). The generated table
  in [docs/validator-rules.md](docs/validator-rules.md) is the authoritative
  list. Expression syntax is checked by the linked data model's checker: the
  ECMAScript checker, or the XPath checker, which also resolves `$name`
  references against the machine's declarations (`SCXML056`) and lints
  relative paths that select nothing under `<datamodel>` (`SCXML057`),
  positional predicates such as `[0]` that no node satisfies (`SCXML059`),
  and `+` on a string, which XPath evaluates as numeric addition
  (`SCXML060`). Under `datamodel="ecmascript"`, a `<data id>` the data
  model cannot declare (`retry-count`, a leading `_`, or a reserved word) is
  an error without a checker, with a rename of the declaration and every
  reference (`SCXML061`). A Unicode bidirectional control character that
  changes how the source is displayed is a warning (`SCXML062`, Trojan
  Source), as is an ECMAScript condition that is an arrow function and so
  always true (`SCXML063`). A `<send>`, `<invoke>`, or `<state>` that repeats
  a child it allows once (`SCXML064`), an `<assign>` whose value does not fit
  its `type` (`SCXML065`), an empty required expression attribute
  (`SCXML066`), an `<scxml>` or `<parallel>` with no child states
  (`SCXML067`), an attribute or element from another namespace that lowering
  cannot place (`SCXML068`), and a document that exceeds a Machine IR bound
  (`SCXML069`) are errors, so `validate` reports every structure `compile`
  rejects.
  Diagnostics carry exact byte spans and rustc/clippy-style fixes and are
  emitted as one canonical `scxml.report/v2` artifact. The validator runs
  zero-config (XML, the bundled official W3C SCXML 1.0 XSD, and semantic
  rules by default), explains any code with `scx --explain CODE`, and
  emits `--format json` (canonical report) or `--format sarif` for agent
  repair loops. `SCXML040` (an expression over the data model's size or
  nesting bound) is an error, because the data model applies the same bound
  at run time, so `compile` refuses such a document.
- Namespace package manifests that bind exact namespace URI, behavior artifact,
  schema digest, and WIT identity, failing closed on mismatch.
- A compiler that lowers SCXML into deterministic, versioned Machine IR
  (schema version `4`; IR of any other version fails as
  `unsupported-version`), covering atomic, compound, parallel, and final
  states, shallow and deep history, and ordered executable content.
- A deterministic engine executing that IR: hierarchy entry/exit, history,
  parallel regions, eventless and event-triggered transitions with guards,
  `log`, `raise`, `assign`, `script`, `if`/`elseif`/`else`, `foreach`, `send`,
  and `cancel`. `Engine::run` is the single
  execution entry point, running inside the machine component: it
  runs each command to quiescence (at most 1024 engine commands per call) and
  owns block-scoped `error.execution` recovery. `scxml_engine::interpret`
  executes every effect over a `SessionCapabilities` trait that mirrors the
  WIT `machine-capabilities` one-to-one: early and late data binding,
  `<data src>`, `<send>` (including BasicHTTP), `<cancel sendid|sendidexpr>`,
  `<donedata>` and invoked-session return data, and `<invoke>` (child
  invocation, cancel on exit, autoforward; a nested invocation fails
  closed). The machine component runs it over its WIT imports.
- An ECMAScript data model built on Boa with explicit structural bounds, early
  and late data binding, and a deny-by-default virtualized Wasm artifact. Boa
  receives an injected clock fixed at the Unix epoch, and the build composes a
  declarative constant `wasi:random/insecure-seed` provider for the one seed
  read that keys Boa's hash tables, so no ambient clock or randomness is
  granted. Each instance keeps one guarded Boa realm mirroring its committed
  state and reuses it while that is provable (a condition, value, or log
  expression, or an assignment or bare-variable script, that runs no user
  code and writes only its target), rebuilding it after anything else, so
  every operation observes exactly what a freshly built realm presents; a
  kept-realm operation costs about 0.1 to 1.8 million fuel instead of the
  realm build's about 82 million.
- An XPath data model (SCXML 1.0 Candidate Recommendation of 2014-03-13,
  Appendix C.3; the final Recommendation omits the XPath data model) over the pure, fuel-bounded
  `scxml-xpath` XPath 1.0 evaluator: one bounded XML data tree, `$name`
  variables, `In()`, and every `<assign type>` placement, as a native crate and
  a deny-by-default virtualized Wasm artifact.
- An optional Wasmtime host that authenticates component manifests by SHA-256
  and runs components under fuel and memory limits. Its session host
  (`SessionHost`) runs a root session and the child sessions it invokes
  entirely through the shipped components (machine, data models, SCXML and
  BasicHTTP I/O processors, clock, and the compiler for child documents),
  ordered by the pure `scxml-scheduler` crate. It compiles each component on
  first use and shares compiled components across hosts and threads by
  authenticated SHA-256, so constructing a host for a null-data-model session
  compiles only the machine, SCXML I/O processor, clock, and null data model.
  Each host call runs on one fuel budget (`DEFAULT_FUEL`, 2 000 000 000
  instructions, about 110 to 160 ms of Wasm on an Apple M3 Max) shared by every
  component the call reaches, including its data-model, I/O processor, clock, BasicHTTP, compiler,
  script, and child-machine calls; logical runaways stop earlier, at the
  machine's 1 024-command per-call bound.
- An official W3C SCXML IRP conformance pipeline: explicit sandboxed corpus
  preparation, the exact `Outcome` log oracle, stage-aware dispositions,
  deterministic snapshots, and baseline regression checks. The pinned
  2015-03-10 corpus is vendored with its W3C license notice, together with
  checked-in measured baselines for the ECMAScript and XPath data models.
  Prepared corpora use manifest schema `5` and conformance reports use payload
  version `8`: checked-in, digest-bound IRP errata (`conformance prepare
  --errata`) correct documented upstream W3C template and `.txml` defects
  (XPath cases 241, 244, 307, 456, 520, 534), bind the upstream and corrected
  output SHA-256 values, and fail closed on drift; the errata identity is part
  of the corpus and report identities, every pass under an erratum is reported
  and counted as such, and cases the W3C declares not applicable to a data
  model (XPath 302, 303, 304) are reported as `NotApplicable` outside the
  applicable denominator. Of the nine cases the W3C manifest marks manual,
  178, 230, 250, 301, 307, and 415 are decided by a documented
  manual-equivalent oracle table, case 513 by a manual-equivalent oracle
  implemented by the external-client harness, and 313 and 314 by the
  `Outcome` oracle.
- Components-only conformance execution: the runner compiles every case
  and applies every oracle, and executes it only through a `CaseExecutor`,
  the shipped components under the Wasmtime session host
  (`scxml_conformance::WasmExecutor`, `wasmtime` feature); the crate links no
  in-process SCXML execution. `scx conformance run` requires
  `--components DIR --components-manifest-sha256 HEX` (a build without the
  `wasmtime` feature fails closed before any case runs), and
  `--canonical-baseline` compares a run with a checked-in canonical baseline
  on every case field (`scxml_conformance::compare_to_baseline`). The
  `irp-components` CI job reproduces both checked-in baselines byte for byte.
- OpenTelemetry instrumentation through a bounded-cardinality facade.
- The session snapshot codec: `scxml-snapshot` encodes a checkpoint capture
  as the canonical `scxml.session-snapshot/v1` document, decodes it with every
  check (size, JSON, version, shape, canonical bytes, digest, bounds, names,
  consistency), diffs two documents as `scxml.session-snapshot-diff/v1`, and
  renders both for people. The optional `scxml-snapshot-codec` component
  exports the same `encode` and `diff`. `scx snapshot show`, `check`
  (`SNAP-<code>` diagnostics with a pointer, explained by
  `--explain SNAP-<code>`), and `diff` read documents, escaping control and
  bidi characters in human output. `scx snapshot run` executes a
  document through the shipped components under Wasmtime (the `wasmtime`
  feature) and writes the final checkpoint document, or the `--at`/`--every`
  checkpoints to `--output-dir DIR`; without the feature it fails with
  `[wasmtime-disabled]`. `conformance run --snapshot-every-checkpoint`
  proves over both IRP corpora that collecting a document at every
  checkpoint never perturbs the run, that every document checks clean, and
  that two runs produce identical documents. A variable too long to show is
  reported as `{"type":"oversized","bytes":N}`. See
  [docs/session-snapshot.md](docs/session-snapshot.md).
- One generic machine component that loads Machine IR through the
  `stateforward:scxml/ir-source` import (bounded, version-checked, and
  validated, failing closed as a typed failed session).
- Codegen-free `component-wasm` packaging (`scxml-packager`, the compiler
  component, and `scx compile --artifact component-wasm`): a data-only
  `ir-source` provider composed with a pinned prebuilt generic machine
  template, with template and IR digests in the package recipe. Browsers use
  runtime `ir-source` mode only.
- A browser host (`hosts/scxml-host-browser`) that runs machines in
  runtime-IR mode in Node and headless Chromium. It transpiles nine guests
  once with a pinned jco and fuel-meters every core module: the generic
  machine, the compiler, the ECMAScript, XPath, and null data models, the
  SCXML and BasicHTTP Event I/O Processors, the clock, and the optional
  session snapshot codec (loaded on first use by `MachineSession.snapshot` and
  `diffSnapshots`). Each session links its own I/O
  processor and clock, so `<send>` (immediate and delayed, on browser timers
  or manually advanced logical time), `<cancel>`, and cross-session delivery
  in the Wasmtime scheduler's order run as they do under Wasmtime.
  `<invoke>` runs child sessions without the page doing anything: by
  default (`invoke.compile: 'auto'`) a dedicated compiler component
  instance per session compiles each child inside `compile-invocation`,
  with the Wasmtime host's request and outcome mapping (a rejected child is
  error.execution, a compiler fault fails the session), per-compile fuel
  (`invoke.fuel`), a 4 MiB source bound, and a per-session
  `(sourceName, source)` cache; `createSession` pre-warms the root IR's
  statically known inline `<content>` children before `start`. A page
  `compileInvocation` (pre-lowered child IR) overrides it, and
  `invoke.compile: 'provided'` turns it off. URLs load only through the
  page: the `createSession({resources: {allow, resolve}})` option prefetches
  every static `<data src>`, unpinned `<script src>`, and `<invoke src>` URL
  the page's `allow` accepts before the machine runs (deny-all by default;
  the Wasmtime host's bounds and error codes), and the synchronous
  `loadDocument` provider answers the rest. BasicHTTP runs through the same
  component: `<send>` reaches the page's `http.send` (default `fetch`, no
  credentials, no redirects) only for URLs `http.allow` accepts (deny-all by
  default; everything else raises `error.communication`), a fire-and-forget
  send that fails later delivers `error.communication` then,
  `session.receiveHttp` injects inbound requests the component decodes, and
  `_ioprocessors` lists BasicHTTP only with `http`, at `http.location`.
  `resources.allow`, `http.allow`, and what they admit to `resolve`, `send`,
  or the default `fetch` all see an `http:` or `https:` URL in its
  canonical form (`new URL(url).href`), so `..`, `%2e%2e`, and backslash
  segments cannot step outside an allowed path; one that carries userinfo
  is denied without reaching `allow`. Each host call runs on one fuel budget
  (`DEFAULT_FUEL`, 50 000 000 charges of function entries and loop
  iterations, about 0.2 s of Wasm) shared by every component the call reaches,
  including its data-model, I/O processor, clock, BasicHTTP, compiler, and
  child-machine calls, each also capped by its own per-call fuel, as under the
  Wasmtime host. It emits OpenTelemetry through the OpenTelemetry JS API
  (`@opentelemetry/api`, API only) and propagates guest W3C trace context.
  Its IRP parity test runs the official cases that the `Outcome` oracle
  decides (195 ECMAScript and 192 XPath cases, loading static `src`
  documents through `resources` and running the 12 BasicHTTP cases of each
  over the in-process loopback) and requires the dispositions and final
  configurations of the checked-in Wasmtime baselines.
- Async browser sessions: with `createSession({ async: 'auto', resources })`,
  the synchronous capability calls can wait on page Promises, so a URL
  computed at run time (`<invoke srcexpr>`) that the page's `loadDocument`
  does not supply loads through `resources.allow` and `resources.resolve`,
  with the prefetch bounds (16384-byte URLs, 64 URLs per session, 16 MiB of
  resolved text).
  In a cross-origin-isolated page the unchanged session runs in a module
  Worker that blocks on a `SharedArrayBuffer` channel (`Atomics.wait`);
  static URLs are prefetched in the page as for a synchronous session and
  handed to the worker. Otherwise the runtime URL fails with the typed
  `unavailable` error, detail `dynamic-url-unavailable`, which becomes
  error.execution. `async: 'off'` is the default and is unchanged.
- Wasm script components for `<script src>` (`stateforward:scxml/script`),
  pinned at compile time or resolved at run time under a digest allowlist.
- `ComponentDataModel` in the Wasmtime host, which links an authenticated
  data-model component as a machine session's data model.
- `ComponentValidator` in the Wasmtime host, which links the validator
  component's parse-only `expression-syntax` import to one authenticated
  data-model component's `check-syntax` (one data model per validator); the
  validator build composes the fixed `wasi:random` provider
  `components/scxml-validator/wasi-random.wat`, beside the ECMAScript data
  model's constant-seed provider.

### Known limitations

- The checked-in IRP baselines pass 202 of 202 applicable cases for the
  ECMAScript data model and 199 of 199 for the XPath data model (6 under
  documented upstream errata; 302, 303, and 304 not applicable to XPath).
  That result is measured through the shipped components under the Wasmtime
  session host; see [docs/conformance.md](docs/conformance.md).
- Only the SCXML and BasicHTTP event I/O processors exist; a send of any other
  type raises `error.execution`, and delayed BasicHTTP delivery is reported as
  `Unsupported`. Conformance runs exercise BasicHTTP over the session
  host's deterministic in-process loopback transport, never a network.
- The simpler Wasmtime proof host answers sends, document loading, and
  child invocations with `unavailable` (an `unavailable` child invocation
  ends the session); the browser host loads URLs only through the
  page-allowlisted `resources` option (a runtime `srcexpr` URL only through
  `loadDocument`, or through `resources` in an `async: 'auto'` session with
  the `worker` mechanism, else `unavailable` with detail
  `dynamic-url-unavailable`),
  sends BasicHTTP only to URLs the page allowlists, and delivers the
  `error.communication` of an asynchronous send failure later as an
  external event; in every host, an invoked child cannot invoke again.
- Browser async sessions need cross-origin isolation (the `worker`
  mechanism) and do not take the `http` option. JSPI is not used in this
  release: the pinned jco makes every machine export async under JSPI,
  which would require a second, async session scheduler. See the browser
  host README, "Async sessions".
- A standalone `core-wasm` phase is never produced. Packaged-component,
  runtime-mode, browser, and component-data-model reproduction of the pinned
  expected trace is proven for one representative machine and event
  sequence, not in general; the browser host is additionally held to the
  Wasmtime IRP baselines only for the cases its parity test runs.
- The canonical release identity is the Linux x86_64 build of the
  `irp-components` CI job. A build on another host reproduces every IRP case
  result but not the identity bytes (component digests, run identity); see
  [docs/conformance.md](docs/conformance.md#canonical-release-identity).
- Boa 0.22 has no allocation hook or interrupt, so ECMAScript evaluation is
  bounded structurally; there is no durable cross-process persistence.

See the [README](README.md#current-limitations) for the complete list.

[Unreleased]: https://github.com/stateforward/scxml.rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/stateforward/scxml.rs/releases/tag/v0.1.0
