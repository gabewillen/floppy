# conversation namespace

`https://github.com/stateforward/floppy/namespaces/conversation` (prefix
`conversation` by convention): the original's conversation module
(`abilities/communication/conversation`) and its turn detector as a Wasm
executable-content extension component (`floppy_conversation`). The
elements read their expressions and assign their results through the data
model; they never send, never hold the history, and do no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<conversation:message_for_turn location direction event message sequence provenance?>` | `_message_for_turn`, `_model_safe_content`, `MessageProvenance` | the committed message |
| `<conversation:relationship_for_input location indexlocation input>` | `Conversation._relationship_for_input`, `_stable_context_ref`, `_new_relationship_ref` | the relationships, a new one appended, and the input's index (`indexlocation`) |
| `<conversation:participant_for_source location source>` | `Conversation._participant_for_source` | the relationship with the source a participant |
| `<conversation:fail_turn location source>` | `Conversation._fail_turn` (its participants) | the relationship without the source |
| `<conversation:detector_refs location relationships>` | `Conversation.detector_refs` | `[session_ref, participant]` pairs |
| `<conversation:new_track_ref location source>` | `_new_track_ref` | the source's track |
| `<conversation:content_for_detector location input source>` | `_content_for_detector` | the stimulus that opens the source's turn |
| `<conversation:complete_turn location event input turns session_ref sequence>` | `Conversation._complete_turn` | the turns' committed message |
| `<conversation:ingest_content location content>` | `TurnDetector._ingest_content` | the open turn's content |
| `<conversation:on_end location turn participant_ref conversation_ref turn_ref source_participant_ref>` | `TurnDetector._on_end` | the closed turn, `{turn}` or `{failure}` |
| `<conversation:turn_participants location input parent?>` | `turn_participants` (`f6c4e08`) | the participants the turn's cognition handoff stamps |

The document appends each message to its own history with an `<assign>`,
so the history never crosses the extension boundary.
The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/conversation.xsd`](package/conversation.xsd), whose digest the
manifest [`package/conversation.extension.json`](package/conversation.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The conversation
([`abilities/conversation/conversation.scx`](../../abilities/conversation/conversation.scx))
commits its messages and keeps its relationships with them, the turn
detector ([`abilities/turn_detector/turn_detector.scx`](../../abilities/turn_detector/turn_detector.scx))
assembles and closes its turns, and the speaking
([`abilities/speaking/speaking.scx`](../../abilities/speaking/speaking.scx))
builds the outbound record it appends.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="conversation-1"></a>**conversation-1**. Each element (message_for_turn, relationship_for_input, participant_for_source, fail_turn, detector_refs, new_track_ref, content_for_detector, complete_turn, ingest_content, on_end, turn_participants) takes exactly its declared attributes, the required ones present and none empty, message_for_turn's direction exactly inbound or outbound, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/conversation.xsd`](package/conversation.xsd), rejects it before any run under scx validate).
- <a id="conversation-2"></a>**conversation-2**. `<conversation:message_for_turn>` writes the committed message {sequence, direction, source_ids, target_ids, content, content_type, provenance}, without the message's other members: inbound audio content is null, other content is kept as given, and absent content is null; a provenance's id, source, target and session_ref are taken when they are strings, else null, and its turn_ref is taken as given; without a session_ref an inbound message's is relationship-<its relationship context reference> and an outbound one's null.
- <a id="conversation-3"></a>**conversation-3**. `<conversation:relationship_for_input>` writes the index of the input's relationship: targetless input shares the first ambient relationship, other input the best scoring one in either direction (names equal, embeddings at least 0.85 alike, one to one); with none it appends a relationship whose context_ref is {"identity_groups":[...]} over the sorted identity groups, whose session_ref is relationship- and that, and whose participants and sequences are empty.
- <a id="conversation-4"></a>**conversation-4**. `<conversation:participant_for_source>` adds the source to the relationship's participants once, `<conversation:fail_turn>` drops it, and `<conversation:detector_refs>` writes [session_ref, participant] of every participant, ordered by session_ref and then identity.
- <a id="conversation-5"></a>**conversation-5**. `<conversation:new_track_ref>` writes a name as its own track, an embedding as track- and its unit vector's JSON, and null for no source; `<conversation:content_for_detector>` writes the stimulus that opens the source's turn: text/plain string content as a text stimulus, audio/* string content as an audio stimulus with its rate and channels, anything else as a content stimulus.
- <a id="conversation-6"></a>**conversation-6**. `<conversation:complete_turn>` writes the inbound message of the turns' product: their texts joined by spaces as text/plain when any has text, each distinct text once; else no content for audio/*; else the input's content; with the first turn's turn_ref and the given session_ref.
- <a id="conversation-7"></a>**conversation-7**. `<conversation:ingest_content>` takes a stimulus into the open turn's content (a text's stripped text when not empty, an audio chunk with the first rate and channels, a content, an image as image/*, an event as application/event), and `<conversation:on_end>` writes the closed turn {turn} with its text, audio or content, or {failure} at stage turn when it has neither text, audio nor content, or both text and audio.
- <a id="conversation-8"></a>**conversation-8**. A value of the wrong kind (relationships that are not an array of relationships, a source that is not an identity, no turns, a stimulus that is not an object or null) raises `error.execution` and assigns nothing.
- <a id="conversation-9"></a>**conversation-9**. `<conversation:turn_participants>` writes the participants a cognition handoff stamps for the turn: the input's string identities, source_ids as sources then target_ids as targets, each group in the original's identity order, then the parent's data's participants, each once, at most 32; an embedding, an empty name, or a name over 200 characters is never stamped; an input without source_ids and target_ids, or carried participants that are not {ref, role}, raise `error.execution` and assign nothing.
