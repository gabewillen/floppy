# Live proof

Each row is one real run of an ignored live test or of a bot. Keys came only
from env files (named by `FLOPPY_LIVE_ENV_FILE` for the live tests, or given
to `scx run` as `--env-file`). No value was printed, logged, or committed.
Each variable was granted only to its own package by the document's `env`
attributes. Entries are dated and keep the names of their day: the product
was named nemobot before decision D23.

## 2026-10-12, `live_conversation` with the per-backend streaming switch and the builder's token bound (D101)

Streaming is set per backend (`FLOPPY_STREAM`, unset by default), an
inactivity bound arms only on a streamed reply's first progress, and the
builder's design and author calls carry `max_completion_tokens` 8192.
`cargo test -p floppy-tests --locked --test floppy_live -- --ignored
--test-threads=1 live_conversation`, transcripts read from
`target/floppy-live/live_conversation.txt`.

| Run | Backend | Streaming | Turns 1-4 | Result |
| --- | --- | --- | --- | --- |
| switch CPA | CPA gateway, `gabe-codex-1/gpt-6-luna` (`.env:.env.cpa`) | off (unset) | turn 2 created `ExactPingReply` ACTIVE; turn 3 "pong" from autonomy; turn 4 "Your favorite color is green." | pass (54 s); no call streamed, no bound fired |
| switch local 1 | vLLM, `Qwen/Qwen3.8-Flash-Next` (`.env` and `FLOPPY_STREAM=true`) | on | turn 1 reflection created a behavior for the favorite color and turn 3 one for ping/pong; every author generation (four) failed `truncated` at 8192 tokens, so both builds ended `no-progress`; turn 3 "pong" (reasoning), turn 4 "Your favorite color is green!" | fails the behavior assertion (1833 s); no stage hit an inactivity bound |
| switch local 2 | the same, through the logging relay | on | reflection selected nothing on turns 1 and 2; turn 3 "pong" (reasoning) and a create accepted ACTIVE (its author reply 14.7 thousand characters of reasoning and 1.7 thousand of content, under the bound); turn 4 "Your favorite color is green, Gabe!" | fails the behavior assertion (234 s: learned on turn 3, not 2); all 10 calls streamed, longest gap 2.9 s, longest wait for the first chunk 4.8 s |

On the vLLM the behavior assertion depends on the model's choices, as in
the runs below (local 2 and 3 failed it too). The token bound turned run 1's
author generations into failures of about 30 thousand characters of reasoning
each instead of replies that ran to the backstop; whether any of them would
have answered is not known (no relay ran then). The answered author replies
logged on this backend so far reasoned at most 14.7 thousand characters.

## 2026-10-12, `live_conversation` with streamed model calls bounded by inactivity (D101)

The runs used floppy with scxml.rs's `outbound-http.send` (subtree pull
`33ee9dc2`): reasoning, reflection's select and the builder's generations
stream their replies, and each stage's bound is one of inactivity with a
backstop. A local relay passed each response through as it arrived and
logged each chunk's arrival time.

On the CPA gateway (`gabe-codex-1/gpt-6-luna`, `.env:.env.cpa`), a streamed
request is answered with `: ping` keep-alive comments every 20 s and no
content (also without a schema, without `reasoning_effort`, with
`stream_options`, and for `gabe-codex-1/gpt-5.6-sol`); the one run there
(`stream 1`) counted those bytes as progress, so reasoning ran to the 240 s
backstop and the bot rebooted. The generate service now counts only `data:`
events as progress. The other runs used the maintainer's vLLM (`.env`
only, `Qwen/Qwen3.8-Flash-Next`), which streams at once.

| Run | Backend | Bounds | Turns 1-4 | Longest gap between chunks | Longest wait for the first chunk | Note |
| --- | --- | --- | --- | --- | --- | --- |
| stream 1 | CPA gateway | inactivity 60 s (counting bytes), stage backstop 240 s | turn 1 rebooted at the backstop | 20 s (pings only) | - | |
| local 1 | vLLM | stage 60 s / 240 s, revising 60 s / 600 s | empty outputs on turns 1-3; turn 3's create authored one draft that streamed for 555 s | 4.7 s | 4.0 s | the test's 10 min wait for reflection ran out first |
| local 2 | vLLM | the same | turn 3 "pong" (reasoning), turn 4 "Your favorite color is green!" | 4.3 s | 2.4 s | reflection selected nothing: fails the behavior assertion |
| local 3 | vLLM | stage 30 s / 240 s, revising 150 s / 600 s | turn 3 "pong", turn 4 "Your favorite color is green!"; the turn 2 create and turn 3 change ran to the 600 s revising backstop while still streaming, the turn 4 change was accepted ACTIVE | 6.2 s | 4.3 s | no stage timed out without progress; fails the behavior assertion (learned on turn 4, not 2) |

No reasoning stage hit its inactivity bound in the vLLM runs (33 calls).
The local model writes very long replies (an author generation of 814 KB
over 548 s), so its builds outlast fixed totals while never pausing; the
backstops were raised to 1200 s (a builder generation) and 1800 s (a
revision) after these runs, and `bot.activate` sets the stage's and the
revision's bounds per run.

## 2026-10-12, `live_conversation` learning the ping/pong behavior (D100, D101, D96), model `gabe-codex-1/gpt-6-luna`

The same relay, now logging each call's start and end. Two measuring runs
had both bounds raised to 600 s, to see the calls whole; the two final
runs used the shipped bounds (120 s). Times are from each run's first
model call.

