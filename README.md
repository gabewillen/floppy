# nemobot

nemobot is a bot runtime built on [scxml.rs](scxml.rs/README.md): every bot is
an SCXML machine run by the scxml.rs Wasm engine, its abilities are invoked
children, and its effects (model generation, decisions, memory, speech,
devices) are invoked services: versioned Wasm components that never block
the machine. Extension namespaces hold only synchronous, quick executable
content.

nemobot is the successor of mosfet (a Python bot runtime of hsm state machines
and Starlark behaviors); [docs/port-plan.md](docs/port-plan.md) maps one onto the
other, and [docs/decisions.md](docs/decisions.md) records the decisions that shape it.

## Layout

| Path | What |
| --- | --- |
| `scxml.rs/` | scxml.rs as a squashed git subtree (see [docs/decisions.md](docs/decisions.md) for the source commit) |
| `host/` | `nemobot-host`: the Rust embedder around the scxml.rs Wasmtime session host |
| `namespaces/{scxml,decision}/` | canonical extension namespaces (Wasm components; synchronous executable content: `sx:validate`, `decision:menu`) |
| `services/{generate,decision}/` | canonical invoked services (Wasm components: model generation, decisions) |
| `bots/builder/` | the builder bot's SCXML documents |
| `scripts/` | build and proof scripts |

Contributor rules are in [AGENTS.md](AGENTS.md).

## License

Dual-licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
