# `memory` implementation report plan

The implementation report plan of [`memory.scx`](../memory.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="memory-1"></a>**memory-1**. Before the statements, it brings the database to the head of the original's revisions (0001 then 0002, in the engine's dialect) through the migration ability, so the statements see Alembic's `alembic_version` at `0002` and the original's tables, `bot_behavior` with its `lifetime` column.
- <a id="memory-2"></a>**memory-2**. It runs the `statements` <param> (`[{sql, parameters?}]`) in order in one transaction and answers its parent `bot.ability.memory.output` `{results}`: one result per statement, in order, each `{rowcount, rows}` with every row `{columns, values}`; a statement's `parameters` bind its placeholders.
- <a id="memory-3"></a>**memory-3**. A statement that fails (the engine refuses it, or its `rowcount` is not the one it expects) answers `bot.ability.failed` `{message}`, the engine's message, and no `bot.ability.memory.output`.
- <a id="memory-4"></a>**memory-4**. A database the migration cannot bring to head answers `bot.ability.failed` `{message}` with the migration's message, and the statements never run.
- <a id="memory-5"></a>**memory-5**. The database names the engine: a `postgresql://` (or `postgres://`) URI is a Postgres database reached through the pgsql service, anything else a SQLite file through the sql service.
- <a id="memory-6"></a>**memory-6**. After its one answer (`bot.ability.memory.output` or `bot.ability.failed`) the session ends.