| Run | Result | Reasoning calls | Revision (select's end to the last draft's end) | Turn 3 | Turn 4 |
| --- | --- | --- | --- | --- | --- |
| measure 1 | fail ("green") | 1.9 to 4.2 s, and 113.1 s on turn 1 | 38.6 s: plan 10.7 s, drafts 9.6, 8.7, 8.5 s | `ExactPingPong` answered "pong" | `ExactPingPong` answered "pong" again: it tested whether any inbound message in `messages` was "ping" |
| measure 2 | pass | 2.3 to 2.7 s | 26.9 s: plan 6.2 s, drafts 9.7, 10.6 s | `PingPongExactReply` answered "pong" | the behavior was unhandled; reasoning: "Your favorite color is green." |
| final 1 | pass | 2.2 to 16.4 s | 28.2 s: plan 7.1 s, drafts 10.6, 10.1 s | `ExactPingPong` answered "pong" | unhandled; reasoning: "Your favorite color is green." |
| final 2 | pass | 2.1 to 2.5 s | 17.4 s: plan 7.8 s, draft 9.4 s | `ExactPingPong` answered "pong" | unhandled; reasoning: "Your favorite color is green." |

Every run's reflection selected `bot.behavior.create` on turn 2 and stored
the behavior ACTIVE under `bot.ability.conversation.output` (its select
offered only create: the inventory was empty, D100), and autonomy ran it
on turn 3: the D96 contract held live. Measure 1's 113 s call would have
timed out the 30 s stage and rebooted the bot, one revision outlasted the
30 s bound and another came within 2 s of it (D101). After
measure 1 the turn behavior contract names `parent.data` as the message a
conversation turn answers (D96); both final runs' behaviors left turn 4
unhandled.

## 2026-10-12, `live_conversation` on `main` and on `fix/e2e-live-findings`, model `gabe-codex-1/gpt-6-luna`

Each run went through a local logging relay in front of the CPA gateway
(an extra env file set `FLOPPY_BASE_URL` to it; it logged request and
response bodies, never headers), so every reasoning and reflection prompt
was read. No speech backend was configured, so each reply's speaking
failed ("the backend is not configured"), as expected.

| Run | Commit | Turn 4 answer | Behavior learned by turn 2 | Episodes recalled by reflection | Note |
| --- | --- | --- | --- | --- | --- |
| main 1 | `2ddb8968` | "I don't know yet" | no (selected nothing every turn) | 1, 3, 5, 7 | turn 4's prompt held "I'll remember that your favorite color is green" twice in its episodes |
| main 2 | `2ddb8968` | "You told me your favorite color is green." | no (`bot.behavior.create` on turn 3, revision timed out at 30 s) | 1, 3, 5, 6 | |
| branch 1 | `80f85c4f` | none: reasoning timed out at 30 s and the body rebooted | no (turn 2 selected `bot.behavior.break` of an unknown `PingPong` and failed) | 0, 1, 1 | the failed reflection stored no episode, so turn 3 recalled only turn 1's; turn 4's reply, which came after the hop, said "I don't know your favorite color yet" because "the prior response only claimed green without evidence" |
| fixed 1 | D99 and D97 amended | "Your favorite color is green." | no (`bot.behavior.create`; the revision timed out at 30 s) | 0, 1, 2, 3 | test fails on the new behavior assertion |
| fixed 2 | D99 and D97 amended | "Your favorite color is green." | no (`bot.behavior.break` of an unknown `PingPong` on turns 2 and 3, both failed) | 0, 1, 2, 3 | the failed turns' episodes were kept; test fails on the new behavior assertion |

The earlier branch run's "I don't know yet" was not a regression of D96
to D98: `main` answers the same way. Reasoning's episodes (reflection's
since D97, the same shape as reasoning's before) carry only the bot's own
selections, and the conversation's history, which the conversation hands
reasoning in `messages`, rendered as `<value count="N" />` only (D99), so
the model saw its own claim "I'll remember that your favorite color is
green" and nothing the user said. With D99 both runs answered from the
history. Learning the ping/pong request stays model-dependent: reflection
selected nothing, selected a break of a behavior the inventory does not
hold, or selected a create whose revision outlasted `revising_timeout_ms`
(30 s), in every run here.

## 2026-10-12, the whole bot over four turns, model `gabe-codex-1/gpt-6-luna`

The runs used floppy commit `b8526d0c` (main) with wasi-sdk 25. Local relays
forwarded `http://127.0.0.1:8787/v1` to the CPA gateway,
`http://127.0.0.1:8788` to TypeSafe and `http://127.0.0.1:8789/v1beta` to
the Gemini API, each through the session's HTTPS proxy, which adds the keys;
one env file set `FLOPPY_BASE_URL`, `TYPESAFE_BASE_URL` and
`FLOPPY_SPEECH_BASE_URL` to the relays and each key to a placeholder
(`FLOPPY_LIVE_MODEL=gabe-codex-1/gpt-6-luna cargo test -p floppy-tests
--test floppy_live -- --ignored --test-threads=1`).

| Run | Result | Note |
| --- | --- | --- |
| `live_cascade` | pass | |
| `live_chat` | pass | |
| `live_identity` | pass | |
| `live_choices` | fail | intuition escalated for low confidence and handed out `bot.focus_device` as an environment action, and reasoning selected it again, so the output held it twice and the body focused the lamp twice (`floppy.focus` logged twice); the test expects one |
| `live_conversation` (new) | pass | 104 s; the four turns below |

`live_conversation` ran one bot session with every ability wired (memory on
SQLite, reflection with the builder, routines, speaking on Gemini):

