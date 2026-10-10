# floppy

The floppy bot is the port of mosfet's Bot: an SCXML machine,
[`floppy.scx`](floppy.scx), whose abilities are invoked children under
[`../../abilities/`](../../abilities/). Phase 4 is in progress: the body runs
its lifecycle and processing turns, and its abilities are the
[cognition](../../abilities/cognition/cognition.scx), mosfet's cascade of stages
invoked per turn: [autonomy](../../abilities/autonomy/autonomy.scx) (a seam until
phase 5: no behaviors), [intuition](../../abilities/intuition/intuition.scx) (the
decision service picks ignore or hands the turn on, with mosfet's
self-tuning confidence gate), and [reasoning](../../abilities/reasoning/reasoning.scx)
(one structured model call over the turn's choices; memory is a seam until
phase 3), and the [communication](../../abilities/communication/communication.scx),
which holds the [conversation](../../abilities/conversation/conversation.scx) (the
committed text history, one transcript per participant set; memory recall
is a seam until phase 3, and inferring participants and relationships from
voice embeddings, mosfet's turn detectors, is phase 6) and
[speaking](../../abilities/speaking/speaking.scx) (text mode: the words are logged
as `bot.say`; speech synthesis and playout are phase 6). Each turn the body
assembles the choices (its own
`bot.focus_device`, pinned to the turn's focus candidates, and
`bot.clear_focus` while a device is focused, plus each ability's latest
`bot.ability.offers`, tagged with the ability as `target`, compacted and
held to 32 KiB) and routes each selected event of the turn's output to the
body itself or to the ability that offered it: a selected
`bot.ability.communication.respond` is said through speaking and recorded
in the conversation. Input addressed to the bot as
`bot.ability.communication.input` is admitted into the conversation, whose
updated history is the next turn's stimulus. The full Contract, Topology, Bindings, and Proof sections come
with the phase's saved live examples.

## Running

Activate it, then give it inputs, on standard input:

```console
# The locally built packages (the dev cache under target/).
printf '%s\n' '{"name":"bot.activate","data":{}}' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory bots/floppy/floppy.scx
# A conversation: text addressed to the bot, answered through bot.say.
printf '%s\n' '{"name":"bot.activate","data":{}}' \
  '{"name":"bot.ability.communication.input","data":{"source_ids":["gabe"],"target_ids":["floppy"],"content":"Hi floppy!","content_type":"text/plain"}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory bots/floppy/floppy.scx
# floppy's latest release, once one is published (phase 7).
printf '%s\n' '{"name":"bot.activate","data":{}}' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --env-file .env --dir https://github.com/stateforward/floppy/services/sql=memory::/memory bots/floppy/floppy.scx
```

The env file must set `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (the
reasoning's `generate` service has no default backend, decision D31) and
`TYPESAFE_API_KEY` (the intuition's `decision` service; `TYPESAFE_BASE_URL`
is optional, `https://api.typesafe.ai` without it). Without the TypeSafe key
every turn fails with `the intuition failed: the decision failed
(unconfigured)`; without the model endpoint a turn that reaches reasoning
fails with `the reasoning failed (unconfigured)`.
`bot.activate` may name the model (`{"model": "..."}`); the default is
`Qwen/Qwen3.8-Flash-Next`. The memory ability (`abilities/memory.scx`, mosfet's
Memory) keeps the bot's database, `/memory/floppy.sqlite`, through the sql
service (`services/sql`, SQLite), in the directory the run grants that
service: `--dir
https://github.com/stateforward/floppy/services/sql=HOST::/memory`
preopens the host directory `HOST` (here `memory/`, created first with
`mkdir -p memory`) at `/memory`, and the run is refused without it. The
other services import no WASI resources.

The root has no parent, so each event the body emits is a `log` record
labelled with its name, its text the event's data as JSON: `bot.activated`,
`bot.activating.failed`, `bot.deactivated`, `bot.processing.completed`,
`bot.processing.failed`, `bot.output` (the turn's selected events), and,
from the communication, `bot.ability.speaking.output` (what was said) and
`bot.ability.communication.failed`; speaking logs the words as `bot.say`.
`bot.choices` lists each turn's choices (`event` or `event@target`),
`bot.route` each routed selection, `bot.route.dropped` each selection the
turn did not offer or no transition took, and `bot.focus` each focus
change.

## Proof

- `cargo test -p floppy-tests --test body`: the lifecycle and turn matrix
  against a cognition double (`tests/fixtures/phase4/cognition_double.scx`),
  and the choices and routing (the offer union, focus grounding, dropped
  selections, ignore, the 32 KiB budget).
- `cargo test -p floppy-tests --test cognition`: the cascade against fake
  decision and model backends (each path, the tuner, failures, hop timeouts
  and reboot requests, cancels).
- `cargo test -p floppy-tests --test communication`: the communication
  path against the cognition double (a routed response said and recorded,
  conversation input as a turn whose stimulus carries the bounded history,
  a blank response, a response deferred behind one in flight and the hop
  timeout, malformed input, a child that cannot start).
- `cargo test -p floppy-tests --test cognition` also runs the whole text
  path on fake backends: "say hi" escalates to reasoning, whose activate
  and respond the body routes to the communication, which says it.
- `cargo test -p floppy-tests --test contracts`: every event reference and
  cross-document send of the body and its abilities.
- `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
  floppy_live -- --ignored live_cascade`: one live question turn that
  escalates to reasoning, whose selection the body routes.
- `... -- --ignored live_choices`: one live turn on a bot with a lamp: the
  choices are the body's focus (pinned to the lamp) and the cognition's and
  communication's offers, and every selection is routed.
- `... -- --ignored live_chat`: one live text turn through
  `bot.ability.communication.input`: the conversation's product is the
  stimulus, the reply is selected as `bot.ability.communication.respond`,
  said (`bot.say`), and recorded in the conversation.
