# memory namespace

`https://github.com/stateforward/floppy/namespaces/memory` (prefix
`memory` by convention): the original's memory module
(`abilities/memory`) as a Wasm executable-content extension component
(`floppy_memory`): the JSON a conversation memory row holds, and the
contents of one statement's result of a memory select. The memory ability
([`abilities/memory/memory.scx`](../../abilities/memory/memory.scx)) applies
the statements through the sql service; these elements only build and read
what it stores. They never send, and they do no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<memory:to_json location message>` | `Memory.to_json`, `_encode_content`, `value.identities_json` | the row's JSON text |
| `<memory:contents location result>` | `OutputData.contents` (each content parsed as its readers parse it) | the result's JSON objects, in row order |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/memory.xsd`](package/memory.xsd), whose digest the
manifest [`package/memory.extension.json`](package/memory.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The conversation
([`abilities/conversation/conversation.scx`](../../abilities/conversation/conversation.scx))
remembers its inputs with `to_json`; reasoning
([`abilities/reasoning/reasoning.scx`](../../abilities/reasoning/reasoning.scx))
reads its prior episodes and standing directives with `contents`.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="memory-1"></a>**memory-1**. Each element (to_json, contents) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/memory.xsd`](package/memory.xsd), rejects it before any run under scx validate).
- <a id="memory-2"></a>**memory-2**. `<memory:to_json>` writes the original's Memory.to_json of a message as one JSON text: {content, content_type, source_ids, target_ids} with sorted keys and compact separators, the content's arrays and objects tagged __stateforward_memory_content__ list and mapping (a mapping's entries ordered by their keys' ASCII JSON), a number that is not a safe integer as a Python float, and the identities sorted without repeats, embeddings as Python floats; a message without identity arrays raises `error.execution` and assigns nothing.
- <a id="memory-3"></a>**memory-3**. `<memory:contents>` writes, in row order, the first value of each row of one statement's result that is a string holding a JSON object, parsed, and skips every other row; a result without a rows array raises `error.execution` and assigns nothing.
