# `conversation` implementation report plan

The implementation report plan of the `conversation` namespace, in the format of the
[W3C SCXML IRP](../../../scxml.rs/tests/conformance/w3c-irp/README.md) and of
scxml.rs's own namespace suites
([scxml.rs/docs/conformance.md](../../../scxml.rs/docs/conformance.md#namespace-suites)):
9 assertions, one per numbered statement of [`../README.md`](../README.md#statements)
("Statements"), tested by 30 mandatory documents. It runs through exactly
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

The current baseline (manifest SHA-256 `5cf6a657074ff9b7ef457c5f894acb3403aa7dd9fbbf7ab4796b086fd87547da`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
measured on Linux x86_64, the host `tests/tests/namespace_irp.rs` compares
fuel on, when the namespace took the original's `Message.participants` and
dropped `sequence`, 2026-10-22) measures:

| Data model | Passed / Applicable | Not Passing | Corpus digest | Report SHA-256 |
| --- | ---: | ---: | --- | --- |
| ECMAScript | 30 / 30 | 0 | `87fdadded2d7717cd1e5985757b2fa3b3566ccd99796c98345d7f620d3dd6676` | `ea4da5ed003d373316fb9b7ffdf6297801ee5d1cfaca03457a426acc97bc5a31` |
