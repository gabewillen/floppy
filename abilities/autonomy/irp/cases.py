# The autonomy's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
INPUT_EVENT = "bot.ability.autonomy.input"
DONE = "bot.ability.autonomy.output"
DB = "'/memory/floppy.sqlite'"
SPEECH = "bot.ability.listening.speech.output"
INPUT = {"focus": None, "focus_candidates": []}


def compact(value):
    """value as JSON.stringify writes it."""
    return json.dumps(value, separators=(",", ":"))


def speech(source_ids):
    """A transcribed utterance heard from source_ids."""
    return {
        "content": "what is the weather like?",
        "content_type": "text/plain",
        "voice_detection": {"segments": [{"start_seconds": 0.0, "end_seconds": 0.4, "confidence": 0.9}]},
        "media_type": "audio/pcm",
        "sample_rate_hz": 48000,
        "channels": 1,
        "source_ids": source_ids,
    }


TEXT = {"observation": {"event": "text", "data": "hello"}}


def params(database="null", memory_timeout_ms=None):
    """The child's <param>s: its memory, and its memory_timeout_ms when given."""
    params = [memory(database)]
    if memory_timeout_ms is not None:
        params.append(("memory_timeout_ms", js(memory_timeout_ms)))
    return params


def turn_of(turn, stimulus):
    """The step that hands the child one turn."""
    return send(INPUT_EVENT, js({"turn": turn, "input": INPUT, "stimulus": stimulus}))


def run(event, data, turn=1):
    return turn_of(turn, {"event": event, "data": data, "source": "listening"})


def perceived(turn, *stimuli):
    """A turn that perceived stimuli ((event, data) each) together, as the
    body's bot.perceived carries them."""
    percepts = [{"stimulus": {"event": event, "data": data, "source": "listening"}, "source": "listening",
                 "participants": []} for event, data in stimuli]
    return turn_of(turn, {"event": "bot.perceived", "data": {"percepts": percepts}, "source": "session.floppy"})


def case(test_id, number, comment, steps, turn, database="null", memory_timeout_ms=None, **kwargs):
    """A case whose first step hands the child `turn` (run's or perceived's)."""
    S.case(test_id, number, comment, [turn] + steps, params=params(database, memory_timeout_ms), **kwargs)


def answer(turn, reason, candidates, extra=None):
    cond = (f"_event.data.turn === {turn} && _event.data.handled === false && _event.data.reason === {js(reason)}"
            f" && JSON.stringify(_event.data.candidates) === {js(compact(candidates))} && _event.data.selected === undefined")
    if extra:
        cond += " && " + extra
    return expect(DONE, cond)


ADMITTED = (
    "_event.data.turn === 2 && _event.data.handled === true && _event.data.reason === 'behavior'"
    " && JSON.stringify(_event.data.candidates) === '[\"SpeechHeard\"]'"
    " && _event.data.selected.length === 1"
    " && _event.data.selected[0].event === 'bot.ability.communication.input'"
    " && _event.data.selected[0].reason === 'seeded speech admit via communication'"
    " && JSON.stringify(_event.data.selected[0].data.source_ids) === '[\"caller\"]'"
    " && JSON.stringify(_event.data.selected[0].data.target_ids) === '[]'"
    " && _event.data.selected[0].data.content === 'what is the weather like?'"
    " && _event.data.selected[0].data.content_type === 'text/plain'"
    " && _event.data.selected[0].data.sample_rate_hz === 48000 && _event.data.selected[0].data.channels === 1"
    " && _event.data.selected[0].data.parent.event === 'bot.ability.listening.speech.output'"
    " && Object.keys(_event.data.selected[0].data).length === 7")

S.statement(1, "Without memory (`memory` `null`), a stimulus no seed triggers runs nothing: it answers `bot.ability.autonomy.output` `{turn, handled: false, candidates: [], reason: \"no-behaviors\"}`, once.")
S.statement(2, "A `bot.ability.listening.speech.output` stimulus runs the SpeechHeard seed first, with the <param>s turn, input and stimulus. Speech with a source is handled: it ends with `{turn, handled: true, selected, candidates: [\"SpeechHeard\"], reason: \"behavior\"}`, `selected` the seed's one `bot.ability.communication.input` selection (the speech's `source_ids`, empty `target_ids`, `content`, `content_type`, `sample_rate_hz`, `channels`, and `parent`, the speech itself), trusted and naming no target (the body sends it to itself).")
S.statement(3, "A seed that does not handle the turn passes it to the learned inventory; without memory the turn is unhandled: `{turn, handled: false, candidates: [\"SpeechHeard\"], reason: \"unhandled\"}`.")
S.statement(4, "With memory, one transaction of the Memory ability at the `memory` <param> (decision D266) loads the ACTIVE `turn` behaviors whose trigger is the stimulus's event; when none matches, the turn is `no-behaviors`, or `unhandled` with the seed in `candidates` when a seed ran.")
S.statement(5, "A load that fails or outlasts `memory_timeout_ms` matches no behaviors and the turn goes on, as a load with no rows does.")
S.statement(6, "A `bot.perceived` stimulus (the body's percepts taken together) is matched percept by percept, in arrival order, each percept's stimulus as the stimulus: the turn is handled only when every percept was, `selected` the handling behaviors' selections in arrival order; otherwise it is `unhandled` (or `no-behaviors` when nothing was tried), and `acted` `{percepts, acts}` names the handled percepts' positions and their selections when any was.")

