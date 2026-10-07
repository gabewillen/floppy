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
| ECMAScript | 8 / 8 | 8 | 0 | `a1a3f1b5aee8e8970a55efa5ba907c2fa0a95ec23917c9e167dc392f9b4b87a7` | `7c6d6bd979c2116eb395e0148a0e72032fdb98a3558c1a1a5eeea8b1b2b19979` | `85611d3e4ef5816a851be604b2f2037dc488614c06634fc8961d4afea3f78736` |
| XPath | 8 / 8 | 8 | 0 | `ccb8ed7623b83986df1d407ef6033d33f2843676ce1b51ad58b6bcfbb841372c` | `8d050c442ca49fac449db1e307d8abe6b914dd63b1233ba4ac5066b5d3b7fd09` | `bdeacbd39888eb2e9920e2d283eaf155932751970638b19deb6cc6a1c4b45445` |

The report identity lists the bound package: `https://stateforward.org/scxml/example` version `0.2.0`, manifest SHA-256 `9e7a178076fc07942946095c44acde1433c9878b4f2a23c3569aa9c6205d03b9`, schema SHA-256 `45074422d68df4d82aa82e7b0dec09183871c58e59566033fdeaaeb4576b28b3`, component SHA-256 `4028335f1ac401199302e60207c31dd7cea36cf687e19f90ccfbad99cc306468`. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
