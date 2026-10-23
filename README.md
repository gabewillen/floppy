# floppy

floppy is bots and the packages they use, run on
[scxml.rs](scxml.rs/README.md); it has no runtime of its own (decision D14).
Every bot is an SCXML machine run by the scxml.rs Wasm engine through
`scx run`, its abilities are invoked children, and its effects (model
generation, decisions, memory, speech, devices) are invoked services:
versioned Wasm components that never block the machine. Extension namespaces
hold only synchronous, quick executable content.

floppy is the successor of the original (a Python bot runtime of hsm state machines
and Starlark behaviors); [docs/port-plan.md](docs/port-plan.md) maps one onto the
other, and [docs/decisions.md](docs/decisions.md) records the decisions that shape it.

## Layout

| Path | What |
| --- | --- |
| `scxml.rs/` | scxml.rs as a squashed git subtree (see [docs/decisions.md](docs/decisions.md) for the source commit) |
| `namespaces/decision/` | the canonical extension namespace (a Wasm component; synchronous executable content: `decision:choice` and the choice elements); the builder's `scx:validate` is scxml.rs's embedded `scx` namespace |
| `namespaces/processing/` | the original's processing module (a Wasm component; synchronous executable content: `processing:model_facing_schema`, `processing:collect_offered_events`, `processing:send`, `processing:selection_confidence`; [processing](namespaces/processing/README.md)) |
| `namespaces/intuition/` | the original's intuition `ConfidenceTuner` (a Wasm component; synchronous executable content: `intuition:observe`, `intuition:escalate`; [intuition](namespaces/intuition/README.md)) |
| `namespaces/cognition/` | the original's cognition module (a Wasm component; synchronous executable content: `cognition:attention_bias_for_turn`, `cognition:model_facing_content`; [cognition](namespaces/cognition/README.md)) |
| `namespaces/conversation/` | the original's conversation module and its turn detector (a Wasm component; synchronous executable content: `conversation:message_for_turn`, `conversation:relationship_for_input`, `conversation:complete_turn`, `conversation:on_end`, ...; [conversation](namespaces/conversation/README.md)) |
| `namespaces/identity/` | the original's identity module (a Wasm component; synchronous executable content: `identity:hears_name`, `identity:sorted_identities`; [identity](namespaces/identity/README.md)) |
| `namespaces/memory/` | the original's memory module (a Wasm component; synchronous executable content: `memory:to_json`, `memory:contents`, `memory:cut_at_word`, `memory:stm_projection`; [memory](namespaces/memory/README.md)) |
| `namespaces/behavior/` | the original's behavior package (a Wasm component; synchronous executable content: `behavior:instances_from_behavior_results`, `behavior:replace_behavior_clauses`, `behavior:lifetime_errors`, `behavior:verify_tick`; [behavior](namespaces/behavior/README.md)) |
| `namespaces/autonomy/` | the original's autonomy module (a Wasm component; synchronous executable content: `autonomy:behavior_input_payload`; [autonomy](namespaces/autonomy/README.md)) |
| `namespaces/migration/` | Alembic's revision walk as an extension namespace (a Wasm component; synchronous executable content: `migration:plan`) |
| `namespaces/device/` | a bot's read of a device it holds (a Wasm component; synchronous executable content: `device:snapshot`, decision D109; [device](namespaces/device/README.md)) |
| `services/{generate,decision}/` | canonical invoked services (Wasm components: model generation, decisions) |
| `services/files/` | the files service (a Wasm component: reads and writes the directory the run grants, where the builder writes a learned behavior's three documents; decision D121; [files](services/files/README.md)) |
| `services/{sql,pgsql}/` | the memory engines (Wasm components: one SQLite or Postgres transaction per `<invoke>`, [sql](services/sql/README.md), [pgsql](services/pgsql/README.md)) |
| `services/speech/` | the speech service (a Wasm component: speech decoding and encoding on OpenAI-compatible audio APIs, voice detection, diarization and identification on LocalAI's voice APIs; decisions D57, D66; [speech](services/speech/README.md)) |
| `services/vision/` | the vision service (a Wasm component: the original's change measures, `byte_identity` and `frame_diff`, on images in the directory the run grants; decision D185; [vision](services/vision/README.md)) |
| `bots/builder/` | the builder bot: its SCXML document (`builder.scx`), schemas, and saved live examples |
| `bots/floppy/` | the floppy bot: the body (`floppy.scx`, the original's Bot) and its event contracts |
| `environments/room/` | the room, the environment the floppy bot runs in: the root session that invokes its bots (and any devices) as participants, routes host input, and delivers sound and visuals (decisions D102 to D111; [room](environments/room/README.md)) |
| `abilities/` | the abilities and stages the bots invoke, each `<name>/<name>.scx` with `<name>/<name>.js` (memory, migration, cognition, communication, listening, ...) |
| `tests/` | `floppy-tests`: integration tests that run the bots, namespaces, and services through `scx run` |
| `scripts/` | build and proof scripts |

## Quick start

Building needs the Rust toolchain `rust-toolchain.toml` pins (with the
`wasm32-wasip2` target), `wasm-tools` 1.244.0, `wasi-virt` at the revision
`scripts/build-components.sh` names, and wasi-sdk 25 with `WASI_SDK_PATH`
pointing at it (the sql service compiles SQLite with it); the script checks
each and says how to install what is missing.

```console
# The run's variables: copy the example and write the values (see Defaults).
cp .env.example .env
# The session components and every namespace and service component, verified,
# with every package published into a dev repository-package cache under
# target/ (never scx's release cache), pinned under the bots' tagless
# repository URIs.
sh scripts/build-components.sh
# scx, from the subtree, embedding the session components just built.
SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml" \
  cargo install --locked --path scxml.rs/crates/scxml-cli
# The builder bot, with the locally built packages.
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx
# The floppy bot in its environment, the room, which activates it: say hello.
printf '%s\n' \
  '{"name":"bot.ability.communication.input","data":{"source_ids":["gabe"],"target_ids":["floppy"],"content":"Hi floppy!","content_type":"text/plain"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/speech=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" \
    --dir "https://github.com/stateforward/floppy/services/vision=$PWD/media::/media" \
    --fuel 9223372036854775807 environments/room/room.scx
# A bot, with the packages of floppy's latest GitHub release (once one is
# published: docs/release.md).
scx run --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" bots/builder/builder.scx
```

A bot's document is all a run needs: its namespaces and `<invoke type>`s are
tagless repository URIs (`https://github.com/stateforward/floppy/namespaces/<name>`,
`.../services/<name>`), which name the repository's latest release, so the
same document runs against the dev cache `scripts/build-components.sh`
fills (`--offline --cache-dir`) and against a release, unedited; its `env`
attributes decide which `<invoke>` receives which variable of the env file,
and an `<invoke src>` loads by URI (relative to its document, `file:`, or
`https:`) with no flag. The `generate` service reads the images a message
names from a directory the run grants, so every run that binds it gives one
`--dir` (create `media/` first; it is ignored by git). The floppy bot's
runs grant the `speech` service the same directory, where it reads what
the bot hears and writes what it says, and the floppy bot's
memory is a file in the directory the run grants the `sql` service (create
`memory/` first). The builder writes each draft, a learned behavior, as
three documents (`<name>.scx`, `<name>.json`, `<name>.js`) into the
directory the run grants the `files` service, `behaviors/` (create it
first; it is ignored by git), where the documents that run a learned
behavior load it by its path (decision D121). Each bot's README gives its input events and output
([bots/builder/README.md](bots/builder/README.md#running),
[bots/floppy/README.md](bots/floppy/README.md#running)).

## Memory engines

The floppy bot's memory is the original's Memory ability,
[`abilities/memory/memory.scx`](abilities/memory/memory.scx), on one of two
engines with the same contract, picked by the `database` it is given
(decisions D51, D69 to D72):

| `database` | Engine | The run grants |
| --- | --- | --- |
| a file path, such as the bot's `/memory/floppy.sqlite` | [`services/sql`](services/sql/README.md), SQLite | `--dir https://github.com/stateforward/floppy/services/sql=HOST::/memory` |
| a connection URI, `postgresql://USER@IP:PORT/DBNAME` | [`services/pgsql`](services/pgsql/README.md), Postgres over its wire protocol: PGlite served beside `scx` (pglite-socket) or any Postgres that accepts the user without a password | `--wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network` |

`scx` binds every service a document names, so a run that binds the Memory
ability grants both, as the quick start does. Before its statements, the
Memory ability brings the database to the head revision through the
migration ability
([`abilities/migration/migration.scx`](abilities/migration/migration.scx),
Alembic's `command.upgrade` with `<migration:plan>`), with floppy's
revisions `0001`, `0002` and `0003` in the engine's dialect
([`abilities/memory/sqlite/`](abilities/memory/sqlite/),
[`abilities/memory/postgresql/`](abilities/memory/postgresql/)), and records
the version in Alembic's `alembic_version`, so every floppy database upgrades
in place. `0003_stm_turns` is the original's `0002_stm_turns`; the original
rewrote its `0001` in place, so a database the original's current Alembic
wrote is refused (D165, proposed). The bot ships with SQLite (the body's `database` `<data>`); Postgres
support is tested on Postgres 16.

## Defaults

The defaults floppy ships, in one place. Each is a top-level `<data>` of the
document named, so a parent that invokes the document overrides it with a
`<param>` of the same name (SCXML 6.4); the run-time overrides are listed.
`tests/tests/defaults.rs` checks this section against the documents and
`.env.example`.

### Variables

[`.env.example`](.env.example) names every variable a bot reads, without
values. No package has a default model endpoint (decision D31).

| Variable | Read by | Required |
| --- | --- | --- |
| `FLOPPY_BASE_URL` | the `generate` service (`env:baseurl`): the builder's design and author stages, the floppy bot's reasoning | yes: without it a generation fails `unconfigured` |
| `FLOPPY_API_KEY` | the `generate` service (`env:apikey`), as above | for a keyed backend: without it the request carries no key header, for a keyless one (decision D127) |
| `FLOPPY_STREAM` | the `generate` service (`env:stream`): whether the model endpoint streams; `true` or `false` | no: unset (the default), no call is streamed and each model-bound stage is bounded by its backstop alone; `true` makes reasoning's, reflection's and the builder's calls streamed and bounded by inactivity once they stream (decision D101). The CPA gateway does not stream (it answers a streamed request with keep-alive comments only), the maintainer's vLLM does |
| `TYPESAFE_BASE_URL` | the `decision` service (`env:baseurl`): the builder's triage, the floppy bot's intuition | no: the service's declared origin, `https://api.typesafe.ai`, when a key is held; without `TYPESAFE_API_KEY` a held base URL is asked keyless, with no authorization header (decision D150) |
| `TYPESAFE_API_KEY` | the `decision` service (`env:apikey`), as above | for the floppy bot, unless `TYPESAFE_BASE_URL` names a keyless endpoint: with neither, every turn fails (`unconfigured`); the builder triages with its fallback |
| `FLOPPY_SPEECH_BASE_URL` | the `speech` service (`env:baseurl`): the floppy bot's speech decoding (listening) and speech encoding (speaking) | yes: without it decoding and speaking fail `unconfigured` |
| `FLOPPY_SPEECH_API_KEY` | the `speech` service (`env:apikey`), as above | for a keyed backend, as `FLOPPY_API_KEY` |
| `FLOPPY_VOICE_BASE_URL` | the `speech` service (`env:baseurl`): the floppy bot's voice detection, diarization and voice identification (listening), on LocalAI's voice APIs | yes: without it every sound the bot hears fails voice detection |
| `FLOPPY_VOICE_API_KEY` | the `speech` service (`env:apikey`), as above | for a keyed backend, as `FLOPPY_API_KEY` |

The live tests read `FLOPPY_LIVE_ENV_FILE` (the env files, joined like
`PATH`) and `FLOPPY_LIVE_MODEL` (default: the model below); the trace tests
read `FLOPPY_TRACE_BLESS`. Those are test settings, not bot variables.

### Model

`Qwen/Qwen3.8-Flash-Next`, the model the maintainer's DGX Sparks serve
(direct vLLM; the CPA gateway lists it as `sparks/qwen3.8-flash-next`). It is
the `model` `<data>` of `bots/builder/builder.scx` (a `builder.request`'s
`model` overrides it), `bots/floppy/floppy.scx` (its environment's `model`
`<param>` overrides it), `abilities/cognition/cognition.scx`, `abilities/reasoning/reasoning.scx` and
`abilities/reflection/reflection.scx` (each takes its invoker's by `<param>`), and
`abilities/reading/reading.scx` (the original's image decoder).

A bot composes the chat intuition tier only when its environment gives an
`intuition_model` `<param>` (none by default, as the reference's bot
composes intuition only when given a model; decision D144): the cognition
then asks that chat model for the turn before reasoning.

### Speech and voice models

The listening's hearing leaves name the speech service's models:
`abilities/voice_detection/voice_detection.scx`'s `model` is
`silero-vad`, `abilities/voice_diarization/voice_diarization.scx`'s is
`pyannote-diarization`,
`abilities/voice_identification/voice_identification.scx`'s is
`voice-detect-wespeaker-resnet34` (LocalAI's names for the models the original runs
locally), and `abilities/speech_decoding/speech_decoding.scx`'s
`provider` is `gemini` and its `model` `gemini-3.5-flash` (the original's
Gemini speech decoder default). `abilities/speaking/speaking.scx`'s
`provider` is `gemini`, its `model` `gemini-3.1-flash-tts-preview` and its
`voice` `Kore`, the original's speech encoder defaults, which it passes to
vocal for the speech service (vocal's own are `null`: its invoker names
them). Both directions therefore call the Gemini API, so
`FLOPPY_SPEECH_BASE_URL` is its version root,
`https://generativelanguage.googleapis.com/v1beta` (decision D94); a
`provider` of `null` is the OpenAI-compatible API (`whisper-1`, `tts-1`).

### Timeouts and bounds

| Document | `<data>` | Default | What it bounds |
| --- | --- | --- | --- |
| `bots/builder/builder.scx` | `generate_timeout_ms` | `30000` | one design or author generation without progress, once its reply streams: the first progress arms it, each later one re-arms it (decision D101; a `builder.request`'s `generate_timeout_ms` overrides it) |
| `bots/builder/builder.scx` | `generate_ceiling_ms` | `1200000` | one design or author generation in all, the backstop, and the only bound of a reply that is not streamed (decision D101; a `builder.request`'s `generate_ceiling_ms` overrides it) |
| `bots/builder/builder.scx` | `max_completion_tokens` | `8192` | the tokens of one design or author reply, reasoning included: a model that reasons on without answering fails `truncated` (decision D101) |
| `bots/builder/builder.scx` | `decision_timeout_ms` | `60000` | one triage decision |
| `bots/builder/builder.scx` | `max_tests` | `8` | the test scripts one draft may carry |
| `bots/builder/builder.scx` | `sim_test_cap_ms` | `120000` | each test script's simulation run |
| `bots/floppy/floppy.scx` | `activation_timeout_ms` | `30000` | activation until every ability offers (the original's 30 s attachment timeout) |
| `bots/floppy/floppy.scx` | `firmware_timeout_ms` | `30000` | the firmware's start until its first offers, once every ability is ready (the original's attachment bound; decision D160) |
| `bots/floppy/floppy.scx` | `processing_timeout_ms` | `300000` | one turn (the original's 5 min processing timeout) |
| `bots/floppy/floppy.scx` | `reading_turns_timeout_ms` | `1000` | reading the short-term register's recent turns before each turn; the turn starts without them after it (floppy's) |
| `abilities/cognition/cognition.scx` | `child_operation_timeout_ms` | `30000` | each cascade stage without progress (the original's 30 s): autonomy's and the decision layer's from their start, reasoning's once its reply streams (the first progress arms it, each later one re-arms it); live streamed replies had at most 6.2 s between chunks (decision D101; the bot's `bounds` param's `child_operation_timeout_ms` overrides it) |
| `abilities/cognition/cognition.scx` | `child_operation_ceiling_ms` | `240000` | each cascade stage in all, the backstop and the only bound of a reasoning reply that is not streamed, inside the body's `processing_timeout_ms` with the stages before reasoning (decision D101; the bot's `bounds` param's `child_operation_ceiling_ms` overrides it) |
| `abilities/cognition/cognition.scx` | `decision_timeout_ms` | `20000` | the decision layer's own bound on its decision, passed to it as its `decision_timeout_ms` param; a decision not made in time cascades the turn to reasoning (the reference's D-R-36); the bot's `bounds` param's `reflex_timeout_ms` is the reflex step's bound, which fails the turn |
| `abilities/intuition/intuition.scx` | `decision_timeout_ms` | `20000` | the decision layer's decision when its invoker passes no bound |
| `abilities/autonomy/autonomy.scx` | `silence_ms` | `1000` | a behavior's answer (the original's `TURN_SILENCE_BOUND`) |
| `abilities/autonomy/autonomy.scx` | `memory_timeout_ms` | `10000` | one memory transaction |
| `abilities/candidates_runner/candidates_runner.scx` | `memory_timeout_ms` | `10000` | one candidate's memory transaction (autonomy passes its own) |
| `abilities/conversation/conversation.scx` | `memory_timeout_ms` | `5000` | each of the conversation's memory operations, the recall before the turns and the remember after them (the original's 5 s turn stage timeout) |
| `abilities/communication/communication.scx` | `response_timeout_ms` | `36000` | saying one response (speaking's 30 s synthesis bound plus the original's 6 s; decision D98) |
| `abilities/communication/communication.scx` | `encoding_timeout_ms` | `30000` | speaking's speech synthesis (communication passes its own; decision D98) |
| `abilities/reflection/reflection.scx` | `select_timeout_ms` | `30000` | reflection's select generation without progress once its reply streams (the original's 30 s; decision D101) |
| `abilities/reflection/reflection.scx` | `select_ceiling_ms` | `600000` | reflection's select generation in all, the backstop (decision D101) |
| `abilities/reflection/reflection.scx` | `revising_timeout_ms` | `30000` | reflection's revising through the builder without progress while a builder generation streams: its first progress arms it, each later one re-arms it, and the builder's next stage disarms it (decision D101; the bot's `bounds` param's `revising_timeout_ms` overrides it) |
| `abilities/reflection/reflection.scx` | `revising_ceiling_ms` | `1800000` | reflection's revising in all, the backstop (live revisions on the maintainer's vLLM ran past 600 s while making progress; decision D101; the bot's `bounds` param's `revising_ceiling_ms` overrides it) |
| `abilities/identity/identity.scx` | `recall_attempt_timeout_ms` | `1000` | one attempt to recall the adopted name from memory, asked again after `recall_retry_delay_ms` (the original's 1 s and 100 ms) |
| `abilities/identity/identity.scx` | `bring_up_timeout_ms` | `30000` | recalling the name until memory answers, then the identity wakes with the session's name and says so (the original's 30 s attachment timeout; the body passes half its `activation_timeout_ms`) |
| `abilities/persona/persona.scx` | `recall_attempt_timeout_ms` | `1000` | one attempt to recall the self-written description from memory, asked again after `recall_retry_delay_ms` (the original's 1 s and 100 ms) |
| `abilities/persona/persona.scx` | `bring_up_timeout_ms` | `30000` | recalling the description until memory answers, then the persona wakes with none and says so (the original's 30 s attachment timeout; the body passes half its `activation_timeout_ms`) |
| `abilities/memory/memory.scx` | `operation_timeout_ms` | `60000` | one memory operation, its migration and transaction together, which then fails with kind `timeout` (the original's 60 s operation timeout) |
| `abilities/stm/stm.scx` | `open_timeout_ms` | `30000` | opening the short-term register (its database migrated), after which it is degraded (the original's 30 s attachment timeout) |
| `abilities/reading/reading.scx` | `stage_timeout_ms` | `30000` | each reading stage (the original's 30 s) |
| `abilities/conversation/conversation.scx` | `turn_timeout_ms` | `5000` | a turn detector's readiness, and then its turn, for each source of an input (the original's 5 s) |
| `abilities/turn_detector/turn_detector.scx` | `end_of_turn_silence_seconds` | `0.5` | the silence that closes an open or paused turn (the original's 0.5 s; at least 0.01 s) |
| `abilities/speaking/speaking.scx` | `encoding_timeout_ms` | `30000` | one speech synthesis: Gemini answers only once the whole clip is synthesized, which outlasts the original's 5 s for a sentence, so the original's 30 s stage operation timeout (decision D98) |
| `abilities/speaking/speaking.scx` | `recording_timeout_ms` | `5000` | recording the spoken words in the conversation (the original's 5 s) |
| `abilities/listening/listening.scx` | `sensitivity_timeout_ms` | `30000` | scoring one sound (the original's 30 s stage operation timeout) |
| `abilities/interpretation/interpretation.scx` | `stage_timeout_ms` | `30000` | each interpretation stage (the original's 30 s) |
| `abilities/interpretation/interpretation.scx` | `recognize_timeout_ms` | `2000` | identification's extra time past the stage timeout (2 s; decision D177) |
| `abilities/interpretation/interpretation.scx` | `product_threshold_db` | `3.0` | the perceived level a listening product needs (the original's 3 dB) |
| `abilities/seeing/seeing.scx` | `measure_timeout_ms` | `2000` | one change measurement through the vision service (the original's 2 s) |
| `abilities/seeing/seeing.scx` | `measure_every_ms` | `500` | the pace between measurements, whatever the frame rate (the original's 500 ms) |
| `abilities/seeing/seeing.scx` | `settle_after_ms` | `5000` | the settle after every sight: at most one sight per settle, plus looks (the original's 5 s) |
| `abilities/seeing/seeing.scx` | `max_view_age_ms` | `60000` | how long a held view stays fresh (the original's 60 s) |
| `abilities/seeing/seeing.scx` | `stage_timeout_ms` | `30000` | each object detection or description call (the original's 30 s) |

The bot's `bounds` param may also bound a cognition tier's step on its own (the reference's
`_tier_timeouts`, [floppy fields]): `reflex_timeout_ms` bounds the learned behaviors and the decision
layer (a step bound shorter than the decision layer's own `decision_timeout_ms` fails the turn before
the decision cascades), `intuition_timeout_ms` the chat tier,
and `reasoning_timeout_ms` reasoning, each as both the step's inactivity bound and its backstop; a tier
with none takes `child_operation_timeout_ms` and `child_operation_ceiling_ms`. A step that runs out of
time or fails fails only its turn, which is handed back to the cognition as a
`bot.ability.cognition.step.timed_out` or `bot.ability.cognition.step.failed` stimulus.

The bot's `bounds` param may also give reasoning's model context, `max_context_tokens` (none by default:
nothing is fitted), and the tokens reserved for its reply, `completion_reserve_tokens` (0), [floppy fields]:
each prompt is then fitted to the context as the reference's `ContextFit` fits it (decision D143).

The cognition always runs reflection and routines beside its turns, as
The original's bots compose them. The floppy bot's `database` is
`/memory/floppy.sqlite`, so its runs grant the `sql` service a memory
directory; without a database reflection and routines log `absent`.
Its `traits`, which give it its persona, are `['warm', 'patient',
'curious', 'honest']`, the original's companion example's owner traits
(the original composes a persona only from traits, and the floppy bot
always has one); an environment's `<invoke>` may give others as a
`<param>`, one to 24 traits of at most 200 characters each.

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
