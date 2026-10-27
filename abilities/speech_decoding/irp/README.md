# `speech_decoding` implementation report plan

The implementation report plan of [`speech_decoding.scx`](../speech_decoding.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="speech_decoding-1"></a>**speech_decoding-1**. By default it decodes through the OpenAI-compatible API: the speech service's `decode` with provider `null` and model `nemotron-speech-streaming-en-0.6b` (the lab's Nemotron), `POST {base}/audio/transcriptions`. The service's `bot.ability.hearing.speech.decoding.output` `{text}` is answered to its parent as `bot.ability.hearing.speech.decoding.output` exactly `{id, text}` (the request's `id`, the answer's `text`, trimmed). It is long-lived: each utterance is a request `bot.ability.hearing.speech.decoding.input` `{id, audio, media_type?}`.
- <a id="speech_decoding-2"></a>**speech_decoding-2**. Speech the backend heard none of decodes to empty words: `{id, text: ""}`, not a failure.
- <a id="speech_decoding-3"></a>**speech_decoding-3**. The `provider` and `model` `<param>`s choose the backend: provider `gemini` is Gemini's `POST {base}/interactions`, whose answer's `output_text`, or its `model_output` steps' text, trimmed, becomes the answer's `text`; provider `null` is the OpenAI-compatible `POST {base}/audio/transcriptions` and never calls Gemini's.
- <a id="speech_decoding-4"></a>**speech_decoding-4**. When the service answers `bot.ability.failed` `{message}` (an unreadable or empty audio file, an error status, a malformed answer), it answers `bot.ability.failed` exactly `{id, message}` with the service's message.
- <a id="speech_decoding-5"></a>**speech_decoding-5**. When the decoder cannot run (a malformed `media_type` rejects its start with `error.execution`), it answers `bot.ability.failed` `{id, message: "SpeechDecoding could not run (<error event>)"}`.
