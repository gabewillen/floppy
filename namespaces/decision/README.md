# decision namespace

`https://github.com/stateforward/floppy/namespaces/decision` (prefix
`decision` by convention): the synchronous half of a decision as a Wasm
executable-content extension component (`floppy_decision`): reading the
choices (a TypeSafe `Choice` question's) and grounding one property of them.
The confidence tuner is the [`intuition`](../intuition)
namespace. What becomes of the choices (their model-facing schemas, the
turn's offered events, the dispatch tool, the selection confidence) is the
[`processing`](../processing) namespace. Asking a backend to choose is slow I/O and belongs to the
`decision` invoked service ([`services/decision`](../../services/decision)).
It reads its expressions and the session snapshot and assigns its results
through the data model; it never sends, and it does no I/O.

| Element | Reads | Writes |
| --- | --- | --- |
| `<decision:choice location events? effort?>` | the offered events of the active state, and what they cost (`effort`, decision D206) | the choices `[{event, schema, description?, effort?}]` (`location`) |
| `<decision:pin location event property values>` | the values to ground a property in (for example `bot.focus_device`'s `device` in the focus candidates) | the choices with the property's `enum` pinned (`location`) |

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
- <a id="decision-4"></a>**decision-4**. `<decision:pin>` sets the values as the enum of the named property of every entry for the event, removes those entries when the values are empty, and raises `error.execution`, assigning nothing, for an entry whose schema has no such property.
- <a id="decision-5"></a>**decision-5**. A location that does not hold what the element reads raises `error.execution` and assigns nothing.
- <a id="decision-6"></a>**decision-6**. An `effort`, an expression of a whole number from 0 to 1000, is written as the `effort` of every choice `<decision:choice>` writes; a choice written without one carries none, and an `effort` that is not such a number raises `error.execution` and assigns nothing.
