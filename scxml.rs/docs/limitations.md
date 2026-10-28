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
  `cd7e244f59f8527bd8c20dd5dd448c4fe8e012fa1c0764a8462df5dfc658199e`,
  and canonical artifact SHA-256
  `b0f115c839fae914d372fbf37e094a634471f6b54783c4bfa2239be0fc11a6b7`.
  The XPath evidence binds corpus digest
  `a661ef4af8bc62fc064cea5e137a6b469ede4b7da80ececf932d66d37cc93bca`,
  errata SHA-256
  `72fe9e3ca418f55dadf64d2d6331af724cf8ebbc2a87a0180489db818aa73f2a`,
  producer report digest
  `3462de7f450565500d06f180dcf3123784cc5a8300fc42f684130b3a7a00d5a1`,
  and canonical artifact SHA-256
  `fc1d9b0e40a7b8b703d4e75d7825425a24d84e42cf3f0b475a9a5d3b169fd8cd`.
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
  representative fixture `tests/fixtures/representative-machine.scx`
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
  `if`/`elseif`/`else`, `foreach`, `send`, and `cancel`, plus the `event`
  vocabulary's `<event:defer/>`. Failing executable
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
  An invoked session whose call stops at the 1024-command bound is ended as
  a cancelled invocation is (its `cancel` runs, cancelling the sessions it
  invoked; nothing more it sends reaches its invoking session, and no
  `done.invoke` follows): a start that stopped there raises
  `error.execution` for its `<invoke>`, and a later call
  `error.communication.<invokeid>`, in the invoking session, which runs on
  (proven by `hosts/scxml-host-wasmtime/tests/nested_invocation.rs` and the
  `invoked-runaway` host-parity case); only the root's own stop, and any
  other failure or trap of a session, fails the run.
  Delayed sends run on the `scxml-ioprocessor-scxml` and `scxml-clock`
  components. See [conformance.md](conformance.md#component-execution) for exactly
  who does what.
- Invoked services (`<invoke type>` neither absent nor the SCXML type) run in
  `stateforward:scxml/invoked-service` Wasm components the Wasmtime session
  host binds to exact type URIs (`SessionHost::with_service_components`;
  the CLI's `--service-package`, `--service-component`, and
  `--service-component-sha256`): one instance per invocation, in its own
  store on its own thread, under the per-input fuel and call budget. The
  engine evaluates the request (`src`/`srcexpr` text is data, never loaded),
  and the host calls the WASI 0.3 async export `invoked-service-api.invoke`
  with an `events` stream fed by `#_<invokeid>` sends and autoforwarded
  events; the events the service writes to its returned stream, and its
  future (`ok`: `done.invoke` with data; `err` or a trap: the
  `communication-error` message, `error.communication.<invokeid>` with the invoke id),
  reuse the invocation routing of invoked sessions. Its
  `wasi:clocks/monotonic-clock` runs on the session's logical clock, and
  leaving the invoking state cancels the invocation and drops its store
  (SCXML 6.4). Proven by
  `hosts/scxml-host-wasmtime/tests/service_invoke.rs` with the echo and timer
  example (`components/scxml-service-example`) and by the engine tests in
  `crates/scxml-engine/tests/interpret.rs`. Gaps: types are bound only by
  exact URI (an `<invoke>` of a type no host binds, literal or `typeexpr`,
  raises `error.execution` when it runs, as SCXML 6.4 makes invocation a
  runtime action; the session start is unaffected), and a repository service
  URI (`.../services/<name>[@<tag>]`) is resolved only for a literal
  `<invoke type>` of the document or of an `<invoke src>` document the run
  loads (a `typeexpr` naming one is not resolved, and no shipped
  component answers a repository service URI, so
  `crates/scxml-cli/tests/repository_services.rs` proves resolution,
  pinning, offline use, and authentication up to binding); a session
  snapshot of a run holding a live invoked service is refused
  (`unavailable`; `scxml.session-snapshot/v2` cannot represent one); the
  validator and compiler components' WIT requests carry no service packages,
  so the validator component reports every literal service type as unbound
  (`PKG004`, a warning); and the browser host runs only the service
  components its build ships, and only where JavaScript Promise
  Integration exists (otherwise `start-invocation` of a service answers
  `unavailable`), with no `wasi:sockets`, and what a service returns
  reaches the session in a later event-loop turn rather than at a
  deterministic point of the run.
- Invoked services' outbound HTTP: `outbound-http.fetch` and
  `outbound-http.send` are `async func`s that suspend only the calling
  task. A batch run performs the request through the embedder's transport
  (`ServiceComponents::with_outbound_http`); a live session performs it on
  a transport thread of its own and delivers the response as a recorded
  external input (`http-response`), or a `send`'s status, body bytes, and
  end each as one as they arrive (`http-response-part`), which a replay
  delivers from its log. Proven by the `service_invoke` tests
  (`a_fetch_suspends_only_its_task_and_its_response_arrives_as_an_external_input`,
  `leaving_the_invoking_state_drops_a_request_in_flight`,
  `a_recorded_http_response_replays_without_the_network`,
  `a_streamed_response_reaches_the_service_part_by_part_and_replays`,
  `a_streamed_response_that_fails_midway_fails_the_invocation`,
  `a_streamed_response_arrives_while_the_server_still_writes`, and
  `cancelling_the_invoking_state_closes_the_streaming_connection_and_ends_its_thread`,
  which proves the connection closes and its thread ends within 2 s) and
  the CLI's `live_run` service fetch tests, against loopback servers. Gaps:
  a batch run delivers a `send`'s body whole, after the request ended (it
  performs each request at once); a cancelled request still
  resolving or connecting ends only when the operating system answers or
  gives up (nothing is sent); and the browser host offers none. A request
  has no time limit of its own: the invocation's lifetime bounds it
  (`a_slow_streaming_response_is_read_to_its_end`).
