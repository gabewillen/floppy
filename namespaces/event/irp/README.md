# `event` implementation report plan

The implementation report plan of the `event` attribute vocabulary, in the format of the
[W3C SCXML IRP](../../../tests/conformance/w3c-irp/README.md): 9
assertions, one per numbered statement of [`../README.md`](../README.md#statements) ("Statements"), tested by 30
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

A run binds no extension package: the vocabulary is the toolchain's own,
and nothing touches the network. A document passes when it logs `Outcome` `pass` and finishes in the
`pass` final state the templates generate, exactly as a W3C case does, or,
for a `<reject>` test, when the canonical compiler rejects it with exactly
the listed codes and nothing runs (the `document-rejection` oracle).

Regenerate the prepared corpora from the checked-in sources (no network;
macOS `sandbox-exec` isolation, as for the W3C corpus):

```console
for model in ecmascript xpath; do
  scx conformance prepare --manifest namespaces/event/irp/manifest.xml \
    --source-dir namespaces/event/irp/sources --templates tests/conformance/w3c-irp/templates \
    --datamodel "$model" --cache namespaces/event/irp \
    --output "namespaces/event/irp/prepared/$model" \
    --revision 0.2.0 --namespace event --saxon saxon --offline
done
```

and check a run against its baseline (`$RELEASE` and `$MANIFEST_SHA256` as
in the `irp-components` snippet of
[CONTRIBUTING.md](../../../CONTRIBUTING.md)):

```console
scx conformance run --corpus namespaces/event/irp/prepared/xpath \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --canonical-baseline namespaces/event/irp/baselines/xpath.scxml-report.json
```

The `generate_irp_baselines` example regenerates the baselines with every
other suite's. The current baselines (manifest SHA-256 `752489322a87da78a2a026b58f6f663e507b2822ac5d092d911088fe3960a50d`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 30 / 30 | 17 | 0 | `deed5d764a3e7991a750336b098442c00f7bc688c37f28c340fc0e73bd279e04` | `b1b84a3332c708c15d40311a54115f9376b90f2ae8f406a5a1fe2c1030a906f6` | `a04d3824ad73ec01c8da2ee71b2b350b26e009868d3e4bbe98790fd49bdb0956` |
| XPath | 30 / 30 | 17 | 0 | `8f38379d291faa6a264c3ca33872f414dd9720cc1ae1f4c98f0dc099baa4ad2f` | `33aee575e73722687ffd3e9d494083456e681008cc31e69a45e311514e40fd71` | `c935e9f949186fae34611ac884593c74aa2d4f63b7abfbd16b82e9d04830d6e1` |

The report identity lists no extension component. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
