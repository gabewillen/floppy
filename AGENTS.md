# Project rules

These rules are normative for every contributor to this repository, whether a
human or an AI coding agent. MUST, SHOULD, and MAY are used as in RFC 2119.
[docs/decisions.md](docs/decisions.md) records why they are what they are, and
[docs/port-plan.md](docs/port-plan.md) maps mosfet onto nemobot.

## Repository and the scxml.rs subtree

- nemobot MUST run bots only as SCXML machines executed by the scxml.rs engine inside the generic `scxml-machine` Wasm component; nemobot code MUST NOT select transitions, implement macrosteps or microsteps, or duplicate any engine semantics.
- `scxml.rs/` is a squashed git subtree of scxml.rs (its source branch and commit are recorded in [docs/decisions.md](docs/decisions.md)). Its own [AGENTS.md](scxml.rs/AGENTS.md) is normative for every change inside it, and a change there MUST go upstream to scxml.rs first (through its own review) and arrive here only through `git subtree pull --prefix scxml.rs <scxml.rs> <branch> --squash`; contributors MUST NOT edit files under `scxml.rs/` in this repository.
- The nemobot Cargo workspace MUST exclude `scxml.rs` and depend on its crates by path; it MUST NOT absorb scxml.rs's workspace, lockfile, or members.
- The workspace MUST remain on Rust `1.98`, edition `2024`, and Cargo resolver `3` (matching scxml.rs) unless an explicit reviewed migration changes all authoritative metadata and proof together. `Cargo.lock` MUST remain committed; shared dependencies MUST resolve to the versions scxml.rs's `Cargo.lock` pins (it was seeded from it).
- Canonical extension namespaces live as crates under `namespaces/<name>/`, canonical invoked services under `services/<name>/`, the embedder under `host/`, and bots as SCXML documents under `bots/<name>/`. Their repository URIs MUST be exactly `https://github.com/stateforward/nemobot/namespaces/<name>@<tag>` and `https://github.com/stateforward/nemobot/services/<name>@<tag>`.
- The SCXML authoring namespace MUST be named exactly `scxml` (never `scxml-toolchain` or another variant); documents MUST bind it with a prefix other than `scxml` (for example `sx`).
- Versions of scxml.rs contracts MUST stay at WIT `@0.2.0` and Machine IR `5` until scxml.rs releases a change; nemobot MUST NOT bump them on its own.

## Components

- Namespaces and services MUST be independently deployable Wasm components behind scxml.rs's versioned WIT contracts (`stateforward:scxml/executable-content-extension`, `stateforward:scxml/invoked-service`), built for `wasm32-wasip2` by `scripts/build-components.sh` with the reproducible release flags, virtualized by the pinned `wasi-virt`, validated by the pinned `wasm-tools`, and checked against an exact per-component import allowlist.
- New component crates, worlds, and artifacts MUST use concise domain names and MUST NOT add a redundant `-component` suffix.
- Package manifests (`stateforward.scxml.extension-package/v1`, `stateforward.scxml.service-package/v1`) MUST bind the exact URI, package version, component artifact, schema artifact and SHA-256 where one exists, WIT world and version, and MUST declare their outbound HTTP origins, `env` slots, WASI permissions, and event contracts. Unknown, non-canonical, or over-bound declarations MUST fail closed.
- A component MAY declare any WASI permission in its manifest; it MUST NOT import any WASI interface beyond what its manifest declares and the run grants. Hosts MUST list every declared permission (and what the run grants it) before running, MUST fail closed before the first macrostep when a declared permission is ungranted or a grant names nothing declared, and MUST refuse recording or replaying a run that binds a package declaring any WASI permission.
- Components MUST NOT receive secret values: elements and `<invoke>`s name variables for declared slots with the `env` attribute vocabulary, and only the host substitutes values into admitted requests.
- Event contracts MUST use scxml.rs's `event:schema` vocabulary, which stays metadata-only in the engine. Enforcement belongs to the `decision` namespace (candidates are the enabled transitions and their schemas read through `session-snapshot-host`; the chosen event's data is filled to its schema and validated before it is raised) and to package-declared schemas checked at external boundaries (service emit).
- Model backends MUST stay pluggable through `env` slots (`env:baseurl`, `env:apikey`); no backend URL or key may be compiled into a component.

## Bots

- A bot MUST be an SCXML document; its abilities MUST be invoked children (`<invoke type="scxml">`) or invoked services, and its devices MUST be invoked services. mosfet's attachment protocol MUST NOT be ported: invoke lifecycle covers attach and detach.
- Domain event names (`bot.*`, `bot.ability.*`, `phone.*`) MUST stay byte-for-byte as mosfet names them.
- Retries, timeouts, and bounded loops MUST be modelled in the topology (states, `<send delay>`, counters in the data model), never in ad hoc host or component code.
- The builder bot builds SCXML only (never Rust): its drafts MUST pass `sx:validate` and a simulation as an invoked child before it reports `builder.done`.

## Engineering

- Production code MUST NOT use unhandled `unwrap()`, `expect()`, `panic!()`, `todo!()`, `unreachable!()`, indexing known to panic, unchecked conversions, or ignored `Result` or `Option` values on reachable input or runtime paths; tests MAY use `expect()` with a message that states the invariant.
- Expected failures MUST use typed errors with preserved safe context and MUST fail closed at trust boundaries; unknown fields, versions, enum values, and capability kinds MUST be rejected rather than ignored.
- Every crate root MUST carry `#![forbid(unsafe_code)]` wherever target or generated-binding constraints permit; authored unsafe code MUST NOT be added.
- Resource bounds a document can reach (sizes, counts, fuel, loop iterations) MUST apply to wasm builds only, declared through `scxml_types::wasm_bound`; input-file bounds at the embedder's trust boundary (config, env file, manifests, components) MAY apply on every build, and retries and waits MUST have explicit upper bounds.
- Core logic MUST receive clocks, randomness, filesystems, networks, environment, and storage through explicit capability boundaries; only the embedder (`host/`) reads files and its configured env file.
- Secrets MUST NOT be printed, logged, formatted into errors, copied, or committed. Keys reach a run only through an env file named in the nemobot config (`env-file`, `env`) or the CLI's `--env-file`; `.env*` files are ignored by git.
- Production code MUST emit OpenTelemetry at its boundaries (component entry points through scxml.rs's telemetry WIT facade in guests; the OpenTelemetry API in `host/`); every metric dimension and attribute MUST have a documented bounded-cardinality analysis, and paths, URIs, event or state names, payloads, values, and digests MUST NOT be indexed attributes.
- Pre-1.0, undeployed replacements MUST be hard cutovers that remove the old path, tests, configuration, and documentation in the same change; compatibility shims and legacy fallbacks MUST NOT be added.
- Changes MUST be narrowly scoped and supported by tests matching the exact claim. Before claiming completion, contributors MUST run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` (which includes the `nemobot-host` smoke test), and `sh scripts/build-components.sh` for component changes.
- Contributors MUST NOT use the `find` command in this repository's tooling or agent workflows; use `rg` and `ls`.
- AI agents MUST NOT create commits, amend commits, push, create remotes, open pull requests, or merge unless the maintainer directing the work explicitly requests the exact action; when commits are requested they MUST be local, atomic, and on the named branch only.
