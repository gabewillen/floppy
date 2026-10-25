"""The feature grade of floppy's eval (docs/eval.md): each feature of
tests/eval/features.json graded from the scenarios that exercise it.

A feature's live score is the mean, over the jobs of every scenario that
names it (its `features`) or has a check naming it, of that job's score for
the feature: the checks that name the feature (every check of the scenario
when none does), weighted as the scenario weights them, combined with the
judges' criteria the scenario's `criteria_features` gives the feature (the
judges' overall grade when it gives none) as a job's score combines them.
The letter is the live score against features.json's `grades`.

Beside it, never in it (decision D241): the feature's diagnostics (the
scenario diagnostics that name it), its evidence (the log labels that show
its parts ran, and the ones that should never be logged), and its offline
tests (`cargo test --test <name>`) when the run ran them. A feature no
scenario exercises is reported as ungraded, with its status when it has
no live path at all.
"""

import json
import os
import re
import statistics

import score

PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "features.json")


def load(path=PATH):
    with open(path, encoding="utf-8") as handle:
        spec = json.load(handle)
    names = [f["name"] for f in spec["features"]]
    if len(names) != len(set(names)):
        raise ValueError("a feature is named twice in features.json")
    return spec


def check_names(scenarios, spec):
    """Every feature a scenario, check, diagnostic or criterion names is one
    features.json defines: a misspelled feature is an error, never a feature
    silently left ungraded."""
    known = {f["name"] for f in spec["features"]}
    for name, scenario in scenarios.items():
        named = list(scenario.get("features", []))
        for item in scenario.get("checks", []) + scenario.get("diagnostics", []):
            named += item.get("features", [])
        for feature, criteria in scenario.get("criteria_features", {}).items():
            named.append(feature)
            for criterion in criteria:
                if criterion not in scenario["criteria"]:
                    raise ValueError(f"{name}: criteria_features names the unknown criterion {criterion}")
        for feature in named:
            if feature not in known:
                raise ValueError(f"{name}: unknown feature {feature}")


def evidence_key(item):
    return item["label"] + (f" ~ {item['pattern']}" if item.get("pattern") else "")


def seen(job, spec):
    """{evidence key: whether the job's records hold it} for every evidence
    label of every feature."""
    records = [r for session in job["sessions"] for r in session]
    labels = score.logs(records)
    out = {}
    for feature in spec["features"]:
        for item in feature.get("evidence", []):
            pattern = item.get("pattern")
            out[evidence_key(item)] = any(
                label == item["label"] and (pattern is None or re.search(pattern, text, re.IGNORECASE))
                for label, text in labels
            )
    return out


def job_score(feature, scenario, graded, check_weight):
    """The job's score for `feature`, or None when its scenario does not
    exercise the feature."""
    name = feature["name"]
    tagged = [r for r in graded["results"] if name in r.get("features", [])]
    if not tagged and name not in scenario.get("features", []):
        return None
    results = tagged or graded["results"]
    weights = sum(r["weight"] for r in results)
    checks = sum(r["score"] * r["weight"] for r in results) / weights if weights else 0.0
    criteria = scenario.get("criteria_features", {}).get(name)
    judged = []
    for grade in graded["grades"]:
        if "overall" not in grade:
            continue
        if criteria:
            judged.append(statistics.mean(grade["criteria"].get(c, 0.0) for c in criteria))
        else:
            judged.append(grade["overall"])
    if not judged:
        return checks
    return check_weight * checks + (1 - check_weight) * statistics.mean(judged)


def interval(values):
    """The half-width of the normal 95% interval of the mean of `values`
    (the job scores behind a number), or None with fewer than two."""
    if len(values) < 2:
        return None
    return 1.96 * statistics.stdev(values) / len(values) ** 0.5


def letter(value, grades):
    if value is None:
        return "–"
    for mark, floor in sorted(grades.items(), key=lambda item: -item[1]):
        if value >= floor:
            return mark
    return "F"


