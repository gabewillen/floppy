# The voice_affect's statements and cases (tests/irp/harness.py writes them).

import json

S = suite
# Constant non-zero 16-bit PCM: short.pcm 0.2 s and long.pcm 1 s at 16 kHz
# mono, voice.wav 0.5 s at 24 kHz mono in a WAV container.
S.media({"short.pcm": pcm(1000, 0.2), "long.pcm": pcm(1000, 1), "voice.wav": wav(1000, 0.5, 24000)})
INPUT = "bot.ability.classifying.input"
OUTPUT = "bot.ability.classifying.output"
FAILED = "bot.ability.failed"
CONFIG = ('timeout_ms',)


def asked(params, event, cond):
    """A case that asks the long-lived child once: its params that are not
    the child's configuration (CONFIG) are the request's fields, and the
    child answers `event` of which `cond` holds."""
    return dict(steps=[request(INPUT, [param for param in params if param[0] not in CONFIG]), expect(event, answered(cond))],
                params=[param for param in params if param[0] in CONFIG])


HAPPY = {"label": "happy", "scores": {"happy": 0.9, "neutral": 0.1}, "valence": 0.9, "confidence": 0.9,
         "snr_db": 20.0, "model": "emotion2vec/emotion2vec_plus_large"}


def emotion(rate, body=None, status=200):
    return {"path": f"/v1/emotion?sample_rate={rate}", "status": status, "body": HAPPY if body is None else body}


def segment(name, start=None, end=None, rate=16000, media_type="audio/pcm"):
    value = {"audio": f"media/{name}", "media_type": media_type, "sample_rate_hz": rate, "channels": 1}
    if start is not None:
        value.update(start_seconds=start, end_seconds=end)
    return value

# A loaded host can take longer than the 3 s bound to start the service:
# cases that reach it wait longer (statement 3 tests the bound itself).
def segments(*items, timeout_ms="600000"):
    return [("segments", js(list(items))), ("timeout_ms", timeout_ms)]


def heard(affect):
    return f"JSON.stringify(_event.data) === {js(json.dumps(affect, separators=(',', ':'), sort_keys=True))}"


def unheard(message=None):
    """The failure answered when no tone was heard: `message` exactly, or
    any message."""
    if message is None:
        return "typeof _event.data.message === 'string' && Object.keys(_event.data).length === 1"
    return f"_event.data.message === {js(message)} && Object.keys(_event.data).length === 1"
AFFECT = {"label": "happy", "scores": {"happy": 0.9, "neutral": 0.1}, "valence": 0.9, "confidence": 0.9}

S.statement(1, "It is long-lived: each request is `bot.ability.classifying.input` `{id, segments}`. It hears the tone of the longest of the request's `segments` (the first of equals; a segment with no span counts as long enough) through the speech service's `classify` (`POST {base}/emotion?sample_rate=R`); the service's `bot.ability.classifying.output` is answered to its parent as `bot.ability.classifying.output` exactly `{id, label, scores, valence, confidence}` (mosfet.py's `voice.Affect`), and logs `listening.affect` `heard <label> <confidence>`.")
S.statement(2, "A segment shorter than `min_segment_seconds` (0.3 s) is never sent; with no segment left, nothing is asked and it answers `bot.ability.failed` `{id, message}`, the message being what it logs.")
S.statement(3, "A tone not heard (the service's `bot.ability.failed`, an answer that is not an affect, the service ending or unable to run, or no answer within `timeout_ms`, 3 s) answers `bot.ability.failed` `{id, message}` and logs `listening.affect` `failed: <why>`, the message (mosfet.py's classify answers None).")

S.case(100, 1, "the longest segment's tone is heard.",
       **asked(segments(segment("short.pcm", 0.0, 0.2), segment("long.pcm", 0.2, 1.2), segment("short.pcm", 1.2, 1.4)), OUTPUT, heard(AFFECT)), backend=[emotion(16000)], logs={"listening.affect": ["heard happy 0.9"]})
S.case(101, 1, "a WAV utterance with no span is heard at its own rate.",
       **asked(segments(segment("voice.wav", rate=24000, media_type="audio/wav")), OUTPUT, heard(AFFECT)), backend=[emotion(24000)])
S.case(102, 1, "an answer without scores, valence or confidence is still an affect.",
       **asked(segments(segment("long.pcm", 0.0, 1.0)), OUTPUT, heard({"label": "neutral", "scores": {}, "valence": None, "confidence": None})), backend=[emotion(16000, {"label": "neutral"})], logs={"listening.affect": ["heard neutral"]})

S.case(110, 2, "a segment under 0.3 s is never sent.",
       **asked(segments(segment("short.pcm", 0.0, 0.2)), FAILED, unheard("skipped: no segment of at least 0.3 s")), backend=[emotion(16000)], logs={"listening.affect": ["skipped: no segment of at least 0.3 s"]})
S.case(111, 2, "with no segments nothing is asked.",
       **asked([("segments", "[]")], FAILED, unheard("skipped: no segment of at least 0.3 s")))

S.case(120, 3, "an error status leaves no tone.",
       **asked(segments(segment("long.pcm", 0.0, 1.0)), FAILED, unheard("failed: Affect classification failed: the backend answered an error status.")), backend=[emotion(16000, {"detail": "too short: 0.10 s"}, 422)], logs={"listening.affect": ["failed: Affect classification failed: the backend answered an error status."]})
S.case(121, 3, "an answer that is not an affect leaves no tone.",
       **asked(segments(segment("long.pcm", 0.0, 1.0)), FAILED, unheard()), backend=[emotion(16000, {"label": ""})])
S.case(122, 3, "a segment that is not there cannot be read.",
       **asked(segments(segment("missing.pcm", 0.0, 1.0)), FAILED, unheard("failed: Affect classification failed: the audio cannot be read.")), logs={"listening.affect": ["failed: Affect classification failed: the audio cannot be read."]})
S.case(123, 3, "a tone not heard within timeout_ms is not waited for.",
       **asked(segments(segment("long.pcm", 0.0, 1.0), timeout_ms="1"), FAILED, unheard("failed: not heard within 1 ms")), backend=[emotion(16000)], clock="wall", logs={"listening.affect": ["failed: not heard within 1 ms"]})
