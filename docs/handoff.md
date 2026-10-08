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
| floppy (this repo, formerly nemobot) | `~/VSCode/stateforward/nemobot` (directory not yet renamed) | `main` | bots, floppy's namespaces and services, docs; no runtime of its own (D14) |
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
- Models run on the user's two DGX Sparks:
  - **direct vLLM (preferred):** `http://192.168.2.89:8000/v1`, model
    `Qwen/Qwen3.8-Flash-Next`, key `VLLM_API_KEY` in floppy's `.env`. Streams
    reasoning immediately; 3/3 builder runs passed.
  - gateway: `https://cpa.willen.dev/v1` (or LAN `http://192.168.2.128:8319/v1`),
    model `sparks/qwen3.8-flash-next`, key `CPA_API_KEY`. Holds reasoning back,
    so long generations time out; a report for the gateway's maintainer
    was drafted in the 2026-10-07 session.
  - decision service: TypeSafe (`TYPESAFE_API_KEY` in
    `~/VSCode/stateforward/mosfet/mosfet.py/.env`).
- `.env` files are gitignored; never print or commit their values.

## Where things stand (2026-10-07)

Done (scxml.rs `main` and `feat/invoke-services` at `3bdcf97d`):

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
  (SCXML = spec only, XML, XSD, COMP, PKG, ENV, EVENT, LINT, LIMIT).
- Browser parity slice 1 (validator and extension components in the browser).
- floppy: `generate` and `decision` services, `decision:menu`, the builder
  bot (unrestricted, D28; `no-progress` exit, D26; failure causes logged),
  generate's package validator.

Stopped mid-pass (2026-10-08) — resume this first:

- **scxml.rs pass "datamodel-packages" is UNFINISHED.** Its uncommitted work
  (193 files) was saved as one WIP commit `00330f9e` on branch
  `wip/datamodel-packages` in the scxml.rs repo. It is unproven (no
  fmt/clippy/tests run to completion) and must NOT be merged as is.
  `feat/invoke-services` and scxml.rs `main` are both clean at `3bdcf97d`,
  and floppy's subtree matches.
- To resume: `git -C ~/VSCode/stateforward/scxml.rs-services switch wip/datamodel-packages`,
  inspect `git show --stat HEAD`, finish the work, run the full proof
  (fmt; clippy `-D warnings` with all/no features and wasm32; workspace and
  doc tests; UI goldens; `published_contract_docs`; both `cargo doc` runs;
  `scripts/verify-wasm-capabilities.sh`; `scripts/verify-browser-host.sh`;
  IRP dispositions unchanged, regenerating baselines with the Docker
  linux/amd64 canonical build only if bytes move), then squash or split the
  WIP into real commits on `feat/invoke-services`, fast-forward `main`, and
  pull the subtree into floppy. The stopped agent was adding CHANGELOG
  entries when it was stopped.
- Scope of that pass (all user decisions):
  - **Data models as packages**: a built-in attribute vocabulary
    `https://github.com/stateforward/scxml.rs/namespaces/datamodel` (like `env`/`event`:
    tagless, never fetched). Documents write
    `<scxml datamodel="json" datamodel:src="https://github.com/<o>/<r>/datamodels/json[@tag]">`;
    core `datamodel` stays a spec NMTOKEN (the W3C schema types it so).
    The vocabulary defines ONLY `datamodel:src` (no `srcexpr`, no special
    rule for it; other attributes get the generic unknown-attribute
    did-you-mean). A `DATAMODEL` code family covers `datamodel:src` outside
    `<scxml>` and a token that doesn't match the package's declared name
    (with a fix). Built-ins `null`/`ecmascript`/`xpath` are embedded. A
    non-built-in token with no `datamodel:src` is a `PKG` error. Data-model
    packages resolve like namespaces/services (`datamodels/` repository
    URIs, GitHub resolver component, `@tag` split, resolved once per run),
    with an example data-model package in scxml.rs.
  - **Foreign attributes and elements accepted** (the W3C schema allows
    them): `xml:base` honored for URI resolution; `xml:lang`, `xml:space`,
    `xsi:*` ignored; attributes/elements in a namespace no package binds
    get a `LINT` warning ("… in an unbound namespace has no effect", with
    a maybe-incorrect removal fix); those in a bound package's namespace go
    to that package's validator and are not lowered outside executable
    content. Executable-content custom actions keep their rules.
  - **LIMIT is size bounds only**: delete LIMIT005 (unknown data model) and
    LIMIT006 (foreign markup); renumber LIMIT007–009 → LIMIT005–007.
  - Update `--explain`, docs/validator-rules.md, CHANGELOG (unreleased),
    UI goldens, the browser host, AGENTS.md (built-in vocabularies,
    `datamodels/` repository URIs), and floppy's decisions.md mapping.

## Open decisions (waiting on the user)

1. Default endpoint: neutral package default plus a local `.env` pointing at
   direct vLLM (`FLOPPY_BASE_URL`, provider-neutral `FLOPPY_API_KEY`)?
2. Browser slice 2: bump pinned Node to 26 (JSPI for services in browsers),
   pre-built browser artifact per release, GitHub Pages for CORS on release
   assets.
3. Rename the directory `nemobot/` → `floppy/` (and copy the Claude memory
   folder to the new project path).
4. Save the direct-vLLM traffic-light run as `bots/builder/examples/traffic_light_vllm/`;
   builder drafts should reference schema files instead of inline JSON in
   attributes (the inline quoting caused an `XML001` failure).
5. Send the gateway report (reasoning buffering, keep-alives, abort on disconnect).

## Backlog (agreed, not started)

- Browser parity slice 2 (services, resolution, WASI shims, element trace).
- `scx compile --target native`: one self-contained executable per platform;
  top-level `<data>` ids are the arguments, `<donedata>` JSON on stdout,
  `<log>` on stderr, exit code from the final state.
- Exporter components for session outputs (`<log>`, `<donedata>`, trace):
  stdout default, file/WebSocket/OTLP packages. Low priority.
- Canonical (Docker Linux) builds of floppy's components so pinned digests
  reproduce (D27).
- Flaky `repository_namespaces` test in scxml.rs (parallel tests share a
  document file).
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
