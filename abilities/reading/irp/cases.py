# The reading's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.reading.input"
OUTPUT = "bot.ability.reading.output"
FAILED = "bot.ability.reading.failed"
CHAT = "/chat/completions"
# Under load the generate service can take long to start: the wall-clock
# cases give DecodingImage room so only case 160 tests its bound.
SLOW = ("stage_timeout_ms", "120000")


def reading(kind, content):
    return js({"kind": kind, "content": content})


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def output(text, source_kind, confidence=1):
    return (f"_event.data.text === {js(text)} && _event.data.source_kind === {js(source_kind)}"
            f" && _event.data.confidence === {js(confidence)}")


def failure(stage, message):
    return f"_event.data.stage === {js(stage)} && _event.data.message === {js(message)}"


S.statement(1, "It offers nothing and answers only what it is given: on start it sends its parent nothing, and an input `bot.ability.reading.input` `{kind, content}` whose data its schema refuses (a kind other than `text` or `image`) is never routed to it (`error.execution` in the sender).")
S.statement(2, "A text input whose text is not blank is classified as text (confidence 1) and comes back as `bot.ability.reading.output` `{text, source_kind: \"text\", confidence}`, its text the input's content unchanged; no model is called.")
S.statement(3, "A text input whose text is blank, or an image input with an empty reference, is classified as unreadable and comes back as `bot.ability.reading.output` `{text: \"\", source_kind: \"unreadable\", confidence: 1}`; no model is called.")
S.statement(4, "An image input with a reference is classified as image and read by the generate service (one user message: the original's ImageDecoder prompt as a text part and the reference as an image part, the image read from the granted media directory); the reply, trimmed, comes back as `bot.ability.reading.output` `{text, source_kind: \"image\", confidence: 1}`.")
S.statement(5, "A model call that fails answers `bot.ability.reading.failed` `{stage: \"image_decoding\", message: \"Reading image decoder failed: <code>.\"}`, the generate service's failure code in the message, and the reading returns to Unfocused, taking the next input as on arrival.")
S.statement(6, "An image reference the generate service cannot read (no such file in the granted media directory) rejects the model call's start: the reading answers `bot.ability.reading.failed` `{stage: \"image_decoding\", message: \"the image decoding could not run (error.execution)\"}`.")
S.statement(7, "DecodingImage is bounded by `stage_timeout_ms` (30000 by default, a `<param>`): when it passes first, the reading answers `bot.ability.reading.failed` `{stage: \"image_decoding\", message: \"the image decoding timed out after <stage_timeout_ms> ms\"}` and the model call is cancelled.")
S.statement(8, "An input that arrives while Focused is deferred: it is taken, in order, once the reading is back in Unfocused, so every input gets exactly one answer in the order the inputs arrived.")

S.case(100, 1, "the reading sends nothing on start: nothing arrives before the first answer.",
       [wait("300ms"), send(INPUT, reading("text", "Read this note.")), expect(OUTPUT, output("Read this note.", "text"))],
       params=[SLOW], clock="wall")
S.case(101, 1, "an input whose kind its schema refuses is never routed: the sender gets error.execution.",
       [send(INPUT, reading("audio", "media/a.wav")), expect("error.execution"),
        send(INPUT, reading("text", "next")), expect(OUTPUT, output("next", "text"))])

S.case(110, 2, "a text input is read as its own text.",
       [send(INPUT, reading("text", "Read this note.")), expect(OUTPUT, output("Read this note.", "text"))])
S.case(111, 2, "a text input keeps its surrounding white space (text is passed through untrimmed).",
       [send(INPUT, reading("text", "  Total: 3\n")), expect(OUTPUT, output("  Total: 3\n", "text"))])

S.case(120, 3, "blank text is unreadable.",
       [send(INPUT, reading("text", "  \n ")), expect(OUTPUT, output("", "unreadable"))])
S.case(121, 3, "an image input with an empty reference is unreadable.",
       [send(INPUT, reading("image", "")), expect(OUTPUT, output("", "unreadable"))])

S.case(130, 4, "an image is read by the model and its reply trimmed.",
       [send(INPUT, reading("image", "media/receipt.png")), expect(OUTPUT, output("Total due: $14.21", "image"))],
       backend=[{"path": CHAT, "status": 200, "body": completion("  Total due: $14.21\n")}], params=[SLOW], clock="wall")
S.case(131, 4, "an absolute reference to the granted media directory is read too.",
       [send(INPUT, reading("image", "/media/receipt.png")), expect(OUTPUT, output("Total due: $14.21", "image"))],
       backend=[{"path": CHAT, "status": 200, "body": completion("Total due: $14.21")}], params=[SLOW], clock="wall")

S.case(140, 5, "a model that answers 500 fails the image decoding with the status code.",
       [send(INPUT, reading("image", "media/receipt.png")),
        expect(FAILED, failure("image_decoding", "Reading image decoder failed: status."))],
       backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}], params=[SLOW], clock="wall")
S.case(141, 5, "a reply cut short by its token limit fails as truncated; the reading then reads the next input.",
       [send(INPUT, reading("image", "media/receipt.png")),
        expect(FAILED, failure("image_decoding", "Reading image decoder failed: truncated.")),
        send(INPUT, reading("text", "after")), expect(OUTPUT, output("after", "text"))],
       backend=[{"path": CHAT, "status": 200, "body": {"object": "chat.completion", "choices": [
           {"index": 0, "finish_reason": "length", "message": {"role": "assistant", "content": "Total"}}]}}],
       params=[SLOW], clock="wall")

S.case(150, 6, "a reference to no file in the media directory cannot start the model call.",
       [send(INPUT, reading("image", "media/missing.png")),
        expect(FAILED, failure("image_decoding", "the image decoding could not run (error.execution)"))],
       backend=[], params=[SLOW], clock="wall")

S.case(160, 7, "the image decoding times out after the given stage timeout.",
       [send(INPUT, reading("image", "media/receipt.png")),
        expect(FAILED, failure("image_decoding", "the image decoding timed out after 50 ms"))],
       params=[("stage_timeout_ms", "50")],
       backend=[{"path": CHAT, "status": 200, "body": completion("too late")}])

S.case(170, 8, "an input that arrives while an image is being read waits its turn.",
       [send(INPUT, reading("image", "media/receipt.png")), send(INPUT, reading("text", "Read this note.")),
        expect(OUTPUT, output("Total due: $14.21", "image")), expect(OUTPUT, output("Read this note.", "text"))],
       backend=[{"path": CHAT, "status": 200, "body": completion("Total due: $14.21")}], params=[SLOW], clock="wall")
S.case(171, 8, "three inputs sent at once are answered in order.",
       [send(INPUT, reading("text", "one")), send(INPUT, reading("text", " ")), send(INPUT, reading("text", "three")),
        expect(OUTPUT, output("one", "text")), expect(OUTPUT, output("", "unreadable")),
        expect(OUTPUT, output("three", "text"))])
