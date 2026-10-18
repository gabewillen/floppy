# `conversation` implementation report plan

The implementation report plan of the `conversation` namespace, in the format of the
[W3C SCXML IRP](../../../scxml.rs/tests/conformance/w3c-irp/README.md) and of
scxml.rs's own namespace suites
([scxml.rs/docs/conformance.md](../../../scxml.rs/docs/conformance.md#namespace-suites)):
8 assertions, one per numbered statement of [`../README.md`](../README.md#statements)
("Statements"), tested by 26 mandatory documents. It runs through exactly
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

The current baseline (manifest SHA-256 `ef3b8ee2c29e20c8ed0b96b709eafb8fac06b8a0a6ac80c3830910dbb304eb92`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
measured on macOS arm64 when the namespace took the original's conversation
module and turn detector; regenerate it on Linux x86_64, the host
`tests/tests/namespace_irp.rs` compares fuel on) measures:

| Data model | Passed / Applicable | Not Passing | Corpus digest | Report SHA-256 |
| --- | ---: | ---: | --- | --- |
| ECMAScript | 26 / 26 | 0 | `d89b8e1f1223f781beec99adf780159cd57116fa510c1a62198ca851e4b6423c` | `2022d958b6025f55ac0a9babb6b152d1514475057d1f8c1e01d9da66dfa67cf8` |
