# floppy design and phases

A floppy bot is a **body** (lifetime, environment I/O, stimulus fan-out)
and a **cognition** (interpretation, behavior selection, dispatch). Every
stateful concern is a state machine, abilities are the composition unit,
devices face the environment, services own the backends, and learned
**behaviors** are SCXML documents the builder bot writes.

Every machine is an SCXML document run by the scxml.rs engine inside the
`scxml-machine` Wasm component, and every effect is a versioned Wasm
component (an extension namespace or an invoked service). The decisions
behind this plan are in [decisions.md](decisions.md); the rules that bind
it are in [../AGENTS.md](../AGENTS.md). What is still to build is the
handoff's [open work](handoff.md#open-work).

## Concept map

| Concept | floppy |
| --- | --- |
| A bot (body and cognition) | One SCXML document per bot under `bots/<name>/`, invoked by its environment, which `scx run` runs as the live root session (floppy has no runtime of its own, decision D14; the bot is a participant, decision D102) |
| An environment (membership, holdings, placement, broadcast) | An environment document, `environments/<name>/<name>.scx` (today `environments/environment/environment.scx`), run as the root session; its invoked children are its participants, joining when the invoke starts and leaving when it ends; it routes host input by its payload (D108), broadcasts every `environment.sound` and `environment.visual` to the participants that perceive it (placement, the free-field law and thresholds, D103), and logs the events its bots send it (decisions D102 to D111) |
| State machines | SCXML documents (table below) |
| Abilities (listening, speaking, cognition, memory, ...) | Invoked children, one `.scx` per machine under `abilities/<name>/`: `<invoke id="<name>" type="scxml" src="../../abilities/<name>/<name>.scx">` from the body (a stage or sub-ability from its ability: `src="../<name>/<name>.scx"`), literal `id` and `src`, talking to the parent through `#_parent` and to the child through `#_<invokeid>` (decisions D33, D34) |
| Attaching and detaching abilities | No attachment events: the `<invoke>` lifecycle is attach (start on state entry) and detach (cancel on state exit, `done.invoke.<id>` on completion); a group's attach is the body's parallel `<invoke>`s joined on each member's `bot.ability.ready` (decisions D6 and D242) |
| Devices | SCXML documents an environment invokes (`devices/<name>/<name>.scx`), any real I/O inside them an invoked service; a bot holds them through the environment's `holdings` and `owned_devices`, reads them with `<device:snapshot>` and acts on them at their address (decisions D105, D109, D110); none is built yet (the tests use the lamp fixture) |
| Backends | Invoked services (`services/`) for every effect: anything asynchronous, I/O-bound, or long-running (decision D13); extension namespaces (`namespaces/`) only for synchronous, quick executable content (table below) |
| Learned behaviors | Validator-gated SCXML: the builder bot authors the behavior as an SCXML document, `scx:validate` gates it, and a simulation as an invoked child proves it before it is stored; a matched behavior runs as an invoked child |
| Persistent routines (schedules across turns) | Long-lived invoked children whose schedules are self-rescheduling `<send delay>` loops; each tick is an event to the parent, which decides on it in a new turn |
| Event data contracts | JSON Schema files referenced by `event:schema` on the transitions that take the event; scxml.rs checks them where the event is created (`<send>`, `<raise>`, a send between sessions) and on host input (decision D41) |
| Payload checks | `event:schema` only; a `cond` correlates (which child, which turn, which timer) and never checks a payload's shape (decision D42) |
| Synchronous helpers (attention bias, envelopes, choice assembly and compaction, routing, confidence tuner, message commit) | floppy namespace elements that write to a `location` (`namespaces/conversation`, `namespaces/decision`, `namespaces/processing`, `namespaces/intuition`, `namespaces/cognition`, `namespaces/identity`, `namespaces/memory`, `namespaces/behavior`, `namespaces/autonomy`; decisions D40, D113, D114, D115, D116) |
| Model-facing event choices and payload filling | `<decision:choice>` (the `decision` namespace) writes the enabled transitions with their `event:schema`, read through `session-snapshot-host`, as canonical choices (`<decision:offers>` reads another session's, with the effort its `offerable` data declares, D206, D242); the `decision` service, invoked with it, asks TypeSafe `system_one` `choice` questions (D243), fills the offered events' data to their schemas and validates it, and emits `decision.selected` with the one chosen event and its confidence, or `decision.abstain` with the tier a pass hands the turn to (the deliberative handoff is never offered) |
| Event names `bot.*`, `bot.ability.*`, `phone.*` | Kept byte for byte: never renamed or re-cased |
| Telemetry (`telemetry`) | scxml.rs's OpenTelemetry facade (guests through the telemetry WIT; the native runtime, `scx`, through scxml.rs's own) |

## State machine constructs in SCXML

