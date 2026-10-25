"""The question report of floppy's eval (docs/eval.md): the maintainer's
questions about floppy (tests/eval/questions.json), each answered from the
scenarios that bear on it.

A question's score is the mean over its scenarios of each scenario's mean
job score, the same black-box score as the suite (D241); its answer is
`yes`, `partly` or `no` against questions.json's `answers`. The evidence
under it is what happened: each scenario's transcript (what the people
said, what floppy said and said out loud), the judges' reasons, and the
texts of the log labels the question names (what floppy stored, recalled,
built or failed inside). The inside lines are evidence beside the answer,
never in it. A question with `decisions` also tallies the events floppy's
turns selected (`floppy.output`).
"""

import collections
import json
import os
import statistics

import score

PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "questions.json")


def load(path=PATH):
    with open(path, encoding="utf-8") as handle:
        spec = json.load(handle)
    names = [q["name"] for q in spec["questions"]]
    if len(names) != len(set(names)):
        raise ValueError("a question is named twice in questions.json")
    return spec


def check_names(scenarios, spec):
    """Every scenario a question names exists: a misspelled one is an
    error, never a question silently answered from less."""
    for question in spec["questions"]:
        for name in question["scenarios"]:
            if name not in scenarios:
                raise ValueError(f"question {question['name']}: unknown scenario {name}")


def labels(spec):
    return sorted({label for q in spec["questions"] for label in q.get("inside", [])})


def inside(records, wanted, limit=6):
    """{label: (count, [the first `limit` distinct texts])} of the records'
    logs under each wanted label."""
    out = {}
    for label in wanted:
        texts = [text for name, text in score.logs(records) if name == label]
        if not texts:
            continue
        distinct = list(dict.fromkeys(texts))
        out[label] = [len(texts), [" ".join(t.split())[:200] for t in distinct[:limit]]]
    return out


def selected(records):
    """{event: count} of the events floppy's turns selected."""
    tally = collections.Counter()
    for name, text in score.logs(records):
        if name != "floppy.output":
            continue
        try:
            output = json.loads(text).get("output")
        except (ValueError, AttributeError):
            continue
        for entry in output if isinstance(output, list) else []:
            if isinstance(entry, dict) and entry.get("event"):
                tally[entry["event"]] += 1
    return dict(tally)


def answer(value, answers):
    if value is None:
        return "not run"
    if value >= answers["yes"]:
        return "yes"
    if value >= answers["partly"]:
        return "partly"
    return "no"


def answer_all(spec, graded):
    """Each question's {name, question, score, answer, scenarios: {name:
    score}, jobs: [graded job, ...]}. A job with an infra error is listed
    but not scored."""
    out = []
    for question in spec["questions"]:
        jobs = [g for g in graded if g["scenario"] in question["scenarios"]]
        by = {}
        for g in jobs:
            if not g.get("error"):
                by.setdefault(g["scenario"], []).append(g["score"])
        means = {name: statistics.mean(by[name]) for name in question["scenarios"] if name in by}
        value = statistics.mean(means.values()) if means else None
        out.append({"name": question["name"], "question": question["question"], "score": value,
                    "answer": answer(value, spec["answers"]), "scenarios": means, "jobs": jobs,
                    "decisions": bool(question.get("decisions")), "inside": question.get("inside", [])})
    return out


def _quote(text):
    return "\n".join("    " + line for line in text.rstrip().splitlines())


def markdown(title, rows):
    lines = [f"# {title}", "",
             "Each question is answered by its scenarios' black-box scores (floppy's replies, what it said out "
             "loud, and the judges' grade). Under it is the evidence: what was said, the judges' reasons, and what "
             "floppy logged inside (beside the answer, never in it).", "",
             "| Question | Answer | Score | Scenarios |", "| --- | --- | --- | --- |"]
    for r in rows:
        value = "n/a" if r["score"] is None else f"{r['score']:.2f}"
        scenarios = ", ".join(f"{k} {v:.2f}" for k, v in r["scenarios"].items()) or "none run"
        lines.append(f"| {r['question']} | {r['answer']} | {value} | {scenarios} |")
    for r in rows:
        lines += ["", f"## {r['question']}", ""]
        value = "n/a" if r["score"] is None else f"{r['score']:.2f}"
        lines.append(f"Answer: **{r['answer']}** ({value}).")
        if r["decisions"]:
            tally = collections.Counter()
            for g in r["jobs"]:
                tally.update(g.get("selected", {}))
            if tally:
                lines += ["", "What floppy's turns selected, over these scenarios: "
                          + ", ".join(f"`{event}` {n}" for event, n in tally.most_common()) + "."]
        for g in sorted(r["jobs"], key=lambda x: x["id"]):
            lines.append("")
            if g.get("error"):
                lines.append(f"### {g['id']}: not scored ({g['error']})")
                continue
            lines += [f"### {g['id']}: {g['score']:.2f}", ""]
            if g.get("transcript"):
                lines += [_quote(g["transcript"]), ""]
            for grade in g.get("grades", []):
                if "overall" in grade:
                    lines.append(f"- judge {grade['judge']} {grade['overall']:.1f}: {grade['explanation']}")
            found = g.get("inside", {})
            for label in r["inside"]:
                if label in found:
                    count, texts = found[label]
                    shown = "; ".join(f"`{t}`" for t in texts)
                    lines.append(f"- inside (not scored): `{label}` x{count}: {shown}")
    return "\n".join(lines) + "\n"
