# floppy's eval

The eval grades every feature of the floppy bot on real input
([`tests/eval/features.json`](../tests/eval/features.json)): conversation,
cognition's tiers, decisions and the ignore, ability offers, prompts,
memory and recall, people, persona, hearing, turn taking, speaking, voices
and diarization, rest and consolidation, seeing, failed turns, the room,
and what it learns from what people tell it: its **name** (the identity
ability adopts a name, answers to it, and recalls it after a restart), its
**reflexes** (turn behaviors reflection learns and autonomy then runs on
matching input) and its **routines** (persistent behaviors that act on a
schedule with no input). It began as the learning eval, because the live
runs (docs/live-proof.md, 2026-10-20 and 2026-10-21) showed reflection
rarely learns a behavior, and no test checked a learned behavior against
the input it should handle; it grew to every feature on 2026-10-25 (#82).

It has five parts:

- **The driver**, `tests/tests/floppy_eval.rs`, an ignored test like
  `floppy_live`: it runs the bot in the room on real backends through the
  steps of each scenario of a plan, several at a time, and writes every
  run's records. It asserts nothing about the bot.
- **The grade**, `tests/eval/score.py`, `tests/eval/judge.py` and
  `tests/eval/sound.py`, black box: each scenario's checks read what the
  bot said back (and, for the voice, what a person heard it say out loud),
  and two LLM judges (a CPA `gabe-codex-*` model and Gemini) grade the
  conversation alone against the scenario's criteria.
- **The feature grade**, `tests/eval/features.py`: each feature graded from
  the scenarios that exercise it, with its diagnostics, its log evidence
  and its offline tests beside the grade, never in it (`features.md`).
- **The question report**, `tests/eval/questions.py`: the maintainer's
  questions about floppy (what it learns, remembers, understands and
  decides, whether its memory works, whether it knows its own voice and
  learns other people), each answered from its scenarios with the
  transcripts, the judges' reasons and floppy's own logs as evidence
  (`questions.md`).
- **The hill climb**, `tests/eval/eval.py climb`: an LLM proposes a rewrite
  of one learning prompt from the failures, the eval scores it, and the best
  is kept, one prompt per generation.

The runner is `tests/eval/eval.py` (Python 3, standard library only). It
never edits the workspace: everything goes under `target/floppy-eval/`.

## Scenarios

`tests/eval/scenarios/<name>.json`, one per scenario:

| Scenario | Category | What it teaches, then probes |
| --- | --- | --- |
| `name_adopt` | name | "your name is Juniper"; answers to Juniper and does not claim to be Pixel; restarted, says it is Juniper and still does not claim to be Pixel |
| `name_rename` | name | Juniper, then Biscuit; answers to Biscuit and no longer claims to be Juniper; restarted, says it is Biscuit and does not claim to be Juniper |
| `reflex_pingpong` | reflex | "whenever I say exactly 'ping', reply 'pong'"; two pings get exactly "pong", an unrelated question gets no pong, a ping after a restart still gets exactly "pong" |
| `reflex_greeting` | reflex | "good morning" gets "Good morning, sunshine!"; a variation from another person; "good night" must not; "good morning" again after a restart |
| `reflex_keyword` | reflex | a mention of bananas inside a longer sentence gets just "yellow alert"; an unrelated request does not; a banana sentence after a restart still gets just "yellow alert" |
| `routine_reminder` | routine | a water reminder every minute; reminders with no input; stopped when asked; quiet after |
| `routine_restart` | routine | "stretch break" every minute; keeps running after a restart with no new request |
| `chat_basics` | conversation | a capital, a follow-up ("And what about Japan?"), what was asked, a sum; one reply each |
| `chat_ignore` | conversation | Gabe and Ana talk to each other (ambient input): floppy stays quiet; then answers Gabe directly |
| `recall_restart` | memory | a dog's name and a favorite color; both recalled after a restart |
| `recall_people` | memory | Gabe and Ana each say one fact; each asks about the other |
| `persona_style` | persona | "always talk like a pirate"; a pirate's answer, and still one after a restart |
| `voice_question` | voice | two questions out loud, answered and said out loud correctly |
| `voice_overlap` | voice | a second question out loud while floppy still says its first answer: both answered, floppy's own words not taken for the person's |
| `voice_pause` | voice | a question out loud with a pause in the middle: answered once, not at the fragment |
| `voice_people` | voice | two voices on one microphone, each asking its own name |
| `voice_rest` | voice | an introduction, a quiet spell long enough to rest and consolidate (`rest_after_ms` 20 s), then the name and a fact |
| `seeing_look` | seeing | a camera shows "Total due: $14.21" (seeing on); floppy is asked the total |
| `chat_understand` | conversation | answers never said outright: when to leave for a 7 am flight, what to grab in the rain, the day after tomorrow from Friday |
| `recall_many` | memory | five facts in one message, two unrelated questions; three of the facts after a restart |
| `recall_update` | memory | a favorite food, corrected later; the corrected one after a restart |
| `people_overheard` | memory | Gabe and Ana talk to each other (ambient) and mention their pottery and marathon; later each asks floppy about themselves |
| `voice_self` | voice | floppy's own answer echoed back to the microphone in its own voice (`warm`): no reply to itself; then the person's next question answered |
| `turn_failure` | failure | reasoning bounded to one second: floppy still answers the person, then a greeting |

The name is Juniper, not floppy, because the reasoning prompt already calls
the bot floppy: a test of learning a name must use one the prompts do not
hold.

A scenario is `{category, features, intent, bot_name, sessions, checks,
criteria}`, with optional `criteria_features` (`{feature: [criterion]}`:
the judges' criteria that grade a feature), `params` (`{name: expr}`,
data the room passes the bot beside its model: `seeing_enabled`, `bounds`)
and `edits` (`[{path, old, new}]`, each `old` found exactly once in the
job's scratch copy of the bot).
Each session is the bot started again on the same memory and behaviors
directories (a restart with what it learned). A step is `{"say": text,
"source"?: id, "timeout_s"?: n}`, one `bot.ability.communication.input`
waited on until its reflection settles (or its turn fails) and then 15 s
more, since its reply can be said after its reflection settles. A lesson
the turn gave the learning ability (`learning.recalled`) is learned after
the turn, so the step then also waits, bounded by `timeout_s` again, until
each lesson closes (`learning.learned` or `learning.failed`) and one more
turn has settled for each (the turn carrying its result). A settle
from another turn meanwhile (a routine's tick) also counts, so the wait
can end early, never late. `steps.json` records, per step, whether its
turn settled (`completed`), how many lessons closed during it
(`lessons`, a count, not attributed to a lesson) and whether the lesson
wait finished (`lessons_settled`); a step whose lessons did not finish
still lets the session go on, and a turn that never settled has
`lessons_settled` false. The other step is `{"wait_s": n}`,
the wall clock passing with no input; its `lessons` counts the lessons
closed in it, which it does not wait on (`lessons_settled` is true).

A said step may name its `targets` (default `["floppy"]`; `[]` is ambient
input the room gives everyone, which floppy overhears). A heard step,
`{"hear": text, "voice"?, "segments"?, "source"?, "who"?}`, is the person
saying it out loud: `sound.py` synthesizes it on the speech backend's
`/audio/speech` in `voice` (default `canvas`; floppy speaks in `warm`), or
`segments` (text pieces and numbers, a pause of that many seconds), and the
driver gives the room the 24 kHz PCM as `environment.sound` from `source`
with a second of silence, as `scripts/talk-live.py` does; `who` names the
person for the judges when the source is a shared microphone. It is waited
on as a said step. A shown step, `{"show": path, "describe", "wait_s"?}`,
gives the room the image as `environment.visual` from a `camera` and waits
`wait_s` (20). A said or heard step's `settle` is `turn` (the default),
`said` (the step ends when floppy starts to speak, so the next one
overlaps its speech; the turn's settling is waited on later) or `quiet`
(a turn floppy may rightly leave alone: the step ends when its turn
settles, or as completed when none has 3 minutes past its `timeout_s`; a
late turn is still waited for, so its reply is not read as the next
step's). Each job's media directory is
`jobs/<id>/media/`, granted to the speech, generate and vision services,
so the sounds floppy made stay with the job. `criteria` are what the judges
score, 0 to 10 each.

### Checks and diagnostics

The grade is black box (decision D241): a scenario's `checks` score only
what floppy said back to what it was told, and the judges read only the
conversation. A check looks at one step (`at: [session, step]`) or up to
the end of one (`by`), and scores 0 to 1 with a `weight`:

| Check | Passes when |
| --- | --- |
| `says`, `not_says` | a reply matches (none matches) the pattern, case-insensitive |
| `ticks` | at least `min` replies matching the pattern during a wait (no input, so each is a routine's turn); partial credit below `min` |
| `no_ticks` | no reply during a wait |
| `speaks` | what floppy said out loud in the step (its sounds in the room, transcribed on the speech backend's `/audio/transcriptions`, joined) matches the pattern |
| `replies_at_most` | at most `max` (1) replies in the step |

What is learned shows in what floppy says: a name is the name it gives when
asked, a reflex is the taught reply to the trigger and no reply of it to a
near miss, a routine is replies coming with no input and stopping when
asked, and a name floppy claims is checked as a claim: `not_says`
`\b(i am|i['’]m|my name is|my name['’]s) pixel\b`, matched without case,
so a reply that only mentions the name passes; a claim in another phrasing
is left to the judges. A taught reply is matched as the reply (`^\W*pong\W*$`, `^\W*yellow
alert\W*$`) or as the whole taught phrase, never one word of it. By
convention, in every scenario with a probe after a restart (a `says` or `ticks`
check in a session after the first), that probe carries the largest weight of any
check: it shows the lesson outlasted the session that taught it
(`test_a_probe_after_a_restart_carries_the_largest_weight` holds the
scenarios in `tests/eval/scenarios/` to it).

A scenario's `diagnostics` read floppy's own logs. They say how it got
there and are reported in `report.md` beside the grade, in the "Inside
floppy" table and in each job's "- inside (not scored):" lines,
and the hill climb's proposer reads the failed ones, but they are never
scored: `score.score` raises `ValueError` on a scenario that lists one
under `checks`, and `score.diagnostics` raises on a black-box check listed
under `diagnostics`; `score.score` refuses an unknown kind
(`unknown check: <kind>`) and `score.diagnostics` an unknown diagnostic
(`unknown diagnostic: <kind>`),
so a misfiled or misspelled spec is an error, never a silent change. A
full grade is therefore not proof that floppy learned anything: reasoning
that recites the lesson passes every check. Whether a behavior was accepted
is the `learned` diagnostic, which those "- inside (not scored):" lines show and the
builder track tallies as its own "Learned" rate, a separate number that no
grade or suite score includes:

| Diagnostic | Passes when |
| --- | --- |
| `adopted_name` | `identity.name` is `adopted "<name>"` |
| `recalled_name` | `identity.name` is `recalled "<name>"` in that session (a restarted bot remembered it) |
| `addressed`, `not_addressed` | `identity.heard` is (is not) `addressed as <name>` |
| `learned` | `reflection.inventory` accepts a behavior: `<Name> ACTIVE <lifetime> ... (accept)`, `turn` for a reflex, `persistent` for a routine (the revision logs it for reflection and for a lesson alike) |
| `broken` | `reflection.inventory` breaks one: `<Name> BROKEN (break)`, or the learning ability does: `learning.learned` `lesson of turn N: <Name> … BROKEN` |
| `reflex`, `not_reflex` | `cognition.stage` is (is not) `autonomy handled`: a learned behavior, not reasoning, handled the real input |
| `logged`, `not_logged` | a log record labelled `label` (whose text matches `pattern`, when given) is (is not) in the records |

A reply is a `bot.ability.communication.respond` the turn selected, read
from `floppy.output` (its parts' words, joined). That is what floppy chose
to say, with or without a speech service: `floppy.say` is logged only once
speech encoded the words, and an eval column runs floppy on Sparks alone,
with no hosted speech (decision D216). A turn output the eval cannot read
fails `not_says` and `no_ticks` rather than passing them. The judges read
each reply as `floppy says: …`, each restart as `(floppy is restarted)`
(also before a restarted session none of whose steps ran), a reply the
eval cannot read as `(a reply of floppy's could not be read)`, and a step
that did not settle with a note that whether floppy replied after it is
unknown; the judge prompt counts both markers as unknown, neither silence
nor a reply. Nothing else: no log of floppy's.

Within a session the conversation's history reaches reasoning (decision
D99), so reasoning can answer "ping" with "pong" from the instruction
alone. The grade counts that as a right answer, as a person would; the
`reflex` diagnostic shows whether a learned behavior or reasoning gave it.

## Grading

A job's score is `0.6 × checks + 0.4 × judges` (`--check-weight`): the
weighted mean of its checks and the mean of the judges' overall grades. The
suite score is the mean over scenarios of each one's mean over repeats;
`report.md` also gives each category, each check kind's pass rate, each
judge criterion, and the judges' mean disagreement.

The judges, in `judge.py`'s `JUDGES`, answer JSON (each with fallbacks):

| Judge | Backend | Models |
| --- | --- | --- |
| `codex` | the CPA gateway (`CPA_BASE_URL`, default `https://cpa.willen.dev/v1`; key `CPA_API_KEY`) | `gabe-codex-0/gpt-6.1-sol`, `gabe-codex-1/gpt-6.1-sol`, `gabe-codex-1/gpt-6-sol` |
| `gemini` | the Gemini API (key `FLOPPY_GEMINI_API_KEY`, else `GEMINI_API_KEY`) | `gemini-4-argon`, `gemini-3.8-flash` |

Keys come from the calling environment or the `--judge-env-file` files
(only the variables a judge names are used), go only to their own backend,
and are never printed or written. A key left unset sends no credential,
for a network that injects it (a cloud session's proxy). The judges read
only the conversation `score.summary` builds: what each person said, the time
passing, each restart, what floppy said, each step that never ran, and a
marker where the eval could not read a reply of floppy's or a step did not
settle.

## Feature grade

Each check, diagnostic and scenario names the features it grades
(`features`); `features.py` refuses a feature `features.json` does not
define. A feature's live score is the mean over the scenarios exercising
it of each one's mean over repeats, where a job's score for the feature is
`0.6 × checks + 0.4 × judges` over the checks that name the feature (all
the scenario's checks when none does) and the criteria `criteria_features`
gives it (the judges' overall grade when it gives none). Its letter is A
from 0.9, B from 0.8, C from 0.7, D from 0.6, else F. Beside the grade,
never in it: the feature's diagnostics' pass rate, how many jobs logged
each evidence label of `features.json` (labels that show its parts ran,
and failure labels that should never be logged), and its offline tests
(`cargo test -p floppy-tests --test <name>`, run once each by `eval.py
features`). A feature with no live path (reading, which nothing invokes,
and the api.willen.dev emotion, voiceprint and streaming endpoints floppy
does not call) carries a `status` and is listed ungraded.

`eval.py features` runs the offline tests (each target once; about 8 minutes each on four cores, so `--no-tests` skips them and `eval.py tests <run>` then `eval.py grade <run>` adds them later) and every scenario, and writes
`features.md` and `features.json`; `eval.py run` and `eval.py grade`
write them too, without the tests.

## Questions

[`tests/eval/questions.json`](../tests/eval/questions.json) holds the
maintainer's questions ("what does floppy learn, what does floppy remember,
what does floppy understand, what does floppy decide to do, does floppy
memory work well, does floppy know it's own voice and does it naturally
learn others"), each with the scenarios that answer it and the log labels
that show what floppy did inside. A question's score is the mean of its
scenarios' scores, answered `yes` from 0.8, `partly` from 0.5, else `no`.
Under each answer `questions.md` quotes every job's transcript, the
judges' reasons, the texts of the question's log labels (what floppy
stored, recalled, built, or failed to decode), and for "what does floppy
decide" the events its turns selected. The logs are evidence beside the
answer, never in it (D241). Every grade writes it beside `features.md`.

`eval.py merge --out DIR RUN...` joins runs (a run of new scenarios beside
a full one) into one directory for `eval.py grade`.

## Eval health

The eval follows the health checklist of Anthropic's eval guides (the
`claude-api` skill's `build-eval` and `hillclimb`, 2026-09-28):

- **The rig is not floppy.** A job the driver did not finish, in which no
  step ran, or in which floppy never activated in a session (a backend or
  the room down before any input) has an infra error
  (`score.infra_error`): it is listed apart in `report.md` and
  `features.md` and kept out of every score. A step that never settled
  stays scored: a stuck turn is floppy's.
- **Error bars.** The suite score and each feature's live score carry
  the half-width of a normal 95% interval over their job scores; a
  difference smaller than it is noise. One repeat per scenario leaves most
  features on one to three jobs, so `--repeats 3` is the setting to act
  on a difference.
- **Known negatives.** `eval.py judge-check` grades each scenario's
  transcript with floppy silent at every step and with floppy saying "I
  don't know." at every step, with each judge, and lists every overall
  grade above `--ceiling` (0.3). The first check (2026-10-25) passed 73
  of 76; the three above were partial credit for steps floppy rightly
  left quiet (`chat_ignore`) and `turn_failure`, whose first step accepts
  any reply by design; its greeting check now asks for a greeting, so "I
  don't know." no longer passes it.
- **Held-out test set.** `climb` splits its scenarios into train and test
  (`--test-share` 0.34 of each category, seeded by `--seed`, never by
  score): candidates are scored and the proposer reads failures on train
  only, and each accepted winner is run once on test, the climb's
  headline in `climb.md`.

## Hill climbing

`eval.py climb` changes prompt `<data>` of the bot's documents, by default
the four that decide what floppy learns and whether it is built
(`prompts.DEFAULT_IDS`, each found in whichever document declares it, so a prompt that moves to another document stays a target):

- `abilities/reflection/reflection.scx#select_instructions` (whether a turn
  selects a create, change or break at all),
- `abilities/learning/learning.scx#learn_instructions` (which create or
  change a lesson someone taught becomes, decision D149),
- `abilities/revision/revision.scx#change_instructions` (the spec the
  builder gets),
- `bots/builder/builder.scx#author_system` (how the builder writes SCXML).

By default the climb runs the learning scenarios (`name`, `reflex`,
`routine`), split into train and test as above. `eval.py targets` lists every prompt `<data>` it can change (`--target
<path>#<id>` picks others). Each generation takes the next target in turn,
asks the proposers (`codex` and `gemini`, alternating, `--candidates` per
generation) for a rewrite from the incumbent's failures (failed checks,
the judges' main failure, reflection's and the builder's logs, on train
only) and the earlier attempts on that target, writes each candidate as an overlay
(`prompts.overlay`: the changed documents in a directory laid out like the
workspace, which the driver copies over each job's scratch bot), and runs
the train scenarios on every candidate at once. The best is run again on
train to confirm (`--no-confirm` skips it) and on test, and replaces the
incumbent when the mean of both train runs beats it by `--min-gain`
(0.02); its test score is recorded beside it. The climb stops after
`--generations` or `--budget-hours`, and resumes from its `climb.json` when
given the same `--out`.

It writes `climb.md` (each generation's target, proposer, score and
rationale), `best.patch` (the winning prompts as a diff of the workspace,
for review; apply with `git apply`) and `best/overlay/`. A winner is a
proposal: it changes the bot only when a maintainer applies it, through
review, like any change.

`eval.py expand` asks the proposers for held-out variations of the
scenarios (new names, triggers, replies and probes), checked to load, so a
climb's winner can be run (`eval.py run --scenarios-dir <dir>`) on input it
never climbed on.

## Builder track

The maintainer's bar for the builder (2026-10-22): it must succeed with
small models, which may only take longer and more attempts. `eval.py
builder` measures that where the builder really runs, inside reflection:
it runs the reflex and routine scenarios once per `--column
label=builder_model[;env=<env files>][;bot=<model>]`, each with
`bots/builder/builder.scx` pinned to the column's model in an overlay (its
`<assign location="model">` takes a literal instead of the request's
model), so the bot reasons and reflects on its own model (`bot`, else
`FLOPPY_LIVE_MODEL`) and builds on the column's. The builder model must be
served by the column's `FLOPPY_BASE_URL` (its `env` files, else
`FLOPPY_LIVE_ENV_FILE`).

`score.builds` reads each build from the builder's own logs, followed by
each record's `session` since reflection's and the learning ability's
builders can run at once (`author`
attempt 1, or a `design` that follows neither `triage` nor `retry_gate`, starts one, and any
other builder line with no build open opens one; `done` "built X in N attempt(s)" or `failed`
"<reason>: ..." ends it; a build its turn cancelled stays `unfinished`, and
one cut before any draft or failure (or whose log ended there, at the job's end) is counted apart as "cut before a draft", since it
measures the turn, not the builder; the 8-failure budget also counts a
failed design, so a build can exhaust it in fewer than 8 attempts),
and `builder.md` gives, per column and scenario, the builds, the accepted
ones, the success rate, the attempts per accepted build and whether the
scenario's behavior was learned anywhere in the job, then per column the
failed builds by reason and first diagnostic (`attempts-exhausted (author-failed
schema-mismatch)`, `attempts-exhausted ...`, `unfinished`). A job counts
as unfinished in `report.md` when any step never ran or never settled. For example, on the CPA
gateway with the decision server on the lab:

```sh
python3 tests/eval/eval.py builder --judges none --repeats 2 \
  --column "sparks=sparks/qwen3.8-flash-next;env=.env.cpa-lab;bot=sparks/qwen3.8-flash-next"
```

Sparks is the bar (the maintainer, 2026-10-24: "try it on sparks if that
model can do it i'm fine with that being the bar"; the builder must work on
a small model, taking more attempts if it must). Everything inside floppy's
framework runs on it, the bot and the builder, with no hosted speech; hosted
models only judge or play the person ("i'm fine with using that as a judge /
person just not within the framework"; decision D216).

## Running it

From a cloud session `scx` does not use the session's proxy, so each
backend is a local relay (`tests/eval/relay.py PORT https://upstream`, the
proxy adding each credential) named in the env file: CPA for
`FLOPPY_BASE_URL`, TypeSafe for `TYPESAFE_BASE_URL`, and api.willen.dev for
both `FLOPPY_SPEECH_BASE_URL` and `FLOPPY_VOICE_BASE_URL`, each key a
placeholder. The eval's own calls (the judges, `sound.py`) are Python and
use the proxy directly:

```sh
python3 tests/eval/relay.py 18081 https://cpa.willen.dev &
python3 tests/eval/relay.py 18082 https://api.typesafe.ai &
python3 tests/eval/relay.py 18085 https://api.willen.dev &
export FLOPPY_LIVE_ENV_FILE=live.env FLOPPY_LIVE_MODEL=sparks/qwen3.8-flash-next
python3 tests/eval/eval.py features --parallel 4
```

The bot's backends are the live tests' (`FLOPPY_LIVE_ENV_FILE`,
`FLOPPY_LIVE_MODEL`; floppy_live's module comment). On Sparks through the
CPA gateway, with the systemone decision server on the maintainer's lab
(`192.168.2.2:8102`, docs/live-proof.md 2026-10-21, reachable from the
maintainer's network only), `.env.cpa-lab` as in the builder example above. The judges' hosted models
take their keys from `--judge-env-file` (see Grading); nothing else in a
run uses a hosted model:

```sh
export FLOPPY_LIVE_ENV_FILE=.env.cpa-lab FLOPPY_LIVE_MODEL=sparks/qwen3.8-flash-next
python3 tests/eval/eval.py run --judge-env-file .env           # the suite once
python3 tests/eval/eval.py run --scenario reflex --repeats 3   # one category
python3 tests/eval/eval.py climb --judge-env-file .env --generations 6 --budget-hours 8
python3 tests/eval/eval.py grade target/floppy-eval/run-<time>  # grade again (a builder run: its table too)
python3 -m unittest discover -s tests/eval                     # offline tests
```

`--parallel` (4) bot runs at a time. A scenario takes 5 to
15 minutes (a said step waits for its reflection and any builder revision,
at most `timeout_s`, 15 minutes by default; a wait step its seconds), so
the suite takes about half an hour at four at a time and a climb
generation of two candidates about an hour (estimates).

Every run writes, under `target/floppy-eval/<run>/`: `plan.json`,
`driver.log` (the driver's output; no value is printed), per job
`jobs/<id>/session-<n>.json` (the records), `steps.json`, `transcript.txt`
(what the judges read) and `grade.json`, and `report.json` and
`report.md` for the run.
