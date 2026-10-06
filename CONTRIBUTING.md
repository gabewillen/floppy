# Contributing

Thank you for your interest in contributing. Before you start:

- [AGENTS.md](AGENTS.md) is the normative project rulebook for **both human
  contributors and AI coding agents**. Changes must satisfy it.
  [.agents/rules/project.rules.md](.agents/rules/project.rules.md) restates the
  conformance rules.
- Participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).
- Report security vulnerabilities privately as described in
  [SECURITY.md](SECURITY.md), never in public issues.
- Record user-visible changes in the `[Unreleased]` entry of
  [CHANGELOG.md](CHANGELOG.md).
- Contributions are dual licensed under Apache-2.0 OR MIT, as described in the
  [README](README.md#license).

## Required workspace proof

Rust 1.98, `rustfmt`, `clippy`, and `wasm32-wasip2` are pinned by
`rust-toolchain.toml`.

Component packaging and validation additionally require exact tool identities:
`wasm-tools 1.240.0`, `cargo-deny 0.20.2`, and `wasi-virt 0.2.0` built from
upstream WASI-Virt revision `448f6df8f688cee5d6995e96b1ffc31f9bf00742`.
Several upstream WASI-Virt commits report version `0.2.0`, and the checked-in
IRP baselines bind the exact component bytes that revision emits, so the
revision, not the version, is the pin. Install them locally with:

```console
cargo install --locked wasm-tools@1.240.0 cargo-deny@0.20.2
cargo install --locked --git https://github.com/bytecodealliance/WASI-Virt --rev 448f6df8f688cee5d6995e96b1ffc31f9bf00742 wasi-virt
```

CI installs `wasm-tools` and `cargo-deny` at those versions through
`taiki-e/install-action` and runs the same `wasi-virt` install command in every
job. `scripts/verify-wasm-capabilities.sh` fails closed when the `wasm-tools`
or `wasi-virt` version drifts, when `cargo install --list` does not record
`wasi-virt` at the pinned revision, or when the `wasi-virt` on `PATH` is not
that cargo-installed binary.

CI also pins its supply-chain inputs by immutable identity:

- `actions/checkout` commit `11d5960a326750d5838078e36cf38b85af677262`
- `dtolnay/rust-toolchain` commit
  `02cb101ec7c40f2c49e1d9714d64511d8e1b74de`
- `taiki-e/install-action` commit
  `9114bf4d891761788c546334fd37538eae1bf8b3`
- `actions/upload-artifact` commit
  `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (v7.0.1)

The commands below are exactly the steps in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml). Keep the two in sync.
The `rust` job first frees disk space on the runner (the host tests build
every release component into a second target directory, and the stock
runner image does not leave room for both; there is no local equivalent to
run):

```console
sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL && df -h /
```

and then runs:

```console
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --all-features --locked
cargo test -p scxml-wit --locked
cargo clippy --locked --target wasm32-wasip2 -p scxml-telemetry -p scxml-validator-xsd -p scxml-clock -p scxml-compiler -p scxml-data-model-ecmascript -p scxml-data-model-null -p scxml-data-model-xpath -p scxml-ioprocessor-scxml -p scxml-ioprocessor-basichttp -p scxml-machine -p scxml-extension-example -p scxml-extension-openai -p scxml-reporter-rustc -p scxml-reporter-sarif -p scxml-script-example -p scxml-service-example -p scxml-service-files -p scxml-snapshot-codec -p scxml-validator -- -D warnings
cargo check -p scxml-host-wasmtime --features wasmtime --locked
cargo test -p scxml-host-wasmtime --features wasmtime,http --locked
cargo deny check
```

and then two documentation drift checks. The first fails when the source-set
SHA-256 published in
[`tests/conformance/w3c-irp/README.md`](tests/conformance/w3c-irp/README.md)
differs from the digest recorded in either prepared `corpus.json` file:

```console
for model in ecmascript xpath; do
  digest=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['suite']['source_digest'])" "tests/conformance/w3c-irp/prepared/$model/corpus.json")
  grep -qF "$digest" tests/conformance/w3c-irp/README.md || { echo "tests/conformance/w3c-irp/README.md does not publish $model source-set digest $digest"; exit 1; }
done
```

The second fails when a format version (Machine IR, report payload, session
snapshot document, delayed-send wire) or a baseline, report, corpus, errata, source-set,
or manifest digest published in `README.md`, `docs/conformance.md`,
`CHANGELOG.md`, or `tests/conformance/w3c-irp/README.md` differs from the code
and the checked-in baselines. The same test binary also fails when a `run`
command, installed tool version, or pinned action commit in `ci.yml` is missing
from this section:

```console
cargo test -p scxml-conformance --test published_contract_docs --locked
```

The IRP gate compiles through the canonical compiler with no expression
checker linked, so a separate gate runs the path users run: every applicable
official case of both prepared corpora goes through `scxml-cli compile`, whose
validator links the ECMAScript and XPath expression-syntax checkers, and must
compile with zero error diagnostics (case 301, which the W3C requires the
processor to reject, must be rejected with only `COMP003`). Warnings are
allowed and listed on standard output
([docs/conformance.md](docs/conformance.md#validator-linked-compile-gate)):

```console
cargo test -p scxml-cli --test irp_validator_path --locked -- --nocapture
```

The `wasm32-tests` job runs the tests of the bounds that apply to wasm builds
only (resource bounds, `scxml_types::wasm_bound`): every test named
`wasm_only_*` is compiled for `wasm32-wasip2` and run under the Wasmtime CLI.
The script fails closed when such a test lives in a target it does not run,
and when test code gates anything else on wasm32 (a
`#[cfg(target_arch = "wasm32")]` item or block, or a
`cfg!(target_arch = "wasm32")` branch), since no job would run it. A crate
whose wasm32 build imports a project WIT interface (a component, or a crate
that records telemetry through the host) cannot run under the Wasmtime CLI:
its own tests check the native side and pin each bound's documented value
with `wasm_bound`, and the Wasmtime host's component tests check the bound
through the shipped component:

