"""Offline tests of the learning eval's scorer and prompt codec:
`python3 -m unittest discover -s tests/eval`. They make no model call."""

import json
import os
import tempfile
import unittest

import eval as runner
import prompts
import score


def log(label, text):
    return {"type": "log", "label": label, "text": text}


def said(words):
    """A turn's floppy.output whose one selection is a reply of `words`."""
    output = [{"event": "bot.ability.communication.respond", "data": {"parts": [{"words": words}]}}]
    return log("floppy.output", json.dumps({"turn": 1, "output": output}))


def write_job(directory, sessions, steps):
    for index, records in enumerate(sessions):
        with open(os.path.join(directory, f"session-{index}.json"), "w", encoding="utf-8") as handle:
            json.dump(records, handle)
    with open(os.path.join(directory, "steps.json"), "w", encoding="utf-8") as handle:
        json.dump(steps, handle)
    with open(os.path.join(directory, "done.json"), "w", encoding="utf-8") as handle:
        handle.write("{}")


def step(session, index, start, end, kind="say", completed=True):
    return {"session": session, "step": index, "kind": kind, "start_index": start, "end_index": end,
            "started_ms": 0, "ended_ms": 0, "completed": completed}


class Scorer(unittest.TestCase):
    def setUp(self):
        self.scenarios = runner.load_scenarios()

    def test_every_scenario_check_points_at_one_of_its_steps(self):
        for name, scenario in self.scenarios.items():
            steps = {(s, i) for s, spec in enumerate(scenario["sessions"]) for i, _ in enumerate(spec["steps"])}
            for spec in scenario["checks"]:
                where = tuple(spec.get("at") or spec.get("by"))
                self.assertIn(where, steps, f"{name}: {spec}")
            for spec in scenario.get("diagnostics", []):
                self.assertIn(tuple(spec.get("at") or spec.get("by")), steps, f"{name}: {spec}")
            # Every check is black box, and an empty job scores each 0 without raising.
            self.assertTrue(all(c["check"] in score.BLACK_BOX for c in scenario["checks"]), name)
            value, results = score.score(scenario, {"steps": [], "sessions": [], "done": False})
            self.assertEqual(value, 0.0)
            self.assertEqual(len(results), len(scenario["checks"]))

    def test_a_check_on_floppy_s_logs_is_refused(self):
        scenario = {"sessions": [{"steps": [{"say": "ping"}]}], "checks": [{"check": "reflex", "at": [0, 0]}]}
        with self.assertRaises(ValueError):
            score.score(scenario, {"steps": [], "sessions": [], "done": False})
        scenario["checks"] = [{"check": "sayz", "at": [0, 0]}]
        with self.assertRaisesRegex(ValueError, "unknown check: sayz"):
            score.score(scenario, {"steps": [], "sessions": [], "done": False})
        scenario["checks"], scenario["diagnostics"] = [], [{"check": "says", "at": [0, 0], "pattern": "pong"}]
        with self.assertRaisesRegex(ValueError, "it is a check, not a diagnostic"):
            score.diagnostics(scenario, {"steps": [], "sessions": [], "done": False})

    def test_a_probe_after_a_restart_carries_the_largest_weight(self):
        for name, scenario in self.scenarios.items():
            later = [c["weight"] for c in scenario["checks"] if c["check"] == "says" and (c.get("at") or c.get("by"))[0] > 0]
            if later:
                self.assertTrue(all(max(later) >= c["weight"] for c in scenario["checks"]), name)

    def test_a_reflex_answered_by_reasoning_scores_and_shows_inside(self):
        scenario = self.scenarios["reflex_pingpong"]
        session0 = [
            log("reflection.inventory", 'PingPong ACTIVE turn [{"event":"bot.ability.cognition.input"}] (accept)'),
            log("cognition.stage", "reasoning handled"),
            said("pong"),
            log("cognition.stage", "reasoning handled"),
            said("Four."),
            log("cognition.stage", "autonomy handled"),
            said("pong"),
        ]
        session1 = [log("cognition.stage", "autonomy handled"), said("Pong!")]
        with tempfile.TemporaryDirectory() as directory:
            write_job(directory, [session0, session1], [
                step(0, 0, 0, 1), step(0, 1, 1, 3), step(0, 2, 3, 5), step(0, 3, 5, 7), step(1, 0, 0, 2)])
            job = score.load_job(directory)
        # The grade sees only the replies: every one is right.
        value, _ = score.score(scenario, job)
        self.assertEqual(value, 1.0)
        # Inside, the first ping was reasoning's, not the reflex's.
        inside = {(r["check"], tuple(r["where"])): r["score"] for r in score.diagnostics(scenario, job)}
        self.assertEqual(inside[("learned", (0, 0))], 1.0)
        self.assertEqual(inside[("reflex", (0, 1))], 0.0)
        self.assertEqual(inside[("not_reflex", (0, 2))], 1.0)
        self.assertEqual(inside[("reflex", (1, 0))], 1.0)
        # The judges read the conversation alone.
        transcript = score.summary(scenario, job)
        self.assertIn("(floppy is restarted)", transcript)
        self.assertIn("floppy says: Pong!", transcript)
        self.assertNotIn("handled", transcript)
        self.assertNotIn("PingPong", transcript)

    def test_ticks_count_the_routine_s_words_in_a_wait(self):
        scenario = self.scenarios["routine_reminder"]
        session = [
            log("reflection.inventory", "WaterReminder ACTIVE persistent [] (accept)"),
            said("Time to drink some water!"),
            said("Hello."),
            log("reflection.inventory", "WaterReminder BROKEN (break)"),
        ]
        with tempfile.TemporaryDirectory() as directory:
            write_job(directory, [session], [step(0, 0, 0, 1), step(0, 1, 1, 3, "wait"), step(0, 2, 3, 4), step(0, 3, 4, 4, "wait")])
            job = score.load_job(directory)
        by = {r["check"]: r["score"] for r in score.score(scenario, job)[1] + score.diagnostics(scenario, job)}
        self.assertEqual(by["learned"], 1.0)
        self.assertEqual(by["ticks"], 0.5)
        self.assertEqual(by["broken"], 1.0)
        self.assertEqual(by["no_ticks"], 1.0)

    def test_a_break_by_the_learning_ability_is_broken(self):
        scenario = self.scenarios["routine_reminder"]
        session = [
            log("reflection.inventory", "WaterReminder ACTIVE persistent [] (accept)"),
            said("Time to drink some water!"),
            said("Okay, I'll stop."),
            log("learning.learned", "lesson of turn 6: WaterReminder ACTIVE persistent BROKEN"),
        ]
        with tempfile.TemporaryDirectory() as directory:
            write_job(directory, [session], [step(0, 0, 0, 1), step(0, 1, 1, 2, "wait"), step(0, 2, 2, 4), step(0, 3, 4, 4, "wait")])
            results = score.diagnostics(scenario, score.load_job(directory))
        by = {r["check"]: r["score"] for r in results}
        self.assertEqual(by["broken"], 1.0)

    def test_a_name_is_adopted_recalled_and_heard(self):
        scenario = self.scenarios["name_adopt"]
        session0 = [log("identity.name", 'adopted "Juniper"'), log("identity.heard", "addressed as Juniper"),
                    said("I'm Juniper."), log("identity.heard", "not addressed")]
        session1 = [log("identity.name", 'recalled "Juniper"'), log("identity.heard", "addressed as Juniper"),
                    said("Yes!"), said("My name is Juniper.")]
        with tempfile.TemporaryDirectory() as directory:
            write_job(directory, [session0, session1], [step(0, 0, 0, 1), step(0, 1, 1, 3), step(0, 2, 3, 4), step(1, 0, 0, 3), step(1, 1, 3, 4)])
            job = score.load_job(directory)
        value, _ = score.score(scenario, job)
        self.assertEqual(value, 1.0)
        self.assertTrue(all(r["score"] == 1.0 for r in score.diagnostics(scenario, job)))

    def test_a_reply_is_the_respond_the_turn_selected(self):
        output = [
            {"event": "bot.focus_device", "data": {"device": "lamp"}},
            {"event": "bot.ability.communication.respond",
             "data": {"parts": [{"words": "Time to "}, {"expression": {"pause": 0.2}}, {"words": "drink water!"}]}},
        ]
        records = [
            log("floppy.output", json.dumps({"turn": 2, "output": output})),
            log("floppy.output", json.dumps({"turn": 3, "output": []})),
            # Spoken only with a speech service; the turn's output is what it said.
            log("floppy.say", "spoken twice"),
        ]
        self.assertEqual(score.replies(records), ["Time to drink water!"])
        scenario = {"sessions": [{"steps": [{"wait_s": 60}]}], "checks": []}
        job = {"steps": [step(0, 0, 0, 3, "wait")], "sessions": [records], "done": True}
        self.assertIn("floppy says: Time to drink water!", score.summary(scenario, job))
        self.assertNotIn("spoken twice", score.summary(scenario, job))
        # An output that is not a turn's says so, rather than looking silent.
        job["sessions"][0].append(log("floppy.output", "not json"))
        job["steps"][0]["end_index"] = 4
        self.assertIn("(a reply of floppy's could not be read)", score.summary(scenario, job))
        value, why = score.check(job, {"check": "no_ticks", "at": [0, 0]})
        self.assertEqual(value, 0.0, why)
        # So does a reply whose words are not text; a reply of a sound alone is silent.
        sound = {"event": "bot.ability.communication.respond", "data": {"parts": [{"expression": {"sound": "laugh"}}]}}
        self.assertEqual(score._replies(json.dumps({"turn": 4, "output": [sound]})), [])
        bad = {"event": "bot.ability.communication.respond", "data": {"parts": [{"words": ["Time", "to"]}]}}
        self.assertIsNone(score._replies(json.dumps({"turn": 4, "output": [bad]})))


