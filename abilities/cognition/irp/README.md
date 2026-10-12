# `cognition` implementation report plan

The implementation report plan of [`cognition.scx`](../cognition.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="cognition-1"></a>**cognition-1**. On start, and on every return to `idle`, it sends `bot.ability.offers` `{ability: "cognition", choices}` to its parent: its one choice, `bot.ability.cognition.ignore`, its schema compacted (`$schema`, `title` and `examples` dropped).
- <a id="cognition-2"></a>**cognition-2**. A `bot.ability.cognition.input` `{turn, stimulus, focus?, focus_candidates?, choices?}` runs one turn of the cascade: autonomy (without a database it leaves the turn unhandled), then intuition, whose confident decision handles the turn: `bot.ability.cognition.output` `{turn, output}`, the output intuition's selections.
- <a id="cognition-3"></a>**cognition-3**. When intuition leaves the turn unhandled (its decision abstains), reasoning (the generate service, schema `dispatch`) handles it, and the output is reasoning's selections; with a database (the `<param>` `database`) autonomy and reasoning use the memory and the turn is answered the same way.
- <a id="cognition-4"></a>**cognition-4**. An intuition that escalates for low confidence hands out its environment actions (its selections less ignore), which the output carries first, before reasoning's selections.
- <a id="cognition-5"></a>**cognition-5**. A stage that fails answers the turn with `bot.ability.failed` `{message, turn, input: {focus, focus_candidates}}`, the message the stage's reason (`the decision failed (status)`, `the reasoning failed (status)`, `the reasoning failed (schema-mismatch)`).
- <a id="cognition-6"></a>**cognition-6**. A stage that does not answer within `child_operation_timeout_ms` ends the turn without an answer: the cognition sends `bot.reboot` `{reason: "cognition_child_teardown_failed"}` to its parent and waits in `rebooting`, where it acknowledges any `bot.ability.cognition.cancel` `{turn}` at once with `bot.ability.cognition.cancelled` `{turn}`.
- <a id="cognition-7"></a>**cognition-7**. `bot.ability.cognition.cancel` `{turn}` for the turn in hand cancels the stage and is acknowledged with `bot.ability.cognition.cancelled` `{turn}`, and the cognition is idle again (it offers); in `idle` a cancel is acknowledged at once; a cancel for another turn is not taken and the turn goes on.
- <a id="cognition-8"></a>**cognition-8**. `bot.ability.cognition.ignore` (its offer) is taken in `idle` and while processing, and only logged: it sends nothing and does not end a turn.
- <a id="cognition-9"></a>**cognition-9**. The events it takes are checked against `abilities/schemas/` where they are created: a `bot.ability.cognition.input` without a stimulus is refused at its sender (`error.execution` there) and starts no turn.
