# The room

[`room.scx`](room.scx) is floppy's environment (decision D102): the root session `scx run` runs, whose
invoked children are its participants. It invokes the floppy bot
([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx)) as `floppy`,
which activates on joining (D107), and holds no device (no device is
ported yet; the tests add the lamp fixture, `tests/fixtures/environment/lamp/`).

| Data | What |
| --- | --- |
| `participants` | the invoke ids of the children present: set when the state that invokes them is entered; a child's `done.invoke.<id>` or `error.communication.<id>` removes it (each logged with its `invokeid` and data) |
| `holdings` | bot invoke id to the device invoke ids it holds (D105); its keys are the bots |
| `placements` | invoke id to `{position: {x, y, z}, threshold_db}` (D103); a participant with none hears every sound |
| `objects` | the ids of the physical things participants sit on (D159); a placement's `on` names one |
| `supervision` | `{max_restarts, backoff_initial_ms, backoff_max_ms, stable_after_ms}` (D158; by default 5, 1 s, 1 min, 1 min) |
| `attempts`, `activation_failed` | each bot's consecutive failed attempts, and whether its last activation failed |

What it does (decisions D103, D105, D108):

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
- **Supervision** (D158): each bot runs
  in its own region of `present` (`supervising.<bot>`). A bot that leaves
  degraded, after a failed activation, or failed is invoked again after a
  doubling backoff (`room.restarted`), until another attempt would not fit
  `max_restarts` (`room.exhausted`); one that has run `stable_after_ms` has
  its attempts reset, and one that leaves on `bot.deactivate` stays gone.
- **The bots' events** (`bot.activated`, `bot.processing.completed`, ...)
  are logged as `<log label="EVENT">` with their data. A voice embedding is
  never written: an identity that is an array of numbers in `source_ids` or
  `target_ids`, and an embedding's JSON inside a `*_ref`, are written as
  `<biometric:VoiceEmbedding>`.

Each bot gets `environment` (the room's `_sessionid`, D106),
`owned_devices` (each held device's reference to its address,
`<_sessionid>.<invoke id>`, D110) and `placed_on` (each held device that
sits on a known object, its reference to the object's id, D159) as
`<param>`s. The schemas it takes are
referenced where they live, in
[`bots/floppy/schemas/`](../../bots/floppy/schemas) and
[`abilities/schemas/`](../../abilities/schemas). Run it as
[`bots/floppy/README.md`](../../bots/floppy/README.md) says, with
`--fuel 9223372036854775807` (D111).
