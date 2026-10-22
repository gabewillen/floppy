# `behavior` implementation report plan

The implementation report plan of the `behavior` namespace, in the format of the
[W3C SCXML IRP](../../../scxml.rs/tests/conformance/w3c-irp/README.md) and of
scxml.rs's own namespace suites
([scxml.rs/docs/conformance.md](../../../scxml.rs/docs/conformance.md#namespace-suites)):
5 assertions, one per numbered statement of [`../README.md`](../README.md#statements)
("Statements"), tested by 18 mandatory documents. It runs through exactly
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
SAXON='java -jar /opt/saxon/saxon-he-12.8.jar' sh scripts/namespace-irp.sh prepare behavior
sh scripts/namespace-irp.sh baselines behavior
```

The current baseline (manifest SHA-256 `7859ec4537b319a391b64cfcdc21f781057b44813b51eff9e18e63340b4efe7c`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
measured on macOS arm64 when the namespace was added; regenerate it on
Linux x86_64, the host `tests/tests/namespace_irp.rs` compares fuel on)
measures:

| Data model | Passed / Applicable | Not Passing | Corpus digest | Report SHA-256 |
| --- | ---: | ---: | --- | --- |
| ECMAScript | 18 / 18 | 0 | `dbaa603476cf98afe53054d8d20d2b619d747fb3dcfeb2017c782cbb2b9bb7e0` | `64529499bc330c2fbc1cd9d736d7187ec7be621c965a9a3b54d9ceae2e17bc7b` |
