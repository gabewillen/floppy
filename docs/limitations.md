# Status and limitations

This page is the complete, evidence-backed statement of what the
implementation covers and where it stops. The [README](../README.md#current-limitations)
summarises it. Resource bounds and implementation-defined choices are in
[limits.md](limits.md); places where the official IRP and the SCXML 1.0 text
disagree are in [spec-errata.md](spec-errata.md).

## Conformance evidence

- The exact prepared 2015-03-10 ECMAScript and XPath runs each select all 202
  starts with the empty/default filter identity: every selected case runs
  always, including manual cases, and nothing is skipped. With runner `0.2.0`
  and Machine IR schema `5`, ECMAScript reports 201/201 applicable cases
  Passed (167/167 mandatory, 34/34 optional), with 436 (null profile,
  `datamodel="null"`) `NotApplicable`; XPath reports 178/178 Passed
  (163/163 mandatory, 15/15 optional), of which 5 under documented errata
  (241, 244, 307, 520, 534), with 24 `NotApplicable` cases that never
  exercise the XPath data model: 301, 302, 303, and 304 (W3C: scripting
  only), 436 (null profile, `datamodel="null"`), and 444–446, 448, 449, 451–453, 456, 457, 459, 460, 557, 558, 560–562, 569, and 578 (ECMAScript
  profile, `datamodel="ecmascript"`). Neither suite has a `Failed`, `Unsupported`,
  `Invalid`, or `HarnessError` case. The ECMAScript evidence binds corpus
  digest
  `9f005f20c9cbcd7188507833d8fd8a71158cc49cf70e9428fd3fc807e11396ca`,
  errata SHA-256
  `54c7f1cb44686e2bf499a0edf2596d311f4623245b2fe380abf0ac5b4663303f`,
  producer report digest
  `1975a1a0467b6e0a4a80befe2a5b9902169e85c018a4d22768534dd27a54d682`,
  and canonical artifact SHA-256
  `6c8be254619f14afc8193355259c8759f15d4bf2016a46dbd4e0a36be2a6eeee`.
  The XPath evidence binds corpus digest
  `a661ef4af8bc62fc064cea5e137a6b469ede4b7da80ececf932d66d37cc93bca`,
  errata SHA-256
  `72fe9e3ca418f55dadf64d2d6331af724cf8ebbc2a87a0180489db818aa73f2a`,
  producer report digest
  `c07da9831092692099d7c46696945feab67fe99044ca7cec9df8356db56276e1`,
  and canonical artifact SHA-256
  `3451259154ec3784b5c229fafcbeef77c70418b0fea088be6fd2136933d6d8a7`.
  The report bytes are checked in under
  [`tests/conformance/w3c-irp/baselines`](../tests/conformance/w3c-irp/baselines).
  These are measured IRP results of the shipped components under the
  Wasmtime session host (runner `scxml-conformance/wasm`, engine
  `scxml-machine`, each identity bound to the exact release component
  bytes), not broad conformance claims for every host. The canonical
  release is the Linux x86_64 build of the `irp-components` CI job: a
  build on another host reproduces every case result but not the identity
  bytes (see
  [conformance.md](conformance.md#canonical-release-identity)).
- The pinned W3C manifest marks nine IRP cases manual: mandatory 178, 230,
  250, 301, 307, 313, 314, and 415, and optional 513. All of them always run.
  313 and 314 log an `Outcome` and keep the `Outcome` oracle. The six
  mandatory cases 178, 230, 250, 301, 307, and 415 cannot log an `Outcome` of
  `pass` and are decided by the documented manual-equivalent oracle table
  (`ManualOracle::ALL` in `crates/scxml-conformance/src/manual_oracle.rs`,
  see [conformance.md](conformance.md#manual-equivalent-oracle)).
  IRP case 513 (a W3C instruction for an external HTTP client, not a
  machine) is decided by a manual-equivalent oracle implemented by the
  external-client harness. All seven are recorded as
  `oracle: manual-equivalent` and pass in both suites (XPath 307 under its
  documented erratum).

## What the implementation covers

- Machine IR lowering covers atomic, compound, parallel, and final states,
  shallow and deep history pseudostates, `<initial>` elements and `initial`
  attributes, eventless and event-triggered transitions with guards,
  targetless transitions, and multi-target transitions and initial
  transitions whose targets lie in distinct regions of one parallel (other
  target sets are rejected by Machine IR validation). The engine executes that
  hierarchy (entry/exit order, history recording and restoration, parallel
  regions entered through several explicit targets, and `done.state.*`
  events) deterministically (`crates/scxml-engine/tests/multi_target.rs`).
  Macrostep and microstep semantics are measured by the official IRP
  ([Conformance evidence](#conformance-evidence)); no claim is made beyond
  that evidence.
- The machine component embeds no tables: it loads compiler-produced IR
  through `ir-source`. `scripts/verify-wasm-capabilities.sh` packages the
  representative fixture `tests/fixtures/representative-machine.scxml`
  (compound, parallel, deep history, internal raise, ECMAScript `In()`
  conditions and log expressions) through the CLI, checks byte determinism,
  `wasm-tools validate`, and the import/export allowlist, and the Wasmtime host
  tests execute those exact bytes (packaged mode) and the generic machine with
  the same IR (runtime mode), require both modes to commit identical call results,
  and require the representative session's per-command status,
  configuration, ordered log records, and ordered data-model calls to equal
  the checked-in `tests/fixtures/representative-machine.trace.json`
  (`machine_ir_modes`). Hostile runtime IR (oversized,
  wrong version, malformed, semantically invalid, unsupported) fails closed
  with typed codes. This proves one representative machine and event
  sequence; it is not a general equivalence claim.
- Executable content lowered into Machine IR and executed by the engine covers
  `log`, `raise`, `assign`, inline and external (`src`) `script`,
  `if`/`elseif`/`else`, `foreach`, `send`, and `cancel`. Failing executable
  content raises `error.execution` and aborts the rest of its own block only;
  later `<onentry>`, `<onexit>`, and transition blocks of the same engine command still
  run. The engine decides this in `Engine::run`, the one execution entry
  point, which runs inside the machine component, together with
  running each command to a stable configuration (at most 1024 engine commands per
  call). In IRP runs the machine component evaluates expressions through the
  suite's ECMAScript, XPath, or null data-model component, and the runner
  applies the exact `Outcome` oracle; final-state arrival is never
  substituted for the oracle.
- Child SCXML invocations (`<invoke>`) run in the machine component through
  its invoked-session capability: the engine plans invoke, cancel,
  `<finalize>` (an empty one updates each `namelist` and `<param location>`
  location from the child's event data, SCXML 6.5.2), and autoforward, and a host that links the capability (the
  Wasmtime session host) owns the invoked sessions, `done.invoke`, and
  cross-session routing, ordered by the pure `scxml-scheduler` crate.
  Delayed sends run on the `scxml-ioprocessor-scxml` and `scxml-clock`
  components. See [conformance.md](conformance.md#component-execution) for exactly
  who does what.
- Invoked services (`<invoke type>` neither absent nor the SCXML type) run in
  `stateforward:scxml/invoked-service` Wasm components the Wasmtime session
  host binds to exact type URIs (`SessionHost::with_service_components`;
  the CLI's `--service-package`, `--service-component`, and
  `--service-component-sha256`): one instance per invocation, a leaf with no
  machine, under the per-call fuel and call budget. The engine evaluates the
  request (`src`/`srcexpr` text is data, never loaded), the host runs `start`,
  delivers `#_<invokeid>` and autoforwarded events to `send`, runs `wake` on
  the shared logical clock, and calls `cancel` when the invoking state exits;
  `emit`, `complete` (`done.invoke` with data), and retirement (`failed`, an
  `error.communication` with the invoke id) reuse the invocation routing of
  invoked sessions. Proven by
  `hosts/scxml-host-wasmtime/tests/service_invoke.rs` with the echo and timer
  example (`components/scxml-service-example`) and by the engine tests in
  `crates/scxml-engine/tests/interpret.rs`. Gaps of this first slice: a
  literal type no host binds fails the session start closed and an unbound
  `typeexpr` raises `error.execution`, types are bound only by exact
  URI, and a repository service URI (`.../services/<name>@<tag>`) is
  resolved only for a literal `<invoke type>` (a `typeexpr` naming one is not
  resolved, and no shipped component answers a repository service URI, so
  `crates/scxml-cli/tests/repository_services.rs` proves resolution,
  pinning, offline use, and authentication up to the host's `type-uri`
  check); a session snapshot of a run
  holding a live invoked service is refused (`unavailable`;
  `scxml.session-snapshot/v2` cannot represent one); child
  documents compiled at run time by the compiler component are not checked
  for unbound service types (`COMP004`), which then fail their session start
  instead; the validator and compiler components' WIT requests carry no
  service packages, so the validator component reports every literal
  service type as unbound (`SCXML094`, a warning); and the browser host binds no service (`load-services` and
  `start-service` answer `unavailable`).
- Package event contracts: a service or extension package may declare the
  events its component emits and accepts with JSON Schemas (`events`). The
  Wasmtime host enforces a service's contract on every `emit` (a mismatch
  retires the service: `error.communication`) and every `<send>` to it (a
  mismatch raises `error.execution`; a delayed send is checked when sent and
  when due), and the validator checks a document's `event:schema` against it
  (`SCXML095`, `SCXML096`). Proven by the `service_invoke` contract tests and
  the validator's and CLI's service event tests. Gaps: autoforwarded events
  are not checked; an extension package's contract is declared, verified,
  and listed but not enforced (nothing checks the events an extension
  element raises or sends); schema compatibility is exact identity of
  canonical schemas, not subsumption; the validator does not check
  `<send>` data to a service against `accepts`; and the browser host binds
  no service.
- WASI permissions: a service or extension package may declare the WASI 0.2
  interfaces its component imports (`wasi-permissions`: `wasi:clocks`,
  `wasi:filesystem` with a named mount and access, `wasi:random`,
  `wasi:sockets/ip-name-lookup`, `wasi:sockets/tcp`; unknown entries fail
  closed). The Wasmtime host links into a service component exactly the
  declared permissions the embedder grants (`ServiceComponents::with_wasi_grants`;
  the CLI's `--grant URI=PERMISSION[=VALUE]`) and fails the binding or the
  session start closed otherwise. Proven by the `service_invoke` tests with
  `components/scxml-service-files` (a granted read-write mount is read and
  written, a read-only mount refuses the write, an ungranted or undeclared
  permission and an undeclared WASI import fail closed). Gaps: extension
  components get no WASI interface (the host refuses an extension package
  that declares one: its store data is not `Send`, which `wasmtime-wasi`
  requires); `wasi:cli`, `wasi:http`, and UDP are not grantable; individual
  WASI calls get no span of their own (the service's export-call spans cover
  them); a run binding a package that declares any permission cannot be
  recorded or replayed; and the browser host links no WASI interface.
  The SCXML and `BasicHTTP` Event I/O processors are supported; no other
  Event I/O processor exists, so a `<send>` of any other type raises
  `error.execution` (SCXML 6.2, IRP case 199). `BasicHTTP`
  sends run through the shipped `scxml-ioprocessor-basichttp` component,
  over the session host's deterministic in-process loopback `http-transport` (no
  socket, thread, or wall clock; locations live under the reserved
  `http://basichttp.loopback.invalid` authority) in every batch and IRP run;
  a live session with a `LiveHttp` network also posts to allowlisted hosts
  and receives POSTs at `<public base>/sessions/<_sessionid>/basichttp`,
  proven over real local sockets by
  `hosts/scxml-host-wasmtime/tests/basichttp_network.rs` and the two-process
  test in `crates/scxml-cli/tests/live_run.rs`. A delayed `BasicHTTP` send
  is checked by the `BasicHTTP` processor at the `<send>`, held on the
  session clock by the SCXML Event I/O Processor (`schedule-basichttp`), so
  `<cancel>` cancels it and a terminated session discards it, and posted
  only when its delay elapses (SCXML 6.2); one that cannot be dispatched
  then raises `error.communication` with its send id
  (`hosts/scxml-host-wasmtime/tests/session_host.rs`,
  `hosts/scxml-host-wasmtime/tests/basichttp_network.rs`, and
  `hosts/scxml-host-browser/test/basichttp.test.mjs`). The Wasmtime host also links
  the `BasicHTTP` component standalone to an embedder-supplied
  `BasicHttpTransport`; the machine component's `send` capability goes to
  whatever the host links (the simpler proof host answers `unavailable`),
  and the browser host links the component to the page's allowlisted
  `http` capability (deny-all by default).
  IRP case 513 is decided by a documented manual-equivalent oracle
  implemented by the external-client harness, not by an `Outcome` log from
  W3C corpus bytes; see
  [conformance.md](conformance.md#basichttp-and-case-513).
- The machine component binds data itself (early and late binding,
  `<data expr>`, `<data src>` through the host's `load-document`, whose
  answer carries the media type the source indicated: under ECMAScript a
  JSON or XML indication is tried first (SCXML B.2.1), and any other type,
  `text/plain` included, leaves the content to B.2.1's own rules),
  evaluates `<donedata>` payloads (a top-level `<final>`'s `<donedata>` is
  the invoked session's return data for `done.invoke`, W3C Appendix D
  `returnDoneEvent`), executes `<cancel sendidexpr>`, and iterates
  `<foreach>` arrays of every data model through `foreach-items`. Each of
  these needs the matching host capability; the simpler proof host links
  none of document loading, sends, or invocations, and the browser host
  sends BasicHTTP only to URLs the page's `http` option allowlists and loads
  URLs only through the page's allowlisted `resources` option or
  `loadDocument` provider.
- `<assign>` and `<foreach>` `item`/`index` semantics have one definition per
  data model, executed by the data-model components;
  `hosts/scxml-host-wasmtime/tests/data_model_semantics.rs` runs official
  W3C cases 150, 151, and 152 (ECMAScript) and assignment probes through the
  Wasmtime machine and data-model components.
- The complete official IRP inputs and unmodified W3C ECMAScript/XPath templates
  are bundled with license and digest provenance, as is the complete official
  W3C SCXML 1.0 XSD. Templates for other data models are not bundled.

## Hosts

- The Wasmtime session host (`SessionHost`,
  `hosts/scxml-host-wasmtime/src/session_host`) implements every
  `machine-capabilities` call by linking the shipped components: the data model
  the document names, the SCXML Event I/O Processor and clock, the `BasicHTTP`
  processor over an in-process loopback transport, and the compiler component
  for invoked documents. Each invoked session is its own machine session with
  its own internal and external event queues, and may invoke in turn. The
  embedder may set `InvokeLimits` (`SessionHost::with_invoke_limits`:
  `max_invoke_depth` and `max_invoked_sessions` under one root session); the
  default sets none, and the host has no ceiling of its own. An invocation
  beyond limits the embedder set fails as that `<invoke>`'s `error.execution`,
  as in the browser host. One cooperative executor (`scxml-scheduler`) runs the
  root session and every session it invoked, for batch and live runs alike. Live
  sessions (`SessionHost::open`, `scxml-cli run`) report a finished root's
  completion with the evaluated `<donedata>` of its top-level final state
  (`SessionOutput::Done`, the CLI's `done` record; SCXML 5.5: the processor
  "SHOULD return the data to the environment in an implementation-dependent
  manner"), and reach the network only through a configured `LiveHttp`
  (`scxml-cli run --http-allow`/`--http-listen`), which `--record` and
  `--replay` cover; outbound requests carry no W3C trace headers, and the built-in
  listener frames bodies by `Content-Length` only (no chunked requests, one
  request per connection); the CLI accepts no trace carrier on its inputs and
  handles signals on Unix only. The simpler proof host's session driver
  (`execute_machine_session`) links a data model (a native provider, or an
  authenticated data-model component through `ComponentDataModel`) and records
  logs, but answers `raise`, `send`, `cancel`, and extension-namespace
  capability calls with `unavailable`; extension namespaces execute under the
  session host (`SessionHost::with_extension_components`, proven by
  `hosts/scxml-host-wasmtime/tests/executable_content_extensions.rs` and
  `crates/scxml-cli/tests/extension_packages.rs`).
  The compiler component's packaging path is built and validated for
  `wasm32-wasip2` but is exercised end to end only through the native CLI and
  library; under Wasmtime the compiler component only compiles invoked child
  documents. Packaged components are accepted by the Wasmtime host only up to
  its pre-compilation ceiling of 2 GiB − 1 bytes, GitHub's per-file
  release-asset limit (components ship as release assets).
- The browser host ([its README](../hosts/scxml-host-browser/README.md))
  runs the generic machine in runtime-IR mode, reproduces the representative
  trace in Node and headless Chromium, bundles the machine, compiler, the
  three data models, the SCXML and BasicHTTP Event I/O Processors, the
  clock, and the session snapshot codec, runs delayed sends and `<invoke>` (children compiled in the host,
  or page-supplied pre-lowered child IR), and bounds every component call
  with build-time metered fuel.
  BasicHTTP reaches only URLs the page's `http` option allowlists, and URLs
  load only through the page's allowlisted `resources` option or
  `loadDocument` provider; a runtime `srcexpr` URL loads through `resources`
  only in a cross-origin-isolated `async: 'auto'` session (the `worker`
  mechanism), and otherwise needs `loadDocument`. Async sessions do not take
  the `http` option, so they have no BasicHTTP.

## Known gaps

In short:

- No Event I/O Processor other than SCXML and BasicHTTP exists; a `<send>`
  of any other type raises `error.execution`.
- XPath IRP cases 241, 244, 307, 520, and 534 pass only under the documented
  upstream errata in `tests/conformance/w3c-irp/errata/xpath.json`. 24 XPath
  cases that never exercise the XPath data model are `NotApplicable` to it
  per the W3C (301-304 scripting only; 436 fixes `datamodel="null"`; the 19
  B.2 ECMAScript-profile cases fix `datamodel="ecmascript"`), and ECMAScript
  case 436 is `NotApplicable` to the ECMAScript corpus for the same reason
  (`tests/conformance/w3c-irp/errata/ecmascript.json`).
- The IRP results are measured through the shipped components under the
  Wasmtime session host, the only conformance execution path.
- Boa 0.22 has no allocation hook or interrupt, so ECMAScript evaluation is
  bounded structurally (with Wasm memory and fuel, or only process limits
  natively, as the outer bound).
- There is no durable cross-process persistence.
- The compiler component's packaging path is not exercised under Wasmtime.
- The proof hosts answer `raise`, `send`, `cancel`, and extension-namespace
  capability calls with `unavailable` (the machine's own `<raise>` is
  engine-internal).
- The browser host loads URLs only through the page (its allowlisted
  `resources` option or `loadDocument`; a runtime `srcexpr` URL through
  `resources` only in a cross-origin-isolated `async: 'auto'` worker
  session), and async sessions do not take the `http` option.

In detail:

- Extension namespace elements execute through `executable-content-extension`
  components only under the Wasmtime session host. The browser host still
  answers `execute-extension` from the page's `executeExtension` callback
  (`handled` or `not-handled`, with no session capabilities and no queued
  events) and instantiates no extension component; it loads no repository
  element, offers no `env` slot references, and without that callback a
  document that uses an extension namespace fails its session start. The
  standalone proof host answers `unavailable`. Extension component instance
  state is not part of session snapshots, and the CLI's `conformance run`
  binds no extension packages.
- Repository namespaces are GitHub repositories only. A child document an
  `<invoke src>` loads at run time compiles against the namespaces already
  bound, so a repository namespace only such a child uses is not resolved for
  it (its invocation fails with `error.execution`); inline `<content>`
  children use the namespaces their parent loads. A slot named by an
  `env:<slot>expr` expression is evaluated per execution, so the CLI's element
  listing cannot say whether the run holds its value.
- An `event:schema` ([namespaces/event](../namespaces/event/README.md))
  references only files relative to the document, never a
  repository-namespace resource or another absolute URI. A child document
  the Wasmtime host compiles when it is invoked, and every document the
  browser host compiles, get no schema files, so a schema-file reference
  rejects it; inline schemas and `event:schemaexpr` work everywhere. A
  session snapshot lists schemas on transitions only and never evaluates
  `event:schemaexpr`. An integer outside the 64-bit range in a schema is
  read as the nearest 64-bit float, without a diagnostic. Compiling a
  schema compiles every `pattern` it holds as a regular expression; the
  work of compiling a pathological pattern is bounded only by the schema's
  size (and, in the components, fuel), not by a pattern-specific bound.
- Extension components' `outbound-http` requests block the executing session
  until the response or the transport's timeout (10 s connect, 120 s total);
  responses are delivered whole (no streaming), are not recorded in `run`
  replay logs (so a recorded or replayed `run` offers extension components no
  outbound HTTP: every request is refused the same way in both), and carry no
  W3C trace headers. The `openai` package
  does not implement agentml-go's `stream`, `retry`, or template prompts
  ([namespaces/openai/README.md](../namespaces/openai/README.md)); its
  function-calling mode and snapshot system message need the Wasmtime
  host's `session-snapshot-host`, which the browser host does not offer.
- The BasicHTTP network of live sessions (`LiveHttp`, `scxml-cli run
  --http-allow`/`--http-listen`): outbound requests carry no W3C trace headers (so `_event.raw` at a
  receiver never depends on telemetry) and their response bodies are never
  read; the built-in listener accepts only `Content-Length`-framed bodies
  (`Transfer-Encoding` is answered 400) and one request per connection, and
  serves plain HTTP only (TLS terminates at a proxy or tunnel named by
  `--http-public-url`); a late `error.communication` carries no data, as in
  the browser host (its `_event.sendid` names the failed `<send>`).
- There is no save or restore of session state, in process or durable;
  SCXML defines no persistence.
- Boa has no allocation hook or interrupt; ECMAScript evaluation is bounded
  structurally, with Wasm memory and fuel as the outer bound (only process
  limits in native builds); see
  [limits.md](limits.md#the-boa-bounding-gap).
