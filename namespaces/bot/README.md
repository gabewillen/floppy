# bot namespace

`https://github.com/stateforward/floppy/namespaces/bot` (prefix `bot` by
convention): a bot body's synchronous per-turn bookkeeping as a Wasm
executable-content extension component (`floppy_bot`). It reads its
expressions and assigns its results through the data model; it never
sends, and it does no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<bot:attention location focuslocation? stimulus devices focus>` | `attention_bias_for_turn`, `_interrupt_reference`, `_target_device_reference` | the focus (`focuslocation`) and the focus candidates (`location`) |
| `<bot:verdict location rejectionlocation selections focus candidates>` | `without_ignore_selections`, `attention_selection_error` | the selections without the ignore ones (`location`) and the first refused focus or clear selection's message, else `null` (`rejectionlocation`) |
| `<bot:stimulus location event data source target? id?>` | the `bot.ability.cognition.input` stimulus | the envelope `{event, data, source, target?, id?}` |
| `<bot:recognize location name text>` | `recognition.hears_name` (the text `NameRecognizer`) | whether the text names the bot (`true`/`false`) |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/bot.xsd`](package/bot.xsd), whose digest the
manifest [`package/bot.extension.json`](package/bot.extension.json) pins.
`scripts/build-components.sh` builds, virtualizes and checks the component
(imports: `types`, `telemetry`, `data-model-host`) and publishes it into the
dev package cache. The floppy body ([`bots/floppy/floppy.scx`](../../bots/floppy/floppy.scx))
uses `attention`, `verdict` and `stimulus`; autonomy, intuition and reasoning
use `verdict`; the identity ability
([`abilities/identity/identity.scx`](../../abilities/identity/identity.scx)) uses `recognize`.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="bot-1"></a>**bot-1**. Each element (attention, verdict, stimulus, recognize) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/bot.xsd`](package/bot.xsd), rejects it before any run under `scx validate`).
- <a id="bot-2"></a>**bot-2**. `<bot:stimulus>` writes the envelope {event, data, source, target?, id?}: event a non-empty string, source null when it evaluates to null, undefined or the empty string, and target and id included when given and not undefined; an empty event raises `error.execution` and assigns nothing.
- <a id="bot-3"></a>**bot-3**. `<bot:attention>` writes the focus (the given focus when it is a configured device, else null) as the first candidate, then the stimulus's device (a bot.input's data.target_device, which the environment stamps on a device's input) when it is configured and not a candidate already; devices that are not an array raise `error.execution` and assign nothing.
- <a id="bot-4"></a>**bot-4**. `<bot:verdict>` writes the selections without bot.ability.cognition.ignore ones, and the original's message for the first one the attention policy refuses (a focus_device outside the candidates or with invalid data, a clear_focus for another target or with no focus), else null.
- <a id="bot-5"></a>**bot-5**. `<bot:recognize>` writes true exactly when the name's case-folded letter-and-digit tokens appear as a whole run among the text's; a missing name or one with no tokens is never heard, and a text that is neither a string nor null raises `error.execution` and assigns nothing.
