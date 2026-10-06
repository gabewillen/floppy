# Official W3C SCXML IRP conformance

`scxml-conformance` is an official-IRP-first preparation and execution pipeline.
The authoritative source is the W3C SCXML implementation report plan manifest
(`assertions/assert/test/start/dep`) and the `.txml` files named by that
manifest. The official corpus is checked in, prepared from those sources,
under [`tests/conformance/w3c-irp`](../tests/conformance/w3c-irp). The separate
[`tests/conformance/corpus.json`](../tests/conformance/corpus.json), with its
`cases/` and `resources/`, is a synthetic harness corpus for parser and runner
self-tests only; its results are never W3C IRP pass claims.

## Pipeline at a glance

`scx conformance prepare` parses the official W3C
`assertions/assert/test/start/dep` XML manifest, consumes a local pinned corpus
or explicitly downloads into a cache, and invokes an explicitly configured
Saxon process against a selected data-model XSLT. It records source, revision,
manifest/template/source/output digests and transformer identity in immutable,
digest-keyed prepared artifacts. Normal tests and runs never download or invoke
XSLT.

With `--offline`, preparation accepts only `saxon`, `saxon-he`, or
`java -jar LOCAL_PATH`. On macOS, every identity and transform process launches
through `/usr/bin/sandbox-exec` with a generated default-deny profile and a
cleared environment containing only fixed locale and timezone values. Exact
transformer, JAR, source, stylesheet, and runtime paths are readable, one
canonical output/temp area is writable, and other filesystem, network, and
process access fails closed. The raw transformer is never spawned directly.
Hosts without verified process isolation, including the current Linux implementation, fail
closed before transformer execution. The adapter also owns Saxon's restricted
options, disabling DTD/entity, extension, XInclude, and schema-location
processing, and rejects resolver-capable command options and external
entities/DTDs before starting the transformer. Each stylesheet is parsed with
namespace resolution and must stay inside a closed vocabulary: exactly the XSLT
elements and attributes the pinned W3C templates use (under any prefix), and
XPath that calls only `regex-group` and the `node()` kind test, with no named
function reference (`name#arity`) or dynamic function call. Canonical identity records a portable argument vector (the sandbox
wrapper removed and install paths reduced to file names) and a digest over that
vector and the Saxon JAR (or transformer executable) bytes; the sandbox binary,
generated policy, and JVM are verified in-process but not recorded. Each process remains
bounded to 120 seconds and is killed and reaped on timeout.

`scxml-conformance` then compiles every applicable prepared SCXML source through
`scxml-compiler-core` and executes the resulting Machine IR only through a
`CaseExecutor`: `WasmExecutor` (`wasmtime` feature) runs it through the shipped
components under the Wasmtime session host (the generic machine, which
contains `scxml-engine`, the data model the document names, the SCXML and
BasicHTTP I/O processors, the clock, and the compiler for invoked child
documents). The runner applies the official `Outcome` log oracle to the root
session's log records. The engine inside the machine component is the only
transition executor; the runner links no in-process SCXML execution and does
not duplicate selection semantics. Missing engine/compiler capabilities are
fail-closed `Unsupported` results and count in Not Passing.

Results are stage-aware: `Passed`, `Failed`, `Unsupported`, `Invalid`,
`HarnessError`, `Skipped`, and `NotApplicable`. The authoritative summary is
`Passed / Applicable Selected IRP`; Not Passing is Failed + Unsupported +
Invalid + HarnessError, with mandatory/optional, spec, and data-model groups.
`NotApplicable` is only a W3C declaration recorded in a checked-in errata file
(ECMAScript: 436; XPath: 301–304, 436, and the 19 ECMAScript-profile
cases) and stays outside
the denominator; the report lists every such case with its quoted reason. Official cases whose
upstream template or source is defective run from digest-bound errata
corrections (`--errata`, XPath only), and every pass under an erratum is
reported and counted as such.
Stable snapshots refuse incompatible baselines by default and track exact prior
Pass → non-Pass regressions.

The sections below specify preparation, provenance, `Outcome` semantics,
and cardinality; [limitations.md](limitations.md) lists the current gaps.

## Completion contract

For an exact corpus identity and selected data model:

