# Invoked services

An `<invoke>` whose `type` is neither absent nor the SCXML type (SCXML 6.4)
names an *invoked service*: a leaf invocation with no machine, run by the
Wasm component (WIT world `stateforward:scxml/invoked-service@0.2.0`) the
host binds to that exact type URI. The machine evaluates the `<invoke>` and
hands the service a `service-invocation`; the service `emit`s events to its
invoking session, `complete`s with `done.invoke.<id>` data, asks for `wake`s
on the session's logical clock, receives the events the session sends to
`#_<invokeid>` (or autoforwards), and is `cancel`led when the invoking state
exits first. A service that traps or fails is retired, and its invoking
session receives `error.communication` with the invoke id.

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
| `wasi-permissions` | optional: the WASI interfaces its component may import ([WASI permissions](executable-content-extensions.md#wasi-permissions)) |
| `events` | optional: its event contract ([Event contracts](#event-contracts)) |

Unknown fields and values fail closed when the manifest is read. The CLI
binds a package with `--service-package MANIFEST.json` and authenticates its
component with `--service-component-sha256 URI=HEX`
([cli.md](cli.md#flags-and-exit-status)).

## Loading a service from a repository

An `<invoke type>` can name its service's package in a GitHub repository at a
release tag, exactly like a repository namespace
([executable-content-extensions.md](executable-content-extensions.md#loading-an-element-from-a-repository)),
so a document needs no flag to bind it:

```xml
<invoke type="https://github.com/stateforward/nemobot/services/memory@v0.1.0" id="memory"/>
```

- **The URI form.** Exactly `https://github.com/<owner>/<repo>/services/<name>@<tag>`:
  the literal `services` folder, one name segment (`[A-Za-z0-9._-]`), no
  trailing slash, and the release tag after the last `@`. The manifest's
  `service-uri` must equal it byte for byte, tag included; a new version is a
  new URI. A `services/` URI is never a repository namespace, and a
  `namespaces/` URI never a repository service; any other path is an
  ordinary service type URI.
- **The layout.** The manifest is `services/<name>/package/<name>.service.json`;
  its optional `schema-artifact` and every `events` schema file are plain
  relative paths beside it; the component is the release asset the
  manifest's `service-artifact` names (a plain asset name), listed in the
  release's `SHA256SUMS`.
- **When it loads.** Only a literal `<invoke type>` that no
  `--service-package` binds is resolved (a `typeexpr` is never resolved: it
  runs only against what the run binds). `validate` and `compile` (and the
  compile step of `snapshot run` and `run`) resolve the package: the tag to a
  commit through the GitHub API, then the manifest, its schema, and its event
  schemas at that commit, each checked against the digest the manifest
  declares. `snapshot run` and `run` then resolve the component from the
  release, authenticated by its `SHA256SUMS` entry, and bind it before the
  session starts. Any failure fails closed: validation stops, or the command
  fails before anything runs. The host acquires nothing.
- **The cache, offline, network bounds, and telemetry** are the repository
  namespaces' exactly: the same content-addressed cache and `--cache-dir`,
  the URI pinned on first use (`stateforward.scxml.service-cache/v1`: the
  URI, commit, manifest digest, and, once read, the component digest; the
  pinned manifest pins its schema and event schema digests), `--offline`,
  HTTPS only with loopback HTTP for explicit mirror bases, bounded redirects,
  timeouts, and bodies, no retry, no ambient credentials, and the CLI spans
  `scxml.cli.resolve-service-package` and `scxml.cli.resolve-service-component`
  with closed outcomes only.
- **Importing is consent.** The CLI lists each repository service with its
  release tag and pinned commit (`= version: release <tag> (commit <commit>),
  package <version>`), its declared origins, slots, WASI permissions, and
  events, before anything runs.

## Values are references: `env` slots

A service never receives a secret value. Like an executable-content
extension element ([executable-content-extensions.md](executable-content-extensions.md#values-are-references-env-slots)),
its `<invoke>` names, per slot its service package declares (`env-slots`),
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
  bindings on the invoke (`env`, sorted by slot, omitted when empty), and the
  machine hands them to the host in `service-invocation.env` (names only).
- The host binds each slot whose variable the run holds a value for to a
  fresh opaque reference, numbered per invocation, and hands them to the
  service's `start`. **Unlike an extension element's per-call references, a
  service's references live for the whole invocation**: every later `send`,
  `wake`, and `cancel` of that one instance may use them, only inside its own
  `outbound-http` requests, which the host admits only to an origin the
  package declares or one a reference's value supplies entirely. No other
  instance, invocation, or element ever receives them, and they end when the
  instance is dropped.
- A variable the run holds no value for leaves its slot absent; the service
  decides what that means. A slot the package does not declare, one slot
  bound twice, or a name that is not a variable name rejects the start: the
  `<invoke>` raises `error.execution`.
- Only an invoked service's `<invoke>` has slots: one on an SCXML session's
  `<invoke>` is `SCXML084`, and a `typeexpr` that evaluates to the SCXML type
  with slots bound raises `error.execution`. The validator judges a literal
  type's slots against its bound package (`SCXML083`, with a "did you mean"
  fix); a `typeexpr`'s slots only for their syntax.
- The CLI supplies the run's values to services exactly as to extension
  elements (`--env`, `--env-file`), and their outbound transport only when
  the run is not recorded or replayed. The echo example declares the slot
  `endpoint`: its `fetch` event GETs the URL that slot's value supplies.

## Event contracts

A package may declare the events its service emits and the events it
accepts, each with a JSON Schema for the event's data:

```json
"events": {
  "emits": [
    { "name": "echo.reply", "schema-artifact": "events.json#/$defs/Reply", "schema-sha256": "<64 hex>" }
  ],
  "accepts": [
    { "name": "echo.say", "schema-artifact": "events.json#/$defs/Say", "schema-sha256": "<64 hex>" }
  ]
}
```

- `name` is an exact event name: an SCXML event name of at most 256 bytes
  with no `*` and no empty `.`-separated token. Each list holds at most 64
  entries, strictly sorted by name. A present `events` field declares at
  least one event; a package with none omits it.
- `schema-artifact` names a JSON file relative to the manifest (a relative
  path at or below the manifest's directory, percent-encoded like a URI
  path), optionally with a JSON Pointer fragment; `schema-sha256` is the
  SHA-256 of the file's bytes, so entries naming the same file carry the
  same digest.
- Every command that reads the manifest reads each file (at most 4 MiB),
  checks its digest, and resolves each schema exactly like an
  `event:schema` reference: the same dialects (2020-12 by default, 2019-09,
  draft-07), no fetching, the same JSON Pointer bundling rules, and the
  same canonical JSON ([namespaces/event](../namespaces/event/README.md)).
  Any failure fails the command closed.
- The CLI lists the contract with the package's consent lines:
  `= events: emits echo.reply; accepts echo.say`.

### What the host enforces

The Wasmtime session host compiles a bound service's contract once
(`ServicePackage::events`, `ServiceComponents::bind`; a malformed contract
fails the binding with `ArtifactError::EventContractInvalid`) and checks it
at the service's two external boundaries. A declared contract is closed: an
event it does not list fails like a schema mismatch.

| Boundary | Check | On mismatch |
|---|---|---|
| the service `emit`s an event | the name is in `emits` and its schema admits the data | the emit is invalid: the host retires the service, and the invoking session receives `error.communication` with the invoke id (no `done.invoke`) |
| the invoking session's `<send target="#_<invokeid>">` | the name is in `accepts` and its schema admits the data | the `<send>` raises `error.execution` (a delayed send is checked when it is sent, and again when it falls due, when a mismatch makes it undeliverable: `error.communication` with its send id) |

Event data is checked as JSON: no data and `null` are `null`; numbers,
strings, booleans, arrays, and objects are themselves; XML data is admitted
only by the boolean schema `true`. Autoforwarded events are delivered
unchecked (an autoforward is not a `<send>` and has no failure to raise).
A service without a contract is not checked.

A document's own `event:schema` attributes never take part: they stay
metadata only, and `event_schemas_never_change_execution`
(`hosts/scxml-host-wasmtime/tests/session_snapshot.rs`) still proves that a
machine with them runs exactly as without them.

### What the validator checks

Given the service packages (`--service-package`), the validator
cross-checks every transition in the state that invokes a bound service with
a contract, or in a descendant of that state, that names a declared emitted
event exactly among its descriptors:

- `SCXML095` (error): its `event:schema` is neither the package's schema for
  the event (compared as canonical resolved JSON, so formatting and key order
  do not matter) nor `true`. The `maybe-incorrect` fix replaces it with the
  package's schema.
- `SCXML096` (note): it has no `event:schema`. The `maybe-incorrect` fix
  inserts the package's schema, declaring the `event` namespace on the
  transition when no prefix for it is in scope.

Compatibility is identity, not JSON Schema subsumption. `event:schemaexpr`,
descriptors that only prefix the event name (`svc`, `svc.*`, `*`),
`<invoke typeexpr>`, and services whose package the validator was not given
are not judged. `scxml-cli --explain SCXML095` and `--explain SCXML096` give
each rule with an example.
