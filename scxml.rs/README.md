# scxml

A Rust implementation of W3C [SCXML 1.0](https://www.w3.org/TR/scxml/)
(State Chart XML) built on the WebAssembly Component Model. A compiler
validates SCXML and lowers it into a portable, deterministic Machine IR; one
generic machine component runs that IR inside Wasm. Native and browser hosts
link capability components (data models, Event I/O Processors, clock,
telemetry) and drive the session; they never interpret SCXML themselves.

- **Conformant.** Every applicable case of the official W3C SCXML 1.0
  Implementation Report Plan (IRP) test suite passes, for both the ECMAScript
  and the XPath data model.
- **One engine everywhere.** The same Wasm components run under Wasmtime
  (CLI and Rust embedders) and in the browser.
- **Built for tools and agents.** rustc-grade validator diagnostics with
  JSON and SARIF output, live sessions with record and replay, and canonical
  JSON session snapshots with a diff.

## Status

Version `0.2.0`, pre-1.0. The WIT packages, Machine IR, and report and
snapshot formats are versioned, and they may change between minor versions: a
breaking change bumps the affected WIT package's minor version or data-format
version, together with the workspace minor version; additive and compatible
changes bump the patch version. Pin exact versions. See [Versions](#versions).

No crate is published to crates.io yet. The project is distributed as source
and as Wasm components built from this workspace.

### Conformance

The [IRP](https://www.w3.org/Voice/2013/scxml-irp/) is the W3C's official
SCXML 1.0 test suite. Against its 2015-03-10 corpus, every applicable case
passes:

| Data model | Passed / applicable | Mandatory | Optional | Under errata | Not applicable |
| --- | ---: | ---: | ---: | ---: | ---: |
| ECMAScript | 201 / 201 | 167 / 167 | 34 / 34 | 0 | 1 |
| XPath | 178 / 178 | 163 / 163 | 15 / 15 | 5 | 24 |

- No case is `Failed`, `Unsupported`, `Invalid`, or `HarnessError`, and
  nothing is skipped. The manual cases that cannot log an `Outcome` (178,
  230, 250, 301, 307, 415, and 513; 301 only for ECMAScript) are decided by
  a documented
  manual-equivalent oracle that checks exactly what the W3C instruction asks
  a human to verify.
- `NotApplicable` cases are those whose documents fix another data model or
  need scripting, so the prepared corpus never exercises its own data model:
  ECMAScript 436 (`datamodel="null"`); XPath 301-304 (scripting only), 436,
  and the 19 ECMAScript-profile cases.
- XPath cases 241, 244, 307, 520, and 534 have defective upstream inputs and
  run from documented, digest-bound corrections
  ([conformance.md](docs/conformance.md#irp-errata-and-not-applicable-cases)).
- The XPath data model is measured against the SCXML 1.0 Candidate
  Recommendation of 2014-03-13 (Appendix C.3), because the final
  Recommendation dropped it.
- Every case runs through the shipped Wasm components (machine, data models,
  Event I/O Processors, clock, and compiler) under the Wasmtime host, and each
  run reproduces the checked-in baseline case for case. The browser host is
  held to the same baselines for the cases its parity test runs.

The baselines (runner `0.2.0`, Machine IR schema `5`) are checked in under
[`tests/conformance/w3c-irp/baselines`](tests/conformance/w3c-irp/baselines)
and bind these digests:

| Evidence | ECMAScript | XPath |
| --- | --- | --- |
| corpus digest | `9f005f20c9cbcd7188507833d8fd8a71158cc49cf70e9428fd3fc807e11396ca` | `a661ef4af8bc62fc064cea5e137a6b469ede4b7da80ececf932d66d37cc93bca` |
| errata SHA-256 | `54c7f1cb44686e2bf499a0edf2596d311f4623245b2fe380abf0ac5b4663303f` | `72fe9e3ca418f55dadf64d2d6331af724cf8ebbc2a87a0180489db818aa73f2a` |
| producer report digest | `e3ede7c7940b91d20ba4866e70559deaea2b958ad5187960f3f7282dfed0f2da` | `e638452638539e158b3697c90f23dea0bdf2de7f0ae8632bb32d69ba4c27044f` |
| canonical artifact SHA-256 | `42599ca8ae2c032752b22a886475cf7ecf917d2e66eb28d15167bab673399bc4` | `759288f4141efb51ffa62952545707f7c4ecb410ad66ed0f2c6ccdaaf71e1e40` |

Each repository namespace also has its own implementation report plan in
the same format, run through the same pipeline and reported separately
(never counted with the W3C IRP;
[conformance.md](docs/conformance.md#namespace-suites)):

| Namespace suite | Assertions | Tests | ECMAScript | XPath |
| --- | ---: | ---: | ---: | ---: |
| [`env`](namespaces/env/irp/README.md) | 2 | 8 | 8 / 8 | 8 / 8 |
| [`event`](namespaces/event/irp/README.md) | 9 | 29 | 29 / 29 | 29 / 29 |

[docs/conformance.md](docs/conformance.md) describes the pipeline, the
oracles, and how to reproduce the baselines;
[docs/spec-errata.md](docs/spec-errata.md) lists where the IRP and the SCXML
1.0 text disagree and what this implementation does.

## Quickstart

The Rust toolchain (1.98 with `rustfmt`, `clippy`, and the `wasm32-wasip2`
target) is pinned by [`rust-toolchain.toml`](rust-toolchain.toml) and
installed by `rustup` on first use. Building the Wasm components also needs
`wasm-tools 1.244.0` and `wasi-virt` at a pinned revision:

```console
cargo install --locked wasm-tools@1.244.0
cargo install --locked --git https://github.com/bytecodealliance/WASI-Virt --rev 448f6df8f688cee5d6995e96b1ffc31f9bf00742 wasi-virt
```

Build and verify the release session components first: `scx` embeds them
(its build reads `target/wasm-artifacts/release`, checks each component
against the manifest, and fails rather than building without them), with the
`scx` namespace's component it embeds too. The
example extension component is built too, for the namespace examples:

```console
sh scripts/verify-wasm-capabilities.sh \
  scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath \
  scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_extension_example \
  scxml_compiler scxml_machine scx
RELEASE="$PWD/target/wasm-artifacts/release"
MANIFEST_SHA256=$( (sha256sum "$RELEASE/manifest.tsv" 2>/dev/null \
  || shasum -a 256 "$RELEASE/manifest.tsv") | cut -d ' ' -f 1)
scx() { cargo run --release --locked -p scxml-cli -- "$@"; }
```

Validate and compile a small document. Validation runs XML well-formedness,
the bundled official W3C SCXML 1.0 XSD, and the semantic rules:

```console
mkdir -p target
cat > target/hello.scx <<'EOF'
<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="ready">
  <state id="ready"><transition event="finish" target="done"/></state>
  <final id="done"/>
</scxml>
EOF
scx validate target/hello.scx
scx compile target/hello.scx --output target/hello.scxml-ir.json
```

Run the document as a live session: events go in as newline-delimited JSON,
and one JSON record per line comes out. `--record` writes a log that
`--replay` reproduces record for record:

```console
printf '%s\n' '{"name":"finish"}' | scx run target/hello.scx \
  --record target/hello.live.ndjson
scx run target/hello.scx \
  --replay target/hello.live.ndjson
```

Take a session snapshot at every checkpoint and diff the first and last
(`snapshot diff` exits 1 when the documents differ, as these do):

```console
scx snapshot run target/hello.scx \
  --event finish --every --output-dir target/hello-snapshots
scx snapshot diff target/hello-snapshots/0000.snapshot.json target/hello-snapshots/0002.snapshot.json
```

Run the checked-in W3C IRP corpus (offline) through the same components and
compare it with the checked-in baseline:

```console
scx conformance run \
  --corpus tests/conformance/w3c-irp/prepared/ecmascript \
  --canonical-baseline tests/conformance/w3c-irp/baselines/ecmascript.scxml-report.json
```

Use `prepared/xpath` and `xpath.scxml-report.json` for the XPath corpus.
[docs/cli.md](docs/cli.md) covers every command and flag, and
[CONTRIBUTING.md](CONTRIBUTING.md) lists the full proof matrix CI runs.

## How it works

```text
SCXML document (+ extension package manifests and schemas)
        │  validator component: XML, XSD 1.0, semantic rules
        ▼
compiler component ──► Machine IR (portable JSON data, not code)
        │
        ├─ runtime mode: the host serves the IR to the generic machine
        └─ packaged mode: the IR is composed into a machine component
        ▼
machine component (the SCXML engine)
        │  imports narrow capabilities
        ▼
data model · Event I/O Processors · clock · invoked sessions · extensions
        (each a Wasm component, linked by a thin Wasmtime or browser host)
```

The compiler never runs a native process and never generates code, so it
runs unchanged inside Wasm, including in a browser. It emits Machine IR: plain
data with a deterministic recipe. In runtime mode, a host hands that IR to the
one generic machine component; this is the only mode browsers use. In
packaged mode, the packager composes the IR into a data-only provider and the
pinned machine component, producing one self-contained component for
ahead-of-time hosts.

The machine component contains the engine. It runs each external event to a
stable configuration, macrostep by macrostep, and reaches everything else
through imported capabilities: the data model the document names
(ECMAScript by default, through the Boa 0.22 interpreter; XPath 1.0; or
null), the SCXML and BasicHTTP Event I/O Processors, a deterministic logical
clock for delayed `<send>`, invoked sessions, and executable content
extensions. Every component is deny-by-default: it imports only the WIT
interfaces it needs and no ambient file system, network, clock, or
randomness.

Two hosts link the components. The Wasmtime host
([`hosts/scxml-host-wasmtime`](hosts/scxml-host-wasmtime)) authenticates each
component against a SHA-256 release manifest, bounds every call with fuel,
and runs invoked sessions on one cooperative scheduler; the CLI uses it. The
browser host ([`hosts/scxml-host-browser`](hosts/scxml-host-browser)) runs the
same components, transpiled once by jco into ES modules over core Wasm. Neither host selects transitions
or interprets SCXML. [docs/architecture.md](docs/architecture.md) describes
each component, its WIT contract, and the hosts in detail.

## Features

**Validator diagnostics and reporters.** Diagnostics carry stable codes,
exact byte spans, notes, help, specification references, and rustc-style
fixes with applicability. `scx validate` renders them like rustc, or
emits the canonical JSON report or SARIF 2.1.0 for editors and agent repair
loops; `scx --explain CODE` explains any code. See
[validator-rules.md](docs/validator-rules.md) and
[cli.md](docs/cli.md#validator-diagnostics-and-reporters).

**Live sessions with record and replay.** `scx run` keeps one session
(and every session it invokes) open, processes each input event to a stable
configuration, and fires delayed sends at their wall-clock deadline. A
recorded run replays record for record on a logical clock. Rust embedders get the
same through `SessionHost::open`. See [live-session.md](docs/live-session.md).

**Session snapshots.** A canonical JSON document shows a session and its
invoked sessions at a checkpoint: configuration, data model, queues, pending
sends, and the transitions an event could take. Snapshots are read-only;
`snapshot show`, `check`, and `diff` render, check, and compare them. See
[session-snapshot.md](docs/session-snapshot.md).

**BasicHTTP.** The BasicHTTP Event I/O Processor (SCXML Appendix C.2) runs
over an in-process loopback in batch runs, and over the network in live
sessions: `--http-listen` serves each session's location and `--http-allow`
admits outbound origins (deny-all by default). See
[cli.md](docs/cli.md#basichttp-over-the-network).

**Browser host.** An ES module package that runs the generic machine, the
validator, the compiler, the three data models, both Event I/O Processors,
the clock, and extension components in the page, with fuel-metered calls,
an allowlist for URLs and BasicHTTP, and OpenTelemetry through the JS API; a
shared corpus holds it to the Wasmtime host's outcomes. See the
[browser host README](hosts/scxml-host-browser/README.md).

**Wasm script components.** `<script src="x.wasm"/>` runs a Wasm component
that reads and assigns the session's data model. Scripts are pinned by
SHA-256 at compile time or resolved at run time under a digest allowlist. See
[wasm-scripts.md](docs/wasm-scripts.md).

**Executable content extensions.** An extension namespace (SCXML §4.10) is a
package: an XSD for structure plus a Wasm extension component that runs the
elements. A namespace URI that names a GitHub repository at a release tag is
loaded from that release, verified, cached, and pinned, so a document needs no
flag to use it. An extension element is executable content and does no I/O;
work that waits on the network belongs in an invoked service, which never sees
secrets either: its `<invoke>` names an `env` slot, the run supplies values
with `--env` and `--env-file`, and the component receives only an opaque
reference, which the host substitutes into its outbound requests to the
origins the package declares or a base URL the run supplies.
See [executable-content-extensions.md](docs/executable-content-extensions.md),
the [`env` vocabulary](namespaces/env/README.md), and the
[example extension](components/scxml-extension-example). `scx` itself ships
one extension namespace, [`scx`](namespaces/scx/README.md)
(`<scx:validate>`: the validator as executable content, for machines that
check SCXML they write), embedded in the binary so documents use it with no
flag or fetch.

**Event data schemas.** `event:schema` on a `<transition>`, `<send>`,
`<raise>`, or `<donedata>` describes the event data it handles or creates
with a JSON Schema, inline or from a file next to the document (with a JSON
Pointer); `event:schemaexpr` gives it as a data-model expression. The
compiler checks each schema against its dialect's metaschema (2020-12,
2019-09, or draft-07) and embeds it with its SHA-256 in the Machine IR, and
session snapshots show it on each transition. At run time a `<send>` or
`<raise>` whose data its own schema, or for an event the session delivers to
itself the schema of a transition handling it, rejects is not dispatched and
raises `error.execution`; so is a send to another session whose receiving
document's transitions reject its data, and host input they reject is
refused. Transitions still match by name. See the
[`event` vocabulary](namespaces/event/README.md).

**Observability.** Spans and measurements go through OpenTelemetry at
component and host boundaries only, with closed label sets, so cardinality
stays bounded; trace context propagates through events. See
[observability.md](docs/observability.md).

## Versions

Workspace version `0.2.0` ships the `@0.2.0` WIT packages, Machine IR `5`,
`scxml.report/v3` (conformance payload `9`), the session snapshot document
`scxml.session-snapshot/v2` (diff `scxml.session-snapshot-diff/v2`),
delayed-send wire record `8`, the `scxml.live-log/v1` live-session log, the
`stateforward.scxml.extension-package/v2` extension package manifest, and the
`stateforward.scxml.extension-cache/v1` extension cache index entry, the
`stateforward.scxml.service-cache/v1` service cache index entry, the
`stateforward.scxml.data-model-package/v1` data-model package manifest, and
the `stateforward.scxml.data-model-cache/v1` data-model cache index entry. Every
contract whose shape changed since `v0.1.0` has a new version:

| Contract | `0.1.0` | `0.2.0` |
| --- | --- | --- |
| WIT packages | `@0.1.0` | `@0.2.0` |
| Machine IR | `4` | `5` |
| Canonical report envelope | `scxml.report/v2` | `scxml.report/v3` |
| Conformance report payload | `8` | `9` |
| Session snapshot document | `scxml.session-snapshot/v1` | `scxml.session-snapshot/v2` |
| Session snapshot diff document | `scxml.session-snapshot-diff/v1` | `scxml.session-snapshot-diff/v2` |
| Delayed-send wire record | `7` | `8` |
| Prepared conformance corpus manifest | `5` | `6` |
| IRP errata file | `1` | `1` |
| Manual-equivalent oracle table | `2` | `4` |
| Live-session log | none | `scxml.live-log/v1` |
| Extension package manifest | none | `stateforward.scxml.extension-package/v2` |
| Extension cache index entry | none | `stateforward.scxml.extension-cache/v1` |
| Service cache index entry | none | `stateforward.scxml.service-cache/v1` |
| Data-model package manifest | none | `stateforward.scxml.data-model-package/v1` |
| Data-model cache index entry | none | `stateforward.scxml.data-model-cache/v1` |
| Rust crates and the browser package | `0.1.0` | `0.2.0` |

A released version never changes in place. Because the project is pre-1.0,
replacements are hard cutovers with no deprecation period. The authoritative
policy is the "Versioning policy" section of
[`crates/scxml-wit/src/lib.rs`](crates/scxml-wit/src/lib.rs).

## Documentation

| Document | Covers |
| --- | --- |
| [Architecture](docs/architecture.md) | the pipeline, every component and WIT package, data models, validation and extension packages, hosts |
| [Command-line interface](docs/cli.md) | every `scx` command with examples, flags, and exit statuses |
| [Conformance](docs/conformance.md) | the official IRP pipeline, preparation, oracles, baselines, and digests |
| [Validator rules](docs/validator-rules.md) | every diagnostic code and its specification basis |
| [Live sessions](docs/live-session.md) | `scx run`, its JSON protocol, record and replay, BasicHTTP networking, the host API |
| [Session snapshots](docs/session-snapshot.md) | the snapshot and diff documents and the `snapshot` commands |
| [Executable content extensions](docs/executable-content-extensions.md) | writing an extension component, `env` slots, loading an element from a repository |
| [`event` vocabulary](namespaces/event/README.md) | JSON Schemas for event data (`event:schema`, `event:schemaexpr`): resolution, references, snapshots, and validator codes |
| [`scx` namespace](namespaces/scx/README.md) | `<scx:validate>`: the validator as an extension element, embedded in `scx` |
| [Wasm script components](docs/wasm-scripts.md) | `<script src>` backed by a Wasm component |
| [Implementation-defined choices and limits](docs/limits.md) | where SCXML leaves a choice to the platform, fuel, and resource bounds |
| [Observability](docs/observability.md) | OpenTelemetry spans, measurements, and cardinality |
| [Specification errata](docs/spec-errata.md) | where the IRP and the SCXML 1.0 text disagree |
| [Status and limitations](docs/limitations.md) | what is covered, with evidence, and the known gaps |
| [Browser host](hosts/scxml-host-browser/README.md) | the browser package, its options, fuel, and tests |
| [Contributing](CONTRIBUTING.md) | workflow and the proof commands CI runs |

Repository layout: [`crates/`](crates) holds the Rust libraries and the CLI,
[`components/`](components) the Wasm components,
[`hosts/`](hosts) the Wasmtime and browser hosts,
[`namespaces/`](namespaces) the repository namespaces (executable content
extension packages and the host's `env` and `event` attribute
vocabularies), and
[`tests/conformance/w3c-irp`](tests/conformance/w3c-irp) the vendored W3C IRP
corpus and baselines.

## Current limitations

The full list, with evidence, is in
[docs/limitations.md](docs/limitations.md). In short:

- No Event I/O Processor other than SCXML and BasicHTTP exists; a `<send>`
  of any other type raises `error.execution`.
- Five XPath IRP cases (241, 244, 307, 520, 534) pass only under documented
  upstream errata; 24 XPath cases and ECMAScript case 436 are `NotApplicable`
  because they never exercise the corpus's data model (see
  [Conformance](#conformance)).
- The IRP results are measured through the shipped components under the
  Wasmtime host, the only conformance execution path.
- Boa 0.22 has no allocation hook or interrupt, so ECMAScript evaluation is
  bounded structurally, with Wasm memory and fuel as the outer bound (only
  process limits in native builds); a data-model call that reaches its
  fuel cap ends the session.
- `event:schema` contracts are not checked for autoforwarded copies,
  invoked services' events, BasicHTTP sends, donedata, or `event:schemaexpr`.
- There is no save or restore of session state, in process or durable;
  SCXML defines no persistence.
- The compiler component's packaging path is not exercised under Wasmtime.
- The minimal proof hosts answer `raise`, `send`, `cancel`, and extension
  calls with `unavailable` (the machine's own `<raise>` is engine-internal);
  the Wasmtime session host, which the CLI uses, implements them.
- Data-model packages run under the Wasmtime session host (every session
  whose `datamodel` names a bound package) and, in the browser host, only as
  the root session's page-supplied provider of a component the package
  ships transpiled; the browser host resolves no package at run time, and an
  invoked child there whose `datamodel` names a package gets no data model.
  `scx validate` does not link a package's checker, so its expressions get
  the `LINT010` note.
- `xml:base` is resolved within one document: an inline `<content>`
  machine starts from its own root, not from the bases of the elements
  around it.
- The browser host loads URLs only through the page (its allowlisted
  `resources` option or `loadDocument`; a runtime `srcexpr` URL through
  `resources` only in a cross-origin-isolated `async: 'auto'` worker
  session), its async sessions do not take the `http` option, and it runs no
  extension components.
- Invoked services (`<invoke type>` other than SCXML) run only under the
  Wasmtime session host, bound by exact type URI (`--service-package`); a
  session holding one cannot be snapshotted, and the browser host binds none
  (a service `<invoke>` there raises `error.execution` when it runs). WASI
  is linked into service components only, by rule (extension elements are
  synchronous executable content), as each component's own `wasi:*` imports
  with the runtime resources the run grants (`--dir`, `--wasi`); `wasi:cli`,
  `wasi:http`, and UDP are never linked.
- Repository namespaces and services are GitHub repositories only; a
  release serves its package only under the exact URI its manifest names
  (pinned or tagless), and a child loaded by `<invoke srcexpr>` or
  `<content expr>` compiles against what the run already bound.
- An `event:schema` references only files relative to the document; child
  documents compiled when invoked get no schema files, and a browser page
  supplies them to the validator and compiler itself.

## Security

See [SECURITY.md](SECURITY.md) for how to report vulnerabilities privately.
This project parses untrusted SCXML, XML, XSD, Machine IR, and Wasm
inputs, fetches repository namespaces, and confines the values a run holds to
references; reports about bypasses of the documented bounds, of repository
element verification, or of the isolation between elements are in scope.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project, as defined in the
Apache-2.0 license, is dual licensed as above without any additional terms or
conditions.

The vendored W3C SCXML Implementation Report corpus and templates under
[`tests/conformance/w3c-irp`](tests/conformance/w3c-irp) are third-party
material distributed under the W3C terms recorded in
[`tests/conformance/w3c-irp/LICENSE.md`](tests/conformance/w3c-irp/LICENSE.md).
The `scxml-conformance` crate compiles in the W3C manual instructions its
manual-equivalent oracle quotes verbatim (and its tests embed upstream IRP
files), so its Cargo license metadata records
`(Apache-2.0 OR MIT) AND LicenseRef-W3C-Test-Suite`, and `deny.toml` allows
that reference for that crate only.

The vendored official W3C SCXML 1.0 XML Schema set under
[`crates/scxml-validator-xsd/schemas/w3c-scxml-1.0`](crates/scxml-validator-xsd/schemas/w3c-scxml-1.0)
is compiled into `scxml-validator-xsd`'s `BundledSchemaResolver`, and so into
the validator Wasm component (`scxml_validator.wasm`) and the `scx`
binary, which both use that resolver. The compiler component does not embed
it: it validates only against schemas its caller supplies. The schema set
is third-party material under the W3C notice reproduced in
[`crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/NOTICE.md`](crates/scxml-validator-xsd/schemas/w3c-scxml-1.0/NOTICE.md),
which must accompany every copy. It is not covered by the Apache-2.0 OR MIT
grant above. The `scxml-validator-xsd` crate's Cargo license metadata records
it as `(Apache-2.0 OR MIT) AND LicenseRef-W3C-SCXML-Schema`, and `deny.toml`
allows that reference for that crate only.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the proof matrix and
[CHANGELOG.md](CHANGELOG.md) for release notes.
