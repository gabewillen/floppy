# `communication` implementation report plan

The implementation report plan of [`communication.scx`](../communication.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="communication-1"></a>**communication-1**. It is ready once both its children, the conversation and speaking, are ready (their first `bot.ability.offers`): on every entry into active it sends `bot.ability.offers` `{ability: "communication", choices}`, choices the one model-callable `bot.ability.communication.respond` with its schema compacted (no `$schema`, title or examples), no target.
- <a id="communication-2"></a>**communication-2**. `bot.ability.communication.input` in active goes to the conversation; a text product comes back to the body as `bot.ability.cognition.input` `{stimulus}` unchanged, its stimulus carrying the conversation's history with the input as its latest, inbound, message.
- <a id="communication-3"></a>**communication-3**. A product that is not text reaches the body as the conversation's `bot.ability.conversation.output` itself (its latest message inbound); history whose latest message is outbound is not forwarded.
- <a id="communication-4"></a>**communication-4**. A conversation that cannot take an input fails it: its `bot.ability.conversation.failed` `{stage, message}` goes on to the body unchanged.
- <a id="communication-5"></a>**communication-5**. `bot.ability.communication.respond` `{text}` (non-blank) in active is said through speaking: in order, the body gets speaking's efference copy `bot.ability.speaking.efference` `{mouth: "speaking", duration, media_type, sample_rate_hz, channels}`, the communication puts speaking's `environment.sound` `{audio, media_type, sample_rate_hz, channels, source: "speaking"}` into the environment whose id its `environment` names (`#_scxml_` + that id), and the body gets `bot.ability.speaking.output` `{text, media_type, sample_rate_hz, channels}`; the communication is active again and offers again, and the outbound record goes to the conversation as history only.
- <a id="communication-6"></a>**communication-6**. A blank response answers `bot.ability.communication.failed` `{message: "Communication response text must be non-blank."}` without speaking; the communication stays active and says the next response.
- <a id="communication-7"></a>**communication-7**. A speaking failure answers `bot.ability.communication.failed` `{message}` (speaking's failure message) and the communication is active again.
- <a id="communication-8"></a>**communication-8**. A response is bounded by `response_timeout_ms` (36000 by default, a `<param>`: speaking's 30 s encoding bound plus the original's 6 s): when it passes before speaking answers, the body gets `bot.ability.communication.failed` `{message: "Speaking response timed out."}` and the communication is active again; speaking's late answer is not forwarded. `encoding_timeout_ms` (30000 by default, a `<param>`; decision D98) is passed to speaking as its encoding bound.
- <a id="communication-9"></a>**communication-9**. Responses and input that arrive while a response is in hand, and input that arrives before it is active, are deferred and taken, in order, once it is active.
- <a id="communication-10"></a>**communication-10**. `bot.ability.communication.activate` `{}` is taken only in inactive: in active it does nothing.
