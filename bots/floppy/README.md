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
(one structured model call over the offered events; memory is a seam until
phase 3). The full Contract, Topology, Bindings, and Proof sections come
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

## Proof

- `cargo test -p floppy-tests --test body`: the lifecycle and turn matrix
  against a cognition double (`tests/fixtures/phase4/cognition_double.scx`).
- `cargo test -p floppy-tests --test cognition`: the cascade against fake
  decision and model backends (each path, the tuner, failures, hop timeouts
  and reboot requests, cancels).
- `cargo test -p floppy-tests --test contracts`: every event reference and
  cross-document send of the body and its abilities.
- `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
  floppy_live -- --ignored live_cascade`: one live question turn that
  escalates to reasoning and is answered.
