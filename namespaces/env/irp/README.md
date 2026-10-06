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

Every run binds the `openai` package (`../../openai/package/openai.extension.json`), the only package that declares slots, and its component. The run holds only the harness's fake values for `env` slots and
reaches only the harness's deterministic in-process stand-in for the OpenAI
Responses API (`crates/scxml-conformance/src/openai_service.rs`, whose
`model` table each test document's comment relies on); nothing touches the
network. A document passes when it logs `Outcome` `pass` and finishes in the
`pass` final state the templates generate, exactly as a W3C case does, or,
for a `<reject>` test, when the canonical compiler rejects it with exactly
the listed codes and nothing runs (the `document-rejection` oracle).

Regenerate the prepared corpora from the checked-in sources (no network;
macOS `sandbox-exec` isolation, as for the W3C corpus):

```console
for model in ecmascript xpath; do
  scxml-cli conformance prepare --manifest namespaces/env/irp/manifest.xml \
    --source-dir namespaces/env/irp/sources --templates tests/conformance/w3c-irp/templates \
    --datamodel "$model" --cache namespaces/env/irp \
    --output "namespaces/env/irp/prepared/$model" \
    --revision 0.2.0 --namespace env --saxon saxon --offline
done
```

and check a run against its baseline (`$RELEASE`, `$MANIFEST_SHA256`, and
`$OPENAI_SHA256` as in the `irp-components` snippet of
[CONTRIBUTING.md](../../../CONTRIBUTING.md)):

```console
scxml-cli conformance run --corpus namespaces/env/irp/prepared/xpath \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --extension-package namespaces/openai/package/openai.extension.json \
  --extension-component-sha256 "https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0=$OPENAI_SHA256" \
  --canonical-baseline namespaces/env/irp/baselines/xpath.scxml-report.json
```

The `generate_irp_baselines` example regenerates the baselines with every
other suite's. The current baselines (manifest SHA-256 `3ca06bb2da29117c7c450f76c749a5aa8b247f932d8a1a6a0cf6fd2846e3d14d`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 20 / 20 | 6 | 0 | `f4a43a5eddc3f7bb1178985ab2a8729829cd00d9e5e4145bcb166025725012d6` | `9bc362f4ca5540e9d8000403e8dceb80a5155673b886e8947b646e6447436d56` | `83595f7c1b00d80bee889587f30fd13f2f43ed8098853b05585248505347df05` |
| XPath | 20 / 20 | 6 | 0 | `04eb6dab4f637664cd3ffd71fb862ca01e75763982ca680664fa0dca48d346c9` | `0b9a861dadc2f3add3411f533ac77e8d10ae27694853a6a581e62303f474d2c4` | `225dc12b65157860b462373f8fe58e7be50f271b946c288b21b2a0fcec32f5ba` |

The report identity lists the bound package: `https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0` version `0.2.0`, manifest SHA-256 `2f9002da162a9f4f500c1e423098a49dc9b8db2f0832a28da35306c652e206c1`, schema SHA-256 `8117595b5835a700ac025ecde686d6384c9de0d0de01ed44319c3e61e311dd06`, component SHA-256 `47c6d37a035e7d723c9616c3d0975c1f4ed748aee0ef80a6014fefe92c0c64a9`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
