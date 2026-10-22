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
- <a id="speaking-2"></a>**speaking-2**. `bot.ability.speaking.input` `{parts}` (the original's input: 1 to 64 parts `{expression?, words?}`) is encoded by the vocal child, which gets the parts, with the speaking's `provider`, `model`, `voice` and `media_type` (by default the original's gemini encoder, `audio/pcm`), written to `output_prefix` + the utterance's number + `.` + the media subtype (`media/speaking-<n>.pcm`); the utterance's text of record is the parts' words joined exactly as written (the original's joined words, nothing stripped).
- <a id="speaking-3"></a>**speaking-3**. `playing` sends the efference copy `bot.ability.speaking.efference` `{mouth: "speaking", duration, media_type, sample_rate_hz, channels}` before the sound, `environment.sound` `{audio, media_type, sample_rate_hz, channels, source: "speaking"}` (its source the copy's mouth, so the listening can tell its own voice apart), to its parent; with no measured duration no copy is sent, and a rate or channel count the encoder leaves null is the speaking's own (24000 Hz mono).
- <a id="speaking-4"></a>**speaking-4**. `recording` sends `bot.ability.conversation.append` `{message}` to its parent, the outbound message `<conversation:message_for_turn>` builds (content the text, content_type `text/plain`, participants `[]` until the speaking has a voice of its own, provenance id `speaking-<n>` and event `bot.ability.conversation.append`), then answers `bot.ability.speaking.output` `{text, media_type, sample_rate_hz, channels}` and returns to `idle`.
- <a id="speaking-5"></a>**speaking-5**. An utterance that says nothing (its words, joined, blank; a part whose words are blank; a part with neither words, a pause nor a sound; an expression that directs no cue) is refused before any encoding: `bot.ability.failed` `{message: "A reply must say something: every part needs words, a sound or a pause, and the words must not be blank."}`, and back to `idle`; one its schema refuses (no parts, empty words) never arrives: the send raises `error.execution` in the sender.
- <a id="speaking-6"></a>**speaking-6**. A vocal failure (its `bot.ability.failed`) answers the parent with `bot.ability.failed` `{message}` carrying the vocal's message, and back to `idle`, where the next input is spoken.
- <a id="speaking-7"></a>**speaking-7**. Encoding is bounded by `encoding_timeout_ms` (30 s by default, decision D98; the original's 5 s is shorter than Gemini's unstreamed synthesis of a sentence): its timer answers `bot.ability.failed` `{message: "Speech encoding timed out."}` and returns to `idle`.
- <a id="speaking-8"></a>**speaking-8**. Input for another utterance that arrives outside `idle` is held (`<event:defer/>`), up to `held_utterances_bound` (16, a `<param>`), and taken, in order, on the return to `idle`; past that bound it is refused with `bot.ability.failed` `{message: "Speaking is holding too many utterances; this one is refused."}` (as the original holds them).
- <a id="speaking-9"></a>**speaking-9**. An utterance's id is its input's sendid (none when the sender names none). A finished utterance's id is kept among the last `finished_utterances_bound` (16) finished ids, and an input with a finished id, or another copy of the utterance in hand or of a held one, is dropped (logged `speaking.late`): nothing is said or answered for it (the original's new-input check), and a re-sent held id takes no second slot toward `held_utterances_bound` (the first copy is kept, where the original keeps the newer).
