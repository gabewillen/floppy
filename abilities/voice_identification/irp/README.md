# `voice_identification` implementation report plan

The implementation report plan of [`voice_identification.scx`](../voice_identification.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="voice_identification-1"></a>**voice_identification-1**. It is long-lived: each request is `bot.ability.hearing.voice.identification.input` `{id, segments}` or, for direct identification, `{id, audio, sample_rate_hz, channels, start_seconds, end_seconds, output}`. Given `segments`, it clips nothing: the speech service's `identify` (one `POST {base}/voice/embed` per segment) answers `bot.ability.hearing.voice.identification.output` `{embeddings}`, and it answers its parent with `bot.ability.hearing.voice.identification.output` exactly `{id, segments, embeddings}`: the segments as given and one embedding per segment, in segment order, each `{embedding, model, confidence: null}`.
- <a id="voice_identification-2"></a>**voice_identification-2**. The `model` `<param>` names the embedding model (`voice-detect-wespeaker-resnet34` by default), and each embedding reports it.
- <a id="voice_identification-3"></a>**voice_identification-3**. Without `segments` it identifies directly: `clipping` invokes the speech service's `clip` on `audio` (`sample_rate_hz`, `channels`) over `start_seconds`..`end_seconds` (the end clamped to the sound) into `output`, and the one clipped segment `{audio: output, media_type: "audio/pcm", sample_rate_hz, channels, start_seconds, end_seconds}` is what it identifies and returns as `segments`.
- <a id="voice_identification-4"></a>**voice_identification-4**. When the clip answers `bot.ability.failed` `{message}` (a span that holds no audio, a sound that cannot be read), it answers `bot.ability.failed` `{id, message: "Voice identification opening segment conversion failed: <message>"}` and nothing is identified.
- <a id="voice_identification-5"></a>**voice_identification-5**. When the identification answers `bot.ability.failed` `{message}` (an error status, a malformed embedding), it answers `bot.ability.failed` exactly `{id, message}`.
- <a id="voice_identification-6"></a>**voice_identification-6**. When a step cannot run (segments that are not ascending `audio/pcm` voice segments, or no segments and no sound to clip, reject its start with `error.execution`), it answers `bot.ability.failed` `{id, message: "VoiceIdentification could not run (<error event>)"}`.
