# `stm` implementation report plan

The implementation report plan of [`stm.scx`](../stm.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="stm-1"></a>**stm-1**. With a database it opens it (migrating it to the head revision through the memory ability) and is ready: it sends `bot.ability.offers` `{ability: "stm", choices: []}` once; without one it is ready the same way, holds nothing (records and turns are dropped) and answers every read empty.
- <a id="stm-2"></a>**stm-2**. `bot.ability.memory.stm.record` `{stimulus_name, payload, event_id?, source?, target?}` is written with no reply, and `bot.ability.memory.stm.recent` `{stimulus_name?, limit?}` answers `bot.ability.memory.stm.recent.output` `{entries}`: the records inside the recency window, newest first, each `{stm_id, stimulus_name, payload, event_id, source, target, created_at, last_accessed_at}`, optionally only one stimulus's, at most `limit`.
- <a id="stm-3"></a>**stm-3**. Each record evicts, in its own transaction, every record beyond `capacity` by the effective clock (last access, else admission), so the register holds the newest `capacity`; a read past `recency_window_ms` finds nothing.
- <a id="stm-4"></a>**stm-4**. `bot.ability.memory.stm.reinforce` `{stm_ids}` stamps those records' `last_accessed_at` (no reply), which moves their effective clock forward for reads and eviction.
- <a id="stm-5"></a>**stm-5**. `bot.ability.memory.stm.turn` `{id, direction, speakers, text, cut, conversation}` is written with no reply, and `bot.ability.memory.stm.recent_turns` `{limit?, within?}` answers `bot.ability.memory.stm.recent_turns.output` `{turns}`: the newest turns, at most `limit`, oldest first, each `{id, at, direction, speakers, text, cut, conversation}` with `at` UTC (Z).
- <a id="stm-6"></a>**stm-6**. A turn that puts the register over `turn_capacity` sheds the older half at once, in the same transaction: the newest `turn_capacity - floor(turn_capacity / 2)` stay.
- <a id="stm-7"></a>**stm-7**. A turn's words longer than 2048 UTF-8 bytes are cut at a whole word and its `cut` is true.
- <a id="stm-8"></a>**stm-8**. The turns' window is `turn_window_ms`, or `within` (milliseconds) when that is shorter; its floor moves in whole steps of half the window, so a turn is read for at least the window and at most one and a half windows.
- <a id="stm-9"></a>**stm-9**. Requests and notifications are applied one at a time, in arrival order: what arrives while it is opening or applying waits, so a read sees every write sent before it.
- <a id="stm-10"></a>**stm-10**. A database it cannot open leaves it degraded, without an offer: it sends `bot.ability.failed` `{message: "unavailable: StmMemory is degraded."}`, answers every request the same way, and drops every notification.
