# Packages, build scripts and Cargo (`package/`, `scripts/`, `Cargo.toml`, `rust-toolchain.toml`)

Review against AGENTS.md (Repository, Components, Engineering) and cite it.

## Package manifests

- Layout: `namespaces/<name>/package/<name>.extension.json` with `<name>.xsd`;
  `services/<name>/package/<name>.service.json`.
- Extension manifest (`stateforward.scxml.extension-package/v2`): exactly
  `namespace-uri`, `package-version`, `extension-artifact`, `schema-artifact`,
  `schema-sha256`, `wit-world` (`stateforward:scxml/executable-content-extension`),
  `wit-version` (`0.2.0`), and optionally `validator-artifact` +
  `validator-sha256` together. Never `http-origins`, `env-slots`, WASI or
  events: an extension does no I/O, and scxml.rs refuses unknown fields.
- Service manifest (`stateforward.scxml.service-package/v1`): `service-uri`,
  `package-version`, `service-artifact`, `schema-artifact` / `schema-sha256`
  (both strings or both null), `wit-world` (`stateforward:scxml/invoked-service`),
  `wit-version` (`0.2.0`), `http-origins` and `env-slots` (both required, may
  be empty), optional validator pair. No WASI, no events.
- URIs are the exact tagless
  `https://github.com/stateforward/floppy/{namespaces,services}/<name>`; the
  XSD `targetNamespace` is the same URI (D21).
- `schema-sha256` is the SHA-256 of the exact XSD bytes: an XSD edit without a
  digest update is a defect. A `services/generate/validator` change updates
  `validator-sha256` from a canonical Linux x86_64 build (D27).
- Adding an `http-origins` entry widens the outbound allowlist and needs a
  decision; generate and speech declare none (D31, D57).
- WIT `@0.2.0` and Machine IR `5` are never bumped by floppy.

## Component build (`scripts/build-components.sh`)

- A new or removed component changes, in the same change: the component list,
  its exact `expected_imports` allowlist, `Cargo.toml` members, and for a
  namespace its `irp/` suite and `SUITES` in `tests/tests/namespace_irp.rs`,
  plus the docs that list namespaces (AGENTS.md, README, port-plan).
- Extension allowlists never contain `outbound-http` or any `wasi:*`. Service
  allowlists list exactly the WASI they keep, each with a reason.
- Pinned tools stay pinned (`wasm-tools`, `wasi-virt` rev, wasi-sdk) and the
  reproducible flags come from `scripts/reproducible-wasm-env.sh`.
- A dev build writes only `target/components/package-cache`, never `scx`'s
  release cache; documents are never edited to point at local builds.
- No new naming suffix `-component`; crates are `floppy-<name>`,
  `floppy-service-<name>`, `floppy-service-<name>-validator`.

## Scripts

- Shell scripts are POSIX `sh` with `set -eu`, a usage header, and checks that
  fail closed with a message on stderr. **Never the `find` command**: use
  `rg`, `ls` or `read_dir`.
- Python tooling is fine; it runs with `python3 -I`. Review it for correctness,
  not for being Python.
- Scripts never print or log secret values or read keys other than through
  the env file a run names.

## Cargo

- Workspace stays on Rust `1.98` (`rust-toolchain.toml` `1.98.0`, target
  `wasm32-wasip2`), edition `2024`, resolver `3`, changed only by an explicit
  reviewed migration.
- The workspace `exclude`s `scxml.rs` and depends on its crates by path; it
  never absorbs scxml.rs's workspace, lockfile or members.
- `Cargo.lock` stays committed; shared dependencies resolve to the versions
  scxml.rs's lockfile pins, with exact pins and `default-features = false`. Any
  divergence is a recorded decision (as D185 records `miniz_oxide` 0.8.9).
