# behavior namespace

`https://github.com/stateforward/floppy/namespaces/behavior` (prefix
`behavior` by convention): a Wasm executable-content extension component
(`floppy_behavior`) for learned behaviors: the behavior inventory rows of a
memory select read as instances, the statements that replace one
behavior's row and triggers, the schedule bounds a behavior's lifetime
implies, the check of a persistent routine's forced tick, and the check of
a turn behavior's run on its live turn. The memory ability applies the
statements through the sql service and the builder simulates the routine;
these elements only build and read values. They never send, and they do
no I/O.

| Element | Writes |
| --- | --- |
| `<behavior:instances_from_behavior_results location behaviors triggers>` | the behavior instances, each with its triggers |
| `<behavior:replace_behavior_clauses location instance>` | the statements `[{sql, parameters}]` that delete the behavior's triggers and row and insert its replacement |
| `<behavior:lifetime_errors location scxml min_every? turn_after_bound? lifetimelocation? scheduleslocation? ticklocation?>` | the E0009 diagnostics (`location`), the lifetime (`lifetimelocation`), the schedules' events (`scheduleslocation`) and the forced tick's copy (`ticklocation`, decision D153) |
| `<behavior:verify_tick location ticks>` | the E0008 messages of a persistent routine's forced tick |
| `<behavior:verify_apply location done turn input targets error? choices?>` | the E0008 messages of a turn behavior's one run on its live turn, its selections' data checked against the live values and, with the turn's choices, against each choice's schema |

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
`verify_apply` (decision D131). The next-run time of an `at` schedule stays in the reflection's
ECMAScript, which a routine carries as data: a builder draft that used a
namespace element would fail `<scx:validate>`, which binds no extension
package.

## Statements

The contract above as numbered statements, each tested by the namespace's
own implementation report plan ([`irp/`](irp/README.md)), in the format and
through the pipeline of scxml.rs's namespace suites.

- <a id="behavior-1"></a>**behavior-1**. Each element (instances_from_behavior_results, replace_behavior_clauses, lifetime_errors, verify_tick, verify_apply) takes exactly its declared attributes, the required ones present and none empty, and no content, and the namespace has no other element: a malformed element raises `error.execution` and assigns nothing (its schema, [`package/behavior.xsd`](package/behavior.xsd), rejects it before any run under scx validate).
- <a id="behavior-2"></a>**behavior-2**. `<behavior:instances_from_behavior_results>` writes one instance per behavior row, in row order: every column as a member, examples parsed from its JSON text, used_count and failed_count as numbers, and triggers the second value of each trigger row whose first value is the instance's name; a result without rows raises `error.execution` and assigns nothing.
- <a id="behavior-3"></a>**behavior-3**. `<behavior:replace_behavior_clauses>` writes the statements that delete the behavior's triggers and row and insert its row and each of its distinct triggers, in that order, the row's absent fields an instance's defaults (examples [], lifetime turn, status ACTIVE, counters 0); an instance without a name raises `error.execution` and assigns nothing.
- <a id="behavior-4"></a>**behavior-4**. `<behavior:lifetime_errors>` writes E0009 diagnostics of an SCXML document read as XML: a schedule (a delayed send of an event a transition sending bot.behavior.tick takes) shorter than min_every, a turn behavior's other delayed send longer than turn_after_bound, a schedule with a literal target (decision D147), a schedule whose send carries event:schema (decision D152), and a persistent routine (one that sends bot.behavior.tick) with no schedule, none when neither bound is given; lifetimelocation receives the lifetime, scheduleslocation the schedules' events, each once, and ticklocation the forced tick's copy (each schedule's delay or delayexpr replaced by a delay of 1500ms, every other byte kept, decision D153); a text that does not parse has none, is a turn behavior, and has no copy (null).
- <a id="behavior-5"></a>**behavior-5**. `<behavior:verify_tick>` writes E0008 messages for a persistent routine's ticks: none when the first carries name, trigger (every or at), scheduled_at and fired_at and its output selects at least one event whose data agrees with those fields, else why; ticks that are not an array raise `error.execution` and assign nothing.
- <a id="behavior-6"></a>**behavior-6**. `<behavior:verify_apply>` writes E0008 messages for a turn behavior's run on the live turn (for the run, the donedata contract, nothing selected, the targets, and selection data that disagrees with input; decision D131): none when its donedata is `{turn, handled, selected}` with the live turn, handled true, and at least one selection, each with an event, a target among `targets`, and data agreeing with input's scalar members; else why (an `error`, or no donedata, first). With `choices` (the turn's choices, `[{event, target, schema}]`; decision D206), each selection must also be an event its target takes on that turn, with data the choice's schema admits, as the candidates runner's dispatch is checked against the receiving transition. An input that is not an object, targets that are not strings, or choices that are not objects raise `error.execution` and assign nothing.
