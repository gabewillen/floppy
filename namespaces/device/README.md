# device namespace

`https://github.com/stateforward/floppy/namespaces/device` (prefix `device`
by convention): a bot's synchronous read of a device it holds, as a Wasm
executable-content extension component (`floppy_device`). It reads another
session of the run through scxml.rs's `session-snapshot-host.snapshot` with
a target (decision D109) and assigns the read through the data model; it
never sends, and it does no I/O.

| Element | Reads | Writes |
| --- | --- | --- |
| `<device:snapshot location target>` | a device's state machine snapshot, read for the bot's environment block and its focused device's choices | `{id, state, attributes, events}`: the session's `_sessionid`, its active atomic states (space-separated), its data model, and the event names its active transitions accept, each once (`*` is not listed) |

`target` is an expression of the session's address as a `<send>` target
names it (`#_parent`, `#_<invokeid>`, `#_scxml_<sessionid>`). A target no
running session of the run answers (a session that is not running,
finished or failed) is nothing to read: the element assigns nothing and
raises nothing.

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/device.xsd`](package/device.xsd), whose digest the
manifest [`package/device.extension.json`](package/device.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`,
`session-snapshot-host`) and publishes it into the dev package cache.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="device-1"></a>**device-1**. `<device:snapshot>` takes exactly `location` and `target`, both present and not empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/device.xsd`](package/device.xsd), rejects it before any run under `scx validate`).
- <a id="device-2"></a>**device-2**. `<device:snapshot>` writes `{id, state, attributes, events}` of the session its `target` names: its `_sessionid` (for an invoked child, `<parent _sessionid>.<invokeid>`), its active atomic states in document order separated by one space, its data model by `<data>` id, and the event names its active transitions accept, each once, a descriptor `*` naming none.
- <a id="device-3"></a>**device-3**. A `target` that no running session of the run answers assigns nothing and raises nothing: `location` keeps its value.
