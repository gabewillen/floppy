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
| Source commit | `5eb077c4b` ("Let invoked services make slow outbound requests without blocking the session"), on top of `18fce591` ("Fail an unbound literal invoke type when its invoke starts, not at session start") |
| Squash commits here | `353ce1a` ("Squashed 'scxml.rs/' content from commit 9971c05"), then `5be41b24` ("Squashed 'scxml.rs/' changes from 9971c05a..fe7d555d"), then `070aa258` ("Squashed 'scxml.rs/' changes from fe7d555d..dd18cd11"), then `0a20ad2b` ("Squashed 'scxml.rs/' changes from dd18cd11..2bf0dd55"), then `bf96cbf9` ("Squashed 'scxml.rs/' changes from 2bf0dd55..ebc051ae"), then `a2aaf0da` ("Squashed 'scxml.rs/' changes from ebc051ae1..5eb077c4b") |
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
| D13 | 2026-10-06 | Anything asynchronous, I/O-bound, or long-running is an invoked service (`<invoke>`). Extension elements are executable content: synchronous and quick, per the SCXML specification. So extension components get no WASI (services only; the host refusal stays, as the rule rather than a gap), and model calls never run inside executable content. `generate` becomes `services/generate`; `decision` splits into the synchronous `<decision:menu>` (namespace) and `services/decision`. |

## Notes on applying the decisions

- D4 and D13 (2026-10-06): slice 2 delivers WASI permissions for invoked
  services only, and the scxml.rs session host refuses an extension package
  that declares a WASI permission (`ArtifactError::WasiExtensionUnsupported`),
  as does `nemobot-host`. Review round 1 recorded this as an open gap of
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
  invoking state preempts a model call in flight. A logical-clock session
  never moves its clock past a pending request. `nemobot-host` gives
  services this deferred transport only, and extensions no transport.
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

- D10 and AGENTS.md (2026-10-06, review round 1): AGENTS.md forbids compiling
  a backend URL into a component, so the `generate` and `decision` services
  compile in no default base URL and their `env:baseurl` slot is required at
  run time (a missing value emits `generate.failed` or `decision.failed`
  with `unconfigured`, like a missing key). D10's
  "optional `OPENAI_BASE_URL`" holds at the env-file level: a nemobot config
  may give a granted variable a non-secret default (`env-defaults`, for
  example `{"OPENAI_BASE_URL": "https://api.openai.com/v1"}`) that the env
  file's value overrides. The manifests' `http-origins` stay as the
  consented origins.
- Env scoping (2026-10-06, approved by the maintainer as overnight decision
  4): each granted variable in a nemobot config names the exact package URIs
  (extension namespaces or services) allowed to receive it
  (`"env": [{"name": "OPENAI_API_KEY", "packages": ["<generate service URI>"]}]`).
  The scope is enforced by scxml.rs's session host (`EnvVariable`,
  `ExtensionEnvironment` on `feat/invoke-services`), so a slot reference to
  a variable not granted to its element's or `<invoke>`'s package is absent
  exactly like an unheld value. A grant to an unbound package, or to one
  that declares no `env` slot, fails closed when the packages bind; the
  consent listing shows each variable and its packages (names only). The
  scxml.rs CLI's `--env` became `--env NAME@URI[=VALUE]` in the same hard
  cutover, with no unscoped form.
