"""The deterministic half of the learning eval's grade (docs/eval.md): each
scenario check reads the records tests/tests/learning_eval.rs wrote for one
job and scores 0 to 1.

Every check names where it looks: `at: [session, step]` is that step's
records only, `by: [session, step]` is every record up to the end of that
step (every earlier session included). What each reads is a log label the
bot already writes:

  adopted_name   identity.name `adopted "<name>"` (the identity took it)
  recalled_name  identity.name `recalled "<name>"` in the session up to the
                 step (a restarted bot remembered it)
  addressed      identity.heard `addressed as <name>`
  not_addressed  no identity.heard `addressed as ...`
  learned        reflection.inventory `<name> ACTIVE <lifetime> ... (accept)`
                 (reflection learned a behavior, and the builder's draft
                 passed)
  broken         reflection.inventory `<name> BROKEN (break)`
  reflex         cognition.stage `autonomy handled`: a learned behavior,
                 not reasoning, handled the real input
  not_reflex     no cognition.stage `autonomy handled`
  says           a floppy.say matches `pattern` (case-insensitive regex)
  not_says       the step completed and no floppy.say matches `pattern`
  ticks          at least `min` floppy.say matching `pattern` in a wait
                 step (with no input, so each is a routine's turn); scores
                 the fraction of `min` reached
  no_ticks       no floppy.say in a wait step
"""

import json
import os
import re

# Labels a step summary keeps, beside every label that names a failure.
SUMMARY_LABELS = (
    "cognition.stage",
    "identity.name",
    "identity.heard",
    "reflection.selected",
    "reflection.revision",
    "reflection.inventory",
    "autonomy",
    "autonomy.candidate",
    "routines",
    "bot.ability.routines.reconciled",
    "floppy.say",
)


def load_job(out):
    """The records of one job's out directory: {"steps": [...],
    "sessions": [[record, ...], ...], "done": bool}."""
    steps_path = os.path.join(out, "steps.json")
    job = {"steps": [], "sessions": [], "done": os.path.exists(os.path.join(out, "done.json"))}
    if os.path.exists(steps_path):
        with open(steps_path, encoding="utf-8") as handle:
            job["steps"] = json.load(handle)
    index = 0
    while os.path.exists(os.path.join(out, f"session-{index}.json")):
        with open(os.path.join(out, f"session-{index}.json"), encoding="utf-8") as handle:
            job["sessions"].append(json.load(handle))
        index += 1
    return job


def logs(records):
    return [
        (r.get("label") or "", r.get("text") if isinstance(r.get("text"), str) else json.dumps(r.get("text")))
        for r in records
        if r.get("type") == "log"
    ]


def _step(job, at):
    for step in job["steps"]:
        if [step["session"], step["step"]] == list(at):
            return step
    return None


def step_records(job, at):
    step = _step(job, at)
    if step is None or step["session"] >= len(job["sessions"]):
        return None
    return job["sessions"][step["session"]][step["start_index"] : step["end_index"]]


def records_by(job, by):
    """Every record up to the end of the step `by`, or None when it never ran."""
    step = _step(job, by)
    if step is None or step["session"] >= len(job["sessions"]):
        return None
    out = []
    for session in range(step["session"]):
        out.extend(job["sessions"][session])
    out.extend(job["sessions"][step["session"]][: step["end_index"]])
    return out


def _texts(records, label):
    return [text for name, text in logs(records) if name == label]


def _matches(pattern, text):
    return re.search(pattern, text.strip(), re.IGNORECASE | re.MULTILINE) is not None


