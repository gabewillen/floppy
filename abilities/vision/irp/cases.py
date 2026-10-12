# The vision's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.vision.classification.input"
OUTPUT = "bot.ability.vision.classification.output"


def reading(kind, content):
    return js({"kind": kind, "content": content})


def classified(kind):
    return f"_event.data.kind === {js(kind)} && _event.data.confidence === 1"

# On the logical clock a delayed send to itself fires once the run is idle:
# a step after which nothing else arrives.
QUIET = wait("1s")


S.statement(1, "On start it waits in `Unclassified` and sends nothing; it is long-lived: it never reaches a final state.")
S.statement(2, "`bot.ability.vision.classification.input` `{kind: \"text\", content}` whose text is not blank is classified as text: it answers its parent with `bot.ability.vision.classification.output` `{kind: \"text\", confidence: 1.0}`.")
S.statement(3, "An input `{kind: \"image\", content}` with a reference (non-empty content) is classified as image: `{kind: \"image\", confidence: 1.0}`; classifying reads nothing, so the reference need not name a file.")
S.statement(4, "Anything else is unreadable: blank text or an image with an empty reference is answered `{kind: \"unreadable\", confidence: 1.0}`.")
S.statement(5, "After answering it is back in `Unclassified`: every input is answered once, in order.")
S.statement(6, "An input whose data does not satisfy its schema (`kind` other than text or image, or a missing `content`) is `error.execution` in its sender and is never classified.")

S.case(100, 1, "on start it sends nothing and is not done.",
       QUIET)
S.case(110, 2, "non-blank text is text.",
       [send(INPUT, reading("text", "Read this note.")), expect(OUTPUT, classified("text"))])
S.case(111, 2, "text with surrounding blanks is still text.",
       [send(INPUT, reading("text", "  hi \n")), expect(OUTPUT, classified("text"))])
S.case(120, 3, "an image with a reference is image.",
       [send(INPUT, reading("image", "media/receipt.png")), expect(OUTPUT, classified("image"))])
S.case(130, 4, "blank text is unreadable.",
       [send(INPUT, reading("text", " \t\n")), expect(OUTPUT, classified("unreadable"))])
S.case(131, 4, "empty text is unreadable.",
       [send(INPUT, reading("text", "")), expect(OUTPUT, classified("unreadable"))])
S.case(132, 4, "an image with an empty reference is unreadable.",
       [send(INPUT, reading("image", "")), expect(OUTPUT, classified("unreadable"))])
S.case(140, 5, "three inputs are answered once each, in order.",
       [send(INPUT, reading("image", "media/a.png")), send(INPUT, reading("text", "")),
        send(INPUT, reading("text", "words")),
        expect(OUTPUT, classified("image")), expect(OUTPUT, classified("unreadable")),
        expect(OUTPUT, classified("text"))])
S.case(150, 6, "a kind outside the schema is error.execution in the sender and is not answered.",
       [send(INPUT, reading("video", "media/a.mp4")), expect("error.execution")] + QUIET)
S.case(151, 6, "a missing content is error.execution in the sender and is not answered.",
       [send(INPUT, js({"kind": "text"})), expect("error.execution")] + QUIET)
