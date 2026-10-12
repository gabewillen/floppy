# `sound_classification` implementation report plan

The implementation report plan of [`sound_classification.scx`](../sound_classification.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="sound_classification-1"></a>**sound_classification-1**. It takes the sound as the `<param>`s `audio`, `media_type` and `kind`, labels it once in `applying` and reaches its final `applied` at once; the label is its `<donedata>`, received by the invoker as `done.invoke.sound_classification`. It sends nothing else.
- <a id="sound_classification-2"></a>**sound_classification-2**. A kind that is a string with text is trimmed and is the one label, with confidence 1.0: `{labels: [kind], confidence: 1.0}`.
- <a id="sound_classification-3"></a>**sound_classification-3**. A kind that is null, absent or blank is unlabeled: `{labels: []}`, with no confidence. A kind that is no string is unlabeled too (the code takes only a string; the header names null and blank).
