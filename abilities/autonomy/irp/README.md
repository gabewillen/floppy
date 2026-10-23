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

- <a id="autonomy-1"></a>**autonomy-1**. Without memory (`database` `null`), a stimulus no seed triggers runs nothing: it ends with donedata `{turn, handled: false, candidates: [], reason: "no-behaviors"}`. Its donedata is its only answer: it sends its parent no event.
- <a id="autonomy-2"></a>**autonomy-2**. A `bot.ability.listening.speech.output` stimulus runs the SpeechHeard seed first, with the <param>s turn, input and stimulus. Speech with a source is handled: it ends with `{turn, handled: true, selected, candidates: ["SpeechHeard"], reason: "behavior"}`, `selected` the seed's one `bot.ability.communication.input` selection (the speech's `source_ids`, empty `target_ids`, `content`, `content_type`, `sample_rate_hz`, `channels`), trusted and naming no target (the body sends it to itself).
- <a id="autonomy-3"></a>**autonomy-3**. A seed that does not handle the turn passes it to the learned inventory; without memory the turn is unhandled: `{turn, handled: false, candidates: ["SpeechHeard"], reason: "unhandled"}`.
- <a id="autonomy-4"></a>**autonomy-4**. With memory, one transaction of the Memory ability on the `database` <param> loads the ACTIVE `turn` behaviors whose trigger is the stimulus's event; when none matches, the turn is `no-behaviors`, or `unhandled` with the seed in `candidates` when a seed ran.
- <a id="autonomy-5"></a>**autonomy-5**. A load that fails or outlasts `memory_timeout_ms` matches no behaviors and the turn goes on, as a load with no rows does.
- <a id="autonomy-6"></a>**autonomy-6**. A `bot.perceived` stimulus (the body's percepts taken together) is matched percept by percept, in arrival order, each percept's stimulus as the stimulus: the turn is handled only when every percept was, `selected` the handling behaviors' selections in arrival order; otherwise it is `unhandled` (or `no-behaviors` when nothing was tried), and `acted` `{percepts, acts}` (the reference's ActedData) names the handled percepts' positions and their selections when any was.
