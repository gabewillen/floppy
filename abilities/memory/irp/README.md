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

- <a id="memory-1"></a>**memory-1**. Given a SQLite database path and statements, it migrates the database to head and runs the statements in one transaction, answering `bot.ability.memory.output` `{results}` (one result per statement, in order), then its session ends.
