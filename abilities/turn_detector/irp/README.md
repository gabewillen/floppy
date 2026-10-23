# `turn_detector` implementation report plan

The implementation report plan of [`turn_detector.scx`](../turn_detector.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="turn_detector-1"></a>**turn_detector-1**. On start it sends `bot.ability.turn_detector.ready` `{participant_ref, conversation_ref}` to its parent: its `<param>`s `participant_ref` (default `"bot"`) and `conversation_ref` (default `"conversation"`). It is long-lived: it never reaches a final state.
- <a id="turn_detector-2"></a>**turn_detector-2**. In `idle`, `bot.ability.turn_detector.turn.start` whose `self_participant_ref` (default `"bot"`) is the detector's `participant_ref` opens the turn (`open`), taking its `turn_ref`, `conversation_ref` (default `"conversation"`) and `source_participant_ref` (default `"caller"`); a start for another participant opens nothing.
- <a id="turn_detector-3"></a>**turn_detector-3**. `bot.ability.turn_detector.turn.end` for the open turn adds its content and closes it: `normalizing` answers the parent with `bot.ability.turn_detector.turn.complete` `{conversation_ref, turn_ref, participant_ref, self_participant_ref, source_participant_ref, text, audio, content, content_type, sample_rate_hz, channels}`, and the detector is `idle` again. Text stimuli are stripped and kept when not empty, joined by spaces; a turn with text has content the text and content_type `text/plain`.
- <a id="turn_detector-4"></a>**turn_detector-4**. `bot.ability.turn_detector.turn.update` and a repeated `start` for the open turn add their content; `bot.ability.turn_detector.turn.pause` moves it to `paused`, from which update and start reopen it and end closes it. An update, pause or end whose `turn_ref`, `conversation_ref` or `source_participant_ref` is not the open turn's is ignored.
- <a id="turn_detector-5"></a>**turn_detector-5**. Audio stimuli are kept in order by reference: the turn's audio and content are the references, content_type `audio/pcm`, and its sample_rate_hz and channels the first chunk's that carries them. Content and event stimuli replace the modality-neutral content: a content stimulus's content and content_type, an event as `{event, payload}` (payload default `{}`) with `application/event`; with no text or audio the turn hands that content on. An image stimulus changes nothing (a completed turn carries no images).
- <a id="turn_detector-6"></a>**turn_detector-6**. A turn closed with neither text, audio nor modality-neutral content, or with both text and audio, answers `bot.ability.turn_detector.failed` `{stage: "turn", message, participant_ref, conversation_ref, turn_ref, source_participant_ref}` instead; the detector is `idle` again and takes the next turn.
- <a id="turn_detector-7"></a>**turn_detector-7**. `bot.ability.listening.speech.output` with voice spans opens a turn in `idle` (its turn_ref the next `turn-<n>`, the detector's conversation_ref, source `caller`) and, in `open` or `paused`, adds the observation's content as an audio chunk while its content_type is `audio/pcm` (decoded words carry no frames), with its sample_rate_hz and channels (default 1); without voice spans it opens nothing in `idle` and moves `open` to `paused`.
- <a id="turn_detector-8"></a>**turn_detector-8**. `open` and `paused` each wait `end_of_turn_silence_seconds` (at least 10 ms) from their entry, re-armed on every re-entry; when it passes, `closing` raises `bot.ability.turn_detector.turn.silence.expired` to itself and the turn is closed and answered as by an end.
- <a id="turn_detector-9"></a>**turn_detector-9**. A started turn's `conversation_ref` becomes the detector's own: a later turn opened by voice takes it.
- <a id="turn_detector-10"></a>**turn_detector-10**. A turn event whose data its schema rejects (e.g. a start with an empty text stimulus, an update with no content) is `error.execution` in its sender and never reaches the detector.
