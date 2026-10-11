# migration namespace

`https://github.com/stateforward/floppy/namespaces/migration` (prefix
`migration` by convention): Alembic's revision graph walk as a Wasm
executable-content extension component (`floppy_migration`), agnostic of the
database. It knows revisions and never SQL or an engine; it reads its
expressions and assigns its result through the data model, and does no I/O.

| Element | Alembic | Writes |
| --- | --- | --- |
| `<migration:plan location revisions current target?>` | `ScriptDirectory.iterate_revisions`, the `head`, `base`, `+N`, `-N` targets | `{from, to, steps}`, each step `{revision, direction, statements}` (`location`) |

The migration ability ([`abilities/migration/migration.scx`](../../abilities/migration/migration.scx),
Alembic's `command.upgrade` and `command.downgrade`) reads `alembic_version`
through the sql service, plans with this element, and applies the planned
statements and the version row's guard in one sql transaction (decisions D48,
D49). The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/migration.xsd`](package/migration.xsd), whose digest
the manifest [`package/migration.extension.json`](package/migration.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes it
into the dev package cache.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="migration-1"></a>**migration-1**. A `<migration:plan>` takes location, revisions and current, optionally target, each non-empty, no other attribute and no content: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/migration.xsd`](package/migration.xsd), rejects it before any run under `scx validate`).
- <a id="migration-2"></a>**migration-2**. Upgrading plans each revision after current up to the target, oldest first, each step {revision, direction: upgrade, statements: its upgrade statements}; from is current (null for base), to the target revision, and the target defaults to head.
- <a id="migration-3"></a>**migration-3**. Downgrading plans each revision from current down to the one after the target, newest first, each with its downgrade statements and direction downgrade; to is null for base.
- <a id="migration-4"></a>**migration-4**. A relative target +N or -N moves N revisions from current, and a plan already at its target has no steps.
- <a id="migration-5"></a>**migration-5**. Revisions must be an array forming one linear history (one first revision, every down_revision known, no revision followed by two, no cycle, no revision named twice); otherwise the element raises `error.execution` and assigns nothing.
- <a id="migration-6"></a>**migration-6**. A current or target revision the history does not hold, or a relative step past base or head, raises `error.execution` and assigns nothing.
