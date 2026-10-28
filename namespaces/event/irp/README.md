# `event` implementation report plan

The implementation report plan of the `event` attribute vocabulary, in the format of the
[W3C SCXML IRP](../../../tests/conformance/w3c-irp/README.md): 9
assertions, one per numbered statement of [`../README.md`](../README.md#statements) ("Statements"), tested by 31
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
other suite's. The current baselines (manifest SHA-256 `1651fc1a929746acaee97fdfe5821711070421cedd7e2a5ea451e9997b6f8fbf`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 31 / 31 | 16 | 0 | `14f2e7b643902c2a1ecec6792a6f6c2114ad457ca59db8daa1b6d99e0811f464` | `6c8f496667ee1c0403fd6a57cd4357e3efc7fba78a5d5c7ee058585bfa96fdc5` | `dbd8610a84d5c27ac4356c75ca66b777ad09b0cfcfd7b61312d113768589a3d4` |
| XPath | 31 / 31 | 16 | 0 | `61a6243340c5699cfc62bd982740a125d33591eab5cf5ea95645fe3eed2ba920` | `cc9759be77349c8318a72e53aca502fcbfcbf5803cda185542cd90eb97941d37` | `ebd9878dfcb8d5c2a2ad66bf37cfa0f37ef68243d83abef375f10506e88c2d0c` |

The report identity lists no extension component. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
