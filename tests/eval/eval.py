"""floppy's eval: grades every feature of the floppy bot on real input
(tests/eval/features.json: conversation, memory, people, persona, hearing,
turn taking, speaking, voices, seeing, failed turns, its name, reflexes and
routines, ...), and hill-climbs the prompts that decide what it learns.
docs/eval.md is the guide; this is the runner.

Usage: python3 tests/eval/eval.py COMMAND [options]

  run      run scenarios (tests/eval/scenarios/*.json) through the bot,
           score them, have the judges grade them, and grade each feature
  features `run` over every scenario, with each feature's offline tests
           (`cargo test --test <name>`) run first: the full feature grade
  tests    each feature's offline tests alone, into a run directory
           (`grade` it again to see them in its features.md)
  grade    score and judge a run directory again (a run made elsewhere)
  climb    hill-climb prompt <data> (prompts.DEFAULT_IDS, or --target):
           an LLM proposes a rewrite from the failures, the eval scores it,
           and the best is kept
  expand   have an LLM write held-out variations of the scenarios (new
           wordings, names, triggers and probes) for checking a climb's
           winner on input it never saw
  builder  the builder track: the reflex and routine scenarios once per
           --column, each with the builder pinned to one model (the bot
           keeps its own), reporting each model's build success rate and
           attempts per accepted build
  judge-check  grade each scenario's known-negative transcripts (floppy
           silent, or "I don't know." at every step) with each judge: a
           judge that passes them is too lenient to steer a climb
  targets  list every prompt <data> the climb can change

The bot runs through tests/tests/floppy_eval.rs (`cargo test`), which
needs FLOPPY_LIVE_ENV_FILE (the bot's backends, as for floppy_live) and
FLOPPY_LIVE_MODEL. The judges, and the speech backend that speaks for the
people and hears floppy (tests/eval/sound.py), read their keys from the
environment or from --judge-env-file; nothing here prints a key. Everything is written under
target/floppy-eval/ (gitignored, like target/floppy-live/).
"""

import argparse
import concurrent.futures
import datetime
import glob
import json
import os
import shutil
import random
import statistics
import subprocess
import sys
import time

import features
import judge as judges
import prompts
import questions
import score
import sound

ROOT = prompts.ROOT
SCENARIOS = os.path.join(ROOT, "tests", "eval", "scenarios")
OUT = os.path.join(ROOT, "target", "floppy-eval")

# What each default prompt (by <data> id) decides, for the proposer.
USAGE = {
    "select_instructions": (
        "reflection's select call after every turn: it chooses at most one of bot.behavior.create, change or break "
        "(or nothing) from the turn, prior episodes and the behavior inventory. If it selects nothing, nothing is learned."
    ),
    "learn_instructions": (
        "the learning's select call for a lesson someone taught the bot: it chooses exactly one of bot.behavior.create "
        "or change from the lesson, the turn it was taught on, prior episodes and the behavior inventory. If it selects "
        "nothing, the lesson is not learned."
    ),
    "change_instructions": (
        "the spec the revision gives the builder bot for a create or change (reflection's, or a lesson's with the "
        "lesson as its instruction): the builder writes the behavior's SCXML from it, validates it and simulates it; a draft that fails repeatedly is given up after max_attempts (attempts-exhausted)."
    ),
    "author_system": (
        "the builder's author stage system prompt: it writes one complete SCXML document from the plan and spec; "
        "validation diagnostics such as XML001, XSD errors, no-document, script-not-loaded and truncated mean the "
        "draft was rejected."
    ),
}


def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def log(message):
    print(f"[eval {datetime.datetime.now().strftime('%H:%M:%S')}] {message}", flush=True)


def load_scenarios(names=None, directory=SCENARIOS):
    found = {}
    for path in sorted(glob.glob(os.path.join(directory, "*.json"))):
        with open(path, encoding="utf-8") as handle:
            scenario = json.load(handle)
        name = os.path.splitext(os.path.basename(path))[0]
        scenario["name"] = name
        if names is None or name in names or scenario["category"] in names:
            found[name] = scenario
    if names and not found:
        raise SystemExit(f"no scenario matches {names}")
    features.check_names(found, FEATURES)
    return found


def judge_env(args):
    env = dict(os.environ)
    if args.judge_env_file:
        env.update(judges.read_env_files(args.judge_env_file))
    return env


FEATURES = features.load()
QUESTIONS = questions.load()

# The variables the judges and the speech backend read (judge_env), set
# once by main: drive and grade_job are called from every command.
ENV = {}


# Running the bot


def drive(jobs, out, parallel, dry=False):
    """Runs `jobs` ([{id, scenario, overlay, out}]) through the driver test
    and waits for it; the driver's output goes to `<out>/driver.log`."""
    os.makedirs(out, exist_ok=True)
    backend = sound.Backend(ENV)
    cache = os.path.join(OUT, "voices")
    for job in jobs:
        # A heard step's utterance is synthesized before the bot runs, and
        # a shown step's image is named by its absolute path.
        job["scenario"] = sound.prepare(job["scenario"], job["out"], None if dry else backend, cache)
    plan = os.path.join(out, "plan.json")
    with open(plan, "w", encoding="utf-8") as handle:
        # The driver pops jobs from the end, so reversed runs them in order.
        json.dump({"parallel": parallel, "jobs": list(reversed(jobs))}, handle, indent=2)
    if dry:
        return 0
    command = [
        os.environ.get("CARGO", "cargo"), "test", "-p", "floppy-tests", "--locked", "--test", "floppy_eval",
        "--", "--ignored", "--nocapture", "--exact", "floppy_eval",
    ]
    env = dict(os.environ, FLOPPY_EVAL_PLAN=plan)
    if "FLOPPY_LIVE_ENV_FILE" not in env:
        raise SystemExit("set FLOPPY_LIVE_ENV_FILE to the bot's env files (docs/eval.md)")
    log(f"driving {len(jobs)} jobs, {parallel} at a time (log: {os.path.relpath(out, ROOT)}/driver.log)")
    with open(os.path.join(out, "driver.log"), "w", encoding="utf-8") as handle:
        status = subprocess.call(command, cwd=ROOT, env=env, stdout=handle, stderr=subprocess.STDOUT)
    log(f"driver finished with status {status}")
    return status


