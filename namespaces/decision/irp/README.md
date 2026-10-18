# `decision` implementation report plan

The implementation report plan of the `decision` namespace, in the format of the
[W3C SCXML IRP](../../../scxml.rs/tests/conformance/w3c-irp/README.md) and of
scxml.rs's own namespace suites
([scxml.rs/docs/conformance.md](../../../scxml.rs/docs/conformance.md#namespace-suites)):
11 assertions, one per numbered statement of [`../README.md`](../README.md#statements)
("Statements"), tested by 27 mandatory documents. It runs through exactly
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
SAXON='java -jar /opt/saxon/saxon-he-12.8.jar' sh scripts/namespace-irp.sh prepare decision
sh scripts/namespace-irp.sh baselines decision
```

The current baseline (manifest SHA-256 `80dec813566c93be572914288f3aac8b17820755d4e926601b90b1d1a5b15ebf`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
measured on Linux x86_64) measures:

| Data model | Passed / Applicable | Not Passing | Corpus digest | Report SHA-256 |
| --- | ---: | ---: | --- | --- |
| ECMAScript | 26 / 26 | 0 | `982c000e9b2d2cab703579f7ba9695418f79ce049578afb6337e965dfe2519f6` | `78647a737c39eead5dd1c3f4a1aa6e7a1ac8cba55cb4e1de15b5a98cf5d126ab` |
