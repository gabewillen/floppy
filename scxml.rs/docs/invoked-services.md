# Invoked services

An `<invoke>` whose `type` is neither absent nor the SCXML type (SCXML 6.4)
names an *invoked service*: an external service run by the Wasm component
(WIT world `stateforward:scxml/invoked-service@0.2.0`) the host binds to that
exact type URI. The machine evaluates the `<invoke>` and asks the host to
start it (`machine-capabilities.start-invocation` with
`invocation.service(invoke-request)`); the host instantiates the component
and calls its one export, a WASI 0.3 / Component Model async function:

```wit
world invoked-service {
  import wasi:clocks/monotonic-clock@0.3.0;
  import outbound-http;
  import telemetry;
  export invoked-service-api;
}

interface invoked-service-api {
  invoke: async func(request: invoke-request, events: stream<event>)
    -> result<tuple<stream<event>, future<result<option<data-value>, component-error>>>, component-error>;
}
```

- **`request`** (`invoke-request`) carries the `invoke-id`, the `type-uri`
  exactly as authored or evaluated, the evaluated `source` (`src`/`srcexpr`;
  data for the service, which nothing loads), `content`, `parameters`
  (`<param>` and `namelist`, SCXML 6.4.2), the `<invoke>`'s `env`
  `attributes` (`list<extension-attribute>`), the slot `references` the host
  bound (`list<slot-reference>`; see below), and the `trace-context`.
- **`events`** carries every event the invoking session sends to the
  invocation (`<send target="#_<invokeid>">`) and every event it
  autoforwards (`<invoke autoforward="true">`), in order.
- **The returned stream** carries the events the service sends to its
  invoking session. Each reaches the session's external queue stamped with
  the invoke id (the host sets `invokeid`, sets `origintype` to the invoke
  type, and the event runs the invocation's `<finalize>` first).
- **The returned future** resolves when the service is done: `ok(data)` is
  `done.invoke.<id>` carrying `data`, after every event returned before it;
  `err`, a trap (fuel exhaustion included), or a returned event outside the
  `types.event` bounds is the invocation's `communication-error` message
  (the session raises `error.communication.<id>` with the invoke id, and no
  `done.invoke` follows). Named after the invocation as `done.invoke.<id>`
  is, one invocation's failure is selected by descriptor; a transition on
  `error.communication` or `error.*` still matches it.
- **Rejecting `invoke`** (it returns an error, traps, or the component
  cannot be instantiated) fails the `<invoke>`: the session raises
  `error.execution`.

Invocation is a runtime action (SCXML 6.4), so a type the host does not
bind is judged when its `<invoke>` runs, never earlier: an `<invoke>` of a
type no service component is bound to, literal or given by `typeexpr`,
raises `error.execution` when it runs, and the document still validates
(`PKG004` is a warning), compiles, and runs normally until then.

## Cancellation

When the invoking state exits first, or the invoking session ends, the
invocation is cancelled (SCXML 6.4): the host cancels the `invoke` subtask
and drops the invocation's store, so nothing the service returns afterwards
reaches the session and no `done.invoke` follows. The connection of every
request still in flight is closed at once, so its transport thread ends and
a server streaming its answer sees the connection close (proved within 2 s
by the `service_invoke` and `live_run` tests). A service has nothing to
release.

## Execution and determinism

Each invocation runs on a thread of its own, with its own instance in its
own store, driven by wasmtime's async event loop (`Store::run_concurrent`,
`invoke` called with `call_concurrent`). The session hands each input to
the invocation (its start, an event, its due timers, an HTTP response) with
the session's logical time, and waits until the invocation's event loop has
nothing left to run, so what the service returns is applied at the same
point of every run.

The service's `wasi:clocks/monotonic-clock@0.3.0` is served from the
session's *logical* clock: its marks are logical milliseconds since the
session opened, in nanoseconds, with a resolution of 1 ms. A `wait-for` or
`wait-until` completes when the session's scheduler reaches its deadline, in
order with every delayed `<send>` of the run.

Bounds: between two inputs the invocation runs at most the configured fuel
(`ServiceComponents::with_call_fuel`), and returns at most 1024 events of at
most 4 MiB in all; past either the invocation fails like a trap. At most 16
requests of one invocation are in flight at once; one more is answered
`capacity-exhausted`. These are amplification guards and apply on every
build.

## Outbound HTTP

