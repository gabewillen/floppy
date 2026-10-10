# conversation namespace

`https://github.com/stateforward/floppy/namespaces/conversation` (prefix
`conversation` by convention): one committed conversation message as a Wasm
executable-content extension component (`floppy_conversation`). It reads its
expressions and assigns its results through the data model; it never sends,
never holds the history, and does no I/O.

| Element | mosfet | Writes |
| --- | --- | --- |
| `<conversation:commit location keylocation? direction event message sequence provenance?>` | `_message_for_turn`, `_model_safe_content`, `_stable_identity_basis`, `_stable_context_ref`, `MessageProvenance` | the committed message (`location`) and its transcript key, the sorted source ids (`keylocation`) |

The document appends the message to its own history and transcripts with
short `<assign>`s, so the history never crosses the extension boundary. The
exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/conversation.xsd`](package/conversation.xsd), whose
digest the manifest
[`package/conversation.extension.json`](package/conversation.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes it
into the dev package cache. The conversation
([`abilities/conversation/conversation.scx`](../../abilities/conversation/conversation.scx)) commits
inbound and outbound messages with it, and the communication
([`abilities/communication/communication.scx`](../../abilities/communication/communication.scx)) builds
the outbound record it appends.
