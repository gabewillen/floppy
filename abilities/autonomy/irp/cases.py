# The autonomy's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT_EVENT = "bot.ability.autonomy.input"
DONE = "bot.ability.autonomy.output"
DB = "'/memory/floppy.sqlite'"
SPEECH = "bot.ability.listening.speech.output"
INPUT = {"focus": None, "focus_candidates": []}


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
            f" && JSON.stringify(_event.data.candidates) === {js(js(candidates))} && _event.data.selected === undefined")
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