def make_jobs(scenarios, repeats, out, overlay=None, prefix="", extra=None):
    jobs = []
    for name, scenario in scenarios.items():
        for repeat in range(repeats):
            job_id = f"{prefix}{name}-r{repeat}"
            job = {"id": job_id, "scenario": scenario, "overlay": overlay, "out": os.path.join(out, "jobs", job_id)}
            job.update(extra or {})
            jobs.append(job)
    return jobs


# Grading


def grade_job(job, env, judge_names, check_weight):
    """The job's checks, the judges' grades, and its combined score."""
    scenario = job["scenario"]
    records = score.load_job(job["out"])
    # What floppy said aloud, transcribed once (kept in heard.json).
    groups = score.sound_groups(scenario, records)
    if groups:
        records["heard"] = sound.transcripts(job["out"], groups, sound.Backend(env))
    checks, results = score.score(scenario, records)
    transcript = score.summary(scenario, records)
    all_records = [r for session in records["sessions"] for r in session]
    grades = []
    if judge_names:
        for backend in judges.backends(env, judge_names):
            try:
                grades.append(judges.judge(backend, scenario, transcript))
            except RuntimeError as error:
                grades.append({"judge": backend.name, "error": str(error)})
    overalls = [g["overall"] for g in grades if "overall" in g]
    judged = statistics.mean(overalls) if overalls else None
    combined = checks if judged is None else check_weight * checks + (1 - check_weight) * judged
    graded = {
        "id": job["id"],
        "scenario": scenario["name"],
        "category": scenario["category"],
        "done": records["done"],
        "checks": checks,
        "judged": judged,
        "score": combined,
        "results": results,
        "diagnostics": score.diagnostics(scenario, records),
        "grades": grades,
        "evidence": score.evidence(records),
        "builds": score.builds(records),
        "complete": score.complete(scenario, records),
        "learned_any": score.learned_any(scenario, records),
        "seen": features.seen(records, FEATURES),
        "error": score.infra_error(scenario, records),
        # The question report's evidence (questions.py).
        "transcript": transcript,
        "inside": questions.inside(all_records, questions.labels(QUESTIONS)),
        "selected": questions.selected(all_records),
    }
    # A job that crashed before writing anything has no directory yet.
    os.makedirs(job["out"], exist_ok=True)
    with open(os.path.join(job["out"], "grade.json"), "w", encoding="utf-8") as handle:
        json.dump(graded, handle, indent=2)
    with open(os.path.join(job["out"], "transcript.txt"), "w", encoding="utf-8") as handle:
        handle.write(transcript + "\n")
    return graded


def grade_all(jobs, env, judge_names, check_weight):
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        return list(pool.map(lambda job: grade_job(job, env, judge_names, check_weight), jobs))


def aggregate(graded):
    """The suite score (the mean over scenarios of each one's mean over its
    repeats), per category, per check kind and per judge criterion, with
    its 95% interval. A job with an infra error (score.infra_error) is
    counted apart and kept out of every number."""
    errors = [g for g in graded if g.get("error")]
    graded = [g for g in graded if not g.get("error")]
    by_scenario = {}
    for g in graded:
        by_scenario.setdefault(g["scenario"], []).append(g)
    scenarios = {name: statistics.mean(x["score"] for x in runs) for name, runs in by_scenario.items()}
    categories = {}
    for name, runs in by_scenario.items():
        categories.setdefault(runs[0]["category"], []).append(scenarios[name])
    checks = {}
    diagnostics = {}
    criteria = {}
    for g in graded:
        for r in g["results"]:
            checks.setdefault(r["check"], []).append(r["score"])
        for r in g["diagnostics"]:
            diagnostics.setdefault(r["check"], []).append(r["score"])
        for grade in g["grades"]:
            for name, value in grade.get("criteria", {}).items():
                criteria.setdefault(f"{g['category']}.{name}", []).append(value)
    agreement = []
    for g in graded:
        overalls = [x["overall"] for x in g["grades"] if "overall" in x]
        if len(overalls) == 2:
            agreement.append(abs(overalls[0] - overalls[1]))
    return {
        "score": statistics.mean(scenarios.values()) if scenarios else 0.0,
        "categories": {k: statistics.mean(v) for k, v in sorted(categories.items())},
        "scenarios": dict(sorted(scenarios.items())),
        "checks": {k: statistics.mean(v) for k, v in sorted(checks.items())},
        "diagnostics": {k: statistics.mean(v) for k, v in sorted(diagnostics.items())},
        "criteria": {k: statistics.mean(v) for k, v in sorted(criteria.items())},
        "judge_disagreement": statistics.mean(agreement) if agreement else None,
        "interval": features.interval([g["score"] for g in graded]),
        "jobs": len(graded),
        "errors": [{"id": g["id"], "error": g["error"]} for g in errors],
        # A scored job is unfinished when any of its steps never settled or ran.
        "unfinished": sum(1 for g in graded if not (g["done"] and g.get("complete", True))),
    }


