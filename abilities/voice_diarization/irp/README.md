# `voice_diarization` implementation report plan

The implementation report plan of [`voice_diarization.scx`](../voice_diarization.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="voice_diarization-1"></a>**voice_diarization-1**. It diarizes the utterance through the speech service's `diarize` (`POST {base}/audio/diarization`, model `pyannote-diarization` by default); its `bot.ability.hearing.voice.diarization.output` ends it in `applied` with the done data (`done.invoke.voice_diarization`) exactly `{segments}`: the original's OutputData, one clipped PCM segment per answered span, ascending by start, each `{audio: "<output>-<index>", media_type: "audio/pcm", sample_rate_hz, channels, start_seconds, end_seconds, confidence}` (`confidence` `null` when the answer has none); raw speaker labels never enter.
- <a id="voice_diarization-2"></a>**voice_diarization-2**. The utterance is a WAV container (its own format) or raw 16-bit PCM in the `sample_rate_hz` and `channels` `<param>`s; raw PCM with no format is a failure.
- <a id="voice_diarization-3"></a>**voice_diarization-3**. When the service answers `bot.ability.failed` `{message}` (an error status, an answer with no spans, a span outside the utterance, a format it cannot read), the done data is exactly the original's FailureData `{message}`.
- <a id="voice_diarization-4"></a>**voice_diarization-4**. When the diarizer cannot run (a missing `output`, or a malformed `model`, rejects its start with `error.execution`), the done data is `{message: "VoiceDiarization could not run (<error event>)"}`.