```console
cargo install --locked wasmtime-cli@49.0.2
sh scripts/test-wasm32.sh
```

The `wasm-components` job builds, virtualizes, validates, and import-allowlists
every executable guest through the authoritative component inventory:

```console
python3 scripts/verify-component-authored-unsafe.py
sh scripts/verify-wasm-capabilities.sh
```

Release guest bytes must be reproducible. `scripts/verify-wasm-capabilities.sh`
and `scripts/verify-reproducible-wasm.sh` both source
`scripts/reproducible-wasm-env.sh`, which remaps the workspace, `CARGO_HOME`
registry and git checkouts, and the toolchain's `rust-src` path to fixed
prefixes through `CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS` (it fails closed if
`RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` is set, since either would override
it), and every component is built in its own `cargo build -p` invocation so
dependency feature unification cannot depend on which components were
requested together. The same job proves the result:

```console
sh scripts/verify-reproducible-wasm.sh scxml-machine
```

It snapshots the working tree into two copies at different absolute paths,
builds the guest in two target directories and in the relocated copy with a
different `CARGO_HOME`, and requires identical SHA-256 digests and no embedded
build or home path.

`scripts/verify-wasm-capabilities.sh` writes its verified artifacts and
`manifest.tsv` to the directory named by `--artifacts-dir DIR`. Without the
flag it falls back to `SCXML_WASM_ARTIFACTS_DIR` (which the Rust test
harnesses that build components set), and without either to
`$CARGO_TARGET_DIR/wasm-artifacts/release`. Callers that share one Cargo
target directory (Cargo serializes the builds) must give every concurrent run
its own artifacts directory; each test suite that builds components does, so
no suite reads a manifest another suite is rewriting. The script also gives
each run its own `TMPDIR` inside that directory, because the pinned
`wasi-virt` passes components to `wasm-opt` through a fixed temporary file
name and concurrent runs sharing one temporary directory fail.

The required `irp-components` job is the conformance gate. It builds and
verifies the eight release components the conformance executor links and
the `openai` repository namespace component, runs every `scxml-conformance`
test with the `wasmtime` feature through them (`irp_components.rs`
reproduces both checked-in W3C IRP baselines and every namespace suite's
byte for byte), and reproduces all of them through the CLI:

```console
export CARGO_TARGET_DIR="$PWD/target/irp-components"
RELEASE="$CARGO_TARGET_DIR/wasm-artifacts/irp-components"
sh scripts/verify-wasm-capabilities.sh --artifacts-dir "$RELEASE" \
  scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath \
  scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_compiler scxml_machine \
  scxml_extension_openai
MANIFEST_SHA256=$(sha256sum "$RELEASE/manifest.tsv" | cut -d ' ' -f 1)
OPENAI_SHA256=$(sha256sum "$RELEASE/scxml_extension_openai.virtualized.wasm" | cut -d ' ' -f 1)
# The uploaded release-components artifact carries the license texts.
cp LICENSE-APACHE LICENSE-MIT "$RELEASE/"
SCXML_COMPONENTS_DIR="$RELEASE" SCXML_COMPONENTS_MANIFEST_SHA256="$MANIFEST_SHA256" \
  SCXML_OPENAI_COMPONENT_SHA256="$OPENAI_SHA256" \
  cargo test --release --locked -p scxml-conformance --features wasmtime
cargo build --release --locked -p scxml-cli --features wasmtime
for model in ecmascript xpath; do
  "$CARGO_TARGET_DIR/release/scxml-cli" conformance run \
    --corpus "tests/conformance/w3c-irp/prepared/$model" \
    --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
    --canonical-baseline "tests/conformance/w3c-irp/baselines/$model.scxml-report.json"
done
# Each repository namespace's own suite (docs/conformance.md, "Namespace
# suites"), binding the openai package where it is used.
OPENAI_URI='https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0'
OPENAI_PACKAGE="--extension-package namespaces/openai/package/openai.extension.json --extension-component-sha256 $OPENAI_URI=$OPENAI_SHA256"
for suite in env:openai event: openai:openai; do
  name=${suite%%:*}
  packages=''
  if [ -n "${suite#*:}" ]; then packages=$OPENAI_PACKAGE; fi
  for model in ecmascript xpath; do
    "$CARGO_TARGET_DIR/release/scxml-cli" conformance run \
      --corpus "namespaces/$name/irp/prepared/$model" \
      --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" $packages \
      --canonical-baseline "namespaces/$name/irp/baselines/$model.scxml-report.json"
  done
done
# The session-snapshot non-perturbation proof (docs/session-snapshot.md,
# "Non-perturbation proof over the IRP"): three runs per corpus whose reports
# must be byte-identical to each other and to the checked-in baseline, with
# sound, reproducible documents.
for model in ecmascript xpath; do
  "$CARGO_TARGET_DIR/release/scxml-cli" conformance run \
    --corpus "tests/conformance/w3c-irp/prepared/$model" \
    --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
    --snapshot-every-checkpoint \
    --canonical-baseline "tests/conformance/w3c-irp/baselines/$model.scxml-report.json"
done
# `scxml-cli snapshot run` end to end through the same release.
SCXML_COMPONENTS_DIR="$RELEASE" SCXML_COMPONENTS_MANIFEST_SHA256="$MANIFEST_SHA256" \
  cargo test --release --locked -p scxml-cli --features wasmtime --test snapshot_run --test live_run
```

