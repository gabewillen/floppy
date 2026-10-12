# `interpretation` implementation report plan

The implementation report plan of [`interpretation.scx`](../interpretation.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="interpretation-1"></a>**interpretation-1**. On start it sends `bot.ability.offers` `{ability: "interpretation", choices: []}` to its parent: it offers nothing.
- <a id="interpretation-2"></a>**interpretation-2**. Voice identification (`voice_classifier`) with speech decoding (`speech_decoder`) is refused, as the original's constructor refuses it: the interpretation ends in its final state `degraded` without offering.
- <a id="interpretation-3"></a>**interpretation-3**. With the default configuration (voice detection, sound classification, speech decoding), a voiced scored sound opens the open-speech window, each further voiced sound joins it, and the first sound without voice closes it; the utterance is joined into a WAV file, decoded, and answered as `bot.ability.listening.speech.output` `{content: <words>, content_type: "text/plain", voice_detection, sample_rate_hz, channels, media_type: "audio/pcm", source_ids: [], parent}`, its parent the loudest frame's (the first with a level when none is louder).
- <a id="interpretation-4"></a>**interpretation-4**. A sound without voice is classified: a labelled one (a non-blank kind) is handed to the parent as `bot.ability.cognition.input` `{stimulus}`, the stimulus `{event: "environment.sound", data: <the sound as voice detection normalized it, its PCM written beside it as floppy-sound-<n>.pcm>, source: "listening"}`; an unlabelled one is dropped while speech decoding is configured.
- <a id="interpretation-5"></a>**interpretation-5**. A product is handed off only when audible: its `perceived_level_db` null or at least `product_threshold_db` (3 dB); a quieter one is dropped.
- <a id="interpretation-6"></a>**interpretation-6**. With neither speech decoding nor sound classification, a sound without voice is a silence observation: a speech product of the sound's PCM (`content` its floppy-sound-<n>.pcm, `content_type` `audio/pcm`, no voice spans); a PCM sound with no sample rate is handed off as the `environment.sound` stimulus instead. While the window is open and neither decoding nor diarization owns the product, each frame (the closing one too) is streamed as such a speech product with its voice spans.
- <a id="interpretation-7"></a>**interpretation-7**. With diarization the closed utterance is diarized: one speech product per clipped segment (`content` floppy-diarized-<n>-<index>, its span, `confidence`, a `voice_detection` of its own length, `voice_embedding` null and no source ids), or, with voice identification too, each identified by its embedding (`source_ids` `[embedding]`, `voice_embedding` the embedding); with speech decoding too, the utterance is decoded instead.
- <a id="interpretation-8"></a>**interpretation-8**. Voice identification without diarization identifies the opening voice span directly: its one embedding is the window's source identity, carried by every streamed frame; a sound that is not PCM with a sample rate cannot be identified and fails the `voice_identification` stage.
- <a id="interpretation-9"></a>**interpretation-9**. A stage failure (its child's failure, an error, or its timer) answers `bot.ability.listening.failed` `{stage, message}` and returns to `Idle`, where the next sound is interpreted; decoded speech of a sound with no sample rate fails the `speech_decoding` stage.
- <a id="interpretation-10"></a>**interpretation-10**. Scored sounds that arrive while one is being interpreted are held (`<event:defer/>`) and taken in order.
- <a id="interpretation-11"></a>**interpretation-11**. Each stage is bounded by `stage_timeout_ms` (the original's 30 s): its timer fails it with `{stage, message: "Listening <stage> timed out."}`.
