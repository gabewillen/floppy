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

- <a id="intuition-1"></a>**intuition-1**. It asks the decision service (kind `select`) over the `choices` <param> less `bot.ability.reasoning.input`, with the stimulus's object data as evidence: each event's required data is filled from it. A confident product is handled: it ends with donedata `{turn, handled: true, selected: [{event, data}], reason: "none", tuner}`, every selection the service returned; its donedata is its only answer.
- <a id="intuition-2"></a>**intuition-2**. The turn's confidence is the minimum of its selections' probabilities as whole numbers on the 0-100 scale. The `tuner` <param> `{mean, var, n}` observes it and comes back updated in every donedata; the turn is low-confidence when the confidence is below the updated threshold (the floor 35 during the warmup of 8 observations, then `max(17.5, mean - std)`).
- <a id="intuition-3"></a>**intuition-3**. A low-confidence product is unhandled and escalates: `{turn, handled: false, selected, reason: "low_confidence", tuner}`, `selected` its environment actions (its selections less `bot.ability.reasoning.input` and `bot.ability.cognition.ignore`).
- <a id="intuition-4"></a>**intuition-4**. When the service abstains (the pass criterion wins, or no event could be offered) the product is empty: `{turn, handled: false, reason: "empty_product", tuner}` with no `selected`, and the tuner observes nothing.
- <a id="intuition-5"></a>**intuition-5**. The selections handed out are checked before it answers: `<bot:verdict>` drops `bot.ability.cognition.ignore` and applies the attention policy with the `focus` and `focus_candidates` <param>s, then `<decision:route>` places each on the target offering it. A rejection fails the stage: `{turn, handled: false, failed: true, reason: "Selection rejected: <message>", tuner}`.
- <a id="intuition-6"></a>**intuition-6**. A failed decision (`decision.failed {code}`) fails the stage with reason `the decision failed (<code>)`; one that cannot start (`error.*`) with `the decision could not run (<event>)`; and no answer within `decision_timeout_ms` with `the decision timed out after <n> ms`; each `{turn, handled: false, failed: true, reason, tuner}`.
