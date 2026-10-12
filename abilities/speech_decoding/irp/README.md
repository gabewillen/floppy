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

- <a id="speech_decoding-1"></a>**speech_decoding-1**. By default it decodes through the original's gemini SpeechDecoder: the speech service's `decode` with provider `gemini` and model `gemini-3.5-flash`, `POST {base}/interactions`. The service's `bot.ability.hearing.speech.decoding.output` `{text}` ends it in `applied`, its done data (`done.invoke.speech_decoding`) exactly `{text}`: the answer's `output_text`, or its `model_output` steps' text, trimmed.
- <a id="speech_decoding-2"></a>**speech_decoding-2**. Speech the backend heard none of decodes to empty words: done data `{text: ""}`, not a failure.
- <a id="speech_decoding-3"></a>**speech_decoding-3**. The `provider` and `model` `<param>`s choose the backend: provider `null` is the OpenAI-compatible `POST {base}/audio/transcriptions`, whose answer's `text` becomes the done data's `text`.
- <a id="speech_decoding-4"></a>**speech_decoding-4**. When the service answers `bot.ability.failed` `{message}` (an unreadable or empty audio file, an error status, a malformed answer), the done data is the original's FailureData, exactly `{message}` with the service's message.
- <a id="speech_decoding-5"></a>**speech_decoding-5**. When the decoder cannot run (a missing `audio`, or a malformed `media_type`, rejects its start with `error.execution`), the done data is `{message: "SpeechDecoding could not run (<error event>)"}`.
