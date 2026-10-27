# The sound_classification's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.hearing.sound.classification.input"
OUTPUT = "bot.ability.hearing.sound.classification.output"
CONFIG = ()


def asked(params, event, cond):
    """A case that asks the long-lived child once: its params that are not
    the child's configuration (CONFIG) are the request's fields, and the
    child answers `event` of which `cond` holds."""
    return dict(steps=[request(INPUT, [param for param in params if param[0] not in CONFIG]), expect(event, answered(cond))],
                params=[param for param in params if param[0] in CONFIG])


SOUND = [("audio", "'media/bark.wav'"), ("media_type", "'audio/wav'")]


def kind(expr):
    return SOUND + [("kind", expr)]


def unlabeled():
    return f"_event.data.labels.length === 0 && !('confidence' in _event.data)"


def labeled(label):
    return (f"_event.data.labels.length === 1 && _event.data.labels[0] === {js(label)}"
            " && _event.data.confidence === 1")


S.statement(1, "It is long-lived: it takes each sound as a request `bot.ability.hearing.sound.classification.input` `{id, audio, media_type?, kind?}` and answers its parent at once with `bot.ability.hearing.sound.classification.output`, the request's `id` and the label. It sends nothing else.")
S.statement(2, "A kind that is a string with text is trimmed and is the one label, with confidence 1.0: `{id, labels: [kind], confidence: 1.0}`.")
S.statement(3, "A kind that is null, absent or blank is unlabeled: `{id, labels: []}`, with no confidence.")

S.case(100, 1, "with a kind, the sound is answered at once with its label.",
       **asked(kind("'dog bark'"), OUTPUT, labeled("dog bark")))
S.case(101, 1, "with no kind, it is still answered at once, unlabeled.",
       **asked(SOUND, OUTPUT, unlabeled()))
S.case(110, 2, "the kind is trimmed into the one label with confidence 1.0.",
       **asked(kind("'  doorbell\\t'"), OUTPUT, labeled("doorbell")))
S.case(120, 3, "a null kind is unlabeled.",
       **asked(kind("null"), OUTPUT, unlabeled()))
S.case(121, 3, "a blank kind is unlabeled.",
       **asked(kind("'   '"), OUTPUT, unlabeled()))
