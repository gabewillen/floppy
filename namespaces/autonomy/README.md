# autonomy namespace

`https://github.com/stateforward/floppy/namespaces/autonomy` (prefix
`autonomy` by convention): a Wasm executable-content extension component
(`floppy_autonomy`) that builds the input a learned behavior runs with. The
element reads its expressions and assigns its result through the data
model; it never sends, and it does no I/O.

| Element | Writes |
| --- | --- |
| `<autonomy:behavior_input_payload location stimulus focus focus_candidates>` | the stimulus data (or `{event: data}`) with the turn's focus |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/autonomy.xsd`](package/autonomy.xsd), whose digest the
manifest [`package/autonomy.extension.json`](package/autonomy.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. Autonomy
([`abilities/autonomy/autonomy.scx`](../../abilities/autonomy/autonomy.scx))
runs its services with it, and the reflection
([`abilities/reflection/reflection.scx`](../../abilities/reflection/reflection.scx))
tells the builder with it what a turn behavior will run with.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="autonomy-1"></a>**autonomy-1**. `<autonomy:behavior_input_payload>` takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/autonomy.xsd`](package/autonomy.xsd), rejects it before any run under scx validate).
- <a id="autonomy-2"></a>**autonomy-2**. `<autonomy:behavior_input_payload>` writes the stimulus data's members when the data is an object (not an array), else {event: data}, with focus and focus_candidates set to the values given (one that is undefined left out); a stimulus that is not an object raises `error.execution` and assigns nothing.
