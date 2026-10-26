# Running floppy

floppy has no runtime of its own (decision D14): every bot runs with
scxml.rs's `scx run`. This page covers building the packages, running the
bots against them or against a release, the directories a run grants, and
the memory engines. Each bot's README gives its input events and output
([builder](../bots/builder/README.md#running),
[floppy](../bots/floppy/README.md#running)); [defaults.md](defaults.md)
lists every variable, model and bound.

## Prerequisites

- The Rust toolchain `rust-toolchain.toml` pins, with the `wasm32-wasip2`
  target.
- `wasm-tools` 1.244.0.
- `wasi-virt` at the revision `scripts/build-components.sh` names.
- wasi-sdk 25, with `WASI_SDK_PATH` pointing at it (the sql service compiles
  SQLite with it).

`scripts/build-components.sh` checks each and says how to install what is
missing.

## Build

```console
# The run's variables: copy the example and write the values (defaults.md).
cp .env.example .env
# The directories the runs grant (all ignored by git).
mkdir -p media memory behaviors
# The session components and every namespace and service component, verified,
# published into a dev package cache under target/ (never scx's release
# cache) under the bots' tagless repository URIs.
sh scripts/build-components.sh
# scx, from the subtree, embedding the session components just built.
SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml" \
  cargo install --locked --path scxml.rs/crates/scxml-cli
```

## Run

The builder bot, with the locally built packages:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env \
    --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" \
    bots/builder/builder.scx
```

The floppy bot in its environment, the room, which activates it; say hello:

```console
printf '%s\n' \
  '{"name":"bot.ability.communication.input","data":{"source_ids":["gabe"],"target_ids":["floppy"],"content":"Hi floppy!","content_type":"text/plain"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env \
    --dir https://github.com/stateforward/floppy/services/sql=memory::/memory \
    --wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network \
    --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/speech=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/vision=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" \
    --fuel 9223372036854775807 environments/room/room.scx
```

Talk to the floppy bot out loud from a Mac with
[`scripts/talk.py`](../scripts/talk.py) (decision D240), run in a terminal
that has microphone access, with ffmpeg installed: it runs the room above
under `scx run`, gives each utterance the microphone hears to the room as
`environment.sound`, and plays each sound floppy makes on the speaker
(muting the microphone while it does). A typed line is sent as text, and
`--say FILE` sends a recorded utterance instead. The env file names the
lab's shared model API for both speech variables
(`FLOPPY_SPEECH_BASE_URL=https://api.willen.dev/v1`, and the same for
`FLOPPY_VOICE_BASE_URL`, each key one of its bearer keys; decision D244),
and `FLOPPY_EMOTION_BASE_URL=https://api.willen.dev/emotion/v1` for the
tone floppy hears (decision D247):

```console
python3 scripts/talk.py --env-file .env --scx scx
```

To let a simulated person talk to floppy instead,
[`scripts/talk-live.py`](../scripts/talk-live.py) puts OpenAI's GPT-Live
(`gpt-live-1`, on its Live sessions WebSocket) in the room in place of the
microphone and speaker: GPT-Live's voice is cut into utterances and given
to the room as `environment.sound`, each sound floppy makes is streamed back
to it, and both sides' words are kept in a transcript
(`media/talk-live/transcript.txt`). It needs `OPENAI_API_KEY` in its own
environment (not the env file) and the `websockets` package; `--persona`
and `--opening` set who GPT-Live is and what it says first,
`--floppy-model` floppy's model, and `--minutes` how long it runs:

```console
python3 scripts/talk-live.py --env-file .env --scx scx --minutes 5
```

A bot against floppy's latest GitHub release (once one is published:
[release.md](release.md)) drops `--offline --cache-dir`:

```console
scx run --env-file .env \
  --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
  --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" \
  bots/builder/builder.scx
```

## How a run resolves

A bot's document is all a run needs. Its namespaces and `<invoke type>`s are
tagless repository URIs (`https://github.com/stateforward/floppy/namespaces/<name>`,
`.../services/<name>`), which name the repository's latest release, so the
same document runs unedited against the dev cache (`--offline --cache-dir`)
and against a release. Its `env` attributes decide which `<invoke>` receives
which variable of the env file, and an `<invoke src>` loads by URI (relative
to its document, `file:`, or `https:`) with no flag.

## What a run grants

`scx run` fails closed when a service imports a resource the run does not
grant, so every run grants what its services read and write:

| Directory | Service | Why |
| --- | --- | --- |
| `media/` | `generate` | reads the images a message names |
| `media/` | `speech` | reads what the bot hears and writes what it says |
| `media/` | `vision` | reads the views seeing measures |
| `memory/` | `sql` | the floppy bot's memory file |
| `behaviors/` | `files` | the builder writes each learned behavior there as three documents (`<name>.scx`, `<name>.json`, `<name>.js`), which the documents that run it load by path (decision D121) |

## Memory engines

The floppy bot's memory ability,
[`abilities/memory/memory.scx`](../abilities/memory/memory.scx), runs on one
of two engines with the same contract, picked by the `database` it is given
(decisions D51, D69 to D72):

| `database` | Engine | The run grants |
| --- | --- | --- |
| a file path, such as the bot's `/memory/floppy.sqlite` | [`services/sql`](../services/sql/README.md), SQLite | `--dir https://github.com/stateforward/floppy/services/sql=HOST::/memory` |
| a connection URI, `postgresql://USER@IP:PORT/DBNAME` | [`services/pgsql`](../services/pgsql/README.md), Postgres over its wire protocol: PGlite served beside `scx` (pglite-socket) or any Postgres that accepts the user without a password | `--wasi https://github.com/stateforward/floppy/services/pgsql=inherit-network` |

`scx` binds every service a document names, so a run that binds the memory
ability grants both, as the floppy bot's command does. Before its statements,
the memory ability brings the database to the head revision through the
migration ability
([`abilities/migration/migration.scx`](../abilities/migration/migration.scx),
an Alembic-style upgrade with `<migration:plan>`), with floppy's one
revision, `0001`, in the engine's dialect
([`abilities/memory/sqlite/`](../abilities/memory/sqlite/),
[`abilities/memory/postgresql/`](../abilities/memory/postgresql/)), and
records the version in `alembic_version`. A database whose
`alembic_version` names a revision the history does not hold is refused
untouched (decision D193). The bot ships with SQLite (the body's `database` `<data>`);
Postgres support is tested on Postgres 16.
