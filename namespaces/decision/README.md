# decision namespace

`https://github.com/stateforward/floppy/namespaces/decision` (prefix
`decision` by convention): the synchronous half of a decision as a Wasm
executable-content extension component (`floppy_decision`): reading the
choices (a TypeSafe `Choice` question's), its own or another session's.
The confidence tuner is the [`intuition`](../intuition)
namespace. What becomes of the choices (their model-facing schemas, the
turn's offered events, the dispatch tool, the selection confidence) is the
[`processing`](../processing) namespace. Asking a backend to choose is slow I/O and belongs to the
`decision` invoked service ([`services/decision`](../../services/decision)).
It reads its expressions and the session snapshot and assigns its results
through the data model; it never sends, and it does no I/O.

| Element | Reads | Writes |
| --- | --- | --- |
| `<decision:choice location events?>` | the offered events of the active state | the choices `[{event, schema, description?}]` (`location`) |
| `<decision:offers location target undeclared?>` | the live session `target` names (decision D242): its enabled transitions and its `offerable` [floppy field] data, which declares what each costs (`effort`, decision D206) | the choices `[{event, schema, description?, effort?}]` it offers its parent (`location`) |

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
- <a id="decision-4"></a>**decision-4**. An expression that does not evaluate to what the element reads (`<decision:offers>`' `target`, a string) raises `error.execution` and assigns nothing.
- <a id="decision-5"></a>**decision-5**. `<decision:offers>` writes, for the session its target names, one choice {event, schema, description?, effort?} per event its event:schema transitions without a cond take that its offerable data declares, with the declared effort, in the selection walk's order; a transition with a cond is never offered but still takes its events from later transitions, and undeclared="all" offers every such event.
- <a id="decision-6"></a>**decision-6**. A `<decision:offers>` target that no running session of the run answers assigns nothing and raises nothing; an offerable that is not an object of {effort?} entries raises `error.execution` and assigns nothing.