def grade(spec, scenarios, graded, check_weight, tests=None):
    """Each feature's grade: {name, title, live, letter, jobs, scenarios,
    diagnostics, evidence, absent, tests, status}."""
    out = []
    for feature in spec["features"]:
        name = feature["name"]
        scores, used, inside = [], set(), []
        for g in graded:
            scenario = scenarios.get(g["scenario"])
            # A job with an infra error measured the rig, not floppy.
            if scenario is None or g.get("error"):
                continue
            value = job_score(feature, scenario, g, check_weight)
            if value is not None:
                scores.append((g["scenario"], value))
                used.add(g["scenario"])
            inside += [r["score"] for r in g["diagnostics"] if name in r.get("features", [])]
        # Each scenario counts once, by the mean of its repeats.
        per_scenario = {}
        for scenario_name, value in scores:
            per_scenario.setdefault(scenario_name, []).append(value)
        means = {k: statistics.mean(v) for k, v in per_scenario.items()}
        live = statistics.mean(means.values()) if means else None
        evidence, absent = {}, {}
        for item in feature.get("evidence", []):
            key = evidence_key(item)
            hits = sum(1 for g in graded if g.get("seen", {}).get(key))
            (absent if item.get("absent") else evidence)[key] = hits
        feature_tests = None
        if tests is not None and feature.get("tests"):
            feature_tests = {t: tests.get(t) for t in feature["tests"]}
        out.append({
            "name": name,
            "title": feature["title"],
            "what": feature["what"],
            "status": feature.get("status"),
            "live": live,
            "interval": interval([value for _, value in scores]),
            "letter": letter(live, spec["grades"]),
            "scenarios": dict(sorted(means.items())),
            "jobs": len(scores),
            "diagnostics": statistics.mean(inside) if inside else None,
            "evidence": evidence,
            "absent": absent,
            "tests": feature_tests,
        })
    return out


def _tests_cell(tests):
    if not tests:
        return "–"
    cells = []
    for name, result in tests.items():
        if result is None:
            cells.append(f"{name} not run")
        else:
            mark = "ok" if result["failed"] == 0 and result["status"] == 0 else "FAIL"
            cells.append(f"{name} {mark} {result['passed']}/{result['passed'] + result['failed']}")
    return ", ".join(cells)


def markdown(title, rows, summary):
    graded_rows = [r for r in rows if r["live"] is not None]
    overall = statistics.mean(r["live"] for r in graded_rows) if graded_rows else None
    lines = [f"# {title}", ""]
    if overall is not None:
        lines.append(f"Mean over the {len(graded_rows)} graded features: **{overall:.3f}**, from {summary['jobs']} scored "
                     f"jobs ({summary['unfinished']} unfinished). {len(rows) - len(graded_rows)} features have no live grade.")
    errors = summary.get("errors", [])
    if errors:
        lines += ["", f"{len(errors)} job(s) measured the rig, not floppy, and are not scored:"]
        lines += [f"- {e['id']}: {e['error']}" for e in errors]
    lines += ["", "± is the half-width of the 95% interval over the feature's job scores (n/a with one job): a "
              "difference between two runs smaller than it is noise. With one repeat per scenario most features "
              "rest on one to three jobs; run `--repeats 3` before acting on a difference."]
    lines += ["", "A feature's live score is black box: floppy's replies, what it said out loud, and the judges' grade "
              "of the conversation. Diagnostics, evidence and offline tests read floppy's insides and are reported "
              "beside it, never in it.", "",
              "| Feature | Grade | Live | ± | Scenarios | Inside (diagnostics) | Offline tests |",
              "| --- | --- | --- | --- | --- | --- | --- |"]
    for r in rows:
        live = "n/a" if r["live"] is None else f"{r['live']:.2f}"
        spread = "n/a" if r.get("interval") is None else f"{r['interval']:.2f}"
        scenarios = ", ".join(f"{k} {v:.2f}" for k, v in r["scenarios"].items()) or (r["status"] or "no scenario")
        inside = "–" if r["diagnostics"] is None else f"{r['diagnostics']:.2f}"
        lines.append(f"| {r['title']} | {r['letter']} | {live} | {spread} | {scenarios} | {inside} | {_tests_cell(r['tests'])} |")
    lines += ["", "## Evidence", "",
              "How many of the run's jobs logged each label that shows a feature's parts ran, and each label that "
              "should never be logged (a failure or a degraded part).", "",
              "| Feature | Ran (jobs) | Should never log (jobs) |", "| --- | --- | --- |"]
    for r in rows:
        if not r["evidence"] and not r["absent"]:
            continue
        ran = ", ".join(f"`{k}` {v}" for k, v in r["evidence"].items()) or "–"
        never = ", ".join(f"`{k}` {v}" for k, v in r["absent"].items()) or "–"
        lines.append(f"| {r['title']} | {ran} | {never} |")
    lines += ["", "## Features", ""]
    for r in rows:
        lines.append(f"- **{r['title']}** ({r['letter']}): {r['what']}" + (f" *{r['status']}.*" if r["status"] else ""))
    return "\n".join(lines) + "\n"


def parse_test_output(text):
    """(passed, failed, ignored) summed over the `test result:` lines."""
    passed = failed = ignored = 0
    for match in re.finditer(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", text):
        passed += int(match.group(1))
        failed += int(match.group(2))
        ignored += int(match.group(3))
    return passed, failed, ignored