def report_markdown(title, summary, graded):
    interval = "" if summary.get("interval") is None else f" ± {summary['interval']:.3f} (95%)"
    lines = [f"# {title}", "", f"Suite score: **{summary['score']:.3f}**{interval} over {summary['jobs']} jobs ({summary['unfinished']} unfinished)."]
    for e in summary.get("errors", []):
        lines.append(f"- not scored, infra error: {e['id']}: {e['error']}")
    if summary["judge_disagreement"] is not None:
        lines.append(f"Mean judge disagreement (|codex - gemini| overall): {summary['judge_disagreement']:.2f}.")
    lines += ["", "| Category | Score |", "| --- | --- |"]
    lines += [f"| {k} | {v:.3f} |" for k, v in summary["categories"].items()]
    lines += ["", "| Check | Pass rate |", "| --- | --- |"]
    lines += [f"| {k} | {v:.2f} |" for k, v in summary["checks"].items()]
    if summary["criteria"]:
        lines += ["", "| Judge criterion | Mean (0-1) |", "| --- | --- |"]
        lines += [f"| {k} | {v:.2f} |" for k, v in summary["criteria"].items()]
    if summary["diagnostics"]:
        lines += ["", "Inside floppy (its own logs; not scored):", "", "| Diagnostic | Pass rate |", "| --- | --- |"]
        lines += [f"| {k} | {v:.2f} |" for k, v in summary["diagnostics"].items()]
    lines += ["", "## Jobs", ""]
    for g in sorted(graded, key=lambda x: x["id"]):
        if g.get("error"):
            lines += [f"### {g['id']}: not scored ({g['error']})", ""]
            continue
        judged = "n/a" if g["judged"] is None else f"{g['judged']:.2f}"
        lines.append(f"### {g['id']}: {g['score']:.3f} (checks {g['checks']:.2f}, judges {judged})")
        lines.append("")
        for r in g["results"]:
            mark = "pass" if r["score"] >= 1 else ("part" if r["score"] > 0 else "fail")
            lines.append(f"- {mark} `{r['check']}` at {r['where']}: {r['why'][:240]}")
        for r in g["diagnostics"]:
            mark = "yes" if r["score"] >= 1 else "no"
            lines.append(f"- inside (not scored): {mark} `{r['check']}` at {r['where']}: {r['why'][:240]}")
        for grade in g["grades"]:
            if "error" in grade:
                lines.append(f"- judge {grade['judge']}: {grade['error']}")
            else:
                lines.append(f"- judge {grade['judge']} ({grade['model']}) {grade['overall']:.1f}: {grade['explanation']}")
        lines.append("")
    return "\n".join(lines) + "\n"


def write_features(out, scenarios, graded, check_weight, tests=None):
    """features.json and features.md of a run: each feature's grade."""
    rows = features.grade(FEATURES, scenarios, graded, check_weight, tests)
    summary = aggregate(graded)
    with open(os.path.join(out, "features.json"), "w", encoding="utf-8") as handle:
        json.dump({"features": rows, "tests": tests}, handle, indent=2)
    with open(os.path.join(out, "features.md"), "w", encoding="utf-8") as handle:
        handle.write(features.markdown(f"floppy feature grade {os.path.basename(out)}", rows, summary))
    answered = questions.answer_all(QUESTIONS, graded)
    with open(os.path.join(out, "questions.json"), "w", encoding="utf-8") as handle:
        json.dump([{k: v for k, v in r.items() if k != "jobs"} | {"jobs": [g["id"] for g in r["jobs"]]}
                   for r in answered], handle, indent=2)
    with open(os.path.join(out, "questions.md"), "w", encoding="utf-8") as handle:
        handle.write(questions.markdown(f"What floppy does: {os.path.basename(out)}", answered))
    return rows


def evaluate(scenarios, args, out, overlay=None, prefix=""):
    jobs = make_jobs(scenarios, args.repeats, out, overlay, prefix)
    if getattr(args, "dry", False):
        drive(jobs, out, args.parallel, dry=True)
        return None, []
    drive(jobs, out, args.parallel)
    graded = grade_all(jobs, judge_env(args), args.judge_names, args.check_weight)
    summary = aggregate(graded)
    with open(os.path.join(out, "report.json"), "w", encoding="utf-8") as handle:
        json.dump({"summary": summary, "jobs": graded}, handle, indent=2)
    with open(os.path.join(out, "report.md"), "w", encoding="utf-8") as handle:
        handle.write(report_markdown(f"floppy eval {os.path.basename(out)}", summary, graded))
    write_features(out, scenarios, graded, args.check_weight, getattr(args, "tests_results", None))
    log(f"{os.path.relpath(out, ROOT)}: score {summary['score']:.3f} {summary['categories']}")
    return summary, graded


# Commands


def cmd_run(args):
    scenarios = load_scenarios(args.scenario, args.scenarios_dir)
    out = args.out or os.path.join(OUT, "run-" + now())
    evaluate(scenarios, args, out, overlay=os.path.abspath(args.overlay) if args.overlay else None)
    print(os.path.join(out, "report.md"))


def cmd_grade(args):
    with open(os.path.join(args.dir, "plan.json"), encoding="utf-8") as handle:
        jobs = json.load(handle)["jobs"]
    # A builder track's run: its table again, from the models it saved.
    # Checked before anything is written: a run without them fails at once.
    builder_json = os.path.join(args.dir, "builder.json")
    models = None
    if os.path.exists(builder_json):
        with open(builder_json, encoding="utf-8") as handle:
            models = json.load(handle).get("models")
        if not models or not isinstance(models, dict):
            sys.exit(f"{builder_json} names no column models (a run from before they were saved): "
                     "it cannot be regraded")
    for job in jobs:
        # A run made on another machine: its out directories moved with it.
        job["out"] = os.path.join(args.dir, "jobs", job["id"])
    graded = grade_all(jobs, judge_env(args), args.judge_names, args.check_weight)
    summary = aggregate(graded)
    with open(os.path.join(args.dir, "report.json"), "w", encoding="utf-8") as handle:
        json.dump({"summary": summary, "jobs": graded}, handle, indent=2)
    with open(os.path.join(args.dir, "report.md"), "w", encoding="utf-8") as handle:
        handle.write(report_markdown(f"floppy eval {os.path.basename(args.dir)}", summary, graded))
    tests = None
    if os.path.exists(os.path.join(args.dir, "tests.json")):
        with open(os.path.join(args.dir, "tests.json"), encoding="utf-8") as handle:
            tests = json.load(handle)
    write_features(args.dir, {j["scenario"]["name"]: j["scenario"] for j in jobs}, graded, args.check_weight, tests)
    if models is not None:
        write_builder(args.dir, [{"label": label, "builder": model} for label, model in models.items()], graded)
    print(json.dumps(summary, indent=2))


