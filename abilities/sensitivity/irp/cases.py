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


def level(value, producing=None):
    cond = f"_event.data.perceived_level_db === {js(value)}"
    if producing is not None:
        cond += f" && _event.data.producing === {js(producing)}"
    return cond


S.statement(1, "On start it offers nothing: it sends `bot.ability.offers` `{ability: \"sensitivity\", choices: []}` to its parent.")
S.statement(2, "Every arrival scored (`bot.ability.listening.sensitivity.score` `{stimulus}`) comes back as `bot.ability.listening.sensitivity.output` `{sound, parent, perceived_level_db, producing}`, its sound the stimulus's data, its parent the stimulus, and `producing` whether it arrived while the body was producing (true for every arrival inside the window, whoever made it); quiet, `producing` is false and the level is the sound's `received_level_db`, `null` when nothing was measured.")
S.statement(3, "`bot.ability.speaking.efference` `{mouth, duration}` with sound still to come (`duration` > 0) opens a producing window on that mouth, or re-arms the open one on the new duration; inside it, an arrival from another source is scored by its `received_level_db`.")
S.statement(4, "Inside the window, an arrival from the producing mouth is scored against the prediction: with no levels to compare, 0; with no learned path gain yet, the observed gain (`received_level_db - amplitude_db`) is learned and the level is 0; afterwards the level is `|observed - learned|`, and the learned gain is never retaught.")
S.statement(5, "The window lasts the command's duration: once it has passed, an arrival from the mouth is scored as quiet.")
S.statement(6, "A silent copy, `bot.ability.speaking.efference` with `duration` 0 (the mouth has nothing left to play, the original's silent copy), closes an open window at once, and in quiet opens none: an arrival from the mouth after it is scored as quiet.")

S.case(500, 1, "the sensitivity offers nothing on start.", [READY])
S.case(510, 2, "a quiet arrival comes back whole, at its received level.",
       [READY, send(SCORE, sound("mic", received=-30.5)),
        expect(OUTPUT, f"{level(-30.5, False)} && _event.data.sound.audio === 'media/a.wav' && _event.data.parent.source === 'mic' && _event.data.parent.event === 'environment.sound'")])
S.case(511, 2, "a quiet arrival with no measured level is scored null.",
       [READY, send(SCORE, sound("mic")), expect(OUTPUT, level(None))])
S.case(520, 3, "producing, an arrival from another source is scored by its received level.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        send(SCORE, sound("mic", received=-20, amplitude=-10)), expect(OUTPUT, level(-20, True))])
S.case(530, 4, "producing, the mouth's arrival with no levels to compare is scored 0.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        send(SCORE, sound("speaker")), expect(OUTPUT, level(0, True))])
S.case(531, 4, "producing, the first gain is learned (0), later arrivals are scored by their distance from it, and it is never retaught.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        send(SCORE, sound("speaker", received=-30, amplitude=-10)), expect(OUTPUT, level(0)),
        send(SCORE, sound("speaker", received=-25, amplitude=-10)), expect(OUTPUT, level(5)),
        send(SCORE, sound("speaker", received=-35, amplitude=-10)), expect(OUTPUT, level(5))])
S.case(540, 5, "after the window, the mouth's arrival is scored as quiet.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 0.05})),
        wait("300ms"),
        send(SCORE, sound("speaker", received=-30, amplitude=-10)), expect(OUTPUT, level(-30, False))],
       clock="wall")
S.case(541, 3, "a later copy re-arms the window on its own duration: past the first command's duration the mouth is still producing.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 0.05})), send(EFFERENCE, js({"mouth": "speaker", "duration": 60})),
        wait("300ms"),
        send(SCORE, sound("speaker")), expect(OUTPUT, level(0, True))],
       clock="wall")
S.case(550, 6, "a silent copy closes the open window: the mouth's arrival is scored as quiet.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 60})), send(EFFERENCE, js({"mouth": "speaker", "duration": 0})),
        send(SCORE, sound("speaker", received=-30, amplitude=-10)), expect(OUTPUT, level(-30, False))])
S.case(551, 6, "in quiet a silent copy opens no window.",
       [READY, send(EFFERENCE, js({"mouth": "speaker", "duration": 0})),
        send(SCORE, sound("speaker", received=-30, amplitude=-10)), expect(OUTPUT, level(-30, False))])