On a system without `sha256sum` (older macOS), use
`shasum -a 256 "$RELEASE/manifest.tsv"`. Without `SCXML_COMPONENTS_DIR`, the
executing tests build their own verified release under
`target/conformance-components`. While iterating, `SCXML_IRP_DATAMODEL`
(`ecmascript` or `xpath`) limits the `irp_components` corpus run to that one
data model:

```console
SCXML_IRP_DATAMODEL=xpath cargo test --release --locked -p scxml-conformance \
  --features wasmtime --test irp_components
```

CI leaves it unset, so both corpora always run in the gate.

The canonical release identity is this job's Linux x86_64 build
([docs/conformance.md](docs/conformance.md), "Canonical release identity").
Each host reproduces its own release bytes exactly, but component bytes
differ between build hosts, so on any other host (macOS, for example) the
steps above reproduce every case result but not the identity bytes the IRP
baselines bind, and the byte-for-byte baseline checks fail. The job uploads
its release directory as the `release-components` workflow artifact. To
check identity locally, or to regenerate the baselines after a source
change, download it (`gh run download <run-id> -n release-components -D
ci-release`) and set `RELEASE="$PWD/ci-release"` in place of the build step
above. Without CI, build the same bytes on any Docker host (Linux x86_64 is
emulated elsewhere) with `scripts/build-canonical-components.sh`, which runs
the job's toolchain, pinned tools, and build command in a pinned
`linux/amd64` container and prints the `manifest.tsv` SHA-256:

```console
sh scripts/build-canonical-components.sh --artifacts-dir "$PWD/canonical-release"
RELEASE="$PWD/canonical-release"
MANIFEST_SHA256=$(shasum -a 256 "$RELEASE/manifest.tsv" | cut -d ' ' -f 1)
```