class Builds(unittest.TestCase):
    def test_each_build_is_counted_with_its_attempts_and_end(self):
        session = [
            log("design", "plan"), log("author", "create draft, attempt 1"), log("retry_gate", "failure 1: validate XML001"),
            log("author", "repair draft, attempt 2"), log("done", "built PingPong in 2 attempt(s)"),
            log("author", "create draft, attempt 1"), log("retry_gate", "failure 1: validate XML001"),
            log("author", "repair draft, attempt 2"), log("failed", "no-progress: validate XML001"),
            log("author", "create draft, attempt 1"),
        ]
        found = score.builds({"steps": [], "sessions": [session], "done": True})
        self.assertEqual([(b["accepted"], b["attempts"], b["reason"], b["cause"]) for b in found],
                         [(True, 2, "done", ""), (False, 2, "no-progress", "validate XML001"), (False, 1, "unfinished", "")])

    def test_a_failure_before_the_first_draft_is_its_build_s(self):
        session = [
            log("design", "plan"), log("retry_gate", "failure 1 of 8: generate author-failed (transport)"),
            log("author", "create draft, attempt 1"), log("retry_gate", "failure 2 of 8: validate XML001"),
            log("author", "repair draft, attempt 2"), log("done", "built PingPong in 2 attempt(s)"),
        ]
        found = score.builds({"steps": [], "sessions": [session], "done": True})
        self.assertEqual([(b["accepted"], b["attempts"], b["diagnostics"][0][:9]) for b in found], [(True, 2, "failure 1")])

    def test_a_build_cut_in_its_design_is_its_own(self):
        session = [
            log("design", "plan"),
            log("design", "plan"), log("author", "create draft, attempt 1"), log("retry_gate", "failure 1 of 8: validate XML001"),
            log("triage", "redesign"), log("design", "plan"), log("author", "create draft, attempt 2"),
            log("done", "built PingPong in 2 attempt(s)"),
        ]
        found = score.builds({"steps": [], "sessions": [session], "done": True})
        self.assertEqual([(b["accepted"], b["attempts"], b["reason"]) for b in found],
                         [(False, 0, "unfinished"), (True, 2, "done")])

    def test_two_builders_at_once_are_two_builds(self):
        def at(where, label, text):
            return {**log(label, text), "session": where}
        learning, reflection = "session.learning.builder", "session.reflection.builder"
        session = [
            at(learning, "author", "create draft, attempt 1"),
            at(reflection, "author", "change draft, attempt 1"),
            at(learning, "retry_gate", "failure 1 of 8: validate XML001"),
            at(learning, "author", "repair draft, attempt 2"),
            at(reflection, "done", "built PingPong in 1 attempt(s)"),
            at(learning, "failed", "attempts-exhausted: simulate test-failed"),
        ]
        found = score.builds({"steps": [], "sessions": [session], "done": True})
        self.assertEqual(sorted((b["accepted"], b["attempts"], b["reason"], len(b["diagnostics"])) for b in found),
                         [(False, 2, "attempts-exhausted", 2), (True, 1, "done", 0)])


    def test_a_build_cut_before_a_draft_is_apart_unless_it_failed(self):
        def build(attempts, failures, reason):
            return {"accepted": reason == "done", "attempts": attempts, "failures": failures, "reason": reason,
                    "cause": "", "diagnostics": []}
        graded = [{"id": "a-pingpong-r0", "scenario": "pingpong", "learned_any": True,
                   "builds": [build(0, 0, "unfinished"), build(0, 1, "unfinished"), build(1, 0, "done")]}]
        _, summary = runner.builder_table([{"label": "a", "builder": "m"}], graded)
        self.assertEqual((summary["a"]["builds"], summary["a"]["cut_before_draft"], summary["a"]["builder"]), (2, 1, "m"))
        with tempfile.TemporaryDirectory() as directory:
            runner.write_builder(directory, [{"label": "a", "builder": "m"}], graded)
            with open(os.path.join(directory, "builder.json"), encoding="utf-8") as handle:
                self.assertEqual(json.load(handle)["models"], {"a": "m"})