- Invoked services' outbound WebSocket: `outbound-websocket.connect`
  suspends only the calling task; a live session runs the connection on a
  thread of its own and delivers its opening, messages, and end as recorded
  external inputs (`websocket` records without `processor`), which a replay
  delivers from its log. Proven by
  `hosts/scxml-host-wasmtime/tests/websocket_processor.rs`
  (`an_invoked_service_talks_over_a_websocket_with_a_referenced_token_and_replays`,
  `a_service_connection_outside_its_package_or_without_a_transport_fails_the_invocation`)
  against a loopback server. Gaps: a batch run and the browser host refuse
  every connection (`unavailable`); and a service's connections are
  counted under the processors' `processor-websocket-connect` and
  `processor-websocket` spans. Admission, including a reference that
  supplies the origin, is proven by the host's unit test
  `a_websocket_url_is_admitted_by_its_http_form_or_by_the_origin_a_reference_supplies`.
- WASI for invoked services: the WASI interfaces a service may use are its
  component's own `wasi:*` imports; no manifest declares them. Only runtime
  resources need a grant, named after wasmtime CLI's options and scoped to
  the service's exact type URI (`ServiceComponents::with_wasi`; the CLI's
  `--dir URI=HOST::GUEST`, `--wasi URI=inherit-network`, and
  `--wasi URI=allow-ip-name-lookup`); an import without its grant, a grant
  without its import, or a `wasi:*` import the host never links fails the
  binding closed. Proven by the `service_invoke` tests with
  `components/scxml-service-files`
  (`a_preopened_directory_is_what_the_filesystem_import_reads_and_writes`,
  `an_ungranted_or_unused_wasi_resource_fails_the_binding_closed`) and the
  CLI's `a_wasi_service_runs_only_with_its_dir_granted`. By rule, not as a
  gap, extension components get no WASI interface (extension elements are
  synchronous executable content, and I/O-bound work belongs in invoked
  services). Gaps: `wasi:cli`, `wasi:http`, and UDP are never linked; TCP is
  all or nothing (`inherit-network`); individual WASI calls get no span of
  their own (the invocation's spans cover them); a replay log does not
  capture what a service reads through its WASI resources (files, sockets),
  so a replay needs the same ones; and the browser host links no WASI
  interface.
- The SCXML and `BasicHTTP` Event I/O processors are built in. Any other
  `<send>` type goes to the host, and the Wasmtime session host sends it to
  the Event I/O Processor component a package binds to that type
  ([event-io-processors.md](event-io-processors.md)); a type no package
  binds, and every such type in the browser host and in batch and IRP runs,
  raises `error.execution` (SCXML 6.2, IRP case 199). The shipped WebSocket
  processor is proven against a loopback server, live and replayed, by
  `hosts/scxml-host-wasmtime/tests/websocket_processor.rs` and
  `crates/scxml-cli/tests/live_run.rs`. Only `scx run` binds processor
  packages; `_ioprocessors` lists no bound processor; a document cannot bind
  one itself; a document closes a WebSocket connection with code 1000 only;
  and a session snapshot does not include processor instances or their
  connections. `BasicHTTP`
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
  session clock by the SCXML Event I/O Processor (`schedule-processor`), so
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
  sessions (`SessionHost::open`, `scx run`) report a finished root's
  completion with the evaluated `<donedata>` of its top-level final state
  (`SessionOutput::Done`, the CLI's `done` record; SCXML 5.5: the processor
  "SHOULD return the data to the environment in an implementation-dependent
  manner"), and reach the network only through a configured `LiveHttp`
  (`scx run --http-allow`/`--http-listen`), which `--record` and
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
  clock, the session snapshot codec, the validator, and the extension
  components of the example namespace and `scx`, validates documents
  (the `scx validate` report, byte for byte),
  runs extension elements through bound extension components with the
  Wasmtime host's semantics (session capabilities during `execute`,
  per-session instances, per-call fuel, retirement after a trap, the
  executable-content snapshot), takes `event:schema` files from the page,
  runs delayed sends and `<invoke>` (children compiled in the host,
  or page-supplied pre-lowered child IR), and bounds every component call
  with build-time metered fuel. A shared corpus
  (`tests/fixtures/host-parity`) and the `env` and `event` namespace
  suites hold it to the Wasmtime host's outcomes in Node and headless
  Chromium.
  BasicHTTP reaches only URLs the page's `http` option allowlists, and URLs
  load only through the page's allowlisted `resources` option or
  `loadDocument` provider; a runtime `srcexpr` URL loads through `resources`
  only in a cross-origin-isolated `async: 'auto'` session (the `worker`
  mechanism), and otherwise needs `loadDocument`. Async sessions do not take
  the `http` option, so they have no BasicHTTP.

## Known gaps

In short:

- Only the Wasmtime session host and `scx run` bind Event I/O Processor
  components; elsewhere a `<send>` whose type is neither SCXML nor BasicHTTP
  raises `error.execution`.
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
- Data-model packages run under the Wasmtime session host (every session
  whose `datamodel` names a bound package) and, in the browser host, only as
  the root session's page-supplied provider of a component the package
  ships transpiled; the browser host resolves no package at run time, and an
  invoked child there whose `datamodel` names a package gets no data model.
  `scx validate` does not link a package's checker, so its expressions get
  the `LINT010` note.
- `xml:base` is resolved within one document: an inline `<content>`
  machine starts from its own root, not from the bases of the elements
  around it.
- The browser host loads URLs only through the page (its allowlisted
  `resources` option or `loadDocument`; a runtime `srcexpr` URL through
  `resources` only in a cross-origin-isolated `async: 'auto'` worker
  session), and async sessions do not take the `http` option.

In detail:

- Extension namespace elements execute through `executable-content-extension`
  components under the Wasmtime session host and the browser host. The
  browser host runs only the extension components it ships transpiled and
  fuel-metered at build time (the example namespace and `scx`): a component
  resolved at run time would need a browser build of its own, so it loads
  no repository element (nor repository service: it runs the GitHub
  resolver component `scx` embeds nowhere, a page cannot read `github.com`
  release downloads across origins, and a page's `fetch` cannot hand a
  redirect to the component for the host to admit; see the browser host's
  README). Its async sessions (`async: 'auto'`) take no extension
  components. Its fuel counts function entries and loop iterations, not
  instructions, so a fuel bound stops a spinning element at a different
  point than under Wasmtime (the outcome, `error.execution` and a retired
  instance, is the same). The standalone proof host answers `unavailable`. Extension component instance
  state is not part of session snapshots, and the CLI's `conformance run`
  binds no extension packages.
- Repository namespaces are GitHub repositories only. A release's manifest
  names its package's tagless URI; a pin (`@<tag>`) is split off when the
  document is parsed and is resolution input only. The
  repository namespaces and services of every literal `<invoke src>`
  document are resolved with the root's before the session starts; a child
  document an `<invoke srcexpr>` loads, or a `<content expr>` child, compiles
  against what is already bound, so a repository namespace or service only
  such a child uses is not resolved for it (its invocation fails with
  `error.execution`), and an `<invoke srcexpr>` document that pins a release
  (`@<tag>`) does not load (`error.execution`), since its pin was never
  resolved. The documents a run loads are fetched again by
  a `--replay`, not read from its log. A slot named by an
  `env:<slot>expr` expression is evaluated per execution, so the CLI's element
  listing cannot say whether the run holds its value.
- An `event:schema` ([namespaces/event](../namespaces/event/README.md))
  references only files relative to the document, never a
  repository-namespace resource or another absolute URI. An
  `<invoke src>` child (literal or `srcexpr`, `file:` or `https:`) gets the
  files it references, resolved against its own base URI
  (`EmbeddedDocument::event_schemas`): a literal child's before the
  session, a `srcexpr` child's each time its `<invoke>` runs, as they are
  then, like the `srcexpr` document itself and the scripts its
  `<script src>` names (`SessionEmbedder::scripts`). An inline `<content>`
  child, and every document the browser host compiles
  without the page supplying them, get no schema files, so a schema-file
  reference rejects it; a browser page supplies a
  document's schema files to the validator and compiler
  (`eventSchemas`), and inline schemas and `event:schemaexpr` work
  everywhere. `EVENT011` judges only the invoked documents its caller
  supplies: `scx validate` and `compile` read `file:` ones and fetch no
  remote `src`, a `srcexpr` document is never judged, and a browser page
  supplies them (`invokedDocuments`). A
  session snapshot lists schemas on transitions only and never evaluates
  `event:schemaexpr`. An integer outside the 64-bit range in a schema is
  read as the nearest 64-bit float, without a diagnostic. Compiling a
  schema compiles every `pattern` it holds as a regular expression; the
  work of compiling a pathological pattern is bounded only by the schema's
  size (and, in the components, fuel), not by a pattern-specific bound.
- The events `<event:defer/>` holds
  ([namespaces/event](../namespaces/event/README.md#deferred-events)) are
  not in a session snapshot (`scxml.session-snapshot/v2` lists the internal
  and external queues only), and the `event` vocabulary's implementation
  report plan has no statement for `<event:defer/>`:
  `crates/scxml-engine/tests/interpret.rs` and the validator's
  `event_attributes` tests cover it.
- Run-time `event:schema` contracts
  ([namespaces/event](../namespaces/event/README.md#at-run-time)) are not
  checked for an `<invoke autoforward>` copy, the events an invoked
  service returns, a `BasicHTTP` `<send>`, donedata and done events, or
  `event:schemaexpr`. XML data (the XPath data model's) satisfies only the
  schema `true`.
- A data-model call that exhausts its per-call fuel
  (`DEFAULT_DATA_MODEL_CALL_FUEL`) or otherwise traps ends the session
  (`machine-platform`), not `error.execution`: Wasmtime leaves a trapped
  instance unusable, so the session's data model is gone. The ECMAScript
  data model bounds loops (`MAX_LOOP_ITERATIONS`) and built-in results, but
  not the callbacks a native built-in makes (for example `Array.from` with
  a map function over tens of thousands of items), which can reach the fuel
  cap first.
- By rule, not as a gap, extension components do no I/O: no WASI, no
  `outbound-http`, and no `env` values or references (an `env` attribute on
  an extension element is `ENV002`). Network calls and secrets belong in
  invoked services, whose `fetch` suspends only their own task.
- The conformance runner binds no invoked services yet: a namespace suite
  cannot exercise one at run time, so the `env` suite
  ([namespaces/env/irp](../namespaces/env/irp/README.md)) holds only its
  document-rejection statements, and the run-time `env` statements are
  proven by the echo service's host and CLI tests instead.
- The BasicHTTP network of live sessions (`LiveHttp`, `scx run
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
