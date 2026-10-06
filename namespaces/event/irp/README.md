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

A run binds no extension package: the vocabulary is the toolchain's own. The run holds only the harness's fake values for `env` slots and
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
  scxml-cli conformance prepare --manifest namespaces/event/irp/manifest.xml \
    --source-dir namespaces/event/irp/sources --templates tests/conformance/w3c-irp/templates \
    --datamodel "$model" --cache namespaces/event/irp \
    --output "namespaces/event/irp/prepared/$model" \
    --revision 0.2.0 --namespace event --saxon saxon --offline
done
```

and check a run against its baseline (`$RELEASE`, `$MANIFEST_SHA256`, and
`$OPENAI_SHA256` as in the `irp-components` snippet of
[CONTRIBUTING.md](../../../CONTRIBUTING.md)):

```console
scxml-cli conformance run --corpus namespaces/event/irp/prepared/xpath \
  --components "$RELEASE" --components-manifest-sha256 "$MANIFEST_SHA256" \
  --canonical-baseline namespaces/event/irp/baselines/xpath.scxml-report.json
```

The `generate_irp_baselines` example regenerates the baselines with every
other suite's. The current baselines (manifest SHA-256 `d6bda3346705552f8ba109d7166cd14b353a10974a5f4d49e9d11779d8ca1ee3`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
the canonical release of docs/conformance.md, empty filter identity) measure:

| Data model | Passed / Applicable | Rejections | Not Passing | Corpus digest | Producer report digest | Artifact SHA-256 |
| --- | ---: | ---: | ---: | --- | --- | --- |
| ECMAScript | 29 / 29 | 17 | 0 | `466755eac36c408a41076a995cbe3e1d1257e8f7c057f939743becb740c7fe61` | `0963d44f06123fdeea788dd92694a35e91a5bd0f8969df69e706f767bef82a09` | `e6bd35e02064b0d74456510708d4864d8448c9ec3af06006c127d1e8ec842ee4` |
| XPath | 29 / 29 | 17 | 0 | `6714a32d12b248d1a809d7a6b26b2c8f5df1b00dc77aa07bfad9945bd389b3db` | `f2501f311b97448dac5ede8023efd61c693c59e6465a3c813ea4c87d8910aae5` | `be86a80c71eae366bcd62539c2d969a9e2ae034327e1dd82470b4d300b3d3feb` |

The report identity lists no extension component. `crates/scxml-conformance/tests/checked_in_namespace_irp.rs` checks
the suite and its baselines statically, and `irp_components.rs` reproduces
both baselines through the components byte for byte.
