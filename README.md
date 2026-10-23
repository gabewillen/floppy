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

Every variable, model, timeout and bound floppy ships is in
[docs/defaults.md](docs/defaults.md).

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
