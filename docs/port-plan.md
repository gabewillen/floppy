# The original to floppy port plan

The original is a Python runtime for software robots: a bot is a **body** (lifetime,
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

| The original | floppy |
| --- | --- |
| `the original.Bot` (body + cognition) | One SCXML document per bot under `bots/<name>/`, invoked by its environment, which `scx run` runs as the live root session (floppy has no runtime of its own, decision D14; the bot is a participant, decision D102) |
| `Environment` (a context: membership, holdings, placement, broadcast) | An environment document, `environments/<name>/<name>.scx` (today `environments/room/room.scx`), run as the root session; its invoked children are its participants, joining when the invoke starts and leaving when it ends; it routes host input by its payload (D108), broadcasts every `environment.sound` and `environment.visual` to the participants that perceive it (placement, the free-field law and thresholds, D103), and logs the events its bots send it (decisions D102 to D111) |
| hsm state machines | SCXML documents (table below) |
| Abilities (listening, speaking, cognition, memory, ...) | Invoked children, one `.scx` per the original machine in the flat `abilities/` directory: `<invoke id="<name>" type="scxml" src="../../abilities/<name>.scx">` from the body (a stage or sub-ability from its ability: `src="<name>.scx"`), literal `id` and `src`, talking to the parent through `#_parent` and to the child through `#_<invokeid>` (decisions D33, D34) |
| Ability attachment protocol (`attachment.*`, `attachment.group.*`, `bot.ability.attachment.terminal`, yamux) | Not ported, and none of these names has a schema: `<invoke>` lifecycle is attach (start on state entry) and detach (cancel on state exit, `done.invoke.<id>` on completion); a group's attach is the body's parallel `<invoke>`s joined on each member's first `bot.ability.offers` (decision D6) |
| Devices (phone, audio, smart phone) | SCXML documents an environment invokes (`devices/<name>/<name>.scx`), any real I/O inside them an invoked service; a bot holds them through the environment's `holdings` and `owned_devices`, reads them with `<device:snapshot>` and acts on them at their address (decisions D105, D109, D110); none is ported yet (the tests use the lamp fixture) |
| Providers (SDK owners) | Invoked services (`services/`) for every effect: anything asynchronous, I/O-bound, or long-running (decision D13); extension namespaces (`namespaces/`) only for synchronous, quick executable content (table below) |
| Starlark behaviors (compiled, stored, cached) | Validator-gated SCXML: the builder bot authors the behavior as an SCXML document, `scx:validate` gates it, and a simulation as an invoked child proves it before it is stored; a matched behavior runs as an invoked child |
| Persistent routines (`hsm.every` / `hsm.at` across turns) | Long-lived invoked children whose schedules are self-rescheduling `<send delay>` loops; each tick is an event to the parent, which decides on it in a new turn |
| pydantic event data models | JSON Schema files referenced by `event:schema` on the transitions that take the event; scxml.rs checks them where the event is created (`<send>`, `<raise>`, a send between sessions) and on host input (decision D41) |
| Hand-written payload validators (`model_validate`, field checks) | `event:schema` only; a `cond` correlates (which child, which turn, which timer) and never checks a payload's shape (decision D42) |
| Helper functions (attention bias, envelopes, choice assembly and compaction, routing, confidence tuner, message commit) | floppy namespace elements that write to a `location` (`namespaces/conversation`, `namespaces/decision`, `namespaces/processing`, `namespaces/intuition`, `namespaces/cognition`, `namespaces/identity`, `namespaces/memory`, `namespaces/behavior`, `namespaces/autonomy`; decisions D40, D113, D114, D115, D116) |
| Model-facing event choice lists and payload filling (Intuition, Processor) | `<decision:choice>` (the `decision` namespace) writes the enabled transitions with their `event:schema`, read through `session-snapshot-host`, as canonical choices; the `decision` service, invoked with it, asks a backend, fills the offered events' data to their schemas and validates it; its kinds are OpenAI Decisions API's question types (D120): `choice` emits the chosen event (or, over `labels`, `decision.classified`), `predicate` (the original's TypeSafe `Processor.process`, which intuition uses, with the stimulus as a param and the deliberative handoff never offered) emits `decision.selected` with every groundable event and its own confidence, and `score` emits `decision.ranked` |
| Event names `bot.*`, `bot.ability.*`, `phone.*` | Unchanged, byte for byte |
| Telemetry (`telemetry`) | scxml.rs's OpenTelemetry facade (guests through the telemetry WIT; the native runtime, `scx`, through scxml.rs's own) |

## hsm DSL to SCXML

Usage counts are from `rg` over the reference implementation's source and show what the port
has to cover.

| hsm (uses) | SCXML |
| --- | --- |
| `hsm.define(name, ...)` (16) | `<scxml name="..." initial="...">`, one document per machine |
| `hsm.state` (299) | `<state id>`; parallel regions become `<parallel>` |
| `hsm.initial(hsm.target(...))` (99) | the `initial` attribute, or `<initial><transition target/></initial>` when the initial transition has effects |
| `hsm.final` (15) | `<final id>`, with `<donedata>` for a result |
| `hsm.transition(...)` (725) with `hsm.target` (649) | `<transition event="..." target="...">`; a targetless transition is internal (`type="internal"` where the source is compound) |
| `hsm.guard` (574) | `cond` (ECMAScript data model) |
| `hsm.effect` (567) | executable content (synchronous and quick): `<assign>`, `<raise>`, `<send>`, `<log>`, `<foreach>`, `<if>`, and extension elements (`decision:*`, `processing:*`, `conversation:*`, `behavior:*`, `scx:validate`); never `<script>` in a floppy document (decision D39); an effect that does I/O becomes an `<invoke>` of a service |
| `hsm.entry` (20) / `hsm.exit` (13) | `<onentry>` / `<onexit>` |
| `hsm.activity` (138) | `<invoke>`: started on entry, cancelled on exit, `done.invoke.<id>` and `error.*` on completion or failure; every model call (the `generate` and `decision` services) is one |
| `hsm.defer` (104) | `<event:defer/>` (scxml.rs `namespaces/event`, decision D93): in a deferring state, a targetless transition on E holds the event; it comes back unchanged, in order, on the internal queue once the state is exited or a state inside it that handles E is entered (see [Phase 4 conventions](#phase-4-conventions)) |
| `hsm.after(duration)` (48) | `<send event="<state>.after" delay="..." id="<state>-after"/>` on entry, `<cancel sendid="<state>-after"/>` on exit, and a transition on the event |
| `hsm.every(period)` (28) | a self-rescheduling loop: the transition on the tick re-sends the tick with the same `delay` |
| `hsm.at(time)` (22) | `<send delayexpr>` computed from the absolute time and the data model's clock (`Date.now()` in ECMAScript); a routine's `at` schedule (time of day, weekdays, IANA zone) gets its next occurrence from `next_after(at, Date.now())` in its own data model, the original's `At.next_after` over the zone's POSIX TZ rule, since the ECMAScript data model has no time zone database (reflection's persistent routine contract gives the function) |
| `hsm.choice` (20) | a transient state with eventless `<transition cond>` branches (the last unconditional) |
| `hsm.shallow_history` (1) | `<history type="shallow">` |
| `hsm.start` (2) / `hsm.stop` (19) | `<invoke>` / reaching a `<final>` (or the invoking state exiting, which cancels the child) |
| `hsm.event` (18) and typed event classes | event descriptors plus `event:schema` contracts |

Retries, timeouts, and bounded loops stay in the topology (counters in the
data model, `<send delay>` timers, states for each attempt), never in host or
component code.

## Providers to namespaces and services

| The original provider | floppy | Phase |
| --- | --- | --- |
| `openai_compat` | `services/generate`, OpenAI backend (`env:apikey`, `env:baseurl`) | 1 |
| `gemini` (text) | `services/generate` through an OpenAI-compatible base URL | any (by `FLOPPY_BASE_URL`) |
| `typesafe` (`system_one` Choice/Score) | `services/decision`, TypeSafe backend (`TYPESAFE_API_KEY`) | 1 |
| `needle` (local tool-calling reflex model) | `services/decision` backend (the `Backend` trait) | later |
| `sqlite_memory` | `services/sql`, SQLite compiled into the component (it imports `wasi:filesystem`; the run grants a directory) under `abilities/memory/memory.scx`, migrated by `abilities/migration/migration.scx` (decisions D44 to D51) | 3 |
| `postgres_memory` | `services/pgsql`, the Postgres wire protocol over `wasi:sockets/tcp` (the run grants `inherit-network`) to PGlite served beside `scx` (pglite-socket) or any Postgres, behind the same sql contract and the same Memory ability, which picks it for a `postgresql://` `database`; tested on Postgres 16 (decisions D51, D69 to D72) | 7 |
| `gemini`, `mlx_audio`, `moonshine` (speech) | `services/speech`: `decode` on `/audio/transcriptions` and `encode` on `/audio/speech` of an OpenAI-compatible backend (`FLOPPY_SPEECH_BASE_URL`, decision D57) | 6 |
| `elevenlabs` | `services/speech` `encode` with `<param name="provider" expr="'elevenlabs'">` (its `text-to-speech` API, decision D57); no floppy document selects it yet | 6 |
| `pyannote`, `mlx_audio` (voice activity, diarization, voice embeddings) | `services/speech` voice operations `detect`, `diarize`, `identify` on LocalAI's `/vad`, `/audio/diarization`, `/voice/embed` (`FLOPPY_VOICE_BASE_URL`, decision D66), with the media steps `normalize`, `append`, `clip` on files | 6 |
| `mlx_vlm` | image decoding through `services/generate` with an image part (decision D58); the visual classifier is rule-based in `abilities/vision/vision.scx` | 6 |
| `livekit` | the `phone` device service | out of scope (devices) |

No backend is a default (decision D31): `generate` reaches only the
endpoint a run names with `FLOPPY_BASE_URL` (direct vLLM on the DGX Sparks
in the gitignored `.env`), `decision` uses TypeSafe through its own
`env` slots, and `speech` reaches only the endpoints a run names with
`FLOPPY_SPEECH_BASE_URL` (speech decoding and encoding) and
`FLOPPY_VOICE_BASE_URL` (voice detection, diarization and identification).

## Phases

| Phase | Scope | Status |
| --- | --- | --- |
| 0 | scxml.rs invoked services: slice 1 (`invoked-service` world, host and CLI binding, echo example) and slice 2 (WASI permissions and grants, event contracts, env slots on `<invoke>`, repository services), and unbound invoke types failing at invocation start; floppy repository scaffold (subtree, workspace, an embedder and its smoke test, namespace skeletons, rules, this plan; decision D14 deleted the embedder, and the smoke test now drives `scx run`) | Done. Services are now one async `invoke` export on WASI 0.3 that awaits `outbound-http.fetch` (decision D15, which deleted the `outbound-http.begin`/`take`/`wake` API), with no host time limit on their I/O: the invoking state's `<send delay>` bounds it, and leaving the state cancels the invocation and closes its connection (SCXML 6.4). Extension components get no WASI, by rule (decision D13), not as a gap |
| 1 | Namespaces and services: `scxml` (now scxml.rs's embedded `scx` namespace, decision D17: `scx:validate` wrapping the validator component, returning canonical diagnostics and fixes), `generate` (OpenAI text and structured output), `decision` (enabled-transition candidates, TypeSafe ranking, schema fill and validation before emit) | `scxml` namespace implemented, then moved to scxml.rs as `scx` (`scx:validate`); `services/generate` implemented (one Chat Completions call per `<invoke>`, text or the calls of one function tool (asked for with `"tool_choice": "required"` and returned as the original's text generator returns them, not `response_format`, decision D112), `generate.result`/`generate.failed`, never blocking the session; `env:apikey`/`env:baseurl`); `decision` split per D13: the namespace's `decision:choice` (snapshot choices of `event:schema` transitions whose guards evaluate to true) and `services/decision` (request kinds `choice`, `predicate`, `score`, D120; grounded per-member fill, validation against the choices' schema before emit, `decision.abstain`, `decision.failed`; TypeSafe `system_one` backend behind the `Backend` trait, `env:apikey`/`env:baseurl`) |
| 2 | Builder bot: `bots/builder/` SCXML (intake, design, author, `scx:validate`, fix or regenerate with bounded retries as states, simulate the draft as an invoked child via `#_<invokeid>`, ending in the final states `done`/`failed` whose `<donedata>` carries the result), run with `scx run` | `bots/builder/builder.scx` implemented (contract schemas in `bots/builder/schemas`; design and author invoke the generate service and triage invokes the decision service on the `decision:choice` choices, each with `<finalize>`/`done.invoke` and a `builder.timeout` that preempts and cancels the call; cond-based fallback; simulation as an invoked child); runs with `scx run --env-file .env bots/builder/builder.scx` (decisions D14, D16; [bots/builder/README.md](../bots/builder/README.md#running)); deterministic harness in `tests/tests/builder_bot.rs`, which runs that command (including a timeout firing while a generation is in flight); live results in [live-proof.md](live-proof.md) |
| 3 | Memory service on SQLite | Done: `services/sql` (`floppy_service_sql`), one SQLite transaction of the original's `Statement`s per `<invoke>` on a database file in a directory the run grants (`--dir URI=HOST::/memory`), answering `sql.result` or `sql.failed`; the original's Memory ability as `abilities/memory/memory.scx`, which migrates the original's schema and applies statements through it, answering `bot.ability.memory.output` or `bot.ability.failed`; reasoning recalls cognitive episodes and standing directives (it retained each turn's episode until D97 left that to reflection), the identity recalls and remembers its adopted name, and the conversation recalls and remembers a relationship's memories (decisions D44 to D47, [plan](plans/phase3-memory.mdscript.md)). Reflection and behaviors use it in phase 5 |
| 4 | Bot core: body and cognition as SCXML, abilities as invoked children, the original's pydantic event models as `event:schema` JSON Schemas, `bot.*` names unchanged | Done through the script-validator cutover: the body `bots/floppy/floppy.scx`; `abilities/` cognition (autonomy seam, intuition, reasoning), communication (conversation, speaking in text mode); the `bot` and `conversation` namespaces and the decision elements; event contracts and runtime `event:schema` checks; no `<script>` (decisions D33 to D43, [Phase 4 conventions](#phase-4-conventions)). Identity, the phase 5 seams, and the saved live examples finish it ([plan](plans/phase4-bot-core.mdscript.md)) |
| 5 | Behaviors and routines: learned behaviors authored by the builder bot, validated, simulated, stored in memory, invoked when matched; routines as long-lived children | Done. The cognition runs reflection and routines beside its turns in every bot run (D80; with no database each logs `absent`). Autonomy runs the SpeechHeard seed and then the ACTIVE turn behaviors the stimulus triggers, each by `<invoke srcexpr>` on its row's path (D121) under a 1 s silence bound, and records usage; reflection recalls the episodes and the inventory, selects at most one of `bot.behavior.create`, `change`, `break` through its enabled transitions, revises a behavior with the builder bot (which checks the reference `check()`'s lifetime bounds and forces a routine's first tick) and stores it ACTIVE (or the rejected source DRAFT `validation`), and stores the turn's episode; routines reconciles the ACTIVE persistent rows into a chain of runners that passes `bot.behavior.tick` up. All of it is kept by the Memory ability (decisions D60 to D64, D73 to D75, D80, [plan](plans/phase5-behaviors.mdscript.md)) |
| 6 | Speech, vision, and devices as services (`speech`, `vision`, `phone`, audio) | Done for speech and vision; devices are out of scope. `services/speech` (`floppy_service_speech`) decodes and encodes speech on OpenAI-compatible audio APIs and detects, diarizes and identifies voices on LocalAI's voice APIs (decisions D57, D66); the generate service reads images (D58). The original's hearing leaves, Listening (`Perceiving{Listening, Sensing}`, Sensitivity, Interpretation with the open-speech window; decision D67) and the SpeechHeard seed turn `environment.sound` into the turn's `bot.ability.listening.speech.output` stimulus and then `bot.ability.communication.input`; vocal and Speaking speak audio (D59, superseding D38); vision and Reading are ported, invoked by nothing yet as in the original ([Phase 6 conventions](#phase-6-conventions), [plan](plans/phase6-speech-vision.mdscript.md)) |
| 7 | Defaults and release: PGlite memory, tagged releases with `SHA256SUMS` for repository namespaces and services (releases wait for going public) | Done except publishing. Memory on Postgres: `services/pgsql` behind the unchanged Memory ability, selected by its `database` (a `postgresql://` URI), migrating the original's full head schema in the Postgres dialect through the Alembic-style migration ability, with every ability's SQL portable to both engines and tested on Postgres 16; PGlite is reached over its socket server, not compiled into a component (decisions D48 to D51, D69 to D72). Defaults: README.md's [Defaults](../README.md#defaults) and `.env.example`, checked by `tests/tests/defaults.rs`; there is no default backend, so every run names its endpoint with `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (decision D31). Releases: canonical Linux builds, `SHA256SUMS` and a dry run of the release layout, prepared and checked locally ([release.md](release.md)); no tag, release or upload until the repositories are public ([plan](plans/phase7-pglite-defaults.mdscript.md)) |

## Phase 4 event contracts

Every phase 4 event has one JSON Schema 2020-12 file titled by its name,
extracted from the original's own pydantic models
(`embeddable_json_schema(event_json_schema(E))` for each `hsm.Event` E):
body events in `bots/floppy/schemas/`, ability events in
`abilities/schemas/`, and the full list of the original's domain event names in
`abilities/schemas/floppy-event-names.json`. Documents reference these files
(`event:schema="schemas/<event>.json"`, or
`../../abilities/schemas/<event>.json` from another directory); they never
carry inline copies. A schema reference resolves against its document's
base URI like any relative URI, so each event has exactly one schema file
(decision D125, which replaced the byte-identical twins of D33, D37 and
D65); `bot.reboot` lives with the abilities' events because the cognition
takes it from the reflection too.

The extraction script runs with the original's interpreter; it is kept outside
this repository, with the floppy plans, as
`~/.agents/projects/floppy/scripts/extract_phase4_contracts.py`:

```sh
python3 -I \
  ~/.agents/projects/floppy/scripts/extract_phase4_contracts.py ~/VSCode/stateforward/floppy
```

Mapping rules (the byte-for-byte rule covers event names, not data shapes):

- `title` is the event name and `$schema` is 2020-12; the original's
  descriptions and examples are kept.
- The original's runtime-only fields are dropped (`operation_id`, `generation`,
  `parent_operation_id`, `parent_generation`, `token`, `request_id`,
  `actor_id`, `capability`): SCXML invoke cancellation replaces the
  cancel-token protocol. Fields pydantic marks `SkipJsonSchema` never
  appear.
- Correlated, non-tool events of a processing turn (the body's
  `bot.processing.*` results and the cognition cascade) require
  `turn`, a positive integer the body counts. `bot.ability.cognition.input`
  carries it only on the body's input to the cognition: an ability's
  handoff of a stimulus to the body (the original's `cognition.InputEvent`
  terminal) has no turn yet. The original's cognition
  `TurnData` object collapses into `turn` plus its `input`; a bare list
  payload (`bot.ability.cognition.output`) becomes `{turn, output}`. Tool
  events, whose data a model fills, carry no `turn`.
- floppy fields that the original carried as runtime objects or process
  configuration are added as data, marked `[floppy field]`:
  `bot.ability.cognition.input` requires `stimulus`, the envelope
  `{event, data, id?, source?, target?}` of the event the body took for the
  turn (the original passed the live object, `SkipJsonSchema`), and may carry
  `choices`, the turn's choices the body assembled (its own and each
  ability's `bot.ability.offers`, tagged with the offering ability's
  `target`; the original built its dispatch inventory from live actor
  snapshots), and `environment`, the bot's environment block (D106); the
  bot's `model` (an activation `<param>` its environment gives, D107) goes
  to its model-backed abilities.
- Media is a reference: `environment.sound`'s `audio` and
  `environment.visual`'s `image` are `uri-reference` strings, not base64
  bytes. `timedelta` would map to integer milliseconds and other `bytes` to
  base64url strings; no phase 4 event has either.
- `StimulusData` stays the envelope `{event, data, id?, source?, target?}`.
- A recursive "any JSON value" definition (conversation's
  `MessageContent`) becomes `{}`, which admits the same values, so every
  schema is ref-free.
- A name with several the original models (`bot.ability.failed`) is their
  `anyOf`.
- floppy-only names are marked `[floppy name]` at the start of the
  description (phase 4: `bot.ability.offers`).

`generate.*` and `decision.*` are not copied beside the abilities: they are
service results, never offered as choices, so an ability needs no schema on
the transitions taking them (scxml.rs does not check an invoked service's
events, so their transitions keep a short shape cond such as
`Object(_event.data).reason`). `tests/tests/contracts.rs` checks the set,
the rules, that every reference resolves to its event's one schema file,
and every `.scx` under `abilities/` and
`bots/floppy/` against the document structure rule (decision D39).

## Phase 4 conventions

How phase 4 writes the original machine as SCXML. The decisions behind them are
D33 to D43.

- **Document structure.** One `.scx` per the original state machine, invoked by a
  literal `id` and a literal relative `src`. The bot root only composes
  (states, `<invoke>`s, `<send>`s). No `<script>` and no `function`; each
  `cond` and each `expr` outside `<data>` is at most 160 characters.
  Reusable synchronous logic is a namespace element that writes to a
  `location`; I/O is an invoked service (decision D39).
- **Readiness and offers.** An ability is ready when its first
  `bot.ability.offers` [floppy name] reaches the body; the body activates
  when no expected ability is left in its `waiting` list, and an expected
  ability that ends while active degrades the bot. Each offer is the
  ability's `<decision:choice>` filtered to its own events and made
  model-facing with `<processing:model_facing_schema>` (decisions D35,
  D113).
- **Choices as data.** On each `processing` entry the body collects the
  turn's choices with `<processing:collect_offered_events>` (its own
  `bot.focus_device`, pinned to the turn's focus candidates, and
  `bot.clear_focus`, plus each ability's latest offers and the focused
  device's events, each tagged with its offerer's session id as
  `target`), holds them to 32 KiB, and sends them as
  `bot.ability.cognition.input.choices` [floppy field]. Each stage that
  dispatches in the original (autonomy, intuition, reasoning) sends its own
  selections to their targets with a visible `<foreach>`/`<send
  targetexpr="'#_scxml_' + s.target">` (decision D113); the body logs
  `bot.processing.completed` and sends itself only the selections that
  name no target (the trusted seed's).
- **Per-turn stage children.** The cognition invokes each stage (autonomy,
  intuition, reasoning) per turn with `<param>`s named after the original
  input event's fields and takes its result as `done.invoke.<stage>`
  `<donedata>`; leaving the stage cancels it, which replaces the original's
  cancel-token protocol. State a stage keeps across turns (the intuition's
  `confidence_tuner`) is passed in by `<param>` and returned in `<donedata>`.
- **Partial selections.** A stage that leaves the turn unhandled may still
  hand out selections (the intuition's environment actions on
  `low_confidence` or `deliberative_input`, the reference `_environment_actions`)
  as the `selected` of its unhandled `<donedata>`. The cognition keeps them
  and puts them first in the turn's one `bot.ability.cognition.output`,
  before the handling stage's selections, so the body routes them first;
  a handling-stage selection that repeats one of them (the same event and
  data) is not put in again, since the original had dispatched it before
  reasoning ran, so a turn acts once; the reflection gets the handling
  stage's selections only.
- **Deferral.** A deferring state holds the event with a targetless
  transition whose content is `<event:defer/>` (and any `<log>`); the
  state that handles it takes it by the same transition as on arrival, in
  order, with its name, data and `invokeid` unchanged (D95). A state the
  machine passes through on its way back (speaking's playing and
  recording) defers it too, as the original's does. The routines keep their
  `ticks` queue: its drain drops a tick whose routine left the running set
  (the reference `_drop_stale_tick`).
- **Timers.** The original's operation machines (processing, attachment, hop and
  reply operations) are a `<send delay id>` in the owning state, a
  `<cancel>` on exit, and a sendid or turn cond. They have no document of
  their own.