case(100, 1, "with no memory and no seed, nothing runs.",
     [answer(1, "no-behaviors", []), wait("300ms")], turn=run("bot.input", TEXT), clock="wall")
case(101, 1, "non-speech stimulus data of any shape runs nothing.",
     [answer(4, "no-behaviors", [])], turn=run("environment.sound", "noise", turn=4))

case(110, 2, "speech with a source is admitted as communication input.",
     [expect(DONE, ADMITTED)], turn=run(SPEECH, speech(["caller"]), turn=2))
case(111, 2, "the seed answers before the learned inventory is loaded.",
     [expect(DONE, ADMITTED)], turn=run(SPEECH, speech(["caller"]), turn=2), database=DB)

case(120, 3, "speech with no source is unhandled, the seed named.",
     [answer(3, "unhandled", ["SpeechHeard"])], turn=run(SPEECH, speech([]), turn=3))

case(130, 4, "a fresh inventory matches nothing: no-behaviors.",
     [answer(5, "no-behaviors", [])], turn=run("bot.input", TEXT, turn=5), database=DB)
case(131, 4, "after an unhandled seed, a fresh inventory leaves the turn unhandled.",
     [answer(6, "unhandled", ["SpeechHeard"])], turn=run(SPEECH, speech([]), turn=6), database=DB)

case(140, 5, "a load that fails matches nothing, and the turn goes on.",
     [answer(7, "no-behaviors", [])], turn=run("bot.input", TEXT, turn=7), database="'/elsewhere/floppy.sqlite'")
case(141, 5, "a load that outlasts memory_timeout_ms matches nothing, and the turn goes on.",
     [answer(8, "unhandled", ["SpeechHeard"])],
     turn=run(SPEECH, speech([]), turn=8), database=DB, memory_timeout_ms=0)

case(160, 6, "two sourced speech percepts are each admitted: the turn is handled with both selections.",
     [expect(DONE, "_event.data.turn === 9 && _event.data.handled === true && _event.data.reason === 'behavior'"
                     " && JSON.stringify(_event.data.candidates) === '[\"SpeechHeard\",\"SpeechHeard\"]'"
                     " && _event.data.selected.length === 2"
                     " && _event.data.selected[0].data.source_ids[0] === 'caller'"
                     " && _event.data.selected[1].data.source_ids[0] === 'gabe'")],
     turn=perceived(9, (SPEECH, speech(["caller"])), (SPEECH, speech(["gabe"]))))
case(161, 6, "a batch only partly handled is unhandled, and carries what the seed did as acted.",
     [answer(10, "unhandled", ["SpeechHeard"],
               "JSON.stringify(_event.data.acted.percepts) === '[1]' && _event.data.acted.acts.length === 1"
               " && _event.data.acted.acts[0].event === 'bot.ability.communication.input'")],
     turn=perceived(10, ("bot.input", TEXT), (SPEECH, speech(["caller"]))))
case(162, 6, "a batch no seed triggers runs nothing: no-behaviors, no acted.",
     [answer(11, "no-behaviors", [], "_event.data.acted === undefined")],
     turn=perceived(11, ("bot.input", TEXT), ("environment.sound", "noise")))

S.statement(7, "It is long-lived (mosfet.py's Cognition holds its Autonomy, decision D272): it answers each `bot.ability.autonomy.input` in turn, and a `bot.ability.processing.cancel` ends the turn in flight unanswered, so the next input is the next turn.")

case(170, 7, "two turns are answered in order by one session.",
     [expect(DONE, ADMITTED), run(SPEECH, speech([]), turn=3), answer(3, "unhandled", ["SpeechHeard"])],
     turn=run(SPEECH, speech(["caller"]), turn=2))
case(171, 7, "a cancelled turn is never answered; the next turn is.",
     [send("bot.ability.processing.cancel", js({})), run("bot.input", TEXT, turn=13),
      answer(13, "no-behaviors", [])],
     turn=run("bot.input", TEXT, turn=12), database=DB)

