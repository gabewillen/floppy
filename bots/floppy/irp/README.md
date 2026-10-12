# `floppy` implementation report plan

The implementation report plan of [`floppy.scx`](../floppy.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="floppy-1"></a>**floppy-1**. `bot.activate` enters `live`, which invokes the abilities (`cognition`, `communication`, `identity`, `listening`); the bot is `active`, and takes a turn's input, only once every expected ability (the data `expected`) has sent its first `bot.ability.offers`.
- <a id="floppy-2"></a>**floppy-2**. While `activating`, an `error.*` event, an ability's `done.invoke`, or the activation timer (`activation_timeout_ms`) fails the activation back to `inactive`, where no input is taken until the next `bot.activate`.
- <a id="floppy-3"></a>**floppy-3**. `bot.deactivate` leaves `live` for `inactive`, from `active` or `activating`; a later `bot.activate` makes the bot live again.
- <a id="floppy-4"></a>**floppy-4**. `bot.reboot` `{reason}` re-enters `live`: the abilities are invoked again and the bot waits for their offers again before it takes input; the turn count goes on.
- <a id="floppy-5"></a>**floppy-5**. In `unfocused` or `focused`, `bot.input` or a `bot.ability.cognition.input` `{stimulus}` handoff starts a `processing` turn, counted from 1; the cognition's `bot.ability.cognition.output` or `bot.ability.failed` for the current turn ends it (back to attention), and one for another turn is dropped.
- <a id="floppy-6"></a>**floppy-6**. `bot.processing.timed_out` for the current turn moves to `cancelling_processing`; the acknowledgement (`bot.ability.cognition.cancelled` or `bot.ability.processing.cancelled` for the turn) ends the turn; `bot.processing.cancel.timed_out` for it degrades the bot: the session ends in the final state `degraded` with `{reason: "turn <n>: the cognition did not acknowledge its cancel"}`. A timer for another turn is not taken.
- <a id="floppy-7"></a>**floppy-7**. An input that arrives during a turn waits and starts the next turn when the bot returns to attention.
- <a id="floppy-8"></a>**floppy-8**. An ability that ends while the bot is active (`done.invoke.<id>`) degrades it: the session ends in `degraded` with `{reason: "the ability <id> ended while active"}`.