1. "My favorite color is green. Please remember that." Reasoning answered
   and retained the turn; reflection selected nothing. Speaking failed with
   "Speech encoding timed out." (the 5 s `encoding_timeout_ms`), so this
   reply was not spoken.
2. "Whenever I say exactly 'ping', reply with exactly 'pong'." Reflection
   selected `bot.behavior.create`, the builder authored `PingPongReply`,
   and it was stored ACTIVE with the trigger `bot.ability.conversation.output`.
3. "ping". Autonomy matched `PingPongReply` and ran it, but it returned
   unhandled: the builder's source tests `stimulus.event ===
   'bot.ability.communication.input'` while the turn's stimulus is
   `bot.ability.conversation.output`. Reasoning answered "pong", and
   reflection then broke the behavior ("the inventory trigger is
   conversation output, and it has not been used"). The turn behavior
   contract had told the builder that `input` holds the stimulus;
   decision D96 makes it name the behavior's triggers and its actual
   input, after this run.
4. "What is my favorite color?" Reasoning recalled memory and answered
   "Your favorite color is green."

Each turn the listening heard the bot's own speech and failed at voice
detection (no LocalAI), as on 2026-10-11. Decision D98 (after this run,
not rerun live) fixes two of these: speaking's encoding bound is 30 s
(floppy's side of an utterance measured 0.06 to 0.17 s against a fake
backend, so the 5 s went to Gemini's unstreamed synthesis), and the bot's own sound now names its source,
so its product is withheld instead of scored as an outside sound. Voice
detection still runs on it and fails without a voice backend, by design.
Reflection's recalled episode
count grew by two per turn (1, 3, 5, 7), since reasoning and reflection
both insert a `cognitive_episode` row. Decision D97 fixed it: reflection alone stores
the turn's episode, and `a_reasoned_turn_is_recalled_by_the_next` pins one
episode per turn on the fake backends.

## 2026-10-11, speaking and speech decoding on the Gemini API (decision D94)

The runs used floppy's working tree on commit `365f38c6` with the speech
service's `gemini` provider (D94). The cloud session reaches
`generativelanguage.googleapis.com` only through its HTTPS proxy, which
adds the key, so a local relay forwarded `http://127.0.0.1:8789/v1beta` to
it; `.env.gemini` sets `FLOPPY_SPEECH_BASE_URL` to the relay and
`FLOPPY_SPEECH_API_KEY` to a placeholder. The chat also had `.env.cpa`
(the CPA gateway and TypeSafe through their relays, as on 2026-10-10).

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `live_speech_round_trip_through_an_env_file` | `gemini-3.1-flash-tts-preview` (voice `Kore`), then `gemini-3.5-flash` | pass | 1 | 36 s; `FLOPPY_LIVE_SPEECH_PROVIDER=gemini`: "The quick brown fox jumps over the lazy dog." encoded to `audio/wav` and decoded back with "fox" and "lazy" |
| `floppy_live` `live_chat` | `gabe-codex-1/gpt-6-luna`, speech on Gemini | pass | 1 | 53 s; `FLOPPY_LIVE_ENV_FILE=.env.cpa:.env.gemini`: floppy said "Hello! Great to hear from you." and speaking encoded it with its defaults to `media/speaking-1.pcm` (24 kHz mono `audio/pcm`, `bot.ability.speaking.output`); the listening heard the bot's own sound and failed at voice detection (`the backend is not configured`: no LocalAI) |

The listening has not run live: voice detection, which every sound goes
through, needs LocalAI at `FLOPPY_VOICE_BASE_URL`, and no backend the
session reaches serves it.

## 2026-10-10, reading, hearing and speaking through the CPA gateway, model `gabe-codex-1/gpt-6-luna`

The run used floppy commit `9a611e48` with wasi-sdk 25. The cloud session
reaches `cpa.willen.dev` and `api.typesafe.ai` only through its HTTPS proxy,
which adds the keys, and `scx` sends no proxy, so a local relay forwarded
`http://127.0.0.1:8787/v1` to the gateway. `.env.cpa` sets
`FLOPPY_BASE_URL` to the relay and `FLOPPY_API_KEY` to a placeholder
(`FLOPPY_LIVE_ENV_FILE=$PWD/.env.cpa
FLOPPY_LIVE_MODEL=gabe-codex-1/gpt-6-luna cargo test -p floppy-tests --test
generate_service live_image_reading -- --ignored --test-threads=1`).

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `live_image_reading_through_an_env_file` | `gabe-codex-1/gpt-6-luna` | pass | 1 | 401 s with the component build; the reply read "Total due: $14.21" from `tests/fixtures/total-due.png`, final state `done` |
| `live_speech_round_trip_through_an_env_file` | none | not run | 0 | no speech backend: the CPA gateway and TypeSafe both answer 404 on `/v1/audio/speech` and `/v1/audio/transcriptions`, CPA's 260 models include no speech model, and it rejects `gemini-3.1-flash-tts-preview` (`model_not_found`) |

Hearing, listening and speaking need `FLOPPY_SPEECH_BASE_URL` (and
listening `FLOPPY_VOICE_BASE_URL`, LocalAI), which neither gateway serves,
so none of them ran live.

## 2026-10-08, the endpoint cutover (decision D31), direct vLLM, model `Qwen/Qwen3.8-Flash-Next`

