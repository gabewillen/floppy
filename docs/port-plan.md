# mosfet to floppy port plan

mosfet is a Python runtime for software robots: a bot is a **body** (lifetime,
environment I/O, stimulus fan-out) and a **cognition** (interpretation,
behavior selection, dispatch), every stateful concern is a `stateforward-hsm`
state machine, abilities are the composition unit, devices face the
environment, providers own SDKs, and learned **behaviors** are Starlark
programs compiled to HSM behaviors.

floppy keeps that shape and replaces the substrate: every machine is an SCXML
document run by the scxml.rs engine inside the `scxml-machine` Wasm component,
and every effect is a versioned Wasm component (an extension namespace or an
invoked service). The decisions behind this plan are in
[decisions.md](decisions.md); the rules that bind it are in
[../AGENTS.md](../AGENTS.md).

## Concept map

| mosfet | floppy |
| --- | --- |
| `mosfet.Bot` (body + cognition) | One root SCXML document per bot under `bots/<name>/`, run as a live session by scxml.rs's `scx run` (floppy has no runtime of its own, decision D14) |
| hsm state machines | SCXML documents (table below) |
| Abilities (listening, speaking, cognition, memory, ...) | Invoked children, one `.scx` per mosfet machine in the flat `abilities/` directory: `<invoke id="<name>" type="scxml" src="../../abilities/<name>.scx">` from the body (a stage or sub-ability from its ability: `src="<name>.scx"`), literal `id` and `src`, talking to the parent through `#_parent` and to the child through `#_<invokeid>` (decisions D33, D34) |
| Ability attachment protocol (`attachment.*`, `attachment.group.*`, `bot.ability.attachment.terminal`, yamux) | Not ported, and none of these names has a schema: `<invoke>` lifecycle is attach (start on state entry) and detach (cancel on state exit, `done.invoke.<id>` on completion); a group's attach is the body's parallel `<invoke>`s joined on each member's first `bot.ability.offers` (decision D6) |
| Devices (phone, audio, smart phone) | Invoked services: `<invoke type="https://github.com/stateforward/floppy/services/<device>@<tag>">` (later phases) |
| Providers (SDK owners) | Invoked services (`services/`) for every effect: anything asynchronous, I/O-bound, or long-running (decision D13); extension namespaces (`namespaces/`) only for synchronous, quick executable content (table below) |
| Starlark behaviors (compiled, stored, cached) | Validator-gated SCXML: the builder bot authors the behavior as an SCXML document, `scx:validate` gates it, and a simulation as an invoked child proves it before it is stored; a matched behavior runs as an invoked child |
| Persistent routines (`hsm.every` / `hsm.at` across turns) | Long-lived invoked children whose schedules are self-rescheduling `<send delay>` loops; each tick is an event to the parent, which decides on it in a new turn |
| pydantic event data models | JSON Schema files referenced by `event:schema` on the transitions that take the event; scxml.rs checks them where the event is created (`<send>`, `<raise>`, a send between sessions) and on host input (decision D41) |
| Hand-written payload validators (`model_validate`, field checks) | `event:schema` only; a `cond` correlates (which child, which turn, which timer) and never checks a payload's shape (decision D42) |
| Helper functions (attention bias, envelopes, choice assembly and compaction, routing, confidence tuner, message commit) | floppy namespace elements that write to a `location` (`namespaces/bot`, `namespaces/conversation`, `namespaces/decision`; decision D40) |
| Model-facing event choice lists and payload filling (Intuition, Processor) | `<decision:choice>` (the `decision` namespace) writes the enabled transitions with their `event:schema`, read through `session-snapshot-host`, as a canonical choices; the `decision` service, invoked with it, asks a backend, fills the chosen event's data to its schema, validates it, and emits it |
| Event names `bot.*`, `bot.ability.*`, `phone.*` | Unchanged, byte for byte |
| Telemetry (`mosfet.telemetry`) | scxml.rs's OpenTelemetry facade (guests through the telemetry WIT; the native runtime, `scx`, through scxml.rs's own) |

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
| `hsm.effect` (567) | executable content (synchronous and quick): `<assign>`, `<raise>`, `<send>`, `<log>`, `<foreach>`, `<if>`, and extension elements (`decision:*`, `bot:*`, `conversation:*`, `scx:validate`); never `<script>` in a floppy document (decision D39); an effect that does I/O becomes an `<invoke>` of a service |
| `hsm.entry` (20) / `hsm.exit` (13) | `<onentry>` / `<onexit>` |
| `hsm.activity` (138) | `<invoke>`: started on entry, cancelled on exit, `done.invoke.<id>` and `error.*` on completion or failure; every model call (the `generate` and `decision` services) is one |
| `hsm.defer` (104) | No SCXML equivalent; modelled with a data-model queue (see [Phase 4 conventions](#phase-4-conventions)): in a deferring state, a targetless transition on E appends `<assign location="queue" expr="queue.concat([...])"/>`; in the state that handles them, an eventless `<transition cond="queue.length &gt; 0">` takes `queue.shift()`, one per return, in order (no `#_internal` replay) |
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

| mosfet provider | floppy | Phase |
| --- | --- | --- |
| `openai_compat` | `services/generate`, OpenAI backend (`env:apikey`, `env:baseurl`) | 1 |
| `gemini` (text) | `services/generate` through an OpenAI-compatible base URL | any (by `FLOPPY_BASE_URL`) |
| `typesafe` (`system_one` Choice/Score) | `services/decision`, TypeSafe backend (`TYPESAFE_API_KEY`) | 1 |
| `needle` (local tool-calling reflex model) | `services/decision` backend (the `Backend` trait) | later |
| `sqlite_memory` | `services/sql`, SQLite compiled into the component (it imports `wasi:filesystem`; the run grants a directory) under `abilities/memory/memory.scx`, migrated by `abilities/migration/migration.scx` (decisions D44 to D51) | 3 |
| `postgres_memory` | `services/pgsql`, the Postgres wire protocol over `wasi:sockets/tcp` (the run grants `inherit-network`) to PGlite served beside `scx` (pglite-socket) or any Postgres, behind the same sql contract and the same Memory ability, which picks it for a `postgresql://` `database`; tested on Postgres 16 (decisions D51, D69 to D72) | 7 |
| `elevenlabs`, `moonshine`, `mlx_audio`, `pyannote` | `services/speech` (text-to-speech, speech-to-text, diarization backends) | 6 |
| `livekit` | the `phone` device service | 6 |
| `mlx_vlm` | a `vision` (reading) service | 6 |

No backend is a default (decision D31): `generate` reaches only the
endpoint a run names with `FLOPPY_BASE_URL` (direct vLLM on the DGX Sparks
in the gitignored `.env`), and `decision` uses TypeSafe through its own
`env` slots.

## Phases

| Phase | Scope | Status |
| --- | --- | --- |
| 0 | scxml.rs invoked services: slice 1 (`invoked-service` world, host and CLI binding, echo example) and slice 2 (WASI permissions and grants, event contracts, env slots on `<invoke>`, repository services), and unbound invoke types failing at invocation start; floppy repository scaffold (subtree, workspace, an embedder and its smoke test, namespace skeletons, rules, this plan; decision D14 deleted the embedder, and the smoke test now drives `scx run`) | Done. Services are now one async `invoke` export on WASI 0.3 that awaits `outbound-http.fetch` (decision D15, which deleted the `outbound-http.begin`/`take`/`wake` API), with no host time limit on their I/O: the invoking state's `<send delay>` bounds it, and leaving the state cancels the invocation and closes its connection (SCXML 6.4). Extension components get no WASI, by rule (decision D13), not as a gap |
| 1 | Namespaces and services: `scxml` (now scxml.rs's embedded `scx` namespace, decision D17: `scx:validate` wrapping the validator component, returning canonical diagnostics and fixes), `generate` (OpenAI text and structured output), `decision` (enabled-transition candidates, TypeSafe ranking, schema fill and validation before emit) | `scxml` namespace implemented, then moved to scxml.rs as `scx` (`scx:validate`); `services/generate` implemented (one Chat Completions call per `<invoke>`, text or schema-validated structured output, `generate.result`/`generate.failed`, never blocking the session; `env:apikey`/`env:baseurl`); `decision` split per D13: the namespace's `decision:choice` (snapshot choices of `event:schema` transitions whose guards evaluate to true) and `services/decision` (request kinds `choose`, `rank`, `classify`; grounded per-member fill, validation against the choices' schema before emit, `decision.abstain`, `decision.failed`; TypeSafe `system_one` backend behind the `Backend` trait, `env:apikey`/`env:baseurl`) |
| 2 | Builder bot: `bots/builder/` SCXML (intake, design, author, `scx:validate`, fix or regenerate with bounded retries as states, simulate the draft as an invoked child via `#_<invokeid>`, ending in the final states `done`/`failed` whose `<donedata>` carries the result), run with `scx run` | `bots/builder/builder.scx` implemented (contract schemas in `bots/builder/schemas`; design and author invoke the generate service and triage invokes the decision service on the `decision:choice` choices, each with `<finalize>`/`done.invoke` and a `builder.timeout` that preempts and cancels the call; cond-based fallback; simulation as an invoked child); runs with `scx run --env-file .env bots/builder/builder.scx` (decisions D14, D16; [bots/builder/README.md](../bots/builder/README.md#running)); deterministic harness in `tests/tests/builder_bot.rs`, which runs that command (including a timeout firing while a generation is in flight); live results in [live-proof.md](live-proof.md) |
| 3 | Memory service on SQLite | Done: `services/sql` (`floppy_service_sql`), one SQLite transaction of mosfet's `Statement`s per `<invoke>` on a database file in a directory the run grants (`--dir URI=HOST::/memory`), answering `sql.result` or `sql.failed`; mosfet's Memory ability as `abilities/memory/memory.scx`, which migrates mosfet's schema and applies statements through it, answering `bot.ability.memory.output` or `bot.ability.failed`; reasoning recalls and retains cognitive episodes and standing directives, the identity recalls and remembers its adopted name, and the conversation recalls and remembers a relationship's memories (decisions D44 to D47, [plan](plans/phase3-memory.mdscript.md)). Reflection and behaviors use it in phase 5 |
| 4 | Bot core: body and cognition as SCXML, abilities as invoked children, mosfet's pydantic event models as `event:schema` JSON Schemas, `bot.*` names unchanged | Done through the script-validator cutover: the body `bots/floppy/floppy.scx`; `abilities/` cognition (autonomy seam, intuition, reasoning), communication (conversation, speaking in text mode); the `bot` and `conversation` namespaces and the decision elements; event contracts and runtime `event:schema` checks; no `<script>` (decisions D33 to D43, [Phase 4 conventions](#phase-4-conventions)). Identity, the phase 5 seams, and the saved live examples finish it ([plan](plans/phase4-bot-core.mdscript.md)) |
| 5 | Behaviors and routines: learned behaviors authored by the builder bot, validated, simulated, stored in memory, invoked when matched; routines as long-lived children | Not started |
| 6 | Speech, vision, and devices as services (`speech`, `vision`, `phone`, audio) | Not started |
| 7 | Defaults and release: PGlite memory, tagged releases with `SHA256SUMS` for repository namespaces and services (releases wait for going public) | Done except publishing. Memory on Postgres: `services/pgsql` behind the unchanged Memory ability, selected by its `database` (a `postgresql://` URI), migrating mosfet's full head schema in the Postgres dialect through the Alembic-style migration ability, with every ability's SQL portable to both engines and tested on Postgres 16; PGlite is reached over its socket server, not compiled into a component (decisions D48 to D51, D69 to D72). Defaults: README.md's [Defaults](../README.md#defaults) and `.env.example`, checked by `tests/tests/defaults.rs`; there is no default backend, so every run names its endpoint with `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (decision D31). Releases: canonical Linux builds, `SHA256SUMS` and a dry run of the release layout, prepared and checked locally ([release.md](release.md)); no tag, release or upload until the repositories are public ([plan](plans/phase7-pglite-defaults.mdscript.md)) |

## Phase 4 event contracts

Every phase 4 event has one JSON Schema 2020-12 file titled by its name,
extracted from mosfet's own pydantic models
(`embeddable_json_schema(event_json_schema(E))` for each `hsm.Event` E):
body events in `bots/floppy/schemas/`, ability events in
`abilities/schemas/`, and the full list of mosfet's domain event names in
`abilities/schemas/mosfet-event-names.json`. Documents reference these files
(`event:schema="schemas/<event>.json"`); they never carry inline copies. A
schema reference is relative to its document and cannot leave the
document's directory (no `..`), so an ability document lives directly in
`abilities/` (`abilities/<ability>.scx`) and the body in `bots/floppy/`.
The ability events the body receives (`bot.ability.offers`,
`bot.ability.cognition.{output,cancelled}`, `bot.ability.failed`,
`bot.ability.processing.cancelled`,
`bot.ability.communication.{input,failed}`,
`bot.ability.conversation.output`, `bot.ability.speaking.output`) are
copied byte for byte into `bots/floppy/schemas/` as twins (decision D37).

The extraction script runs with mosfet's interpreter; it is kept outside
this repository, with the floppy plans, as
`~/.agents/projects/floppy/scripts/extract_phase4_contracts.py`:

```sh
~/VSCode/stateforward/mosfet/mosfet.py/.venv/bin/python -I \
  ~/.agents/projects/floppy/scripts/extract_phase4_contracts.py ~/VSCode/stateforward/floppy
```

Mapping rules (the byte-for-byte rule covers event names, not data shapes):

- `title` is the event name and `$schema` is 2020-12; mosfet's
  descriptions and examples are kept.
- mosfet's runtime-only fields are dropped (`operation_id`, `generation`,
  `parent_operation_id`, `parent_generation`, `token`, `request_id`,
  `actor_id`, `capability`): SCXML invoke cancellation replaces the
  cancel-token protocol. Fields pydantic marks `SkipJsonSchema` never
  appear.
- Correlated, non-tool events of a processing turn (the body's
  `bot.processing.*` results and the cognition cascade) require
  `turn`, a positive integer the body counts. mosfet's cognition
  `TurnData` object collapses into `turn` plus its `input`; a bare list
  payload (`bot.ability.cognition.output`) becomes `{turn, output}`. Tool
  events, whose data a model fills, carry no `turn`.
- floppy fields that mosfet carried as runtime objects or process
  configuration are added as data, marked `[floppy field]`:
  `bot.ability.cognition.input` requires `stimulus`, the envelope
  `{event, data, id?, source?, target?}` of the event the body took for the
  turn (mosfet passed the live object, `SkipJsonSchema`), and may carry
  `choices`, the turn's choices the body assembled (its own and each
  ability's `bot.ability.offers`, tagged with the offering ability's
  `target`; mosfet built its dispatch inventory from live actor
  snapshots); `bot.activate` takes an optional `model` the body hands to
  its model-backed abilities.
- Media is a reference: `environment.sound`'s `audio` and
  `environment.visual`'s `image` are `uri-reference` strings, not base64
  bytes. `timedelta` would map to integer milliseconds and other `bytes` to
  base64url strings; no phase 4 event has either.
- `StimulusData` stays the envelope `{event, data, id?, source?, target?}`.
- A recursive "any JSON value" definition (conversation's
  `MessageContent`) becomes `{}`, which admits the same values, so every
  schema is ref-free.
- A name with several mosfet models (`bot.ability.failed`) is their
  `anyOf`.
- floppy-only names are marked `[floppy name]` at the start of the
  description (phase 4: `bot.ability.offers`).

`generate.*` and `decision.*` are not copied beside the abilities: they are
service results, never offered as choices, so an ability needs no schema on
the transitions taking them (scxml.rs does not check an invoked service's
events, so their transitions keep a short shape cond such as
`Object(_event.data).reason`). `tests/tests/contracts.rs` checks the set,
the rules, the twins, and every `.scx` under `abilities/` and
`bots/floppy/` against the document structure rule (decision D39).

## Phase 4 conventions

How phase 4 writes a mosfet machine as SCXML. The decisions behind them are
D33 to D43.

- **Document structure.** One `.scx` per mosfet state machine, invoked by a
  literal `id` and a literal relative `src`. The bot root only composes
  (states, `<invoke>`s, `<send>`s). No `<script>` and no `function`; each
  `cond` and each `expr` outside `<data>` is at most 160 characters.
  Reusable synchronous logic is a namespace element that writes to a
  `location`; I/O is an invoked service (decision D39).
- **Readiness and offers.** An ability is ready when its first
  `bot.ability.offers` [floppy name] reaches the body; the body activates
  when no expected ability is left in its `waiting` list, and an expected
  ability that ends while active degrades the bot. Each offer is the
  ability's `<decision:choice>` filtered to its own events and compacted
  with `<decision:compact>` (decision D35).
- **Choices as data.** On each `processing` entry the body assembles the
  turn's choices (its own `bot.focus_device`, pinned to the turn's focus
  candidates, and `bot.clear_focus`, plus each ability's latest offers
  tagged with its `target`), holds them to 32 KiB, and sends them as
  `bot.ability.cognition.input.choices` [floppy field]. Reasoning selects
  only from them; the body's `routing` state delivers each selection to
  `#_internal` or `#_<target>` with a visible `<foreach>`/`<send>` and logs
  `bot.route`/`bot.route.dropped`.
- **Per-turn stage children.** The cognition invokes each stage (autonomy,
  intuition, reasoning) per turn with `<param>`s named after the mosfet
  input event's fields and takes its result as `done.invoke.<stage>`
  `<donedata>`; leaving the stage cancels it, which replaces mosfet's
  cancel-token protocol. State a stage keeps across turns (the intuition's
  tuner) is passed in by `<param>` and returned in `<donedata>`.
- **Defer queue.** A deferring state appends the event (the body uses a
  `<bot:stimulus>` envelope) to the `queue` data with
  `queue.concat([...])`; the state that handles it takes `queue.shift()`
  with an eventless `cond="queue.length &gt; 0"` transition, one per
  return, in order. Nothing is replayed through `#_internal`, so a replayed
  event has no `from` field to check.
- **Timers.** mosfet's operation machines (processing, attachment, hop and
  reply operations) are a `<send delay id>` in the owning state, a
  `<cancel>` on exit, and a sendid or turn cond. They have no document of
  their own.
- **Memory.** Each memory step is a compound state whose first child
  picks by `database` (the `<param>` the body passes, null without
  memory): with a database it invokes the memory ability
  (`abilities/memory/memory.scx`, over `services/sql`) with mosfet's statements as
  SQL `<data>` and takes `bot.ability.memory.output` (its `event:schema`)
  or `bot.ability.failed` by invoke id; without one it is
  the phase 4 seam, which answers itself at once through `#_internal` with
  mosfet's event and logs `<log label="memory" expr="'absent'">`. Either
  way the step ends with mosfet's event (`bot.ability.reasoning.recalled`,
  `.retained`, `bot.ability.identity.recalled`, `.adopted`), so the
  topology does not change (decision D44).
- **Speaking in text mode.** Speaking says a response as
  `<log label="bot.say">` and answers `bot.ability.speaking.output` until
  the phase 6 speech service (decision D38).
- **Traces.** Each deterministic test in `body.rs`, `cognition.rs` and
  `communication.rs` compares its whole run with
  `tests/fixtures/phase4/traces/<test>.ndjson` byte for byte; rewrite them
  with `FLOPPY_TRACE_BLESS=1` only for an intended change.

### Document structure map

| mosfet unit | floppy artifact | kind |
| --- | --- | --- |
| `bot.py` Bot body | `bots/floppy/floppy.scx` | bot root `.scx` |
| `bot.py` attachment Group, `_AttachmentOperation`, `_ProcessingOperation` | the body's `<invoke>`s in `live`, its offers bookkeeping and its delayed `<send>`/`<cancel>` | composition inside the body |
| `attention_bias_for_turn`, `_interrupt_reference`, `_target_device_reference` | `<bot:attention>` | `namespaces/bot` element |
| stimulus envelope | `<bot:stimulus>` | `namespaces/bot` element |
| `model_facing_event_json_schema` and the 32 KiB bound | `<decision:compact>` | `namespaces/decision` element |
| `collect_offered_events`, `_dispatch_bot_processing` | `<decision:assemble>` | `namespaces/decision` element |
| focus pinning | `<decision:pin>` | `namespaces/decision` element |
| `_resolve_target`, `fill_unique_selection_targets` | `<decision:route>` (the body's `<foreach>`/`<send>` delivers) | `namespaces/decision` element |
| choice-name logs | `<decision:names>` | `namespaces/decision` element |
| `_selection_item_branch`, `dispatch_tool` | `<decision:selection>` | `namespaces/decision` element |
| `ConfidenceTuner` | `<decision:tuner>` | `namespaces/decision` element |
| `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`, `_stable_context_ref`, `MessageProvenance` | `<conversation:commit>` | `namespaces/conversation` element |
| `cognition.py` Cognition | `abilities/cognition/cognition.scx` | ability `.scx` child |
| `autonomy.py` Autonomy | `abilities/autonomy/autonomy.scx` (phase 5: the inventory's ACTIVE turn behaviors the stimulus triggers run as `<content expr>` children) | stage `.scx` child |
| `intuition.py` Intuition | `abilities/intuition/intuition.scx` invoking `services/decision` | stage `.scx` child and service |
| `reasoning.py` Reasoning | `abilities/reasoning/reasoning.scx` invoking `services/generate` | stage `.scx` child and service |
| `communication.py` Communication | `abilities/communication/communication.scx` | ability `.scx` child |
| `conversation.py` Conversation | `abilities/conversation/conversation.scx` | ability `.scx` child |
| `speaking.py` Speaking | `abilities/speaking/speaking.scx` (text mode) | ability `.scx` child |
| `identity/identity.py` Identity | `abilities/identity/identity.scx`; text name match `<bot:recognize>`; adoption and recall through `abilities/memory/memory.scx` (decisions D43, D44) | ability `.scx` child |
| Reflection, Routines | `abilities/reflection/reflection.scx`, `abilities/routines/routines.scx` seams; routines are builder-built `.scx` children (phase 5) | ability `.scx` child |
| Behaviors (Starlark) | builder-built `.scx` documents | generated `.scx` |
| Memory and its providers | `abilities/memory/memory.scx` over `services/sql` (SQLite, phase 3) or `services/pgsql` (Postgres, phase 7), migrated by `abilities/migration/migration.scx` with `<migration:plan>` | ability `.scx` child + service |
| Listening, reading, turn detectors, devices | phase 6 `.scx` children and Wasm services | out of phase 4 |
| payload validators | `event:schema` on the receiving transition plus a short correlation cond | `event:schema` |

### Builder output

A builder-produced behavior or routine document is stored as its
inventory row's `bot_behavior.source` and runs as an SCXML child invoked
from that text (`<invoke id><content expr="source"/></invoke>`, phase 5),
the one exception to the literal relative `src` rule. scxml.rs reads
`event:schema` files only for a literal `<invoke src>` child; an invoked
source gets none, and a schema-file reference rejects it when it is
invoked (`error.execution`). So builder output carries no `event:schema`
reference (and, like every floppy document, no inline schema and no
`<script>`): its host checks what it answers. `abilities/autonomy/autonomy.scx`
runs the ACTIVE turn behaviors the stimulus triggers with `turn` and
`input` and answers with the first one's `<donedata>` `handled` and
`selected` (the phase 5 plan's "Turn Behavior Contract").
`tests/fixtures/phase4/behavior_double.scx` stands in for one;
`tests/tests/contracts.rs` checks the rule on it and
`tests/tests/cognition.rs` runs it from a memory inventory.

## Known gaps carried from phase 0

- By rule (decision D13), not as a gap: extension components get no WASI
  interface (scxml.rs refuses an extension package that declares a WASI
  permission), and floppy's extension packages declare no `http-origins`,
  so no request of theirs is admitted. Every effect is an invoked service.
- A cancelled service's request that is still resolving its host or
  opening its TCP connection ends only within the connect timeout (10 s;
  nothing is sent). Once connected, cancellation closes the connection at
  once. Whether the backend then stops generating and billing is up to the
  backend: `OpenAI` documents neither (see `services/generate`).
- `scx run` lists, before running, each package's origins, slots, and
  WASI imports (with what the run grants), and each variable the document
  names with the elements and `<invoke>`s it goes to.
- floppy publishes no GitHub release yet, so its tagless repository URIs
  resolve only from the dev cache `scripts/build-components.sh` publishes
  into (`scx run --offline --cache-dir target/components/package-cache`);
  phase 7 prepared releases ([release.md](release.md)), published once
  the repositories are public, and the same documents then resolve the
  latest one unedited. A release's manifests name the tagless URIs, and a
  document that pins `@<tag>` binds that release under the same tagless
  namespace (decision D21: scxml.rs splits the tag off when it parses the
  document). The dev cache pins each tagless URI to the local build, so a
  pinned document resolves only against a published release.
- Sessions that bind a package declaring a WASI permission cannot be
  recorded or replayed (scxml.rs refuses it); the builder binds none and is
  not recorded either.
- Each variable is scoped to the package URIs of the elements and
  `<invoke>`s whose `env` attributes name it (scxml.rs's document scopes); a
  slot of any other package that names it is absent, exactly like an
  unheld value. The scope is by package URI, so a child session an
  `<invoke type="scxml">` compiles at run time (which inherits the parent's
  packages) reaches a variable through those packages' elements too, which
  is how a builder draft that invokes a bound service reaches it (decision
  D28).
