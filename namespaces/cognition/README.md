# cognition namespace

`https://github.com/stateforward/floppy/namespaces/cognition` (prefix
`cognition` by convention): a Wasm executable-content extension component
(`floppy_cognition`) for the turn's attention bias and the model-facing
projection of a turn's input. The elements read their expressions and
assign their results through the data model; they never send, and they do
no I/O.

| Element | Writes |
| --- | --- |
| `<cognition:attention_bias_for_turn location focuslocation? stimulus devices focus>` | the focus (`focuslocation`) and the focus candidates (`location`) |
| `<cognition:model_facing_content location failurelocation? systemlocation? fitlocation? stimulus? self? recent? environment? acted? prior_episodes? standing_directives? feedback? instructions? schema? budget?>` | the user content, stable first: what the bot has in mind's stable part, the situation, the rest of what it has in mind, the stimulus, the feedback (`location`), the system channel (`systemlocation`), the fit (`fitlocation`), or `null`s and the exceeded budget's message (`failurelocation`) |

Recalled memory never fails the projection (decision D117):
the recent turns (short-term memory's, kept from the newest and uncounted, so
each turn's prompt opens as the last one did) claim the bytes first, then the
episodes and directives (floppy's recalled memory until associative memory
exists, decision D97), each into the bytes the system channel, the schema,
the situation, the stimulus and the feedback leave of the prompt's 262144
(or of a fitted `budget`, less the system channel and the schema); an item
that does not fit is left out, `count` (where the field has one) is how many
were kept, and `omitted="<n>"` follows when any was left out, and a part
whose frame does not fit is left out whole. The stimulus is never cut: over
a fitted budget it still renders whole and the fit is `overflow`. Object
members render in the data model's order (sorted keys). A conversation
output's messages render without `provenance`, `source_ids` and
`target_ids`, fields a model never sees. On wasm, every value crossing the
data model's boundary is bounded at 65536 bytes (scxml.rs's ECMAScript data
model, `MAX_VALUE_BYTES`), so a prompt that would be trimmed is refused by
the assignment first (`error.execution`); the trimming is tested natively
([`src/tests.rs`](src/tests.rs)), and the bound is an open question in
[parity-gaps.md](../../docs/plans/parity-gaps.md).

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/cognition.xsd`](package/cognition.xsd), whose digest the
manifest [`package/cognition.extension.json`](package/cognition.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The floppy body ([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx))
takes its turn's attention bias; reasoning
([`abilities/reasoning/reasoning.scx`](../../abilities/reasoning/reasoning.scx))
projects its prompt and system channel.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="cognition-1"></a>**cognition-1**. Each element (attention_bias_for_turn, model_facing_content) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/cognition.xsd`](package/cognition.xsd), rejects it before any run under `scx validate`).
- <a id="cognition-2"></a>**cognition-2**. `<cognition:attention_bias_for_turn>` writes the focus (the given focus when it is a configured device, else null) as the first candidate, then the stimulus's device (a bot.input's data.target_device, which the environment stamps on a device's input) when it is configured and not a candidate already; devices that are not an array raise `error.execution` and assign nothing.
- <a id="cognition-3"></a>**cognition-3**. `<cognition:model_facing_content>` writes the model-facing content of a turn, each block indented two spaces per level and the blocks joined by newlines, most stable first: the recent turns (a cognition:recalled holding an uncounted recent of memory:turn lines), the situation (the environment block, then a cognition:acted of what earlier stages already did), the recalled episodes and directives (a cognition:recalled), the stimulus, when given, as the root element of its own block (tagged by its payload's type, stamped stimulus:event, then stimulus:id, stimulus:source and stimulus:target when set), then the feedback; systemlocation receives the instructions then the self block (a persona element), and failurelocation null.
- <a id="cognition-4"></a>**cognition-4**. Over one of the projection budgets (262144 encoded bytes, 4096 nodes, 4096 collection items, 65536 characters per scalar, and 65536 characters for the system channel, the situation and the feedback; the system channel and the schema reserved first from the prompt's 262144 bytes), failurelocation receives the exceeded budget's message and location null; without failurelocation the element raises error.execution and assigns nothing. Recalled memory never fails the projection: it keeps its newest items that fit, or is left out when its frame does not fit. With a budget (a fitted request size in bytes), fitlocation receives fit when the content fits whole beside the system channel and the schema, cut when recalled memory was trimmed to fit, and overflow when it is still over.
- <a id="cognition-5"></a>**cognition-5**. A stimulus that is neither an object with an event nor null, or recent turns, episodes or directives that are neither an array nor null, raise error.execution and assign nothing.