The runs used floppy commit `39c3b5df` (scxml.rs subtree `92742d65`): the
generate package declares no origin, and floppy's `.env` holds
`FLOPPY_BASE_URL` (the direct vLLM `http://192.168.2.89:8000/v1`) and
`FLOPPY_API_KEY` (a copy of the vLLM key, made without printing it).
`scx` was the debug binary the tests build from the subtree
(`target/scx/debug/scx`), with the dev cache
`target/components/package-cache`. The live test also had
`TYPESAFE_API_KEY` from `.env.local`
(`FLOPPY_LIVE_ENV_FILE=.env:.env.local
FLOPPY_LIVE_MODEL=Qwen/Qwen3.8-Flash-Next cargo test -p floppy-tests --test
builder_bot -- --ignored`); the README runs used the README's command,
`--env-file .env` only, so triage ran its fallback
(`fallback:unavailable`).

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `live_builder_through_an_env_file` | `Qwen/Qwen3.8-Flash-Next` | pass | 1 | 1033 s, final state `done` on attempt 8 after 7 failures (`validate` and `simulate test-failed`), triage `triage.repair` seven times through TypeSafe, one machine-applicable fix |
| README command, run 1 | `Qwen/Qwen3.8-Flash-Next` | fail | 1 of 2 | exit 0 in 615 s, final state `failed` (`no-progress`): both author generations cancelled at the builder's 300 s `author-timeout` |
| README command, run 2 | `Qwen/Qwen3.8-Flash-Next` | pass | 2 of 2 | exit 0 in 380 s, final state `done` on attempt 5 after `validate SCXML008` (with `XSD-cvc-complex-type.3.2.2`), `simulate test-failed`, `validate XML001`, and `simulate test-failed`, fallback triage four times, one machine-applicable fix; plan `traffic_light_pedestrian`; saved as `bots/builder/examples/traffic_light_vllm/` (`scx validate`: 0 errors, 0 warnings) |
| README command without `FLOPPY_BASE_URL` | none | pass (fail fast) | 1 | `--env-file .env.local` only: the consent listing showed `env FLOPPY_BASE_URL (no value)` and `env FLOPPY_API_KEY (no value)`, the design generation sent nothing, and the builder ended in `failed` (`unconfigured`, its diagnostic naming both variables) 76 ms after the request |

## 2026-10-07, the builder against vLLM directly (no gateway), model `Qwen/Qwen3.8-Flash-Next`

The runs used floppy commit `4909f064` (the failure-cause logging below;
the generate service, the dev cache, the subtree scxml.rs `1a1defa7`, and
the `target/scx-live` `scx` are unchanged from the entries below), with the
builder's model calls going straight to vLLM on the DGX Spark
(`http://192.168.2.89:8000/v1`, model `Qwen/Qwen3.8-Flash-Next`) instead of
through the CPA gateway. The vLLM key reached the builder's
`env:apikey="CPA_API_KEY"` slot from the calling shell only: a subshell
exported `CPA_API_KEY` from `VLLM_API_KEY` in floppy's `.env` and `scx` got
a bare `--env CPA_API_KEY`; floppy's `.env` was not given as an env file
(its own `CPA_API_KEY` would have won), and no value was written to disk.
`TYPESAFE_API_KEY` came from `.env.local`. The base URL came
from a scratch env file holding only `CPA_BASE_URL`, naming a local
streaming tap (`http://127.0.0.1:<port>/v1`, a Python script outside the
repository) that forwarded each request byte for byte to
`192.168.2.89:8000` and its streamed answer chunk by chunk, and recorded,
per request, when the first and last `reasoning` and `content` deltas
arrived and whether the stream completed or the client closed it (it never
recorded a header). In the floppy root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"model":"Qwen/Qwen3.8-Flash-Next"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env CPA_API_KEY --env-file .env.local \
    --env-file <file with only CPA_BASE_URL> bots/builder/builder.scx
