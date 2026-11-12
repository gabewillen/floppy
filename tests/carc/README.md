# Continual-ARC on floppy

[Continual-ARC](https://github.com/unifyai/continual-arc) asks whether a learner
gets better at work that recurs: a stream of ARC grid puzzles whose tasks come
back after gaps with fresh inputs, each instance answered by submitting a grid
(a wrong one costs 1), buying a demonstration (costs 1) or giving up (costs the
whole 16). Every measure reads the learner against itself wiped before every
instance, so only what it keeps counts, not how strong its model is.

`floppy_learner.py` is floppy as that learner: one floppy in its environment
under `scx run`, every benchmark message typed to it as text from `gabe`, its
reply read from the turn's `floppy.output` (floppy speaks, so it says its
action and its grid as rows of digits instead of writing JSON), each turn waited on until it and its
lessons settle. What floppy keeps is its memory and its behaviors; the reset
deletes both. Every model call goes to the hosted gateway
(`FLOPPY_REFLECTION_BASE_URL` when an env file sets it, else `FLOPPY_BASE_URL`;
`FLOPPY_CARC_ENV=.env:.env.cpa` takes the decision key from `.env` and the
gateway from `.env.cpa`), with one model, `FLOPPY_CARC_MODEL` (default
`gpt-6-luna`, the model of the paper's harness rows); the lab's embedding,
speech, voice and emotion backends are left unconfigured. The module docstring
has the details.

## Running it

From the floppy checkout, with `scx` built as for the eval
([docs/running.md](../../docs/running.md)) and `uv` on `PATH`:

```console
sh tests/carc/run.sh probe    # one round: does a grid make it through a reply?
sh tests/carc/run.sh stream   # the arc1-smoke stream (120 instances)
sh tests/carc/run.sh reset    # the same stream, floppy wiped before every instance
sh tests/carc/run.sh learned  # the share learned, stream against reset
```

`run.sh` clones continual-arc at a pinned commit into `target/floppy-carc/`,
installs it and its baselines (for the text protocol), fetches its data, and
writes every run there: the benchmark's run directory (`records.jsonl`,
`summary.json`, `events.jsonl`) and beside it floppy's work directory
(`turns.jsonl`, each message and reply; `floppy.log`, every record; its memory
and behaviors). The plain `continual-arc run` does not resume, so a stopped
stream starts again.
