# device namespace

`https://github.com/stateforward/floppy/namespaces/device` (prefix `device`
by convention): a bot's synchronous read of a device it holds, as a Wasm
executable-content extension component (`floppy_device`). It reads another
session of the run through scxml.rs's `session-snapshot-host.snapshot` with
a target (decision D109) and assigns the read through the data model; it
never sends, and it does no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<device:snapshot location target>` | a device's state machine snapshot, read for the bot's environment block and its focused device's choices | `{id, state, attributes, events}`: the session's `_sessionid`, its active atomic states (space-separated), its data model, and the event names its active transitions accept, each once (`*` is not listed) |

`target` is an expression of the session's address as a `<send>` target
names it (`#_parent`, `#_<invokeid>`, `#_scxml_<sessionid>`). A session the
run cannot read (unknown, finished, or failed) raises `error.execution` and
assigns nothing.

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/device.xsd`](package/device.xsd), whose digest the
manifest [`package/device.extension.json`](package/device.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`,
`session-snapshot-host`) and publishes it into the dev package cache.
