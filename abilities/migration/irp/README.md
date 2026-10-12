# `migration` implementation report plan

The implementation report plan of [`migration.scx`](../migration.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="migration-1"></a>**migration-1**. It reads the database's revision from Alembic's `alembic_version` (creating the table when the database lacks it; no row is base) and, with no `target` <param>, upgrades to head: it answers its parent `bot.ability.migration.output` `{from, to}`, the revisions before and after (`null` for base).
- <a id="migration-2"></a>**migration-2**. The `target` <param> is Alembic's: `head`, `base`, a revision, or a relative `+N` / `-N` from the revision read.
- <a id="migration-3"></a>**migration-3**. The planned steps' statements run oldest first, after the version row's guard, in one transaction on the database's engine; a statement the engine refuses answers `bot.ability.failed` `{message}` with the engine's message.
- <a id="migration-4"></a>**migration-4**. At the target already, nothing is applied: it answers `{from, to}` with both the revision read.
- <a id="migration-5"></a>**migration-5**. A target the history does not hold, a relative step past base or head, or revisions that are not one linear history answer `bot.ability.failed` `{message: "Migration could not plan from <current> to <target>"}`.
- <a id="migration-6"></a>**migration-6**. A transaction that fails with a lost race's code (`rowcount`, `SQLITE_BUSY`, `23505`, `40001`, `40P01`) is read, planned and applied again once; failing again, it answers `bot.ability.failed` with the engine's message.
- <a id="migration-7"></a>**migration-7**. A database the engine cannot open answers `bot.ability.failed` `{message}`; the database names the engine (a `postgresql://` URI the pgsql service, else the sql service's SQLite file).
- <a id="migration-8"></a>**migration-8**. After its one answer (`bot.ability.migration.output` or `bot.ability.failed`) the session ends.
