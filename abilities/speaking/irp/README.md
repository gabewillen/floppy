# `speaking` implementation report plan

The implementation report plan of [`speaking.scx`](../speaking.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="speaking-1"></a>**speaking-1**. Every entry into `idle` (its start, and the end of each utterance, spoken or failed) sends `bot.ability.offers` `{ability: "speaking", choices: []}` to its parent: it offers nothing.
- <a id="speaking-2"></a>**speaking-2**. `bot.ability.speaking.input` `{text}` is encoded by the vocal child with the speaking's `provider`, `model`, `voice` and `media_type` (by default the original's gemini encoder, `audio/pcm`), written to `output_prefix` + the utterance's number + `.` + the media subtype (`media/speaking-<n>.pcm`), the text stripped first.
- <a id="speaking-3"></a>**speaking-3**. `playing` sends the efference copy `bot.ability.speaking.efference` `{mouth: "speaking", duration, media_type, sample_rate_hz, channels}` before the sound, `environment.sound` `{audio, media_type, sample_rate_hz, channels, source: "speaking"}` (its source the copy's mouth, so the listening can tell its own voice apart), to its parent; with no measured duration no copy is sent, and a rate or channel count the encoder leaves null is the speaking's own (24000 Hz mono).
- <a id="speaking-4"></a>**speaking-4**. `recording` sends `bot.ability.conversation.append` `{message}` to its parent, the outbound message `<conversation:message_for_turn>` builds (content the text, content_type `text/plain`, sequence 0, provenance id `speaking-<n>` and event `bot.ability.conversation.append`), then answers `bot.ability.speaking.output` `{text, media_type, sample_rate_hz, channels}` and returns to `idle`.
- <a id="speaking-5"></a>**speaking-5**. A blank text (after stripping) is refused before any encoding: `bot.ability.failed` `{message: "Speaking requires non-blank text."}`, and back to `idle`; an empty text never arrives: the input's schema refuses it at the send, raising `error.execution` in the sender.
- <a id="speaking-6"></a>**speaking-6**. A vocal failure (its `bot.ability.failed`) answers the parent with `bot.ability.failed` `{message}` carrying the vocal's message, and back to `idle`, where the next input is spoken.
- <a id="speaking-7"></a>**speaking-7**. Encoding is bounded by `encoding_timeout_ms` (30 s by default, decision D98; the original's 5 s is shorter than Gemini's unstreamed synthesis of a sentence): its timer answers `bot.ability.failed` `{message: "Speech encoding timed out."}` and returns to `idle`.
- <a id="speaking-8"></a>**speaking-8**. Input that arrives outside `idle` is held (`<event:defer/>`) and taken, in order, on the return to `idle`.
