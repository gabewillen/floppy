# `listening` implementation report plan

The implementation report plan of [`listening.scx`](../listening.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="listening-1"></a>**listening-1**. It waits for both children, the sensitivity and the interpretation (each ready with its first `bot.ability.offers`), then sends `bot.ability.offers` `{ability: "listening", choices: []}` to its parent: it offers nothing.
- <a id="listening-2"></a>**listening-2**. Each `environment.sound` is scored by the sensitivity (`bot.ability.listening.sensitivity.score` `{stimulus: {event: "environment.sound", data: <the sound>}}`) and the score goes on to the interpretation; the interpretation's speech product is handed to the parent as `bot.ability.cognition.input` `{stimulus: {event: "bot.ability.listening.speech.output", data: <the product>, source: "listening"}}`, the product's parent the sound's envelope.
- <a id="listening-3"></a>**listening-3**. A sound is scored at its `received_level_db`: a product the interpretation finds below its threshold is dropped, a louder one handed on.
- <a id="listening-4"></a>**listening-4**. The interpretation's own `bot.ability.cognition.input` (labelled non-speech) and its `bot.ability.listening.failed` `{stage, message}` go on to the parent unchanged.
- <a id="listening-5"></a>**listening-5**. `bot.ability.speaking.efference` goes to the sensitivity from wherever perception is, without interrupting it; the score request's stimulus carries the sound's `source` (none when the sound names none), so while the copy's window is open a sound off the copy's `mouth` (speaking stamps its own sound `source: "speaking"`) is scored against the prediction, fully predicted at 0 dB, and its product is withheld, while a sound from any other source is scored at its received level; a silent copy (`duration` 0) closes the window, so the mouth's sound after it is scored at its received level again.
- <a id="listening-6"></a>**listening-6**. Sounds that arrive before the children are ready, or while one is being scored, are held (`<event:defer/>`) and taken in order.
- <a id="listening-7"></a>**listening-7**. Scoring is bounded by `sensitivity_timeout_ms` (the original's 30 s): its timer answers `bot.ability.listening.failed` `{stage: "sensitivity", message: "Listening sensitivity timed out."}` and returns to `Listening`.
- <a id="listening-8"></a>**listening-8**. A sound its schema refuses (no audio reference) is never routed here: the send raises `error.execution` in the sender.
