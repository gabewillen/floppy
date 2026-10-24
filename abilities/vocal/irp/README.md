# `vocal` implementation report plan

The implementation report plan of [`vocal.scx`](../vocal.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="vocal-1"></a>**vocal-1**. On start, and on every later entry into `idle`, it offers nothing: it sends `bot.ability.offers` `{ability: "vocal", choices: []}` to its parent.
- <a id="vocal-2"></a>**vocal-2**. `bot.ability.vocal.speech.encoding.input` `{parts}` (1 to 64 parts `{expression?, words?}`) in `idle` is encoded by the speech service's `encode` with the `<param>`s `parts`, `model`, `voice`, `output` and `media_type` (`audio/pcm` by default); with provider `null` (the default) that is the OpenAI-compatible `POST {base}/audio/speech`. The service's answer is sent to the parent as `bot.ability.vocal.speech.encoding.output` `{audio, media_type, sample_rate_hz, channels, duration}` (the audio as the `output` reference; raw PCM is 24 kHz mono, its duration measured from its bytes; a format the service cannot measure has `null`s), and it returns to `idle`.
- <a id="vocal-3"></a>**vocal-3**. The `provider` `<param>` names another provider instead: `elevenlabs` is `POST {base}/text-to-speech/{voice}/stream?output_format=…`, `gemini` is `POST {base}/interactions` (its answer's base64 audio part), each answered as above.
- <a id="vocal-4"></a>**vocal-4**. When the service answers `bot.ability.failed` `{message}` (an error status, an empty answer, an `output` that cannot be written), it sends the parent `bot.ability.failed` with the service's message and returns to `idle`.
- <a id="vocal-5"></a>**vocal-5**. When the service cannot run (a missing `voice`, rejecting its start with `error.execution`), it sends the parent `bot.ability.failed` `{message: "The speech service could not run (<error event>)."}` and returns to `idle`.
- <a id="vocal-6"></a>**vocal-6**. An input that arrives while `applying` is dropped: only the input in hand is encoded and answered.
- <a id="vocal-7"></a>**vocal-7**. Each input is a new encoding: after an answer or a failure, the next input in `idle` is encoded again.
- <a id="vocal-8"></a>**vocal-8**. The service says the parts (decision D178): walking them in order, a part with a pause opens a new stretch, each stretch whose words are not blank is one backend request saying its words joined as written, one at a time in order, and no other cue (a sound, an emotion) is applied. With a pause the answers are joined as 16-bit PCM with each pause's silence at its place (0.4 s short, 1 s long), written raw for `audio/pcm` or in a WAV container for `audio/wav`; a pause with another `media_type` rejects the service's start, answered as in statement 5.
- <a id="vocal-9"></a>**vocal-9**. The service streams its encoding (decision D203): each `bot.ability.vocal.speech.encoding.chunk` `{audio, sequence, media_type, sample_rate_hz, channels, duration}` it returns while `applying` is sent to the parent at once, as it arrives, before the output: its audio is chunk `sequence`'s own file (the `output` reference with `.<sequence>` before its extension), a streamed answer is cut every 24000 bytes (half a second of 24 kHz mono PCM), Gemini's answer is one chunk per stretch, and a pause is a chunk of its silence.
