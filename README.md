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
| `namespaces/bot/` | the bot body's extension namespace (a Wasm component; synchronous executable content: `bot:attention`, `bot:stimulus`, `bot:recognize`) |
| `namespaces/conversation/` | the conversation's extension namespace (a Wasm component; synchronous executable content: `conversation:commit`) |
| `namespaces/migration/` | Alembic's revision walk as an extension namespace (a Wasm component; synchronous executable content: `migration:plan`) |
| `services/{generate,decision}/` | canonical invoked services (Wasm components: model generation, decisions) |
| `services/{sql,pgsql}/` | the memory engines (Wasm components: one SQLite or Postgres transaction per `<invoke>`, [sql](services/sql/README.md), [pgsql](services/pgsql/README.md)) |
| `services/speech/` | the speech service (a Wasm component: speech decoding and encoding on OpenAI-compatible audio APIs, voice detection, diarization and identification on LocalAI's voice APIs; decisions D57, D66; [speech](services/speech/README.md)) |
| `bots/builder/` | the builder bot: its SCXML document (`builder.scx`), schemas, and saved live examples |
| `bots/floppy/` | the floppy bot: the body (`floppy.scx`, the original's Bot) and its event contracts |
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
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" bots/builder/builder.scx
# The floppy bot: activate it and say hello.
printf '%s\n' '{"name":"bot.activate","data":{}}' \
  '{"name":"bot.ability.communication.input","data":{"source_ids":["gabe"],"target_ids":["floppy"],"content":"Hi floppy!","content_type":"text/plain"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/speech=$PWD/media::/media" bots/floppy/floppy.scx
# A bot, with the packages of floppy's latest GitHub release (once one is
# published: docs/release.md).
scx run --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" bots/builder/builder.scx
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
`memory/` first). Each bot's README gives its input events and output
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
Alembic's `command.upgrade` with `<migration:plan>`), with the original's
revisions `0001` and `0002` in the engine's dialect
([`abilities/memory/sqlite/`](abilities/memory/sqlite/),
[`abilities/memory/postgresql/`](abilities/memory/postgresql/)), and records
the version in Alembic's `alembic_version`, so the original database is used in
place. The bot ships with SQLite (the body's `database` `<data>`); Postgres
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
| `FLOPPY_API_KEY` | the `generate` service (`env:apikey`), as above | yes, as above |
| `TYPESAFE_BASE_URL` | the `decision` service (`env:baseurl`): the builder's triage, the floppy bot's intuition | no: the service's declared origin, `https://api.typesafe.ai` |
| `TYPESAFE_API_KEY` | the `decision` service (`env:apikey`), as above | for the floppy bot: without it every turn fails (`unconfigured`); the builder triages with its fallback |
| `FLOPPY_SPEECH_BASE_URL` | the `speech` service (`env:baseurl`): the floppy bot's speech decoding (listening) and speech encoding (speaking) | yes: without it decoding and speaking fail `unconfigured` |
| `FLOPPY_SPEECH_API_KEY` | the `speech` service (`env:apikey`), as above | yes, as above |
| `FLOPPY_VOICE_BASE_URL` | the `speech` service (`env:baseurl`): the floppy bot's voice detection, diarization and voice identification (listening), on LocalAI's voice APIs | yes: without it every sound the bot hears fails voice detection |
| `FLOPPY_VOICE_API_KEY` | the `speech` service (`env:apikey`), as above | yes, as above |

The live tests read `FLOPPY_LIVE_ENV_FILE` (the env files, joined like
`PATH`) and `FLOPPY_LIVE_MODEL` (default: the model below); the trace tests
read `FLOPPY_TRACE_BLESS`. Those are test settings, not bot variables.

### Model

`Qwen/Qwen3.8-Flash-Next`, the model the maintainer's DGX Sparks serve
(direct vLLM; the CPA gateway lists it as `sparks/qwen3.8-flash-next`). It is
the `model` `<data>` of `bots/builder/builder.scx` (a `builder.request`'s
`model` overrides it), `bots/floppy/floppy.scx` (a `bot.activate`'s `model`
overrides it), `abilities/cognition/cognition.scx`, `abilities/reasoning/reasoning.scx` and
`abilities/reflection/reflection.scx` (each takes its invoker's by `<param>`), and
`abilities/reading/reading.scx` (the original's image decoder).

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
| `bots/builder/builder.scx` | `generate_timeout_ms` | `300000` | one design or author generation (a `builder.request`'s `generate_timeout_ms` overrides it) |
| `bots/builder/builder.scx` | `decision_timeout_ms` | `60000` | one triage decision |
| `bots/builder/builder.scx` | `max_tests` | `8` | the test scripts one draft may carry |
| `bots/builder/builder.scx` | `sim_test_cap_ms` | `120000` | each test script's simulation run |
| `bots/floppy/floppy.scx` | `activation_timeout_ms` | `30000` | activation until every ability offers (the original's 30 s attachment timeout) |
| `bots/floppy/floppy.scx` | `processing_timeout_ms` | `300000` | one turn (the original's 5 min processing timeout) |
| `abilities/cognition/cognition.scx` | `child_operation_timeout_ms` | `30000` | each cascade stage (the original's 30 s) |
| `abilities/intuition/intuition.scx` | `decision_timeout_ms` | `20000` | the intuition's decision |
| `abilities/autonomy/autonomy.scx` | `silence_ms` | `1000` | a behavior's answer (the original's `TURN_SILENCE_BOUND`) |
| `abilities/autonomy/autonomy.scx` | `memory_timeout_ms` | `10000` | one memory transaction |
| `abilities/candidates_runner/candidates_runner.scx` | `memory_timeout_ms` | `10000` | one candidate's memory transaction (autonomy passes its own) |
| `abilities/conversation/conversation.scx` | `memory_timeout_ms` | `5000` | each of the conversation's memory operations, the recall before the turns and the remember after them (the original's 5 s turn stage timeout) |
| `abilities/communication/communication.scx` | `response_timeout_ms` | `6000` | saying one response (the original's 6 s) |
| `abilities/communication/communication.scx` | `encoding_timeout_ms` | `5000` | speaking's speech synthesis (communication passes its own) |
| `abilities/reflection/reflection.scx` | `select_timeout_ms` | `30000` | reflection's select generation (the original's 30 s) |
| `abilities/reflection/reflection.scx` | `revising_timeout_ms` | `30000` | reflection's revising through the builder (the original's 30 s) |
| `abilities/reading/reading.scx` | `stage_timeout_ms` | `30000` | each reading stage (the original's 30 s) |
| `abilities/conversation/conversation.scx` | `turn_timeout_ms` | `5000` | a turn detector's readiness, and then its turn, for each source of an input (the original's 5 s) |
| `abilities/turn_detector/turn_detector.scx` | `end_of_turn_silence_seconds` | `0.5` | the silence that closes an open or paused turn (the original's 0.5 s; at least 0.01 s) |
| `abilities/speaking/speaking.scx` | `encoding_timeout_ms` | `5000` | one speech synthesis (the original's 5 s) |
| `abilities/speaking/speaking.scx` | `recording_timeout_ms` | `5000` | recording the spoken words in the conversation (the original's 5 s) |
| `abilities/listening/listening.scx` | `sensitivity_timeout_ms` | `30000` | scoring one sound (the original's 30 s stage operation timeout) |
| `abilities/interpretation/interpretation.scx` | `stage_timeout_ms` | `30000` | each interpretation stage (the original's 30 s) |
| `abilities/interpretation/interpretation.scx` | `product_threshold_db` | `3.0` | the perceived level a listening product needs (the original's 3 dB) |

The cognition always runs reflection and routines beside its turns, as
The original's bots compose them. The floppy bot's `database` is
`/memory/floppy.sqlite`, so its runs grant the `sql` service a memory
directory; without a database reflection and routines log `absent`.

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
