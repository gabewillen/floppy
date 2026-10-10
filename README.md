# floppy

floppy is bots and the packages they use, run on
[scxml.rs](scxml.rs/README.md); it has no runtime of its own (decision D14).
Every bot is an SCXML machine run by the scxml.rs Wasm engine through
`scx run`, its abilities are invoked children, and its effects (model
generation, decisions, memory, speech, devices) are invoked services:
versioned Wasm components that never block the machine. Extension namespaces
hold only synchronous, quick executable content.

floppy is the successor of mosfet (a Python bot runtime of hsm state machines
and Starlark behaviors); [docs/port-plan.md](docs/port-plan.md) maps one onto the
other, and [docs/decisions.md](docs/decisions.md) records the decisions that shape it.

## Layout

| Path | What |
| --- | --- |
| `scxml.rs/` | scxml.rs as a squashed git subtree (see [docs/decisions.md](docs/decisions.md) for the source commit) |
| `namespaces/decision/` | the canonical extension namespace (a Wasm component; synchronous executable content: `decision:choice`); the builder's `scx:validate` is scxml.rs's embedded `scx` namespace |
| `namespaces/bot/` | the bot body's extension namespace (a Wasm component; synchronous executable content: `bot:attention`, `bot:stimulus`) |
| `namespaces/conversation/` | the conversation's extension namespace (a Wasm component; synchronous executable content: `conversation:commit`) |
| `services/{generate,decision}/` | canonical invoked services (Wasm components: model generation, decisions) |
| `bots/builder/` | the builder bot: its SCXML document (`builder.scx`), schemas, and saved live examples |
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
    bots/floppy/floppy.scx
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
`--dir` (create `media/` first; it is ignored by git), and the floppy bot's
memory is a file in the directory the run grants the `sql` service (create
`memory/` first). Each bot's README gives its input events and output
([bots/builder/README.md](bots/builder/README.md#running),
[bots/floppy/README.md](bots/floppy/README.md#running)).

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
`abilities/reading/reading.scx` (mosfet's image decoder).

### Timeouts and bounds

| Document | `<data>` | Default | What it bounds |
| --- | --- | --- | --- |
| `bots/builder/builder.scx` | `generate_timeout_ms` | `300000` | one design or author generation (a `builder.request`'s `generate_timeout_ms` overrides it) |
| `bots/builder/builder.scx` | `decision_timeout_ms` | `60000` | one triage decision |
| `bots/builder/builder.scx` | `max_tests` | `8` | the test scripts one draft may carry |
| `bots/builder/builder.scx` | `sim_test_cap_ms` | `120000` | each test script's simulation run |
| `bots/floppy/floppy.scx` | `activation_timeout_ms` | `30000` | activation until every ability offers (mosfet's 30 s attachment timeout) |
| `bots/floppy/floppy.scx` | `processing_timeout_ms` | `300000` | one turn (mosfet's 5 min processing timeout) |
| `bots/floppy/floppy.scx` | `cancel_timeout_ms` | `5000` | a timed-out turn's cancel acknowledgement (clamped to 100 to 5000) |
| `abilities/cognition/cognition.scx` | `hop_timeout_ms` | `30000` | each cascade stage (mosfet's 30 s) |
| `abilities/intuition/intuition.scx` | `decision_timeout_ms` | `20000` | the intuition's decision |
| `abilities/autonomy/autonomy.scx` | `silence_ms` | `1000` | a behavior's answer (mosfet's `TURN_SILENCE_BOUND`) |
| `abilities/autonomy/autonomy.scx` | `memory_timeout_ms` | `10000` | one memory transaction |
| `abilities/communication/communication.scx` | `response_timeout_ms` | `6000` | saying one response (mosfet's 6 s) |
| `abilities/reflection/reflection.scx` | `select_timeout_ms` | `30000` | reflection's select generation (mosfet's 30 s) |
| `abilities/reading/reading.scx` | `stage_timeout_ms` | `30000` | each reading stage (mosfet's 30 s) |
| `abilities/conversation/conversation.scx` | `max_messages` | `20` | the history that reaches one turn |

The cognition's phase 5 gates are off (`reflection_enabled`,
`routines_enabled`). The floppy bot's `database` is `/memory/floppy.sqlite`,
so its runs grant the `sql` service a memory directory.

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
