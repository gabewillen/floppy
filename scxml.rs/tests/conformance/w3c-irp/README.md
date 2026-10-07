# W3C SCXML Implementation Report corpus

This directory pins the complete SCXML 1.0 Implementation Report test corpus
published by W3C on 10 March 2015. It contains 200 assertions, 202 test starts,
and 9 declared dependencies. The 202 starts comprise 201 `.txml` machines and
one opaque manual-test resource; 9 starts are marked manual.

`upstream/manifest.xml` and `upstream/sources/` are unmodified bytes acquired
from `https://www.w3.org/Voice/2013/scxml-irp/`. Their manifest SHA-256 is
`5974da27031fbe0deacf1101028a9afaaf1757d50b4c4b89ce514ef07310a73e`,
and their deterministic source-set SHA-256 is
`b2f199cf442a78a6a58a94fbab287a873cb3fea6ef9fb2568e915e502c9bc9c8`.

`templates/ecmascript.xsl` and `templates/xpath.xsl` are the unmodified W3C
stylesheets linked by the Implementation Report. Their SHA-256 identities are:

- ECMAScript: `f93de8fb0a72e53419997075d053f75d49152a973c28b45e3ad576745fb372aa`
- XPath: `765aed3e6c280c316b73d783b4619072506ddb1f897f1e8be61b3b727446bba7`

`prepared/` contains deterministic SCXML specializations generated with
SaxonJ-HE 12.8. Each schema-v6 `corpus.json` records the exact inputs, the
transformer's portable argv and its argv-and-JAR digest, required OfficialIrp revision,
per-case digests, dependencies, and output identity. The `agentmlx`
conversion pipeline of the separately maintained AgentML project (revision
`f20e151333a424f7ec7357d0583cbcf61ea92b08`) was consulted as an external
behavioral reference only; it is not needed to build, test, or verify anything
here. Its AgentML-namespace ECMAScript and Starlark adaptations are
intentionally not copied because they are not SCXML documents. The pinned outputs instead use the
unmodified W3C ECMAScript and XPath transformations and preserve the exact
`http://www.w3.org/2005/07/scxml` namespace.

Regenerate from pinned local bytes, without network access:

```console
cargo run -p scxml-cli -- conformance prepare \
  --manifest tests/conformance/w3c-irp/upstream/manifest.xml \
  --base-uri https://www.w3.org/Voice/2013/scxml-irp \
  --source-dir tests/conformance/w3c-irp/upstream/sources \
  --templates tests/conformance/w3c-irp/templates \
  --datamodel ecmascript \
  --cache tests/conformance/w3c-irp/upstream \
  --output tests/conformance/w3c-irp/prepared/ecmascript \
  --revision 2015-03-10 \
  --saxon saxon \
  --offline
```

Add `--errata tests/conformance/w3c-irp/errata/ecmascript.json`, and repeat
with `--datamodel xpath`, the XPath output directory, and
`--errata tests/conformance/w3c-irp/errata/xpath.json` for that variant.

