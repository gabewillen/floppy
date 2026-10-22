# `vision` implementation report plan

The implementation report plan of [`vision.scx`](../vision.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="vision-1"></a>**vision-1**. On start it waits in `Unclassified` and sends nothing; it is long-lived: it never reaches a final state.
- <a id="vision-2"></a>**vision-2**. `bot.ability.vision.classification.input` `{kind: "text", content}` whose text is not blank is classified as text: it answers its parent with `bot.ability.vision.classification.output` `{kind: "text", confidence: 1.0}`.
- <a id="vision-3"></a>**vision-3**. An input `{kind: "image", content}` (its reference) is classified as image: `{kind: "image", confidence: 1.0}`; classifying reads nothing, so the reference need not name a file.
- <a id="vision-4"></a>**vision-4**. Anything else is unreadable: text that is only white space is answered `{kind: "unreadable", confidence: 1.0}`.
- <a id="vision-5"></a>**vision-5**. After answering it is back in `Unclassified`: every input is answered once, in order.
- <a id="vision-6"></a>**vision-6**. An input whose data does not satisfy its schema (`kind` other than text or image, a missing `content`, or an empty `content`, the original's `min_length=1`) is `error.execution` in its sender and is never classified.
- <a id="vision-7"></a>**vision-7**. The confidences are the classifier's `text_confidence`, `image_confidence` and `unreadable_confidence` (each 1.0 by default, the original's defaults), which the invoker may set as `<param>`s.
