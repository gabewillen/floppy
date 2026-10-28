# processing namespace

`https://github.com/stateforward/floppy/namespaces/processing` (prefix
`processing` by convention): a Wasm executable-content extension component
(`floppy_processing`). It works out what a stage may send and how
confident a selection is: the model-facing schemas of the offered events,
the turn's offered events tagged with the session that offers each, the
`dispatch` tool, and the selection confidence. Dispatching
itself is SCXML's `<send>` ("dispatch is obviously send"): each stage sends
its own selections with `<foreach>` and
`<send eventexpr="s.event" targetexpr="'#_scxml_' + s.target">`, so this
namespace reads its expressions and assigns its results through the data
model; it never sends, and it does no I/O.

| Element | Writes |
| --- | --- |
| `<processing:model_facing_schema location bound? overlocation?>` | the choices in place without `$schema`, `title` and `examples`, over the bound, when one is named, without descriptions, still over `[]` (`location`), and whether that happened (`overlocation`) |
| `<processing:collect_offered_events location choices offers? devices? focus? bound? overlocation? sizelocation?>` | the turn's choices, each tagged with its offering session's id as `target`: the document's own with its `_sessionid`, each child's offers with `<_sessionid>.<invoke id>`, the focused device's events with its id (`location`), their size (`sizelocation`) and whether they are over the bound (`overlocation`) |
| `<processing:send location choices max?>` | the `dispatch` tool's schema, `max` its `events`' `maxItems` (`location`) |
| `<processing:selection_confidence location selections>` | the least normalized confidence of the selections, or `null` (`location`) |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/processing.xsd`](package/processing.xsd), whose
digest the manifest
[`package/processing.extension.json`](package/processing.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The floppy body
([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx)) collects its
turn's offered events; the cognition makes its offers model-facing;
reasoning and reflection build their `dispatch` tool; reflection tags its
own choices; and intuition takes its selection confidence.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="processing-1"></a>**processing-1**. Each element takes exactly its declared attributes, the required ones present, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/processing.xsd`](package/processing.xsd), rejects it before any run under `scx validate`).
- <a id="processing-2"></a>**processing-2**. `<processing:model_facing_schema>` drops $schema, title and examples at every schema level, keeping property names; over the bound it drops every description, and still over it writes [], with overlocation true exactly when it wrote [].
- <a id="processing-3"></a>**processing-3**. `<processing:collect_offered_events>` writes the document's own choices tagged with its _sessionid, then each offer's choices and the focused device's events (schema {type: object}, tagged with its id) in name order, each offer's tagged with <_sessionid>.<invoke id>, made model-facing, with its size and whether it is over the bound.
- <a id="processing-4"></a>**processing-4**. `<processing:send>` writes the dispatch tool as one schema: its description as the root description and its parameters {events: [anyOf one branch per offered event]} with the tool's fixed descriptions and examples, and max as the events' maxItems; each branch {event, data, reason?, target?} has event a one-value enum, data the choice's schema without $schema (an empty object for none, wrapped as value for a non-object), required in every branch (decision D231), the schema's description else the event name, and target, required, a sorted enum of the offering targets when one has a target; nothing is normalized for strict decoding.
- <a id="processing-5"></a>**processing-5**. `<processing:selection_confidence>` writes the least of the selections' confidences, each normalized (a number in (0, 1] scaled to 0-100, rounded half to even, clamped to 0-100), leaving out a selection whose confidence is not a number, and null when none is left.
- <a id="processing-6"></a>**processing-6**. A location or expression that does not hold what the element reads raises `error.execution` and assigns nothing.
