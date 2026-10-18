# decision namespace

`https://github.com/stateforward/floppy/namespaces/decision` (prefix
`decision` by convention): the synchronous half of a decision as a Wasm
executable-content extension component (`floppy_decision`): reading,
compacting, assembling and naming the choices, placing the selections,
building the original's dispatch tool schema, and tuning the confidence
threshold. Asking a backend to choose is slow I/O and belongs to the
`decision` invoked service ([`services/decision`](../../services/decision)).
It reads its expressions and the session snapshot and assigns its results
through the data model; it never sends, and it does no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<decision:choice location events?>` | the offered events of the active state | the choices `[{event, schema, description?}]` (`location`) |
| `<decision:compact location bound? overlocation?>` | `model_facing_event_json_schema` and the offer bound | the compacted choices (`location`) and whether they were over the bound (`overlocation`) |
| `<decision:pin location event property values>` | `bot.focus_device` grounded in the focus candidates | the choices with the property's `enum` pinned (`location`) |
| `<decision:assemble location choices offers bound? overlocation? sizelocation?>` | `collect_offered_events` | the turn's choices, tagged by offering target (`location`) |
| `<decision:route location rejectionlocation selections choices>` | `dispatch_selected_events`, `_resolve_target` | the placed selections (`location`) and the first rejection message (`rejectionlocation`) |
| `<decision:names location choices>` | | the choices' names (`location`) |
| `<decision:selection location choices>` | `_selection_item_branch`, `dispatch_tool` | the dispatch tool schema (`location`) |
| `<decision:tuner location thresholdlocation? observe?>` | `ConfidenceTuner` | the tuner state (`location`) and the threshold (`thresholdlocation`) |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/decision.xsd`](package/decision.xsd), whose digest
the manifest [`package/decision.extension.json`](package/decision.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`,
`session-snapshot-host`) and publishes it into the dev package cache.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="decision-1"></a>**decision-1**. Each element takes exactly its declared attributes, the required ones present, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/decision.xsd`](package/decision.xsd), rejects it before any run under `scx validate`).
- <a id="decision-2"></a>**decision-2**. `<decision:choice>` writes one choice {event, schema, description?} per event named by an active transition that carries an event:schema, in document order, each offered once (an earlier transition of the same state takes it); * and schemaless transitions are not offered, the description is the schema's description, else its title, first line, and events keeps only the choices a descriptor matches.
- <a id="decision-3"></a>**decision-3**. A transition whose cond is exactly true is offered; one whose cond is false is not.
- <a id="decision-4"></a>**decision-4**. `<decision:compact>` drops $schema, title and examples at every schema level, keeping property names; over the bound it drops every description, and still over it writes [], with overlocation true exactly when it wrote [].
- <a id="decision-5"></a>**decision-5**. `<decision:pin>` sets the values as the enum of the named property of every entry for the event, removes those entries when the values are empty, and raises `error.execution`, assigning nothing, for an entry whose schema has no such property.
- <a id="decision-6"></a>**decision-6**. `<decision:assemble>` writes the document's own choices untagged, then each offer's choices in member-name order tagged with the member's name as target, compacted, with its size and whether it is over the bound.
- <a id="decision-7"></a>**decision-7**. `<decision:route>` places each selection on the target it names when that target offers the event, else on the event's only offerer (null for the document's own); a selection that cannot be placed fails all of them with the original's message and an empty placement.
- <a id="decision-8"></a>**decision-8**. `<decision:names>` writes the choices' events, event@target for a targeted one, space separated.
- <a id="decision-9"></a>**decision-9**. `<decision:selection>` writes the original's dispatch tool as one schema: its description as the root description and its parameters {events: [anyOf one branch per offered event]} with the original's descriptions and examples; each branch {event, data, reason?, target?} has event a one-value enum, data the choice's schema without $schema (an empty object for none, wrapped as value for a non-object), required only when the schema requires a field, the schema's description else the event name, and target, required, a sorted enum of the offering targets only when one has a target; nothing is normalized for strict decoding.
- <a id="decision-10"></a>**decision-10**. `<decision:tuner>` keeps the original's ConfidenceTuner state {mean, var, n}: anything else is replaced by the initial state, an observation moves it, and the threshold is the floor during the warmup, else max(floor/2, mean - k_sigma * deviation), as a probability; an observation that is not a number raises `error.execution` and assigns nothing.
- <a id="decision-11"></a>**decision-11**. A location that does not hold what the element reads raises `error.execution` and assigns nothing.
