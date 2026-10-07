# Live proof

Each row is one real run of an ignored live test. Keys came only from the
env file named by `NEMOBOT_LIVE_ENV_FILE`
(`mosfet/mosfet.py/.env`). No value was printed, logged, or committed.
Each variable was granted only to its own package: `OPENAI_*` to `generate`,
and `TYPESAFE_*` to `decision`.

## 2026-10-06, tagless URIs, the draft profile, and the base URL default (decisions D18, D19, D20), model `gpt-6-luna`

The runs used nemobot commit `05157554` (subtree scxml.rs `73f340b7`):
`sh scripts/build-components.sh` publishing the decision namespace and both
services into the dev cache `target/components/package-cache` (pinned under
their tagless URIs to release tag `v0.1.0` at that commit; `scx`'s release
cache untouched), and `scx` built from the subtree with those session
components embedded (`SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml"
CARGO_TARGET_DIR=target/scx-live cargo build --release --locked -p
scxml-cli`). The mosfet env file sets no `TYPESAFE_BASE_URL`; the decision
service used its package's declared origin `https://api.typesafe.ai`
(decision D18), so the builder's triage went to the decision service, not
the fallback. Drafts were checked with `<scx:validate profile="draft">`
(decision D20). The builder command, from a clean shell
(`env -i HOME="$HOME" PATH="/usr/bin:/bin"`), in the nemobot root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4,"model":"gpt-6-luna"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file /Users/gabrielwillen/VSCode/stateforward/mosfet/mosfet.py/.env bots/builder/builder.scx
```

The consent listing named each package with `release v0.1.0 (commit
05157554...)`, the generate service's origin `https://api.openai.com/v1`, and
the decision service's `https://api.typesafe.ai`.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| builder, run 1 | `gpt-6-luna` | pass | 1 | exit 0 in 151 s; attempt 1 of 4, 0 failures, no triage, so the decision service did not run |
| builder, run 2 | `gpt-6-luna` | pass | 1 | exit 0 in 100 s; attempt 1 of 4, no triage |
| builder, run 3 | `gpt-6-luna` (generate), TypeSafe `system_one` (decision) | pass | 1 | exit 0 in 133 s; attempt 2 of 4 after 1 failure (`validate` / `SCXML070`), triage decided by the decision service: `decisions` = `["triage.repair"]` (no `fallback:*`); plan `pedestrian_traffic_light`; all five of its tests passed in simulation; progress `log`s `design`, `author`, `validate`, `retry_gate`, `triage`, `author`, `validate`, `simulate`, `done`; the built document compiled again with `scx compile` (no packages) |
| `generate_service live_generate_through_an_env_file` | `gpt-6-luna` | pass | 1 | the env file's `OPENAI_BASE_URL`; no shell default |
| `decision_service live_decision_through_an_env_file` | TypeSafe `system_one` | pass (`shipped`) | 1 | no `TYPESAFE_BASE_URL` anywhere: the package's declared origin |

Runs 1 and 2 are listed because they were run: a draft that passes on its
first attempt never reaches triage, so run 3 was the first to exercise the
decision service inside the builder. The builder's output was written to
`target/builder-live-cli/` and is not committed. Nothing in the tests was
changed to make them pass.

## 2026-10-06, `scx run` straight from the document (decisions D16, D17), model `gpt-6-luna`

The runs used nemobot commit `7b0b0b8b` (subtree scxml.rs `50283481`):
`scx` installed from the subtree with its session components and the `scx`
namespace embedded (`SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml"
cargo install --locked --path scxml.rs/crates/scxml-cli`), and
`sh scripts/build-components.sh` publishing the decision namespace and both
services into `scx`'s default package cache. The builder's `env` attributes
scoped `OPENAI_*` to the generate service's `<invoke>`s and `TYPESAFE_*` to
the decision service's; the consent listing showed exactly that (names
only). The mosfet env file sets no `TYPESAFE_BASE_URL`, so in the builder
run the decision service's `baseurl` slot was absent and the builder used
its fallback triage. The builder command, from a clean shell
(`env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin"`), in the
nemobot root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4,"model":"gpt-6-luna"}}' |
  scx run --env-file /Users/gabrielwillen/VSCode/stateforward/mosfet/mosfet.py/.env bots/builder/builder.scx
```

The live tests: `NEMOBOT_LIVE_ENV_FILE=<mosfet .env> NEMOBOT_LIVE_MODEL=gpt-6-luna
cargo test -p nemobot-tests --test <test> -- --ignored --nocapture`.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `scx run --env-file <mosfet .env> bots/builder/builder.scx` | `gpt-6-luna` (generate service); decision unconfigured (no `TYPESAFE_BASE_URL`) | pass | 1 of 1 | exit 0 in 116 s; the `done` record's data: attempt 2 of 4 after 1 failure (`validate` / `SCXML070`), 0 machine fixes, triage `fallback:unavailable`, plan `pedestrian_traffic_light`, all four of its tests passed in simulation; progress `log`s `design`, `author`, `validate`, `retry_gate`, `triage`, `author`, `validate`, `simulate`, `done`; the built document compiled again with `scx compile` (no packages) |
| `generate_service live_generate_through_an_env_file` | `gpt-6-luna` | pass | 1 | strict structured output through `generate.result` |
| `decision_service live_decision_through_an_env_file` | TypeSafe `system_one` (no OpenAI model) | pass (`shipped`) | 1 | the consent check is strict again: the declared origin `https://api.typesafe.ai` is listed though the run's base URL equals it |

