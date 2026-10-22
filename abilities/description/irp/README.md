# `description` implementation report plan

The implementation report plan of [`description.scx`](../description.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="description-1"></a>**description-1**. Its input is its `<param>`s: it sends itself `bot.ability.vision.description.input` `{image, media_type}` (the original's media.Image, by reference), which its schema checks, so an `image` that is missing or empty sends nothing to the model and answers `bot.ability.failed` `{message: "Description could not run (error.execution)"}`; nothing reaches the parent before its one answer.
- <a id="description-2"></a>**description-2**. It asks the generate service once, with one user message (the original's DESCRIBE_INSTRUCTIONS as a text part and the image's reference as an image part, the image read from the granted media directory), and answers its parent `bot.ability.vision.description.output` `{description}`, the reply stripped of white space as the original's `str.strip()` strips it; then it ends (`done.invoke.<id>` follows the answer).
- <a id="description-3"></a>**description-3**. A reply that is empty once stripped answers `bot.ability.failed` `{message: "the generator answered with an empty description"}`.
- <a id="description-4"></a>**description-4**. A description of at most 4000 characters (the original's MAX_DESCRIPTION_CHARS, counted in characters, not bytes) is answered; a longer one is an output its schema refuses and answers `bot.ability.failed` `{message: "Description produced output that does not match its output schema."}`, the original's invalid-output failure.
- <a id="description-5"></a>**description-5**. A model call that fails answers `bot.ability.failed` `{message: "Description generator failed: <code>."}`, the generate service's failure code in the message, and an image reference the generate service cannot read rejects its start: `{message: "Description could not run (error.execution)"}`.
