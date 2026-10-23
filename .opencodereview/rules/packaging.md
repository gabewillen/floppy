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