def check(job, spec):
    """(score 0..1, why) of one check."""
    kind = spec["check"]
    where = spec.get("at") or spec.get("by")
    if "by" in spec:
        records = records_by(job, spec["by"])
    else:
        records = step_records(job, spec["at"])
    if records is None:
        return 0.0, f"step {where} never ran"
    step = _step(job, where)
    name = spec.get("name", "")
    if kind == "adopted_name":
        ok = any(t.lower() == f'adopted "{name.lower()}"' for t in _texts(records, "identity.name"))
        return float(ok), "adopted " + name if ok else f"never adopted {name}: {_texts(records, 'identity.name')}"
    if kind == "recalled_name":
        session = job["sessions"][where[0]][: step["end_index"]]
        ok = any(t.lower() == f'recalled "{name.lower()}"' for t in _texts(session, "identity.name"))
        return float(ok), "recalled " + name if ok else f"did not recall {name}: {_texts(session, 'identity.name')}"
    if kind == "addressed":
        heard = _texts(records, "identity.heard")
        ok = any(t.lower() == f"addressed as {name.lower()}" for t in heard)
        return float(ok), f"heard {heard}"
    if kind == "not_addressed":
        heard = _texts(records, "identity.heard")
        ok = not any(t.startswith("addressed as") for t in heard)
        return float(ok), f"heard {heard}"
    if kind == "learned":
        lifetime = spec.get("lifetime", "turn")
        accepted = [
            t for t in _texts(records, "reflection.inventory")
            if f" ACTIVE {lifetime} " in t and t.endswith("(accept)")
        ]
        if accepted:
            return 1.0, accepted[0][:200]
        revisions = _texts(records, "reflection.revision")
        return 0.0, f"no {lifetime} behavior accepted (revisions {revisions[:4]})"
    if kind == "broken":
        broken = [t for t in _texts(records, "reflection.inventory") if " BROKEN" in t]
        return float(bool(broken)), (broken[0] if broken else "nothing was broken")
    stages = _texts(records, "cognition.stage")
    says = _texts(records, "floppy.say")
    if kind == "reflex":
        ok = "autonomy handled" in stages
        return float(ok), f"stages {stages}"
    if kind == "not_reflex":
        ok = "autonomy handled" not in stages
        return float(ok), f"stages {stages}"
    if kind == "says":
        ok = any(_matches(spec["pattern"], t) for t in says)
        return float(ok), f"said {[s[:120] for s in says]}"
    if kind == "not_says":
        if not step["completed"]:
            return 0.0, "the step did not complete"
        ok = not any(_matches(spec["pattern"], t) for t in says)
        return float(ok), f"said {[s[:120] for s in says]}"
    if kind == "ticks":
        hits = [t for t in says if _matches(spec["pattern"], t)]
        want = max(1, int(spec.get("min", 1)))
        return min(1.0, len(hits) / want), f"{len(hits)} of {want} wanted ({[s[:80] for s in says]})"
    if kind == "no_ticks":
        return float(not says), f"said {[s[:80] for s in says]}"
    raise ValueError("unknown check: " + kind)


def score(scenario, job):
    """The checks' weighted score (0..1) and each check's result."""
    results = []
    total = 0.0
    weights = 0.0
    for spec in scenario["checks"]:
        value, why = check(job, spec)
        weight = float(spec.get("weight", 1))
        total += value * weight
        weights += weight
        results.append({"check": spec["check"], "where": spec.get("at") or spec.get("by"), "score": value, "weight": weight, "why": why})
    return (total / weights if weights else 0.0), results


def summary(scenario, job, limit=600):
    """The job, step by step, as the judges read it: what was said to the
    bot, what it said, which stage handled each turn, and what it learned
    or failed at. Text only; nothing here holds a secret (records are the
    bot's own logs)."""
    lines = []
    for step in job["steps"]:
        spec = scenario["sessions"][step["session"]]["steps"][step["step"]]
        head = f"[session {step['session']} step {step['step']}]"
        if step["kind"] == "say":
            lines.append(f"{head} {spec.get('source', 'gabe')} says: {spec['say']!r}")
        else:
            lines.append(f"{head} {spec['wait_s']} s pass with no input")
        if not step["completed"]:
            lines.append("  (the step did not settle within its bound)")
        records = job["sessions"][step["session"]][step["start_index"] : step["end_index"]]
        for label, text in logs(records):
            if label in SUMMARY_LABELS or "failed" in label or "degraded" in label:
                lines.append(f"  {label}: {text[:limit]}")
    ran = {(s["session"], s["step"]) for s in job["steps"]}
    for session, spec in enumerate(scenario["sessions"]):
        for index, _ in enumerate(spec["steps"]):
            if (session, index) not in ran:
                lines.append(f"[session {session} step {index}] never ran")
    return "\n".join(lines)


def evidence(job, limit=400):
    """The learning failures of a job, for the hill climb's proposer: what
    reflection selected, each revision, and every failure it logged."""
    lines = []
    for index, records in enumerate(job["sessions"]):
        for label, text in logs(records):
            if label.startswith("reflection") or label.startswith("builder") or "failed" in label:
                lines.append(f"session {index} {label}: {text[:limit]}")
    return lines
