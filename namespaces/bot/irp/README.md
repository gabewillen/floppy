# `bot` implementation report plan

The implementation report plan of the `bot` namespace, in the format of the
[W3C SCXML IRP](../../../scxml.rs/tests/conformance/w3c-irp/README.md) and of
scxml.rs's own namespace suites
([scxml.rs/docs/conformance.md](../../../scxml.rs/docs/conformance.md#namespace-suites)):
2 assertions, one per numbered statement of [`../README.md`](../README.md#statements)
("Statements"), tested by 7 mandatory documents. It runs through exactly
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
SAXON='java -jar /opt/saxon/saxon-he-12.8.jar' sh scripts/namespace-irp.sh prepare bot
sh scripts/namespace-irp.sh baselines bot
```

The current baseline (manifest SHA-256 `2e8aa941444656e39620ae269ff0519b5bf106a3e35eaa04fd4521035508baec`,
component runner `scxml-conformance/wasm` `0.2.0`, Machine IR schema `5`,
measured on macOS arm64 after `<bot:recognize>` moved to the `identity`
namespace as `<identity:hears_name>`; regenerate it on Linux x86_64, the host
`tests/tests/namespace_irp.rs` compares fuel on) measures:

| Data model | Passed / Applicable | Not Passing | Corpus digest | Report SHA-256 |
| --- | ---: | ---: | --- | --- |
| ECMAScript | 7 / 7 | 0 | `a9fdf8d6f04925486cb4995b1371a6a4c5a57cc67ebd930390583ce938e9cf70` | `d91973c8fe054c7164099ace0d317df598da9925915f3d2c7c71f87bd1c71aa2` |
