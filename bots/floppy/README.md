# floppy

The floppy bot is the port of mosfet's Bot: an SCXML machine,
[`floppy.scx`](floppy.scx), whose abilities are invoked children under
[`../../abilities/`](../../abilities/). Phase 4 is in progress: the body runs
its lifecycle and processing turns, and its one ability is a thin
[cognition](../../abilities/cognition.scx) that answers each turn with one
model call. The full Contract, Topology, Bindings, and Proof sections come
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

The env file must set `FLOPPY_BASE_URL` and `FLOPPY_API_KEY`: the
cognition's `generate` service has no default backend (decision D31), and
without them every turn fails with `the generation failed (unconfigured)`.
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
- `cargo test -p floppy-tests --test contracts`: every event reference and
  cross-document send of the body and its abilities.
- `FLOPPY_LIVE_ENV_FILE=<paths> cargo test -p floppy-tests --test
  floppy_live -- --ignored live_text_turn`: one live text turn.
