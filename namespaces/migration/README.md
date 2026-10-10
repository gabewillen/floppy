# migration namespace

`https://github.com/stateforward/floppy/namespaces/migration` (prefix
`migration` by convention): Alembic's revision graph walk as a Wasm
executable-content extension component (`floppy_migration`), agnostic of the
database. It knows revisions and never SQL or an engine; it reads its
expressions and assigns its result through the data model, and does no I/O.

| Element | Alembic | Writes |
| --- | --- | --- |
| `<migration:plan location revisions current target?>` | `ScriptDirectory.iterate_revisions`, the `head`, `base`, `+N`, `-N` targets | `{from, to, steps}`, each step `{revision, direction, statements}` (`location`) |

The migration ability ([`abilities/migration.scx`](../../abilities/migration.scx),
Alembic's `command.upgrade` and `command.downgrade`) reads `alembic_version`
through the sql service, plans with this element, and applies the planned
statements and the version row's guard in one sql transaction (decisions D48,
D49). The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/migration.xsd`](package/migration.xsd), whose digest
the manifest [`package/migration.extension.json`](package/migration.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes it
into the dev package cache.
