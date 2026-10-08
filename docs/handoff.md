# Handoff: resuming the floppy / scxml.rs work

Read this first when picking the work up in a new session or account. It
records where things stand, how to work, and what is open. The detailed
history lives in [decisions.md](decisions.md) (numbered decisions),
[port-plan.md](port-plan.md) (mosfet mapping and phases), and
[live-proof.md](live-proof.md) (every live run). Where they disagree with
this file, the newer of decisions.md and this file wins.

Last updated: 2026-10-08.

## Repositories

| Repository | Path | Branch | Notes |
| --- | --- | --- | --- |
| floppy (this repo, formerly nemobot) | `~/VSCode/stateforward/floppy` | `main` | bots, floppy's namespaces and services, docs; no runtime of its own (D14) |
| scxml.rs | `~/VSCode/stateforward/scxml.rs` | `main` | engine, `scx` CLI, hosts, validator, resolver |
| scxml.rs work branch | `~/VSCode/stateforward/scxml.rs-services` (git worktree) | `feat/invoke-services` | all scxml.rs changes are made here first |
| mosfet (source of the port) | `~/VSCode/stateforward/mosfet/mosfet.py` | — | Python runtime being ported; reference only |

`scxml.rs/` inside floppy is a **squashed git subtree** of scxml.rs.

## How to work

- **Merge policy.** Make scxml.rs changes on `feat/invoke-services` in the
  worktree and commit there. After each pass, fast-forward scxml.rs `main`:
  `git -C ~/VSCode/stateforward/scxml.rs merge --ff-only feat/invoke-services`.
  Then pull into floppy:
  `git subtree pull --prefix scxml.rs ~/VSCode/stateforward/scxml.rs feat/invoke-services --squash`.
  Never edit `scxml.rs/` inside floppy directly.
- **Local commits only.** Nothing has been pushed; no PRs, releases, or
  remotes until the user decides to go public.
- **No invented names.** Every public name comes from the W3C SCXML 1.0
  spec, a standard (WASI 0.3, Component Model, wasmtime CLI, OpenAI API,
  rustc/clippy, OpenTelemetry), or an existing scxml.rs name. Ask the user
  before adding a genuinely new name.
- **SCXML first.** Follow the spec's defaults and mechanisms (invocation
  cancel, `<donedata>`, `<data>` + `<param>`, `<send delay>` timeouts)
  instead of adding host mechanisms. Async or I/O work is an invoked
  service; executable content is synchronous (D13).
- **Keep scxml.rs and floppy separate.** Generic SCXML tooling lives in
  scxml.rs; bot policy lives in floppy.
- **Hard cutovers.** 0.2.0 is unreleased: no aliases or fallbacks.
- Pinned `wasm-tools` 1.244.0 is installed globally in `~/.cargo/bin`.

## Running things

- Build floppy's components: `scripts/build-components.sh` (publishes a dev
  package cache under `target/components/package-cache`).
- Run the builder bot (dev, until floppy has a release):
  `scx run --offline --cache-dir target/components/package-cache --env-file .env bots/builder/builder.scx`
  with a `builder.request` event on stdin (see `bots/builder/README.md`).
- Live tests: `FLOPPY_LIVE_ENV_FILE` (colon-separated env files) and
  `FLOPPY_LIVE_MODEL`.
- Model endpoint (decision D31): the `generate` service has no default
  backend; every run needs `FLOPPY_BASE_URL` and `FLOPPY_API_KEY`, and
  without either the builder ends at once in `failed` (`unconfigured`).
  floppy's gitignored `.env` sets them to the user's two DGX Sparks:
  - **direct vLLM:** `FLOPPY_BASE_URL=http://192.168.2.89:8000/v1`, model
    `Qwen/Qwen3.8-Flash-Next` (the builder's default), `FLOPPY_API_KEY` (a
    copy of the vLLM key, also kept as `VLLM_API_KEY`). Streams reasoning
    immediately.
  - the gateway in front of the Sparks is no longer used: it holds
    reasoning back, so long generations time out (D24, D25, live-proof.md);
    a report for its maintainer was drafted in the 2026-10-07 session.
  - decision service: TypeSafe (`TYPESAFE_API_KEY` in
    `~/VSCode/stateforward/mosfet/mosfet.py/.env`).
- `.env` files are gitignored; never print or commit their values.

## Where things stand (2026-10-08)

Done (scxml.rs `main` and `feat/invoke-services` at `92742d65`; floppy's
subtree matches, squash `9d1a67a7`, pull `dea16624`):

- Invoked services on WASI 0.3 / Component Model async (one `invoke`
  export); SCXML §6.4 cancel closes connections; no host time limits on
  service I/O.
- `scx` CLI with embedded engine components, `.scx` documents, document
  env scopes, SCXML-default URI loading, optional `@tag` pins split at
  parse time and resolved once per run, GitHub resolver as a Wasm
  component.
- Validator: rustc-style fixes (closest-match, cross-element such as
  `sendid` → `id`), `package-validator` components (codes render as
  `<prefix>::<code>`), literal-payload `event:schema` checks, code families
  (SCXML = spec only, XML, XSD, COMP, PKG, ENV, EVENT, DATAMODEL, LINT,
  LIMIT).