The builder's output was written to `target/builder-live-cli/` and is not
committed. Nothing in the tests was changed to make them pass.

## 2026-10-06, WASI 0.3 async services (decision D15), model `gpt-6-luna`

The runs used nemobot commit `3b5c51b3` plus the decision live test's
consent check below (subtree scxml.rs `aa12fa6d`): `services/generate` and
`services/decision` are each one async `invoke` export that awaits
`outbound-http.fetch`, on wasmtime 49 with the Component Model async ABI,
through `scxml-cli run`. `OPENAI_*` was granted to the generate service and
`TYPESAFE_*` to the decision service. Commands:
`NEMOBOT_LIVE_ENV_FILE=<mosfet .env> NEMOBOT_LIVE_MODEL=gpt-6-luna cargo test
-p nemobot-tests --test <test> -- --ignored <name>`.

| Test | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `generate_service live_generate_through_an_env_file` | `gpt-6-luna` | pass | 1 | strict structured output through `generate.result`, from a streamed reply |
| `decision_service live_decision_through_an_env_file` | TypeSafe `system_one` (no OpenAI model) | pass (`shipped`) | 2 | the service's returned `order.ship` with schema-valid data. The first run failed only the test's consent check: the env file's `TYPESAFE_BASE_URL` equals the package origin, so scxml-cli masks it in the listing (`outbound HTTP: [masked]`); the check now accepts that when the run's base URL is the origin. The second run, with `--nocapture`, printed the outcome |
| `builder_bot live_builder_through_an_env_file` | `gpt-6-luna` (generate service), TypeSafe `system_one` (decision triage) | pass | 1 of 3 | `builder.done` on attempt 1 of 4: 0 failures, 0 machine fixes, no triage decision (so the decision service did not run inside the builder); plan `traffic_light`; 124 s |

The builder's output was written to `target/builder-live/` and is not
committed. Apart from the consent check above, nothing in the tests was
changed to make them pass.

## 2026-10-06, the builder through `scxml-cli run`, model `gpt-6-luna`

nemobot has no runtime of its own since decision D14, so this run went
through scxml.rs's `scxml-cli run` (subtree scxml.rs `e7d2fd7a`, nemobot
commit `5dc040cc`), exactly the builder README's command:
`scxml-cli run @target/test-components/components.args
@bots/builder/builder.args --clock wall --env-file <NEMOBOT_LIVE_ENV_FILE>
bots/builder/parent.scxml`, with `OPENAI_BASE_URL` and `TYPESAFE_BASE_URL`
defaults in the calling environment (the env file's values win). The
`--env` scopes of `builder.args` granted `OPENAI_*` to the generate service
and `TYPESAFE_*` to the decision service. Command:
`NEMOBOT_LIVE_ENV_FILE=<mosfet .env> NEMOBOT_LIVE_MODEL=gpt-6-luna cargo test
-p nemobot-tests --test builder_bot -- --ignored live_builder_through_an_env_file`.

| Test | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `builder_bot live_builder_through_an_env_file` | `gpt-6-luna` (generate service), TypeSafe `system_one` (decision triage) | pass | 1 of 3 | `builder.done` on attempt 2 of 4 after 1 failure (`validate`: `XSD-cvc-complex-type.2.4` twice and `SCXML070`), 0 machine fixes, 0 warnings, triage decision `triage.repair`; plan `traffic_light` (7 states); all four of its tests passed in simulation, each in its own invocation; the built document compiled again outside the builder (`scxml-cli compile`, no packages); 189 s |

The builder's output was written to `target/builder-live/` and is not
committed. Nothing in the tests was changed to make them pass.

## 2026-10-06, streamed and cancellable model calls, model `gpt-6-luna`

The runs used nemobot commit `847e7960` (subtree scxml.rs `14c4b8e9`):
`services/generate` streams its Chat Completions request (`"stream": true`)
and joins the chunks before it validates, and the host's
`ThreadedServiceHttp` closes a cancelled invocation's connection.
`OPENAI_*` was granted to the generate service and `TYPESAFE_*` to the
decision service.

| Test | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `generate_service live_generate_through_an_env_file` | `gpt-6-luna` | pass | 1 | strict structured output through `generate.result`, now from a streamed reply |
| `builder_bot live_builder_through_an_env_file` | `gpt-6-luna` (generate service), TypeSafe `system_one` (decision triage) | pass | 1 of 3 | `builder.done` on attempt 3 of 4, after 2 failures (`validate` / `SCXML070`: a `<send>` to `#_internal` with `delay`; then `simulate` / `test-failed` in `button_ends_green_early`), 0 machine fixes, 0 warnings, triage decisions `triage.repair` twice; plan `pedestrian_traffic_light` (8 states); all four of its tests passed in simulation |

The builder's output was written to `target/builder-live/` and is not
committed. Nothing in the tests was changed to make them pass. The
cancellation itself is proven offline, against a backend that streams
forever (`generate_service leaving_the_invoking_state_closes_the_streaming_model_call`,
and scxml.rs's `service_invoke` and `live_run` tests); no live call was
cancelled.

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
