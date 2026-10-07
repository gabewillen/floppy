# `env` implementation report plan

The implementation report plan of the `env` attribute vocabulary, in the format of the
[W3C SCXML IRP](../../../tests/conformance/w3c-irp/README.md): 7
assertions, one per numbered statement of [`../README.md`](../README.md#statements) ("Statements"), tested by 20
mandatory documents. It runs through exactly the pipeline the W3C IRP runs
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
(`../../../components/scxml-extension-example/package/example.extension.json`,
whose `<example:fetch>` has the `url` and `token` slots) and its component.
The run holds only the harness's fake values for `env` slots and reaches
only the harness's deterministic in-process stand-in for the server
`<example:fetch>` reaches (`crates/scxml-conformance/src/example_server.rs`,
whose path table each test document relies on); nothing touches the
network. A document passes when it logs `Outcome` `pass` and finishes in the
`pass` final state the templates generate, exactly as a W3C case does, or,
for a `<reject>` test, when the canonical compiler rejects it with exactly
the listed codes and nothing runs (the `document-rejection` oracle).

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
other suite's. The current baselines (manifest SHA-256 `d43479d57492b8065c5c93765b884ec579b7158e5ec340ae95563f60aa314642`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 20 / 20 | 6 | 0 | `e1d5b5d3eda19ad2c73782a372a05d38951300760ac35afa9099e5fc2288437b` | `d3a0cfa5b8ac8722759d5285a94ffb8b7393da67e8bfdefb8d5897d2d5b66939` | `049b9b265bb4059378422cef86fa56c07d60ca769a296a89c7d9a5f7cca75a15` |
| XPath | 20 / 20 | 6 | 0 | `2b7a48c5fe707d16b39368d05842a227ee20fc9e41ec8cb27e45dd33f2854bf0` | `20d7299e3e4bc9cb520e71686be7a1df61ab52b90a96b7e8b1aeaa7308cde168` | `4caf93252866b53b756f3f07aa65c2d6f1c610d32d2488f39265408b682adfb5` |

The report identity lists the bound package: `https://stateforward.org/scxml/example` version `0.2.0`, manifest SHA-256 `8833311c89a10942643edf66bd0e3ed66bfe10cb983d460ab260e6ea999725db`, schema SHA-256 `9b00f091f1c2ccd4610e389d5a9dbdf07624f0193bb2ba82b15ccce16af70e62`, component SHA-256 `70c98d76924282209198a4cb0da1afcc7ae56e28234ac3decec39663d30db07d`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
