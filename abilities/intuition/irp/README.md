# `intuition` implementation report plan

The implementation report plan of [`intuition.scx`](../intuition.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="intuition-1"></a>**intuition-1**. It asks the decision service (kind `select`) over the `choices` <param> less `bot.ability.reasoning.input`, with the stimulus's object data as evidence: each event's required data is filled from it. A confident product is handled: it sends every selection the service returned to its target and ends with donedata `{turn, handled: true, selected: [{event, data, target}], reason: "none", tuner}`.
- <a id="intuition-2"></a>**intuition-2**. The turn's confidence is the minimum of its selections' probabilities as whole numbers on the 0-100 scale. The `tuner` <param> `{mean, var, n}` observes it and comes back updated in every donedata; the turn is low-confidence when the confidence is below the updated threshold (the floor 35 during the warmup of 8 observations, then `max(17.5, mean - std)`).
- <a id="intuition-3"></a>**intuition-3**. A low-confidence product is unhandled and escalates: it sends its selections (the original's environment actions, dispatched before reasoning runs) and ends with `{turn, handled: false, reason: "low_confidence", tuner}`.
- <a id="intuition-4"></a>**intuition-4**. When the service abstains (the pass criterion wins, or no event could be offered) the product is empty: `{turn, handled: false, reason: "empty_product", tuner}` with no `selected`, and the tuner observes nothing.
- <a id="intuition-5"></a>**intuition-5**. Each selection is sent to its target, the one session the choices tag its event with (the decision service's single offerer, the original's `_single_enabler`): the ignore goes to the session that offers it, and an event several sessions offer names no target, so it reaches none.
- <a id="intuition-6"></a>**intuition-6**. A failed decision (`decision.failed {code}`) fails the stage with reason `the decision failed (<code>)`; one that cannot start (`error.*`) with `the decision could not run (<event>)`; and no answer within `decision_timeout_ms` with `the decision timed out after <n> ms`; each `{turn, handled: false, failed: true, reason, tuner}`.