`outbound-http.fetch` and `outbound-http.send` are `async func`s: a
service's call suspends only the calling task, never the session, and the
service's other tasks (reading `events`, a timer) go on. `fetch` answers
the whole response; `send` (named after `wasi:http`'s `client.send`)
answers once the status has arrived, with the body as a `stream<u8>` and a
future that resolves when the body has ended (`err` when the transport
failed or the body passed its 16 MiB bound), so a service can report
progress while a long response, such as a streamed model completion, is
still arriving. The host substitutes the slot references, admits
the request only to an origin or URL prefix the service package declares
(`http-origins`) or to an origin a reference's value supplies entirely, and
then:

- in a batch run, performs it through the embedder's transport
  (`ServiceComponents::with_outbound_http`);
- in a live session, performs it on a transport thread of its own and
  delivers the response later as an external input
  (`LiveSession::deliver_http_responses`; in `scx run`, a `call` with
  cause `http-response`); a `send`'s status, body bytes, and end are each
  such an input, delivered as they arrive (the body bytes that arrived
  since the last delivery joined into one). A logical-clock live session
  never moves its clock past a request in flight: its next part is the
  next thing that happens. A `send` counts against the 16 requests in
  flight until its body has ended.

Admission, in detail:

- **Substitute, then admit.** The host substitutes every reference with its
  value, checks the request against the WIT bounds (method `GET` or `POST`;
  an absolute `http`/`https` URL without userinfo, at most 16384 bytes; at
  most 64 headers, none of `host`, `content-length`, `transfer-encoding`, or
  `connection`, no CR, LF, or NUL in a value; a body of at most 4 MiB, empty
  for `GET`; at most 64 parts per field), canonicalizes the URL with the
  WHATWG parser, and admits it only to an origin or URL prefix the package
  declares (compared in canonical form, so dot segments cannot escape a
  prefix), or to an origin the run supplied: the URL's first part is a
  reference whose value is itself an absolute `http`/`https` URL without
  userinfo, and the rest of the URL, if any, begins its path, query, or
  fragment. A literal that precedes or extends the authority (`https://` +
  value, or value + `:8080/`) supplies no origin. Every other request is
  refused before the transport sees it, so a value is never substituted
  into a request that leaves for anywhere else.
- **The transport sends.** The `http` feature's `ExtensionHttpClient`
  follows no redirect (a 3xx is returned as the response) and uses no
  cookie, credential, or environment proxy; TLS is rustls against the
  operating system's trust store. Connections are kept alive and pooled
  per origin, so consecutive requests to one origin reuse one connection
  and its TLS session. A response head of at most 16 KiB and a
  response body of at most 16 MiB (`payload-too-large` beyond it) apply on
  every build. A request has no time limit of its own: the invocation bounds
  it, since leaving the invoking state cancels the invocation and closes
  the connection the request holds, new or pooled (SCXML 6.4), so a document's timeout is a `<send delay>`
  armed in the invoking state; an unreachable host fails when the operating
  system gives up connecting.

Without a transport every request fails with `unavailable`. The echo
example's `fetch` event GETs the URL its `endpoint` slot references,
followed by the event's data when it is a string (a path, query, or
fragment), with `authorization: Bearer <token>` when the `token` slot is
bound, and returns `echo.fetched` carrying the status; its `send` event
GETs the same URL through `outbound-http.send` and returns `echo.received`
carrying the byte count of each body read as it arrives, then
`echo.fetched`.

## Record and replay

