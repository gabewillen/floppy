# Live proof

Each row is one real run of an ignored live test. Keys came only from the
env file named by `NEMOBOT_LIVE_ENV_FILE`
(`mosfet/mosfet.py/.env`). No value was printed, logged, or committed.
Each variable was granted only to its own package: `OPENAI_*` to `generate`,
and `TYPESAFE_*` to `decision`.

## 2026-10-06, model calls as invoked services, model `gpt-6-luna`

The runs used nemobot commit `d5728e9b` (code at `813d57c8`): `generate`
and `decision` are invoked services (`services/generate`,
`services/decision`) whose requests run off the session thread, the
builder invokes them with `<finalize>`/`done.invoke` and a preempting
`builder.timeout`, and the subtree is scxml.rs `5eb077c4`. `OPENAI_*` was
granted to the generate service and `TYPESAFE_*` to the decision service.

| Test | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `builder_bot live_builder_through_an_env_file` | `gpt-6-luna` (generate service) | pass | 1 of 3 | `builder.done` on attempt 1 of 4: 0 failures, 0 machine fixes, 0 warnings, no triage decision (so the decision service did not run inside the builder); plan `pedestrian_traffic_light` (5 states); both of its tests (`normal_cycle`, `button_ends_green_early`) passed in simulation; the built document compiled again outside the builder (`compile_source`, no packages) |
| `generate_service live_generate_through_an_env_file` | `gpt-6-luna` | pass | 1 | strict structured output through `generate.result` |
| `decision_service live_decision_through_an_env_file` | TypeSafe `system_one` (no OpenAI model) | pass (`shipped`) | 1 | `decision:menu` menu, the service's emitted `order.ship` with schema-valid data |

The builder's output was written to `target/builder-live/` and is not
committed. Nothing in the tests was changed to make them pass.

## 2026-10-06, model `gpt-6-luna`

The runs used the working tree of nemobot commit `57954078`. That commit
switches the defaults to `gpt-6-luna`, and it sits on top of the env-scoping
commit `225c0ac1`. The builder topology (`bots/builder/builder.scxml`) is
unchanged except for its default model.

| Test | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `builder_bot live_builder_through_an_env_file` | `gpt-6-luna` (`generate`), TypeSafe `system_one` (`decision` triage) | pass | 1 of 3 | `builder.done` on attempt 3 of 4, after 2 failures, 0 machine fixes, and triage decisions `triage.repair` twice |
| `generate_namespace live_generate_through_an_env_file` | `gpt-6-luna` | pass | 1 | strict structured output |
| `decision_namespace live_choose_through_an_env_file` | TypeSafe `system_one` (no OpenAI model) | pass (`shipped`) | 1 | |

### The builder run

The spec was "a traffic light with a pedestrian button", with two
constraints: send every light change and walk signal to the invoker, and
let a button press during green end green early. These are the attempts
the builder recorded in its report:

1. Simulation failed (`simulate` / `test-failed`). Tests
   `button_runs_full_cycle` and `button_during_yellow_is_ignored` saw the
   outputs stop after `light.yellow`. Triage chose `triage.repair`.
2. Validation failed (`validate` / `SCXML070`, three times): a `<send>` to
   `#_internal` cannot use `delay`. Triage chose `triage.repair`.
3. The draft passed `sx:validate` and all three simulation tests, so the
   builder sent `builder.done`.

The test then compiled the produced document again outside the builder
(`compile_source`, no packages). `scxml-cli validate --offline` also reports
0 errors, 0 warnings, and 0 notes. The bot is saved, exactly as built, in
[`bots/builder/examples/traffic_light_luna`](../bots/builder/examples/traffic_light_luna).

It is model output, kept as built. Two limits are worth knowing:

- Green has no timer. The light stays green until the button is pressed,
  because the spec gives no phase durations and the tests do not ask for
  one.
- A simulation passes when the expected outputs appear in order, so extra
  outputs are allowed (see the builder's Gaps).

Unlike `examples/traffic_light`, the earlier build, every timed phase
cancels its own timer in `onexit`.

No builder defect needed fixing: the run passed within its first live
attempt budget. Nothing in the test was changed to make it pass.
