# `conversation` implementation report plan

The implementation report plan of the `conversation` namespace, in the format of the
[W3C SCXML IRP](../../../scxml.rs/tests/conformance/w3c-irp/README.md) and of
scxml.rs's own namespace suites
([scxml.rs/docs/conformance.md](../../../scxml.rs/docs/conformance.md#namespace-suites)):
11 assertions, one per numbered statement of [`../README.md`](../README.md#statements)
("Statements"), tested by 37 mandatory documents. It runs through exactly
the pipeline the W3C IRP runs through (the canonical compiler, the shipped
Wasm components under the Wasmtime session host, the same oracles and report
envelope), with the namespace's own extension package and component bound,
and is reported on its own: these results are never W3C IRP results.

| Path | What |
|---|---|
| `manifest.xml` | the assertions (`id` and `specid` name the README statement) and their tests |
| `sources/<id>/test<id>.txml` | the test documents, in the W3C `conf:` vocabulary |
| `prepared/ecmascript/` | the documents specialized by the unmodified W3C ECMAScript template (`scxml.rs/tests/conformance/w3c-irp/templates`) with SaxonJ-HE 12.8 |
| `baselines/` | the measured report (`scx conformance run --json`) and its `SHA256SUMS` |

floppy's documents are ECMAScript only, so only the ECMAScript corpus is
prepared. A document passes when it logs `Outcome` `pass` and finishes in the
`pass` final state the template generates, exactly as a W3C case does. A
malformed element is tested at run time (the component raises
`error.execution` and assigns nothing): `scx conformance run` compiles with
schema policy `off`, so the namespace schema's own rejection is checked by
`scx validate` (`tests/tests/contracts.rs` over every floppy document) rather
than here. Nothing touches the network.

`tests/tests/namespace_irp.rs` runs the suite with `cargo test`, and
`scripts/namespace-irp.sh` prepares the corpus and regenerates the baseline:

```console
SAXON='java -jar /opt/saxon/saxon-he-12.8.jar' sh scripts/namespace-irp.sh prepare conversation
sh scripts/namespace-irp.sh baselines conversation
```

The current baseline (manifest SHA-256 `642f1c1139d88c6e54f6d42ce1239e76e4fd2c1a14f3d6853faa7df915a5f880`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
measured on Linux x86_64, the host `tests/tests/namespace_irp.rs` compares
fuel on, when the namespace took `<conversation:fold_voices>`,
`<conversation:people_to_merge>` and `<conversation:merged_context_refs>`
and `<conversation:people_for_input>` read representatives, decision
D205, 2026-10-24) measures:

| Data model | Passed / Applicable | Not Passing | Corpus digest | Report SHA-256 |
| --- | ---: | ---: | --- | --- |
| ECMAScript | 37 / 37 | 0 | `068ca9b7d03eda7e784a9ead90b61fffa1bc4fdde83adbfa115832376c1345f0` | `f8a2631b28a4c9aa1e7954620f4ed7ffde453d224a9a21f7d7baebf53949604c` |
