# `event` implementation report plan

The implementation report plan of the `event` attribute vocabulary, in the format of the
[W3C SCXML IRP](../../../tests/conformance/w3c-irp/README.md): 9
assertions, one per numbered statement of [`../README.md`](../README.md#statements) ("Statements"), tested by 29
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
other suite's. The current baselines (manifest SHA-256 `d6bda3346705552f8ba109d7166cd14b353a10974a5f4d49e9d11779d8ca1ee3`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 29 / 29 | 17 | 0 | `466755eac36c408a41076a995cbe3e1d1257e8f7c057f939743becb740c7fe61` | `42123f077db971c655f3188baa9b5dbe5413af44465829c738cc3f2a9c13ccc6` | `b8bf92ce60c6f14a72be9702c261184218a34afd5b2d4f187e5270ef99c22bd6` |
| XPath | 29 / 29 | 17 | 0 | `6714a32d12b248d1a809d7a6b26b2c8f5df1b00dc77aa07bfad9945bd389b3db` | `0d7f408744ef8432ad3d10ba82fbe7152471ba58fb2ba1d0eb89a858a94cef8f` | `79fb9260329d6c307c061e2f0c8f760142655eac309c128aaeda5e87656ec183` |

The report identity lists no extension component. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
