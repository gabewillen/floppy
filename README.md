# nemobot

nemobot is bots and the packages they use, run on
[scxml.rs](scxml.rs/README.md); it has no runtime of its own (decision D14).
Every bot is an SCXML machine run by the scxml.rs Wasm engine through
`scx run`, its abilities are invoked children, and its effects (model
generation, decisions, memory, speech, devices) are invoked services:
versioned Wasm components that never block the machine. Extension namespaces
hold only synchronous, quick executable content.

nemobot is the successor of mosfet (a Python bot runtime of hsm state machines
and Starlark behaviors); [docs/port-plan.md](docs/port-plan.md) maps one onto the
other, and [docs/decisions.md](docs/decisions.md) records the decisions that shape it.

## Layout

| Path | What |
| --- | --- |
| `scxml.rs/` | scxml.rs as a squashed git subtree (see [docs/decisions.md](docs/decisions.md) for the source commit) |
| `namespaces/decision/` | the canonical extension namespace (a Wasm component; synchronous executable content: `decision:menu`); the builder's `scx:validate` is scxml.rs's embedded `scx` namespace |
| `services/{generate,decision}/` | canonical invoked services (Wasm components: model generation, decisions) |
| `bots/builder/` | the builder bot: its SCXML document (`builder.scx`), schemas, and saved live examples |
| `tests/` | `nemobot-tests`: integration tests that run the bots, namespaces, and services through `scx run` |
| `scripts/` | build and proof scripts |

## Running a bot

```console
# The session components and every namespace and service component, verified,
# with every package published into scx's repository-package cache
# (${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions), so the bots' repository
# URIs resolve locally with no flag.
sh scripts/build-components.sh
# scx, from the subtree, embedding the session components just built.
SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml" \
  cargo install --locked --path scxml.rs/crates/scxml-cli
# A bot.
scx run --env-file .env bots/builder/builder.scx
```

A bot's document is all a run needs: its namespaces and `<invoke type>`s are
repository URIs (`https://github.com/stateforward/nemobot/...@v0.1.0`) that
`scx` resolves from the cache `scripts/build-components.sh` fills, its `env`
attributes decide which `<invoke>` receives which variable of the env file,
and an `<invoke src>` beside it loads with no flag. Each bot's README gives
its input events ([bots/builder/README.md](bots/builder/README.md#running)).

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
