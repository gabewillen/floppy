# Rust namespace and service crates (`namespaces/**`, `services/**`)

A namespace is an executable-content extension component; a service is an
invoked-service component. Both are Wasm components behind scxml.rs's WIT
contracts (`stateforward:scxml/executable-content-extension`,
`stateforward:scxml/invoked-service`, WIT `@0.2.0`). Review against these rules
(AGENTS.md Components and Engineering; scxml.rs/AGENTS.md) and cite them.

## Safety and failure

- The crate root carries `#![forbid(unsafe_code)]`, or on wasm32 an
  `allow(unsafe_code, reason = "wit-bindgen generated ABI glue only …")` with
  `forbid` elsewhere. No authored `unsafe`.
- Production paths contain no `unwrap()`, `expect()`, `panic!`, `todo!`,
  `unreachable!`, panicking indexing, unchecked `as` casts, or ignored
  `Result`/`Option`. A retained cast carries
  `#[allow(clippy::cast_…, reason = "<proof>")]`. Tests may use
  `expect("<invariant>")`.
- Expected failures are typed errors with safe context and fail closed:
  unknown attributes, fields, enum values, versions and capability kinds are
  rejected, never ignored.
- `component-error.detail` is static text of at most 160 bytes that never
  echoes input (no values, paths, names, SQL).

## Bounds

- Bounds a document can reach (sizes, counts, iterations, decoding steps) are
  declared only through `scxml_types::wasm_bound(...)` and apply to wasm builds;
  a native build does not reject on its own bound. Retries and waits have
  explicit upper bounds.
- Services add no time limits, retry loops or timeouts of their own: the
  invocation's lifetime, ended by the document's `<send delay>` and
  cancellation, bounds the I/O (AGENTS.md Bots).

## Namespaces (executable content)

- Synchronous and quick: no I/O, no WASI, no `outbound-http`, no `env` slots.
  Imports are only `types`, `telemetry`, `data-model-host` and, when it reads
  snapshots, `session-snapshot-host`. Flag any model call, network, file or
  clock read inside an element.
- Shape: pure logic behind a host trait tested natively; a `cfg(wasm32)` guest
  module generated from `../../scxml.rs/crates/scxml-wit/wit/scxml`; an
  initialize-once lifecycle check; an exact namespace-URI check per element;
  `NotHandled` for unknown local names; tagless `NAMESPACE_URI`.
- Elements write results to a `location` and never send.

## Services (invoked)

- One async `invoke` export (WASI 0.3, Component Model async ABI) awaiting
  `outbound-http.fetch`; tagless `SERVICE_URI`.
- The service validates every event it returns itself (D27, D112).
- No compiled-in key or base URL. Without a `baseurl` value, the only default
  is the package's first `http-origins` entry read from the manifest it ships,
  never a second copy (D18). generate and speech declare none and answer
  `unconfigured` / "the backend is not configured" without a base URL (D31,
  D57). With a base URL and no key, send no key header at all (D127).
- Failure events use the closed codes their contract names (`unconfigured`,
  `transport`, `status`, `malformed`, `truncated`, `not-json`,
  `schema-mismatch`, …). Rejection messages never repeat argument values.
- WASI imports are only what the service needs; an import added or removed
  updates the allowlist in `scripts/build-components.sh` and every README and
  test grant.

## Secrets

- Components never receive secret values: only opaque slot references, which
  the host substitutes into admitted requests. No `Debug`/`Display`, log or
  error that could print a request's headers, key or body.

## Telemetry

- Each entry point (`initialize`/`execute`/`shutdown`, or `invoke`) records one
  observation through scxml.rs's telemetry WIT facade with closed operation,
  outcome and signal values. No OTel SDK or exporter in a guest.
- No indexed attribute carries paths, URIs, event or state names, payloads,
  values, digests, session ids, model names or SQL. A new dimension needs a
  documented bounded-cardinality analysis (the crate doc's `# Telemetry`).

## Idiom and docs

- Write idiomatic Rust for the behavior; do not reproduce another language's
  implementation details (Python float or `repr` formatting, datetime string
  shapes copied from a Python serializer, Python-shaped helper names) unless a
  schema or a decision requires the exact bytes, and then cite it.
- The crate `//!` doc states the contract (parameters, result and events,
  failures, `# Bounds`, `# Telemetry`). Public items are documented, fallible
  functions document `# Errors`, every `#[allow]` carries `reason`.
- Code passes `cargo fmt --all --check` and
  `cargo clippy --workspace --all-targets --locked -- -D warnings` (pedantic).
- A namespace behavior change re-baselines its IRP suite
  (`scripts/namespace-irp.sh`) in the same change.

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
