# The sensitivity's statements and cases (tests/irp/harness.py writes them).

S = suite
SCORE = "bot.ability.listening.sensitivity.score"
OUTPUT = "bot.ability.listening.sensitivity.output"
EFFERENCE = "bot.ability.speaking.efference"
READY = expect("bot.ability.offers", "_event.data.ability === 'sensitivity' && _event.data.choices.length === 0")


def sound(source, received=None, amplitude=None):
    data = {"audio": "media/a.wav"}
    if received is not None:
        data["received_level_db"] = received
    if amplitude is not None:
        data["amplitude_db"] = amplitude
    return js({"stimulus": {"event": "environment.sound", "data": data, "source": source}})


def level(value):
    return f"_event.data.perceived_level_db === {js(value)}"


S.statement(1, "On start it offers nothing: it sends `bot.ability.offers` `{ability: \"sensitivity\", choices: []}` to its parent.")
S.statement(2, "Every arrival scored (`bot.ability.listening.sensitivity.score` `{stimulus}`) comes back as `bot.ability.listening.sensitivity.output` `{sound, parent, perceived_level_db}`, its sound the stimulus's data and its parent the stimulus; quiet, the level is the sound's `received_level_db`, `null` when nothing was measured.")
S.statement(3, "`bot.ability.speaking.efference` `{mouth, duration}` opens a producing window on that mouth; inside it, an arrival from another source is scored by its `received_level_db`.")
S.statement(4, "Inside the window, an arrival from the producing mouth is scored against the prediction: with no levels to compare, 0; with no learned path gain yet, the observed gain (`received_level_db - amplitude_db`) is learned and the level is 0; afterwards the level is `|observed - learned|`, and the learned gain is never retaught.")
S.statement(5, "The window lasts the command's duration: once it has passed, an arrival from the mouth is scored as quiet.")

S.case(500, 1, "the sensitivity offers nothing on start.", [READY])
S.case(510, 2, "a quiet arrival comes back whole, at its received level.",
       [READY, send(SCORE, sound("mic", received=-30.5)),
        expect(OUTPUT, f"{level(-30.5)} && _event.data.sound.audio === 'media/a.wav' && _event.data.parent.source === 'mic' && _event.data.parent.event === 'environment.sound'")])
S.case(511, 2, "a quiet arrival with no measured level is scored null.",
       [READY, send(SCORE, sound("mic")), expect(OUTPUT, level(None))])
S.case(520, 3, "producing, an arrival from another source is scored by its received level.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        send(SCORE, sound("mic", received=-20, amplitude=-10)), expect(OUTPUT, level(-20))])
S.case(530, 4, "producing, the mouth's arrival with no levels to compare is scored 0.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        send(SCORE, sound("speaker")), expect(OUTPUT, level(0))])
S.case(531, 4, "producing, the first gain is learned (0), later arrivals are scored by their distance from it, and it is never retaught.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        send(SCORE, sound("speaker", received=-30, amplitude=-10)), expect(OUTPUT, level(0)),
        send(SCORE, sound("speaker", received=-25, amplitude=-10)), expect(OUTPUT, level(5)),
        send(SCORE, sound("speaker", received=-35, amplitude=-10)), expect(OUTPUT, level(5))])
S.case(540, 5, "after the window, the mouth's arrival is scored as quiet.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 0.05})),
        wait("300ms"),
        send(SCORE, sound("speaker", received=-30, amplitude=-10)), expect(OUTPUT, level(-30))],
       clock="wall")
