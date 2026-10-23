# behavior namespace

`https://github.com/stateforward/floppy/namespaces/behavior` (prefix
`behavior` by convention): the original's behavior package (`behavior/`:
`storage.py`, `instance.py`, `verify.py`) as a Wasm executable-content
extension component (`floppy_behavior`): the behavior inventory rows of a
memory select read as instances, the statements that replace one
behavior's row and triggers, the schedule bounds a behavior's lifetime
implies, the check of a persistent routine's forced tick, the check of a
turn behavior's run on its live turn, and the one program shape of a turn
behavior. The memory ability applies the
statements through the sql service and the builder simulates the routine;
these elements only build and read values. They never send, and they do
no I/O.

| Element | The original | Writes |
| --- | --- | --- |
| `<behavior:instances_from_behavior_results location behaviors triggers>` | `storage.instances_from_behavior_results` | the behavior instances, each with its triggers |
| `<behavior:replace_behavior_clauses location instance>` | `storage.replace_behavior_clauses` (`delete_behavior_clauses`, `insert_behavior_clauses`) | the statements `[{sql, parameters}]` that replace the behavior |
| `<behavior:lifetime_errors location scxml min_every? turn_after_bound? lifetimelocation? scheduleslocation?>` | `instance._lifetime_errors` (E0009), the instance's `lifetime` | the diagnostics (`location`), the lifetime (`lifetimelocation`) and the schedules' events (`scheduleslocation`) |
| `<behavior:verify_tick location ticks>` | `verify._verify_tick`, `_selection_binding_errors` (E0008) | the forced tick's messages |
| `<behavior:verify_apply location done turn input targets handled? error?>` | the live-turn check of a turn behavior (one run, its selections' data checked against the live values, E0008) | the messages of a turn behavior's run on its live turn or on an input it must not answer |
| `<behavior:turn_program location script>` | the one program shape of a behavior | the turn behavior template ([`templates/turn.scx`](templates/turn.scx)) naming the behavior's script |

The exact semantics and failures are in [`src/lib.rs`](src/lib.rs); the
structure is in [`package/behavior.xsd`](package/behavior.xsd), whose digest the
manifest [`package/behavior.extension.json`](package/behavior.extension.json)
pins. `scripts/build-components.sh` builds, virtualizes and checks the
component (imports: `types`, `telemetry`, `data-model-host`) and publishes
it into the dev package cache. The reflection
([`abilities/reflection/reflection.scx`](../../abilities/reflection/reflection.scx))
reads its inventory with `instances_from_behavior_results`, stores a
create's draft and an accepted change with `replace_behavior_clauses` and
reads the written document's lifetime with `lifetime_errors`; the builder
([`bots/builder/builder.scx`](../../bots/builder/builder.scx)) checks a
draft with `lifetime_errors`, a routine's forced tick with
`verify_tick`, and a turn behavior's run on its live turn with
`verify_apply` (decision D131), and writes a turn behavior's chart with
`turn_program`, so the model writes only its script (decision D132). The
next-run time of an `at` schedule stays in the reflection's
ECMAScript, which a routine carries as data: a builder draft that used a
namespace element would fail `<scx:validate>`, which binds no extension
package.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="behavior-1"></a>**behavior-1**. Each element (instances_from_behavior_results, replace_behavior_clauses, lifetime_errors, verify_tick, verify_apply, turn_program) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/behavior.xsd`](package/behavior.xsd), rejects it before any run under scx validate).
- <a id="behavior-2"></a>**behavior-2**. `<behavior:instances_from_behavior_results>` writes one instance per behavior row, in row order: every column as a member, examples parsed from its JSON text, used_count and failed_count as numbers, and triggers the second value of each trigger row whose first value is the instance's name; a result without rows raises `error.execution` and assigns nothing.
- <a id="behavior-3"></a>**behavior-3**. `<behavior:replace_behavior_clauses>` writes the statements that delete the behavior's triggers and row and insert its row and each of its distinct triggers, in that order, the row's absent fields the original Instance's defaults (examples [], lifetime turn, status ACTIVE, counters 0); an instance without a name raises `error.execution` and assigns nothing.
- <a id="behavior-4"></a>**behavior-4**. `<behavior:lifetime_errors>` writes the original's E0009 diagnostics of an SCXML document read as XML: a schedule (a delayed send of an event a transition sending bot.behavior.tick takes) shorter than min_every, a turn behavior's other delayed send longer than turn_after_bound, and a persistent routine (one that sends bot.behavior.tick) with no schedule, none when neither bound is given; lifetimelocation receives the lifetime and scheduleslocation the schedules' events, each once; a text that does not parse has none and is a turn behavior.
- <a id="behavior-5"></a>**behavior-5**. `<behavior:verify_tick>` writes the original's E0008 messages for a persistent routine's ticks: none when the first carries name, trigger (every or at), scheduled_at and fired_at and its output selects at least one event whose data agrees with those fields, else why; ticks that are not an array raise `error.execution` and assign nothing.
- <a id="behavior-6"></a>**behavior-6**. `<behavior:verify_apply>` writes E0008 messages for a turn behavior's run on the live turn (for the run, the donedata contract, nothing selected, the targets, and selection data that disagrees with input; decision D131): none when its donedata is `{turn, handled, selected}` with the live turn, handled true, and at least one selection, each with an event, a target among `targets`, and data agreeing with input's scalar members; else why (an `error`, or no donedata, first); a donedata whose `error` is a string (the turn template's: `script` when the script did not load, else the script's call that threw) writes only that, and with `handled` false the run must finish with handled false and nothing selected; an input that is not an object or targets that are not strings raise `error.execution` and assign nothing.
- <a id="behavior-7"></a>**behavior-7**. `<behavior:turn_program>` writes the turn behavior template, [`templates/turn.scx`](templates/turn.scx), with its one `<script src="turn.js"/>` naming the script instead; a script that is not a plain `.js` file name (letters, digits, `_` and `-`, then `.js`) raises `error.execution` and assigns nothing.
