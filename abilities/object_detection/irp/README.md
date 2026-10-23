# `object_detection` implementation report plan

The implementation report plan of [`object_detection.scx`](../object_detection.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="object_detection-1"></a>**object_detection-1**. Its input is its `<param>`s: it sends itself `bot.ability.vision.detection.input` `{image, media_type}` (the image by reference), which its schema checks, so an `image` that is missing or empty sends nothing to the model and answers `bot.ability.failed` `{message: "ObjectDetection could not run (error.execution)"}`; nothing reaches the parent before its one answer.
- <a id="object_detection-2"></a>**object_detection-2**. It asks the generate service once, with one user message (its instructions, which hold no JSON, as a text part and the image's reference as an image part, the image read from the granted media directory) and the detections tool (decision D201), and answers its parent `bot.ability.vision.detection.output` `{objects}`, each object `{label, confidence, box: {x, y, width, height}}` as the call gave it; a call with no objects is `{objects: []}`; then it ends (`done.invoke.<id>` follows the answer).
- <a id="object_detection-3"></a>**object_detection-3**. The objects of every detections call of the reply are read, in order, as one list.
- <a id="object_detection-4"></a>**object_detection-4**. The detections tool's schema admits an object only as `label` a string of 1..100 characters, `confidence` 0..1 or `null` (required), `box` a box (`x`, `y` in 0..1, `width`, `height` in (0, 1]), its numbers JSON numbers (a boolean or a numeric string is refused), and no other keys; an object whose box passes the image's edge (`x + width` or `y + height` over 1 + 1e-6) is skipped, never failing the detection.
- <a id="object_detection-5"></a>**object_detection-5**. The first `max_objects` (32 by default, a `<param>`) objects, in reply order, are kept, then ordered by descending confidence, unrated last, equally rated objects keeping the reply's order.
- <a id="object_detection-6"></a>**object_detection-6**. Arguments the detections tool's schema refuses (an object it does not admit, or no `objects` array) fail the detection: `bot.ability.failed` `{message: "Object detection generator failed: schema-mismatch."}` (never the arguments themselves), and arguments that are not JSON `{message: "Object detection generator failed: not-json."}`.
- <a id="object_detection-7"></a>**object_detection-7**. A model call that fails answers `bot.ability.failed` `{message: "Object detection generator failed: <code>."}`, the generate service's failure code in the message, and an image reference the generate service cannot read rejects its start: `{message: "ObjectDetection could not run (error.execution)"}`.
- <a id="object_detection-8"></a>**object_detection-8**. An output its schema refuses (more than 32 objects, when the invoker allows more) answers `bot.ability.failed` `{message: "Object detection produced output that does not match its output schema."}`.