class Evidence(unittest.TestCase):
    def test_a_full_grade_with_a_failed_diagnostic_reaches_the_proposer(self):
        graded = [{"id": "pingpong-r0", "score": 1.0, "category": "reflex", "results": [], "grades": [], "evidence": [],
                   "diagnostics": [{"check": "reflex", "where": [0, 1], "score": 0.0, "why": "reasoning handled"}]}]
        self.assertIn("inside, not reflex at [0, 1]: reasoning handled", runner.failure_evidence(graded))
        graded[0]["diagnostics"][0]["score"] = 1.0
        self.assertEqual(runner.failure_evidence(graded), [])


class Codec(unittest.TestCase):
    def test_every_prompt_round_trips(self):
        for target in prompts.discover():
            self.assertTrue(prompts.roundtrips(target), target)

    def test_an_overlay_changes_only_its_prompt(self):
        target = "abilities/reflection/reflection.scx#select_instructions"
        text = "Learn what you are told once. Quote 'this' & <that> \"too\"."
        with tempfile.TemporaryDirectory() as directory:
            diff = prompts.overlay({target: text}, directory)
            self.assertEqual(prompts.read(target, base=directory), text)
            changed = [line for line in diff.splitlines() if line.startswith(("+<", "-<", "+ ", "- "))]
            self.assertEqual(len(changed), 2, diff)


if __name__ == "__main__":
    unittest.main()
