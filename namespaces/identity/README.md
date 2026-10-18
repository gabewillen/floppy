# identity namespace

`https://github.com/stateforward/floppy/namespaces/identity` (prefix
`identity` by convention): the original's identity module
(`abilities/identity/recognition.py`, `value.py`) as a Wasm
executable-content extension component (`floppy_identity`): whether a text
names the bot, and the canonical order of participant identities. The
elements read their expressions and assign their results through the data
model; they never send, and they do no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<identity:hears_name location name text>` | `recognition.hears_name` (the text recognizer) | whether the text names the bot (`true`/`false`) |
| `<identity:sorted_identities location ids>` | `value.sorted_identities` | the identities in the original's order |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/identity.xsd`](package/identity.xsd), whose digest the
manifest [`package/identity.extension.json`](package/identity.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The identity ability
([`abilities/identity/identity.scx`](../../abilities/identity/identity.scx))
hears its name with `hears_name`; the conversation
([`abilities/conversation/conversation.scx`](../../abilities/conversation/conversation.scx))
orders an input's sources with `sorted_identities`.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="identity-1"></a>**identity-1**. Each element (hears_name, sorted_identities) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/identity.xsd`](package/identity.xsd), rejects it before any run under scx validate).
- <a id="identity-2"></a>**identity-2**. `<identity:hears_name>` writes true exactly when the name's case-folded letter-and-digit tokens appear as a whole run among the text's; a missing name or one with no tokens is never heard, and a text that is neither a string nor null raises `error.execution` and assigns nothing.
- <a id="identity-3"></a>**identity-3**. `<identity:sorted_identities>` writes the identities in the original's order: names first by their text in code point order, then embeddings by their compact Python JSON, stable and with repeats kept; ids that are not an array of identities raise `error.execution` and assign nothing.
