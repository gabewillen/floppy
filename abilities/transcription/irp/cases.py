# The transcription's statements and cases (tests/irp/harness.py writes them).
#
# The transcription runs the speech service's stream operation for real
# against the fake backend, whose WebSocket sessions each case scripts as
# rounds of api.willen.dev's /v1/audio/stream frames: the first round once
# a session opens, round n once its n-th sound (a binary frame) arrives.
# Each case's audio is in its sources/<id>/media/ (16 kHz mono 16-bit WAV,
# 0.1 s: voiced*.wav of constant non-zero samples).

S = suite
S.media({"voiced.wav": wav(1000), "voiced2.wav": wav(1000), "own.wav": wav(1000)})
SCORED = "bot.ability.listening.sensitivity.output"
TURN = "bot.ability.listening.transcription.turn"
READY = expect("bot.ability.ready")
DONE = expect("done.invoke.transcription")

READY_FRAME = {"type": "ready"}


def said(text, start=0.0, end=0.1):
    """The frames of one turn the session heard with words."""
    return [{"type": "speech_start", "t": start}, {"type": "speech_end", "t": end},
            {"type": "final", "text": text, "confidence": 0.9, "words": [], "t": end},
            {"type": "turn_end", "reason": "eot", "p": 0.9, "text": text, "t": end}]


def scored(name, level=10, **extra):
    data = {"sound": {"audio": f"media/{name}", "media_type": "audio/wav", "sample_rate_hz": 16000, "channels": 1},
            "perceived_level_db": level}
    data.update(extra)
    return send(SCORED, js(data))


def turn(text, audio="voiced.wav", extra="", heard=0):
    frame = "_event.data.frames[0]"
    return expect(TURN, f"_event.data.text === {js(text)} && _event.data.confidence === 0.9"
                        f" && _event.data.frames.length === 1 && {frame}.audio === 'media/floppy-heard-{heard}.pcm'"
                        f" && {frame}.media_type === 'audio/pcm' && {frame}.sample_rate_hz === 16000 && {frame}.channels === 1"
                        f" && _event.data.sensed.sound.audio === 'media/{audio}' && _event.data.participants.length === 0" + extra)


S.statement(1, "It opens a streaming hearing session (the speech service's operation `stream`) and, on the session's first `bot.ability.hearing.speech.stream.ready`, sends `bot.ability.ready` to its parent (decision D242).")
S.statement(2, "Each `bot.ability.listening.sensitivity.output` is streamed; the session's `turn_ended` ends the turn, and a turn with words goes to the parent as `bot.ability.listening.transcription.turn` `{text, confidence, frames, sensed, voice_detection, participants}`: its words, the lowest transcript confidence, its sounds, its loudest scored sound, its voice spans in seconds from the turn's start, and its participants. Scored sounds that arrive before the session is ready are held (`<event:defer/>`) and streamed once it is.")
S.statement(3, "The bot's own voice arriving as predicted (`producing`, with a `perceived_level_db` below `product_threshold_db`) is not streamed and does not join the turn.")
S.statement(4, "A turn without words is let go; what is heard from then on is the next turn's.")
S.statement(5, "A session lost before its first ready (here, a backend that does not stream) ends the transcription in its final state `unavailable`, logged (`transcription`), so the parent hears through the interpretation; so does `enabled` false, at once and unlogged, and a session that does not open within `open_timeout_ms`.")
S.statement(6, "A session lost after it was ready is logged, and after the backoff (`first_backoff_ms`, doubling up to `last_backoff_ms`) a new one is opened, with no second `bot.ability.ready`; the turn it was hearing is lost.")

S.case(100, 1, "the transcription is ready once its session is.", [READY], backend=[], websocket=[[[READY_FRAME]]])

S.case(110, 2, "a streamed sound and the session's end of the turn are handed on as the turn's words.",
       [READY, scored("voiced.wav"),
        turn("what is the weather", extra=" && _event.data.sensed.perceived_level_db === 10"
                                          " && _event.data.voice_detection.segments.length === 1"
                                          " && _event.data.voice_detection.segments[0].start_seconds === 0"
                                          " && _event.data.voice_detection.segments[0].end_seconds === 0.1")],
       backend=[], websocket=[[[READY_FRAME], said("what is the weather")]])
S.case(111, 2, "a sound scored before the session is ready is streamed once it is.",
       [scored("voiced.wav"), READY, turn("early")],
       backend=[], websocket=[[[READY_FRAME], said("early")]])

S.case(120, 3, "the bot's own voice arriving as predicted is not streamed; the next sound is.",
       [READY, scored("own.wav", level=0, producing=True), scored("voiced2.wav"), turn("cut in", audio="voiced2.wav")],
       backend=[], websocket=[[[READY_FRAME], said("cut in")]])

S.case(130, 4, "a turn without words is let go; the next turn has only its own sounds.",
       [READY, scored("voiced.wav"), wait("500ms"), scored("voiced2.wav"), turn("second", audio="voiced2.wav", heard=1)],
       backend=[], websocket=[[[READY_FRAME], [{"type": "turn_end", "reason": "eot", "p": 0.9, "text": "", "t": 0.1}],
                               said("second", 0.1, 0.2)]])

S.case(140, 5, "a backend that does not stream ends the transcription, logged.", [DONE], backend=[],
       logs={"transcription": ["no streaming session (unavailable): the listening hears through the interpretation"]})
S.case(141, 5, "enabled false ends the transcription at once, unlogged.", [DONE], params=[("enabled", "false")],
       backend=[], websocket=[[[READY_FRAME]]], logs={"transcription": []})
S.case(142, 5, "a session that does not open within open_timeout_ms ends the transcription, logged.",
       [DONE], params=[("open_timeout_ms", "0")], backend=[], websocket=[[[]]],
       logs={"transcription": ["the streaming session did not open in time"]})

S.case(150, 6, "a session lost after it was ready is reopened after the backoff, with no second ready; the lost turn's sound is not handed on.",
       [READY, scored("voiced.wav"), wait("1s"), scored("voiced2.wav"), turn("again", audio="voiced2.wav", heard=1)],
       params=[("first_backoff_ms", "10")], backend=[],
       websocket=[[[READY_FRAME], [{"type": "error", "message": "gone"}]], [[READY_FRAME], said("again")]],
       logs={"transcription": ["the streaming session was lost (unavailable); reconnecting in 10 ms"]})
