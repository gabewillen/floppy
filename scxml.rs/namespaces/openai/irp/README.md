# `openai` implementation report plan

The implementation report plan of the `openai` repository namespace, in the format of the
[W3C SCXML IRP](../../../tests/conformance/w3c-irp/README.md): 15
assertions, one per numbered statement of [`../README.md`](../README.md#statements) ("Statements"), tested by 64
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

Every run binds the `openai` package (`../package/openai.extension.json`) and its component. The run holds only the harness's fake values for `env` slots and
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
  scxml-cli conformance prepare --manifest namespaces/openai/irp/manifest.xml \
    --source-dir namespaces/openai/irp/sources --templates tests/conformance/w3c-irp/templates \
    --datamodel "$model" --cache namespaces/openai/irp \
    --output "namespaces/openai/irp/prepared/$model" \
    --revision 0.2.0 --namespace openai --saxon saxon --offline
done
```

and check a run against its baseline (`$RELEASE`, `$MANIFEST_SHA256`, and
`$OPENAI_SHA256` as in the `irp-components` snippet of
[CONTRIBUTING.md](../../../CONTRIBUTING.md)):

```console
scxml-cli conformance run --corpus namespaces/openai/irp/prepared/xpath \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --extension-package namespaces/openai/package/openai.extension.json \
  --extension-component-sha256 "https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0=$OPENAI_SHA256" \
  --canonical-baseline namespaces/openai/irp/baselines/xpath.scxml-report.json
```

The `generate_irp_baselines` example regenerates the baselines with every
other suite's. The current baselines (manifest SHA-256 `23c2953a82d203d833dd58025fee56e50497cfca1bff48130e358c8d3c3b4160`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 64 / 64 | 0 | 0 | `abb681e9efd7048d69b27828da467f7400425c89492d3b31421e77f0e8e623c9` | `5eb73d740ddd199c49fd97bd83f8ecc32a37314f20aef179d46788411ca662a1` | `d24f242650f060b8b78a65d8ce8e7e24c3c39f887d490649f8d290866f948a46` |
| XPath | 64 / 64 | 0 | 0 | `868c6296b26e640305501829950f1ebcb46f03254283009723d0be02435d480a` | `393b2d8bc1a1ceb0427b65bd1e9b91626bfc1db9f0bb138d68c85b31f2d45dc3` | `b34802155be5d9a7e24979c6ccd9ddb69c9e2b6e8942850cc3ddbe18ce3ef4c1` |

The report identity lists the bound package: `https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0` version `0.2.0`, manifest SHA-256 `2f9002da162a9f4f500c1e423098a49dc9b8db2f0832a28da35306c652e206c1`, schema SHA-256 `8117595b5835a700ac025ecde686d6384c9de0d0de01ed44319c3e61e311dd06`, component SHA-256 `47c6d37a035e7d723c9616c3d0975c1f4ed748aee0ef80a6014fefe92c0c64a9`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
