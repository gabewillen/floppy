# nemobot

nemobot is bots and the packages they use, run on
[scxml.rs](scxml.rs/README.md); it has no runtime of its own (decision D14).
Every bot is an SCXML machine run by the scxml.rs Wasm engine through
`scxml-cli run`, its abilities are invoked children, and its effects (model
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
| `namespaces/{scxml,decision}/` | canonical extension namespaces (Wasm components; synchronous executable content: `sx:validate`, `decision:menu`) |
| `services/{generate,decision}/` | canonical invoked services (Wasm components: model generation, decisions) |
| `bots/builder/` | the builder bot: its SCXML documents, schemas, and `scxml-cli` argument file |
| `tests/` | `nemobot-tests`: integration tests that run the bots, namespaces, and services through `scxml-cli run` |
| `scripts/` | build and proof scripts |

## Running a bot

```console
# The scxml.rs CLI (from the subtree) with the Wasmtime host.
cargo install --locked --path scxml.rs/crates/scxml-cli --features wasmtime
# The session components and every namespace and service component, verified;
# also writes target/components/components.args, an argument file that binds
# every package to its component by SHA-256.
sh scripts/build-components.sh
# A bot: its argument file (bots/<name>/<name>.args) and document.
scxml-cli run @target/components/components.args @bots/builder/builder.args \
  --env-file .env bots/builder/parent.scxml
```

`@FILE` is a scxml-cli argument file: one argument per line, as rustc reads
them. Run from the repository root (the bots' argument files use relative
paths). Each bot's README gives its command and its input events
([bots/builder/README.md](bots/builder/README.md#running)).

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
