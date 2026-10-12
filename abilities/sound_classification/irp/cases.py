# The sound_classification's statements and cases (tests/irp/harness.py writes them).

S = suite
DONE = "done.invoke.sound_classification"
SOUND = [("audio", "'media/bark.wav'"), ("media_type", "'audio/wav'")]


def kind(expr):
    return SOUND + [("kind", expr)]


def unlabeled():
    return f"_event.data.labels.length === 0 && !('confidence' in _event.data)"


def labeled(label):
    return (f"_event.data.labels.length === 1 && _event.data.labels[0] === {js(label)}"
            " && _event.data.confidence === 1")


S.statement(1, "It takes the sound as the `<param>`s `audio`, `media_type` and `kind`, labels it once in `applying` and reaches its final `applied` at once; the label is its `<donedata>`, received by the invoker as `done.invoke.sound_classification`. It sends nothing else.")
S.statement(2, "A kind that is a string with text is trimmed and is the one label, with confidence 1.0: `{labels: [kind], confidence: 1.0}`.")
S.statement(3, "A kind that is null, absent or blank is unlabeled: `{labels: []}`, with no confidence. A kind that is no string is unlabeled too (the code takes only a string; the header names null and blank).")

S.case(100, 1, "with a kind, the classification is done at once with its label as donedata.",
       [expect(DONE, labeled("dog bark"))], params=kind("'dog bark'"))
S.case(101, 1, "with no params at all, it is still done at once, unlabeled.",
       [expect(DONE, unlabeled())])
S.case(110, 2, "the kind is trimmed into the one label with confidence 1.0.",
       [expect(DONE, labeled("doorbell"))], params=kind("'  doorbell\\t'"))
S.case(120, 3, "a null kind is unlabeled.",
       [expect(DONE, unlabeled())], params=kind("null"))
S.case(121, 3, "a blank kind is unlabeled.",
       [expect(DONE, unlabeled())], params=kind("'   '"))
S.case(122, 3, "a kind that is no string is unlabeled.",
       [expect(DONE, unlabeled())], params=kind("42"))
