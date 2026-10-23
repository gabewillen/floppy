# intuition namespace

`https://github.com/stateforward/floppy/namespaces/intuition` (prefix
`intuition` by convention): a confidence tuner as a Wasm executable-content
extension component (`floppy_intuition`). The tuner is the fast stage's
self-tuning confidence baseline: each turn observes its confidence, then
escalates to reasoning when the confidence is below the updated threshold
(D114). The state `{mean, var, n}` lives in the document's
`confidence_tuner` datum and crosses turns by `<param>` and `<donedata>`;
the tuner's defaults are Rust constants. The elements read their
expressions and assign their results through the data model; they never
send, and they do no I/O.

| Element | Writes |
| --- | --- |
| `<intuition:observe location confidence>` | the tuner state after observing the confidence, unchanged for `null` (`location`) |
| `<intuition:escalate location confidence tuner>` | whether the confidence is below the tuner's threshold, `false` for `null` (`location`) |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/intuition.xsd`](package/intuition.xsd), whose digest the
manifest [`package/intuition.extension.json`](package/intuition.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The intuition stage
([`abilities/intuition/intuition.scx`](../../abilities/intuition/intuition.scx))
observes each decision's confidence and escalates by it.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="intuition-1"></a>**intuition-1**. Each element (observe, escalate) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/intuition.xsd`](package/intuition.xsd), rejects it before any run under `scx validate`).
- <a id="intuition-2"></a>**intuition-2**. `<intuition:observe>` keeps the tuner state {mean, var, n}: anything else is read as the initial state {mean: 70, var: 100, n: 0}; a number confidence, clamped to 0-100, is observed (the first sets the mean, later ones move the mean and variance by the EMA with alpha 0.08, the variance never under 100), and a null confidence is not.
- <a id="intuition-3"></a>**intuition-3**. `<intuition:escalate>` writes true exactly when the confidence is a number below the tuner's threshold: the floor 35 during the warmup of 8 observations, else max(17.5, mean - sqrt(max(var, 100))); a null confidence never escalates.
- <a id="intuition-4"></a>**intuition-4**. A confidence that is neither a number nor null raises `error.execution` and assigns nothing.
