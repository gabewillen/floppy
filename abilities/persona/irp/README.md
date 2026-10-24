# `persona` implementation report plan

The implementation report plan of [`persona.scx`](../persona.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="persona-1"></a>**persona-1**. Without a database it wakes with no description: on entering knowing it sends `bot.ability.persona.output` `{traits, description: null}` to the parent, traits the `<param>` `traits` in the owner's order with the white space around each stripped, then `bot.ability.ready`, once and with no data (decision D242). What it offers, read from its live session (`<decision:offers>`), is the one model-callable `bot.ability.persona.revise` with its effort (`revise_effort`, 2), its schema and the schema's description, no target.
- <a id="persona-2"></a>**persona-2**. With a database it recalls the most recent description it wrote (query_tags `self_persona`, subject_ref `self`), else none, and tells and is ready the same way; a recall attempt that fails or does not answer within `recall_attempt_timeout_ms` is asked again after `recall_retry_delay_ms`, and when the bring-up budget (`bring_up_timeout_ms`) runs out first it answers `bot.ability.failed` `{message: "Persona could not recall a description: memory did not answer within the bring-up budget."}` and then tells `bot.ability.persona.output` with no description.
- <a id="persona-3"></a>**persona-3**. `bot.ability.persona.revise` `{text, reason}` writes the description down (with a database; without one nothing is written) before it becomes the bot's, then tells `bot.ability.persona.output` `{traits, description: {text, reason, traits, kind: "fact"}}`, the owner's traits it was written under included, and offers it still (it is not ready again); each revision replaces the description before it.
- <a id="persona-4"></a>**persona-4**. A revise its schema refuses (a blank or empty text or reason, a text over 2000 characters, a reason over 500, any other field) is never routed: the sender gets `error.execution`, and the persona is unchanged.
- <a id="persona-5"></a>**persona-5**. A revise that cannot be written down is not taken: it answers `bot.ability.failed` `{message: "Persona could not remember its revision: ..."}`, tells nothing, keeps the description it had, and offers it still.
- <a id="persona-6"></a>**persona-6**. A revise that arrives while it is recalling or revising is deferred and taken, in order, once it is back in knowing.
- <a id="persona-7"></a>**persona-7**. Traits that are not 1 to 24 traits of at most 200 characters (none given included) are refused where the output is created: it is never ready and ends, its invoker getting `done.invoke.persona` with `{message}`.
