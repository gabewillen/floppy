# `reading` implementation report plan

The implementation report plan of [`reading.scx`](../reading.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="reading-1"></a>**reading-1**. It offers nothing and answers only what it is given: on start it sends its parent nothing, and an input `bot.ability.reading.input` `{kind, content}` whose data its schema refuses (a kind other than `text` or `image`, or an empty `content`) is never routed to it (`error.execution` in the sender).
- <a id="reading-2"></a>**reading-2**. A text input whose text is not blank is classified as text (confidence 1) and comes back as `bot.ability.reading.output` `{text, source_kind: "text", confidence}`, its text the input's content unchanged; no model is called.
- <a id="reading-3"></a>**reading-3**. A text input whose text is only white space is classified as unreadable and comes back as `bot.ability.reading.output` `{text: "", source_kind: "unreadable", confidence: 1}`; no model is called.
- <a id="reading-4"></a>**reading-4**. An image input with a reference is classified as image and read by the generate service (one user message: the image decoder's prompt as a text part and the reference as an image part, the image read from the granted media directory); the reply, trimmed, comes back as `bot.ability.reading.output` `{text, source_kind: "image", confidence: 1}`.
- <a id="reading-5"></a>**reading-5**. A model call that fails answers `bot.ability.reading.failed` `{stage: "image_decoding", message: "Reading image decoder failed: <code>."}`, the generate service's failure code in the message, and the reading returns to Unfocused, taking the next input as on arrival.
- <a id="reading-6"></a>**reading-6**. An image reference the generate service cannot read (no such file in the granted media directory) rejects the model call's start: the reading answers `bot.ability.reading.failed` `{stage: "image_decoding", message: "the image decoding could not run (error.execution)"}`.
- <a id="reading-7"></a>**reading-7**. DecodingImage is bounded by `stage_timeout_ms` (30000 by default, a `<param>`): when it passes first, the reading answers `bot.ability.reading.failed` `{stage: "image_decoding", message: "the image decoding timed out after <stage_timeout_ms> ms"}` and the model call is cancelled.
- <a id="reading-8"></a>**reading-8**. An input that arrives while Focused is deferred: it is taken, in order, once the reading is back in Unfocused, so every input gets exactly one answer in the order the inputs arrived.
