# memory namespace

`https://github.com/stateforward/floppy/namespaces/memory` (prefix
`memory` by convention): the original's memory module
(`abilities/memory`) as a Wasm executable-content extension component
(`floppy_memory`): the JSON a conversation memory row holds, the
contents of one statement's result of a memory select, and the two pure
helpers of the original's short-term register (`stm.py`): a turn's words
cut at a whole word and a stimulus's bounded scalar projection. The memory ability
([`abilities/memory/memory.scx`](../../abilities/memory/memory.scx)) applies
the statements through the sql service; these elements only build and read
what it stores. They never send, and they do no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<memory:to_json location message>` | `Memory.to_json`, `_encode_content`, `value.identities_json` | the row's JSON text |
| `<memory:contents location result>` | `OutputData.contents` (each content parsed as its readers parse it) | the result's JSON objects, in row order |
| `<memory:cut_at_word location words>` | `stm._cut_at_word` (what `TurnRecordData.of` does with a turn's words) | `{text, cut}`: the words within 2048 UTF-8 bytes, cut at a whole word |
| `<memory:stm_projection location data>` | `stm._payload_projection` (what `stm.event_record` records) | the data's bounded scalar projection |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/memory.xsd`](package/memory.xsd), whose digest the
manifest [`package/memory.extension.json`](package/memory.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The conversation
([`abilities/conversation/conversation.scx`](../../abilities/conversation/conversation.scx))
remembers its inputs with `to_json`; reasoning
([`abilities/reasoning/reasoning.scx`](../../abilities/reasoning/reasoning.scx))
reads its prior episodes and standing directives with `contents`. The
short-term register
([`abilities/stm/stm.scx`](../../abilities/stm/stm.scx)) bounds every turn
it is told with `cut_at_word`; the producers of its records and turns (the
body's stimulus fan-out, listening's heard speech and the conversation's
said messages) build them with `stm_projection` and `cut_at_word`.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="memory-1"></a>**memory-1**. Each element (to_json, contents, cut_at_word, stm_projection) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/memory.xsd`](package/memory.xsd), rejects it before any run under scx validate).
- <a id="memory-2"></a>**memory-2**. `<memory:to_json>` writes the original's Memory.to_json of a message as one JSON text: {content, content_type, source_ids, target_ids} with sorted keys and compact separators, the content's arrays and objects tagged __stateforward_memory_content__ list and mapping (a mapping's entries ordered by their keys' ASCII JSON), a number that is not a safe integer as a Python float, and the identities sorted without repeats, embeddings as Python floats; a message without identity arrays raises `error.execution` and assigns nothing.
- <a id="memory-3"></a>**memory-3**. `<memory:contents>` writes, in row order, the first value of each row of one statement's result that is a string holding a JSON object, parsed, and skips every other row; a result without a rows array raises `error.execution` and assigns nothing.
- <a id="memory-4"></a>**memory-4**. `<memory:cut_at_word>` writes `{text, cut}` as the original's `_cut_at_word` returns them: words of at most 2048 UTF-8 bytes unchanged and `cut` false; longer words cut, `cut` true, to their whole characters within the bound, less the partial word the bound falls inside when an earlier word break exists, trailing white space (Unicode White_Space) removed; words that are not a string raise `error.execution` and assign nothing.
- <a id="memory-5"></a>**memory-5**. `<memory:stm_projection>` writes the original's `_payload_projection` of a value: an object's fields in their order, keeping each with a non-empty key and a boolean, number, or string of at most 40 characters, at most 16 fields, every other field dropped, never truncated; a bare boolean, number or bounded string as `{value}`; anything else `{}`.
