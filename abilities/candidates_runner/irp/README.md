# `candidates_runner` implementation report plan

The implementation report plan of [`candidates_runner.scx`](../candidates_runner.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="candidates_runner-1"></a>**candidates_runner-1**. It runs the first of its `candidates` <param> (`[{name, source}]`) as the child `candidate` from its source, with the <param>s `turn` and `input`. A candidate that answers by its donedata `{turn, handled: true, selected}` for this turn with at least one selection, all accepted, has its use persisted, and the runner ends with donedata `{tried: [name], handled: true, selected, rejection: null}`, `selected` the candidate's own selections; it sends its parent nothing else.
- <a id="candidates_runner-2"></a>**candidates_runner-2**. The handled candidate's selections are checked before its use is persisted: `<bot:verdict>` drops `bot.ability.cognition.ignore` selections and applies the attention policy with `input.focus` and `input.focus_candidates`, then `<decision:route>` places each remaining one on the target offering it in the `choices` <param>.
- <a id="candidates_runner-3"></a>**candidates_runner-3**. An answer that does not handle this turn (`handled` not `true`, no selections, or another turn) is not handled: with no candidates left, the runner ends with `{tried: [name], handled: false, selected: null, rejection: null}`.
- <a id="candidates_runner-4"></a>**candidates_runner-4**. When its candidate does not handle the turn and candidates are left, the runner invokes itself for the rest (with the same turn, input, choices, database and bounds) and ends with the rest's donedata, its own candidate first in `tried`.
- <a id="candidates_runner-5"></a>**candidates_runner-5**. A candidate that does not answer within `silence_ms` is passed over, and the rest run.
- <a id="candidates_runner-6"></a>**candidates_runner-6**. A candidate whose source cannot run has its failure persisted, and the rest run.
- <a id="candidates_runner-7"></a>**candidates_runner-7**. A handled candidate whose selection the verdict or the route rejects has its failure persisted, and the runner ends with `{tried: [name], handled: false, selected: null, rejection}`, the rejection's message; the rest do not run.
- <a id="candidates_runner-8"></a>**candidates_runner-8**. Each persistence is one transaction of the Memory ability on the `database` <param>; one that fails or outlasts `memory_timeout_ms` takes the same effect as one persisted.