- **Applicable Selected IRP** includes every selected case — filtered cases are
  excluded, but manual cases always run through the same pipeline and are never
  skipped, even when the compiler or engine cannot execute them yet. The only
  other exclusion is a case the W3C itself declares not applicable to the
  prepared data model, or whose W3C assertion and upstream test document fix
  a different data model so that the prepared specialization never exercises
  its own, recorded in the checked-in errata identity (see
  [IRP errata and not-applicable cases](#irp-errata-and-not-applicable-cases));
  it is reported as `NotApplicable`, never executed, and never counted as
  passing;
- a case whose upstream template or source is defective runs from a documented,
  digest-bound erratum correction of that test input and is still decided by
  its real oracle; a pass under an erratum is always reported as such
  (`X passed, of which N under documented errata`);
- **Not Passing** is `Failed + Unsupported + Invalid + HarnessError`.
- mandatory and optional totals are reported separately;
- `Unsupported` is a temporary implementation diagnostic and an authoritative
  gap, not a terminal success state;
- the pinned W3C manifest marks nine cases manual: mandatory 178, 230, 250,
  301, 307, 313, 314, and 415, and optional 513. All of them run like every
  other case and are never skipped. 313 and 314 still log an `Outcome` and
  keep the `Outcome` oracle. The other seven cannot log an `Outcome` of
  `pass`; they are official W3C cases decided by a documented
  manual-equivalent oracle (six in the table under
  [Manual-equivalent oracle](#manual-equivalent-oracle), and 513 through the
  external-client harness under
  [BasicHTTP and case 513](#basichttp-and-case-513)), never by a synthetic
  fixture, and each one stays Not Passing until its real run satisfies that
  oracle;
- 100% means every applicable selected case in the stated group is `Passed`;
- the default CLI gate requires every applicable selected **mandatory** case to
  be `Passed`.

No official count is claimed in this repository until an exact prepared corpus
has actually been run and its identity appears in the report.

## Explicit preparation

Normal tests and `conformance run` do not use the network and do not invoke
XSLT. Corpus acquisition/transformation is an explicit native-tooling step:

```console
cargo run -p scxml-cli -- conformance prepare \
  --manifest /cache/scxml-irp/manifest.xml \
  --base-uri https://www.w3.org/Voice/2013/scxml-irp \
  --source-dir /cache/scxml-irp \
  --templates /opt/w3c-scxml-irp-templates \
  --datamodel ecmascript \
  --cache /cache/scxml-conformance \
  --output /cache/scxml-conformance/prepared-ecmascript \
  --revision w3c-irp-pinned-revision \
  --saxon 'java -jar /opt/saxon-he.jar' \
  --offline
```

Add `--errata tests/conformance/w3c-irp/errata/ecmascript.json` for the
ECMAScript corpus and `--errata tests/conformance/w3c-irp/errata/xpath.json`
for the XPath corpus (see
[IRP errata and not-applicable cases](#irp-errata-and-not-applicable-cases)).

`--manifest` may be an HTTP(S) URL only for an explicit online prepare. The
manifest and every corpus file are cached. `--offline` requires existing local
bytes and also applies the offline transformer contract. Its `--saxon` value
must be exactly `saxon`, `saxon-he`, or `java -jar LOCAL_PATH`; shell wrappers,
caller-supplied Saxon options, configuration files, resolvers, catalogs, and
initializers are rejected. On macOS, the adapter resolves the transformer and
optional JAR to bounded canonical local files and launches identity and
transformation only through `/usr/bin/sandbox-exec` with a generated
`(deny default)` profile. The child environment is cleared and rebuilt with
fixed locale and timezone values. The policy allows reads only from the exact
transformer, optional JAR, source, stylesheet, and generated runtime closure,
allows writes only in the exact output/temp area, and denies all network and
unlisted executable access. The raw transformer is never spawned directly.
Linux and other hosts currently fail closed with
`transformer-offline-isolation-unavailable` until they provide a verified
no-network process-isolation mechanism. The adapter also disables DTD, entity
expansion, extension, XInclude, and schema-location processing. Before any
transformer identity or transformation process starts,
bounded UTF-8 source and stylesheet bytes are screened fail-closed. DTDs,
entities, and stylesheet PIs are forbidden in both. A stylesheet is parsed
with namespace resolution and screened against a deny-by-default vocabulary:
only the XSLT elements and no-namespace attributes the pinned W3C templates
(and their checked-in errata) use, identified by expanded name so a rebound
prefix changes nothing; literal result elements only in no namespace or the
SCXML namespace, with no XSLT attributes; and XPath expressions, patterns, and
attribute value templates that call only `regex-group` and the `node()` kind
test, with no named function reference (`name#arity`), prefixed or `Q{}`
function name, or dynamic function call. Includes, imports, packages, schema
imports, `evaluate`, secondary and result documents, `use-when`, text value
templates, extension instructions, and every external-resource or environment
function therefore fail before any process starts. The canonical transformer digest
covers the portable argument vector and the Saxon JAR (or transformer
executable) bytes; the generated sandbox policy, its allowed path closure, and
the JVM are verified in-process before each launch but are not recorded.
These restrictions define the supported offline preparation surface; a
stylesheet needing external resources must be made self-contained before use.
The transformer process remains subject to the 120-second timeout, kill, and
reap contract. `--force` reacquires/retransforms instead of reusing cached immutable
artifacts. A local manifest directory may be passed directly; absent
`--source-dir`, corpus paths resolve beneath the local manifest directory.
Every manifest `start` and `dep` is acquired and hashed. Every `.txml` start,
including manual cases, is transformed; non-`.txml` starts are retained as
source artifacts but are not presented to the engine as executable SCXML.
When a manifest omits its historical `baseuri` attribute, an HTTP manifest URL
supplies its parent URI automatically. Local preparation must provide the same
trusted value explicitly with `--base-uri`; decoding otherwise fails closed.

The prepared directory contains `corpus.json` and digest-keyed artifacts under
`artifacts/<key>/...`. The key includes manifest, template, data-model, and
transformer identity. Metadata records:

- manifest source URL/path, required bounded OfficialIrp revision, and SHA-256;
- aggregate prepared corpus SHA-256 and exact acquired source-set SHA-256;
- data model;
- per-data-model template SHA-256;
- transformer name, version output, portable command argv (the sandbox
  wrapper removed and absolute install paths reduced to file names), and the
  SHA-256 over that argv and the JAR or executable bytes when available. The
  artifact cache key binds this complete identity, so outputs are never reused
  under another command or transformer build;

Preparation emits only release-bounded OTEL operation and event enums,
including artifact-produced and artifact-reused signals. Transformer names,
versions, argv, paths, revisions, and digests never become telemetry attributes
or metric dimensions; reuse counts remain numeric measurements in the returned
summary. This keeps signal cardinality bounded independently of corpus inputs.
- per-case source and transformed-output SHA-256;
- dependency artifact paths and SHA-256 values;
- preparation failures for cases whose source, dependency, or transform was not
  available;
- when `--errata` is given: the errata identity (file name, SHA-256 of its exact
  bytes, the corrected case id to erratum id map, and the not-applicable case
  ids), and for each corrected case its erratum id, corrected stage, the
  digest-verified corrected input, and the exact upstream-produced output and
  its SHA-256.

A transformation failure does not delete a case. The case remains in
`corpus.json`; execution reports it as `HarnessError` at `Preparation` with the
recorded reason. Loading rejects modified prepared sources or dependencies as
artifact tampering. Manifest schema v6 additionally recomputes the exact
acquired source-set digest and a canonical suite digest over provenance, command
budgets, case metadata, expected oracles, failures, and every referenced
artifact digest. Resources and raw dependency sources carry exact digest maps
and are verified from bounded streaming reads before a suite is exposed.

## IRP errata and not-applicable cases

This section is about defective IRP *inputs*. Where the IRP instead requires
behavior the SCXML specification text contradicts or does not state (the
`origintype` of SCXML events, the name of an unnamed `BasicHTTP` event, the
XPath system-variable elements, and `_event.raw`), the implementation follows
the IRP and [spec-errata.md](spec-errata.md) records each case with both texts
quoted.

Some official IRP inputs are defective for a data model: the unmodified W3C
template or `.txml` source produces a document that no conforming processor
can pass. Such cases are corrected by **errata**, never by changing how the
implementation is judged. `tests/conformance/w3c-irp/errata/<datamodel>.json`
(schema `errata_version` 1, decoded fail-closed with unknown fields rejected)
records per erratum: id, case, the corrected `stage` (`template` or
`source`), the upstream defect with exact template/source line evidence, the
minimal correction, ordered exact-text replacements, and the SHA-256 of both
the upstream-produced output and the corrected output. The errata also pin the
SHA-256 of the unmodified template they were authored against.

`conformance prepare --errata FILE` applies them deterministically, only for
the named cases:

1. the case is prepared exactly as without errata (unmodified template and
   source), and that upstream output's SHA-256 must equal the recorded one
   (`erratum-upstream-drift` otherwise);
2. the named input (a per-case copy of the template, or of the `.txml`
   source) is corrected; every `find` must occur exactly once
   (`erratum-patch-mismatch` otherwise), and the corrected input is screened
   by the same offline XML rules;
3. the corrected input is transformed by the same sandboxed transformer, and
   the corrected output's SHA-256 must equal the recorded one
   (`erratum-corrected-drift` otherwise).

Errata naming a case the manifest does not define (`errata-unknown-case`),
another data model, or another template digest fail closed, as do duplicate
ids or cases. The corrected output is what executes; the corrected input and
the exact upstream output stay under
`artifacts/<key>/<source-digest>/errata/<erratum-id>/` and are digest-verified
on every load. The errata identity is part of the prepared suite identity and
of the conformance report identity, and each case run from a corrected
artifact carries its `erratum` id in the report. Reporters print
`note[erratum]` for such passes and summarize `N/M passed (K under documented
errata)`. Loaders reject a case erratum that disagrees with the suite errata
identity, and reporters reject any report where they disagree.

A prepared corpus carries its errata: preparation writes the exact errata
file bytes beside `corpus.json`, under the file name the suite errata
identity records as its `source` (for example
`prepared/xpath/xpath.json`). Every load, of a checked-in or any standalone
corpus, reads that file (one plain file name inside the corpus directory,
`suite-errata-source` otherwise; `suite-errata-read` when it is missing),
rehashes it against the declared errata digest (`suite-errata-digest`),
requires it to govern the suite's data model and template
(`suite-errata-binding`), and requires the suite errata identity
(`suite-errata-identity`) and every case's erratum and not-applicable record
(`suite-errata-case`) to be exactly those the file produces. A crafted corpus
that edits a correction or a not-applicable declaration therefore fails to
load even with a recomputed corpus identity; editing the errata file as well
changes the errata digest bound into every report identity.

The same file lists cases the W3C declares **not applicable** to the data
model. They are prepared as usual but reported as `NotApplicable` at stage
`selection` with reason code `w3c-not-applicable` and the quoted W3C
instruction, excluded from the applicable denominator, and never executed.

### XPath errata (`errata/xpath.json`)

The errata file SHA-256 is
`72fe9e3ca418f55dadf64d2d6331af724cf8ebbc2a87a0180489db818aa73f2a`.

| Case | Erratum | Stage | Upstream defect | Correction | Upstream output SHA-256 | Corrected output SHA-256 |
| --- | --- | --- | --- | --- | --- | --- |
| 241 | `xpath-241-namelist-value` | source | The namelist-fed child tests `conf:idVal` (`$Var1/text()`), but a namelist value is the parent's `<data id="Var1">` element inserted under the child's `<data>` (SCXML CR 2014 C.3.1), which case 240 checks with `conf:namelistIdVal` (`test241.txml:28`; `test240.txml:27`) | `conf:namelistIdVal` for the namelist child only | `3e23f8be…53f1` | `27dee4b9…f8ab` |
| 244 | `xpath-244-namelist-value` | source | Same defect for its only, namelist-fed child (`test244.txml:26`) | `conf:namelistIdVal` | `3faf35a7…e7fa` | `6363bf68…c4b3` |
| 307 | `xpath-307-substructure-syntax` | template | `conf:varNonexistentStruct` becomes ECMAScript member syntax `$Var1.bar`, an undeclared XPath variable named `Var1.bar` (`xpath.xsl:112-115`) | `$Var1/bar` | `07239075…4c38` | `e9c6acdb…70db` |
| 520 | `xpath-520-unquoted-body` | template | `conf:messageBodyEquals` is emitted unquoted: `contains($_event/raw, this+is+some+content)` (arithmetic over child steps) and a `%20` variant that is an XPath syntax error (`xpath.xsl:372-374`) | quote the value as a string literal | `296e60dd…e122` | `db4a714e…4693` |
| 534 | `xpath-534-param-name` | template | `conf:eventNamedParamHasValue` matches only numeric names (`([0-9]+)(\s+)(\S+)`, `xpath.xsl:364-366`), so `_scxmleventname test` yields `cond=""` | `(\S+)(\s+)(\S+)`, as `ecmascript.xsl:698-700` does | `7ac3fd28…ab68` | `d59d5c84…f1a6` |

Full digests, line evidence, and replacements are in the errata file itself.
Cases 500 and 501 carry no erratum: they run unmodified, and the XPath
template's `_ioprocessors` processor name `#SCXMLProcessor` is answered by
the XPath data model itself, recorded in
[spec-errata section 9](spec-errata.md#9-the-xpath-_ioprocessors-entry-named-scxmlprocessor).
Case 307's label-only logs are authored upstream as `conf:expr=""`
(`test307.txml:18,36`) and both official templates copy that verbatim into
`<log expr="">`; that is the test's own convention for a log without a value,
so no erratum changes it. Lowering treats a blank `<log expr>` as a log without
a value (see "Blank expression attributes" under
[Execution and official oracle](#execution-and-official-oracle)), so
the corrected XPath case passes under its manual-equivalent oracle.

Not applicable for XPath (24 cases; the report lists each one with its
quoted reason and evidence):

- Mandatory cases 301, 302, 303, and 304 (spec `#script`): the manifest
  states "N.B. This test is valid only for datamodels that support
  scripting." (`manifest.xml:508`, `:515`, `:522`, `:529`; 301's source
  repeats it at `test301.txml:4`), and the W3C XPath template states
  "scripting is not supported in Xpath, so not sure what to put here.
  Implementers should know not to bother to run these tests"
  (`xpath.xsl:631-637`, which emits an empty `<script>`). 301 is also a
  manual case; for XPath it is declared, not run.
- Mandatory case 436 (spec `#minimal-profile`, SCXML 1.0 Appendix B.1): the
  W3C scopes the assertion to the null data model ("When the "datamodel"
  attribute of the scxml element has the value "null"", `manifest.xml:1102`)
  and the test document fixes `datamodel="null"` (`test436.txml:3`), which
  the XPath template copies unchanged, so the XPath specialization runs the
  null data model.
- Optional cases 444–446, 448, 449, 451–453, 456, 457, 459, 460, 557, 558,
  560–562, 569, and 578 (spec `#ecma-profile`, SCXML 1.0 Appendix B.2): the
  W3C scopes each assertion to the ECMAScript data model
  (`manifest.xml:1117-1264`) and each test document fixes
  `datamodel="ecmascript"`, which the XPath template copies unchanged, so the
  XPath specialization runs the ECMAScript data model. No erratum applies to
  456: upstream 456 is ECMAScript-only (manifest `#ecma-profile`,
  `test456.txml:4` `datamodel="ecmascript"`), so rewriting its
  XPath-specialized guard into ECMAScript would only turn it into a second
  ECMAScript test. It is declared not applicable to XPath, and the
  ECMAScript corpus runs it from the unmodified upstream input.

The B.2 case 278 stays applicable: its document takes the data model from
`conf:datamodel`, so the XPath specialization runs the XPath data model.

The ECMAScript corpus (`errata/ecmascript.json`) has no errata and declares
exactly one case not applicable (file SHA-256
`54c7f1cb44686e2bf499a0edf2596d311f4623245b2fe380abf0ac5b4663303f`), 436, on the same W3C evidence as its XPath
declaration: the W3C scopes the assertion to the null profile
(`manifest.xml:1101-1102`) and the test document fixes `datamodel="null"`
(`test436.txml:3`), which the ECMAScript template copies unchanged, so the
ECMAScript specialization never exercises the ECMAScript data model. The
template also writes the case's outcomes as ECMAScript value expressions
(`templates/ecmascript.xsl:24-27`, `<log label="Outcome" expr="'pass'"/>`),
which the null data model cannot evaluate (SCXML 1.0 §B.1.4: "There is no
value expression language."), so the null data model rightly rejects the
document it produces.

### XSLT versus XSD

XSLT preparation specializes the W3C `.txml` source for a data model and creates
executable `.scxml`. XSD validation checks the structure of resulting XML.
They are different phases and neither substitutes for the other.

`CorpusTransformer` is the preparation boundary. `scxml-conformance` provides an
explicit external Saxon process adapter; Saxon and process invocation do not
enter the engine or deterministic runner crates. The command is extended with
Saxon-style `-s:`, `-xsl:`, and `-o:` arguments. Preparation also invokes `-t`
to record transformer identity/version. Online preparation may use a deliberately
configured command implementing that protocol. Offline preparation permits only
the closed Saxon command shapes above, requires enforced host isolation, and
records the portable argv plus the argv-and-JAR digest described above;
transforms are never silently faked.

## Templates, provenance, and legal handling

The complete upstream corpus and the unmodified W3C ECMAScript/XPath templates
are pinned under `tests/conformance/w3c-irp`. Its README records source,
revision, exact digests, transformer identity, regeneration commands, and the
external reference used for behavioral comparison. Its license notice preserves the W3C Test Suite and
3-clause BSD options. `conf:pass` and `conf:fail` become final states whose entry
content logs `label="Outcome"` with an expression yielding `pass` or `fail`;
`.txml` dependencies are transformed and opaque resources are copied beside
prepared artifacts.

ECMAScript and Starlark adaptations from the separately maintained AgentML
project are not vendored as SCXML templates because they rewrite the document
root into the AgentML namespace. That project is an external behavioral
reference only; it is not needed to build, test, or verify this repository. Cache identity proves which bytes were used but never substitutes
for license provenance or exact SCXML namespace validation.

## Official manifest contract

The XML parser consumes the real shape:

```xml
<assertions>
  <assert id="403" specnum="3.13" specid="#SelectingTransitions">
    <![CDATA[ assertion description ]]>
    <test id="403" conformance="mandatory" manual="false">
      <start uri="403/test403a.txml"/>
      <start uri="403/test403b.txml"/>
      <dep uri="403/test403sub.txml"/>
    </test>
  </assert>
</assertions>
```

The live 2015 manifest omits `baseuri`. Online preparation derives it from the
manifest URL; pinned local preparation supplies it explicitly with `--base-uri`.
The parser rejects the document when neither trusted source is available.

Every `start` becomes one deterministic case. A single start uses the test ID;
multiple starts use the `testNNN...` stem (`403a`, `403b`, and so on), with a
stable collision suffix if required. Flattening sorts numeric prefixes then full
IDs. Filters cover exact IDs, numeric ranges, `specid`, `specnum`,
mandatory/optional, data model, tags, and manual inclusion.

## Execution and official oracle

Run a prepared corpus with:

```console
cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --corpus /cache/scxml-conformance/prepared-ecmascript \
  --mandatory \
  --json /tmp/irp-report.json \
  --report-copy /tmp/irp-report-copy.json \
  --allow-not-passing
```

The pipeline is parser → validator → `scxml-compiler-core` → Machine IR →
the shipped generic machine component (`scxml-engine` compiled into Wasm)
under the Wasmtime session host → official oracle. The runner does not
implement a second transition selector and executes no SCXML itself. It is deterministic and bounded by `max_commands`; the report
records commands/fuel as numeric measurements, never indexed labels.

Machine IR decoding rejects inputs over 16 MiB before deserialization. Its
multi-target structural checks precompute bounded hierarchy metadata, sort each
target set by hierarchy preorder, and inspect only adjacent selected subtrees;
they do not perform all-pairs ancestry walks. At most 100,000 aggregate target
references are admitted to multi-target validation per machine. Crossing an
ancestor boundary, a non-parallel least common ancestor, a disconnected root,
or that aggregate budget fails closed with a typed lowering error.

The official oracle consumes the root session's executable log records and
passes only when there is exactly one record whose label is exactly `Outcome`
and whose evaluated value is exactly `pass`, and the root session finished in
exactly the `pass` final state: the W3C templates' success criterion
(`templates/ecmascript.xsl:18-30`, `templates/xpath.xsl:18-30`) rewrites
`conf:pass` to a top-level `<final id="pass">` whose entry logs that record,
so a run that logs `Outcome` `pass` and then keeps running, or finishes in
any other configuration, did not reach it and fails with
`outcome-pass-not-final`. A `fail` value, missing record, invalid value, or multiple
Outcome records never pass. Official manifests are rejected unless every case
declares this pass-only oracle; synthetic manifests are separately constrained
to non-official final-state, active-configuration, or terminal-status
expectations. Final-state arrival alone is not an official pass. Executable
`<log>` is lowered into Machine IR and emitted
as an engine effect. The machine component evaluates log expressions through
the suite's ECMAScript, XPath, or null data-model component, and the session
host records the root session's log records, which feed the exact oracle. Unsupported data
models and executable-content semantics fail closed at the capability stage and
count in Not Passing.

### Component execution

Selection, compilation, capability detection, and every oracle stay in the
runner; case execution goes through a `CaseExecutor`
(`crates/scxml-conformance/src/executor.rs`). `conformance run` executes every
case only through the shipped Wasm components under the Wasmtime session host
(`scxml_conformance::wasm::WasmExecutor`, `scx --features wasmtime`): the
generic machine, the data model the document names, the SCXML and `BasicHTTP`
I/O processors, the clock, and the compiler for invoked child documents.
By default they are the release `scx` embeds (the artifacts
`scripts/verify-wasm-capabilities.sh` writes to `wasm-artifacts/release`, or
`SCXML_WASM_ARTIFACTS_DIR`, checked against their manifest when `scx` is
built); `--components DIR` names another release directory (its
`--artifacts-dir DIR`) and `--components-manifest-sha256 HEX` its trusted
manifest digest (`$RELEASE` and `$MANIFEST_SHA256` in the examples). Every
component is authenticated against the manifest's digest, and a build
without the `wasmtime` feature (`--no-default-features`) fails closed with
`wasmtime-disabled` before executing any case. There is no other
execution path: the crate links no in-process SCXML execution, and
`scxml_conformance::run_suite` requires an executor.

`--canonical-baseline BASELINE.scxml-report.json` compares the run with a
checked-in canonical report (`tests/conformance/w3c-irp/baselines`) through
`scxml_conformance::compare_to_baseline`: the report version, suite, schema
policy, IR version, filters, summary, ordered case list, and every field of
every case record (disposition, reasons, oracle, erratum, not-applicable
declarations, diagnostics, configuration, terminal status, commands, and fuel)
must be equal. Implementation identities (compiler, engine, data model,
runner, manual oracle), the run revision, and the report digest name the
producing toolchain and are not compared. Any mismatch exits 1 and lists every
mismatch in sorted order on stderr. `cargo test --release -p
scxml-conformance --features wasmtime --test irp_components` runs both
checked-in corpora this way and additionally requires the fresh canonical
artifact to equal the checked-in baseline byte for byte (the `irp-components`
CI job).

### Manual-equivalent oracle

Six mandatory W3C cases marked `manual="true"` (178, 230, 250, 301, 307, and
415) ask a human to inspect logged output or to observe that the processor
rejected the document, so they can never log an `Outcome` of `pass`. (The
manifest also marks 313 and 314 manual, but they log an `Outcome` and keep
the `Outcome` oracle; optional case 513 is covered under
[BasicHTTP and case 513](#basichttp-and-case-513).)
`crates/scxml-conformance/src/manual_oracle.rs` (`ManualOracle::ALL`) gives each one an
automated oracle that checks exactly what the quoted W3C instruction asks the
tester to verify, over the session-attributed execution trace of the real
engine run. The trace records, in order and per session (root or invoked
child), every completed engine command, every event that command processed
(dequeued and examined by transition selection, whether a transition was
selected or the event was ignored), every `<raise>`, and every `<log>`
record; the host does not bound it, and the `Outcome` oracle never reads
it. Instructions are quoted whitespace-normalized from
the `.txml` source comments.

| Case | W3C instruction | Automated check |
| --- | --- | --- |
| 178 | "The tester must look at the log output and verify that both keys are there." | The root session logs exactly one `_event ` record whose value (`_event.raw`) contains the token `Var1` exactly twice, followed by `2` the first time and `3` the second (tokens are maximal ASCII alphanumeric/`_` runs, so any raw serialization that keeps each key next to its value passes). |
| 230 | "Both the parent and child process print out the contents of the event. The tester must check if they are the same and report his result." | The root session and its single invoked session each log exactly the seven `_event` field labels in document order, the parent's `name` is `childToParent`, and every child value equals the parent value. |
| 250 | "Tester must examine log output from child process to determine success" | The root logs nothing and its single invoked session logs exactly `Exiting sub01` then `Exiting sub0` (SCXML 6.4 onexit on cancel, 3.13 exit order), so never the "not cancelled" log. |
| 301 | "the processor should reject this document because it can't download the script. Therefore we fail if it runs at all." | The processor rejected the document at load because the script loader could not fetch its `<script src>` (`LoadRejection::ScriptUnavailable`): terminal status `rejected`, and no command, fuel, configuration, log, or trace observation at all (SCXML 1.0 section 5.8). |
| 307 | "the tester must report whether the output is the same in the two cases" | For s0 then s1, the root logs either the state's entry value followed by its no-error log, or its error log alone (an access that errors aborts its onentry block before the value log, SCXML 4.9 and 5.9); both states report an error, or both report equal values without one. |
| 415 | "there is no platform-independent way to test that event1 is not processed" | The root session's whole trace is exactly the start command followed by one raise of `event1`: the engine processed no event (the trace records every event transition selection examines, so none for `event1` is the instruction's check), completed no further command, logged nothing, and invoked nothing (SCXML 3.13). |

Every manual-equivalent oracle except 301 (whose document never runs) also requires root trace
logs identical to the logs the `Outcome` oracle reads, no `Outcome` record
(in these documents only the `conf:fail` state logs one), and a finished
machine whose only active state is `final`. Failures carry bounded
`manual-*` reason codes. The oracle is selected only for an official corpus
case whose manifest marks it manual and whose case and assertion IDs equal one
of the six documented IDs; every other official case keeps the `Outcome`
oracle, and synthetic cases can never use it. The capability stage does not
apply the `outcome-log-missing-source` or `outcome-pass-unreachable` gates to
these six cases; any gap surfaces as the real capability, execution, or
oracle disposition of the run.

Capability detection is fail-closed. The supported slice covers atomic,
compound, parallel, and final states, shallow and deep history, eventless
transitions, multiple W3C event descriptors with token-prefix and
wildcard matching, ECMAScript and XPath guards, targetless transitions, and ordered
`log`, `raise`, `assign` (expression or inline markup content), inline and
external (`src`, resolved from the prepared dependency set; see below)
`script`, `if`/`elseif`/`else`, `foreach`, `send` (SCXML and BasicHTTP Event
I/O processors), and `cancel` effects, plus child SCXML `<invoke>` (see
below). Early-bound and late-bound root and state data declarations
initialize in document order through the suite's data-model adapter,
including `file:` external data sources. Initializer forms outside the
expression, inline-text, or `file:` source slice
(`unsupported-data-initializer`), values without a lossless portable
representation (`data-initializer`), and Wasm script
components (`unsupported-script-component`; see
[wasm-scripts.md](wasm-scripts.md)) remain explicitly unsupported in the
runner; no official IRP case reaches these gates.
Multi-target transitions whose targets lie in distinct regions of one
parallel run through the engine's W3C exit and entry sets; Machine IR
validation rejects every other target set.

Blank expression attributes: SCXML 1.0 lets `<log>` omit `expr` (section
4.4), and the upstream W3C case 307 itself authors `<log expr="">` for its
no-error results, so lowering maps a blank (empty or whitespace-only)
`<log expr>` to the absent attribute for every data model, and Machine IR
validation rejects a blank one as non-canonical. Every other expression
attribute keeps its blank text and is evaluated, strictly per SCXML 3.13
and 5.9: a present but blank `<transition cond>` is an invalid expression,
so it raises `error.execution` and the condition is false (the transition
is not taken), and a blank required expression (for example `<if cond="">`)
likewise raises `error.execution`. The upstream XPath template emits
`cond=""` for case 534 on the transition that must be taken; that is a
template defect, corrected by the digest-bound erratum
`xpath-534-param-name`, not a runtime rule. Validator
lint `SCXML042` flags every blank expression attribute.

Execution ownership in IRP runs is split exactly as follows:

- `scxml-engine` (`Engine::run`, the one execution entry point, running
  inside the machine component) owns transition selection, macrosteps and
  microsteps, hierarchy, parallel regions, history, running each command to
  a stable configuration under `MAX_COMMANDS_PER_RUN` (1024 engine commands), and
  block-scoped `error.execution`/`error.communication` recovery: a failing
  effect aborts only its own `<onentry>`, `<onexit>`, or transition block.
  For invocations it plans `Invoke` when a macrostep ends, `CancelInvoke`
  after the invoking state's `<onexit>`, `<finalize>` before transition
  selection for child events, and one `Forward` per `autoforward` invocation
  for every external event it dequeues. Cancelling a running session is the
  engine command `Command::Cancel` (the W3C `exitInterpreter` procedure): it
  exits every active state in exit order, runs its `<onexit>` content and
  cancels its started invocations, then finishes without a `done` event and
  discards every queued event.
- The machine component runs `scxml_engine::interpret` over its WIT
  `machine-capabilities` imports: data binding (early and late, `<data expr>`,
  `<data src>` through `load-document`), `<send>` and `<invoke>` expression
  evaluation, send ids and invoke ids, `<cancel sendid|sendidexpr>`,
  `<donedata>` payloads and invoked-session return data, `<foreach>` item
  snapshots, and autoforward.
- The Wasmtime session host
  (`hosts/scxml-host-wasmtime/src/session_host`) implements those
  capabilities by linking components only. Every session gets its own
  machine store, the data-model component the document names, and a
  `scxml-ioprocessor-scxml` component store linked to its own `scxml-clock`
  component store, which classify `<send>` and `<invoke>` types and targets,
  stamp `_event.origin`, and schedule delayed sends on the logical clock
  (deadline order and cancellation). The `scxml-compiler` component compiles
  invoked documents; each invoked session is its own machine session, and
  may invoke in turn within the host's invoke limits (an invocation beyond
  them fails as that `<invoke>`'s `error.execution`). Every session owns an
  internal and an external event queue; the SCXML Event I/O Processor
  pushes cross-session events onto the target's external queue. The pure
  `scxml-scheduler` crate is the cooperative executor of the root session
  and its invoked sessions: it orders scripted inputs, each session's
  external queue (invocation order), `done.invoke`,
  and the shared virtual clock, which advances only when no session has
  runnable work. A still-running child is cancelled through its machine's `cancel`
  (the engine's `exitInterpreter`) before it is dropped (manual case 250).
  No invoked-service component takes part: every official case invokes
  SCXML sessions only (`<invoke type>` absent, `scxml`, or the SCXML URI).
- The runner compiles with runtime script resolution; the executor supplies
  the session host's `script-loader` (SCXML 1.0 section 5.8): at document
  load, before any data-model initializer or executable content runs, each
  distinct `<script src>` resolves only from the case's integrity-checked
  prepared dependency set (exact URI, or a `file:<name>` matching a
  dependency's final path segment; never the network or filesystem) and is
  authenticated with `scxml_script::verify_script` against the digests of
  those prepared bytes. An unfetchable reference rejects the document as
  `ScriptUnavailable` and a failed authentication as `ScriptInvalid`: no
  session runs and the case records terminal status `rejected` with no
  commands, fuel, or configuration (official case 301, decided by its
  manual-equivalent oracle). An invoked child document rejected at load
  fails its invocation with `error.execution`.

The SCXML and BasicHTTP Event I/O processors are supported; no other Event I/O
processor exists, so a `<send>` of any other type raises `error.execution`
(SCXML 6.2, official case 199), and no run performs network I/O (see
[BasicHTTP and case 513](#basichttp-and-case-513)). Send payloads outside the
supported slice (XML-element or repeated `<content>`, an incomplete `<param>`,
another child element, or `namelist` combined with payload children) and the
remaining portability gaps stay explicit `Unsupported`; none are dropped or
inferred. No official IRP case reaches these gates. The runner completes the canonical
parse and validation decision before inspecting capability features, so a
malformed document that also contains an unsupported feature is `Invalid`,
never `Unsupported`. Only valid documents reach capability classification.
Missing capability evidence then produces `Unsupported` with a bounded stage
and reason code; parser/validator/lowering rejection produces `Invalid`;
preparation, integrity, or runner-contract defects produce `HarnessError`.

## Validator-linked compile gate

The conformance runner compiles every case through the canonical compiler
with no expression-syntax checker linked, so the IRP gate never exercises the
expression diagnostics users see. `scx validate` and `compile` link the
ECMAScript and XPath checkers. The test
`crates/scxml-cli/tests/irp_validator_path.rs` runs that path: it compiles
every applicable official case of both checked-in prepared corpora (every
case not declared `NotApplicable`, less case 513, which has no SCXML
document) with the `scx` binary and requires each compile to succeed
with zero error diagnostics. The one exception is case 301, whose W3C
instruction requires the processor to reject the document ("the processor
should reject this document because it can't download the script"): its
compile must fail with only `COMP003` (no script package supplies the
`<script src>`). Warning diagnostics are allowed; the test lists every one
on standard output.

```console
cargo test -p scxml-cli --test irp_validator_path --locked -- --nocapture
```

## BasicHTTP and case 513

The shipped `scxml-ioprocessor-basichttp` component owns every BasicHTTP
decision (SCXML C.2): it encodes a `<send>` as an
`application/x-www-form-urlencoded` POST (`_scxmleventname` first, then
`namelist` and `<param>` entries in document order; `<content>` as one
percent-encoded body segment), interprets the response (only 2xx delivers),
validates and decodes inbound messages (first `_scxmleventname` form
parameter, else header, else `HTTP.POST`; other parameters as a JSON object of
strings; other content, a non-form body or a bare `<content>` segment, per
SCXML B.2.8.1: a JSON or XML `Content-Type` tried first ("SHOULD try to format
the data according to those indications"), then well-formed JSON as its value
("it MUST create the corresponding ECMAScript object(s)"), then a well-formed
XML document as XML, else space-normalized text ("MUST treat the content as a
space-normalized string literal", over XML white space as XPath 1.0
`normalize-space()` defines it); `_event.raw` is the message as received), and chooses the response
status (200 only after the event is queued). The machine evaluates `<send>`
expressions through its data model, and the session host supplies the
processor's `http-transport` capability as a deterministic in-process
loopback: each
session's `_ioprocessors` BasicHTTP `location` (under both the full type URI
and the `basichttp` alias) is
`http://basichttp.loopback.invalid/sessions/<session-id>/basichttp` (the
reserved `.invalid` authority can never resolve), and a request is delivered
synchronously by location in its canonical form to the sending session, its
parent, or a live child. The loopback opens no socket, starts no thread, and reads no
clock. A missing target, an unknown loopback session (404), a non-2xx
answer, or a URI outside the loopback authority raises `error.communication`;
an invalid target or parameter raises `error.execution`; a delayed BasicHTTP
send is posted when its delay elapses (SCXML 6.2), held until then on the
session clock by the SCXML Event I/O Processor.

Official case 513 is not a machine: its start is the W3C instruction
`513/test513.txt`, which asks a human tester to POST
`key1=value1&key2=value2` with an `_scxmleventname: test` header to the
interpreter's BasicHTTP location and check for a 200 response. The runner
decides it with a documented **manual-equivalent oracle implemented by the
external-client harness**
(`scxml_conformance::external_client`), derived from that instruction and
bound to its exact SHA-256
(`418df239d9c85bc1fe04be64d1cf23b605a236975f14ed66a1c33fd40f2e059b`; a
changed instruction or start URI is a `HarnessError` with
`external-client-oracle-drift`):

- the receiving session is a **harness-authored** document (not W3C corpus
  bytes) in the suite's data model that follows the IRP `conf:pass`/`conf:fail`
  convention: it logs `Outcome` `pass` only on the external event `test`;
- the harness acts as the W3C tester's client and sends the instruction's
  request through the same loopback transport and shipped processor that
  sessions use;
- the case passes only when the response status is exactly 200 **and** the
  receiving session passes the same `Outcome` oracle as every outcome-log
  case: exactly one `Outcome` log of `pass`, in a session that finished in
  exactly the `pass` final state. Its report records the
  `manual-equivalent` oracle kind, never `outcome-log`, and the oracle source
  is bound into the manual-oracle and runner identities.

The Wasmtime host links the BasicHTTP component standalone
(`BasicHttpProcessor`) to an embedder-supplied `BasicHttpTransport`; its
virtualized artifact imports only `stateforward:scxml-basichttp/types`,
`telemetry`, and `http-transport` (no `wasi:sockets` or `wasi:http`), as
`scripts/verify-wasm-capabilities.sh` enforces. In the checked-in baselines the
BasicHTTP cases and case 513 pass in both suites.

BasicHTTP telemetry uses only closed labels: the component's WIT telemetry
enums (`type-uri`, `send`, `receive`, `respond`; `ok`/`rejected`/`failed`; six
closed events) and one attribute-free `body-bytes` numeric sample recorded on
the host's input-size instrument. URIs, event names, parameters, bodies, and
status codes are never attributes.

## Results and baselines

Dispositions are `Passed`, `Failed`, `Unsupported`, `Invalid`, `HarnessError`,
`Skipped`, and `NotApplicable` (selection filters may drop cases entirely;
manual cases are never skipped; `NotApplicable` is only a W3C declaration
recorded in the errata identity and counts outside the applicable
denominator). Counts also record `not_applicable` and `passed_under_erratum`
(a subset of `passed`). Each case records assertion/spec/data-model/conformance/manual,
stage, bounded reason code, detailed report-only reason/diagnostics,
configuration, terminal status, commands, and fuel. Reports group counts by spec
and data model.

Snapshots include corpus URL/path, revision, prepared corpus, acquired source-set, and manifest digests,
template digests, transformer identity/version/canonical argv/command-artifact digest, data model, runner version, IR
version, filters, and case results. Baseline comparison refuses mismatched
identity unless `--allow-baseline-mismatch` is explicit. Regressions are exactly
prior `Passed` → current non-`Passed`; new passes are tracked separately.
`--fail-on-regression` makes regressions nonzero.

The run-report JSON loader also recomputes its canonical report identity over
the suite, schema policy, compiler, engine, data-model, runner, and IR
versions, filters, caller revision, summary, and ordered case results, and fails closed when any bound field changes.

The canonical `scxml.report/v3` conformance payload uses conformance report
schema version 9. Its required `identity` object preserves the same complete
producer-owned evidence: corpus kind, manifest source and digest, pinned source
revision, acquired source-set digest, prepared-corpus digest, every template
digest, transformer name/version/portable argv/argv-and-artifact digest, the
errata identity (or `null`),
specialization data model, schema policy, compiler, engine, data-model, runner,
manual-equivalent oracle (`scxml-conformance-manual-oracle`, its table version,
and a digest of its source), and IR versions, every filter dimension, and the
optional implementation run revision. Every case whose execution reached oracle
evaluation (`Passed` at `Complete`, `Failed` at `Oracle`) records the `oracle`
kind that decided it: `outcome-log`, `manual-equivalent` (official manual
cases only), or `synthetic-expectation` (synthetic corpora only); every other
case omits it. Decoders reject any other combination. Every case also carries
`erratum`: the erratum id when it ran from an erratum-corrected artifact,
which must equal the errata identity's entry for that case, otherwise `null`;
a case is `NotApplicable` exactly when the errata identity declares it so. Set-like filter values are sorted and deduplicated before encoding.
The identity also carries the producer's report digest, which binds those fields
to the authoritative summary and complete case sequence. The lowercase SHA-256
is computed over the compact JSON serialization of
the complete typed conformance payload after replacing only
`identity.report_digest` with the empty string and normalizing the set-like
filter collections. Cases remain in producer canonical numeric-prefix-then-
lexical ID order; they are never lexically resorted by a reporter.
Decoding rejects missing or unknown identity fields, malformed SHA-256 values,
digest mismatches, duplicate case IDs/configuration entries, unknown stage or
severity values, impossible stage/disposition/execution combinations,
noncanonical ordering, invalid ranges, unsupported report versions, and official
IRP identities that omit revision, template, or transformer evidence. Each case
preserves the producer's specification number, manual classification, terminal
status, diagnostics, configuration, command count, fuel count, and oracle kind.

Rustc-style and SARIF reporters consume this validated canonical payload. They
may format its producer-authored identity, cases, dispositions, diagnostics, and
summary, but do not reload a manifest, reselect cases, rerun execution, or
recompute counts. A count claim is therefore bound to the exact identity in the
same canonical bytes.

### Checked-in measured baselines

The repository checks in reporter-facing `scxml.report/v3` canonical bytes
(conformance report schema version 9) for the default runs of both prepared
2015-03-10 suites under
[`tests/conformance/w3c-irp/baselines`](../tests/conformance/w3c-irp/baselines).
Both were produced by the component runner (`scxml-conformance/wasm`
`0.2.0`, engine `scxml-machine`, and the suite's `scxml-data-model-*`
component, each identity bound to the exact release component bytes),
Machine IR schema `5`, manual-equivalent oracle table version `4`, no
implementation revision, and the empty/default filter identity: every selector collection is empty,
and every selected case runs always, including manual cases, through the same
pipeline.

| Data model | Corpus SHA-256 | Producer report digest | Canonical artifact SHA-256 | Passed / Applicable | Under errata | Not applicable | Failed | Unsupported | Invalid | HarnessError | Skipped |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ECMAScript | `9f005f20c9cbcd7188507833d8fd8a71158cc49cf70e9428fd3fc807e11396ca` | `8f5ab67d3fdcaabfb32964c1728140a83b7aed7b8d87d8923867bda15d3c26c1` | `0b54ea5b2fe782eae5b16d20188b54eb55842e9147fa0e104e80480016ec0c92` | 201 / 201 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| XPath | `a661ef4af8bc62fc064cea5e137a6b469ede4b7da80ececf932d66d37cc93bca` | `85c6c5bd0f421a0c477818ade0f9aa5fdbf23c44124ea3a6e7f1ffce71d25a26` | `07a301af5f350c0e5040b24c3a68144e67ed996257894ee9cf554a94c351e91f` | 178 / 178 | 5 | 24 | 0 | 0 | 0 | 0 | 0 |

ECMAScript: 201 passed; mandatory 167 / 167, optional 34 / 34; 436 (null
profile, `datamodel="null"`) is `NotApplicable`. XPath: 178
passed, of which 5 under documented errata (241, 244, 307, 520, 534);
mandatory 163 / 163, optional 15 / 15; 24 cases are `NotApplicable` because
they never exercise the XPath data model: 301, 302, 303, and 304 (W3C: valid
only for data models that support scripting), 436 (null profile), and
444–446, 448, 449, 451–453, 456, 457, 459, 460, 557, 558, 560–562, 569, and
578 (ECMAScript profile). The cases decided by a manual-equivalent
oracle (178, 230, 250, 301, 307, 415, and 513, the last through the
external-client harness) pass in both suites except 301, which is
`NotApplicable` to XPath, as do the manifest-manual cases 313 and 314 under
the `Outcome` oracle.

Both artifacts also bind source-set digest
`b2f199cf442a78a6a58a94fbab287a873cb3fea6ef9fb2568e915e502c9bc9c8`,
manifest digest
`5974da27031fbe0deacf1101028a9afaaf1757d50b4c4b89ce514ef07310a73e`,
the data-model-specific template digest, and the sandboxed Saxon executable and
argument-vector identity; the XPath artifact also binds the errata identity
(file SHA-256
`72fe9e3ca418f55dadf64d2d6331af724cf8ebbc2a87a0180489db818aa73f2a`). Regenerate
the report bytes after explicitly preparing both suites with the pinned
commands above, from a release verified by
`scripts/verify-wasm-capabilities.sh`:

```console
cargo run --release --locked -p scxml-conformance --features wasmtime \
  --example generate_irp_baselines -- \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --extension-component-sha256 "$EXTENSION_SHA256"
```

The generator regenerates every checked-in baseline: both W3C corpora and
every namespace suite ([Namespace suites](#namespace-suites)). The
checked-in bytes were generated from the canonical release: the eight
components the conformance executor links and the example extension
component the namespace suites bind, as the `irp-components` CI job builds
them on Linux x86_64:

```console
sh scripts/verify-wasm-capabilities.sh --artifacts-dir "$RELEASE" \
  scxml_clock scxml_data_model_ecmascript scxml_data_model_null scxml_data_model_xpath \
  scxml_ioprocessor_scxml scxml_ioprocessor_basichttp scxml_compiler scxml_machine \
  scxml_extension_example
```

That build's `manifest.tsv` has SHA-256
`b0bff8b81a00395418c0a60be518f43f3a6c0fcc241cce251aa52311f4aea26c` (machine
component SHA-256
`9e18f84d1bc893f97c5a31e57ebd7a436e6a3e5f4887108bc51bfd8d6dbbaf06`), and
its example extension component SHA-256
`264e74de399ccce880300af36c7821beaca25b13813eb606582211418d05062e`. Another
selection, such as the default (every component), writes a different
manifest.

#### Canonical release identity

The canonical release identity is the Linux x86_64 build of the
`irp-components` CI job. Each host reproduces its own release bytes exactly,
but release component bytes differ between build hosts: Cargo derives each
crate's symbol-mangling hash partly from the host compiler's target triple,
so symbol names, and with them function layout, differ between, for
example, a Linux and a macOS build of the same source. A build on any other
host reproduces every case result, disposition, and configuration in the
baselines, but not the identity bytes (the component-bound source digests
and the report digest), so its regenerated report is not byte-identical.

To check identity bytes locally, download the `release-components` artifact
of the `irp-components` job for the commit you are on (from the workflow
run's page, or with
`gh run download <run-id> -n release-components -D ci-release`), then point
`--components` (or `SCXML_COMPONENTS_DIR`) at that directory and pass the
SHA-256 of its `manifest.tsv`:

```console
MANIFEST_SHA256=$(shasum -a 256 ci-release/manifest.tsv | cut -d ' ' -f 1)
SCXML_COMPONENTS_DIR="$PWD/ci-release" SCXML_COMPONENTS_MANIFEST_SHA256="$MANIFEST_SHA256" \
  cargo test --release --locked -p scxml-conformance --features wasmtime
```

Without CI, `scripts/build-canonical-components.sh` builds the same bytes on
any Docker host: it runs the job's build command
(`scripts/verify-wasm-capabilities.sh` over the eight executor components
and the example extension component,
with the reproducible release flags of `scripts/reproducible-wasm-env.sh`)
in the official `rust:1.98.0-bookworm` image pinned by digest and forced to
`linux/amd64` (emulated on other architectures), with the
`rust-toolchain.toml` toolchain, `wasm-tools` `1.244.0`, and `wasi-virt` at
the workflow's pinned revision. It snapshots the working tree into the
container, writes the verified artifacts and `manifest.tsv` to
`--artifacts-dir`, and prints the manifest SHA-256 and the example
extension component's:

```console
sh scripts/build-canonical-components.sh --artifacts-dir "$PWD/canonical-release"
```

Built from the source of the checked-in baselines on an arm64 macOS Docker
host, the script produces manifest SHA-256
`b0bff8b81a00395418c0a60be518f43f3a6c0fcc241cce251aa52311f4aea26c`, the
components the baselines were generated from; the `irp-components`
artifact of the same source must match it.

Regenerating the baselines after a source change also takes that artifact,
or the script's output: run the generator above with `--components` set to
that directory. Never regenerate them from components built on another
host. The session-snapshot goldens do not bind component identity
(docs/session-snapshot.md, "Parity fixtures") and pass on every host.

The browser host holds itself to the same baselines:
`hosts/scxml-host-browser/test/irp-parity.test.mjs` runs every official case
the `Outcome` oracle decides (194 ECMAScript and 172 XPath cases, the 12
BasicHTTP cases of each included over the Wasmtime host's in-process
loopback) through the browser host and requires the baseline's disposition
and final configuration.

`crates/scxml-conformance/tests/checked_in_irp.rs` verifies the corpus and
both artifacts statically: file, corpus, and producer digests, the component
runner, engine, and data-model identity names, and every published count.
`crates/scxml-conformance/tests/irp_components.rs` (`wasmtime` feature, the
`irp-components` CI job) runs both corpora through the shipped components,
lists any per-field mismatch against the baseline, requires the fresh
canonical artifact to equal the checked-in bytes exactly, and asserts the
exact passed, under-erratum, not-applicable, and manual-equivalent case
sets.

Without `--allow-not-passing`, `conformance run` exits nonzero whenever any
applicable selected mandatory case is not `Passed`. The development override
changes only process exit; it never removes or reclassifies gaps.

### Namespace suites

Each repository namespace has its own implementation report plan in the W3C
IRP format, run through exactly the same pipeline as the W3C IRP and
reported separately (never counted with it):
[`namespaces/env/irp`](../namespaces/env/irp/README.md) and
[`namespaces/event/irp`](../namespaces/event/irp/README.md). Each holds:

- `manifest.xml`: the W3C `assertions`/`assert`/`test`/`start`/`dep`
  format, one assertion per numbered statement of the namespace's README
  (`id` and `specid` name the statement's anchor, for example
  `#env-3`), every test mandatory and unattended;
- `sources/<id>/test<id>.txml`: test documents written with the W3C `conf:`
  vocabulary (`conf:pass`, `conf:fail`, `conf:datamodel`, `conf:location`,
  and so on), and their dependencies;
- `prepared/{ecmascript,xpath}/`: the documents specialized by the
  unmodified W3C templates (`tests/conformance/w3c-irp/templates`) with the
  same pinned SaxonJ-HE 12.8 transformer;
- `baselines/{ecmascript,xpath}.scxml-report.json` and `SHA256SUMS`.

The prepared corpora are `namespace-irp` corpora (suite id
`namespaces/<name>/irp`): prepare one with `conformance prepare` and
`--namespace NAME`, which records that provenance; a namespace corpus takes
no errata. A `test` may list `<reject code="..."/>` children: the document
must then be rejected by the canonical compiler at parse or validation with
exactly those distinct error codes, and it never runs. Such a case is
decided by the `document-rejection` oracle (`Passed` at `Complete`, or
`Failed` at `Oracle` with `document-accepted` or
`rejection-codes-mismatch`, with no configuration, terminal status, or
commands); the W3C manifest has none, and preparing the W3C IRP fails
closed on one. Every other case uses the W3C `Outcome` oracle unchanged.

```console
name=env  # or event
for model in ecmascript xpath; do
  scx conformance prepare \
    --manifest "namespaces/$name/irp/manifest.xml" \
    --source-dir "namespaces/$name/irp/sources" \
    --templates tests/conformance/w3c-irp/templates --datamodel "$model" \
    --cache "namespaces/$name/irp" \
    --output "namespaces/$name/irp/prepared/$model" \
    --revision 0.2.0 --namespace "$name" --saxon saxon --offline
done
```

A run binds the extension packages the suite uses (the example extension,
`components/scxml-extension-example`, for the `env` suite, nothing for
`event`):
`--extension-package MANIFEST.json` names the package, whose
`extension-artifact` is read from `--components DIR` and authenticated by
`--extension-component-sha256 URI=HEX`. The runner then compiles with
exactly those packages (and checks their `env` slots, `SCXML083`), treats
their elements as executable content, and supplies each case's dependencies
below its document's directory as its `event:schema` files. The run holds
exactly the harness's fake values for `env` slots, and the extension
components' only outbound transport is a deterministic in-process stand-in
for the server `<example:fetch>` reaches
(`crates/scxml-conformance/src/example_server.rs`, part of the runner
identity): it serves the base URL one held value supplies, answers each
request from its path (a documented table: whether the request carried a
given key or none), and counts each case's requests so a test can prove
that nothing was sent. Nothing touches the network. The
report identity lists every bound extension package (`extensions`: its
namespace URI, package version, and the SHA-256 of its manifest, schema,
and component bytes).

```console
EXAMPLE_URI='https://stateforward.org/scxml/example'
scx conformance run --corpus namespaces/env/irp/prepared/xpath \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --extension-package components/scxml-extension-example/package/example.extension.json \
  --extension-component-sha256 "$EXAMPLE_URI=$EXTENSION_SHA256" \
  --canonical-baseline namespaces/env/irp/baselines/xpath.scxml-report.json
```

The namespaces themselves do not depend on the data model; both data
models run, since `env` evaluates its expression attributes in the
document's data model and `event` must leave execution unchanged
under each. The current baselines (same runner, engine, and release as the
W3C baselines above; example extension component SHA-256
`264e74de399ccce880300af36c7821beaca25b13813eb606582211418d05062e`):

| Suite | Assertions | Tests (rejections) | ECMAScript | XPath |
| --- | ---: | ---: | ---: | ---: |
| `env` | 7 | 20 (6) | 20 / 20 | 20 / 20 |
| `event` | 9 | 29 (17) | 29 / 29 | 29 / 29 |

Every test is mandatory; nothing is `Failed`, `Unsupported`, `Invalid`,
`HarnessError`, `Skipped`, or `NotApplicable`. Each suite's README lists
its corpus, report, and artifact digests.
`crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks every
suite statically (each assertion cites a README statement, every source is
checked in, both corpora load and store exactly what they reference, and
the baselines' digests, identities, and counts), and `irp_components.rs`
reproduces every baseline byte for byte through the components.

## Synthetic harness proof

The checked-in synthetic corpus exercises classifications, denominator math,
filtering, tamper detection, snapshots, and CLI exits:

```console
cargo run --release -p scxml-cli --features wasmtime -- conformance run \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --corpus tests/conformance \
  --allow-not-passing \
  --json /tmp/synthetic-harness-report.json
```

Its identity is `synthetic-harness`, and its output must not be described as an
IRP pass count.

## Telemetry and cardinality

Conformance and CLI code, like the rest of the workspace, starts spans through
the lightweight `scxml-telemetry` OTEL API facade at boundaries only (a CLI
command, a preparation or run operation, a case execution, an artifact or I/O
call) and records each boundary's closed outcome and failure category; helpers,
getters, and inner loops run inside that span and emit nothing of their own
(see [OpenTelemetry and cardinality](../CONTRIBUTING.md#opentelemetry-and-cardinality)).
Native hosts may install an OTEL
provider/exporter. Wasm uses the versioned WIT adapter and imports host-owned
tracing; no guest SDK/exporter is added, and guests do not use the WASI
OpenTelemetry interfaces.

Allowed indexed attributes are closed and bounded:

| Key/signal | Value set | Cardinality |
| --- | --- | --- |
| `scxml.module` | static `Module` enum | bounded |
| `scxml.operation` | static `Operation` enum, one variant per boundary operation | bounded |
| `error.type` | static `ErrorCategory` enum | bounded |
| OTEL event name | static `SignalEvent` enum | bounded |
| Wasm operation/outcome | versioned WIT enums | bounded |
| component measurements | closed WIT `measurement-kind`: effects, input bytes, snapshot bytes, queue depth, fuel | one attribute-free sample on a kind-specific instrument |
| runtime limits | code-defined `RuntimeMeasurement` enum | one attribute-free sample on a kind-specific instrument |
| artifact sizes/counts | code-defined `ArtifactMeasurement` enum | one attribute-free sample on a kind-specific instrument |
| native facade tracer and meter name | `scxml` | 1 |
| Wasmtime host tracer name | `scxml-host-wasmtime`, `scxml-component-guest`, `scxml-capability-guest`, `scxml-guest` | 4 |
| Wasmtime host meter name | `scxml-host-wasmtime` | 1 |
| Wasmtime host-owned span name | `component-execute`, `machine-graph-link`, `machine-session`, `capability-graph-execute`, `session-run`, `component-compile` (one per session-host compiled-component cache miss, on the global tracer provider) | 6 |
| `scxml.outcome` on Wasmtime host-owned spans | `ok`, `failed` (on `component-execute`, `machine-graph-link`, `capability-graph-execute`, and `component-compile`; `machine-session` and `session-run` record failure only as the error status) | 2 |
| Wasmtime host-owned span event | `guest-telemetry-refused` | 1 |
| Wasmtime guest `scxml.module` | `machine-component`, `data-model-component`, `script-component`, `compiler-component`, `validator-component` (tracer `scxml-component-guest`); `capability-component` (tracer `scxml-capability-guest`) | 6 |
| Wasmtime compiler and validator guest span name and `scxml.operation` | `compile` (compiler), `validate` (validator); `scxml.outcome` `ok` or `failed` | 2 |
| Wasmtime compiler and validator guest span event | WIT `signal-event`: `input-rejected`, `artifact-produced` | 2 |
| Wasmtime compiler and validator guest `error.type` | WIT `error-category`: validator `invalid-input`, `validation`, `report`; compiler `invalid-input`, `validation`, `lowering`, `report`, `packaging` | ≤ 5 |
| `scxml-wasm-meter` `scxml.module`, `scxml.operation`, `error.type`, event name | `wasm-meter`; `scxml.wasm-meter.instrument` (one per `meter()` call), `scxml.wasm-meter.cli` (one per CLI run); `invalid-input`, `packaging` (instrument) plus `input-read`, `harness`, `report` (CLI); `scxml.transformation.completed`, `scxml.input.rejected` | 1; 2; ≤ 5; 2 |
| browser host spans and counter | `scxml.module` `host-browser`; 9 closed host operations; `ok`/`rejected`/`failed` | ≤ 27 series |

Measurement units and instruments are fixed by the host: effects use
`scxml.component.effects` (`{effect}`), input and snapshot sizes use
`scxml.component.input.size`/`scxml.component.snapshot.size` (`By`), queue
depth uses `scxml.component.queue.depth` (`{item}`), and fuel uses
`scxml.component.fuel` (`{fuel}`). Guest `observe` may carry one typed sample;
guest `event` carries no numeric sample, so an outcome/event pair cannot double
record the same measurement. Metrics have no labels or attributes.

Test IDs, paths, URLs, spec IDs, event/state IDs, expressions, messages, corpus
or artifact digests, source text, and diagnostics are unbounded and forbidden as
indexed telemetry attributes. They appear only in the reports and typed errors
that carry them, never in telemetry.

Telemetry must not alter deterministic results.
