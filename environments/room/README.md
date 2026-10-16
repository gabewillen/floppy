# The room

[`room.scx`](room.scx) is floppy's environment (the original's
`Environment`, decision D102): the root session `scx run` runs, whose
invoked children are its participants. It invokes the floppy bot
([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx)) as `floppy`,
which activates on joining (D107), and holds no device (no device is
ported yet; the tests add the lamp fixture, `tests/fixtures/environment/lamp/`).

| Data | What |
| --- | --- |
| `participants` | the invoke ids of the children present: set when the state that invokes them is entered; a child's `done.invoke.<id>` or `error.communication.<id>` removes it (each logged with its `invokeid` and data) |
| `holdings` | bot invoke id to the device invoke ids it holds (D105); its keys are the bots |
| `placements` | invoke id to `{position: {x, y, z}, threshold_db}` (D103); a participant with none hears every sound |

What it does (the original's behavior, decisions D103, D105, D108):

- **Host input** is routed by its payload: `bot.ability.communication.input`
  to the participants its `target_ids` names, to every bot when empty;
  `bot.deactivate` and the host's `bot.input` to every bot.
- **A device's `bot.input`** goes to the bots that hold it, with
  `target_device` stamped as its reference.
- **Sound and visuals** (`environment.sound`, `environment.visual`), from
  the host or a participant, are logged and delivered to every participant
  that perceives them: a sound's `received_level_db` follows the free-field
  law from the sender's placement (1 m reference, 1 cm clamp), and a
  participant below its threshold receives nothing.
- **The bots' events** (`bot.activated`, `bot.processing.completed`, ...)
  are logged as `<log label="EVENT">` with their data.

Each bot gets `environment` (the room's `_sessionid`, D106) and
`owned_devices` (each held device's reference to its address,
`<_sessionid>.<invoke id>`, D110) as `<param>`s. The schemas it takes are
byte-identical twins in [`schemas/`](schemas). Run it as
[`bots/floppy/README.md`](../../bots/floppy/README.md) says, with
`--fuel 9223372036854775807` (D111).