The session-snapshot goldens store a fixed placeholder for component
digests ([docs/session-snapshot.md](docs/session-snapshot.md), "Parity
fixtures"), so the snapshot tests pass on every host.

The `browser-host` job pins Node.js `24.11.1` (`actions/setup-node` commit
`249970729cb0ef3589644e2896645e5dc5ba9c38`) and runs:

```console
cd hosts/scxml-host-browser
npm ci --ignore-scripts --no-audit --no-fund
npx playwright-core install --with-deps --only-shell chromium
cd ../..
sh scripts/verify-browser-host.sh
```

The first two commands install the npm lockfile and the headless Chromium
revision pinned by `playwright-core` (CI runs them in
`hosts/scxml-host-browser` as its working directory); they are the only
network access. The script builds the nine browser guests (the machine,
the compiler, the ECMAScript, XPath, and null data models, the SCXML and
BasicHTTP Event I/O Processors, the clock, and the session snapshot codec)
and the in-workspace `scxml-wasm-meter` tool,
transpiles the guests once with the pinned jco transpiler and fuel-meters every
core module (twice, compared byte for byte), and runs the Node smoke,
contract, capability, and IRP parity tests: the shared trace fixture, hostile
IR, deterministic fuel exhaustion of hostile loops, argument validation, drift
of `index.d.ts` and the closed vocabularies against the WIT and Rust sources,
every machine capability, the BasicHTTP processor, and the official W3C IRP
cases (BasicHTTP included, over the in-process loopback) against the
checked-in Wasmtime baselines. In headless Chromium it replays the same
trace, a hostile nested loop, an event-loop delayed send, an invocation, and
BasicHTTP over the default `fetch`. Pass
`--skip-e2e` to skip only the Chromium run locally; the script prints
`SKIPPED`. After an intended semantic change, regenerate the shared fixture
with `sh scripts/bless-fixtures.sh --trace-fixture`.

A pushed `v*` tag also runs the `release` job, once every other job passed:
it is the only job with `contents: write`, and it publishes the GitHub
release repository namespaces load their extension components from
([docs/executable-content-extensions.md](docs/executable-content-extensions.md),
"Loading an element from a repository"). Before tagging, move every repository
namespace under `namespaces/` to the tag's namespace URI and commit the result
(`sh scripts/set-namespace-version.sh vX.Y.Z`); the job checks it and then
runs:

```console
sh scripts/check-namespace-versions.sh "$GITHUB_REF_NAME"
sh scripts/verify-wasm-capabilities.sh --artifacts-dir "$RELEASE"
cp LICENSE-APACHE LICENSE-MIT "$RELEASE/"
cd "$RELEASE"
sha256sum -- *.virtualized.wasm manifest.tsv > SHA256SUMS
cat SHA256SUMS
cd "$RELEASE"
gh release create "$GITHUB_REF_NAME" --repo "$GITHUB_REPOSITORY" --verify-tag \
--title "$GITHUB_REF_NAME" --notes "Release components of $GITHUB_REF_NAME; verify downloads against SHA256SUMS." \
-- *.virtualized.wasm manifest.tsv SHA256SUMS LICENSE-APACHE LICENSE-MIT
```

Checked-in expected outputs are regenerated only through explicit flags:

```console
sh scripts/bless-fixtures.sh --ui               # scxml-cli UI snapshots
sh scripts/bless-fixtures.sh --validator-rules  # docs/validator-rules.md code table
sh scripts/bless-fixtures.sh --trace-fixture    # shared trace fixture
sh scripts/bless-fixtures.sh --session-snapshot # session-snapshot goldens and diff goldens
sh scripts/bless-fixtures.sh --all
```

Each flag runs one locked `cargo test` with that test's bless switch
(`SCXML_BLESS=1` or `SCXML_UPDATE_TRACE_FIXTURE=1`) set for that process only;
the variables are the harness mechanism behind the flags, and ordinary test
runs never set them. Review the diff before committing.

`scripts/verify-wasm-capabilities.sh` owns the closed component inventory,
including the XPath data model and the example invoked-service guest. It performs the
locked release `wasm32-wasip2` build, deterministic deny-by-default
virtualization, Component Model validation, exact import checks, and artifact
manifest hashing. Adding or removing a component requires changing that single
inventory and its import allowlist; the unsafe verifier independently rejects
inventory drift.

`wit-bindgen` generated ABI glue is the only unsafe-code exception in component
crates. Every component manifest inherits the workspace lints
(`[lints] workspace = true`, where `unsafe_code` is `deny`), each crate root
forbids unsafe code on non-Wasm targets, and each crate root carries one
justified `#![cfg_attr(target_arch = "wasm32", allow(unsafe_code, reason = ...))]`
so that the macro-generated canonical ABI can compile for `wasm32`. Because
that crate-wide allowance cannot distinguish generated code from authored code,
CI also runs:

```console
python3 scripts/verify-component-authored-unsafe.py
```

The verifier checks the exact component-crate inventory (the sixteen crates
under `components/` and the executable-content extension packages under
`namespaces/` (`openai`), matching the inventory in
`scripts/verify-wasm-capabilities.sh`), both lint/configuration requirements,
and the presence of each `wit_bindgen::generate!` boundary. It then lexes every
authored Rust source file below `components/` and `namespaces/`, excluding
comments and string/character literals, and rejects every authored `unsafe`
keyword. This covers unsafe blocks, functions, traits, impls, extern blocks, and
unsafe attributes without inspecting or rejecting generated macro expansion. Any
broader exception requires explicit review for its exact scope, a documented
soundness contract, tests or equivalent evidence, an owner, an expiry condition,
and a removal or containment plan.

Clock import inspection must show only the versioned project clock telemetry
and type interfaces. Any `wasi:clocks`, filesystem, sockets, HTTP, random,
environment, or CLI import is a failure. Virtualization denies the generic
adapter subsystems instead of granting them to the host.

Data-model import inspection must show only versioned project types, telemetry,
and the explicitly declared execution-host query where needed. Any remaining
WASI clock, filesystem, sockets, HTTP, random, environment, CLI, or stream
import is a failure.

Extension-component import inspection must show only the versioned project
types, telemetry, and the `data-model-host`/`execution-host`/`outbound-http`
interfaces the extension component calls. No extension component keeps a
WASI interface: any WASI import (including `wasi:cli/environment`, `wasi:http`,
and `wasi:sockets`), in any component, is a failure. `openai` and the example
reach the network only through `outbound-http`, which the host admits only to
the origins their package declares or a run-supplied value names, and they
receive the run's values only as references (see
[docs/executable-content-extensions.md](docs/executable-content-extensions.md)).

Reporter import inspection must show only
`stateforward:scxml-reporter/telemetry@0.2.0` and
`stateforward:scxml-reporter/types@0.2.0`; any WASI filesystem, sockets, HTTP,
random, clock, environment, CLI, or stream import is a failure.

