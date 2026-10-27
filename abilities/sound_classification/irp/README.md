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

- <a id="sound_classification-1"></a>**sound_classification-1**. It is long-lived: it takes each sound as a request `bot.ability.hearing.sound.classification.input` `{id, audio, media_type?, kind?}` and answers its parent at once with `bot.ability.hearing.sound.classification.output`, the request's `id` and the label. It sends nothing else.
- <a id="sound_classification-2"></a>**sound_classification-2**. A kind that is a string with text is trimmed and is the one label, with confidence 1.0: `{id, labels: [kind], confidence: 1.0}`.
- <a id="sound_classification-3"></a>**sound_classification-3**. A kind that is null, absent or blank is unlabeled: `{id, labels: []}`, with no confidence.