def cmd_merge(args):
    """One run directory from several (a later run of new scenarios beside
    a full one): every job of each, copied, under one plan; `eval.py grade`
    then grades it whole. A job id in two runs keeps the later run's."""
    jobs = {}
    for directory in args.dirs:
        with open(os.path.join(directory, "plan.json"), encoding="utf-8") as handle:
            for job in json.load(handle)["jobs"]:
                job["from"] = os.path.join(directory, "jobs", job["id"])
                jobs[job["id"]] = job
    os.makedirs(os.path.join(args.out, "jobs"), exist_ok=True)
    for job in jobs.values():
        target = os.path.join(args.out, "jobs", job["id"])
        if os.path.exists(target):
            shutil.rmtree(target)
        shutil.copytree(job.pop("from"), target)
        job["out"] = target
    with open(os.path.join(args.out, "plan.json"), "w", encoding="utf-8") as handle:
        json.dump({"jobs": list(jobs.values())}, handle, indent=2)
    if os.path.exists(os.path.join(args.dirs[0], "tests.json")):
        shutil.copy(os.path.join(args.dirs[0], "tests.json"), os.path.join(args.out, "tests.json"))
    print(os.path.join(args.out, "plan.json"))


def offline_tests(out, names):
    """Runs each offline test target once (`cargo test -p floppy-tests
    --test <name>`), each output in `<out>/tests/<name>.log`; returns
    {name: {passed, failed, ignored, status}}, also written to tests.json."""
    os.makedirs(os.path.join(out, "tests"), exist_ok=True)
    results = {}
    for name in sorted(names):
        log(f"offline tests: {name}")
        path = os.path.join(out, "tests", f"{name}.log")
        with open(path, "w", encoding="utf-8") as handle:
            status = subprocess.call(
                [os.environ.get("CARGO", "cargo"), "test", "-p", "floppy-tests", "--locked", "--test", name],
                cwd=ROOT, stdout=handle, stderr=subprocess.STDOUT)
        with open(path, encoding="utf-8") as handle:
            passed, failed, ignored = features.parse_test_output(handle.read())
        results[name] = {"passed": passed, "failed": failed, "ignored": ignored, "status": status}
    with open(os.path.join(out, "tests.json"), "w", encoding="utf-8") as handle:
        json.dump(results, handle, indent=2)
    return results


def cmd_tests(args):
    """The offline tests of every feature into a run directory, whose
    features.md then shows them (`eval.py grade` regrades it)."""
    os.makedirs(args.dir, exist_ok=True)
    offline_tests(args.dir, {t for f in FEATURES["features"] for t in f.get("tests", [])})
    print(os.path.join(args.dir, "tests.json"))


def cmd_features(args):
    scenarios = load_scenarios(args.scenario, args.scenarios_dir)
    out = args.out or os.path.join(OUT, "features-" + now())
    os.makedirs(out, exist_ok=True)
    if args.tests:
        names = {t for f in FEATURES["features"] for t in f.get("tests", [])}
        args.tests_results = offline_tests(out, names)
    evaluate(scenarios, args, out, overlay=os.path.abspath(args.overlay) if args.overlay else None)
    print(os.path.join(out, "features.md"))


def failure_evidence(graded, limit=120):
    """What the proposer reads: each job's failed checks and diagnostics,
    the judges' main failure, and reflection's and the builder's logs."""
    lines = []
    for g in sorted(graded, key=lambda x: x["score"]):
        if g.get("error"):
            continue
        if g["score"] >= 0.999 and all(r["score"] >= 1 for r in g["diagnostics"]):
            continue
        lines.append(f"== {g['id']} (score {g['score']:.2f}, {g['category']})")
        lines += [f"failed {r['check']} at {r['where']}: {r['why'][:200]}" for r in g["results"] if r["score"] < 1]
        lines += [f"inside, not {r['check']} at {r['where']}: {r['why'][:200]}" for r in g["diagnostics"] if r["score"] < 1]
        lines += [f"judge {x['judge']}: {x['failure']}" for x in g["grades"] if x.get("failure")]
        lines += g["evidence"][:25]
    return lines[:limit]


def split_scenarios(scenarios, share, seed):
    """(train, test) names: within each category, a seeded shuffle puts
    `share` of its scenarios (rounded, at least one when it has two or
    more) in the held-out test set. Drawn at random, never by score, so
    train and test are the same population."""
    rng = random.Random(seed)
    train, test = [], []
    by_category = {}
    for name in sorted(scenarios):
        by_category.setdefault(scenarios[name]["category"], []).append(name)
    for category in sorted(by_category):
        names = by_category[category]
        rng.shuffle(names)
        count = max(1, round(share * len(names))) if len(names) >= 2 and share > 0 else 0
        test += names[:count]
        train += names[count:]
    return sorted(train), sorted(test)


def subset_score(graded, names):
    rows = [g for g in graded if g["scenario"] in names]
    return aggregate(rows)["score"] if rows else None


def fmt(value):
    return "n/a" if value is None else f"{value:.3f}"


