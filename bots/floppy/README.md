# floppy

The floppy bot is the port of mosfet's Bot: an SCXML machine,
[`floppy.scx`](floppy.scx), whose abilities are invoked children under
[`../../abilities/`](../../abilities/). Phase 4 is in progress: the body runs
its lifecycle and processing turns, and its one ability is the
[cognition](../../abilities/cognition.scx), mosfet's cascade of stages
invoked per turn: [autonomy](../../abilities/autonomy.scx) (a seam until
phase 5: no behaviors), [intuition](../../abilities/intuition.scx) (the
decision service picks ignore or hands the turn on, with mosfet's
self-tuning confidence gate), and [reasoning](../../abilities/reasoning.scx)
(one structured model call over the turn's choices; memory is a seam until
phase 3). Each turn the body assembles the choices (its own
`bot.focus_device`, pinned to the turn's focus candidates, and
`bot.clear_focus` while a device is focused, plus each ability's latest
`bot.ability.offers`, tagged with the ability as `target`, compacted and
held to 32 KiB) and routes each selected event of the turn's output to the
body itself or to the ability that offered it. The full Contract, Topology, Bindings, and Proof sections come
with the phase's saved live examples.

## Running

Activate it, then give it inputs, on standard input:

```console
# The locally built packages (the dev cache under target/).
printf '%s\n' '{"name":"bot.activate","data":{}}' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --offline --cache-dir target/components/package-cache --env-file .env bots/floppy/floppy.scx
# floppy's latest release, once one is published (phase 7).
printf '%s\n' '{"name":"bot.activate","data":{}}' \
  '{"name":"bot.input","data":{"observation":{"event":"text","data":"hello"}}}' |
  scx run --env-file .env bots/floppy/floppy.scx
```

The env file must set `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (the
reasoning's `generate` service has no default backend, decision D31) and
`TYPESAFE_API_KEY` (the intuition's `decision` service; `TYPESAFE_BASE_URL`
is optional, `https://api.typesafe.ai` without it). Without the TypeSafe key
every turn fails with `the intuition failed: the decision failed
(unconfigured)`; without the model endpoint a turn that reaches reasoning
fails with `the reasoning failed (unconfigured)`.
`bot.activate` may name the model (`{"model": "..."}`); the default is
`Qwen/Qwen3.8-Flash-Next`. The services import no WASI resources, so the run
needs no grants.

The root has no parent, so each event the body emits is a `log` record
labelled with its name, its text the event's data as JSON: `bot.activated`,
`bot.activating.failed`, `bot.deactivated`, `bot.processing.completed`,
`bot.processing.failed`, and `bot.output` (the turn's selected events).
`bot.choices` lists each turn's choices (`event` or `event@target`),
`bot.route` each routed selection, `bot.route.dropped` each selection the
turn did not offer or no transition took, and `bot.focus` each focus
change. No ability offers `bot.ability.communication.respond` until the
communication ability lands, so the bot does not speak yet.

## Proof

- `cargo test -p floppy-tests --test body`: the lifecycle and turn matrix
  against a cognition double (`tests/fixtures/phase4/cognition_double.scx`),
  and the choices and routing (the offer union, focus grounding, dropped
  selections, ignore, the 32 KiB budget).
- `cargo test -p floppy-tests --test cognition`: the cascade against fake
  decision and model backends (each path, the tuner, failures, hop timeouts
  and reboot requests, cancels).
- `cargo test -p floppy-tests --test contracts`: every event reference and
  cross-document send of the body and its abilities.
- `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
  floppy_live -- --ignored live_cascade`: one live question turn that
  escalates to reasoning, whose selection the body routes.
- `... -- --ignored live_choices`: one live turn on a bot with a lamp: the
  choices are the body's focus (pinned to the lamp) and the cognition's
  offer, and every selection is routed.