`errata/xpath.json` documents upstream XPath template and `.txml` defects in
official cases 241, 244, 307, 520, and 534, each with exact line
evidence, the minimal correction of the template or source, and the SHA-256 of
both the output the unmodified upstream inputs produce and the corrected
output; preparation fails closed when either digest drifts. It also declares
24 cases not applicable to XPath because they never exercise the XPath data
model, each with quoted W3C evidence: 301, 302, 303, and 304, quoting the W3C
manifest ("valid only for datamodels that support scripting") and
`templates/xpath.xsl:631-637` ("scripting is not supported in Xpath ...
Implementers should know not to bother to run these tests"); 436, whose
B.1 null-profile document fixes `datamodel="null"`; and the 19 B.2
ECMAScript-profile cases 444–446, 448, 449, 451–453, 456, 457, 459, 460,
557, 558, 560–562, 569, and 578, whose documents fix
`datamodel="ecmascript"`. The upstream bytes and templates in this
directory are never modified: corrected inputs and outputs live beside the
unmodified upstream outputs under each case's `errata/<erratum-id>/`
artifact directory, and the errata identity (file SHA-256
`72fe9e3ca418f55dadf64d2d6331af724cf8ebbc2a87a0180489db818aa73f2a`) is bound
into the prepared corpus and every report. See
[docs/conformance.md](../../../docs/conformance.md#irp-errata-and-not-applicable-cases).
The ECMAScript variant (`errata/ecmascript.json`, file SHA-256
`54c7f1cb44686e2bf499a0edf2596d311f4623245b2fe380abf0ac5b4663303f`) has no errata and declares
436 not applicable: its document fixes `datamodel="null"`, and the template
writes its outcomes as value expressions, which SCXML 1.0 §B.1.4 rules out
for the null data model ("There is no value expression language.").

The recorded transformer identity is portable: the argv has the
`sandbox-exec` wrapper removed and install paths reduced to file names, and
its digest covers that argv and the SaxonJ-HE 12.8 JAR bytes. The sandbox
policy and JVM are verified during preparation but not recorded, so a
different install location does not change the identity; a different Saxon
JAR or command shape does, and then requires regenerating the baselines below.
Offline preparation on Linux and other hosts currently fails closed because no
verified process-isolation mechanism exists there. Running the checked-in
prepared corpora needs no transformer and works on every host, because the
root `.gitattributes` keeps these digest-bound bytes unconverted on every
checkout, whatever `core.autocrlf` is set to.
Acquisition or regeneration is explicit; normal tests and conformance runs are
offline. These prepared artifacts establish corpus identity and harness coverage; the
measured results are the baselines below.

After both prepared suites are regenerated, regenerate their canonical measured
run evidence through the shipped components, from a release verified by
`scripts/verify-wasm-capabilities.sh` (`$RELEASE`, `$MANIFEST_SHA256`, and
`$EXTENSION_SHA256` are built as in the `irp-components` snippet of
[CONTRIBUTING.md](../../../CONTRIBUTING.md)); the generator also regenerates
every namespace suite's baselines
([docs/conformance.md](../../../docs/conformance.md#namespace-suites)):

```console
cargo run --release --locked -p scxml-conformance --features wasmtime \
  --example generate_irp_baselines -- \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --extension-component-sha256 "$EXTENSION_SHA256"
```

`baselines/SHA256SUMS` binds the exact reporter-facing `scxml.report/v3` bytes.
`checked_in_irp.rs` independently checks those hashes, canonical decoding,
the embedded identity, and published disposition counts; `irp_components.rs`
(the `irp-components` CI job) runs both corpora through the components and
requires fresh byte-for-byte regeneration. The baselines are generated from
the canonical release, the Linux x86_64 build of that job, which it uploads
as the `release-components` workflow artifact, and which
`scripts/build-canonical-components.sh` reproduces on any Docker host; other
hosts' native builds reproduce every case result but not the identity bytes
([docs/conformance.md](../../../docs/conformance.md), "Canonical release
identity").
The current baselines (component runner `scxml-conformance/wasm` `0.2.0`,
engine `scxml-machine`, Machine IR schema `5`, conformance report schema `9`,
empty filter identity, every case including manual cases run) measure:

| Data model | Passed / Applicable | Under errata | Not applicable | Mandatory Passed | Not Passing | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| ECMAScript | 201 / 201 | 0 | 1 | 167 / 167 | 0 | `2b15f97fadfadf86c2ed33e6742e0c33eb50ea61f90d186fc4c80cc088a7635c` | `9a8f284ac14f3ad3df8047555aded7fb1f7856df9b8dd2ddd454e6cf461a9306` |
| XPath | 178 / 178 | 5 | 24 | 163 / 163 | 0 | `06d8d5356fb58931e52e078e68f63163fd1cb2dc3879972624f37f0a14303f13` | `e2fe893e834e088e35cfc35db8945add8b8b4c208ff3ea191bd3c411d62138e7` |

The ECMAScript corpus digest is
`9f005f20c9cbcd7188507833d8fd8a71158cc49cf70e9428fd3fc807e11396ca` and the
XPath corpus digest is
`a661ef4af8bc62fc064cea5e137a6b469ede4b7da80ececf932d66d37cc93bca`. XPath
passes 241, 244, 307, 520, and 534 under the documented errata and
reports the 24 cases above as `NotApplicable`, each with its reason. Neither suite has a `Failed`,
`Unsupported`, `Invalid`, or `HarnessError` case. See
[docs/conformance.md](../../../docs/conformance.md#checked-in-measured-baselines).

See [LICENSE.md](LICENSE.md) for the upstream dual-license notice.
