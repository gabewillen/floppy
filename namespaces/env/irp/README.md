# `env` implementation report plan

The implementation report plan of the `env` attribute vocabulary, in the format of the
[W3C SCXML IRP](../../../tests/conformance/w3c-irp/README.md): 2
assertions, the numbered statements of [`../README.md`](../README.md#statements) ("Statements") that a document alone decides (env-1 and env-2), tested by 8
mandatory documents, each a document the canonical compiler must reject. The
other statements are about what a run does with values, which the
conformance runner cannot observe (it binds no invoked service yet); the
README names the tests that prove them. It runs through exactly the pipeline the W3C IRP runs
through (the canonical compiler, the shipped Wasm components under the
Wasmtime session host, the same oracles and report envelope) and is
reported on its own: these results are never W3C IRP results. See
[docs/conformance.md](../../../docs/conformance.md#namespace-suites).

| Path | What |
|---|---|
| `manifest.xml` | the assertions (`id` and `specid` name the README statement) and their tests; a `<reject code>` child marks a document the compiler must reject with exactly that code |
| `sources/<id>/test<id>.txml` | the test documents, in the W3C `conf:` vocabulary, and their dependencies |
| `prepared/{ecmascript,xpath}/` | the documents specialized by the unmodified W3C templates (`tests/conformance/w3c-irp/templates`) with SaxonJ-HE 12.8 |
| `baselines/` | the canonical `scxml.report/v3` reports of both data models and their `SHA256SUMS` |

Every run binds the example extension package
(`../../../components/scxml-extension-example/package/example.extension.json`)
and its component, so env-2's extension element is judged as executable
content of a bound namespace. A document passes when the canonical compiler
rejects it with exactly the listed codes and nothing runs (the
`document-rejection` oracle); nothing touches the network.

Regenerate the prepared corpora from the checked-in sources (no network;
macOS `sandbox-exec` isolation, as for the W3C corpus):

```console
for model in ecmascript xpath; do
  scx conformance prepare --manifest namespaces/env/irp/manifest.xml \
    --source-dir namespaces/env/irp/sources --templates tests/conformance/w3c-irp/templates \
    --datamodel "$model" --cache namespaces/env/irp \
    --output "namespaces/env/irp/prepared/$model" \
    --revision 0.2.0 --namespace env --saxon saxon --offline
done
```

and check a run against its baseline (`$RELEASE`, `$MANIFEST_SHA256`, and
`$EXTENSION_SHA256` as in the `irp-components` snippet of
[CONTRIBUTING.md](../../../CONTRIBUTING.md)):

```console
scx conformance run --corpus namespaces/env/irp/prepared/xpath \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --extension-package components/scxml-extension-example/package/example.extension.json \
  --extension-component-sha256 "https://stateforward.org/scxml/example=$EXTENSION_SHA256" \
  --canonical-baseline namespaces/env/irp/baselines/xpath.scxml-report.json
```

The `generate_irp_baselines` example regenerates the baselines with every
other suite's. The current baselines (manifest SHA-256 `76a666244f526ec93d6f72329880d05676744150431ed64fe833763a073d4860`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 8 / 8 | 8 | 0 | `a1a3f1b5aee8e8970a55efa5ba907c2fa0a95ec23917c9e167dc392f9b4b87a7` | `d6fd1cc308148cf87eda30e13dd4b3b1e51deff99b652a0fa1cc1d51e26f1f59` | `4f48c51a3812230354c641ab62ca9cbdc2d84cf4724fcaf2e533b9ae929aa8ce` |
| XPath | 8 / 8 | 8 | 0 | `ccb8ed7623b83986df1d407ef6033d33f2843676ce1b51ad58b6bcfbb841372c` | `454c1e62a0d1d33d63cd06ad7f199585c04b05c931b48bc3ef6fdb5408e5f8e4` | `9d68d751865030dc883e0946e5c4a8ed407c06049b7137811cfaae66a6eb3b6a` |

The report identity lists the bound package: `https://stateforward.org/scxml/example` version `0.2.0`, manifest SHA-256 `9e7a178076fc07942946095c44acde1433c9878b4f2a23c3569aa9c6205d03b9`, schema SHA-256 `45074422d68df4d82aa82e7b0dec09183871c58e59566033fdeaaeb4576b28b3`, component SHA-256 `e3bb851bc5d94f0fedb17e874edb6c0a84ae8771bd413387a40f9cc61a3ecdd1`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
