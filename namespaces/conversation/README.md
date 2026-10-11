# conversation namespace

`https://github.com/stateforward/floppy/namespaces/conversation` (prefix
`conversation` by convention): one committed conversation message as a Wasm
executable-content extension component (`floppy_conversation`). It reads its
expressions and assigns its results through the data model; it never sends,
never holds the history, and does no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<conversation:commit location keylocation? direction event message sequence provenance?>` | `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`, `_stable_context_ref`, `relationship_context_ref`, `MessageProvenance` | the committed message (`location`) and its relationship context reference, `{"identity_groups":[...]}` (`keylocation`) |

The document appends the message to its own history and relationships with
short `<assign>`s, so the history never crosses the extension boundary. The
exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/conversation.xsd`](package/conversation.xsd), whose
digest the manifest
[`package/conversation.extension.json`](package/conversation.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes it
into the dev package cache. The conversation
([`abilities/conversation/conversation.scx`](../../abilities/conversation/conversation.scx)) commits
inbound and outbound messages with it, and the speaking
([`abilities/speaking/speaking.scx`](../../abilities/speaking/speaking.scx)) builds
the outbound record it appends.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="conversation-1"></a>**conversation-1**. `<conversation:commit>` takes location, direction (exactly inbound or outbound), event, message and sequence, optionally keylocation and provenance, no other attribute and no content: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/conversation.xsd`](package/conversation.xsd), rejects it before any run under `scx validate`).
- <a id="conversation-2"></a>**conversation-2**. The committed message is {sequence, direction, source_ids, target_ids, content, content_type, provenance}, without the message's other members; inbound audio content is null, other content is kept as given, and absent content is null.
- <a id="conversation-3"></a>**conversation-3**. The relationship context reference is {"identity_groups":[...]} over the sorted source and target identity groups, the same in either direction, written to keylocation; without a provenance, an inbound message's session_ref is relationship-<reference> and an outbound one's is null.
- <a id="conversation-4"></a>**conversation-4**. A provenance's id, source, target and session_ref are taken when they are strings, else null, and its turn_ref is taken as given.
- <a id="conversation-5"></a>**conversation-5**. A malformed value (a negative sequence, an empty event, a message without a content_type) raises `error.execution` and assigns nothing.
