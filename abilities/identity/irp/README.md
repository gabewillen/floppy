# `identity` implementation report plan

The implementation report plan of [`identity.scx`](../identity.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="identity-1"></a>**identity-1**. Without a database it recalls the name it was started with (the `<param>` `name`, else none) and is ready: on entering knowing it sends `bot.ability.offers` `{ability: "identity", choices}`, choices the one model-callable `bot.ability.identity.adopt` with its schema compacted (no `$schema`, title or examples) and the schema's description, no target.
- <a id="identity-2"></a>**identity-2**. With a database it recalls the last adopted name from memory, else the name it was started with, and is ready with the same offer; a recall attempt that fails or does not answer within `recall_attempt_timeout_ms` is asked again after `recall_retry_delay_ms`, and when the bring-up budget (`bring_up_timeout_ms`) runs out first it answers `bot.ability.failed` `{message: "Identity could not recall a name: memory did not answer within the bring-up budget."}` and then recalls the name it was started with.
- <a id="identity-3"></a>**identity-3**. A named identity that hears its name in `bot.ability.communication.input` text (string content of a `text/*` content_type; the name's words, split at white space, case-folded and keeping only their letters and digits, appear as a whole run of the text's) hands the body `bot.ability.cognition.input` `{stimulus}`, the stimulus `{event: "bot.ability.identity.addressed", data: {name}, source: "identity"}`.
- <a id="identity-4"></a>**identity-4**. Nothing else it hears produces anything: text that does not name it (the name only as part of a word included), content that is not `text/*`, and anything heard while it has no name.
- <a id="identity-5"></a>**identity-5**. `bot.ability.identity.adopt` `{name, reason?}` takes the name and re-enters knowing, offering again; without a database nothing is written, with one the name is written to memory before it is taken. From then on it answers to that name.
- <a id="identity-6"></a>**identity-6**. A named identity decodes each `environment.sound` through the speech decoding (the speech service) and looks for its name in the transcript: addressed, it hands the body the same `bot.ability.cognition.input` as for text; a nameless one decodes nothing.
- <a id="identity-7"></a>**identity-7**. A decoding that fails answers `bot.ability.failed` `{message}` (the speech decoding's failure message) and the identity goes back to listening.
- <a id="identity-8"></a>**identity-8**. Communication input, sounds and adopt events that arrive while it is recalling or adopting, and input and sounds that arrive while it is recognizing, are deferred and taken, in order, once it is back in knowing (or listening).
- <a id="identity-9"></a>**identity-9**. `bot.ability.identity.ask` `{}` while knowing is answered with `bot.ability.identity.named` `{name}` to the parent, the name it answers to or null; an ask that arrives while recalling or adopting is deferred and answered once it knows; and every adoption sends `bot.ability.identity.named` with the adopted name once it is taken.
