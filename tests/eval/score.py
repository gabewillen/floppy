"""The deterministic half of the learning eval's grade (docs/eval.md): each
scenario check reads the records tests/tests/learning_eval.rs wrote for one
job and scores 0 to 1.

The grade is black box (decision D217): a scenario's `checks` read only
what floppy said back, and the judges read only the conversation, never
how floppy got there. Its `diagnostics` read floppy's own logs and are
reported beside the grade, never scored.

Every check names where it looks: `at: [session, step]` is that step's
records only, `by: [session, step]` is every record up to the end of that
step (every earlier session included).

Checks, on floppy's replies:

  says           a reply matches `pattern` (case-insensitive regex)
  not_says       the step completed and no reply matches `pattern`
  ticks          at least `min` replies matching `pattern` in a wait
                 step (with no input, so each is a routine's turn); scores
                 the fraction of `min` reached
  no_ticks       no reply in a wait step

A reply is a bot.ability.communication.respond the turn selected, from
floppy.output (its parts' words, joined): what floppy chose to say, with or
without a speech service (floppy.say is logged only once speech encoded it).

Diagnostics, on the log labels the bot writes:

  adopted_name   identity.name `adopted "<name>"` (the identity took it)
  recalled_name  identity.name `recalled "<name>"` in the session up to the
                 step (a restarted bot remembered it)
  addressed      identity.heard `addressed as <name>`
  not_addressed  no identity.heard `addressed as ...`
  learned        reflection.inventory `<name> ACTIVE <lifetime> ... (accept)`
                 (reflection learned a behavior, and the builder's draft
                 passed)
  broken         reflection.inventory `<name> BROKEN (break)`, or the
                 learning ability's learning.learned `lesson of turn N:
                 <name> ... BROKEN`
  reflex         cognition.stage `autonomy handled`: a learned behavior,
                 not reasoning, handled the real input
  not_reflex     no cognition.stage `autonomy handled`
"""

import json
import os
import re

# The checks a grade scores: each reads only what floppy said.
BLACK_BOX = ("says", "not_says", "ticks", "no_ticks")

RESPOND = "bot.ability.communication.respond"


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


def logs(records, sessions=False):
    """(label, text) of each log record; with `sessions`, (label, text,
    session), the session path of the child that logged it."""
    out = []
    for r in records:
        if r.get("type") != "log":
            continue
        line = (r.get("label") or "", r.get("text") if isinstance(r.get("text"), str) else json.dumps(r.get("text")))
        out.append((line + (r.get("session") or "",)) if sessions else line)
    return out


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


def _replies(text):
    """The words of each reply one floppy.output text selected, or None
    when the text is not a turn's output."""
    try:
        output = json.loads(text).get("output")
    except (ValueError, AttributeError):
        return None
    if not isinstance(output, list):
        return None
    out = []
    for entry in output:
        if not isinstance(entry, dict) or entry.get("event") != RESPOND:
            continue
        # A reply the eval cannot read makes the whole output unreadable; a
        # part with no words (a sound or a pause) is not.
        data = entry.get("data")
        parts = data.get("parts") if isinstance(data, dict) else None
        if not isinstance(parts, list) or not all(isinstance(p, dict) and isinstance(p.get("words", ""), str) for p in parts):
            return None
        words = "".join(p.get("words", "") for p in parts)
        if words.strip():
            out.append(words)
    return out


def replies(records):
    """The words of each reply the records' turns selected (floppy.output)."""
    return [words for text in _texts(records, "floppy.output") for words in _replies(text) or []]


def _unreadable(records):
    """How many of the records' floppy.output texts are not a turn's output."""
    return sum(_replies(text) is None for text in _texts(records, "floppy.output"))


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
        texts = _texts(records, "reflection.inventory") + _texts(records, "learning.learned")
        broken = [t for t in texts if " BROKEN" in t]
        return float(bool(broken)), (broken[0] if broken else "nothing was broken")
    stages = _texts(records, "cognition.stage")
    says = replies(records)
    if kind == "reflex":
        ok = "autonomy handled" in stages
        return float(ok), f"stages {stages}"
    if kind == "not_reflex":
        ok = "autonomy handled" not in stages
        return float(ok), f"stages {stages}"
    if kind == "says":
        ok = any(_matches(spec["pattern"], t) for t in says)
        return float(ok), f"said {[s[:120] for s in says]}"
    if kind in ("not_says", "no_ticks") and _unreadable(records):
        return 0.0, f"{_unreadable(records)} turn output(s) could not be read"
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


def _results(job, specs):
    results = []
    for spec in specs:
        value, why = check(job, spec)
        results.append({"check": spec["check"], "where": spec.get("at") or spec.get("by"), "score": value,
                        "weight": float(spec.get("weight", 1)), "why": why})
    return results


def score(scenario, job):
    """The checks' weighted score (0..1) and each check's result. Every
    check is black box; a scenario that scores floppy's logs is refused."""
    for spec in scenario["checks"]:
        if spec["check"] not in BLACK_BOX:
            raise ValueError(f"{spec['check']} reads floppy's logs: it is a diagnostic, not a check")
    results = _results(job, scenario["checks"])
    weights = sum(r["weight"] for r in results)
    total = sum(r["score"] * r["weight"] for r in results)
    return (total / weights if weights else 0.0), results


def diagnostics(scenario, job):
    """Each diagnostic's result: what floppy's logs say happened inside it.
    Reported, never scored."""
    return _results(job, scenario.get("diagnostics", []))


