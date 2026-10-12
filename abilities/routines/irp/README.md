# `routines` implementation report plan

The implementation report plan of [`routines.scx`](../routines.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="routines-1"></a>**routines-1**. With no `database` `<param>` (null) it reconciles to nothing: it runs no routine and sends its parent nothing; it is long-lived and never reaches a final state.
- <a id="routines-2"></a>**routines-2**. With a database it recalls the ACTIVE persistent `bot_behavior` rows through the Memory ability (which brings the database to the head revision); an empty inventory runs no routine and sends its parent nothing (the Memory ability's answers stay with the routines).
- <a id="routines-3"></a>**routines-3**. `bot.ability.routines.reconcile` `{}` reads the inventory again, any number of times, back to back (a reconcile while reconciling re-enters it); it answers the parent nothing.
- <a id="routines-4"></a>**routines-4**. A recall that fails (a database the Memory ability refuses) is logged `bot.ability.routines.reconcile.failed` and keeps the running set; nothing reaches the parent and later reconciles still work.
- <a id="routines-5"></a>**routines-5**. A reconcile whose data its schema rejects (any field: it carries no data) is `error.execution` in its sender and never reaches the routines.
