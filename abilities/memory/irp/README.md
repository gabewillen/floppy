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

- <a id="memory-1"></a>**memory-1**. Before it serves a request, it brings the database to the head of floppy's revisions (two revisions, 0001, decision D193, and 0002, decision D256; in the engine's dialect) through the migration ability, so the statements see Alembic's `alembic_version` at `0002` and the tables, `bot_behavior` with its `lifetime` column, `bot_stm_turn`, the people `bot_person` and `bot_person_embedding` (decision D194), each person's representative `bot_person_centroid` and the named sequences `bot_sequence` (decision D205), and `bot_memory_embedding`.
- <a id="memory-2"></a>**memory-2**. A `bot.ability.memory.input` `{id, statements}` runs its `statements` (`[{sql, parameters?}]`) in order in one transaction and answers its sender `bot.ability.memory.output` `{id, results}`: one result per statement, in order, each `{rowcount, rows}` with every row `{columns, values}`; a statement's `parameters` bind its placeholders.
- <a id="memory-3"></a>**memory-3**. A statement that fails (the engine refuses it, or its `rowcount` is not the one it expects) answers its sender `bot.ability.failed` `{id, message, kind}`, the engine's message and kind `store_failed`, and no `bot.ability.memory.output`.
- <a id="memory-4"></a>**memory-4**. When the migration cannot bring the database to head, every request is answered `bot.ability.failed` `{id, message, kind}` with the migration's message and kind `unavailable`, and its statements never run.
- <a id="memory-5"></a>**memory-5**. The database names the engine: a `postgresql://` (or `postgres://`) URI is a Postgres database reached through the pgsql service, anything else a SQLite file through the sql service.
- <a id="memory-6"></a>**memory-6**. It is long-lived (decision D266): it answers each request once, one at a time and in the order they arrive, and goes on serving after an answer, `bot.ability.memory.output` or `bot.ability.failed`, until its invoker ends it.
