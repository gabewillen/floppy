# `conversation` implementation report plan

The implementation report plan of [`conversation.scx`](../conversation.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="conversation-1"></a>**conversation-1**. On start it sends `bot.ability.offers` `{ability: "conversation", choices: []}` to its parent: it offers nothing.
- <a id="conversation-2"></a>**conversation-2**. A direct `bot.ability.conversation.input` (its parent the envelope `{event: "bot.ability.conversation.input", data}`) or a `bot.ability.conversation.routed_input` `{parent}` (the communication's envelope, whose data is the input) is committed as an inbound message (sequence the history's length, provenance event the parent's event); an inbound commit of text hands the parent `bot.ability.cognition.input` `{stimulus}` whose stimulus is `{event: "bot.ability.conversation.output", data: {parent, messages}, source: "conversation"}`.
- <a id="conversation-3"></a>**conversation-3**. Each input is one turn per source identity (names sorted, then embeddings), each exchanged with its own turn detector; what is committed is the turns' product: their stripped texts joined by spaces, each distinct text once, as `text/plain`; with no text, null content for audio and the input's content otherwise. Its provenance's turn_ref is the first turn's, `input-<n>:turn:<track>`. A commit that is not text answers `bot.ability.conversation.output` `{parent, messages}` itself.
- <a id="conversation-4"></a>**conversation-4**. Each input finds its relationship: targetless input shares the first targetless (ambient) relationship; other input takes the relationship whose source and target groups match, in either direction; with no match it begins its own, whose session_ref is `relationship-` and its context reference `{"identity_groups":[...]}`. The committed provenance's session_ref is the relationship's.
- <a id="conversation-5"></a>**conversation-5**. `bot.ability.conversation.snapshot.request` `{request_ref}`, in any state, answers `bot.ability.conversation.snapshot.output` `{request_ref, detector_refs}`: `[session_ref, source]` of every participant (a source whose detector was ready), ordered by relationship and then identity.
- <a id="conversation-6"></a>**conversation-6**. `bot.ability.conversation.append` `{message}` with an outbound message is committed with the next sequence and answered `bot.ability.conversation.output` `{parent, messages}`; one whose provenance id (non-empty), source and target match a committed message's is not committed again but is answered all the same; an inbound one is not taken (nothing is committed or answered).
- <a id="conversation-7"></a>**conversation-7**. Input that arrives while one is in hand is held (`<event:defer/>`) and taken, in order, on the return to `waiting`.
- <a id="conversation-8"></a>**conversation-8**. A turn the detector cannot close (`bot.ability.turn_detector.failed`) or cannot take (an `error.*`: a start its schema refuses) answers `bot.ability.conversation.failed` `{stage: "turn_detector", message}` and commits nothing.
- <a id="conversation-9"></a>**conversation-9**. With a `database` param it recalls the relationship's memories before the turns and remembers the input after them through the memory ability, then commits as without one; a memory failure answers `bot.ability.conversation.failed` `{stage: "memory", message}` and commits nothing.
- <a id="conversation-10"></a>**conversation-10**. An input its schema refuses is never routed here: the send raises `error.execution` in the sender, and nothing is committed.
