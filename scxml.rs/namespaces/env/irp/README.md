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
| ECMAScript | 8 / 8 | 8 | 0 | `915e73792e350c4840d1757bca1f29121f91053f606d96834cfef4791cfa90a9` | `ee5038476cbdfd6acdc2ebc689832ef7654979187f672521effbd84f90538cb7` | `0e6ab64b9131278d1eade2e067a91038f2ce90dd745c26ff3388f83e58d9ce34` |
| XPath | 8 / 8 | 8 | 0 | `536c72c62e8d7932706eb974c7134bb5082443807c283103f8381f04b906d267` | `faacd3a4b00270f567dbc95dc3f95953090b1ce685b0176ca1ca5e82248adca6` | `e0bb28aafc89e75edc18d8cc7b91f478bdbdd4c8de4b274187683b140eacf92f` |

The report identity lists the bound package: `https://stateforward.org/scxml/example` version `0.2.0`, manifest SHA-256 `9e7a178076fc07942946095c44acde1433c9878b4f2a23c3569aa9c6205d03b9`, schema SHA-256 `45074422d68df4d82aa82e7b0dec09183871c58e59566033fdeaaeb4576b28b3`, component SHA-256 `d0d50e16339a5aed0309323b2233bd7f8b9ecf784f79e1736cc842bf53b845dc`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
