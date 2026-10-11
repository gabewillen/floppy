# `sensitivity` implementation report plan

The implementation report plan of [`sensitivity.scx`](../sensitivity.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="sensitivity-1"></a>**sensitivity-1**. On start it offers nothing: it sends `bot.ability.offers` `{ability: "sensitivity", choices: []}` to its parent.
- <a id="sensitivity-2"></a>**sensitivity-2**. Every arrival scored (`bot.ability.listening.sensitivity.score` `{stimulus}`) comes back as `bot.ability.listening.sensitivity.output` `{sound, parent, perceived_level_db}`, its sound the stimulus's data and its parent the stimulus; quiet, the level is the sound's `received_level_db`, `null` when nothing was measured.
- <a id="sensitivity-3"></a>**sensitivity-3**. `bot.ability.speaking.efference` `{mouth, duration}` opens a producing window on that mouth; inside it, an arrival from another source is scored by its `received_level_db`.
- <a id="sensitivity-4"></a>**sensitivity-4**. Inside the window, an arrival from the producing mouth is scored against the prediction: with no levels to compare, 0; with no learned path gain yet, the observed gain (`received_level_db - amplitude_db`) is learned and the level is 0; afterwards the level is `|observed - learned|`, and the learned gain is never retaught.
- <a id="sensitivity-5"></a>**sensitivity-5**. The window lasts the command's duration: once it has passed, an arrival from the mouth is scored as quiet.
