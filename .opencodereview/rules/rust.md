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
