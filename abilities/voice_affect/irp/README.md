# `voice_affect` implementation report plan

The implementation report plan of [`voice_affect.scx`](../voice_affect.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="voice_affect-1"></a>**voice_affect-1**. It hears the tone of the longest of its `segments` (the first of equals; a segment with no span counts as long enough) through the speech service's `classify` (`POST {base}/emotion?sample_rate=R`); the service's `bot.ability.classifying.output` ends it in `applied` with the done data (`done.invoke.voice_affect`) exactly `{affect: {label, scores, valence, confidence}}` (mosfet.py's `voice.Affect`), and logs `listening.affect` `heard <label> <confidence>`.
- <a id="voice_affect-2"></a>**voice_affect-2**. A segment shorter than `min_segment_seconds` (0.3 s) is never sent; with no segment left, nothing is asked and the done data is `{affect: null}`.
- <a id="voice_affect-3"></a>**voice_affect-3**. A tone not heard (the service's `bot.ability.failed`, an answer that is not an affect, the service ending or unable to run, or no answer within `timeout_ms`, 3 s) ends it with `{affect: null}` and logs `listening.affect` `failed: <why>`: a tone is a perception, never a failure of the utterance.
