# cognition namespace

`https://github.com/stateforward/floppy/namespaces/cognition` (prefix
`cognition` by convention): the original's cognition module
(`abilities/cognition/types.py`, `event.py`) as a Wasm executable-content
extension component (`floppy_cognition`): the turn's attention bias and the
model-facing projection of a turn's input. The elements read their
expressions and assign their results through the data model; they never
send, and they do no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<cognition:attention_bias_for_turn location focuslocation? stimulus devices focus>` | `attention_bias_for_turn`, `_interrupt_reference`, `_target_device_reference` | the focus (`focuslocation`) and the focus candidates (`location`) |
| `<cognition:model_facing_content location failurelocation? stimulus prior_episodes standing_directives environment? instructions? schema?>` | `model_facing_content` of the `ProcessorInput` (`_project`, `_fill_field`, `_ProjectionBudget`), `InputData.model_facing_content`'s prompt budget, and `Environment.model_snapshot`'s block | the model-facing XML (`location`), or `null` and the original's budget message (`failurelocation`) |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/cognition.xsd`](package/cognition.xsd), whose digest the
manifest [`package/cognition.extension.json`](package/cognition.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The floppy body ([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx))
takes its turn's attention bias; reasoning
([`abilities/reasoning/reasoning.scx`](../../abilities/reasoning/reasoning.scx))
projects its prompt.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="cognition-1"></a>**cognition-1**. Each element (attention_bias_for_turn, model_facing_content) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/cognition.xsd`](package/cognition.xsd), rejects it before any run under `scx validate`).
- <a id="cognition-2"></a>**cognition-2**. `<cognition:attention_bias_for_turn>` writes the focus (the given focus when it is a configured device, else null) as the first candidate, then the stimulus's device (a bot.input's data.target_device, which the environment stamps on a device's input) when it is configured and not a candidate already; devices that are not an array raise `error.execution` and assign nothing.
- <a id="cognition-3"></a>**cognition-3**. `<cognition:model_facing_content>` writes the original's model-facing XML of the turn's input, indented as ElementTree writes it: `<cognition:processor_input>` holding host_input (a processing:input whose input is the stimulus, its data as content, stamped stimulus:event, then stimulus:id, stimulus:source and stimulus:target when set), prior_episodes and standing_directives with their counts, and the environment block, with failurelocation null.
- <a id="cognition-4"></a>**cognition-4**. Over one of the original's projection budgets (262144 encoded bytes, 4096 nodes, 4096 collection items, 65536 characters per scalar, and 262144 bytes for the instructions, the projection and the schema together), failurelocation receives the original's message and location null; without failurelocation the element raises `error.execution` and assigns nothing.
- <a id="cognition-5"></a>**cognition-5**. A stimulus that is neither an object with an event nor null, or episodes or directives that are not an array, raise `error.execution` and assign nothing.
