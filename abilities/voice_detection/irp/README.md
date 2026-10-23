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

- <a id="voice_detection-1"></a>**voice_detection-1**. It normalizes the sound, then detects its voice: a WAV container's signed 16-bit PCM is written to `output` and the sound becomes `{audio: output, media_type: "audio/pcm", sample_rate_hz, channels}` (the container's own format); the speech service's `detect` (`POST {base}/vad`, model `silero-vad` by default) answers `bot.ability.hearing.voice.detection.output` `{segments}`, and the done data (`done.invoke.voice_detection`) is exactly `{sound, voice_detection: {segments}}`, each segment `{start_seconds, end_seconds, confidence}` (the answer's `confidence`, `probability` or `score`, else `null`) in ascending start order, clamped to the clip.
- <a id="voice_detection-2"></a>**voice_detection-2**. Raw, `audio/l16` or unnamed audio with a rate and channels normalizes to `audio/pcm` in place (the same `audio`); any other media type, or a WAV container that cannot be read, leaves the sound unchanged, and the detection still runs on it.
- <a id="voice_detection-3"></a>**voice_detection-3**. A clip with no voice (no spans answered, or only spans outside the clip) is not a failure: its `voice_detection` is `{segments: []}`.
- <a id="voice_detection-4"></a>**voice_detection-4**. When the detection answers `bot.ability.failed` `{message}` (an error status, a malformed answer, audio that cannot be read), the done data is exactly `{message}`, with no sound.
- <a id="voice_detection-5"></a>**voice_detection-5**. When the normalization fails (`bot.ability.failed`, such as an `output` that cannot be written) it ends with that `{message}` and nothing is detected; when a step cannot run (a missing `output`, or a malformed `model`, rejects its start with `error.execution`), the done data is `{message: "VoiceDetection could not run (<error event>)"}`.
