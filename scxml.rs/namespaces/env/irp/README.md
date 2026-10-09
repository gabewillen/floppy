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
other suite's. The current baselines (manifest SHA-256 `6a84d3fd35c2f15d10030b4a8ebf2562d1f843b742fd524f1ab415d224292d02`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 8 / 8 | 8 | 0 | `915e73792e350c4840d1757bca1f29121f91053f606d96834cfef4791cfa90a9` | `fe6ffff9593bb5f3d59b5d4b1d69f33405163b4ad58f02c1d7d346664565a01a` | `88725e6b244568ba1c5edb7cbb4e9d3202eb7f80503a2faecc38884eae72f750` |
| XPath | 8 / 8 | 8 | 0 | `536c72c62e8d7932706eb974c7134bb5082443807c283103f8381f04b906d267` | `42d061b5dbe7622133b1237f07000bd51a522e4a23c670228f89813d17bd9e27` | `b83ab1e9d7b7d2d5100cb98b47c633de56f4c89583d4c2ee58a1b9d65358efe1` |

The report identity lists the bound package: `https://stateforward.org/scxml/example` version `0.2.0`, manifest SHA-256 `9e7a178076fc07942946095c44acde1433c9878b4f2a23c3569aa9c6205d03b9`, schema SHA-256 `45074422d68df4d82aa82e7b0dec09183871c58e59566033fdeaaeb4576b28b3`, component SHA-256 `d0d50e16339a5aed0309323b2233bd7f8b9ecf784f79e1736cc842bf53b845dc`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
