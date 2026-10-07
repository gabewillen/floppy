# Decisions

Decisions the maintainer made for the mosfet to nemobot port. Each is settled;
reopening one needs the maintainer. All are dated 2026-10-06.

## scxml.rs subtree

| Field | Value |
| --- | --- |
| Prefix | `scxml.rs/` |
| Kind | squashed `git subtree` |
| Source repository | local scxml.rs (`stateforward/scxml.rs`) |
| Source branch | `feat/invoke-services` |
| Source commit | `50f6e592` ("Split a repository package's @<tag> off at parsing: the tag pins, the tagless URI is the namespace"), on top of `f4aec81b` (the rustdoc link to the deleted `WasiExtensionUnsupported`), `73f340b7` ("Regenerate the IRP baselines from the canonical components"), on top of `a5a67c4f` (tagless repository URIs resolved once per run; URI loading by SCXML defaults; `--resource` removed), `2c7de995`, `74020230` (the validator's draft profile), `4da603c2`, `dc452d9a`, `7c6714a8`, `e06b67e4`, `50283481` (the embedded `scx` namespace), `f4b0bcab`, `a82be21d`, `a5e8fdbe`, `44a8d754` (embedded components), `a6451e08`, `191fd8f7` (document env scopes), `1b47a2a0`, `99ee70bb` (`.scx`), and `281f56ef` (the `scx` binary), then `aa12fa6d8` ("Document the WASI 0.3 invoked-service cutover") |
| Squash commits here | `353ce1a` ("Squashed 'scxml.rs/' content from commit 9971c05"), then `5be41b24` ("Squashed 'scxml.rs/' changes from 9971c05a..fe7d555d"), then `070aa258` ("Squashed 'scxml.rs/' changes from fe7d555d..dd18cd11"), then `0a20ad2b` ("Squashed 'scxml.rs/' changes from dd18cd11..2bf0dd55"), then `bf96cbf9` ("Squashed 'scxml.rs/' changes from 2bf0dd55..ebc051ae"), then `a2aaf0da` ("Squashed 'scxml.rs/' changes from ebc051ae1..5eb077c4b"), then `56069d18` ("Squashed 'scxml.rs/' changes from 5eb077c4b..14c4b8e97"), then `5dd298d9` ("Squashed 'scxml.rs/' changes from 14c4b8e97..e7d2fd7a8"), then `91f3feed7` ("Squashed 'scxml.rs/' changes from e7d2fd7a8..f5bf2b674"), then `1c0991c20` ("Squashed 'scxml.rs/' changes from f5bf2b674..aa12fa6d8"), then `f591e5b4b` (aa12fa6d8..f4b0bcabc), `d224eaf14` (f4b0bcabc..502834818), `e1459aad1` (502834818..4da603c23), `4b1a17145` (4da603c23..a5a67c4f1), `7f9512730` (a5a67c4f1..73f340b77), and `5a7f0e930` (73f340b77..50f6e5922) |
| Update | `git subtree pull --prefix scxml.rs <scxml.rs> feat/invoke-services --squash`, after the change lands upstream |

## Decisions

| # | Date | Decision |
| --- | --- | --- |
| D1 | 2026-10-06 | mosfet (a Python bot runtime: hsm state machines plus Starlark behaviors) is ported to nemobot, powered by scxml.rs (the Rust/Wasm SCXML engine). |
| D2 | 2026-10-06 | scxml.rs gains custom `<invoke type="<service URI>">` services: Wasm components of the `invoked-service` world. Slice 1 is done on `feat/invoke-services` (commits `c0cbd9b5`, `6470c5b5`): the `service-invocation` WIT, `invoked-service-host` (`emit`, `complete`, `set-wakeup`, `now-ms`), `load-services`/`start-service`, `invocation-message.failed`, the engine branch, IR `invoke_types`, `SCXML093`/`SCXML094`/`COMP004`, the `stateforward.scxml.service-package/v1` manifest, the host's `service.rs`, the CLI's `--service-package`/`--service-component`/`--service-component-sha256`, the echo example `components/scxml-service-example`, and removal of the `scxml-invoked-service` scaffold. Slice 2 (WASI permissions, grants, event contracts, repository services) followed on the same branch. |
| D3 | 2026-10-06 | Versions stay at WIT `@0.2.0` and Machine IR `5` (0.2.0 is unreleased; repository practice). No bump. |
| D4 | 2026-10-06 | Wasm components (extensions and services) may declare any WASI permission in their manifest; the user accepts it at use time (the CLI lists it before running; an ungranted permission fails closed before the first macrostep). scxml.rs's "no WASI" rule becomes "no WASI interface beyond what the manifest declares and the run grants". Packages declaring effectful permissions refuse `--record`/`--replay`, following the outbound-HTTP precedent. |
| D5 | 2026-10-06 | Event contracts use scxml.rs's existing `event:schema` vocabulary, which stays metadata-only in the engine. Enforcement comes from the `decision` namespace (candidates are the enabled transitions with their `event_schema`, read via `session-snapshot-host`; rank; fill data to the schema; validate before raising) and from package-declared schemas checked at external boundaries (service emit). |
| D6 | 2026-10-06 | mosfet's attachment protocol is not ported: invoke lifecycle covers attach and detach. Devices become services (later). Domain event names (`bot.*`, `bot.ability.*`, `phone.*`) stay byte-for-byte from mosfet. |
| D7 | 2026-10-06 | nemobot is a git repository and Cargo workspace. scxml.rs is a squashed git subtree at `nemobot/scxml.rs` carrying `feat/invoke-services`. Every canonical namespace and service lives as a crate in nemobot: `namespaces/{scxml,generate,decision}`, `services/{memory,speech}` (later), `host/`, `bots/builder/`. Repository URIs: `https://github.com/stateforward/nemobot/namespaces/<name>@<tag>` and `https://github.com/stateforward/nemobot/services/<name>@<tag>`. |
| D8 | 2026-10-06 | The `scxml` namespace is named exactly `scxml` (never `scxml-toolchain`). Its `validate` element wraps the existing validator; documents bind it with a prefix other than `scxml` (for example `sx`). |
| D9 | 2026-10-06 | The builder bot is the first bot. It builds SCXML only (no Rust). It is an SCXML machine used through `<invoke type="scxml" src="...">` and events (`builder.request` to `builder.done` or `builder.failed`); a CLI comes later. Loop: intake spec, design, author SCXML (with event schemas and test event scripts), `sx:validate`, apply machine-applicable fixes or feed diagnostics back to `generate`, bounded retries modelled as states, simulate by invoking the draft as a child (`<invoke type="scxml">` with `<content>`) and sending test events through `#_<invokeid>`, package, done. Retries and timeouts live in the topology (`<send delay>`), never ad hoc. |
| D10 | 2026-10-06 | Interim model backends: `generate` uses OpenAI (`OPENAI_API_KEY`, optional `OPENAI_BASE_URL`); `decision` uses TypeSafe `system_one` (`TYPESAFE_API_KEY`). NVIDIA (Nemotron through an OpenAI-compatible API) and CLM become the defaults later, so backends stay pluggable through `env:baseurl`. Keys live in mosfet's `.env` and are used only through env files and `env` slots: never printed, logged, copied, or committed. |
| D11 | 2026-10-06 | Memory is SQLite first, then PGlite (not in the first night's scope). |
| D12 | 2026-10-06 | Git work is local commits only: no push, no pull requests, no remotes, no branches beyond the named ones. Each repository's AGENTS.md/CONTRIBUTING.md is followed; scxml.rs's rules are normative (fail closed, no unwrap/panic on reachable paths, wasm-only resource bounds, hard cutovers, canonical outputs, thin hosts, Wasm components behind versioned WIT). |
| D14 | 2026-10-06 | nemobot has no runtime of its own. nemobot is bots (SCXML documents plus their schemas) and packages (`namespaces/*`, `services/*`), and they run on scxml.rs through `scxml-cli run`. The `host/` crate of D7 (nemobot's embedder: config file, env grants, package binding, WASI grants, consent listing, deferred transport, extension call fuel, session loop) is deleted as a hard cutover; what scxml-cli lacked went upstream (argument files, `--extension-call-fuel`). Each bot ships a scxml-cli argument file (`bots/<name>/<name>.args`) and, when it answers `#_parent`, a parent document; the tests drive `scxml-cli`. |
| D13 | 2026-10-06 | Anything asynchronous, I/O-bound, or long-running is an invoked service (`<invoke>`). Extension elements are executable content: synchronous and quick, per the SCXML specification. So extension components get no WASI (services only; the host refusal stays, as the rule rather than a gap), and model calls never run inside executable content. `generate` becomes `services/generate`; `decision` splits into the synchronous `<decision:menu>` (namespace) and `services/decision`. |
| D15 | 2026-10-06 | Invoked services cut over to WASI 0.3 and the Component Model async ABI on wasmtime 49.0.2 (scxml.rs `67c811ab`), a hard cutover. The `invoked-service` world imports `wasi:clocks/monotonic-clock@0.3.0` (the session's logical clock), `outbound-http` (whose `fetch` is an `async func`), and `telemetry`, and exports one `invoke: async func(request: invoke-request, events: stream<event>) -> result<tuple<stream<event>, future<result<option<data-value>, component-error>>>, component-error>`. This reverses D4's manifest `wasi-permissions` (a service may use exactly the WASI interfaces its component imports; the run grants runtime resources with wasmtime CLI names: `--dir URI=HOST::GUEST`, `--wasi URI=inherit-network`, `--wasi URI=allow-ip-name-lookup`; `--grant` is deleted) and the package event contracts of D5 (manifests have no `events`; the invoking document describes service events with `event:schema`, so `services/generate/package/events.json` is deleted and its schemas live in `bots/builder/builder.scxml` and `bots/builder/schemas`). The deferred transport (`outbound-http.begin`/`take`, `wake`) is deleted; a service's HTTP responses are recorded inputs, so record and replay work with services. `services/generate` and `services/decision` are each one async `invoke`. |
| D16 | 2026-10-06 | Running a bot is `scx run --env-file .env bots/<name>/<name>.scx`, nothing else (scxml.rs `281f56ef`..`f4b0bcab`, hard cutovers). The scxml.rs binary is `scx` (crate `scxml-cli`); repository-authored SCXML documents are `.scx` (the W3C IRP corpus keeps `.scxml`; the format is content-defined). `scx` embeds the authenticated release session components (`wasmtime` is a default feature; `--components`/`--components-manifest-sha256` only override them). Packages resolve from the document's repository URIs through scxml.rs's resolver; `scripts/build-components.sh` publishes the locally built packages into its cache (`--cache-dir`, default `${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions`) as a local mirror, so documents never change, and `components.args` is deleted (scxml.rs keeps the explicit `--extension-package`/`--service-package` flags for non-repository URIs such as its examples). Env scopes come from the document: a variable goes only to the elements and `<invoke>`s whose `env` attributes name it; `--env NAME@URI` is deleted (`--env NAME[=VALUE]` and `--env-file` remain). An `<invoke src>` beside the document loads without `--resource`. `DEFAULT_EXTENSION_CALL_FUEL` is one billion and `--extension-call-fuel` is deleted (`scx:validate` measured 166M to 214M instructions per call, mostly compiling the W3C XSD set). `@FILE` argument files and every `.args` file are deleted. The builder reports through the `<donedata>` of its final states `done` and `failed` (`done.invoke.<id>` when invoked), logs its progress, and takes `builder.request` as an event (standard input when run directly); its `builder.done`/`builder.failed` sends and `bots/builder/parent.scxml` are deleted. This reverses D14's argument files and the `--env NAME@URI` scoping of the "Env scoping" note below. |
| D17 | 2026-10-06 | The `scxml` namespace (`<sx:validate>`, D8) moves into scxml.rs as `namespaces/scx` (crate and package `scx`, prefix `scx`, `<scx:validate>`; scxml.rs `50283481`), a hard cutover: `namespaces/scxml` is deleted here. Its URI is the tagless `https://github.com/stateforward/scxml.rs/namespaces/scx`, versioned with the `scx` toolchain like the `env` and `event` vocabularies, and `scx` embeds its package and component, so documents use it with no fetch. This supersedes D8. |
| D18 | 2026-10-06 | When no `baseurl` value is held, an invoked service uses its package's declared origin: the first `http-origins` entry of the manifest it ships (read from that manifest, so the origin is written once; no manifest field is added). Precedent: scxml.rs's deleted `openai` namespace defaulted to `https://api.openai.com/v1`. `services/generate` declares `https://api.openai.com/v1` (a path prefix, which `http-origins` admits) and `services/decision` `https://api.typesafe.ai`; a missing key is still `unconfigured`. This reverses the D10 note that made `env:baseurl` required. |
| D19 | 2026-10-06 | Repository URIs take an optional `@<tag>` (scxml.rs `a5a67c4f`): `.../namespaces/<name>@<tag>` and `.../services/<name>@<tag>` pin a release, and a tagless URI resolves the repository's latest GitHub release through the same bounded, verified path (asked once per root session; the cache entry records its `tag`; the consent listing shows the resolved tag and commit); the built-in tagless `env`, `event`, and `scx` are matched first and never fetched. Each URI is resolved once per root session, and every invoked SCXML child (inline `<content>` or `<invoke src>`) with the same import shares the root's resolved, compiled component (one instance per session); a pinned and a tagless URI coexist; a child-only import is resolved with the root's before the session starts, which closes the gap of a repository namespace only an `<invoke src>` child used. `<invoke src>`, `<script src>`, and `<data src>` load `https:`, `file:`, and relative URIs (against the naming document's base URI): every literal URL is fetched once before the first macrostep and listed, an unfetchable `<script src>` rejects the document (SCXML 5.8), and a `srcexpr` URL is fetched when its `<invoke>` runs (`error.execution` on failure); `--resource` and the at-or-below-the-document rule are deleted (hard cutover). nemobot's documents, manifests, and `targetNamespace` use the tagless URIs, and `scripts/build-components.sh` publishes into a dev cache under the artifacts directory (`target/components/package-cache`, its `--cache-dir` flag deleted), never the release cache, so a dev run is `scx run --offline --cache-dir target/components/package-cache --env-file .env bots/<name>/<name>.scx` and a release run drops the two flags. This supersedes D16's "publishes the locally built packages into its cache" and its `<invoke src>` beside-the-document rule. |
| D20 | 2026-10-06 | The builder's draft contract moves into the validator as its draft profile (scxml.rs `74020230`): `<scx:validate profile="draft">` (`Validator::with_profile(Profile::Draft)`) reports, from the parsed XML, `SCXML097` (a namespace other than SCXML's, `event`, and `xml`), `SCXML098` (`<invoke>`, `<script>`), `SCXML099` (`src`, `srcexpr`), `SCXML100` (`typeexpr`, a `<send type>` other than the SCXML Event I/O Processor), and `SCXML101` (a DTD), documented in scxml.rs `docs/validator-rules.md` and `scx --explain`. The schema policy governs only namespaces without a schema and there was no rule-set concept, so `profile` is the attribute (the maintainer's term; SCXML 1.0 Appendix B calls its data-model restrictions profiles). The builder's regex function and its `draft-contract` diagnostics are deleted (hard cutover). |
| D21 | 2026-10-06 | `@<tag>` is resolution syntax, not namespace identity (scxml.rs `50f6e592`). A repository package's namespace and service type identity is always its tagless URI; release manifests and schemas (`targetNamespace`) name it, and a manifest naming a pinned URI is refused. A document may write `.../<name>@<tag>` in an `xmlns` declaration or a literal `<invoke type>` to pin a release: `scxml_types::strip_tag` splits the tag off once, when the document is parsed (`NamespaceDeclaration::tag`, `Attribute::tag`), and only `scx`'s resolver reads it; the validator (which validates such a document as its infoset, so the element namespace equals the schema's tagless `targetNamespace`), compiler, IR, engine, data models, draft profile, and host see only the tagless URI, exactly. Only a repository package URI followed by one tag is split (any other URI with an `@` is unchanged); a malformed pin, one document pinning a package to two releases, and a pinned built-in (`env`, `event`, `scx`) fail closed. The run binds each package to the root's resolution (its pin, else the latest release; for a package only `<invoke src>` children use, the first such child's), shared by every tagless use; an `<invoke src>` child that pins another release binds that release for its own sessions and their inline `<content>` sessions through the host's per-document bindings (`EmbeddedDocument::extensions`, `EmbeddedDocument::services`). A `srcexpr` child that pins a release does not load. scxml.rs's `set-namespace-version.sh` is deleted and `check-namespace-versions.sh` checks tagless URIs. This supersedes D19's "a pinned and a tagless URI coexist" as distinct URIs and D7's tagged repository URIs. |

## Notes on applying the decisions

- D16 (2026-10-06) supersedes the notes below where they name
  `scxml-cli`, argument files (`components.args`, `bots/<name>/<name>.args`),
  `--env NAME@URI`, `--extension-call-fuel`, `parent.scxml`, or
  `builder.done`/`builder.failed`; they are kept as the record of how the
  earlier decisions were first applied.
- D15 (2026-10-06) supersedes the parts of the notes below that name
  `wasi-permissions`, `--grant`, package event contracts,
  `outbound-http.begin`/`take`, `wake`, `ThreadedServiceHttp`, or the
  refusal of `--record`/`--replay` for WASI packages; they are kept as the
  record of how D4, D5, D13, and D14 were first applied. Under D15 the
  `generate` and `decision` services await `outbound-http.fetch` inside
  `invoke` (it suspends only their own task), write their one event to the
  returned stream, and resolve the result future; leaving the invoking
  state drops the invocation and closes its connection (SCXML 6.4), and
  `services/generate` still streams (`"stream": true`, server-sent events)
  internally so the backend can stop generating. The failure code
  `refused` is gone from both services: a request the host refuses before
  sending fails as `transport`, as the host reports both the same way.
  `decision.abstain` and `decision.failed` transitions in the builder gain
  `decision_id !== null` in their `cond` so `<decision:menu>` never offers
  them.

- D4 and D13 (2026-10-06): slice 2 delivers WASI permissions for invoked
  services only, and the scxml.rs session host refuses an extension package
  that declares a WASI permission (`ArtifactError::WasiExtensionUnsupported`),
  as did nemobot's embedder before D14. Review round 1 recorded this as an open gap of
  D4; D13 settles it: extension elements are synchronous executable
  content, so WASI belongs to services only, by rule. Both repositories'
  AGENTS.md and docs state it that way.
- D13 (2026-10-06): a service's slow request must not block the session.
  scxml.rs's `outbound-http.fetch` blocks the calling export call, so
  scxml.rs gained `outbound-http.begin`/`take` (`feat/invoke-services`
  `5eb077c4`): the host admits the request, an embedder transport performs
  it off the session thread (`ThreadedServiceHttp`), and the outcome reaches
  the instance through `wake` (`LiveSession::deliver_service_responses`). A
  cancelled instance abandons its requests, so a `builder.timeout` in the
  invoking state preempts a model call in flight; since scxml.rs
  `50d622d7` the transport also closes each abandoned request's connection
  at once (SCXML 6.4's existing cancel; no new WIT or public name), and
  `services/generate` streams (`"stream": true`) so the backend can stop
  generating when it sees the close. scxml.rs `968095dd` gives `scxml-cli
  run` the same transport, and `14c4b8e9` deletes scxml.rs's `openai`
  namespace (model calls inside executable content), which
  `services/generate` replaces. A logical-clock session
  never moves its clock past a pending request. Since D14 every nemobot run
  uses `scxml-cli run`'s transport, and nemobot's extension packages declare
  no origins.
- D5 and D13 (2026-10-06): "candidates are the enabled transitions" is
  applied by `<decision:menu>`, which evaluates each transition's `cond` in
  the current data model (with the current `_event`) when it builds the
  menu; see `namespaces/decision` ("Guards") for how a failed or
  non-boolean guard is treated. The `decision` service, invoked with that
  menu, fills the chosen event's data from grounded options and validates
  it against the menu's schema before it emits it. scxml.rs closes a
  package's event contract once it declares any event, and the chosen event
  is the document's, so the `decision` service package declares no event
  contract and the service validates what it emits itself (AGENTS.md states
  this exception). `rank` and `classify` are request kinds of the service.
- D2 and the scxml.rs regression (2026-10-06): slice 1 made an `<invoke>`
  whose literal type no service binds fail the session start
  (`load-services`), so scxml.rs's synthetic fixture
  `invoke_with_unknown_processor_type_fails_at_invocation_start` failed. SCXML
  6.4 makes invocation a runtime action (W3C case 216 relies on an
  invocation failing when it runs), so that was a semantic regression:
  scxml.rs `18fce591` loads only bound components at session start and fails
  the `<invoke>` of an unbound type with `error.execution` when it starts,
  literal or `typeexpr`. The fixture passes unchanged.

- D18 (2026-10-06) supersedes this note: a service without a `baseurl`
  value uses its package's first declared origin, so the shell default
  below is no longer needed.
- D10 and AGENTS.md (2026-10-06, review round 1): AGENTS.md forbids compiling
  a backend URL into a component, so the `generate` and `decision` services
  compile in no default base URL and their `env:baseurl` slot is required at
  run time (a missing value emits `generate.failed` or `decision.failed`
  with `unconfigured`, like a missing key). D10's
  "optional `OPENAI_BASE_URL`" holds at the env-file level: a granted
  variable the env file does not set takes the calling shell's value
  (`scxml-cli`'s order: the flag's `=VALUE`, the last `--env-file` entry,
  the shell), so a run gives a non-secret default such as
  `OPENAI_BASE_URL=https://api.openai.com/v1` in its environment and the env
  file's value overrides it. (Before D14 nemobot's own config file had an
  `env-defaults` key for this.) The manifests' `http-origins` stay as the
  consented origins.
- Env scoping (2026-10-06, approved by the maintainer as overnight decision
  4): each granted variable names the exact package URIs (extension
  namespaces or services) allowed to receive it, one
  `scxml-cli run --env NAME@URI` per package (before D14, nemobot's config
  file said the same as `"env": [{"name", "packages"}]`).
  The scope is enforced by scxml.rs's session host (`EnvVariable`,
  `ExtensionEnvironment` on `feat/invoke-services`), so a slot reference to
  a variable not granted to its element's or `<invoke>`'s package is absent
  exactly like an unheld value. The scxml.rs CLI's `--env` became
  `--env NAME@URI[=VALUE]` in the same hard cutover, with no unscoped form.
  Since D14 a scope naming an unbound package, or one with no `env` slot, is
  inert rather than refused (scxml-cli resolves repository namespaces only
  when a session starts), and the consent listing shows each package's
  slots, not the variables granted to it (docs/port-plan.md, known gaps).
- D14 (2026-10-06): the inventory of the deleted embedder and where each
  part went. Config file (`stateforward.nemobot.config/v1`): scxml-cli
  flags, carried in argument files (`@FILE`, rustc's format; scxml.rs
  `91b7c1b3`): `scripts/build-components.sh` writes `components.args` (the
  session components and every package bound to its component by SHA-256)
  and each bot ships `bots/<name>/<name>.args`. Env file and env grants:
  `--env-file` and `--env NAME@URI`; `env-defaults`: the calling shell's
  value. Package binding and component trust: `--extension-package`,
  `--service-package`, and their `-component`/`-component-sha256` flags.
  WASI grants: `--grant URI=PERMISSION[=VALUE]`. Consent listing: the one
  `scxml-cli` writes before running. Deferred service transport and
  cancellation: `scxml-cli run`'s (scxml.rs `968095dd`). Extension call
  fuel: `--extension-call-fuel` (scxml.rs `e7d2fd7a`). `outbound-http:
  false`: a run whose packages declare no origins, or a test env file that
  points at a loopback mock. `fuel`, `max-commands`: `--fuel`,
  `--max-commands`. Session thread and its ingress and egress channels:
  `scxml-cli run`'s standard input and output records. Documents for
  `<invoke src>`: `--resource URI=PATH`. The embedder's tests became the
  `tests/` crate, which drives `scxml-cli`.
