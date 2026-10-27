# `voice_detection` implementation report plan

The implementation report plan of [`voice_detection.scx`](../voice_detection.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="voice_detection-1"></a>**voice_detection-1**. It serves each `bot.ability.hearing.voice.detection.input` `{id, audio, media_type?, sample_rate_hz?, channels?, output?}` by normalizing the sound, then detecting its voice: a WAV container's signed 16-bit PCM is written to `output` and the sound becomes `{audio: output, media_type: "audio/pcm", sample_rate_hz, channels}` (the container's own format); the speech service's `detect` (`POST {base}/vad`, model `silero-vad` by default) answers `bot.ability.hearing.voice.detection.output` `{segments}`, and the detection answers its parent with `bot.ability.hearing.voice.detection.output` exactly `{id, sound, segments}`, each segment `{start_seconds, end_seconds, confidence}` (the answer's `confidence`, `probability` or `score`, else `null`) in ascending start order, clamped to the clip; the clip is sent between half a second of silence (a cold VAD misses voice that starts at the first sample), and the answered spans are shifted back onto it.
- <a id="voice_detection-2"></a>**voice_detection-2**. Raw, `audio/l16` or unnamed audio with a rate and channels normalizes to `audio/pcm` in place (the same `audio`); any other media type, or a WAV container that cannot be read, leaves the sound unchanged, and the detection still runs on it.
- <a id="voice_detection-3"></a>**voice_detection-3**. A clip with no voice (no spans answered, or only spans outside the clip) is not a failure: its `segments` are `[]`.
- <a id="voice_detection-4"></a>**voice_detection-4**. When the detection answers `bot.ability.failed` `{message}` (an error status, a malformed answer, audio that cannot be read), the detection answers `bot.ability.failed` exactly `{id, message}`, with no sound.
- <a id="voice_detection-5"></a>**voice_detection-5**. When the normalization fails (`bot.ability.failed`, such as an `output` that cannot be written) it answers that `{id, message}` and nothing is detected; when a step cannot run (a missing `output`, or a malformed `model`, rejects its start with `error.execution`), it answers `{id, message: "VoiceDetection could not run (<error event>)"}`.
- <a id="voice_detection-6"></a>**voice_detection-6**. It is long-lived (mosfet.py's Interpretation holds its VoiceDetection): it answers request after request, one at a time, and a request that arrives while one is in flight replaces it, so only the newer one is answered.
