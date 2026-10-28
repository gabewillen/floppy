# `transcription` implementation report plan

The implementation report plan of [`transcription.scx`](../transcription.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="transcription-1"></a>**transcription-1**. It opens a streaming hearing session (the speech service's operation `stream`) and, on the session's first `bot.ability.hearing.speech.stream.ready`, sends `bot.ability.ready` to its parent (decision D242).
- <a id="transcription-2"></a>**transcription-2**. Each `bot.ability.listening.sensitivity.output` is streamed; the session's `turn_ended` ends the turn, and a turn with words goes to the parent as `bot.ability.listening.transcription.turn` `{text, confidence, frames, sensed, voice_detection, participants}`: its words, the lowest transcript confidence, its sounds, its loudest scored sound, its voice spans in seconds from the turn's start, and its participants. Scored sounds that arrive before the session is ready are held (`<event:defer/>`) and streamed once it is.
- <a id="transcription-3"></a>**transcription-3**. The bot's own voice arriving as predicted (`producing`, with a `perceived_level_db` below `product_threshold_db`) is not streamed and does not join the turn.
- <a id="transcription-4"></a>**transcription-4**. A turn without words is let go; what is heard from then on is the next turn's.
- <a id="transcription-5"></a>**transcription-5**. A session lost before its first ready (here, a backend that does not stream) ends the transcription in its final state `unavailable`, logged (`transcription`), so the parent hears through the interpretation; so does `enabled` false, at once and unlogged, and a session that does not open within `open_timeout_ms`.
- <a id="transcription-6"></a>**transcription-6**. A session lost after it was ready is logged, and after the backoff (`first_backoff_ms`, doubling up to `last_backoff_ms`) a new one is opened, with no second `bot.ability.ready`; the turn it was hearing is lost.
- <a id="transcription-7"></a>**transcription-7**. Until the turn's voice starts (a `voice_started`) only its last `lead_frames` sounds are kept, and its voice spans are measured from its first kept sound, each sound taken as an equal share of the turn's time; past `max_turn_frames` its oldest sounds are let go.