- Browser parity slice 1 (validator and extension components in the browser).
- floppy: `generate` and `decision` services, `decision:menu`, the builder
  bot (unrestricted, D28; `no-progress` exit, D26; failure causes logged),
  generate's package validator.
- scxml.rs datamodel pass landed (`66f102b0` to `92742d65`, D30): data
  models as packages (`datamodel:src`, `datamodels/` repository URIs,
  example `datamodels/example`), foreign attributes and elements accepted
  (`xml:base` honored, unbound namespaces are `LINT022`), `DATAMODEL001`/
  `002` and `PKG005`, LIMIT size bounds only (`LIMIT007`-`009` renumbered
  to `LIMIT005`-`007`), and `deny.toml` allowing ISC and
  CDLA-Permissive-2.0. The flaky `repository_namespaces` test is fixed
  (`f4f14cc8`: each test writes its document in its own scratch directory).
- floppy is green on it (2026-10-08): `scripts/build-components.sh`, fmt,
  clippy `-D warnings`, all workspace tests, and the `builder_bot` live
  test on direct vLLM; no floppy code or test named a moved code.

Next: phase 4 (bot core and abilities), per
`~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`.

## Port run decisions (user, 2026-10-08)

The remaining mosfet port runs as one ultracode run with these grants:

- **Commits:** commit locally; when the full proof is green, fast-forward
  scxml.rs `main` and subtree-pull into floppy. Never push, publish, or
  release.
- **Live proof:** may run unattended against the DGX Sparks (direct vLLM)
  and the TypeSafe decision service.
- **Names:** the no-invented-names rule applies to scxml.rs only. floppy
  names freely; new scxml.rs names are batched for the user's approval.
- **Order:** backlog order — phase 4 bot core and abilities, phase 3 memory
  on SQLite, phase 5 behaviors and routines, phase 6 speech and vision,
  phase 7 PGlite memory and refreshed defaults.
- **Speech and vision:** OpenAI-compatible audio and vision APIs (served
  from the Sparks); ElevenLabs is an optional backend.
- **Devices (phone, audio):** out of scope for now.
- **Phase 7:** PGlite yes; releases and `SHA256SUMS` wait for going public.
- **Endpoint:** no package default (`FLOPPY_BASE_URL` required, fail fast
  when unset; D31); the gitignored `.env` sets `FLOPPY_BASE_URL` to direct
  vLLM with provider-neutral `FLOPPY_API_KEY`.
- **Builder:** save the vLLM traffic-light run as
  `bots/builder/examples/traffic_light_vllm/`; drafts reference schema
  files instead of inline JSON.
- **Browser slice 2:** after the port.
- **Almost everything is `.scx`:** bots, abilities, behaviors, routines, and
  policy are SCXML documents in `.scx` files; Rust/Wasm components only for
  invoked services doing work SCXML cannot express (async I/O).
- **Phase 4 answers:** bot `bots/floppy/`, abilities in top-level
  `abilities/`, no default model URL (`FLOPPY_BASE_URL` required), intuition
  on the decision service, `turn` field and media by reference in payloads,
  `<decision:menu events>` filter, telemetry one-trace proof deferred,
  scxml.rs `EmbeddedDocument::event_schemas` and `EVENT011` approved. Full
  list in `~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md`.
- **scxml.rs names approved:** the datamodel pass's names, with the example
  package renamed to `datamodels/example`; `deny.toml` allows ISC and
  CDLA-Permissive-2.0 (TLS stack).
- **Pacing:** run through all phases, report after each; stop only for a
  real blocker or scxml.rs names needing approval.

## Open decisions (waiting on the user)

1. Send the gateway report (reasoning buffering, keep-alives, abort on disconnect).

## Backlog (agreed, not started)

- Browser parity slice 2 (services, resolution, WASI shims, element trace).
- `scx compile --target native`: one self-contained executable per platform;
  top-level `<data>` ids are the arguments, `<donedata>` JSON on stdout,
  `<log>` on stderr, exit code from the final state.
- Exporter components for session outputs (`<log>`, `<donedata>`, trace):
  stdout default, file/WebSocket/OTLP packages. Low priority.
- Canonical (Docker Linux) builds of floppy's components so pinned digests
  reproduce (D27).
- Rest of the mosfet port: bot core and abilities (phase 4), behaviors and
  routines (5), memory service SQLite then PGlite (3), speech/vision/devices
  as services (6).
- Website (after the port): mosfet's animated robot (`mosfet/web/src/face`),
  web components + hsm.ts, marketplace of packages, a Mindstorms-style
  workshop whose pieces are inferred from element XSDs, live highlighting
  of running bots.
- Going public: push, CI, releases with `SHA256SUMS`, per-platform `scx`
  binaries, IRP identity regeneration on Linux CI.

## Known stale docs

- [port-plan.md](port-plan.md): phase 0 still describes the deleted
  `outbound-http.begin`/`take`/`wake` API and a 10 s connect timeout
  (services are now WASI 0.3 `invoke` with no host time limits); phase 7
  still says NVIDIA Nemotron/CLM (defaults are now the user's DGX Sparks).
  Refresh it after the datamodel pass lands.