# Learned candidates (decision D273): the harness stores turn behaviors in
# the memory it serves, each triggered by bot.input, then hands the turn.
IGNORE = "bot.ability.cognition.ignore"
# The harness is the run's root session ("session"); a selection for it
# comes to it. Autonomy's own session stands in for the cognition, which
# the ignore goes to (autonomy takes no ignore, so it is dropped).
HARNESS = "session"
OWN = "session.autonomy"
HELLO = {"text": "hello"}


def behavior(donedata):
    """A turn behavior's source: it takes the <param>s turn and input and
    answers by its donedata."""
    return ('<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="done">'
            '<datamodel><data id="turn" expr="0"/><data id="input" expr="null"/></datamodel>'
            f'<final id="done"><donedata><content expr="{donedata}"/></donedata></final></scxml>')


SILENT = '<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="waiting"/></scxml>'
BROKEN = "<scxml"
DECLINES = behavior("({turn: turn, handled: false})")
HANDLES = behavior(f"({{turn: turn, handled: true, selected: [{{event: '{IGNORE}', data: {{}}, reason: 'handled', target: '{OWN}'}}]}})")


def stored(candidates):
    """The steps that store candidates ((name, source) each, tried in name
    order) as ACTIVE turn behaviors triggered by bot.input."""
    statements = []
    for name, source in candidates:
        statements.append({"sql": "INSERT INTO bot_behavior (name, source) VALUES (?, ?)",
                           "parameters": [name, S.behavior(source)]})
        statements.append({"sql": "INSERT INTO bot_behavior_trigger (behavior_name, \"trigger\") VALUES (?, 'bot.input')",
                           "parameters": [name]})
    return [send("bot.ability.memory.input", js({"id": "stored", "statements": statements}), target="#_memory"),
            expect("bot.ability.memory.output", "_event.data.id === 'stored'")]


def tried(candidates, handled, reason, extra=None, turn=3):
    cond = (f"_event.data.turn === {turn} && _event.data.handled === {js(handled)} && _event.data.reason === {js(reason)}"
            f" && JSON.stringify(_event.data.candidates) === {js(compact(candidates))}")
    if not handled:
        cond += " && _event.data.selected === undefined"
    if extra:
        cond += " && " + extra
    return expect(DONE, cond)


def learned(test_id, number, comment, candidates, steps, turn=3, database=DB, **kwargs):
    """A case that stores candidates, then hands autonomy one bot.input turn."""
    S.case(test_id, number, comment, stored(candidates) + [run("bot.input", HELLO, turn=turn)] + steps,
           params=params(database, kwargs.pop("memory_timeout_ms", None)) + kwargs.pop("extra_params", []), **kwargs)


def failed(candidates, rejection):
    return expect(DONE, f"_event.data.turn === 3 && _event.data.handled === false && _event.data.failed === true"
                        f" && _event.data.reason === {js(rejection)} && JSON.stringify(_event.data.candidates) === {js(compact(candidates))}")


HANDLED_SELECTED = (f"_event.data.selected.length === 1 && _event.data.selected[0].event === '{IGNORE}'"
                    " && _event.data.selected[0].reason === 'handled'")

S.statement(8, "Each matched candidate (`{name, source}`, `source` the path of its program, decision D121) runs in turn as the child `candidate` by `<invoke srcexpr>` on its source, with the <param>s `turn` and `input` (the percept's data with `focus` and `focus_candidates`). A candidate that answers by its donedata `{turn, handled: true, selected}` for this turn with at least one selection, all sent, has its use persisted (one transaction of the Memory ability at the `memory` <param>; one that fails or outlasts `memory_timeout_ms` takes the same effect) and handles the percept: `{turn, handled: true, selected, candidates, reason: \"behavior\"}`, `selected` the candidate's own selections.")
S.statement(9, "The handled candidate's selections are sent, in order, each to the session its `target` names (`#_scxml_` + target, the session ids its turn's selections carry), before its use is persisted.")
S.statement(10, "An answer that does not handle this turn (`handled` not `true`, no selections, or another turn) is not handled, and the next candidate runs; when none is left, the turn is `unhandled`, `candidates` naming each one tried, in order.")
S.statement(11, "A candidate that does not answer within `silence_ms` is passed over, and the next runs.")
S.statement(12, "A candidate whose source cannot run has its failure persisted, and the next runs.")
S.statement(13, "A handled candidate a selection of which cannot be sent (a send that raises `error.*`) has its failure persisted, and autonomy fails the turn: `{turn, handled: false, failed: true, candidates, reason}`, reason `Autonomy behavior dispatch failed: <error> (selected <event> to <target>, ...)` (each selection, `with no target` for one without; decision D221); the rest do not run.")
learned(200, 8, "a handled candidate answers the turn with its own selections.", [("A", HANDLES)],
        [tried(["A"], True, "behavior", HANDLED_SELECTED)])
