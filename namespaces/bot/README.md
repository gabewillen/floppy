# bot namespace

`https://github.com/stateforward/floppy/namespaces/bot` (prefix `bot` by
convention): a bot body's synchronous per-turn bookkeeping as a Wasm
executable-content extension component (`floppy_bot`). It reads its
expressions and assigns its results through the data model; it never
sends, and it does no I/O.

| Element | mosfet | Writes |
| --- | --- | --- |
| `<bot:attention location focuslocation? stimulus devices focus sources?>` | `attention_bias_for_turn`, `_interrupt_reference`, `_target_device_reference` | the focus (`focuslocation`) and the focus candidates (`location`) |
| `<bot:stimulus location event data source target? id?>` | the `bot.ability.cognition.input` stimulus | the envelope `{event, data, source, target?, id?}` |
| `<bot:recognize location name text>` | `recognition.hears_name` (the text `NameRecognizer`) | whether the text names the bot (`true`/`false`) |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/bot.xsd`](package/bot.xsd), whose digest the
manifest [`package/bot.extension.json`](package/bot.extension.json) pins.
`scripts/build-components.sh` builds, virtualizes and checks the component
(imports: `types`, `telemetry`, `data-model-host`) and publishes it into the
dev package cache. The floppy body ([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx))
uses `attention` and `stimulus`; the identity ability
([`abilities/identity.scx`](../../abilities/identity.scx)) uses `recognize`.
