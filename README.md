# floppy

floppy is a set of bots, and the packages they use, built as SCXML state
machines. Every bot runs on the [scxml.rs](scxml.rs/README.md) engine through
`scx run`; floppy has no runtime of its own (decision D14).

- **Bots are SCXML documents.** A bot's abilities are invoked child machines.
- **Effects are invoked services.** Model generation, decisions, memory,
  speech and vision are versioned Wasm components that never block the
  machine.
- **Namespaces are quick, synchronous helpers.** Extension elements do no
  I/O.

## Quick start

You need the pinned Rust toolchain, `wasm-tools`, `wasi-virt` and wasi-sdk;
`scripts/build-components.sh` checks each and says how to install what is
missing ([details](docs/running.md#prerequisites)).

```console
cp .env.example .env            # then fill in your endpoints and keys
mkdir -p media memory behaviors # directories the runs grant
sh scripts/build-components.sh  # build and verify every package
SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml" \
  cargo install --locked --path scxml.rs/crates/scxml-cli
```

Then ask the builder bot for a machine:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env \
    --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
    --dir "https://github.com/stateforward/floppy/services/files=$PWD/behaviors::/behaviors" \
    bots/builder/builder.scx
```

[docs/running.md](docs/running.md) has the floppy bot's command, running
against a release, the directories each run grants, and the memory engines.
At minimum a run needs `FLOPPY_BASE_URL`; [docs/defaults.md](docs/defaults.md)
lists every variable, model and timeout.

## Layout

| Path | What |
| --- | --- |
| [`bots/`](bots/) | the [builder](bots/builder/README.md), which writes and tests new SCXML machines, and [floppy](bots/floppy/README.md), the companion bot |
| [`abilities/`](abilities/) | the abilities and stages bots invoke (memory, cognition, communication, listening, seeing, ...) |
| [`environments/`](environments/) | the sessions a bot runs in, such as the [environment](environments/environment/README.md) |
| [`namespaces/`](namespaces/) | extension namespaces, synchronous executable content as Wasm components: [autonomy](namespaces/autonomy/README.md), [behavior](namespaces/behavior/README.md), [cognition](namespaces/cognition/README.md), [conversation](namespaces/conversation/README.md), [decision](namespaces/decision/README.md), [device](namespaces/device/README.md), [identity](namespaces/identity/README.md), [intuition](namespaces/intuition/README.md), [memory](namespaces/memory/README.md), [migration](namespaces/migration/README.md), [processing](namespaces/processing/README.md) |
| [`services/`](services/) | invoked services, Wasm components that do the I/O: [generate](services/generate/README.md), decision, [files](services/files/README.md), [sql](services/sql/README.md), [pgsql](services/pgsql/README.md), [speech](services/speech/README.md), [vision](services/vision/README.md) |
| [`tests/`](tests/) | integration tests that run everything through `scx run` |
| [`scripts/`](scripts/) | build and proof scripts |
| [`scxml.rs/`](scxml.rs/) | the engine, as a squashed git subtree |

## Documentation

- [docs/running.md](docs/running.md): building, running, grants and memory
  engines.
- [docs/defaults.md](docs/defaults.md): every variable, model, timeout and
  bound.
- [docs/decisions.md](docs/decisions.md): the decisions that shape floppy.
- [docs/release.md](docs/release.md): how releases are built and published.
- [docs/handoff.md](docs/handoff.md): current state and work in flight.
- [AGENTS.md](AGENTS.md): the rules every contributor follows.

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
