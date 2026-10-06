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
| Providers (SDK owners) | Extension namespaces (`namespaces/`) for executable content, invoked services (`services/`) for long-lived effects (table below) |
| Starlark behaviors (compiled, stored, cached) | Validator-gated SCXML: the builder bot authors the behavior as an SCXML document, `sx:validate` gates it, and a simulation as an invoked child proves it before it is stored; a matched behavior runs as an invoked child |
| Persistent routines (`hsm.every` / `hsm.at` across turns) | Long-lived invoked children whose schedules are self-rescheduling `<send delay>` loops; each tick is an event to the parent, which decides on it in a new turn |
| pydantic event data models | JSON Schemas referenced by `event:schema` on the transitions that take the event (metadata-only in the engine) |
| Model-facing event menus and payload filling (Intuition, Processor) | The `decision` namespace: candidates are the enabled transitions with their `event:schema` read through `session-snapshot-host`, ranked by a backend, data filled to the schema and validated before the event is raised |
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
| `hsm.effect` (567) | executable content: `<assign>`, `<raise>`, `<send>`, `<log>`, `<script>`, and extension elements (`generate:*`, `decision:*`, `sx:*`) |
| `hsm.entry` (20) / `hsm.exit` (13) | `<onentry>` / `<onexit>` |
| `hsm.activity` (138) | `<invoke>`: started on entry, cancelled on exit, `done.invoke.<id>` and `error.*` on completion or failure |
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
| `openai_compat` | `generate` namespace, OpenAI backend (`env:apikey`, `env:baseurl`) | 1 |
| `gemini` (text) | `generate` namespace through an OpenAI-compatible base URL | 7 |
| `typesafe` (`system_one` Choice/Score) | `decision` namespace, TypeSafe backend (`TYPESAFE_API_KEY`) | 1 |
| `needle` (local tool-calling reflex model) | `decision` namespace backend | 7 |
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
| 0 | scxml.rs invoked services: slice 1 (`invoked-service` world, host and CLI binding, echo example) and slice 2 (WASI permissions and grants, event contracts, env slots on `<invoke>`, repository services); nemobot repository scaffold (subtree, workspace, `nemobot-host` embedder and smoke test, namespace skeletons, rules, this plan) | Done (scxml.rs `feat/invoke-services` at `9971c05a`; this repository's first commits) |
| 1 | Namespaces: `scxml` (`sx:validate` wrapping the validator component, returning canonical diagnostics and fixes), `generate` (OpenAI text and structured output), `decision` (enabled-transition candidates, TypeSafe ranking, schema fill and validation before raise) | `scxml` implemented (`sx:validate`); `generate` implemented (`gen:generate`: Chat Completions text and schema-validated structured output, `env:apikey`/`env:baseurl`); `decision` implemented (`decision:rank`, `decision:classify`, `decision:choose`: snapshot menu of `event:schema` transitions, grounded per-member fill, schema validation before raise, `decision.abstain`; TypeSafe `system_one` backend behind the `Backend` trait, `env:apikey`/`env:baseurl`) |
| 2 | Builder bot: `bots/builder/` SCXML (intake, design, author, `sx:validate`, fix or regenerate with bounded retries as states, simulate the draft as an invoked child via `#_<invokeid>`, package, `builder.done`/`builder.failed`), then a CLI around `nemobot-host` | Not started |
| 3 | Memory service on SQLite | Not started |
| 4 | Bot core: body and cognition as SCXML, abilities as invoked children, mosfet's pydantic event models as `event:schema` JSON Schemas, `bot.*` names unchanged | Not started |
| 5 | Behaviors and routines: learned behaviors authored by the builder bot, validated, simulated, stored in memory, invoked when matched; routines as long-lived children | Not started |
| 6 | Speech, vision, and devices as services (`speech`, `vision`, `phone`, audio) | Not started |
| 7 | Defaults and release: NVIDIA Nemotron and CLM backends, PGlite memory, tagged releases with `SHA256SUMS` for repository namespaces and services | Not started |

## Known gaps carried from phase 0

- The scxml.rs session host still links no WASI interface into extension
  components (`ArtifactError::WasiExtensionUnsupported`), although decision D4
  lets extensions declare any WASI permission; `nemobot-host` therefore refuses
  extension packages that declare one, and grants to extension URIs. Closing
  this belongs upstream in scxml.rs.
- `nemobot-host` reimplements the CLI's embedding steps (package loading,
  validated compilation, `event:schema` file loading) because they are private
  to `scxml-cli`; an upstream embedding library would remove the duplication.
  It links no expression syntax checker, so data-model expressions are judged
  when evaluated rather than at validation.
- `nemobot-host` binds packages only from local files its config names; it
  does not resolve repository namespaces or services (the CLI's resolver does).
- Sessions are not recorded or replayed.
