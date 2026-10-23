# `routines_runner` implementation report plan

The implementation report plan of [`routines_runner.scx`](../routines_runner.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="routines_runner-1"></a>**routines_runner-1**. It takes the `<param>` `routines` `[{name, source}]` (`source` the path of the routine's program, decision D121) and runs the first routine as its child `routine` (`<invoke srcexpr>` on its source); on start it sends nothing of its own, and it never reaches a final state.
- <a id="routines_runner-2"></a>**routines_runner-2**. Each `bot.behavior.tick` its routine sends it goes to its parent unchanged, once.
- <a id="routines_runner-3"></a>**routines_runner-3**. With more than one routine it invokes itself (`rest`) with the rest of the routines, so every routine runs and each tick of the chain reaches the parent once, whatever its depth.
- <a id="routines_runner-4"></a>**routines_runner-4**. A tick whose `name` is none of the chain's routines' names is stale and not forwarded.
- <a id="routines_runner-5"></a>**routines_runner-5**. A routine that ends (`done.invoke.routine`) stays stopped, and the rest keep running.
- <a id="routines_runner-6"></a>**routines_runner-6**. A routine whose program does not compile (`error.execution` in the runner) stays stopped, logged `routines.failed` `<name> (error.execution)`, and the rest keep running.
- <a id="routines_runner-7"></a>**routines_runner-7**. A tick the routine sends whose data its schema rejects never reaches the runner (`error.execution` in the routine) and is not forwarded; the routine's later ticks are.
