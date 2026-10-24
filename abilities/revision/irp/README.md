# `revision` implementation report plan

The implementation report plan of [`revision.scx`](../revision.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="revision-1"></a>**revision-1**. A create (`create` true) stores a DRAFT stub for its name, invokes the builder bot with a `builder.request` built from the intent and the turn, and on the builder's `done` (`accepting`) replaces the row ACTIVE with the built documents' path (decision D121) and answers `bot.ability.reflection.revision.output` `{behavior}`, the applied behavior: the intent's event, name, grounded triggers, description, reason and that path as `source` (decision D148).
- <a id="revision-2"></a>**revision-2**. A change of a name the inventory does not hold fails with `bot.ability.failed` `{message: "Revision selected unknown behavior: <name>."}`, asking no model.
- <a id="revision-3"></a>**revision-3**. A builder that fails with the last draft it rejected (`abandoning`) keeps that draft's path in the row as DRAFT (unless the row was ACTIVE, which keeps its program, decision D224) and fails with `Reflection change abandoned: attempts exhausted:` and the rendered diagnostics (decision D133); a builder that fails with no draft fails with `Reflection revision failed: <reason>: <first diagnostic>`.
- <a id="revision-4"></a>**revision-4**. A memory transaction that fails (here the preparing one, on a database the run does not grant) fails with `Reflection revision failed: <detail>`.
- <a id="revision-6"></a>**revision-6**. With an example and a counterexample, the builder also runs the behavior on the counterexample, which it must decline (decision D229): a draft that answers it too is refused.
- <a id="revision-5"></a>**revision-5**. A lesson (`instruction`) whose intent carries an `example`, on a conversation turn, has the builder run the behavior on this turn's input with the lesson's message replaced by the example, which the behavior must handle (decision D228): a draft that answers the example is accepted, and one that declines it is refused on the live turn.