ECMAScript data-model import inspection must show only
`stateforward:scxml/types`, `stateforward:scxml/telemetry`, and
`stateforward:scxml/execution-host`. Any WASI clock, random, filesystem, socket,
HTTP, environment, or CLI import is a failure. Use the pinned WASI-Virt
revision `448f6df8f688cee5d6995e96b1ffc31f9bf00742` named above; the raw Rust
WASIp2 adapter artifact is build proof, not the deployable capability surface. Before virtualization, the script composes the
raw ECMAScript component with the declarative constant-seed provider
[`components/scxml-data-model-ecmascript/wasi-insecure-seed.wat`](components/scxml-data-model-ecmascript/wasi-insecure-seed.wat)
(`wasm-tools compose`), because Boa's hash tables read
`wasi:random/insecure-seed` once through std's `RandomState` and wasi-virt
denies randomness. The validator component gets the same treatment: the
script composes the fixed `wasi:random` provider
[`components/scxml-validator/wasi-random.wat`](components/scxml-validator/wasi-random.wat)
into it before virtualization, because the XML Schema backend's `ahash`
tables read `wasi:random/random` (and std's `RandomState` reads
`insecure-seed`); `hosts/scxml-host-wasmtime/tests/validator_component.rs`
runs the exact virtualized validator linked to each data-model component
through `ComponentValidator`. Boa itself receives an injected fixed-epoch clock, so no
ambient clock is read. `hosts/scxml-host-wasmtime/tests/data_model_component.rs`
instantiates the exact virtualized bytes under Wasmtime and runs the
representative machine on them.

## Official IRP development oracle

The product oracle is the official W3C SCXML IRP manifest and `.txml` corpus.
The synthetic checked-in corpus tests only the harness.

Prepare pinned local official bytes without network access:

```console
cargo run -p scxml-cli -- conformance prepare \
  --manifest /cache/scxml-irp/manifest.xml \
  --base-uri https://www.w3.org/Voice/2013/scxml-irp \
  --source-dir /cache/scxml-irp \
  --templates /opt/w3c-scxml-irp-templates \
  --datamodel ecmascript \
  --cache /cache/scxml-conformance \
  --output /cache/scxml-conformance/prepared-ecmascript \
  --revision <PINNED-REVISION> \
  --saxon 'java -jar /opt/saxon-he.jar' \
  --offline
```

An intentional online preparation may pass an official HTTP(S) manifest URL and
omit `--offline`; normal tests and runs never download. Verify upstream
license/provenance before copying or publishing templates/corpus bytes.

Start the real implementation loop:

```console
cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --corpus /cache/scxml-conformance/prepared-ecmascript \
  --mandatory \
  --allow-not-passing \
  --json /tmp/irp-progress.json \
  --report-copy /tmp/irp-progress.copy.json
```

Repair compiler/IR/engine semantics from the exact stage/reason groups, prepare
again only when source/template/transformer bytes change, and rerun. Unsupported
is a temporary diagnostic but remains Not Passing. Every selected case runs,
including cases the W3C manifest marks as manual; nothing is skipped. The mandatory
manual cases 178, 230, 250, 301, 307, and 415 cannot log an `Outcome` of `pass`;
they are decided by the documented manual-equivalent oracle in
`crates/scxml-conformance/src/manual_oracle.rs`, and optional manual case 513
by the manual-equivalent oracle in `crates/scxml-conformance/src/external_client.rs`
(all reported as `oracle: manual-equivalent`); manual cases 313 and 314 keep the
`Outcome` oracle. Changing any condition either module checks requires bumping
`MANUAL_ORACLE_VERSION`. Every applicable case of both checked-in prepared
corpora passes, so the default gate exits zero without `--allow-not-passing`;
use that override only for exploratory runs of a changed corpus or engine.

Compare an exact compatible baseline and fail on prior Pass → non-Pass:

```console
cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --corpus /cache/scxml-conformance/prepared-ecmascript \
  --baseline /tmp/irp-progress.snapshot.json \
  --fail-on-regression \
  --allow-not-passing
```

Identity mismatch is rejected unless `--allow-baseline-mismatch` is explicitly
used for exploratory comparison.

## Offline harness proof

```console
cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --corpus tests/conformance \
  --allow-not-passing \
  --json /tmp/synthetic-harness.json

# Must fail because synthetic mandatory cases are deliberately non-passing.
cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --corpus tests/conformance
```

The fixture covers pass, fail, unsupported, invalid, denominator
math, mandatory/optional separation, deterministic filtering, snapshot identity,
and tamper rejection. Unit/integration tests use a fake `CorpusTransformer` and
no live network. Never label these fixture outcomes official IRP passes.

## Architecture constraints

- Deployment semantics live in components. Native Rust builds are test/host
  conveniences, not the production architecture.
- Preparation/XSLT/process/network/filesystem dependencies stay in native
  tooling. `scxml-engine` and the deterministic runner do not invoke Saxon or
  download corpus data.
- The generic machine component contains engine code and obtains immutable
  lowered IR through `ir-source`, from the host at run time or from a composed
  data-only provider in a packaged `component-wasm`. Packaging composes
  pinned prebuilt components and never generates code. Hosts/runners must not
  implement duplicate transition semantics or decode IR.
- Pass schemas and extension packages explicitly; no ambient resolver exists in
  Wasm worlds.
- Validator guests emit only `scxml.report/v3` canonical bytes. Reporter guests
  accept canonical bytes and import no filesystem or network capabilities.
- Canonical validator artifacts preserve complete source name/text for snippets;
  treat them with the source document's privacy and retention policy.
- Distinguish XSLT corpus specialization from XSD validation.
- Distinguish `lowered-ir`, `core-wasm`, and `component-wasm`.
- Capability checks fail closed. Conditions, eventless transitions, hierarchy,
  executable content, and other core semantics are never ignored.