- **Memory.** Each memory step is a compound state whose first child
  picks by `database` (the `<param>` the body passes, null without
  memory): with a database it invokes the memory ability
  (`abilities/memory/memory.scx`, over `services/sql`) with the original's statements as
  SQL `<data>` and takes `bot.ability.memory.output` (its `event:schema`)
  or `bot.ability.failed` by invoke id; without one it
  answers itself at once through `#_internal` with
  the original's event and logs `<log label="memory" expr="'absent'">`. Either
  way the step ends with the original's event (`bot.ability.reasoning.recalled`,
  `.retained`, `bot.ability.identity.recalled`, `.adopted`), so the
  topology does not change (decision D44).
- **Speaking.** Phase 4 spoke in text mode (decision D38); phase 6
  speaks audio (decision D59, [Phase 6 conventions](#phase-6-conventions)),
  and the words are still logged as `<log label="floppy.say">`.
- **Traces.** Each deterministic test in `body.rs`, `cognition.rs` and
  `communication.rs` compares its whole run with
  `tests/fixtures/phase4/traces/<test>.ndjson` byte for byte; rewrite them
  with `FLOPPY_TRACE_BLESS=1` only for an intended change.

### Document structure map

| The original unit | floppy artifact | kind |
| --- | --- | --- |
| `bot.py` Bot body | `bots/floppy/floppy.scx` | bot root `.scx` |
| `bot.py` attachment Group, `_AttachmentOperation`, `_ProcessingOperation` | the body's `<invoke>`s in `live`, its offers bookkeeping and its delayed `<send>`/`<cancel>` | composition inside the body |
| `attention_bias_for_turn`, `_interrupt_reference`, `_target_device_reference` | `<cognition:attention_bias_for_turn>` (D114) | `namespaces/cognition` element |
| stimulus envelope | assigned member by member with `<assign>` before its `<send>` (was `<bot:stimulus>`, D116) | plain SCXML |
| `without_ignore_selections`, `attention_selection_error` | the ignore sent to the cognition, the focus device's pinned enum and the body's guards (D113) | plain SCXML |
| `model_facing_event_json_schema` and the 32 KiB bound | `<processing:model_facing_schema>` | `namespaces/processing` element |
| `collect_offered_events`, `_dispatch_bot_processing` | `<processing:collect_offered_events>` | `namespaces/processing` element |
| focus pinning | `<decision:pin>` | `namespaces/decision` element |
| `dispatch_selected_events`, `_resolve_target`, `fill_unique_selection_targets` | each stage's `<foreach>`/`<send targetexpr="'#_scxml_' + s.target">`, every choice tagged with its offerer's session id (D113) | plain SCXML |
| `_selection_item_branch`, `dispatch_tool` | `<processing:send>` | `namespaces/processing` element |
| `normalize_confidence`, `selection_confidence` | `<processing:selection_confidence>` | `namespaces/processing` element |
| `ConfidenceTuner` (`observe`, `should_escalate`) | `<intuition:observe>`, `<intuition:escalate>` over the `confidence_tuner` datum (D114) | `namespaces/intuition` elements |
| `model_facing_content`, `InputData.model_facing_content` (reasoning's `ProcessorInput` prompt, its environment block and its budgets) | `<cognition:model_facing_content>` (D114) | `namespaces/cognition` element |
| `_message_for_turn`, `_model_safe_content`, `MessageProvenance` | `<conversation:message_for_turn>` (was `<conversation:commit>`, D115) | `namespaces/conversation` element |
| `Conversation._relationship_for_input` (its identity collection score), `_stable_context_ref`, `_new_relationship_ref` | `<conversation:relationship_for_input>` (D115) | `namespaces/conversation` element |
| `Conversation._participant_for_source`, `_fail_turn`'s participants, `detector_refs`, `_new_track_ref` | `<conversation:participant_for_source>`, `<conversation:fail_turn>`, `<conversation:detector_refs>`, `<conversation:new_track_ref>` (D115) | `namespaces/conversation` elements |
| `_content_for_detector`, `Conversation._complete_turn` | `<conversation:content_for_detector>`, `<conversation:complete_turn>` (D115) | `namespaces/conversation` elements |
| `TurnDetector._ingest_content`, `TurnDetector._on_end` | `<conversation:ingest_content>`, `<conversation:on_end>` over the detector's `turn_content` datum (D115) | `namespaces/conversation` elements |
| `identity/recognition.py` `hears_name`, `identity/value.py` `sorted_identities` | `<identity:hears_name>` (was `<bot:recognize>`), `<identity:sorted_identities>` (D115) | `namespaces/identity` elements |
| memory `Memory.to_json` (`_encode_content`, `value.identities_json`), `OutputData.contents` | `<memory:to_json>`, `<memory:contents>` (D115) | `namespaces/memory` elements |
| behavior `storage.instances_from_behavior_results`, `storage.replace_behavior_clauses` (`inventory.load_behavior`, `store_behavior`) | `<behavior:instances_from_behavior_results>`, `<behavior:replace_behavior_clauses>` (D116) | `namespaces/behavior` elements |
| behavior `instance._lifetime_errors` and the lifetime, `verify._verify_tick`, `_selection_binding_errors` | `<behavior:lifetime_errors>`, `<behavior:verify_tick>` (D116) | `namespaces/behavior` elements |
| behavior `schedule.At.next_after` | `next_after` in `abilities/reflection/reflection.js`, carried by a routine as data (a draft cannot use a namespace element: `<scx:validate>` binds no package, D116) | routine ECMAScript |
| autonomy `behavior_input_payload` | `<autonomy:behavior_input_payload>` (D116) | `namespaces/autonomy` element |
| `cognition.py` Cognition | `abilities/cognition/cognition.scx` | ability `.scx` child |
| `autonomy.py` Autonomy | `abilities/autonomy/autonomy.scx` (phase 5: the inventory's ACTIVE turn behaviors the stimulus triggers run as `<invoke srcexpr>` children of the chain `abilities/candidates_runner/candidates_runner.scx`, D93, each by its row's path, D121) | stage `.scx` child |
| `intuition.py` Intuition | `abilities/intuition/intuition.scx` invoking `services/decision` | stage `.scx` child and service |
| `reasoning.py` Reasoning | `abilities/reasoning/reasoning.scx` invoking `services/generate` | stage `.scx` child and service |
| `communication.py` Communication | `abilities/communication/communication.scx` | ability `.scx` child |
| `conversation.py` Conversation | `abilities/conversation/conversation.scx` | ability `.scx` child |
| `speaking.py` Speaking | `abilities/speaking/speaking.scx` (`idle` → `encoding` → `playing` → `recording`, decision D59) | ability `.scx` child |
| `vocal/speech/encoding.py` SpeechEncoding | `abilities/vocal/vocal.scx`, speaking's child, invoking `services/speech` `encode` | ability `.scx` child and service |
| `identity/identity.py` Identity, `identity/recognition.py` SpeechNameRecognizer | `abilities/identity/identity.scx`, one of the body's input abilities: each `environment.sound` is decoded by `abilities/speech_decoding/speech_decoding.scx` and the transcript matched by `<identity:hears_name>`; text input is matched too (D43); adoption and recall through `abilities/memory/memory.scx` (decisions D43, D44) | ability `.scx` child |
| Reflection, Routines | `abilities/reflection/reflection.scx` (revising through `bots/builder/builder.scx`), `abilities/routines/routines.scx` over the chain `abilities/routines_runner/routines_runner.scx`; long-lived children of every cognition (D80) | ability `.scx` child |
| Behaviors (Starlark) | builder-built `.scx` documents | generated `.scx` |
| Memory and its providers | `abilities/memory/memory.scx` over `services/sql` (SQLite, phase 3) or `services/pgsql` (Postgres, phase 7), migrated by `abilities/migration/migration.scx` with `<migration:plan>` | ability `.scx` child + service |
| `listening` Listening (`Perceiving{Listening, Sensing}`) | `abilities/listening/listening.scx`, the body's input ability | ability `.scx` child |
| `listening` Sensitivity, Interpretation | `abilities/sensitivity/sensitivity.scx`, `abilities/interpretation/interpretation.scx` (listening's children) | ability `.scx` child |
| `hearing` SpeechDecoding, SoundClassification, VoiceDetection, VoiceDiarization, VoiceIdentification | `abilities/speech_decoding/`, `sound_classification/`, `voice_detection/`, `voice_diarization/`, `voice_identification/` (the interpretation's children; all but sound classification invoke `services/speech`) | stage `.scx` child and service |
| `communication/behaviors.py` SpeechHeard seed | `abilities/behaviors/speech_heard/speech_heard.scx`, which autonomy runs before the learned behaviors | turn behavior `.scx` |
| `vision` VisualClassification, `reading` Reading | `abilities/vision/vision.scx` (reading's child), `abilities/reading/reading.scx` (image decoding through `services/generate`); nothing invokes reading, as in the original | ability `.scx` child and service |
| `conversation/turn_detector` TurnDetector | `abilities/turn_detector/turn_detector.scx`, which the conversation invokes once per source of each input (its `turn_exchange`; decisions D76 to D79, [conventions](#turn-detector-conventions)) | ability `.scx` child |
| Conversation's voice-centroid participant matching, devices | not ported: each embedding source is its own participant (D79), and devices are out of scope | open |
| `learning` Learning; memory's ShortTermMemory, LongTermMemory, MemoryConsolidation, AssociativeMemory, MemoryClassification, MemoryGeneration; the body's short-term stimulus register (`stm_memory`); `language/text` TextGeneration | not ported: none is composed by the original's Bot by default (Learning and the memory variants are optional, the body's `stm_memory` defaults to none, and TextGeneration serves only host turns). floppy's memory is the base Memory; its schema still creates `bot_stm_memory` | open |
| payload validators | `event:schema` on the receiving transition plus a short correlation cond | `event:schema` |

### Builder output

A builder-produced behavior or routine is three documents beside each
other (decision D121): `<name>.scx` (its structure), `<name>.json` (its
input events' schemas, `$defs` keyed by event) and `<name>.js` (its
ECMAScript, or empty), which the builder writes through `services/files`
into the run's behaviors directory (`behaviors/`, beside `bots/` and
`abilities/`). Its inventory row's `bot_behavior.source` is the path of
the `.scx`, `../../behaviors/<name>.scx`, and it runs as an SCXML child
invoked by that path (`<invoke id srcexpr="source">`), the one exception
to the literal relative `src` rule. scxml.rs loads a `srcexpr` child's
schema files and scripts against its own base URI, as it is when invoked,
so builder output references `<name>.json#/$defs/<event>` and carries at
most its one root `<script src="<name>.js"/>`; its host checks what it
answers. `abilities/autonomy/autonomy.scx`
runs the ACTIVE turn behaviors the stimulus triggers with `turn` and
`input` and answers with the first one's `<donedata>` `handled` and
`selected` (the phase 5 plan's "Turn Behavior Contract").
`tests/fixtures/phase4/behavior_double.scx` stands in for one;
`tests/tests/contracts.rs` checks the rule on it and
`tests/tests/cognition.rs` writes it into a layout's behaviors directory
and runs it by path from a memory inventory.

## Phase 6 conventions

How phase 6 writes the original's hearing, listening, vocal, speaking, vision and
reading as SCXML. The phase 4 conventions hold; the decisions behind these
are D52, D57 to D59, D66 and D67
([plan](plans/phase6-speech-vision.mdscript.md)).

- **Media by reference.** The original's bytes fields (`SoundData.audio`,
  `VisualData.image`, the encoded speech) are URI references to files in a
  directory the run grants the service that reads or writes them
  (`--dir https://github.com/stateforward/floppy/services/speech=HOST::/media`,
  and the same for `services/generate`); `media/hello.wav` names
  `/media/hello.wav` in the guest. Documents never hold media, and a
  reference outside the grant fails. Voiced audio is signed 16-bit PCM, in
  a WAV container or raw with its rate and channels.
- **Backends are the run's.** The speech package declares no
  `http-origins`, so every operation reaches only what the run names:
  speech decoding and encoding `FLOPPY_SPEECH_BASE_URL` and
  `FLOPPY_SPEECH_API_KEY`, voice detection, diarization and identification
  `FLOPPY_VOICE_BASE_URL` and `FLOPPY_VOICE_API_KEY` (LocalAI's
  `silero-vad`, `pyannote-diarization` and `voice-detect-wespeaker-resnet34`),
  image reading `FLOPPY_BASE_URL` and `FLOPPY_API_KEY`. Without a base
  URL the service answers `bot.ability.failed` ("the backend is not
  configured") and sends nothing
  (decisions D31, D57, D66); without a key it asks a keyless backend with
  no key header (D127). The models are each document's `model`
  `<data>` ([README Defaults](../README.md#speech-and-voice-models)).
- **Hearing stages.** Each of the original's hearing leaves is its own `.scx`
  (`abilities/speech_decoding/`, `sound_classification/`,
  `voice_detection/`, `voice_diarization/`, `voice_identification/`),
  invoked by the interpretation per sound from the stage that needs it and
  answering the original's output event (`bot.ability.hearing.*.output`) or
  `bot.ability.failed`; leaving the stage cancels it, and each stage is
  bounded by the interpretation's `stage_timeout_ms` (the original's 30 s). The
  media steps the original does on bytes in memory (normalizing to PCM, joining
  the open speech, clipping a voice span) are the speech service's
  `normalize`, `append` and `clip` on files (decision D66). Sound
  classification is the original's `KindSoundClassifier`: the sound's `kind`
  label, no model.
- **Interpretation.** The original's machine in full: `Idle`, then `Working`
  (`DetectingVoice`, the open-speech window `HearingSpeech` /
  `FeedingSpeech` / `SpeechEnded`, `ClassifyingSound`, `DiarizingVoice`,
  `IdentifyingVoiceDirect`, `IdentifyingVoice`, `DecodingSpeech`, `HandingOff`). Its stages are
  `<data>` switches named as the original's constructor names them
  (`sound_classifier` and `speech_decoder` on, `diarizer` and
  `voice_classifier` off); voice identification with speech decoding is
  refused, as the original refuses it (decision D67).
- **Listening products.** The listening (`Perceiving{Listening, Sensing}`)
  scores each sound with its sensitivity against the efference copy, then
  interprets it. A product the body takes as a turn's stimulus is decoded
  speech, `bot.ability.listening.speech.output` (`content_type`
  `text/plain`), or labelled non-speech, passed through as
  `environment.sound`; either needs a perceived level of
  `product_threshold_db` (the original's 3 dB). An unlabelled sound without voice
  is dropped, and a sound that cannot be heard is
  `bot.ability.listening.failed`, which the body logs.
- **SpeechHeard.** Autonomy runs the original's trusted seed
  (`abilities/behaviors/speech_heard/speech_heard.scx`) before the learned
  behaviors when the stimulus is `bot.ability.listening.speech.output`;
  speech with a source id is handled with the original's selection
  `bot.ability.communication.input`, which the body routes to itself, so
  heard speech enters the conversation as text does.
- **Speaking audio.** Speaking is the original's `idle` → `encoding` → `playing`
  → `recording` (decision D59): vocal encodes through the speech service
  with the original's encoder defaults (`gemini-3.1-flash-tts-preview`, `Kore`,
  24 kHz mono `audio/pcm`), the efference copy
  `bot.ability.speaking.efference` leaves before the sound (the body
  carries it to each of `listening_abilities`), the words are logged as
  `floppy.say`, the audio is broadcast as `environment.sound` by reference,
  and the outbound message is recorded in the conversation. Every terminal
  failure is `bot.ability.failed {message}`.
- **Vision and reading.** Reading is the original's `Reading{Unfocused,
  Focused{Classifying, DecodingText, DecodingImage,
  PreparingUnreadableOutput, EncodingOutput}}` with vision as its
  long-lived child; image decoding is the generate service with an image
  part (decision D58). Nothing in the bot invokes reading, because the original
  routes no `environment.visual` to it.
- **Tests.** `tests/tests/speech_service.rs` (the service against a fake
  backend on 127.0.0.1, ElevenLabs included, and an ignored live round
  trip), `listening.rs` (the listening under a harness with the real speech
  service on a fake backend: decoding, the open-speech window,
  diarization, identification, failures, timeouts, the efference window),
  `reading.rs` (reading against a generate double), and
  `communication.rs` (a response spoken through the real speech service).

## Turn detector conventions

How the original's TurnDetector (`abilities/communication/conversation/turn_detector`)
and the Conversation's `turn_detector` stage are written as SCXML; the
decisions are D76 to D79.

- **The machine.** `abilities/turn_detector/turn_detector.scx` is the original's
  `idle`, `open`, `paused`, `closing` and `normalizing` with the original's
  guards: a start addressed to its `participant_ref` opens a turn, an
  update, pause, end or start for the open turn (its `turn_ref`,
  `conversation_ref` and `source_participant_ref`) is taken, anything for
  another turn is ignored, and heard speech
  (`bot.ability.listening.speech.output`) with voice spans opens or feeds
  a turn while speech without them pauses it. `open` and `paused` each
  arm `<state>.after` for `end_of_turn_silence_seconds` (0.5 s, at least
  0.01 s) on entry; its expiry closes the turn through `closing`. The
  attachment states are not ported (D14), and the detector announces
  `bot.ability.turn_detector.ready` on start (D76).
- **The product.** `normalizing` answers `bot.ability.turn_detector.turn.complete`
  (texts stripped and joined; audio by reference, in order, D77; the
  modality-neutral content of a content, image or event stimulus) or
  `bot.ability.turn_detector.failed` (`stage` `turn`, D78), then clears
  the turn. There is no decoder (D77).
- **The conversation's turns.** Each input is one turn per source, in
  the original's identity order: `turn_exchange` invokes the detector for the
  source's track, waits for its readiness (`readiness`), sends the start
  carrying the original's `_content_for_detector` stimulus and the end
  (`exchange`), and takes the completed turn; each wait is bounded by
  the original's 5 s (`turn_timeout_ms`). A failure, a timeout, or a start the
  detector's schema refuses answers `bot.ability.conversation.failed`
  `{stage: "turn_detector"}` and commits nothing. The committed message is
  the turns' product, each distinct text once, with the first turn's
  `turn_ref` (D79).
- **The conversation's relationships and memory.** Each input first finds
  its relationship as the original's `_relationship_for_input` does (targetless
  input shares the first ambient relationship, other input matches a
  relationship in either direction, and an ambient relationship whose
  participants include a target matches at the similarity threshold);
  a new one takes the input's `relationship_context_ref`
  (`{"identity_groups":[...]}`) and the `session_ref` `relationship-` and
  that (all of it `<conversation:relationship_for_input>`, D115). The memory is
  kept under the relationship's `context_ref`: `recalling_memory` recalls
  it before the turns and `remembering_memory` remembers the input, as
  the original's `Memory.to_json` writes it, after them, each its own invocation
  of the memory ability bounded by 5 s. The conversation answers
  `bot.ability.conversation.snapshot.request` with
  `bot.ability.conversation.snapshot.output {request_ref, detector_refs}`
  (its participants), and a repeated append (the same id, source and
  target) with its unchanged history. Input that arrives while one is in hand waits in the
  conversation's queue and is taken, in order, on the return to `waiting`
  (the original defers it); before the turns every input without memory was
  committed in the macrostep it arrived in, so none could arrive.
- **Tests.** `tests/tests/turn_detector.rs` runs the detector under
  `tests/fixtures/turn_detector/turn_detector_harness.scx` (assembly,
  ignored turns, failures, heard speech and the end-of-turn silence);
  `tests/tests/communication.rs` runs the conversation's turns
  (`the_conversation_commits_each_message_into_its_history`,
  `a_turn_its_detector_cannot_close_fails_the_input_and_commits_nothing`).

## Environment conventions

- **Running.** A bot runs as a participant of its environment:
  `scx run --env-file <file> --fuel 9223372036854775807
  environments/room/room.scx` (D102, D111). The room passes each bot its
  `environment` (the root's `_sessionid`, D106) and `owned_devices`
  (D105) as `<param>`s, and the bot activates on joining (D107).
- **Broadcast.** Every `environment.sound` and `environment.visual` the
  room receives, from a participant or the host, is logged and delivered
  to every participant that perceives it, the emitter included. A bot's
  speech reaches the room through its communication, which sends the
  sound to `#_scxml_` + the environment's id right after the efference
  copy to the body (D103, amended), so the bot's listening has the copy
  first and withholds its own words (D98). A device sends to `#_parent`.
- **Placement.** `placements[<id>]` is `{position: {x, y, z}, threshold_db}`;
  a sound's `received_level_db` at a participant is `amplitude_db -
  20 log10(max(d, 0.01) / 1)` from the sender's placement (only a placed
  child is an origin), filled per recipient; a participant below its
  threshold, or a sound too quiet for everyone, receives nothing; without
  an amplitude, an origin or a placement the sound is unmeasured and
  heard (D103). A visual reaches everyone.
- **Devices.** A bot reads every held device on processing entry with
  `<device:snapshot>` into `device_snapshots` (a device that is not
  running is not read, D109); the focused device's events are the turn's
  choices, and a selection for it is sent straight to its address (D105).
- **The environment block.** The body sends `environment` `{id, self:
  {state, owned_devices: {<ref>: {ref, id, state, attributes}}}}` on
  `bot.ability.cognition.input`; reasoning renders it as the original's
  `<environment id><self state><owned_devices><device ref id state>`
  block, each snapshot attribute a child element under a sanitized tag,
  and intuition gets it as JSON (D106).

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
