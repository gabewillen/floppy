# mosfet to nemobot port plan

mosfet is a Python runtime for software robots: a bot is a **body** (lifetime,
environment I/O, stimulus fan-out) and a **cognition** (interpretation,
behavior selection, dispatch), every stateful concern is a `stateforward-hsm`
state machine, abilities are the composition unit, devices face the
environment, providers own SDKs, and learned **behaviors** are Starlark
programs compiled to HSM behaviors.

nemobot keeps that shape and replaces the substrate: every machine is an SCXML
document run by the scxml.rs engine inside the `scxml-machine` Wasm component,
and every effect is a versioned Wasm component (an extension namespace or an
invoked service). The decisions behind this plan are in
[decisions.md](decisions.md); the rules that bind it are in
[../AGENTS.md](../AGENTS.md).

## Concept map

| mosfet | nemobot |
| --- | --- |
| `mosfet.Bot` (body + cognition) | One root SCXML document per bot under `bots/<name>/`, run as a live session by `nemobot-host` |
| hsm state machines | SCXML documents (table below) |
| Abilities (listening, speaking, cognition, memory, ...) | Invoked children: `<invoke type="scxml" src="abilities/<name>.scxml" id="<name>">`, talking to the parent through `#_parent` and to the child through `#_<invokeid>` |
| Ability attachment protocol (`bot.ability.attachment.*`, yamux) | Not ported: `<invoke>` lifecycle is attach (start on state entry) and detach (cancel on state exit, `done.invoke.<id>` on completion) |
| Devices (phone, audio, smart phone) | Invoked services: `<invoke type="https://github.com/stateforward/nemobot/services/<device>@<tag>">` (later phases) |
| Providers (SDK owners) | Invoked services (`services/`) for every effect: anything asynchronous, I/O-bound, or long-running (decision D13); extension namespaces (`namespaces/`) only for synchronous, quick executable content (table below) |
| Starlark behaviors (compiled, stored, cached) | Validator-gated SCXML: the builder bot authors the behavior as an SCXML document, `sx:validate` gates it, and a simulation as an invoked child proves it before it is stored; a matched behavior runs as an invoked child |
| Persistent routines (`hsm.every` / `hsm.at` across turns) | Long-lived invoked children whose schedules are self-rescheduling `<send delay>` loops; each tick is an event to the parent, which decides on it in a new turn |
| pydantic event data models | JSON Schemas referenced by `event:schema` on the transitions that take the event (metadata-only in the engine) |
| Model-facing event menus and payload filling (Intuition, Processor) | `<decision:menu>` (the `decision` namespace) writes the enabled transitions with their `event:schema`, read through `session-snapshot-host`, as a canonical menu; the `decision` service, invoked with it, asks a backend, fills the chosen event's data to its schema, validates it, and emits it |
| Event names `bot.*`, `bot.ability.*`, `phone.*` | Unchanged, byte for byte |
| Telemetry (`mosfet.telemetry`) | scxml.rs's OpenTelemetry facade (guests through the telemetry WIT; `nemobot-host` through the OpenTelemetry API) |

## hsm DSL to SCXML

Usage counts are from `rg` over `mosfet.py/src/mosfet` and show what the port
has to cover.

| hsm (uses) | SCXML |
| --- | --- |
| `hsm.define(name, ...)` (16) | `<scxml name="..." initial="...">`, one document per machine |
| `hsm.state` (299) | `<state id>`; parallel regions become `<parallel>` |
| `hsm.initial(hsm.target(...))` (99) | the `initial` attribute, or `<initial><transition target/></initial>` when the initial transition has effects |
| `hsm.final` (15) | `<final id>`, with `<donedata>` for a result |
| `hsm.transition(...)` (725) with `hsm.target` (649) | `<transition event="..." target="...">`; a targetless transition is internal (`type="internal"` where the source is compound) |
| `hsm.guard` (574) | `cond` (ECMAScript data model) |
| `hsm.effect` (567) | executable content (synchronous and quick): `<assign>`, `<raise>`, `<send>`, `<log>`, `<script>`, and extension elements (`decision:menu`, `sx:*`); an effect that does I/O becomes an `<invoke>` of a service |
| `hsm.entry` (20) / `hsm.exit` (13) | `<onentry>` / `<onexit>` |
| `hsm.activity` (138) | `<invoke>`: started on entry, cancelled on exit, `done.invoke.<id>` and `error.*` on completion or failure; every model call (the `generate` and `decision` services) is one |
| `hsm.defer` (104) | No SCXML equivalent; modelled with a data-model queue: in a deferring state, `<transition event="E" type="internal"><script>queue.push(_event)</script></transition>`; on exit (or entry of the state that handles them), `<foreach array="queue" item="e"><send eventexpr="e.name" target="#_internal" .../></foreach>` re-raises them in order, then clears the queue |
| `hsm.after(duration)` (48) | `<send event="<state>.after" delay="..." id="<state>-after"/>` on entry, `<cancel sendid="<state>-after"/>` on exit, and a transition on the event |
| `hsm.every(period)` (28) | a self-rescheduling loop: the transition on the tick re-sends the tick with the same `delay` |
| `hsm.at(time)` (22) | `<send delayexpr>` computed from the absolute time and the data model's clock (`Date.now()` in ECMAScript) |
| `hsm.choice` (20) | a transient state with eventless `<transition cond>` branches (the last unconditional) |
| `hsm.shallow_history` (1) | `<history type="shallow">` |
| `hsm.start` (2) / `hsm.stop` (19) | `<invoke>` / reaching a `<final>` (or the invoking state exiting, which cancels the child) |
| `hsm.event` (18) and typed event classes | event descriptors plus `event:schema` contracts |