```

Durations are from the arrival of each stage's `log` record on standard
output to the next one; "reasoning from" is the tap's time of the first
`reasoning` delta of each generation, from the request.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| builder, run 1 | `Qwen/Qwen3.8-Flash-Next` | pass | 1 of 3 | exit 0 in 527 s, final state `done` on attempt 5 after `validate XML001` (an unclosed `event:schema` value), `validate SCXML008` (`<cancel>` without `sendid`), `validate XSD-cvc-complex-type.3.2.2` (`sendid` on `<send>`), and `simulate test-failed`, triage `triage.repair` four times (0.2 s each); generations 15.5 s (design), 74.2 s, 39.6 s, 18.9 s, 45.8 s, 42.5 s (author); simulations 143.3 s and 144.0 s (4 tests each); plan `traffic_light_pedestrian` |
| builder, run 2 | `Qwen/Qwen3.8-Flash-Next` | pass | 2 of 3 | exit 0 in 437 s, final state `done` on attempt 3 after `validate SCXML008` (with `XSD-cvc-complex-type.3.2.2`) and `simulate test-failed` (4 tests), triage `triage.repair` twice; generations 13.6 s (design), 80.0 s, 10.8 s, 40.9 s (author); simulations 144.4 s and 144.9 s (4 tests each); plan `traffic_light_pedestrian` |
| builder, run 3 | `Qwen/Qwen3.8-Flash-Next` | pass | 3 of 3 | exit 0 in 235 s, final state `done` on attempt 5 after `validate XSD-cvc-complex-type.3.2.2`, `simulate test-failed` twice (different observations, so progress), and `validate XML001`, triage `triage.repair` four times; generations 15.9 s (design), 41.9 s, 19.4 s, 34.8 s, 12.6 s, 21.1 s (author); simulations 28.3 s, 28.9 s, 28.9 s (3 tests each); plan `traffic_light` |

No generation came near the builder's `generate_timeout_ms` (300 s): the
longest took 80.0 s, so no generation was cancelled and the question of
whether a cancelled one was still reasoning did not arise. Every one of the
16 generations streamed its reasoning from the start: the first `reasoning`
delta arrived 0.15 s to 0.66 s after the request, the reasoning went on
until 5.1 s to 74.0 s after it (124 to 1,717 deltas), the answer followed
it and took 3.6 s to 7.0 s, and every stream completed (`finish_reason` `stop`, all
`200`). No `design-failed`, `status`, or timeout occurred.

Compared with the gateway runs (the two entries below: two passes in three
each), direct vLLM passed three in three. Its author generations took
10.8 s to 80.0 s, where through the gateway six of the 18 author
generations ended at a limit (three `status` at about 125 s through
Cloudflare, three `author-timeout` at 300 s on the LAN) and the answered
ones took up to 119.9 s; the design generations took
13.6 s to 15.9 s against 9.3 s to 35.3 s. The gateway held the reasoning
back about 19 s before its first byte; vLLM sends it at once, so with vLLM
a generation in flight is visibly progressing. The longest stages are now
the simulations (up to 144.9 s for 4 tests), which run the model's tests
on the wall clock for their own `settle_ms`. The runs needed more repair
attempts (3 to 5) than the gateway's passes (2 to 4) with the same mix of
validator and simulation failures. The built documents were not kept.
Nothing in the tests was changed to make them pass.

### The missing cause of a `design-failed`

The LAN gateway's run 1 below logged `design-failed` after 0.2 s with no
cause. The builder had it: the design stage's diagnostic message named the
generate service's failure code (`the design generation failed (<code>)`),
but the `retry_gate` log carried only the diagnostic's kind and code, and
the `failed` done data carried only the diagnostics of the last failure,
which the later `author-timeout`s had replaced. Commit `4909f064` makes
the `retry_gate` and `failed` logs carry the first diagnostic's message
too, adds the failure history (as in `done`'s report) to the `failed` done
data, names the error event in the `error.execution` and
`error.communication` diagnostics, and says "it ended without an event"
instead of `null`. The fake-backend test
`a_fast_design_failure_keeps_its_cause_in_the_log_and_the_failed_done_data`
(a plan request answered `404` at once, then a draft that fails twice) was
red before the change (`failure 1: generate design-failed`, no history) and
is green after it. The gateway run's code itself is not recoverable from
that run's output.

## 2026-10-07, the builder through the gateway's LAN address (no Cloudflare), model `sparks/qwen3.8-flash-next`

The runs used floppy commit `4bd63aa0` (code and subtree scxml.rs
`1a1defa7` as in the entry below), with the same dev cache and
`target/scx-live` `scx`. The only change is the generate service's base
URL: the same gateway, reached on the LAN at `http://192.168.2.128:8319/v1`
instead of through Cloudflare at `https://cpa.willen.dev/v1`. It came from a
third env file holding only `CPA_BASE_URL=http://192.168.2.128:8319/v1` (no
key), given last, so neither `.env` was edited; the builder's
`env:baseurl="CPA_BASE_URL"` names it, and the consent listing showed
`CPA_BASE_URL` going only to the generate service's `<invoke>`s. No harness
change was needed: `FLOPPY_LIVE_ENV_FILE` takes several files the same way.
The host admitted the run-supplied `http:` origin, which a value supplies
entirely: design generations answered in 15.7 s to 35.3 s and an author
generation answered after 119.9 s. From a clean shell
(`env -i HOME="$HOME" PATH="/usr/bin:/bin"`), in the floppy root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"model":"sparks/qwen3.8-flash-next"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file .env --env-file .env.local \
    --env-file <file with only CPA_BASE_URL> bots/builder/builder.scx
```

Durations are from the arrival of each stage's `log` record on standard
output to the next one.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| builder, run 1 | `sparks/qwen3.8-flash-next` | fail | 1 of 3 | exit 0 in 621 s, final state `failed` (`no-progress`): design 0.2 s (`design-failed`; the log does not carry the cause), design 19.1 s, then both author generations cancelled at the builder's 300 s limit (`timeout author-timeout`, 300.0 s and 300.2 s), the same failure twice in a row |
| builder, run 2 | `sparks/qwen3.8-flash-next` | pass | 2 of 3 | exit 0 in 321 s, final state `done` on attempt 3 after `simulate test-failed` (4 tests) and `validate SCXML008` (with `XSD-cvc-complex-type.3.2.2`, `<cancel event>`), triage `triage.repair` twice; generations 35.3 s (design), 63.8 s, 119.9 s, 37.6 s (author); simulations 27.6 s and 34.9 s (4 tests each); plan `traffic_light` |
| builder, run 3 | `sparks/qwen3.8-flash-next` | pass | 3 of 3 | exit 0 in 448 s, final state `done` on attempt 4 after `timeout author-timeout` (300.3 s), `validate XML001` (an unclosed `event:schema` value), and `validate SCXML008` (`<cancel sendidref>`), triage `triage.repair` three times; generations 15.7 s (design), 300.3 s (cancelled), 69.3 s, 15.9 s, 10.4 s (author); simulation 34.4 s (3 tests); plan `traffic_light_pedestrian` |

Compared with the Cloudflare runs (entry below: two passes in three, three
author generations ended `status` at about 125 s): no generation ended at
about 125 s and no `status` failure occurred, and an author generation that
took 119.9 s, which Cloudflare would most likely have cut off, answered.
Cloudflare's limit is gone, and the long generations now run into the
builder's own `generate_timeout_ms` (300 s), which ended three of them. The
LAN gateway also holds the reasoning back: a direct streamed request
(`reasoning_effort` `high`, a short counting prompt) got its first `data:`
event, the reasoning and the content together, after 19.2 s, and all 527
events within 3.3 s after that, so the 0.35 s time to first byte measured
for it earlier does not mean the output streams while the model reasons, and
nothing showed whether the cancelled
generations were still reasoning or had stalled. The built documents were
not kept. Nothing in the tests was changed to make them pass.

## 2026-10-07, no `scx` time limit on service requests and the no-progress exit (decision D26), model `sparks/qwen3.8-flash-next`

The runs used floppy commit `6f48bcc4` (subtree scxml.rs `1a1defa7`, which
removed `scx`'s 120 s total and 10 s connect timeouts on invoked-service
requests), with the dev cache rebuilt by `sh scripts/build-components.sh`
and `scx` rebuilt with its session components into `target/scx-live`, as
the entries below describe. `CPA_API_KEY` came from floppy's `.env` and
`TYPESAFE_API_KEY` from `.env.local`, no base URL was set, and
the request set no `max_attempts`, so only the no-progress rule could end
the loop. From a clean shell (`env -i HOME="$HOME" PATH="/usr/bin:/bin"`),
in the floppy root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"model":"sparks/qwen3.8-flash-next"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file .env --env-file .env.local bots/builder/builder.scx
```

