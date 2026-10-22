# Learning eval

The learning eval grades how well the floppy bot learns from what people
tell it, on real input: its **name** (the identity ability adopts a name,
answers to it, and recalls it after a restart), its **reflexes** (turn
behaviors reflection learns and autonomy then runs on matching input, the
reference's reflex tier) and its **routines** (persistent behaviors that act
on a schedule with no input). It exists because the live runs so far
(docs/live-proof.md, 2026-10-20 and 2026-10-21) showed reflection rarely
learns a behavior, and no test checked a learned behavior against the input
it should handle (docs/handoff.md, "Backlog": the builder's simulation drives
no input).

It has three parts:

- **The driver**, `tests/tests/learning_eval.rs`, an ignored test like
  `floppy_live`: it runs the bot in the room on real backends through the
  steps of each scenario of a plan, several at a time, and writes every
  run's records. It asserts nothing about the bot.
- **The grade**, `tests/eval/score.py` and `tests/eval/judge.py`: each
  scenario's checks read the bot's own logs, and two LLM judges (a CPA
  `gabe-codex-*` model and Gemini) grade the transcript against the
  scenario's criteria.
- **The hill climb**, `tests/eval/eval.py climb`: an LLM proposes a rewrite
  of one learning prompt from the failures, the eval scores it, and the best
  is kept, one prompt per generation.

The runner is `tests/eval/eval.py` (Python 3, standard library only). It
never edits the workspace: everything goes under `target/floppy-eval/`.

## Scenarios

`tests/eval/scenarios/<name>.json`, one per scenario:

| Scenario | Category | What it teaches, then probes |
| --- | --- | --- |
| `name_adopt` | name | "your name is Juniper"; addressed as Juniper, not as Pixel; restarted, recalls and answers to Juniper |
| `name_rename` | name | Juniper, then Biscuit; answers to Biscuit; restarted, still Biscuit |
| `reflex_pingpong` | reflex | "whenever I say exactly 'ping', reply 'pong'"; two pings, an unrelated question, a ping after a restart |
| `reflex_greeting` | reflex | "good morning" gets "Good morning, sunshine!"; a variation from another person; "good night" must not |
| `reflex_keyword` | reflex | any mention of bananas gets "yellow alert", inside longer sentences, also after a restart |
| `routine_reminder` | routine | a water reminder every minute; reminders with no input; stopped when asked; quiet after |
| `routine_restart` | routine | "stretch break" every minute; keeps running after a restart with no new request |

The name is Juniper, not floppy, because the reasoning prompt already calls
the bot floppy: a test of learning a name must use one the prompts do not
hold.

A scenario is `{category, intent, bot_name, sessions, checks, criteria}`.
Each session is the bot started again on the same memory and behaviors
directories (a restart with what it learned). A step is `{"say": text,
"source"?: id, "timeout_s"?: n}`, one `bot.ability.communication.input`
waited on until its reflection settles (or its turn fails) and then 15 s more, since its reply can be said after its reflection settles, or `{"wait_s":
n}`, the wall clock passing with no input. `criteria` are what the judges
score, 0 to 10 each.

### Checks

Each check reads a log label the bot already writes, at one step (`at:
[session, step]`) or up to the end of one (`by`), and scores 0 to 1 with a
`weight`:

| Check | Passes when |
| --- | --- |
| `adopted_name` | `identity.name` is `adopted "<name>"` |
| `recalled_name` | `identity.name` is `recalled "<name>"` in that session (a restarted bot remembered it) |
| `addressed`, `not_addressed` | `identity.heard` is (is not) `addressed as <name>` |
| `learned` | `reflection.inventory` accepts a behavior: `<Name> ACTIVE <lifetime> ... (accept)`, `turn` for a reflex, `persistent` for a routine |
| `broken` | `reflection.inventory` breaks one: `<Name> BROKEN (break)` |
| `reflex`, `not_reflex` | `cognition.stage` is (is not) `autonomy handled`: a learned behavior, not reasoning, handled the real input |
| `says`, `not_says` | a `floppy.say` matches (none matches) the pattern, case-insensitive |
| `ticks` | at least `min` `floppy.say` matching the pattern during a wait (no input, so each is a routine's turn); partial credit below `min` |
| `no_ticks` | no `floppy.say` during a wait |

`reflex` is the check that exposes the gap: the conversation's history
reaches reasoning (decision D99), so reasoning can answer "ping" with
"pong" from the instruction alone. That is the bot following the
conversation, not a reflex it learned; only `autonomy handled` is.

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
only the step summary `score.summary` builds from the bot's logs.

## Hill climbing

`eval.py climb` changes prompt `<data>` of the bot's documents, by default
the three that decide what floppy learns and whether it is built
(`prompts.DEFAULT_IDS`, each found in whichever document declares it, so a prompt that moves to another document stays a target):

- `abilities/reflection/reflection.scx#select_instructions` (whether a turn
  selects a create, change or break at all),
- `abilities/reflection/reflection.scx#change_instructions` (the spec the
  builder gets),
- `bots/builder/builder.scx#author_system` (how the builder writes SCXML).

`eval.py targets` lists every prompt `<data>` it can change (`--target
<path>#<id>` picks others). Each generation takes the next target in turn,
asks the proposers (`codex` and `gemini`, alternating, `--candidates` per
generation) for a rewrite from the incumbent's failures (failed checks,
the judges' main failure, reflection's and the builder's logs) and the
earlier attempts on that target, writes each candidate as an overlay
(`prompts.overlay`: the changed documents in a directory laid out like the
workspace, which the driver copies over each job's scratch bot), and runs
the suite on every candidate at once. The best is run again to confirm
(`--no-confirm` skips it) and replaces the incumbent when the mean of both
runs beats it by `--min-gain` (0.02). The climb stops after
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

## Running it

The bot's backends are the live tests' (`FLOPPY_LIVE_ENV_FILE`,
`FLOPPY_LIVE_MODEL`; floppy_live's module comment). On the maintainer's lab
(mosfet's stack, docs/live-proof.md 2026-10-21: `gemma-4-26b-a4b` on the
vLLM at `192.168.2.2:8100`, the systemone decision server at
`192.168.2.2:8102`, reachable from the maintainer's network only), with
`env.lab` and `env.speech` as there:

```sh
export FLOPPY_LIVE_ENV_FILE=env.lab:env.speech FLOPPY_LIVE_MODEL=gemma-4-26b-a4b
python3 tests/eval/eval.py run --judge-env-file .env           # the suite once
python3 tests/eval/eval.py run --scenario reflex --repeats 3   # one category
python3 tests/eval/eval.py climb --judge-env-file .env --generations 6 --budget-hours 8
python3 tests/eval/eval.py grade target/floppy-eval/run-<time>  # grade again
python3 -m unittest discover -s tests/eval                     # offline tests
```

`--parallel` (4) bot runs share the lab at a time (its four L4s serve the
vLLM). A scenario takes 5 to 15 minutes (a said step waits for its
reflection and any builder revision, at most `timeout_s`, 15 minutes by
default; a wait step its seconds), so the suite takes about half an hour
at four at a time and a climb generation of two candidates about an hour.

Every run writes, under `target/floppy-eval/<run>/`: `plan.json`,
`driver.log` (the driver's output; no value is printed), per job
`jobs/<id>/session-<n>.json` (the records), `steps.json`, `transcript.txt`
(what the judges read) and `grade.json`, and `report.json` and
`report.md` for the run.