- Runs use deterministic command/fuel budgets, not wall-clock completion guesses.
- The official oracle is exactly the `Outcome` log protocol. If executable log
  content is not lowered, report the concrete unsupported reason; never infer an
  official pass from a final state.
- Do not claim complete SCXML processing, official corpus counts not actually
  run, generated-Wasm/native equivalence, or certification.

## OpenTelemetry and cardinality

Telemetry lives at boundaries, as [AGENTS.md](AGENTS.md) requires. A span
opens where work crosses a boundary: a component or WIT entry point; a public
operation of a facade crate (validate, compile, package, run one engine command,
macrostep, or effect transaction); a capability or I/O
call; a host lifecycle step (load, link, instantiate, session start and
shutdown). That span records the boundary's closed outcome and, on failure,
its closed error category. Everything the boundary calls runs inside it:
getters, accessors, conversions, `Display` implementations, pure predicates,
private helpers, codec internals, and hot inner loops (transition selection,
configuration and hierarchy queries, per-token scanning) emit nothing of their
own, and their failures surface as the boundary's typed failure.

Each span and operation name describes exactly the operation it covers. Do not
reuse an operation label for an unrelated operation (a state lookup is not
`engine-is-stable`), and delete a label in the same change that removes the
last operation it covers.

Telemetry must not measurably regress hot paths. A claim that instrumentation
is cheap, or that a change preserves or improves performance, cites a
benchmark or profile. The deterministic engine benchmark is:

```sh
cargo run --release --locked -p scxml-bench -- 200
```

It drives `tests/fixtures/representative-machine.scxml` and a synthetic
parallel machine swept over 8, 16, 32, and 64 all-toggling regions
(`parallel-wide-<N>`) through `Engine::run` and prints the median
nanoseconds per run and per engine command, plus a work checksum that must
match between the runs being compared. The per-event cost of the sweep must
grow linearly with the region count.

Reference figures for the current boundary-only instrumentation (one
machine, default no-op provider): `representative` 9431-9598 ns/run;
`parallel-wide-16` 31329-31731 ns/run and `parallel-wide-64`
118350-119734 ns/run. Compare only runs with identical work checksums on the
same machine.

Boundaries per crate, which new code follows:

| Crate or component | Spans |
| --- | --- |
| `scxml-engine` | `new`, `run`, one `start`/`send`/`resume`/`cancel` per engine command; failure paths `queue-platform-error`, `block-failed` |
| data-model crates | public operations (initialize, evaluate, assign, execute, check-syntax) |
| component guests | WIT entry points only (plus the machine guest's per-batch span and failed-capability event) |
| `scxml-ioprocessor-scxml` | `prepare`, `dispatch` (each its own operation label) |
| `scxml-ioprocessor-basichttp` | `send`, `receive` |
| `scxml-conformance` | `run-corpus`, `run-suite`, `run-case`, `run-case-wasm`, and file/process/network I/O |
| `scxml-wasm-meter` | `scxml.wasm-meter.instrument` once per `meter()` call and `scxml.wasm-meter.cli` once per CLI run; nothing per function, loop head, re-encoder hook, or `MeterError` format |
| `scxml-xpath` | none (pure) |
| `scxml-scheduler` | none (pure; runs inside the session host's run span) |
| `scxml-snapshot` | none (pure; the host's `session-snapshot` span, the codec's observations, and the CLI spans cover it) |
| `scxml-snapshot-codec` | one `encode` or `diff` observation per call with the `snapshot-bytes` measurement, plus one event; no id, pointer, or code attribute |
| `scxml-cli snapshot` | `snapshot-run`, `snapshot-show`, `snapshot-check`, `snapshot-diff` (one span per subcommand) |
| `scxml-cli run` | `run` (one span per command); the host's live-session spans cover each call |
| `scxml-host-wasmtime` live sessions | `session-open`, `session-send`, `session-advance` (only when a deadline fires or a logical drain runs; never per wait), `session-receive-http` (only when an inbound message reaches a session's queue), `session-communication-error` (only when owed events are delivered), `session-close`, and the existing `session-snapshot`; no attribute beyond the closed operation and outcome labels |
| `scxml-host-wasmtime` extension components | `extension-http-fetch` (one per `outbound-http` request an extension component makes, on the session thread, a child of the `execute` call); only the closed outcome event and error category (`invalid-input` for a malformed or refused request, `unsupported` when no transport is granted, `platform` for a transport failure or an oversized response); never a URL, host, header, body, status, prompt, or model; each `session-snapshot-host.snapshot` an extension component calls is one `session-snapshot` span (the checkpoint snapshot's operation) with only its closed signal event and error category, plus the `snapshot-bytes` measurement, never the document |
| `scxml-host-wasmtime` invoked services | `service-load` (one per `load-services` call), `service-start`, `service-send`, `service-wake`, `service-cancel` (one per export call the host makes on a service, on the session thread), and `service-http-fetch` (one per `outbound-http` request a service makes); only the closed outcome event and error category; never a type URI, invoke id, event, payload, or URL |
| `scxml-host-wasmtime` `http` feature | `basichttp-network-post` (one per outbound request, on its worker thread, parented to the issuing send) `basichttp-listener-request` (one per inbound connection), `basichttp-listener-accept` (one per failed `accept`), and `basichttp-listener-connection` (one per accepted connection dropped or not kept); only the closed outcome event and error category; never a URL, host, port, path, header, status, or body |

### Trace propagation through events

Every SCXML event may carry a W3C Trace Context carrier (WIT `trace-context`:
`traceparent`, `tracestate`, `baggage`, `baggage-policy`). Carriers are
validated at every boundary and never change semantics: no `_event` field,
`_event.raw`, log, trace observation, or conformance result depends on one.

The machine opens one `execute` span per effect batch. Its parent is the
triggering event's carrier; without one, during an invoked child's
`start-invoked`, the invoking batch's carrier; otherwise the host's context
(the Wasmtime `session-run` span; the browser host's innermost open host
span).
Every event the batch raises, sends, autoforwards, or fails with carries
that span's `traceparent` and `tracestate`, the trigger's baggage under the
trigger's `baggage-policy` (`propagate` keeps it, `drop` removes it; no
baggage and `drop` without a trigger carrier), and the trigger's Level 2
`random-trace-id` flag within the same trace. The batch that processes the
event is therefore the child of the operation that emitted it. We use a
parent/child relation, not a span link, on every route, queued and delayed
ones included: each delivered event is processed by exactly one batch, so
the causal parent is unambiguous (the OpenTelemetry messaging convention's
single-message consumer case). When telemetry produces no span of its own
(no provider, a refused carrier, or a non-recording provider that echoes
its parent), the trigger's carrier passes through unchanged rather than
failing the batch. The host starts a root span whenever the ambient parent
is invalid (for example the facade span of a no-op global provider).

Guarantees per route, proven by
`hosts/scxml-host-wasmtime/tests/trace_propagation.rs` (Wasmtime),
`hosts/scxml-host-browser/test/contract.test.mjs` (browser), and
`crates/scxml-engine/tests/interpret.rs` (carrier content):

| Route | The processing batch's parent | Hosts |
| --- | --- | --- |
| internal `<raise>` and `#_internal` | the raising batch | both |
| self `<send>` through the SCXML processor, immediate | the sending batch | both |
| delayed `<send>`, routed when due on the clock | the sending batch | both |
| `#_<invokeid>` (parent to child) | the parent's sending batch | both |
| `#_parent` (child to parent) | the child's sending batch | both |
| autoforward | the parent's forwarding batch, not the carrier the event arrived with | both (Wasmtime test) |
| `done.invoke.<id>` | the child's finishing batch (`call-result.done-trace-context` on `invocation-done`) | both |
| `error.communication` / `error.execution` from a `<send>` or failed invocation | the failing batch | both |
| an invoked child's start (`<invoke>`) | the parent's invoking batch (`invoked-session.trace-context`, passed on `invocation-context.trace-context`) | both |
| `error.execution` from a failed `<if>`/`<elseif>` condition | the batch that ran the content before it | both (engine test) |
| `error.execution` from a failed transition condition | the batch the machine call last opened, whose effects the condition observed; before the call's first batch, the evaluated event's carrier | both |
| `BasicHTTP` self or cross-session post over the loopback | the sending batch (the host carries the validated carrier beside the request; the wire encoding has no trace headers, so `_event.raw` never depends on telemetry) | Wasmtime |
| external `BasicHTTP` client POST (case 513) | the client's W3C `traceparent` and `tracestate` headers; an invalid `traceparent` is ignored and inbound baggage is dropped | Wasmtime |
| embedder-injected event | the caller's carrier (`ScriptedEvent::trace_context`; the browser's `send({traceContext})`) | both |

No route is unlinked. Two parents are not the emitting batch itself, by
construction:

- A top-level session's start batch has no trigger and no invoking batch, so
  it parents to the host's context.
- Transition selection runs before its transaction opens a batch, so a
  condition error cannot name a batch that does not exist yet (and an
  internal-queue scan may process that error before any batch opens). It
  names the batch the call last opened instead, or, for the first selection
  of a `send` or `deliver-from-invocation` call, the carrier of the event
  being selected on, which makes its batch a sibling of that event's batch.

The deterministic validator and reporter benchmark is:

```sh
cargo run --release --locked -p scxml-bench --bin validator -- 5
```

It validates every `tests/fixtures/*.scxml` document and a generated
2000-state document (with and without XSD) and renders one canonical artifact
through the rustc and SARIF reporters, printing the median microseconds per
iteration and a work checksum (documents, diagnostics, rendered bytes) that
must match between the runs being compared.

Reference figures for the current validator path (`-- 5`, one machine,
default no-op provider): `fixtures` 19767-19903 us/iter, `generated-2k`
23572-23720 us/iter, `generated-2k-no-xsd` 9604-9859 us/iter, and `render`
175-183 us/iter.

Native hosts install exporters and providers. Wasm guests use the versioned
WIT adapter; no guest SDK or exporter is introduced, and guests do not use the
WASI OpenTelemetry interfaces. The
browser host is JavaScript: it emits through the OpenTelemetry JS API
(`@opentelemetry/api`, API only) and, like the native hosts, leaves SDK
installation to its embedder (the page).

Allowed indexed dimensions:

| Surface | Key/field | Value set | Cardinality |
| --- | --- | --- | --- |
| native facade | tracer and meter name | `scxml` | 1 |
| native facade | `scxml.module` | static `Module` enum | bounded |
| native facade | `scxml.operation` | static `Operation` enum, one variant per boundary operation | bounded |
| native facade | `error.type` | static `ErrorCategory` enum | bounded |
| native function-call metric `scxml.function.calls` | `scxml.outcome` | `completed`, `failed` | 2 |
| native event metric `scxml.events` | `scxml.event.name` | static `SignalEvent` enum | bounded |
| `scxml-wasm-meter` | `scxml.module`, `scxml.operation`, `error.type`, event name | `wasm-meter`; `scxml.wasm-meter.instrument` (one per `meter()` call), `scxml.wasm-meter.cli` (one per CLI run); `invalid-input`, `packaging` (instrument) plus `input-read`, `harness`, `report` (CLI); `scxml.transformation.completed`, `scxml.input.rejected` | 1; 2; ≤ 5; 2 |
| machine guest | telemetry operation/outcome | WIT enums | bounded |
| clock guest | operation/outcome/event | WIT enums | bounded |
| Wasmtime host | tracer name | `scxml-host-wasmtime`, `scxml-component-guest`, `scxml-capability-guest`, `scxml-guest` | 4 |
| Wasmtime host | meter name | `scxml-host-wasmtime` (component measurements only) | 1 |
| Wasmtime host-owned spans (tracer `scxml-host-wasmtime`) | span name | `component-execute`, `machine-graph-link`, `machine-session`, `capability-graph-execute`, `session-run`, `component-compile` (one per session-host compiled-component cache miss, on the global tracer provider because the cache is process-wide) | 6 |
| Wasmtime host-owned spans | `scxml.outcome` | `ok`, `failed` (on `component-execute`, `machine-graph-link`, `capability-graph-execute`, and `component-compile`; `machine-session` and `session-run` record failure only as the error status `failed`) | 2 |
| Wasmtime host-owned span events | event name | `guest-telemetry-refused` (a refused guest telemetry import: malformed carrier, exhausted span table or lease, unknown handle) | 1 |
| Wasmtime guest spans and signals | `scxml.module` | `machine-component`, `data-model-component`, `script-component`, `compiler-component`, `validator-component` (tracer `scxml-component-guest`); `capability-component` (tracer `scxml-capability-guest`) | 6 |
| Wasmtime guest spans | span name, `scxml.operation`, `scxml.outcome` | WIT `operation` / `outcome` enums; the compiler guest's one span name and operation `compile` and the validator guest's one span name and operation `validate`, each with outcome `ok` or `failed` | bounded |
| Wasmtime compiler and validator guest spans | span event name | WIT `signal-event`: `input-rejected`, `artifact-produced` | 2 |
| Wasmtime compiler and validator guest spans | `error.type` (and error status) | WIT `error-category`: validator `invalid-input`, `validation`, `report`; compiler `invalid-input`, `validation`, `lowering`, `report`, `packaging` | ≤ 5 |
| Wasmtime `scxml-guest` spans (native `TelemetryHost`) | `scxml.module`, `scxml.operation`, `error.type` | static `Module` / `Operation` / `ErrorCategory` enums | bounded |
| browser host | tracer names | `scxml-host-browser`, `scxml-component-guest`, `scxml-capability-guest` | 3 |
| browser host | meter name | `scxml-host-browser` | 1 |
| browser host spans and `scxml.host.operations` counter | `scxml.module`, `scxml.operation`, `scxml.outcome` | `host-browser`; 9 closed host operations; `ok`, `rejected`, `failed` | ≤ 27 series |
| browser guest spans | `scxml.module` | `machine-component`, `data-model-component`, `compiler-component` (tracer `scxml-component-guest`); `capability-component` (tracer `scxml-capability-guest`) | 4 |
| browser guest spans | span name, `scxml.operation`, `scxml.outcome`, `error.type` | WIT `operation` / `outcome` enums, the clock's `operation` / `outcome` enums, and the compiler `error-category` enum | bounded |
| component measurements | no attributes | closed WIT `measurement-kind` selects a fixed instrument/unit | 0 |
| Wasmtime limit measurements | no attributes | static `RuntimeMeasurement` selects a fixed instrument/unit (fuel and store/engine limits) | 0 |
| canonical artifact measurements | no attributes | static `ArtifactMeasurement` selects a fixed instrument/unit | 0 |

Span status messages reuse the same closed values (an outcome or error
category). The browser host's full table, including its closed host
operations, is in
[hosts/scxml-host-browser/README.md](hosts/scxml-host-browser/README.md#telemetry-cardinality);
`hosts/scxml-host-browser/test/contract.test.mjs` pins its names against the
Wasmtime host and the Rust facade.

Guest `observe` carries at most one typed numeric sample. Guest `event` carries
no sample, preventing outcome/event pairs from recording duplicates. Effects,
input bytes, snapshot bytes, queue depth, and fuel use distinct instruments and
stable units; metric labels are forbidden.

Never index test IDs, file paths, URLs, spec IDs, event/state IDs, expressions,
messages, source text, corpus/artifact digests, or timestamps. Detailed context
belongs in reports/diagnostics. Telemetry must not affect deterministic output.