def summary(scenario, job, limit=600):
    """The job as the judges read it: the conversation alone, what each
    person said to floppy, the time that passed with no input, each restart
    and what floppy said back. Nothing of how floppy got there. Text only;
    nothing here holds a secret."""
    lines = []
    restarted = set()
    for step in job["steps"]:
        spec = scenario["sessions"][step["session"]]["steps"][step["step"]]
        if step["session"] > 0 and step["session"] not in restarted:
            restarted.add(step["session"])
            lines.append("(floppy is restarted)")
        head = f"[session {step['session']} step {step['step']}]"
        if step["kind"] == "say":
            lines.append(f"{head} {spec.get('source', 'gabe')} says: {spec['say']!r}")
        else:
            lines.append(f"{head} {spec['wait_s']} s pass with no input")
        records = job["sessions"][step["session"]][step["start_index"] : step["end_index"]]
        for text in _texts(records, "floppy.output"):
            said = _replies(text)
            if said is None:
                lines.append("  (a reply of floppy's could not be read)")
            for words in said or []:
                lines.append(f"  floppy says: {words[:limit]}")
        if not step["completed"]:
            lines.append("  (the step did not settle within its bound; a later reply is not shown)")
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
            if label.startswith(("reflection", "learning", "builder")) or "failed" in label:
                lines.append(f"session {index} {label}: {text[:limit]}")
    return lines


# The builder's own log labels (bots/builder/builder.scx), each logged in
# the builder's session.
BUILDER_LABELS = ("design", "author", "validate", "simulate", "triage", "retry_gate", "done", "failed")


def builds(job):
    """Every build the builder ran in the job (reflection or the learning
    ability invokes it for each create or change), from its own logs
    (BUILDER_LABELS): its first line opens a build, `author` "<mode> draft,
    attempt N" counts its attempts, `retry_gate` "failure N ..." its
    failures, and its final state is `done` "built <name> in N attempt(s)"
    or `failed` "<reason>: <diagnostic>". Two builders can
    run at once (reflection's and the learning ability's), so each build is
    followed by its record's `session`. A build the turn cancelled has no
    final state; the next `author` attempt 1, or a `design` that follows
    neither `triage` nor `retry_gate`, starts the next build. Each is {accepted, attempts, failures, reason, cause,
    diagnostics}."""
    out = []
    for records in job["sessions"]:
        current = {}
        for label, text, where in logs(records, sessions=True):
            if label not in BUILDER_LABELS:
                continue
            build = current.get(where)
            if label == "author":
                match = re.search(r"attempt (\d+)$", text)
                attempt = int(match.group(1)) if match else 1
                # Attempt 1 after an earlier author call is a new build
                # (one its turn cancelled before it ended).
                if build is not None and attempt == 1 and build["attempts"] >= 1:
                    out.append(_closed(build))
                    build = None
            elif label == "design" and build is not None and build["last"] not in ("triage", "retry_gate"):
                # A build redesigns only after triage: a design after any
                # other line starts a new build (the open one was cut).
                out.append(_closed(build))
                build = None
            if build is None:
                # Any of a builder's lines opens a build: a failure before
                # its first author call (its design) is that build's too.
                build = current[where] = {"accepted": False, "attempts": 0, "failures": 0, "reason": "unfinished", "cause": "", "diagnostics": []}
            build["last"] = label
            if label == "author":
                build["attempts"] = max(build["attempts"], attempt)
            elif label == "retry_gate":
                # "failure N of M": the budget counts a failed design as
                # well as each failed draft.
                match = re.match(r"failure (\d+)", text)
                if match:
                    build["failures"] = max(build["failures"], int(match.group(1)))
                else:
                    build["failures"] += 1
                build["diagnostics"].append(text[:200])
            elif label == "done":
                match = re.search(r"in (\d+) attempt", text)
                build.update(accepted=True, reason="done", attempts=int(match.group(1)) if match else build["attempts"])
                out.append(_closed(current.pop(where)))
            elif label == "failed":
                # "<reason>: <kind> <code> ...": the reason alone hides
                # whether the draft failed its schema, validation or tests.
                words = re.findall(r"[A-Za-z0-9_.-]+", text)
                build["reason"] = words[0][:40] if words else "failed"
                build["cause"] = " ".join(words[1:3])[:60]
                build["diagnostics"].append(text[:200])
                out.append(_closed(current.pop(where)))
        out.extend(_closed(build) for build in current.values())
    return out


def _closed(build):
    """A build without its bookkeeping (`last`, its previous builder line)."""
    build.pop("last", None)
    return build


def complete(scenario, job):
    """Whether every step of the scenario ran and settled."""
    wanted = sum(len(session["steps"]) for session in scenario["sessions"])
    return len(job["steps"]) == wanted and all(step["completed"] for step in job["steps"])


def learned_any(scenario, job):
    """Whether the job learned, anywhere, a behavior of the lifetime its
    scenario's first `learned` diagnostic asks for (None without one)."""
    spec = next((c for c in scenario.get("diagnostics", []) if c["check"] == "learned"), None)
    if spec is None or not job["steps"]:
        return None if spec is None else False
    last = job["steps"][-1]
    value, _ = check(job, {"check": "learned", "by": [last["session"], last["step"]], "lifetime": spec.get("lifetime", "turn")})
    return value >= 1
