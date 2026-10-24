# `reasoning` implementation report plan

The implementation report plan of [`reasoning.scx`](../reasoning.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="reasoning-1"></a>**reasoning-1**. It runs one turn (the `<param>`s turn, stimulus, focus, focus_candidates, choices, model and database) and ends: its `<donedata>`, received as `done.invoke.reasoning`, is `{turn, handled: true, selected}` or `{turn, handled: false, failed: true, reason}`, turn the given turn.
- <a id="reasoning-2"></a>**reasoning-2**. With no choices it fails the turn without a model call: reason `the reasoning has no choices to select from`.
- <a id="reasoning-3"></a>**reasoning-3**. With choices it makes one generate call offering the dispatch tool, which allows only the offered events (with their targets); the events of its dispatch calls, in order, are returned as the model gave them with `handled: true`, each `{event, data, reason, target}`, `bot.ability.cognition.ignore` selections included; an empty selection is handled with nothing selected.
- <a id="reasoning-4"></a>**reasoning-4**. A generate call that fails fails the turn with reason `the reasoning failed (<code>)`, the generate service's failure code: `status` for an error status, `truncated` for a reply cut short, `malformed` for an answer that is no chat completion. A selection the dispatch tool refuses (`schema-mismatch`, or `not-json`) is repaired first (decision D142): the call is made again with the rejection as the prompt's feedback, at most twice, and the same rejection twice running, or one once the repairs are spent, fails the turn with reason `Selection rejected: <message>` (the rejection's message, else its code).
- <a id="reasoning-5"></a>**reasoning-5**. A prompt over the projection budgets (262144 encoded bytes, 4096 nodes, 4096 collection items, 65536 characters per scalar) fails the turn without a model call, its reason the budget's message (for example `prompt projection collection item budget exceeded`).
- <a id="reasoning-6"></a>**reasoning-6**. Each selection is sent, in order, to the session its target names (`#_scxml_` + target) before the turn ends; a selection without its data is refused by the dispatch tool, whose every branch requires it (decision D231, where D113 sent it with `{}`); a send that fails fails the turn with reason `the reasoning could not run (<error>)`.
- <a id="reasoning-7"></a>**reasoning-7**. With a database it recalls prior episodes, standing directives and the bot's active learned behaviors (named in the system channel's self block, decision D154) before the model call and writes nothing (reflection stores the turn's episode, decision D97), and the turn is handled as without one; a memory that fails fails the turn with reason `Reasoning memory recall failed: ...` without a model call.
- <a id="reasoning-8"></a>**reasoning-8**. Its model call streams its reply when the run's `FLOPPY_STREAM` is `true`, and each `generate.progress` the generate service returns while the reply arrives is sent on to the parent unchanged, so the cognition can bound the stage by inactivity (decision D101).
- <a id="reasoning-9"></a>**reasoning-9**. As the intuition tier (the `<param>` tier `intuition`, decision D144) it may select the deliberative handoff, `bot.ability.reasoning.input`, among its choices: the handoff is never sent, the other selections are sent in order, and it ends `{turn, handled: false, reason: "handoff", acted}`, acted what was sent; a selection without the handoff is handled as reasoning's is, and its failures name the tier (`the intuition failed (<code>)`).