learned(201, 8, "the candidate gets the turn and input <param>s.",
        [("Echo", behavior(f"({{turn: turn, handled: true, selected: [{{event: '{IGNORE}', data: {{turn: turn, text: input.text}}, target: '{OWN}'}}]}})"))],
        [tried(["Echo"], True, "behavior", "_event.data.selected[0].data.turn === 7 && _event.data.selected[0].data.text === 'hello'", turn=7)],
        turn=7)

learned(210, 9, "a selection is sent to its target, and the candidate answers.",
        [("Say", behavior(f"({{turn: turn, handled: true, selected: [{{event: 'bot.say', data: {{text: 'hi'}}, target: '{HARNESS}'}}]}})"))],
        [expect("bot.say", "_event.data.text === 'hi'"),
         tried(["Say"], True, "behavior", "_event.data.selected.length === 1 && _event.data.selected[0].event === 'bot.say'")])
learned(211, 9, "selections are sent in order.",
        [("Focus", behavior(f"({{turn: turn, handled: true, selected: [{{event: 'bot.focus_device', data: {{device: 'kitchen'}}, target: '{HARNESS}'}}, {{event: 'bot.say', data: {{}}, target: '{HARNESS}'}}]}})"))],
        [expect("bot.focus_device", "_event.data.device === 'kitchen'"), expect("bot.say"),
         tried(["Focus"], True, "behavior", "_event.data.selected[0].event === 'bot.focus_device' && _event.data.selected[1].event === 'bot.say'")])

learned(220, 10, "handled false is not handled.", [("Declines", DECLINES)], [tried(["Declines"], False, "unhandled")])
learned(221, 10, "handled with no selections is not handled.",
        [("Empty", behavior("({turn: turn, handled: true, selected: []})"))], [tried(["Empty"], False, "unhandled")])
learned(222, 10, "an answer for another turn is not handled.",
        [("Stale", behavior(f"({{turn: turn + 1, handled: true, selected: [{{event: '{IGNORE}', data: {{}}}}]}})"))],
        [tried(["Stale"], False, "unhandled")])
learned(223, 10, "the next runs after a declining candidate, and the first that handles answers.",
        [("A", DECLINES), ("B", HANDLES)], [tried(["A", "B"], True, "behavior", HANDLED_SELECTED)])
learned(224, 10, "every candidate declining leaves the turn unhandled, each named in order.",
        [("A", DECLINES), ("B", DECLINES), ("C", DECLINES)], [tried(["A", "B", "C"], False, "unhandled")])
learned(225, 10, "a candidate after the one that handles never runs.",
        [("A", HANDLES), ("B", BROKEN)], [tried(["A"], True, "behavior", HANDLED_SELECTED)])

learned(230, 11, "a silent candidate is passed over after silence_ms.",
        [("A", SILENT), ("B", HANDLES)], [tried(["A", "B"], True, "behavior", HANDLED_SELECTED)],
        extra_params=[("silence_ms", js(200))], clock="wall")
learned(231, 11, "a silent last candidate leaves the turn unhandled.",
        [("A", SILENT)], [tried(["A"], False, "unhandled")], extra_params=[("silence_ms", js(200))], clock="wall")

learned(240, 12, "a source that cannot run is passed over, and the next runs.",
        [("A", BROKEN), ("B", HANDLES)], [tried(["A", "B"], True, "behavior", HANDLED_SELECTED)])
learned(241, 12, "a last source that cannot run leaves the turn unhandled.",
        [("A", BROKEN)], [tried(["A"], False, "unhandled")])

learned(250, 13, "a selection that names no target cannot be sent, and the rest do not run.",
        [("A", behavior("({turn: turn, handled: true, selected: [{event: 'bot.ability.reasoning.input', data: {}}]})")), ("B", HANDLES)],
        [failed(["A"], "Autonomy behavior dispatch failed: error.communication (selected bot.ability.reasoning.input with no target)")])
learned(251, 13, "a selection whose target no session of the run answers cannot be sent.",
        [("Nowhere", behavior("({turn: turn, handled: true, selected: [{event: 'bot.say', data: {}, target: 'session.nowhere'}]})"))],
        [failed(["Nowhere"], "Autonomy behavior dispatch failed: error.communication (selected bot.say to session.nowhere)")])
learned(252, 13, "the rejection names each selection, in order, with its target or that it has none.",
        [("Both", behavior("({turn: turn, handled: true, selected: [{event: 'bot.say', data: {}, target: 'session.nowhere'}, {event: 'bot.ability.reasoning.input', data: {}}]})"))],
        [failed(["Both"], "Autonomy behavior dispatch failed: error.communication (selected bot.say to session.nowhere, bot.ability.reasoning.input with no target)")])