Durations are from the arrival of each stage's `log` record on standard
output to the next one.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| builder, run 1 | `sparks/qwen3.8-flash-next` | pass | 1 of 3 | exit 0 in 319 s, final state `done` on attempt 3 after `generate author-failed` (`status`) and `simulate test-failed`, triage `triage.repair` twice through the decision service; generations 9.3 s (design), 125.5 s (author, `status`), 46.1 s, 25.2 s (author); simulations 54.4 s and 54.9 s (2 tests each) |
| builder, run 2 | `sparks/qwen3.8-flash-next` | fail | 2 of 3 | exit 0 in 267 s, final state `failed` (`no-progress`): design 14.0 s, then both author generations ended `status` after 125.6 s and 125.3 s, the same failure twice in a row |
| builder, run 3 | `sparks/qwen3.8-flash-next` | pass | 3 of 3 | exit 0 in 220 s, final state `done` on attempt 2 after `validate SCXML008` (with `XSD-cvc-complex-type.3.2.2`, `<cancel event>`), triage `triage.repair`; generations 12.7 s (design), 45.2 s, 20.2 s (author); simulation 139.4 s (4 tests) |

No request ended at 120 s any more: the three long author generations ran
about 125 s and ended as `status`, an HTTP error answer from the gateway,
consistent with Cloudflare's `524` at about 125 s measured in the entry
below (the done data does not carry the status code). Run 2's `no-progress`
is that gateway limit hit twice in a row. The builder's `builder.timeout`
values are long enough: `generate_timeout_ms` (300 s) is more than twice the
longest generation, which the gateway itself ends at about 125 s, and the
longest simulation (139.4 s) is far below `simulate-timeout` (8 tests of
120 s plus 60 s), so none was raised. The built documents were not kept.
Nothing in the tests was changed to make them pass.

## 2026-10-07, `reasoning_effort` (decision D25), model `sparks/qwen3.8-flash-next`

The runs used floppy commit `834a9c08` (the subtree unchanged from the
entry below), built and run exactly as that entry describes: the dev cache
pinned to commit `834a9c08`, `scx` rebuilt with its session components,
`CPA_API_KEY` from floppy's `.env` and `TYPESAFE_API_KEY` from
`.env.local`, no base URL set, and the same builder command.

### Choosing the default

Each request below went straight to `https://cpa.willen.dev/v1` with the
body `generate` sends (strict `json_schema`, `stream: true`, the builder's
system message and prompt), one at a time. "First event" is the first
`data:` line, "total" the last.

| Prompt | `reasoning_effort` | First event | Total | Validator errors in the draft |
| --- | --- | --- | --- | --- |
| design | (none) | none | `524` at 125.2 s | |
| design | `low` | 13.7 s | 20.0 s | |
| design | `medium` | 32.9 s | 44.6 s | |
| design | `high` | 13.9 s, 14.2 s | 22.3 s, 20.9 s | |
| author | `low` | 55.3 s, 32.8 s | 60.4 s, 37.8 s | 6, 10 |
| author | `medium` | 88.5 s, 50.5 s | 94.5 s, 55.2 s | 0, 6 |
| author | `high` | 22.3 s, 73.8 s, none | 27.6 s, 79.0 s, `524` at 125.2 s | 9, 6 |
| generate live test | `low` | 2.5 s | 2.6 s | |

Every draft parsed as XML and ran into the same errors (mostly `<cancel
event>`, which SCXML does not have), which the builder's repair loop is
for, so quality did not separate the values; `low` had the best worst case,
so it is the default. Two things did not match the expectation the change
was made on:

- The gateway does not stream the reasoning live with `reasoning_effort`
  either. In every request the first reasoning chunk arrived within 0.2 s
  of the first content chunk (on a short prompt with `high` and no schema:
  reasoning at 8.4 s, content at 8.5 s, 205 chunks in the first second), so
  the gateway holds the reasoning back until the answer starts, and no byte
  flows while the model reasons.
- The value hardly bounds the reasoning: the author prompt with `none`
  still reasoned (1,273 reasoning chunks, first event at 59.1 s) and with
  `minimal` got a `524` at 125.3 s.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `live_generate_through_an_env_file` | `sparks/qwen3.8-flash-next` | pass | 1 | 14 s; first event 2.5 s when measured directly |