def cmd_climb(args):
    # The climb tunes the learning prompts: by default it runs the learning
    # scenarios, not every feature's.
    scenarios = load_scenarios(args.scenario or ["name", "reflex", "routine"], args.scenarios_dir)
    targets = args.target or prompts.default_targets()
    for target in targets:
        prompts.read(target)
        if not prompts.roundtrips(target):
            raise SystemExit(f"{target}: the prompt codec is not exact for it; the climb would change more than the prompt")
    out = args.out or os.path.join(OUT, "climb-" + now())
    os.makedirs(out, exist_ok=True)
    env = judge_env(args)
    proposers = judges.backends(env, args.proposer_names)
    deadline = time.time() + args.budget_hours * 3600
    state_path = os.path.join(out, "climb.json")
    if os.path.exists(state_path):
        with open(state_path, encoding="utf-8") as handle:
            state = json.load(handle)
        log(f"resuming at generation {state['generation']}, incumbent {state['score']:.3f}")
    else:
        train, test = split_scenarios(scenarios, args.test_share, args.seed)
        log(f"train {train}; held-out test {test}")
        log("evaluating the incumbent (the workspace's prompts)")
        _, graded = evaluate(scenarios, args, os.path.join(out, "gen-0-baseline"))
        score_train, score_test = subset_score(graded, train), subset_score(graded, test)
        state = {"generation": 0, "prompts": {}, "split": {"train": train, "test": test},
                 "score": score_train, "baseline": score_train, "test": score_test, "baseline_test": score_test,
                 "graded": [g for g in graded if g["scenario"] in train], "history": [], "targets": targets}
    def save():
        with open(state_path, "w", encoding="utf-8") as handle:
            json.dump(state, handle, indent=2)
    save()
    # Candidates are scored on train only; the proposer reads train
    # failures only; the held-out test set is run once per accepted winner.
    train_set = {k: v for k, v in scenarios.items() if k in state["split"]["train"]}
    while state["generation"] < args.generations and time.time() < deadline:
        generation = state["generation"] + 1
        target = targets[(generation - 1) % len(targets)]
        current = state["prompts"].get(target) or prompts.read(target)
        evidence = failure_evidence(state["graded"])
        tried = [h for h in state["history"] if h["target"] == target]
        log(f"generation {generation}: {target}, incumbent {state['score']:.3f}")
        candidates = []
        for index in range(args.candidates):
            backend = proposers[index % len(proposers)]
            try:
                text, rationale, model = judges.propose(backend, target, current, USAGE.get(prompts.split(target)[1], "a prompt <data> of " + target), evidence, tried)
            except RuntimeError as error:
                log(f"  proposer {backend.name}: {error}")
                continue
            candidates.append({"index": index, "proposer": backend.name, "model": model, "text": text, "rationale": rationale})
        if not candidates:
            log("  no candidate; stopping")
            break
        gen_dir = os.path.join(out, f"gen-{generation}")
        results = []
        jobs = []
        for cand in candidates:
            overlay = os.path.join(gen_dir, f"cand-{cand['index']}", "overlay")
            chosen = dict(state["prompts"], **{target: cand["text"]})
            diff = prompts.overlay(chosen, overlay)
            with open(os.path.join(gen_dir, f"cand-{cand['index']}", "prompts.patch"), "w", encoding="utf-8") as handle:
                handle.write(diff)
            cand_jobs = make_jobs(train_set, args.repeats, gen_dir, overlay, prefix=f"c{cand['index']}-")
            jobs.extend(cand_jobs)
            results.append((cand, chosen, cand_jobs))
        drive(jobs, gen_dir, args.parallel)
        graded_all = grade_all(jobs, env, args.judge_names, args.check_weight)
        best = None
        for cand, chosen, cand_jobs in results:
            ids = {j["id"] for j in cand_jobs}
            graded = [g for g in graded_all if g["id"] in ids]
            summary = aggregate(graded)
            cand["score"] = summary["score"]
            cand["categories"] = summary["categories"]
            log(f"  candidate {cand['index']} ({cand['proposer']}): {summary['score']:.3f} {summary['categories']}")
            if best is None or summary["score"] > best[0]["score"]:
                best = (cand, chosen, graded)
        with open(os.path.join(gen_dir, "report.md"), "w", encoding="utf-8") as handle:
            handle.write(report_markdown(f"Climb generation {generation} ({target})", aggregate(graded_all), graded_all))
        cand, chosen, graded = best
        accepted = cand["score"] > state["score"] + args.min_gain
        if accepted:
            # One lucky run is not a climb: run the winner again on train
            # (unless --no-confirm) and keep the mean of both, and run it on
            # the held-out test set, the climb's headline.
            confirm_dir = os.path.join(gen_dir, "confirm")
            overlay = os.path.join(gen_dir, f"cand-{cand['index']}", "overlay")
            names = state["split"]["test"] + (state["split"]["train"] if args.confirm else [])
            _, confirm_graded = evaluate({k: v for k, v in scenarios.items() if k in names}, args, confirm_dir, overlay)
            cand["test"] = subset_score(confirm_graded, state["split"]["test"])
            if args.confirm:
                confirmed = subset_score(confirm_graded, state["split"]["train"])
                cand["confirmed"] = confirmed
                mean = (cand["score"] + confirmed) / 2
                log(f"  confirmation {confirmed:.3f}, mean {mean:.3f}")
                accepted = mean > state["score"] + args.min_gain
                cand["score"] = mean
                graded = graded + [g for g in confirm_graded if g["scenario"] in state["split"]["train"]]
            log(f"  held-out test {cand['test']} (incumbent {state['test']})")
        state["history"].append({
            "generation": generation, "target": target, "proposer": cand["proposer"], "model": cand["model"],
            "score": cand["score"], "incumbent": state["score"], "test": cand.get("test"), "incumbent_test": state["test"],
            "accepted": accepted, "rationale": cand["rationale"],
            "candidates": [{k: c.get(k) for k in ("index", "proposer", "score", "rationale")} for c, _, _ in results],
        })
        if accepted:
            log(f"  accepted: {state['score']:.3f} -> {cand['score']:.3f}")
            state["prompts"], state["score"], state["graded"] = chosen, cand["score"], graded
            state["test"] = cand.get("test")
        else:
            log("  kept the incumbent")
        state["generation"] = generation
        save()
    best_dir = os.path.join(out, "best")
    shutil.rmtree(best_dir, ignore_errors=True)
    diff = prompts.overlay(state["prompts"], os.path.join(best_dir, "overlay")) if state["prompts"] else ""
    with open(os.path.join(out, "best.patch"), "w", encoding="utf-8") as handle:
        handle.write(diff)
    lines = [
        "# Learning eval hill climb", "",
        f"Held-out test (the headline): baseline {fmt(state.get('baseline_test'))}, best {fmt(state.get('test'))}. "
        f"Train: baseline {state['baseline']:.3f}, best {state['score']:.3f}, after {state['generation']} generations. "
        f"Train {state['split']['train']}; test {state['split']['test']}. A train gain the test score does not "
        "show is the climb fitting its own scenarios.", "",
        "| Gen | Target | Proposer | Train | Incumbent | Test | Incumbent test | Accepted | Rationale |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for h in state["history"]:
        lines.append(f"| {h['generation']} | `{h['target']}` | {h['proposer']} | {h['score']:.3f} | {h['incumbent']:.3f} | "
                     f"{fmt(h.get('test'))} | {fmt(h.get('incumbent_test'))} | {'yes' if h['accepted'] else 'no'} | {h['rationale'][:200]} |")
    lines += ["", "The best prompts are `best.patch` (apply with `git apply`) and `best/overlay/` (pass to `eval.py run --overlay`)."]
    with open(os.path.join(out, "climb.md"), "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")
    print(os.path.join(out, "climb.md"))


# The builder track


# Where builder.scx takes the request's model; the track pins it instead.
BUILDER_MODEL = '<assign location="model" expr="requestModel(_event)"/>'


def pin_builder_model(model, directory, base=None):
    """Writes into `directory` a builder.scx (from the overlay `base` when it
    holds one, else the workspace's) that builds with `model` whatever model
    reflection asks for, so the bot reasons on one model and builds on
    another."""
    rel = os.path.join("bots", "builder", "builder.scx")
    source = os.path.join(base, rel) if base and os.path.exists(os.path.join(base, rel)) else os.path.join(ROOT, rel)
    with open(source, encoding="utf-8") as handle:
        document = handle.read()
    if document.count(BUILDER_MODEL) != 1 or "'" in model or '"' in model:
        raise SystemExit("builder.scx no longer takes the request's model where the track pins it (eval.py BUILDER_MODEL)")
    if base:
        shutil.copytree(base, directory, dirs_exist_ok=True)
    out = os.path.join(directory, rel)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(document.replace(BUILDER_MODEL, f'<assign location="model" expr="\'{model}\'"/>'))


def parse_column(text):
    """`label=builder_model[;env=a:b][;bot=model]`."""
    label, _, rest = text.partition("=")
    parts = rest.split(";")
    column = {"label": label, "builder": parts[0], "env_files": None, "bot": None}
    for part in parts[1:]:
        key, _, value = part.partition("=")
        if key == "env":
            column["env_files"] = [os.path.abspath(p) for p in value.split(os.pathsep) if p]
        elif key == "bot":
            column["bot"] = value
        else:
            raise SystemExit(f"unknown column field {key!r} in {text!r}")
    if not label or not column["builder"]:
        raise SystemExit(f"a column is label=builder_model[;env=files][;bot=model]: {text!r}")
    return column


def builder_table(columns, graded):
    """Per column and scenario: builds run, accepted, the success rate, and
    the attempts per accepted build; with each column's failure reasons."""
    lines = ["# Builder track", "",
             "Each column runs the bot's reflex and routine scenarios with its builder pinned to one model. A build is one "
             "builder run reflection or the learning ability started (a create or change); attempts are its author attempts "
             "(its budget of failures also counts a failed design). A revision its turn cut before any draft is counted "
             "apart, not as a build.", "",
             "| Column | Builder model | Scenario | Builds | Accepted | Success | Attempts per accepted | Learned |",
             "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    summary = {}
    for column in columns:
        rows = [g for g in graded if g["id"].startswith(column["label"] + "-")]
        totals = {"builds": 0, "accepted": 0, "attempts": [], "reasons": {}, "learned": [], "cut": 0}
        for scenario in sorted({g["scenario"] for g in rows}):
            runs = [g for g in rows if g["scenario"] == scenario]
            # A revision its turn cut before the builder wrote a draft or
            # failed once measures the turn, not the builder: counted apart.
            # A log cut at the job's end before a draft lands here too.
            started = [b for g in runs for b in g["builds"]]
            built = [b for b in started if b["attempts"] or b["failures"] or b["reason"] != "unfinished"]
            totals["cut"] += len(started) - len(built)
            accepted = [b for b in built if b["accepted"]]
            # Learned anywhere in the job, not only on the teaching turn: a
            # behavior reflection learns turns later still counts here.
            learned = [float(g["learned_any"]) for g in runs if g.get("learned_any") is not None]
            totals["builds"] += len(built)
            totals["accepted"] += len(accepted)
            totals["attempts"] += [b["attempts"] for b in accepted]
            totals["learned"] += learned
            for b in built:
                if not b["accepted"]:
                    key = b["reason"] + (f" ({b['cause']})" if b.get("cause") else "")
                    totals["reasons"][key] = totals["reasons"].get(key, 0) + 1
            rate = f"{len(accepted) / len(built):.2f}" if built else "n/a"
            attempts = f"{statistics.mean(b['attempts'] for b in accepted):.1f}" if accepted else "n/a"
            mean_learned = f"{statistics.mean(learned):.2f}" if learned else "n/a"
            lines.append(f"| {column['label']} | `{column['builder']}` | {scenario} | {len(built)} | {len(accepted)} | {rate} | {attempts} | {mean_learned} |")
        summary[column["label"]] = {
            "builder": column["builder"],
            "builds": totals["builds"],
            "accepted": totals["accepted"],
            "success": totals["accepted"] / totals["builds"] if totals["builds"] else None,
            "attempts_per_accepted": statistics.mean(totals["attempts"]) if totals["attempts"] else None,
            "learned": statistics.mean(totals["learned"]) if totals["learned"] else None,
            "failures": totals["reasons"],
            "cut_before_draft": totals["cut"],
        }
    lines += ["", "| Column | Builds | Success | Attempts per accepted | Learned | Failed builds by reason | Cut before a draft |",
              "| --- | --- | --- | --- | --- | --- | --- |"]
    for label, s in summary.items():
        success = "n/a" if s["success"] is None else f"{s['success']:.2f}"
        attempts = "n/a" if s["attempts_per_accepted"] is None else f"{s['attempts_per_accepted']:.1f}"
        learned = "n/a" if s["learned"] is None else f"{s['learned']:.2f}"
        reasons = ", ".join(f"{k} {v}" for k, v in sorted(s["failures"].items())) or "none"
        lines.append(f"| {label} | {s['builds']} | {success} | {attempts} | {learned} | {reasons} | {s['cut_before_draft']} |")
    return "\n".join(lines) + "\n", summary


def write_builder(out, columns, graded):
    """builder.json and builder.md of a builder track's run."""
    table, summary = builder_table(columns, graded)
    with open(os.path.join(out, "builder.json"), "w", encoding="utf-8") as handle:
        # The models are the run's inputs: a regrade reads them back.
        models = {column["label"]: column["builder"] for column in columns}
        json.dump({"columns": summary, "models": models, "jobs": graded}, handle, indent=2)
    with open(os.path.join(out, "builder.md"), "w", encoding="utf-8") as handle:
        handle.write(table)


def cmd_builder(args):
    scenarios = load_scenarios(args.scenario or ["reflex", "routine"], args.scenarios_dir)
    columns = [parse_column(c) for c in args.column]
    out = args.out or os.path.join(OUT, "builder-" + now())
    jobs = []
    for column in columns:
        overlay = os.path.join(out, "columns", column["label"], "overlay")
        pin_builder_model(column["builder"], overlay, args.overlay)
        extra = {}
        if column["env_files"]:
            extra["env_files"] = column["env_files"]
        if column["bot"]:
            extra["model"] = column["bot"]
        jobs += make_jobs(scenarios, args.repeats, out, overlay, prefix=column["label"] + "-", extra=extra)
    drive(jobs, out, args.parallel, dry=args.dry)
    if args.dry:
        return
    graded = grade_all(jobs, judge_env(args), args.judge_names, args.check_weight)
    write_builder(out, columns, graded)
    with open(os.path.join(out, "report.md"), "w", encoding="utf-8") as handle:
        handle.write(report_markdown(f"Builder track {os.path.basename(out)}", aggregate(graded), graded))
    print(os.path.join(out, "builder.md"))


EXPAND_SYSTEM = (
    "You write held-out test scenarios for floppy, a companion robot bot that learns its name, reflexes (turn "
    "behaviors) and routines from what people tell it. Given one scenario as JSON, write a variation that tests the "
    "same skill with different wording, a different name, trigger, reply or reminder, and probes that check the "
    "learned behavior on real input (positive probes that should trigger it, near misses that should not). Keep "
    "the same JSON shape: category, intent, bot_name, sessions (steps of {say, source?} or {wait_s}), checks (the "
    "same check kinds, with at/by pointing at your steps, and patterns or names matching your variation), and "
    "criteria. Routine intervals must be at least one minute and waits long enough for two ticks. Answer with the "
    "scenario JSON object only."
)


def cmd_expand(args):
    scenarios = load_scenarios(args.scenario, args.scenarios_dir)
    out = args.out or os.path.join(OUT, "scenarios-" + now())
    os.makedirs(out, exist_ok=True)
    backends = judges.backends(judge_env(args), args.proposer_names)
    count = 0
    for name, scenario in scenarios.items():
        for index in range(args.variants):
            backend = backends[(count) % len(backends)]
            count += 1
            source = {k: v for k, v in scenario.items() if k != "name"}
            try:
                variant, _ = backend.ask_json(EXPAND_SYSTEM, json.dumps(source, indent=2))
                score.score(variant, {"steps": [], "sessions": [], "done": False})
            except (RuntimeError, KeyError, ValueError, TypeError) as error:
                log(f"{name} variant {index}: rejected ({error})")
                continue
            with open(os.path.join(out, f"{name}_v{index}.json"), "w", encoding="utf-8") as handle:
                json.dump(variant, handle, indent=2)
    print(out)


def null_transcripts(scenario):
    """Transcripts a judge must fail: floppy silent at every step, and
    floppy answering every step with "I don't know." (the eval-audit's
    known negatives)."""
    lines_silent, lines_unknown = [], []
    for index, session in enumerate(scenario["sessions"]):
        if index > 0:
            for lines in (lines_silent, lines_unknown):
                lines.append("(floppy is restarted)")
        for number, step in enumerate(session["steps"]):
            head = f"[session {index} step {number}]"
            who = step.get("who", step.get("source", "gabe"))
            if "say" in step:
                said = f"{head} {who} says: {step['say']!r}"
            elif "hear" in step:
                how = f" ({step['instructions']})" if step.get("instructions") else ""
                said = f"{head} {who} says out loud{how}: {step['hear']!r}"
            elif "show" in step:
                said = f"{head} {step.get('source', 'camera')} shows floppy an image: {step.get('describe', 'an image')}"
            else:
                said = f"{head} {step['wait_s']} s pass with no input"
            lines_silent.append(said)
            lines_unknown.append(said)
            if "say" in step or "hear" in step:
                lines_unknown.append("  floppy says: I don't know.")
    return {"silent": "\n".join(lines_silent), "unknown": "\n".join(lines_unknown)}


def cmd_judge_check(args):
    """Each judge on each scenario's known-negative transcripts: every
    overall grade must be at most --ceiling, or the judge is too lenient
    to steer a climb. Writes judge-check.json and judge-check.md."""
    scenarios = load_scenarios(args.scenario, args.scenarios_dir)
    out = args.out or os.path.join(OUT, "judge-check-" + now())
    os.makedirs(out, exist_ok=True)
    backends = judges.backends(judge_env(args), args.judge_names)
    tasks = [(name, kind, text, backend) for name, scenario in scenarios.items()
             for kind, text in null_transcripts(scenario).items() for backend in backends]

    def grade(task):
        name, kind, text, backend = task
        # A step where floppy rightly stays quiet passes silent: such a
        # scenario's silent transcript is not a known negative overall.
        try:
            result = judges.judge(backend, scenarios[name], text)
        except RuntimeError as error:
            return {"scenario": name, "kind": kind, "judge": backend.name, "error": str(error)}
        return {"scenario": name, "kind": kind, "judge": backend.name, "overall": result["overall"], "why": result["explanation"]}

    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        rows = list(pool.map(grade, tasks))
    lenient = [r for r in rows if r.get("overall", 0) > args.ceiling]
    with open(os.path.join(out, "judge-check.json"), "w", encoding="utf-8") as handle:
        json.dump({"ceiling": args.ceiling, "rows": rows}, handle, indent=2)
    lines = ["# Judge check", "", f"{len(rows)} known-negative transcripts graded; {len(lenient)} scored above "
             f"{args.ceiling} overall.", "", "| Scenario | Transcript | Judge | Overall | Why |", "| --- | --- | --- | --- | --- |"]
    for r in sorted(rows, key=lambda r: (-r.get("overall", 0), r["scenario"])):
        overall = f"{r['overall']:.1f}" if "overall" in r else r.get("error", "")
        lines.append(f"| {r['scenario']} | {r['kind']} | {r['judge']} | {overall} | {r.get('why', '')[:160]} |")
    with open(os.path.join(out, "judge-check.md"), "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")
    print(os.path.join(out, "judge-check.md"))


def cmd_targets(_args):
    defaults = prompts.default_targets()
    for target in prompts.discover():
        print(f"{target}  ({len(prompts.read(target))} chars){'  default' if target in defaults else ''}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)

    def common(p, run=True):
        p.add_argument("--scenario", action="append", help="a scenario name or category (repeatable; default all)")
        p.add_argument("--scenarios-dir", default=SCENARIOS)
        p.add_argument("--judge-env-file", action="append", help="an env file holding the judges' keys (repeatable)")
        p.add_argument("--judges", default="codex,gemini", help="judges by name, comma separated, or none")
        p.add_argument("--check-weight", type=float, default=0.6, help="the checks' share of a job's score (the judges' is the rest)")
        p.add_argument("--out")
        if run:
            p.add_argument("--repeats", type=int, default=1)
            p.add_argument("--parallel", type=int, default=4, help="bot runs at a time (the lab serves four L4s)")

    p = sub.add_parser("run")
    common(p)
    p.add_argument("--overlay", help="a directory laid out like the workspace, copied over each run's bot")
    p.add_argument("--dry", action="store_true", help="write the plan only")
    p.set_defaults(func=cmd_run)
    p = sub.add_parser("features")
    common(p)
    p.add_argument("--overlay", help="a directory laid out like the workspace, copied over each run's bot")
    p.add_argument("--no-tests", dest="tests", action="store_false", help="skip the offline tests")
    p.add_argument("--dry", action="store_true", help="write the plan only")
    p.set_defaults(func=cmd_features)
    p = sub.add_parser("tests")
    p.add_argument("dir")
    p.set_defaults(func=cmd_tests)
    p = sub.add_parser("merge")
    p.add_argument("--out", required=True)
    p.add_argument("dirs", nargs="+")
    p.set_defaults(func=cmd_merge)
    p = sub.add_parser("grade")
    common(p, run=False)
    p.add_argument("dir")
    p.set_defaults(func=cmd_grade)
    p = sub.add_parser("climb")
    common(p)
    p.add_argument("--target", action="append", help="<path>#<data id> (repeatable; default prompts.DEFAULT_IDS wherever declared)")
    p.add_argument("--generations", type=int, default=6)
    p.add_argument("--candidates", type=int, default=2, help="proposals per generation (alternating proposers)")
    p.add_argument("--proposers", default="codex,gemini")
    p.add_argument("--min-gain", type=float, default=0.02)
    p.add_argument("--test-share", type=float, default=0.34, help="share of each category's scenarios held out as the test set")
    p.add_argument("--seed", type=int, default=7, help="the seed of the train/test split")
    p.add_argument("--no-confirm", dest="confirm", action="store_false")
    p.add_argument("--budget-hours", type=float, default=10.0)
    p.set_defaults(func=cmd_climb)
    p = sub.add_parser("expand")
    common(p, run=False)
    p.add_argument("--variants", type=int, default=1)
    p.add_argument("--proposers", default="codex,gemini")
    p.set_defaults(func=cmd_expand)
    p = sub.add_parser("builder")
    common(p)
    p.add_argument("--column", action="append", required=True,
                   help="label=builder_model[;env=a:b][;bot=model]: one builder model, with the column's env files and bot model when they differ (repeatable)")
    p.add_argument("--overlay", help="an overlay every column starts from")
    p.add_argument("--dry", action="store_true", help="write the plan only")
    p.set_defaults(func=cmd_builder)
    p = sub.add_parser("judge-check")
    common(p, run=False)
    p.add_argument("--ceiling", type=float, default=0.3, help="the highest overall grade a known negative may get")
    p.set_defaults(func=cmd_judge_check)
    p = sub.add_parser("targets")
    p.set_defaults(func=cmd_targets)

    args = parser.parse_args(argv)
    # The driver runs with its crate (tests/) as its directory, so every
    # path it is given is absolute.
    if getattr(args, "dirs", None):
        args.dirs = [os.path.abspath(d) for d in args.dirs]
    for name in ("out", "dir", "overlay", "scenarios_dir"):
        if getattr(args, name, None):
            setattr(args, name, os.path.abspath(getattr(args, name)))
    if hasattr(args, "judges"):
        args.judge_names = None if args.judges == "none" else [n for n in args.judges.split(",") if n]
        if args.judge_names == []:
            args.judge_names = None
    if hasattr(args, "proposers"):
        args.proposer_names = [n for n in args.proposers.split(",") if n]
    if hasattr(args, "judge_env_file"):
        ENV.update(judge_env(args))
    args.func(args)


if __name__ == "__main__":
    sys.exit(main())
