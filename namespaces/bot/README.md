# bot namespace

`https://github.com/stateforward/floppy/namespaces/bot` (prefix `bot` by
convention): a bot body's synchronous per-turn bookkeeping as a Wasm
executable-content extension component (`floppy_bot`). It reads its
expressions and assigns its results through the data model; it never
sends, and it does no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<bot:stimulus location event data source target? id?>` | the `bot.ability.cognition.input` stimulus | the envelope `{event, data, source, target?, id?}` |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/bot.xsd`](package/bot.xsd), whose digest the
manifest [`package/bot.extension.json`](package/bot.extension.json) pins.
`scripts/build-components.sh` builds, virtualizes and checks the component
(imports: `types`, `telemetry`, `data-model-host`) and publishes it into the
dev package cache. The floppy body ([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx))
uses `stimulus` (the cognition, the conversation and the listening use it too); the turn's
attention bias is the [`cognition`](../cognition) namespace's, and the identity ability's
name recognition the [`identity`](../identity) namespace's.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="bot-1"></a>**bot-1**. `<bot:stimulus>` takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/bot.xsd`](package/bot.xsd), rejects it before any run under `scx validate`).
- <a id="bot-2"></a>**bot-2**. `<bot:stimulus>` writes the envelope {event, data, source, target?, id?}: event a non-empty string, source null when it evaluates to null, undefined or the empty string, and target and id included when given and not undefined; an empty event raises `error.execution` and assigns nothing.