| Construct | SCXML |
| --- | --- |
| A machine | `<scxml name="..." initial="...">`, one document per machine |
| A state | `<state id>`; parallel regions become `<parallel>` |
| An initial transition | the `initial` attribute, or `<initial><transition target/></initial>` when the initial transition has effects |
| A final state | `<final id>`, with `<donedata>` for a result |
| A transition and its target | `<transition event="..." target="...">`; a targetless transition is internal (`type="internal"` where the source is compound) |
| A guard | `cond` (ECMAScript data model) |
| An effect | executable content (synchronous and quick): `<assign>`, `<raise>`, `<send>`, `<log>`, `<foreach>`, `<if>`, and extension elements (`decision:*`, `processing:*`, `conversation:*`, `behavior:*`, `scx:validate`); never `<script>` in a floppy document (decision D39); an effect that does I/O becomes an `<invoke>` of a service |
| Entry and exit actions | `<onentry>` / `<onexit>` |
| An activity | `<invoke>`: started on entry, cancelled on exit, `done.invoke.<id>` and `error.*` on completion or failure; every model call (the `generate` and `decision` services) is one |
| Deferral | `<event:defer/>` (scxml.rs `namespaces/event`, decision D93): in a deferring state, a targetless transition on E holds the event; it comes back unchanged, in order, on the internal queue once the state is exited or a state inside it that handles E is entered (see [Phase 4 conventions](#phase-4-conventions)) |
| A timeout after a duration | `<send event="<state>.after" delay="..." id="<state>-after"/>` on entry, `<cancel sendid="<state>-after"/>` on exit, and a transition on the event |
| A periodic schedule | a self-rescheduling loop: the transition on the tick re-sends the tick with the same `delay` |
| A wall-clock schedule | `<send delayexpr>` computed from the absolute time and the data model's clock (`Date.now()` in ECMAScript); a routine's `at` schedule (time of day, weekdays, IANA zone) gets its next occurrence from `next_after(at, Date.now())` in its own data model, over the zone's POSIX TZ rule, since the ECMAScript data model has no time zone database (reflection's persistent routine contract gives the function) |
| A choice | a transient state with eventless `<transition cond>` branches (the last unconditional) |
| Shallow history | `<history type="shallow">` |
| Starting and stopping a machine | `<invoke>` / reaching a `<final>` (or the invoking state exiting, which cancels the child) |
| An event and its type | event descriptors plus `event:schema` contracts |

Retries, timeouts, and bounded loops stay in the topology (counters in the
data model, `<send delay>` timers, states for each attempt), never in host or
component code.

## Backends and services

| Backend | floppy | Phase |
| --- | --- | --- |
| OpenAI-compatible text generation | `services/generate`, OpenAI backend (`env:apikey`, `env:baseurl`) | 1 |
| Gemini text | `services/generate` through an OpenAI-compatible base URL | any (by `FLOPPY_BASE_URL`) |
| TypeSafe (`system_one` Choice/Score) | `services/decision`, TypeSafe backend (`TYPESAFE_API_KEY`) | 1 |
| A local tool-calling reflex model | `services/decision` backend (the `Backend` trait) | later |
| SQLite memory | `services/sql`, SQLite compiled into the component (it imports `wasi:filesystem`; the run grants a directory) under `abilities/memory/memory.scx`, migrated by `abilities/migration/migration.scx` (decisions D44 to D51) | 3 |
| Postgres memory | `services/pgsql`, the Postgres wire protocol over `wasi:sockets/tcp` (the run grants `inherit-network`) to PGlite served beside `scx` (pglite-socket) or any Postgres, behind the same sql contract and the same Memory ability, which picks it for a `postgresql://` `database`; tested on Postgres 16 (decisions D51, D69 to D72) | 7 |
| Speech decoding and encoding | `services/speech`: `decode` on `/audio/transcriptions` and `encode` on `/audio/speech` of an OpenAI-compatible backend (`FLOPPY_SPEECH_BASE_URL`, decision D57) | 6 |
| ElevenLabs | `services/speech` `encode` with `<param name="provider" expr="'elevenlabs'">` (its `text-to-speech` API, decision D57); no floppy document selects it yet | 6 |
| Voice activity, diarization and voice embeddings | `services/speech` voice operations `detect`, `diarize`, `identify` on LocalAI's `/vad`, `/audio/diarization`, `/voice/embed` (`FLOPPY_VOICE_BASE_URL`, decision D66), with the media steps `normalize`, `append`, `clip` on files | 6 |
| Image decoding | through `services/generate` with an image part (decision D58); the visual classifier is rule-based in `abilities/vision/vision.scx` | 6 |
| LiveKit | the `phone` device service | out of scope (devices) |

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
| 1 | Namespaces and services: `scxml` (now scxml.rs's embedded `scx` namespace, decision D17: `scx:validate` wrapping the validator component, returning canonical diagnostics and fixes), `generate` (OpenAI text and structured output), `decision` (enabled-transition candidates, TypeSafe ranking, schema fill and validation before emit) | `scxml` namespace implemented, then moved to scxml.rs as `scx` (`scx:validate`); `services/generate` implemented (one Chat Completions call per `<invoke>`, text or the calls of one function tool (asked for with `"tool_choice": "required"` and returned as tool calls, not `response_format`, decision D112), `generate.result`/`generate.failed`, never blocking the session; `env:apikey`/`env:baseurl`); `decision` split per D13: the namespace's `decision:choice` (snapshot choices of `event:schema` transitions whose guards evaluate to true) and `services/decision` (request kinds `choice`, `predicate`, `score`, D120; grounded per-member fill, validation against the choices' schema before emit, `decision.abstain`, `decision.failed`; TypeSafe `system_one` backend behind the `Backend` trait, `env:apikey`/`env:baseurl`) |
| 2 | Builder bot: `bots/builder/` SCXML (intake, design, author, `scx:validate`, fix or regenerate with bounded retries as states, simulate the draft as an invoked child via `#_<invokeid>`, ending in the final states `done`/`failed` whose `<donedata>` carries the result), run with `scx run` | `bots/builder/builder.scx` implemented (contract schemas in `bots/builder/schemas`; design and author invoke the generate service and triage invokes the decision service on the `decision:choice` choices, each with `<finalize>`/`done.invoke` and a `builder.timeout` that preempts and cancels the call; cond-based fallback; simulation as an invoked child); runs with `scx run --env-file .env bots/builder/builder.scx` (decisions D14, D16; [bots/builder/README.md](../bots/builder/README.md#running)); deterministic harness in `tests/tests/builder_bot.rs`, which runs that command (including a timeout firing while a generation is in flight); live results in [live-proof.md](live-proof.md) |
| 3 | Memory service on SQLite | Done: `services/sql` (`floppy_service_sql`), one SQLite transaction of statements per `<invoke>` on a database file in a directory the run grants (`--dir URI=HOST::/memory`), answering `sql.result` or `sql.failed`; the Memory ability, `abilities/memory/memory.scx`, which migrates the memory schema and applies statements through it, answering `bot.ability.memory.output` or `bot.ability.failed`; reasoning recalls cognitive episodes and standing directives (it retained each turn's episode until D97 left that to reflection), the identity recalls and remembers its adopted name, and the conversation recalls and remembers a relationship's memories (decisions D44 to D47, [plan](plans/phase3-memory.mdscript.md)). Reflection and behaviors use it in phase 5 |
| 4 | Bot core: body and cognition as SCXML, abilities as invoked children, event contracts as `event:schema` JSON Schemas, `bot.*` names unchanged | Done through the script-validator cutover: the body `bots/floppy/floppy.scx`; `abilities/` cognition (autonomy seam, intuition, reasoning), communication (conversation, speaking in text mode); the `bot` and `conversation` namespaces and the decision elements; event contracts and runtime `event:schema` checks; no `<script>` (decisions D33 to D43, [Phase 4 conventions](#phase-4-conventions)). Identity, the phase 5 seams, and the saved live examples finish it ([plan](plans/phase4-bot-core.mdscript.md)) |
| 5 | Behaviors and routines: learned behaviors authored by the builder bot, validated, simulated, stored in memory, invoked when matched; routines as long-lived children | Done. The cognition runs reflection and routines beside its turns in every bot run (D80; with no database each logs `absent`). Autonomy runs the SpeechHeard seed and then the ACTIVE turn behaviors the stimulus triggers, each by `<invoke srcexpr>` on its row's path (D121) under a 1 s silence bound, and records usage; reflection recalls the episodes and the inventory, selects at most one of `bot.behavior.create`, `change`, `break` through its enabled transitions, revises a behavior with the builder bot (which checks the lifetime bounds and forces a routine's first tick) and stores it ACTIVE (or the rejected source DRAFT `validation`), and stores the turn's episode; routines reconciles the ACTIVE persistent rows into a chain of runners that passes `bot.behavior.tick` up. All of it is kept by the Memory ability (decisions D60 to D64, D73 to D75, D80, [plan](plans/phase5-behaviors.mdscript.md)) |
| 6 | Speech, vision, and devices as services (`speech`, `vision`, `phone`, audio) | Done for speech and vision; devices are out of scope. `services/speech` (`floppy_service_speech`) decodes and encodes speech on OpenAI-compatible audio APIs and detects, diarizes and identifies voices on LocalAI's voice APIs (decisions D57, D66); the generate service reads images (D58). The hearing leaves, Listening (`Perceiving{Listening, Sensing}`, Sensitivity, Interpretation with the open-speech window; decision D67) and the SpeechHeard seed turn `environment.sound` into the turn's `bot.ability.listening.speech.output` stimulus and then `bot.ability.communication.input`; vocal and Speaking speak audio (D59, superseding D38); vision and Reading are built, invoked by nothing yet ([Phase 6 conventions](#phase-6-conventions), [plan](plans/phase6-speech-vision.mdscript.md)) |
| 7 | Defaults and release: PGlite memory, tagged releases with `SHA256SUMS` for repository namespaces and services (releases wait for going public) | Done except publishing. Memory on Postgres: `services/pgsql` behind the unchanged Memory ability, selected by its `database` (a `postgresql://` URI), migrating the full head schema in the Postgres dialect through the Alembic-style migration ability, with every ability's SQL portable to both engines and tested on Postgres 16; PGlite is reached over its socket server, not compiled into a component (decisions D48 to D51, D69 to D72). Defaults: docs/defaults.md's [Defaults](defaults.md) and `.env.example`, checked by `tests/tests/defaults.rs`; there is no default backend, so every run names its endpoint with `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (decision D31). Releases: canonical Linux builds, `SHA256SUMS` and a dry run of the release layout, prepared and checked locally ([release.md](release.md)); no tag, release or upload until the repositories are public ([plan](plans/phase7-pglite-defaults.mdscript.md)) |

## Phase 4 event contracts

Every phase 4 event has one JSON Schema 2020-12 file titled by its name:
body events in `bots/floppy/schemas/`, ability events in
`abilities/schemas/`, and the full list of domain event names in
`abilities/schemas/floppy-event-names.json`. Documents reference these files
(`event:schema="schemas/<event>.json"`, or
`../../abilities/schemas/<event>.json` from another directory); they never
carry inline copies. A schema reference resolves against its document's
base URI like any relative URI, so each event has exactly one schema file
(decision D125, which replaced the byte-identical twins of D33, D37 and
D65); `bot.reboot` lives with the abilities' events because the cognition
takes it from the reflection too.

Schema rules (the byte-for-byte rule covers event names, not data shapes):

- `title` is the event name and `$schema` is 2020-12; a schema carries
  its descriptions and examples.
- No schema carries operation bookkeeping (operation ids, generations,
  cancel tokens, request or actor ids): SCXML invoke cancellation ends an
  operation.
- Correlated, non-tool events of a processing turn (the body's
  `bot.processing.*` results and the cognition cascade) require
  `turn`, a positive integer the body counts. `bot.ability.cognition.input`
  carries it only on the body's input to the cognition: an ability's
  handoff of a stimulus to the body has no turn yet. A turn's data is
  `turn` plus its `input`; a bare list payload (`bot.ability.cognition.output`) becomes `{turn, output}`. Tool
  events, whose data a model fills, carry no `turn`.
- Fields that carry runtime context or configuration as data are marked
  `[floppy field]`:
  `bot.ability.cognition.input` requires `stimulus`, the envelope
  `{event, data, id?, source?, target?}` of the event the body took for the
  turn, and may carry
  `choices`, the turn's choices the body assembled (its own and what
  each ability offers, read from its live session that turn, tagged with
  the offering ability's `target`), and `environment`, the bot's environment block (D106); the
  bot's `model` (an activation `<param>` its environment gives, D107) goes
  to its model-backed abilities.
- Media is a reference: `environment.sound`'s `audio` and
  `environment.visual`'s `image` are `uri-reference` strings, not base64
  bytes. `timedelta` would map to integer milliseconds and other `bytes` to
  base64url strings; no phase 4 event has either.
- `StimulusData` stays the envelope `{event, data, id?, source?, target?}`.
- Any JSON value (a conversation message's content) is `{}`, so every
  schema is ref-free.
- An event with several shapes (`bot.ability.failed`) is their `anyOf`.
- floppy-only names are marked `[floppy name]` at the start of the
  description (phase 4: `bot.ability.offers`, removed by D242).

`generate.*` and `decision.*` are not copied beside the abilities: they are
service results, never offered as choices, so an ability needs no schema on
the transitions taking them (scxml.rs does not check an invoked service's
events, so their transitions keep a short shape cond such as
`Object(_event.data).reason`). `tests/tests/contracts.rs` checks the set,
the rules, that every reference resolves to its event's one schema file,
and every `.scx` under `abilities/` and
`bots/floppy/` against the document structure rule (decision D39).

## Phase 4 conventions

How phase 4 writes the bot's machines as SCXML. The decisions behind them are
D33 to D43.

- **Document structure.** One `.scx` per state machine, invoked by a
  literal `id` and a literal relative `src`. The bot root only composes
  (states, `<invoke>`s, `<send>`s). No `<script>` and no `function`; each
  `cond` and each `expr` outside `<data>` is at most 160 characters.
  Reusable synchronous logic is a namespace element that writes to a
  `location`; I/O is an invoked service (decision D39).
- **Readiness and offers.** An ability is ready when its
  `bot.ability.ready`, sent once when it starts serving, reaches the body;
  the body activates when no expected ability is left in its `waiting`
  list, and an expected ability that ends while active degrades the bot.
  Nothing pushes offers: each turn the body reads what each ability offers
  from its live session with `<decision:offers>` (its enabled `event:schema`
  transitions without a cond that its `offerable` data declares), made
  model-facing with `<processing:model_facing_schema>` and dropped for the
  turn when over 32 KiB (decisions D113, D242).
- **Choices as data.** On each `processing` entry the body collects the
  turn's choices with `<processing:collect_offered_events>` (its own
  `bot.focus_device`, which its guard takes only for one of the turn's focus candidates (there is no
  `bot.clear_focus`: focus stays on a device until the bot focuses
  another, D155), plus what each ability offers and the focused
  device's events, each tagged with its offerer's session id as
  `target`), holds them to 32 KiB, and sends them as
  `bot.ability.cognition.input.choices` [floppy field]. Each stage that
  selects (autonomy, intuition, reasoning) sends its own
  selections to their targets with a visible `<foreach>`/`<send
  targetexpr="'#_scxml_' + s.target">` (decision D113); the body logs
  `bot.processing.completed` and sends itself only the selections that
  name no target (the trusted seed's).
- **Per-turn stage children.** The cognition invokes each stage (autonomy,
  intuition, reasoning) per turn with `<param>`s named after its input
  event's fields and takes its result as `done.invoke.<stage>`
  `<donedata>`; leaving the stage cancels it. State a stage keeps across turns (the intuition's
  `confidence_tuner`) is passed in by `<param>` and returned in `<donedata>`.
- **Effort ceilings.** Each tier is offered only the choices within its
  effort ceiling (D196, built by D206): an offerer declares an action's
  effort on its `<decision:choice effort>` (0 without one), the cognition
  frames each stage's choices with `<cognition:within_ceiling>` against the
  tier's ceiling (`decision_effort_ceiling` 0, `intuition_effort_ceiling` 1,
  which the bounds override; reasoning's is the whole frame), and the
  heavier actions are the stage's `held` `<param>`, rendered as the
  `<held>` block of its model-facing content, so it can only hand them on.
- **Partial selections.** A stage that leaves the turn unhandled may still
  act: on `low_confidence` or `deliberative_input` intuition sends the
  environment actions itself before the turn escalates, never the ignore
  or the handoff, and its donedata carries `acted`, what it sent (D113,
  D150). The turn's output is the handling stage's selections, which the
  reflection gets.
- **Deferral.** A deferring state holds the event with a targetless
  transition whose content is `<event:defer/>` (and any `<log>`); the
  state that handles it takes it by the same transition as on arrival, in
  order, with its name, data and `invokeid` unchanged (D95). A state the
  machine passes through on its way back (speaking's playing and
  recording) defers it too. The routines keep their `ticks` queue: its
  drain drops a tick whose routine left the running set.
- **Timers.** Operations (processing, attachment, hop and reply) are a
  `<send delay id>` in the owning state, a
  `<cancel>` on exit, and a sendid or turn cond. They have no document of
  their own.
- **Memory.** Each memory step is a compound state whose first child
  picks by `memory` (the `<param>` the body passes, the address of the
  bot's one memory, null without memory): with a memory it sends the
  memory (`abilities/memory/memory.scx`, the body's long-lived child over
  `services/sql`, decision D266) a `bot.ability.memory.input {id,
  statements}` request with its statements as SQL `<data>` and takes
  `bot.ability.memory.output` (its `event:schema`) or `bot.ability.failed`
  by the request's id; without one it
  answers itself at once through `#_internal` with
  the same event and logs `<log label="memory" expr="'absent'">`. Either
  way the step ends with its event (`bot.ability.reasoning.recalled`,
  `.retained`, `bot.ability.identity.recalled`, `.adopted`,
  `bot.ability.persona.recalled`, `.revised`), so the
  topology does not change (decision D44).
- **Speaking.** Phase 4 spoke in text mode (decision D38); phase 6
  speaks audio (decision D59, [Phase 6 conventions](#phase-6-conventions)),
  and the words are still logged as `<log label="floppy.say">`.
- **Traces.** Each deterministic test in `body.rs`, `cognition.rs` and
  `communication.rs` compares its whole run with
  `tests/fixtures/phase4/traces/<test>.ndjson` byte for byte; rewrite them
  with `FLOPPY_TRACE_BLESS=1` only for an intended change.

### Document structure map

| Unit | floppy artifact | kind |
| --- | --- | --- |
| The body | `bots/floppy/floppy.scx` | bot root `.scx` |
| Attaching the abilities, the processing operation | the body's `<invoke>`s in `live`, its readiness bookkeeping and its delayed `<send>`/`<cancel>` | composition inside the body |
| The turn's attention bias (the interrupt and the target device) | `<cognition:attention_bias_for_turn>` (D114) | `namespaces/cognition` element |
| stimulus envelope | assigned member by member with `<assign>` before its `<send>` (was `<bot:stimulus>`, D116) | plain SCXML |
| Removing the ignore, checking the focus | the ignore sent to the cognition and the body's guards (D113, D243) | plain SCXML |
| The model-facing event schema and the 32 KiB bound | `<processing:model_facing_schema>` | `namespaces/processing` element |
| Collecting the turn's offered events | `<processing:collect_offered_events>` | `namespaces/processing` element |
| Sending selections to their targets | each stage's `<foreach>`/`<send targetexpr="'#_scxml_' + s.target">`, every choice tagged with its offerer's session id (D113) | plain SCXML |
| The dispatch tool's schema | `<processing:send>` | `namespaces/processing` element |
| A selection's confidence | `<processing:selection_confidence>` | `namespaces/processing` element |
| The confidence tuner (observe, escalate) | `<intuition:observe>`, `<intuition:escalate>` over the `confidence_tuner` datum (D114) | `namespaces/intuition` elements |
| Reasoning's prompt: its input, its environment block and its budgets | `<cognition:model_facing_content>` (D114) | `namespaces/cognition` element |
| A committed message and its provenance | `<conversation:message_for_turn>` (was `<conversation:commit>`, D115) | `namespaces/conversation` element |
| The relationship an input belongs to (its identity collection score), its context reference | `<conversation:relationship_for_input>` (D115) | `namespaces/conversation` element |
| A source's participant, a failed turn's participants, the detector refs, a track ref | `<conversation:participant_for_source>`, `<conversation:fail_turn>`, `<conversation:detector_refs>`, `<conversation:new_track_ref>` (D115) | `namespaces/conversation` elements |
| The detector's content, the completed turn | `<conversation:content_for_detector>`, `<conversation:complete_turn>` (D115) | `namespaces/conversation` elements |
| The turn detector's content and its end | `<conversation:ingest_content>`, `<conversation:on_end>` over the detector's `turn_content` datum (D115) | `namespaces/conversation` elements |
| Hearing the bot's name, sorting identities | `<identity:hears_name>` (was `<bot:recognize>`), `<identity:sorted_identities>` (D115) | `namespaces/identity` elements |
| A memory row's JSON (its identities included), a result's contents | `<memory:to_json>`, `<memory:contents>` (D115) | `namespaces/memory` elements |
| Loading and storing behavior rows | `<behavior:instances_from_behavior_results>`, `<behavior:replace_behavior_clauses>` (D116) | `namespaces/behavior` elements |
| A behavior's lifetime and its bounds, the forced tick's check | `<behavior:lifetime_errors>`, `<behavior:verify_tick>` (D116) | `namespaces/behavior` elements |
| A turn behavior's live-turn check (one run on the turn it is learned from, its selections' data checked against the live values) | `<behavior:verify_apply>` (D131) | `namespaces/behavior` element |
| A routine's next `at` occurrence | `next_after` in `abilities/revision/revision.js`, carried by a routine as data (a draft cannot use a namespace element: `<scx:validate>` binds no package, D116) | routine ECMAScript |
| A behavior's input | `<autonomy:behavior_input_payload>` (D116) | `namespaces/autonomy` element |
| Cognition | `abilities/cognition/cognition.scx` | ability `.scx` child |
| Rest and consolidation (reinforcing each person's representative voice, merging duplicate people) | the cognition's `idle { awake, resting }`: on resting it invokes `abilities/consolidation/consolidation.scx` over the memory ability with `<conversation:fold_voices>`, `<conversation:people_to_merge>` and `<conversation:merged_context_refs>`; input cancels it (D195, D205) | ability `.scx` child + namespace |
| Autonomy | `abilities/autonomy/autonomy.scx` (phase 5: the inventory's ACTIVE turn behaviors the stimulus triggers run one at a time as `<invoke srcexpr>` children of its `running_service` state, D93, D273 and D274, each by its row's path, D121) | stage `.scx` child |
| Intuition | `abilities/intuition/intuition.scx` invoking `services/decision` | stage `.scx` child and service |
| Reasoning | `abilities/reasoning/reasoning.scx` invoking `services/generate` | stage `.scx` child and service |
| Communication | `abilities/communication/communication.scx` | ability `.scx` child |
| Conversation | `abilities/conversation/conversation.scx` | ability `.scx` child |
| Speaking | `abilities/speaking/speaking.scx` (`idle` → `encoding` → `playing` → `recording`, decision D59) | ability `.scx` child |
| Speech encoding | `abilities/vocal/vocal.scx`, speaking's child, invoking `services/speech` `encode` | ability `.scx` child and service |
| Identity and name recognition | `abilities/identity/identity.scx`, one of the body's input abilities: each `environment.sound` is decoded by `abilities/speech_decoding/speech_decoding.scx` and the transcript matched by `<identity:hears_name>`; text input is matched too (D43); adoption and recall through `abilities/memory/memory.scx` (decisions D43, D44) | ability `.scx` child |
| Persona (the owner's traits, the bot's description) | `abilities/persona/persona.scx`, the body's child: the owner's traits are its `traits` `<param>`, the self-written description is recalled and revised as a self-fact through `abilities/memory/memory.scx`, and `bot.ability.persona.output` goes to the body on waking and after each revision | ability `.scx` child |
| Reflection, Learning, Routines | `abilities/reflection/reflection.scx` and `abilities/learning/learning.scx` (each revising through `abilities/revision/revision.scx`, which invokes `bots/builder/builder.scx`, D148 and D149), `abilities/routines/routines.scx` over the chain `abilities/routines_runner/routines_runner.scx`; long-lived children of every cognition (D80) | ability `.scx` child |
| Learned behaviors | builder-built `.scx` documents | generated `.scx` |
| Memory and its engines | `abilities/memory/memory.scx`, the body's one long-lived memory (decision D266), over `services/sql` (SQLite, phase 3) or `services/pgsql` (Postgres, phase 7), migrated once by `abilities/migration/migration.scx` with `<migration:plan>` | ability `.scx` child + service |
| Listening (`Perceiving{Listening, Sensing}`) | `abilities/listening/listening.scx`, the body's input ability | ability `.scx` child |
| Sensitivity, interpretation | `abilities/sensitivity/sensitivity.scx`, `abilities/interpretation/interpretation.scx` (listening's children) | ability `.scx` child |
| Hearing: speech decoding, sound classification, voice detection, diarization and identification | `abilities/speech_decoding/`, `sound_classification/`, `voice_detection/`, `voice_diarization/`, `voice_identification/` (the interpretation's children; all but sound classification invoke `services/speech`) | stage `.scx` child and service |
| The SpeechHeard seed | `abilities/behaviors/speech_heard/speech_heard.scx`, which autonomy runs before the learned behaviors | turn behavior `.scx` |
| Visual classification, reading | `abilities/vision/vision.scx` (reading's child), `abilities/reading/reading.scx` (image decoding through `services/generate`); nothing invokes reading | ability `.scx` child and service |
| Seeing (`perceiving{watching, next, measuring, pacing, interpreting, settling}`) | `abilities/seeing/seeing.scx`, the body's input ability for `environment.visual` while its `seeing_enabled` switch is on (D189); views are interpreted one after another (D187) | ability `.scx` child |
| Change measures (`byte_identity`, `frame_diff`) | `services/vision` (`floppy_service_vision`), which seeing invokes once per measured view (D185); an embedding change measure and a cascade of measures are not built | service |
| Object detection, description | `abilities/object_detection/object_detection.scx`, `abilities/description/description.scx`: seeing's per-view stage children over `services/generate`, whose cancel ends a stuck stage (D186) | stage `.scx` child and service |
| Face detection, face identity | not built until decision 6 (D188) | open |
| The turn detector | `abilities/turn_detector/turn_detector.scx`, which the conversation invokes once per source of each input (its `turn_exchange`; decisions D76 to D79, [conventions](#turn-detector-conventions)) | ability `.scx` child |
| People: matching a voice to a person in long-term memory | the conversation's `identifying_memory`, `numbering_memory` and `tagging_memory` over the memory ability (`bot_person`, `bot_person_embedding`, each person's compact representative in `bot_person_centroid`, new people numbered from `bot_sequence`) with `<conversation:people_for_input>`; faces and the bot's own voice are not built (D194, D204, D205) | ability `.scx` child + namespace |
| The body's short-term register (`bot.register`) of admitted stimuli and of heard and said turns | `abilities/stm/stm.scx`, the body's long-lived child `stm` over the memory ability (`bot_stm_memory`, `bot_stm_turn`); the body builds each admitted stimulus's record with `<memory:stm_projection>`, and a turn's producer cuts its words with `<memory:cut_at_word>` (no child writes turns yet) | ability `.scx` child |
| Learning; short- and long-term memory, consolidation into episodes and links, associative memory, memory classification and generation; text generation for host turns | not built: Learning is #16, associative memory and consolidation are the [graph memory plan](plans/graph-memory.md). floppy's memory is the Memory ability | open |
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

How phase 6 writes hearing, listening, vocal, speaking, vision and
reading as SCXML. The phase 4 conventions hold; the decisions behind these
are D52, D57 to D59, D66 and D67
([plan](plans/phase6-speech-vision.mdscript.md)).

- **Media by reference.** Media fields (`environment.sound`'s `audio`,
  `environment.visual`'s `image`, the encoded speech) are URI references to files in a
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
  `<data>` ([Defaults](defaults.md#speech-and-voice-models)).
- **Hearing stages.** Each hearing leaf is its own `.scx`
  (`abilities/speech_decoding/`, `sound_classification/`,
  `voice_detection/`, `voice_diarization/`, `voice_identification/`),
  invoked by the interpretation per sound from the stage that needs it and
  answering its output event (`bot.ability.hearing.*.output`) or
  `bot.ability.failed`; leaving the stage cancels it, and each stage is
  bounded by the interpretation's `stage_timeout_ms` (30 s). The media
  steps (normalizing to PCM, joining the open speech, clipping a voice
  span) are the speech service's `normalize`, `append` and `clip` on
  files (decision D66). Sound classification reads the sound's `kind`
  label, with no model.
- **Interpretation.** `Idle`, then `Working`
  (`DetectingVoice`, the open-speech window `HearingSpeech` /
  `FeedingSpeech` / `SpeechEnded`, `ClassifyingSound`, `DiarizingVoice`,
  `IdentifyingVoiceDirect`, `IdentifyingVoice`, `DecodingSpeech`, `HandingOff`). Its stages are
  `<data>` switches (`sound_classifier` and `speech_decoder` on,
  `diarizer` and `voice_classifier` off); voice identification with
  speech decoding is refused (decision D67).
- **Listening products.** The listening (`Perceiving{Listening, Sensing}`)
  scores each sound with its sensitivity against the efference copy, then
  interprets it. A product the body takes as a turn's stimulus is decoded
  speech, `bot.ability.listening.speech.output` (`content_type`
  `text/plain`), or labelled non-speech, passed through as
  `environment.sound`; either needs a perceived level of
  `product_threshold_db` (3 dB). An unlabelled sound without voice
  is dropped, and a sound that cannot be heard is
  `bot.ability.listening.failed`, which the body logs.
- **SpeechHeard.** Autonomy runs the trusted seed
  (`abilities/behaviors/speech_heard/speech_heard.scx`) before the learned
  behaviors when the stimulus is `bot.ability.listening.speech.output`;
  speech with a source id is handled with the selection
  `bot.ability.communication.input`, which the body routes to itself, so
  heard speech enters the conversation as text does.
- **Speaking audio.** Speaking is `idle` → `encoding` → `playing`
  → `recording` (decision D59): vocal encodes through the speech service
  with the encoder defaults (`gemini-3.1-flash-tts-preview`, `Kore`,
  24 kHz mono `audio/pcm`), the efference copy
  `bot.ability.speaking.efference` leaves before the sound (the body
  carries it to each of `listening_abilities`), the words are logged as
  `floppy.say`, the audio is broadcast as `environment.sound` by reference,
  and the outbound message is recorded in the conversation. Every terminal
  failure is `bot.ability.failed {message}`.
- **Vision and reading.** Reading is `Reading{Unfocused,
  Focused{Classifying, DecodingText, DecodingImage,
  PreparingUnreadableOutput, EncodingOutput}}` with vision as its
  long-lived child; image decoding is the generate service with an image
  part (decision D58). Nothing in the bot invokes reading:
  `environment.visual` goes to seeing (D189).
- **Tests.** `tests/tests/speech_service.rs` (the service against a fake
  backend on 127.0.0.1, ElevenLabs included, and an ignored live round
  trip), `listening.rs` (the listening under a harness with the real speech
  service on a fake backend: decoding, the open-speech window,
  diarization, identification, failures, timeouts, the efference window),
  `reading.rs` (reading against a generate double), and
  `communication.rs` (a response spoken through the real speech service).

## Turn detector conventions

How the turn detector and the conversation's `turn_detector` stage are
written as SCXML; the decisions are D76 to D79.

- **The machine.** `abilities/turn_detector/turn_detector.scx` is
  `idle`, `open`, `paused`, `closing` and `normalizing`, guarded so: a start addressed to its `participant_ref` opens a turn, an
  update, pause, end or start for the open turn (its `turn_ref`,
  `conversation_ref` and `source_participant_ref`) is taken, anything for
  another turn is ignored, and heard speech
  (`bot.ability.listening.speech.output`) with voice spans opens or feeds
  a turn while speech without them pauses it. `open` and `paused` each
  arm `<state>.after` for `end_of_turn_silence_seconds` (0.5 s, at least
  0.01 s) on entry; its expiry closes the turn through `closing`. It has
  no attachment states (D14), and the detector announces
  `bot.ability.turn_detector.ready` on start (D76).
- **The product.** `normalizing` answers `bot.ability.turn_detector.turn.complete`
  (texts stripped and joined; audio by reference, in order, D77; the
  modality-neutral content of a content, image or event stimulus) or
  `bot.ability.turn_detector.failed` (`stage` `turn`, D78), then clears
  the turn. There is no decoder (D77).
- **The conversation's turns.** Each input is one turn per source, in
  identity order: `turn_exchange` invokes the detector for the
  source's track, waits for its readiness (`readiness`), sends the start
  carrying `<conversation:content_for_detector>`'s stimulus and the end
  (`exchange`), and takes the completed turn; each wait is bounded by
  5 s (`turn_timeout_ms`). A failure, a timeout, or a start the
  detector's schema refuses answers `bot.ability.conversation.failed`
  `{stage: "turn_detector"}` and commits nothing. The committed message is
  the turns' product, each distinct text once, with the first turn's
  `turn_ref` (D79).
- **The conversation's relationships and memory.** Each input first finds
  its relationship (targetless
  input shares the first ambient relationship, other input matches a
  relationship in either direction, and an ambient relationship whose
  participants include a target matches at the similarity threshold);
  a new one takes the input's `relationship_context_ref`
  (`{"identity_groups":[...]}`) and the `session_ref` `relationship-` and
  that (all of it `<conversation:relationship_for_input>`, D115). The memory is
  kept under the relationship's `context_ref`: `recalling_memory` recalls
  it before the turns and `remembering_memory` remembers the input, as
  `<memory:to_json>` writes it, after them, each its own invocation
  of the memory ability bounded by 5 s. The conversation answers
  `bot.ability.conversation.snapshot.request` with
  `bot.ability.conversation.snapshot.output {request_ref, detector_refs}`
  (its participants), and a repeated append (the same id, source and
  target) with its unchanged history. Input that arrives while one is in hand waits in the
  conversation's queue and is taken, in order, on the return to `waiting`;
  before the turns every input without memory was
  committed in the macrostep it arrived in, so none could arrive.
- **Tests.** `tests/tests/turn_detector.rs` runs the detector under
  `tests/fixtures/turn_detector/turn_detector_harness.scx` (assembly,
  ignored turns, failures, heard speech and the end-of-turn silence);
  `tests/tests/communication.rs` runs the conversation's turns
  (`the_conversation_commits_each_message_into_its_exchange`,
  `a_turn_its_detector_cannot_close_fails_the_input_and_commits_nothing`).

## Environment conventions

- **Running.** A bot runs as a participant of its environment:
  `scx run --env-file <file> --fuel 9223372036854775807
  environments/environment/environment.scx` (D102, D111). The environment passes each bot its
  `environment` (the root's `_sessionid`, D106) and `owned_devices`
  (D105) as `<param>`s, and the bot activates on joining (D107).
- **Broadcast.** Every `environment.sound` and `environment.visual` the
  environment receives, from a participant or the host, is logged and delivered
  to every participant that perceives it, the emitter included. A bot's
  speech reaches the environment through its communication, which sends the
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
  `bot.ability.cognition.input`; reasoning renders it as the
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