| builder, run 1 | `sparks/qwen3.8-flash-next` | fail | 1 of 3 | exit 0 in 510 s, `attempts-exhausted`: designs took 120.3 s (`transport`), 120.0 s (`transport`), and 27.0 s, then both author generations 120.0 s (`transport`) |
| builder, run 2 | `sparks/qwen3.8-flash-next` | pass | 2 of 3 | exit 0 in 276 s, final state `done` on attempt 3 after `validate SCXML011` and `simulate test-failed` (triage `triage.repair` twice); generations 19.9 s (design), 49.1 s, 11.6 s, 23.9 s (author); all 3 simulation tests passed |
| builder, run 3 | `sparks/qwen3.8-flash-next` | fail | 3 of 3 | exit 0 in 301 s, `attempts-exhausted`: design 18.2 s, author 120.3 s (`transport`), then repairs in 56.0 s, 23.5 s, 24.3 s that failed `XSD-cvc-pattern-valid`, `schema-not-json`, and `simulate test-failed` |

Every `transport` failure ended at 120 s, `scx`'s fixed limit for a whole
service request (`DEFAULT_EXTENSION_TOTAL_TIMEOUT`, before Cloudflare's
`524` at about 125 s). No bytes were flowing then: the gateway sends
nothing until the answer starts, and every answer measured ended within 14 s
of its first byte. So an idle limit in `scx` instead of the total one would
not have saved these generations; what would is a gateway that streams the
reasoning as it is produced. The built document of run 2 was not kept.
Nothing in the tests was changed to make them pass.

## 2026-10-07, floppy (decision D23) on the DGX Sparks (decision D24), model `sparks/qwen3.8-flash-next`

The runs used floppy commit `790785a2` (subtree scxml.rs `76654ac1`):
`sh scripts/build-components.sh` publishing the decision namespace and both
services into the dev cache `target/components/package-cache` (pinned to
release tag `v0.1.0` at commit `790785a2`), and `scx` built from the
subtree with those session components embedded
(`SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml"
CARGO_TARGET_DIR="$PWD/target/scx-live" cargo build --release --locked -p
scxml-cli --manifest-path scxml.rs/Cargo.toml`). `CPA_API_KEY` came from
floppy's `.env` and `TYPESAFE_API_KEY` from `.env.local`; no
`CPA_BASE_URL` or `TYPESAFE_BASE_URL` was set, so each service used its
package's declared origin (`https://cpa.willen.dev/v1`,
`https://api.typesafe.ai`), as the consent listing showed. The live tests
(which run `scx run --offline --cache-dir` against the test build's cache):
`FLOPPY_LIVE_ENV_FILE=<env file> cargo test --locked -p floppy-tests --test
<test> -- --ignored <name>`. The builder, from a clean shell
(`env -i HOME="$HOME" PATH="/usr/bin:/bin"`), in the floppy root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file .env --env-file .env.local bots/builder/builder.scx
```

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `live_generate_through_an_env_file` | `sparks/qwen3.8-flash-next` | pass | 1 | 14 s; strict `json_schema` structured output over `stream: true`, no token limit sent |
| `live_decision_through_an_env_file` | TypeSafe `system_one` | pass | 2 | 12 s; final state `shipped`, data admitted by its schema. The first run failed before any call: the shell's `wasm-tools` was 1.240.0, not the pinned 1.244.0, so the component build refused |
| builder | `sparks/qwen3.8-flash-next` | fail | 1 | exit 0 in 484 s, final state `failed` (`attempts-exhausted`): all 4 design generations failed as `transport` (`design-failed`); triage ran 3 times through the decision service |

Why the builder failed: the gateway does not stream. Measured with the
builder's own design prompt and `curl`, the first response byte arrives
only with the last (time to first byte equals total time, with or without
`accept: text/event-stream`): 8 s at a 600-token limit, 24 s at 1,500, all
of them reasoning tokens. The unlimited design generation got Cloudflare's
`524` after 125 s, and in the builder run `scx` ended each one first at its
fixed 120 s limit for a whole invoked-service request
(`DEFAULT_EXTENSION_TOTAL_TIMEOUT`). So a generation whose reasoning takes
longer than about 100 s cannot succeed through `https://cpa.willen.dev/v1`
as it is now, and the buffering also means that closing the connection does
not stop the generation early. A token limit would not help: it ends the
reasoning before the answer (`truncated`). Short generations (the generate
live test) pass. Nothing in the tests was changed to make them pass.

## 2026-10-07, repository packages resolved by the embedded GitHub resolver component (decision D22), model `gpt-6-luna`

The run used nemobot commit `a1367dbe` (subtree scxml.rs `f6b8a1c6`) with
the change to `scripts/build-components.sh` this entry is committed with
(it builds the resolver component `scx` now embeds): `sh
scripts/build-components.sh` publishing the decision namespace and both
services into the dev cache `target/components/package-cache` (pinned to
release tag `v0.1.0` at commit `a1367dbe`), and `scx` built from the subtree
with those session components and the resolver embedded
(`SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml"
CARGO_TARGET_DIR="$PWD/target/scx-live" cargo build --release --locked -p
scxml-cli --manifest-path scxml.rs/Cargo.toml`). The builder command, from a
clean shell (`env -i HOME="$HOME" PATH="/usr/bin:/bin"`), in the nemobot
root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4,"model":"gpt-6-luna"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file .env.local bots/builder/builder.scx
```

`--offline` resolves every repository package from the dev cache, so the
resolver component is never asked; the run proves the cutover leaves
offline resolution and the consent listing as they were: the decision
namespace and both services by their tagless URIs with `release v0.1.0
(commit a1367dbe...)`, the generate service's origin
`https://api.openai.com/v1`, and the decision service's
`https://api.typesafe.ai`; `TYPESAFE_BASE_URL` had no value (decision D18's
declared origin applies).

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| builder | `gpt-6-luna` | pass | 1 | exit 0 in 89 s; attempt 2 of 4, 1 failure (`validate` / `SCXML070`: a `<send>` to `#_internal` cannot use `delay`), 0 fixes, triage `triage.repair` from the decision service; plan `pedestrian_traffic_light`; both of its tests (`normal_cycle`, `button_ends_green_early`) passed in simulation; progress `log`s `design`, `author`, `validate`, `retry_gate`, `triage`, `author`, `validate`, `simulate`, `done`; the built document compiled again with `scx compile` (no packages) |

The builder's output was written to `target/builder-live-cli/` and is not
committed. Nothing in the tests was changed to make them pass.

## 2026-10-07, `@<tag>` split at parsing, the tagless URI as identity (decision D21), model `gpt-6-luna`

The run used nemobot commit `c5295cd2` (subtree scxml.rs `50f6e592`):
`sh scripts/build-components.sh` publishing the decision namespace and both
services into the dev cache `target/components/package-cache` (their
manifests and the namespace's schema name the tagless URIs, pinned in the
cache to release tag `v0.1.0` at commit `84fe9ca0`), and `scx` built from the
subtree with those session components embedded
(`SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml"
CARGO_TARGET_DIR="$PWD/target/scx-live" cargo build --release --locked -p
scxml-cli --manifest-path scxml.rs/Cargo.toml`). The builder command, from a
clean shell (`env -i HOME="$HOME" PATH="/usr/bin:/bin"`), in the nemobot
root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4,"model":"gpt-6-luna"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file .env.local bots/builder/builder.scx
```

The consent listing named the decision namespace and both services by their
tagless URIs with `release v0.1.0 (commit 84fe9ca0...)`, the generate
service's origin `https://api.openai.com/v1`, and the decision service's
`https://api.typesafe.ai`; `TYPESAFE_BASE_URL` had no value (decision D18's
declared origin applies).

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| builder | `gpt-6-luna` | pass | 1 | exit 0 in 98 s; attempt 1 of 4, 0 failures, 0 fixes, no triage (the decision service did not run); plan `traffic_light_pedestrian`; both of its tests passed in simulation; progress `log`s `design`, `author`, `validate`, `simulate`, `done`; the built document compiled again with `scx compile` (no packages) |

The builder's output was written to `target/builder-live-cli/` and is not
committed. Nothing in the tests was changed to make them pass.

## 2026-10-06, tagless URIs, the draft profile, and the base URL default (decisions D18, D19, D20), model `gpt-6-luna`

The runs used nemobot commit `05157554` (subtree scxml.rs `73f340b7`):
`sh scripts/build-components.sh` publishing the decision namespace and both
services into the dev cache `target/components/package-cache` (pinned under
their tagless URIs to release tag `v0.1.0` at that commit; `scx`'s release
cache untouched), and `scx` built from the subtree with those session
components embedded (`SCXML_WASM_ARTIFACTS_DIR="$PWD/target/components/scxml"
CARGO_TARGET_DIR=target/scx-live cargo build --release --locked -p
scxml-cli`). The the original env file sets no `TYPESAFE_BASE_URL`; the decision
service used its package's declared origin `https://api.typesafe.ai`
(decision D18), so the builder's triage went to the decision service, not
the fallback. Drafts were checked with `<scx:validate profile="draft">`
(decision D20). The builder command, from a clean shell
(`env -i HOME="$HOME" PATH="/usr/bin:/bin"`), in the nemobot root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4,"model":"gpt-6-luna"}}' |
  target/scx-live/release/scx run --offline --cache-dir target/components/package-cache \
    --env-file .env.local bots/builder/builder.scx
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
only). The the original env file sets no `TYPESAFE_BASE_URL`, so in the builder
run the decision service's `baseurl` slot was absent and the builder used
its fallback triage. The builder command, from a clean shell
(`env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin"`), in the
nemobot root:

```console
echo '{"name":"builder.request","data":{"spec":"a traffic light with a pedestrian button","constraints":["send every light change and every walk signal to the invoker","a button press during green ends green early"],"max_attempts":4,"model":"gpt-6-luna"}}' |
  scx run --env-file .env.local bots/builder/builder.scx
```

The live tests: `NEMOBOT_LIVE_ENV_FILE=<the original .env> NEMOBOT_LIVE_MODEL=gpt-6-luna
cargo test -p nemobot-tests --test <test> -- --ignored --nocapture`.

| Run | Model | Result | Runs used | Note |
| --- | --- | --- | --- | --- |
| `scx run --env-file <the original .env> bots/builder/builder.scx` | `gpt-6-luna` (generate service); decision unconfigured (no `TYPESAFE_BASE_URL`) | pass | 1 of 1 | exit 0 in 116 s; the `done` record's data: attempt 2 of 4 after 1 failure (`validate` / `SCXML070`), 0 machine fixes, triage `fallback:unavailable`, plan `pedestrian_traffic_light`, all four of its tests passed in simulation; progress `log`s `design`, `author`, `validate`, `retry_gate`, `triage`, `author`, `validate`, `simulate`, `done`; the built document compiled again with `scx compile` (no packages) |
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
`NEMOBOT_LIVE_ENV_FILE=<the original .env> NEMOBOT_LIVE_MODEL=gpt-6-luna cargo test
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
`NEMOBOT_LIVE_ENV_FILE=<the original .env> NEMOBOT_LIVE_MODEL=gpt-6-luna cargo test
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
