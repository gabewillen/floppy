# `reflection` implementation report plan

The implementation report plan of [`reflection.scx`](../reflection.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="reflection-1"></a>**reflection-1**. Without a database (the `<param>` `database` null, its default) it offers nothing on start and answers each `bot.ability.reflection.input` `{turn, cognition_input, cognition_output}` at once with `bot.ability.reflection.output` (no data), asking no model.
- <a id="reflection-2"></a>**reflection-2**. With a database, `recalling` reads the prior episodes and the behavior inventory in one memory transaction and `processing` asks the generate service (schema `dispatch`) to select at most one of `bot.behavior.create`, `bot.behavior.change` or `bot.behavior.break`; an empty selection goes to `storing`, which stores the turn's episode and answers `bot.ability.reflection.output`.
- <a id="reflection-3"></a>**reflection-3**. More than one selected event fails the turn with `bot.ability.failed` `{message: "Reflection select must return at most one behavior event."}`; a failed select call fails it with `Reflection select failed (<code>)`; a select call that does not answer within `select_timeout_ms` fails it with `Reflection child cancellation timed out.`.
- <a id="reflection-4"></a>**reflection-4**. A selected `bot.behavior.break` (`breaking`) marks the named behavior BROKEN with its trimmed reason and keeps the row, then stores the episode and answers `bot.ability.reflection.output`; a break of a name the inventory does not hold fails with `Reflection break failed: Reflection break selected unknown behavior: <name>.`.
- <a id="reflection-5"></a>**reflection-5**. A selected `bot.behavior.create` (`revising`) stores a DRAFT stub for a new name, invokes the builder bot with a `builder.request`, and on the builder's `done` (`accepting`) replaces the row ACTIVE with the built document, then stores the episode and answers `bot.ability.reflection.output`; a `bot.behavior.change` of a stored name revises it the same way, and a change of a name the inventory does not hold fails with `Revision selected unknown behavior: <name>.`.
- <a id="reflection-6"></a>**reflection-6**. A builder that fails with the last draft it rejected (`abandoning`) keeps that draft in the row as DRAFT and fails the turn with `Reflection change abandoned: same diagnostic message after fix:` and the rendered diagnostics (no-progress); a builder that fails with no draft fails the turn with `Reflection revision failed: <reason>: <first diagnostic>`, the create's DRAFT stub kept.
- <a id="reflection-7"></a>**reflection-7**. `revising` is bounded by `revising_timeout_ms`: when it passes, the revision is cancelled and the turn fails with `Reflection child cancellation timed out.`.
- <a id="reflection-8"></a>**reflection-8**. A memory transaction that fails (here the recall, on a database the run does not grant) fails the turn with `Reflection memory recall failed: <detail>`; after any failure the turn's episode is stored with no behavior (unless storing it is what failed; decision D97) and the reflection is idle again and takes the next input.
- <a id="reflection-9"></a>**reflection-9**. An input that arrives while a reflection is in hand waits and runs after it, in order: each turn gets its own answer.
