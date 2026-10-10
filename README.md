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

## Running a bot

```console
# The session components and every namespace and service component, verified,
# with every package published into a dev repository-package cache under
# target/ (never scx's release cache), pinned under the bots' tagless
# repository URIs.
sh scripts/build-components.sh
# scx, from the subtree, embedding the session components just built.
SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml" \
  cargo install --locked --path scxml.rs/crates/scxml-cli
# A bot, with the locally built packages.
scx run --offline --cache-dir target/components/package-cache --env-file .env --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" bots/builder/builder.scx
# A bot, with the packages of floppy's latest GitHub release (once phase 7
# publishes one).
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
`--dir` (create `media/` first; it is ignored by git). Each bot's README gives
its input events ([bots/builder/README.md](bots/builder/README.md#running)).

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