- Member crates inherit package metadata with `.workspace = true`, use
  `[lints] workspace = true`, components use `crate-type = ["cdylib", "rlib"]`,
  and each dependency carries a one-line comment saying why. Host stacks
  (wasmtime, rustls, ureq) stay out of guest crates.

## Rules for every file

floppy is a bot written entirely as SCXML documents, plus the Wasm packages
(extension namespaces under `namespaces/`, invoked services under
`services/`) those documents use. It has no runtime of its own: every bot runs
under scxml.rs's `scx run`, and the scxml.rs engine selects transitions and
runs macrosteps. `AGENTS.md` is normative (RFC 2119 MUST/SHOULD/MAY);
`docs/decisions.md` records each decision as `D<n>` and is cited by number.
Cite the AGENTS.md rule or decision a finding rests on.

- **No edits under `scxml.rs/`.** It is a squashed subtree; a change there goes
  upstream first and arrives only through `git subtree pull --squash`. Any
  hunk under `scxml.rs/` in an ordinary pull request is a defect.
- **No host code.** floppy adds no embedder, session loop, config format, or
  other runtime code; a capability a bot needs from the runtime is added to
  scxml.rs upstream. Code that selects transitions or mimics engine semantics
  is a defect.
- **Hard cutovers.** Pre-1.0, a replacement removes the old path, its tests,
  configuration and documentation in the same change. Flag compatibility
  shims, legacy fallbacks, "deprecated" aliases, or both paths left in place.
- **Idiomatic behavior, not ported artifacts.** floppy matches the behavior
  it ports, written idiomatically for SCXML, ECMAScript and Rust. Flag code
  that reproduces another language's implementation details instead of the
  behavior: mirroring Python or pydantic float formatting (`1.0` vs `1`),
  number-to-string shims that imitate `repr`, datetime formatting copied
  from a Python serializer when the schema only needs a valid timestamp, or
  helper names and structure carried over from Python. In ECMAScript, numbers
  are plain JS numbers. Python *tooling* (scripts under `scripts/`, test
  helpers, generators) is fine and is not a finding.
- **Wording in new text.** New code, comments, docs, schema descriptions,
  test names and commit-visible text MUST NOT name the upstream Python
  project the bot was ported from (the word `mosfet`, its module paths such as
  `src/mosfet/`) and MUST NOT describe behavior as "the original's" or "the
  reference's" ("as the original does", "the reference implementation",
  "matches the original"). State the behavior itself, and cite a decision
  (`D<n>`) for why. Only flag lines the diff adds or changes; existing text
  elsewhere is out of scope.
- **Terminology.** Use the repository's terms: *bot* (an SCXML document under
  `bots/<name>/`), *body* (the bot root, which only composes), *ability* (an
  invoked child document under `abilities/`), *environment* (the root session
  a bot runs in, `environments/<name>/`), *device* (a document an environment
  invokes, `devices/<name>/`), *namespace* (an executable-content extension
  package), *service* (an invoked-service package), *package* (a namespace or
  service with its manifest), *event contract* / *event schema* (an
  `event:schema` JSON Schema file). Domain event names (`bot.*`,
  `bot.ability.*`, `phone.*`) are kept byte for byte; flag a renamed or
  re-cased domain event.
- **Naming.** Concise domain names; no redundant `-component` suffix on
  crates, worlds or artifacts. Package identities are the exact tagless URIs
  `https://github.com/stateforward/floppy/namespaces/<name>` and
  `https://github.com/stateforward/floppy/services/<name>`.
- **Secrets.** Secret values are never printed, logged, formatted into
  errors, committed, or written into commands or documents
  (`--env NAME=VALUE`). Keys reach a run only through `scx --env-file`, and
  only `env`-slotted elements and `<invoke>`s receive them.
- **Tooling.** Never the `find` command in scripts or agent workflows; use
  `rg` and `ls`.
- **Scope.** A change is narrowly scoped and backed by a test matching its
  exact claim. Flag behavior changes with no test, and unrelated edits mixed
  into a change.