Retries, timeouts, and bounded loops stay in the topology (counters in the
data model, `<send delay>` timers, states for each attempt), never in host or
component code.

## Providers to namespaces and services

| mosfet provider | nemobot | Phase |
| --- | --- | --- |
| `openai_compat` | `services/generate`, OpenAI backend (`env:apikey`, `env:baseurl`) | 1 |
| `gemini` (text) | `services/generate` through an OpenAI-compatible base URL | 7 |
| `typesafe` (`system_one` Choice/Score) | `services/decision`, TypeSafe backend (`TYPESAFE_API_KEY`) | 1 |
| `needle` (local tool-calling reflex model) | `services/decision` backend (the `Backend` trait) | 7 |
| `sqlite_memory` | `services/memory`, SQLite (a service declaring `wasi:filesystem`) | 3 |
| `postgres_memory` | `services/memory`, PGlite | 7 |
| `elevenlabs`, `moonshine`, `mlx_audio`, `pyannote` | `services/speech` (text-to-speech, speech-to-text, diarization backends) | 6 |
| `livekit` | the `phone` device service | 6 |
| `mlx_vlm` | a `vision` (reading) service | 6 |

NVIDIA Nemotron (OpenAI-compatible) and CLM become the default backends in
phase 7; until then every backend is selected by its `env:baseurl` slot.

## Phases

| Phase | Scope | Status |
| --- | --- | --- |
| 0 | scxml.rs invoked services: slice 1 (`invoked-service` world, host and CLI binding, echo example) and slice 2 (WASI permissions and grants, event contracts, env slots on `<invoke>`, repository services), then deferred service requests (`outbound-http.begin`/`take`, outcomes through `wake`) and unbound invoke types failing at invocation start; nemobot repository scaffold (subtree, workspace, `nemobot-host` embedder and smoke test, namespace skeletons, rules, this plan) | Done (scxml.rs `feat/invoke-services` at `9971c05a`, now `5eb077c4`; this repository's first commits). Extension components get no WASI, by rule (decision D13), not as a gap |
| 1 | Namespaces and services: `scxml` (`sx:validate` wrapping the validator component, returning canonical diagnostics and fixes), `generate` (OpenAI text and structured output), `decision` (enabled-transition candidates, TypeSafe ranking, schema fill and validation before emit) | `scxml` namespace implemented (`sx:validate`); `services/generate` implemented (one Chat Completions call per `<invoke>`, text or schema-validated structured output, `generate.result`/`generate.failed`, never blocking the session; `env:apikey`/`env:baseurl`); `decision` split per D13: the namespace's `decision:menu` (snapshot menu of `event:schema` transitions whose guards evaluate to true) and `services/decision` (request kinds `choose`, `rank`, `classify`; grounded per-member fill, validation against the menu's schema before emit, `decision.abstain`, `decision.failed`; TypeSafe `system_one` backend behind the `Backend` trait, `env:apikey`/`env:baseurl`) |
| 2 | Builder bot: `bots/builder/` SCXML (intake, design, author, `sx:validate`, fix or regenerate with bounded retries as states, simulate the draft as an invoked child via `#_<invokeid>`, package, `builder.done`/`builder.failed`), then a CLI around `nemobot-host` | `bots/builder/builder.scxml` implemented (contract schemas in `bots/builder/schemas`; design and author invoke the generate service and triage invokes the decision service on the `decision:menu` menu, each with `<finalize>`/`done.invoke` and a `builder.timeout` that preempts and cancels the call; cond-based fallback; simulation as an invoked child); deterministic harness in `host/tests/builder_bot.rs` (including a timeout firing while a generation is in flight); live results in [live-proof.md](live-proof.md); the CLI is not started |
| 3 | Memory service on SQLite | Not started |
| 4 | Bot core: body and cognition as SCXML, abilities as invoked children, mosfet's pydantic event models as `event:schema` JSON Schemas, `bot.*` names unchanged | Not started |
| 5 | Behaviors and routines: learned behaviors authored by the builder bot, validated, simulated, stored in memory, invoked when matched; routines as long-lived children | Not started |
| 6 | Speech, vision, and devices as services (`speech`, `vision`, `phone`, audio) | Not started |
| 7 | Defaults and release: NVIDIA Nemotron and CLM backends, PGlite memory, tagged releases with `SHA256SUMS` for repository namespaces and services | Not started |

## Known gaps carried from phase 0

- By rule (decision D13), not as a gap: extension components get no WASI
  interface and no outbound transport; `nemobot-host` refuses extension
  packages that declare a WASI permission, and grants to extension URIs.
  Every effect is an invoked service.
- A cancelled service's request is abandoned, not interrupted: the
  transport thread runs until the response or its own timeout (600 s in
  `nemobot-host`), and the outcome is dropped.
- `nemobot-host` produces the consent listing (`Packages::consent`) but no
  binary shows it yet: showing it before running belongs to the CLI of
  phase 2, which is not started.
- `nemobot-host` reimplements the CLI's embedding steps (package loading,
  validated compilation, `event:schema` file loading) because they are private
  to `scxml-cli`; an upstream embedding library would remove the duplication.
  It links no expression syntax checker, so data-model expressions are judged
  when evaluated rather than at validation.
- `nemobot-host` binds packages only from local files its config names; it
  does not resolve repository namespaces or services (the CLI's resolver does).
- Sessions are not recorded or replayed.
- Each granted `env` variable is scoped to the package URIs the config
  lists for it (`env: [{"name", "packages"}]`); a slot of any other
  package that names it is absent, exactly like an unheld value. The scope
  is by package URI, so a child session an `<invoke type="scxml">` compiles
  at run time (which inherits the parent's packages) reaches a variable
  through those packages' elements too; the builder's draft contract still
  refuses drafts that use foreign namespaces.