A service's HTTP responses are recorded inputs: `--record` writes one
`http-response` record per `fetch` response (its time, the invocation's
lineage of invoke ids, the request's number within the invocation, and the
status and body or the transport failure) and one `http-response-part`
record per delivered part of a `send`'s response (the same fields, then its
`status`, its body bytes, its `end`, or the failure that ended it), and
`--replay` delivers each from the log without the network ([live-session.md](live-session.md#replay-log)).
Timers run on the logical clock, so a run with services, including services
that import WASI interfaces, records and replays. What a service reads
through its WASI resources (a preopened directory, a socket) is not
recorded: a replay needs the same ones.

## Service packages

A service package manifest (`stateforward.scxml.service-package/v1`) binds
the exact service type URI to its component, like an extension package
binds a namespace ([executable-content-extensions.md](executable-content-extensions.md)):

| Field | What |
|---|---|
| `format` | `stateforward.scxml.service-package/v1` |
| `service-uri` | the exact absolute type URI an `<invoke type>` names |
| `package-version` | the package version |
| `service-artifact` | the component, relative to the manifest |
| `schema-artifact`, `schema-sha256` | an optional schema and its SHA-256 (both a string or both `null`) |
| `wit-world`, `wit-version` | `stateforward:scxml/invoked-service`, `0.2.0` |
| `http-origins` | the origins or URL prefixes its `outbound-http` requests may reach (binding consents to them) |
| `env-slots` | the `env` slots an `<invoke>` of it may bind |
| `validator-artifact`, `validator-sha256` | optional: a package validator component and its SHA-256 (both present or both absent; [validator-rules.md](validator-rules.md#package-validators)) |

Unknown fields and values fail closed when the manifest is read. The CLI
binds a package with `--service-package MANIFEST.json` and authenticates its
component with `--service-component-sha256 URI=HEX`
([cli.md](cli.md#flags-and-exit-status)); a host binds it with
`ServiceComponents::bind`.

## WASI imports and resources

A manifest declares no WASI. The WASI interfaces a service may use are its
component's own `wasi:*` imports, read when the service is bound. Clocks,
randomness, and the `wasi:io` interfaces need no grant. Only runtime
resources do, named after wasmtime CLI's options and scoped to the
service's exact type URI (`ServiceComponents::with_wasi(uri, WasiOptions {
dirs, inherit_network, allow_ip_name_lookup })`, or the CLI flags):

| Import | Grant | What |
|---|---|---|
| `wasi:filesystem` | `--dir URI=HOST::GUEST` | preopens `HOST` read-write at the absolute guest path `GUEST` (repeatable) |
| `wasi:sockets/tcp` | `--wasi URI=inherit-network` | TCP connections may reach any address (wasmtime's `-S inherit-network`) |
| `wasi:sockets/ip-name-lookup` | `--wasi URI=allow-ip-name-lookup` | name lookup (wasmtime's `-S allow-ip-name-lookup`) |

Everything fails closed when the service is bound, before anything runs:
an import without its grant (`ArtifactError::WasiResourceUngranted`), a
grant whose interface the component does not import (`WasiGrantUndeclared`),
a `wasi:*` import the host never links, such as `wasi:cli` or `wasi:http`
(`UnknownImport`), and a grant outside its bounds
(`WasiGrantInvalid`). Extension components get no I/O at all
([executable-content-extensions.md](executable-content-extensions.md#no-io)).
[`components/scxml-service-files`](../components/scxml-service-files)
imports `wasi:filesystem` and runs only with `--dir URI=HOST::/workspace`.

## Loading a service from a repository

An `<invoke type>` can name its service's package in a GitHub repository at a
release tag, exactly like a repository namespace
([executable-content-extensions.md](executable-content-extensions.md#loading-an-element-from-a-repository)),
so a document needs no flag to bind it:

```xml
<invoke type="https://github.com/acme/widgets/services/store@v0.1.0" id="store"/>
```

- **The URI form.** Exactly `https://github.com/<owner>/<repo>/services/<name>`:
  the literal `services` folder, one name segment (`[A-Za-z0-9._-]`), no
  trailing slash. That tagless URI is the service's identity and names the
  repository's latest release, resolved once per root session and listed
  with the tag and commit it resolved to; a literal `<invoke type>` may pin a
  release with one trailing `@<tag>`, which is split off when the document
  is parsed and read only by the CLI's resolver, exactly as for repository
  namespaces ([executable-content-extensions.md](executable-content-extensions.md#loading-an-element-from-a-repository)):
  the Machine IR, the engine, and the host see the tagless type URI. The
  manifest's `service-uri` names the tagless URI byte for byte (a manifest
  naming a pinned URI is refused). A `services/` URI is never a repository
  namespace, and a `namespaces/` URI never a repository service; any other
  path is an ordinary service type URI.
- **The layout.** The manifest is `services/<name>/package/<name>.service.json`;
  its optional `schema-artifact` is a plain relative path beside it; the
  component is the release asset the manifest's `service-artifact` names (a
  plain asset name), listed in the release's `SHA256SUMS`.
- **When it loads.** Only a literal `<invoke type>` that no
  `--service-package` binds is resolved, in the document or in an
  `<invoke src>` document the run loads (a `typeexpr` is never resolved: it
  runs only against what the run binds). Each resolution is made once per
  root session and its component bound once, shared by every session that
  invokes it, with the repository namespaces' rule for which release a
  document binds: the root's, shared by every tagless use, and an
  `<invoke src>` document's own pin for its own sessions
  (`EmbeddedDocument::services`). `validate` and `compile` (and the
  compile step of `snapshot run` and `run`) resolve the package: the tag (or
  the latest release's tag) to a commit through the GitHub API (both fetched by the embedded GitHub resolver component, as for repository namespaces), then the manifest and its schema at that
  commit, each checked against the digest the manifest declares. `snapshot
  run` and `run` then resolve the component from the release, authenticated
  by its `SHA256SUMS` entry, and bind it before the session starts. Any
  failure fails closed: validation stops, or the command fails before
  anything runs. The host acquires nothing.
- **The cache, offline, network bounds, and telemetry** are the repository
  namespaces' exactly: the same content-addressed cache and `--cache-dir`,
  the URI pinned on first use (`stateforward.scxml.service-cache/v1`: the
  URI, a tagless URI's `tag`, the commit, manifest digest, and, once read, the component digest; the
  pinned manifest pins its schema digest), `--offline`, HTTPS only with
  loopback HTTP for explicit mirror bases, bounded redirects, timeouts, and
  bodies, no retry, no ambient credentials, and the CLI spans
  `scxml.cli.resolve-service-package` and `scxml.cli.resolve-service-component`
  with closed outcomes only.
- **Importing is consent.** The CLI lists each repository service with its
  release tag and pinned commit (`= version: release <tag> (commit <commit>),
  package <version>`), its declared origins and slots, the WASI interfaces
  its component imports (`= WASI imports:`), and the WASI resources the run
  grants it (`= WASI resources:`), before anything runs.

## Values are references: `env` slots

A service never receives a secret value. Its `<invoke>` names, per slot its
service package declares (`env-slots`),
the variable whose value the host binds, in the host's `env` vocabulary
(`https://github.com/stateforward/scxml.rs/namespaces/env`):

```xml
<invoke type="https://stateforward.org/scxml/services/echo" id="echo"
        env:endpoint="ECHO_URL"/>
<invoke typeexpr="serviceType" env:endpointexpr="'ECHO_' + region"/>
```

- `env:<slot>="NAME"` names the variable; `env:<slot>expr="EXPR"` is an
  expression the machine evaluates, with no current event, when the
  `<invoke>` runs (a failure, or a result that is not a string, raises
  `error.execution` and the service never starts). Machine IR carries the
  attributes on the invoke (`attributes`, sorted by local name, omitted when
  empty), and the machine hands them to the host in
  `invoke-request.attributes` (`env:<slot>expr` as `env:<slot>` with its
  value; names only).
- The host binds each slot whose variable the run holds a value for to a
  fresh opaque reference, numbered per invocation, in
  `invoke-request.references`. A service's references live for the whole
  invocation, only inside its own `outbound-http` requests, which the host admits only to an
  origin the package declares or one a reference's value supplies entirely.
  No other invocation or element ever receives them, and they end when the
  invocation's store is dropped.
- Every value is granted to exact package URIs (`EnvVariable`; the CLI
  grants a variable to the service type URI of each `<invoke>` whose `env`
  attribute names it): a variable granted only to other packages is treated
  exactly like one the run holds no value for.
- A variable the run holds no value for leaves its slot absent; the service
  decides what that means. A slot the package does not declare, one slot
  bound twice, or a name that is not a variable name rejects the start: the
  `<invoke>` raises `error.execution`.
- Only an invoked service's `<invoke>` has slots: one on an SCXML session's
  `<invoke>`, or on any element other than an `<invoke>` (an
  executable-content extension element included), is `ENV002`, and a `typeexpr` that evaluates to the SCXML type
  with slots bound raises `error.execution`. The validator judges a literal
  type's slots against its bound package (`ENV001`, with a "did you mean"
  fix); a `typeexpr`'s slots only for their syntax.
- The CLI supplies the run's values with `--env NAME=VALUE`, else the last
  `--env-file PATH` entry (Docker's format: `NAME=VALUE` with the value taken
  literally, a bare `NAME` from the shell, `#` comments, blank lines
  ignored), else, for a bare `--env NAME`, the calling shell's; a variable no
  slot can name is dropped. Nothing by default; there is no
  whole-environment form. Before the run it lists each variable the document
  names and the `<invoke>`s it goes to (names only). Values are masked
  (`[masked]`) in the text the CLI writes (in a live-session record only
  inside its JSON strings, so every record stays one JSON line); canonical snapshot documents are
  written exactly (their `sha256` covers their bytes). The echo example
  declares the slots `endpoint` and `token`.

## Event schemas

A package declares no events. The schemas of the events a service sends
live in the document itself: an `event:schema` on each transition that
handles them ([namespaces/event](../namespaces/event/README.md)). A
service's events are not checked against them (a service is not an SCXML
session; the checks cover the events SCXML sessions create with `<send>`
and `<raise>`, and the host's own input), and transitions match a
service's events by name exactly as without them.

## What the validator checks

- `PKG003` (warning): the literal `<invoke type>` is a platform-specific
  value, not a URI. SCXML 6.4.1 allows platform-specific types, but a
  service package binds by an exact absolute URI, so on a host built from
  this repository the `<invoke>` raises `error.execution` when it runs.
- `PKG004` (warning): no given service package binds the literal type
  URI; without a bound package the `<invoke>` raises `error.execution` when
  it runs (invocation is a runtime action).
- `ENV001` and `ENV002`: `env` slots (above).

`compile` never rejects an unbound type. `scx --explain PKG003` and
`--explain PKG004` give each rule with an example.
