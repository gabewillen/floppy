# `learning` implementation report plan

The implementation report plan of [`learning.scx`](../learning.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="learning-1"></a>**learning-1**. A `bot.ability.learning.lesson` `{turn, content, media_type, cognition_input, cognition_output, participants}` recalls the prior episodes and the inventory, asks the generate service (schema `dispatch`, its `events` at most one) to implement the lesson as a `bot.behavior.create`, `bot.behavior.change` or `bot.behavior.break` this session offers, and invokes the revision ability with the teaching turn and the lesson as its instruction; a built behavior answers `bot.ability.learning.output` `{lesson, lesson_turn, learned: true, behavior: {name, description, status: "ACTIVE"}, participants}`, lesson_turn the teaching turn's, and logs `learning.learned` with `lesson of turn <turn>: ` before the behavior's name and source (decisions D149 and D204).
- <a id="learning-2"></a>**learning-2**. A lesson that does not take answers `bot.ability.learning.output` with `learned` false and `behavior` null, its failure only logged (`learning.failed`, `lesson of turn <turn>: ` before why): without a database, for a lesson that is not text, for a reply that selects nothing, and for a build the revision fails.
- <a id="learning-3"></a>**learning-3**. A lesson that arrives while another is in hand waits and runs after it, in order: each lesson gets its own answer.
- <a id="learning-4"></a>**learning-4**. A lesson implemented as `bot.behavior.break` sets the named behavior's status BROKEN with the trimmed reason, building nothing, and answers `bot.ability.learning.output` with `learned` true and the behavior's `status` `BROKEN`; a break of a name the inventory does not hold is not learned (decision D205).
