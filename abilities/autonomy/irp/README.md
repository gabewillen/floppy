# `autonomy` implementation report plan

The implementation report plan of [`autonomy.scx`](../autonomy.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="autonomy-1"></a>**autonomy-1**. Without memory (`memory` `null`), a stimulus no seed triggers runs nothing: it answers `bot.ability.autonomy.output` `{turn, handled: false, candidates: [], reason: "no-behaviors"}`, once.
- <a id="autonomy-2"></a>**autonomy-2**. A `bot.ability.listening.speech.output` stimulus runs the SpeechHeard seed first, with the <param>s turn, input and stimulus. Speech with a source is handled: it ends with `{turn, handled: true, selected, candidates: ["SpeechHeard"], reason: "behavior"}`, `selected` the seed's one `bot.ability.communication.input` selection (the speech's `source_ids`, empty `target_ids`, `content`, `content_type`, `sample_rate_hz`, `channels`, and `parent`, the speech itself), trusted and naming no target (the body sends it to itself).
- <a id="autonomy-3"></a>**autonomy-3**. A seed that does not handle the turn passes it to the learned inventory; without memory the turn is unhandled: `{turn, handled: false, candidates: ["SpeechHeard"], reason: "unhandled"}`.
- <a id="autonomy-4"></a>**autonomy-4**. With memory, one transaction of the Memory ability at the `memory` <param> (decision D266) loads the ACTIVE `turn` behaviors whose trigger is the stimulus's event; when none matches, the turn is `no-behaviors`, or `unhandled` with the seed in `candidates` when a seed ran.
- <a id="autonomy-5"></a>**autonomy-5**. A load that fails or outlasts `memory_timeout_ms` matches no behaviors and the turn goes on, as a load with no rows does.
- <a id="autonomy-6"></a>**autonomy-6**. A `bot.perceived` stimulus (the body's percepts taken together) is matched percept by percept, in arrival order, each percept's stimulus as the stimulus: the turn is handled only when every percept was, `selected` the handling behaviors' selections in arrival order; otherwise it is `unhandled` (or `no-behaviors` when nothing was tried), and `acted` `{percepts, acts}` names the handled percepts' positions and their selections when any was.
- <a id="autonomy-7"></a>**autonomy-7**. It is long-lived (mosfet.py's Cognition holds its Autonomy, decision D272): it answers each `bot.ability.autonomy.input` in turn, and a `bot.ability.processing.cancel` ends the turn in flight unanswered, so the next input is the next turn.
- <a id="autonomy-8"></a>**autonomy-8**. Each matched candidate (`{name, source}`, `source` the path of its program, decision D121) runs in turn as the child `candidate` by `<invoke srcexpr>` on its source, with the <param>s `turn` and `input` (the percept's data with `focus` and `focus_candidates`). A candidate that answers by its donedata `{turn, handled: true, selected}` for this turn with at least one selection, all sent, has its use persisted (one transaction of the Memory ability at the `memory` <param>; one that fails or outlasts `memory_timeout_ms` takes the same effect) and handles the percept: `{turn, handled: true, selected, candidates, reason: "behavior"}`, `selected` the candidate's own selections.
- <a id="autonomy-9"></a>**autonomy-9**. The handled candidate's selections are sent, in order, each to the session its `target` names (`#_scxml_` + target, the session ids its turn's selections carry), before its use is persisted.
- <a id="autonomy-10"></a>**autonomy-10**. An answer that does not handle this turn (`handled` not `true`, no selections, or another turn) is not handled, and the next candidate runs; when none is left, the turn is `unhandled`, `candidates` naming each one tried, in order.
- <a id="autonomy-11"></a>**autonomy-11**. A candidate that does not answer within `silence_ms` is passed over, and the next runs.
- <a id="autonomy-12"></a>**autonomy-12**. A candidate whose source cannot run has its failure persisted, and the next runs.
- <a id="autonomy-13"></a>**autonomy-13**. A handled candidate a selection of which cannot be sent (a send that raises `error.*`) has its failure persisted, and autonomy fails the turn: `{turn, handled: false, failed: true, candidates, reason}`, reason `Autonomy behavior dispatch failed: <error> (selected <event> to <target>, ...)` (each selection, `with no target` for one without; decision D221); the rest do not run.
