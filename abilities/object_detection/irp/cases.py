# The object_detection's statements and cases (tests/irp/harness.py writes them).

S = suite
# scene.png: a 1x1 white PNG (the fake backend reads it).
S.media({"scene.png": png()})
OUTPUT = "bot.ability.vision.detection.output"
FAILED = "bot.ability.failed"
DONE = "done.invoke.object_detection"
CHAT = "/chat/completions"
SCENE = [("image", "'media/scene.png'"), ("media_type", "'image/png'")]


def json_text(value):
    import json
    return json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=False)


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def replying(content):
    return [{"path": CHAT, "status": 200, "body": completion(content)}]


def box(x, y, width, height):
    return {"x": x, "y": y, "width": width, "height": height}


def item(label, confidence, where):
    return {"label": label, "confidence": confidence, "box": where}


def reply(items):
    import json
    return json.dumps(items)


def objects(found):
    return f"JSON.stringify(_event.data) === {js(json_text({'objects': found}))}"


def message(value):
    return f"JSON.stringify(_event.data) === {js(json_text({'message': value}))}"


CUP = item("cup", 0.87, box(0.4, 0.5, 0.1, 0.15))
PERSON = item("person", 0.5, box(0, 0, 0.5, 1))
LAMP = item("lamp", None, box(0.75, 0.25, 0.25, 0.5))

S.statement(1, "Its input is its `<param>`s: it sends itself `bot.ability.vision.detection.input` `{image, media_type}` (the image by reference), which its schema checks, so an `image` that is missing or empty sends nothing to the model and answers `bot.ability.failed` `{message: \"ObjectDetection could not run (error.execution)\"}`; nothing reaches the parent before its one answer.")
S.statement(2, "It asks the generate service once, with one user message (its instructions as a text part and the image's reference as an image part, the image read from the granted media directory), and answers its parent `bot.ability.vision.detection.output` `{objects}`, each object `{label, confidence, box: {x, y, width, height}}` as the reply gave it; a reply of `[]` is `{objects: []}`; then it ends (`done.invoke.<id>` follows the answer).")
S.statement(3, "The reply is trimmed, and a reply fenced as markdown (starting with three backticks) is read inside the fence: the text after its first line, a closing fence removed, trimmed again.")
S.statement(4, "Each reply item is validated on its own as a detected object, its numbers JSON numbers (a boolean or a numeric string is refused): `label` a string of 1..100 characters, `confidence` 0..1 or `null` (required), `box` a box (`x`, `y` in 0..1, `width`, `height` in (0, 1], `x + width` and `y + height` at most 1 + 1e-6), and no other keys; an item that is not one is skipped, never failing the detection.")
S.statement(5, "The first `max_objects` (32 by default, a `<param>`) valid items, in reply order, are kept, then ordered by descending confidence, unrated last, equally rated objects keeping the reply's order.")
S.statement(6, "A reply over `max_reply_bytes` (65536 by default, a `<param>`; UTF-8 bytes) answers `bot.ability.failed` `{message: \"the reply of <n> bytes exceeds max_reply_bytes (<max>)\"}`, and a reply that is not a JSON array answers `{message: \"the reply is not a JSON array of objects\"}` (never the reply itself).")
S.statement(7, "A model call that fails answers `bot.ability.failed` `{message: \"Object detection generator failed: <code>.\"}`, the generate service's failure code in the message, and an image reference the generate service cannot read rejects its start: `{message: \"ObjectDetection could not run (error.execution)\"}`.")
S.statement(8, "An output its schema refuses (more than 32 objects, when the invoker allows more) answers `bot.ability.failed` `{message: \"Object detection produced output that does not match its output schema.\"}`.")

# 1: the input.
S.case(100, 1, "with no image nothing is asked of the model: the input's schema refuses it.",
       [expect(FAILED, message("ObjectDetection could not run (error.execution)"))])
S.case(101, 1, "an empty image is refused by the input's schema.",
       [expect(FAILED, message("ObjectDetection could not run (error.execution)"))],
       params=[("image", "''"), ("media_type", "'image/png'")])
S.case(102, 1, "an empty media type is refused by the input's schema.",
       [expect(FAILED, message("ObjectDetection could not run (error.execution)"))],
       params=[("image", "'media/scene.png'"), ("media_type", "''")])

# 2: the model's objects.
S.case(110, 2, "the reply's objects come back, and the detection then ends.",
       [expect(OUTPUT, objects([CUP, PERSON])), expect(DONE)],
       params=SCENE, backend=replying(reply([CUP, PERSON])))
S.case(111, 2, "an absolute reference to the granted media directory, with no media type, is read too.",
       [expect(OUTPUT, objects([CUP]))],
       params=[("image", "'/media/scene.png'")], backend=replying(reply([CUP])))
S.case(112, 2, "a reply of [] is no objects, not a failure.",
       [expect(OUTPUT, objects([])), expect(DONE)],
       params=SCENE, backend=replying("[]"))

# 3: trimming and fences.
S.case(120, 3, "white space around the reply is trimmed.",
       [expect(OUTPUT, objects([CUP]))],
       params=SCENE, backend=replying("\t\n  " + reply([CUP]) + " \n\r"))
S.case(121, 3, "a fenced reply is read inside its fence.",
       [expect(OUTPUT, objects([CUP]))],
       params=SCENE, backend=replying("```json\n" + reply([CUP]) + "\n```"))
S.case(122, 3, "a fence with no closing fence is read to the end.",
       [expect(OUTPUT, objects([CUP]))],
       params=SCENE, backend=replying("  ```\n" + reply([CUP]) + "\n"))
S.case(123, 3, "a fence with nothing after its first line is no array.",
       [expect(FAILED, message("the reply is not a JSON array of objects"))],
       params=SCENE, backend=replying("```" + reply([CUP]) + "```"))

# 4: each item on its own.
S.case(130, 4, "items that are not a detected object are skipped, the valid ones kept.",
       [expect(OUTPUT, objects([CUP, LAMP]))],
       params=SCENE, backend=replying(reply([
           "cup", 3, None, [CUP],
           CUP,
           {"label": "cup", "box": box(0, 0, 1, 1)},
           {"label": "cup", "confidence": 0.5, "box": box(0, 0, 1, 1), "color": "red"},
           item("", 0.5, box(0, 0, 1, 1)),
           item("x" * 101, 0.5, box(0, 0, 1, 1)),
           item(7, 0.5, box(0, 0, 1, 1)),
           item("cup", 1.5, box(0, 0, 1, 1)),
           item("cup", -0.1, box(0, 0, 1, 1)),
           item("cup", "high", box(0, 0, 1, 1)),
           item("cup", 0.5, [0, 0, 1, 1]),
           item("cup", 0.5, {"x": 0, "y": 0, "width": 1}),
           item("cup", 0.5, {"x": 0, "y": 0, "width": 1, "height": 1, "depth": 1}),
           item("cup", 0.5, box(-0.1, 0, 0.5, 0.5)),
           item("cup", 0.5, box(0, 1.5, 0.5, 0.5)),
           item("cup", 0.5, box(0, 0, 0, 0.5)),
           item("cup", 0.5, box(0, 0, 0.5, 1.5)),
           item("cup", 0.5, box(0.6, 0, 0.5, 0.5)),
           item("cup", 0.5, box(0, 0.5, 0.5, 0.500002)),
           LAMP,
       ])))
S.case(131, 4, "a box past an edge by at most 1e-6, a 100-character label (emoji count once) and a blank label are valid.",
       [expect(OUTPUT, objects([
           item("\U0001F600" * 100, 0.5, box(0.5, 0, 0.5000005, 1)),
           item(" ", 0.5, box(0, 0.5, 0.5, 0.5000005)),
       ]))],
       params=SCENE, backend=replying(reply([
           item("\U0001F600" * 100, 0.5, box(0.5, 0, 0.5000005, 1)),
           item(" ", 0.5, box(0, 0.5, 0.5, 0.5000005)),
       ])))
S.case(132, 4, "numbers are JSON numbers: a boolean or a numeric string is refused, as confidence or in a box.",
       [expect(OUTPUT, objects([
           item("a", 1, box(0, 0, 1, 1)),
           item("f", 0.5, box(0, 0, 0.5, 0.5)),
       ]))],
       params=SCENE, backend=replying(reply([
           item("a", 1, box(0, 0, 1, 1)),
           item("b", True, box(0, 0, 0.5, 0.5)),
           item("c", "0.5", box(0, 0, 0.5, 0.5)),
           item("d", 0.5, box("0.25", 0, 0.5, 0.5)),
           item("e", 0.5, box(False, 0, 0.5, 0.5)),
           item("f", 0.5, box(0, 0, 0.5, 0.5)),
       ])))
S.case(133, 4, "a reply with no valid item is no objects, not a failure.",
       [expect(OUTPUT, objects([]))],
       params=SCENE, backend=replying(reply([{"name": "cup"}, "cup"])))

# 5: max_objects and the order.
S.case(140, 5, "objects come back most confident first, unrated last, ties in the reply's order.",
       [expect(OUTPUT, objects([
           CUP,
           item("left", 0.5, box(0, 0, 0.5, 0.5)),
           item("right", 0.5, box(0.5, 0, 0.5, 0.5)),
           item("floor", 0, box(0, 0.5, 1, 0.5)),
           LAMP,
           item("door", None, box(0, 0, 0.25, 1)),
       ]))],
       params=SCENE, backend=replying(reply([
           LAMP,
           item("left", 0.5, box(0, 0, 0.5, 0.5)),
           item("floor", 0, box(0, 0.5, 1, 0.5)),
           CUP,
           item("door", None, box(0, 0, 0.25, 1)),
           item("right", 0.5, box(0.5, 0, 0.5, 0.5)),
       ])))
S.case(141, 5, "the first max_objects valid items are kept before they are ordered.",
       [expect(OUTPUT, objects([CUP, LAMP]))],
       params=SCENE + [("max_objects", "2")],
       backend=replying(reply([{"label": "skipped"}, LAMP, CUP, PERSON])))
S.case(142, 5, "32 objects at most by default.",
       [expect(OUTPUT, objects([item(f"o{n}", None, box(0, 0, 1, 1)) for n in range(32)]))],
       params=SCENE, backend=replying(reply([item(f"o{n}", None, box(0, 0, 1, 1)) for n in range(40)])))

# 6: replies it cannot read.
S.case(150, 6, "a reply over max_reply_bytes is refused, counted in UTF-8 bytes.",
       [expect(FAILED, message("the reply of 12 bytes exceeds max_reply_bytes (11)"))],
       params=SCENE + [("max_reply_bytes", "11")], backend=replying("[\"\u00e9\u00e9\u00e9\u00e9\"]"))
S.case(151, 6, "a reply of exactly max_reply_bytes is read.",
       [expect(OUTPUT, objects([]))],
       params=SCENE + [("max_reply_bytes", "10")], backend=replying("[]        "))
S.case(152, 6, "a JSON object is not a JSON array.",
       [expect(FAILED, message("the reply is not a JSON array of objects"))],
       params=SCENE, backend=replying(json_text({"objects": [CUP]})))
S.case(153, 6, "prose is not a JSON array.",
       [expect(FAILED, message("the reply is not a JSON array of objects"))],
       params=SCENE, backend=replying("I see a cup on a table."))
S.case(154, 6, "an empty reply is not a JSON array.",
       [expect(FAILED, message("the reply is not a JSON array of objects"))],
       params=SCENE, backend=replying("  "))

# 7: the model call fails or cannot start.
S.case(160, 7, "a model that answers 500 fails the detection with the status code, and the detection ends.",
       [expect(FAILED, message("Object detection generator failed: status.")), expect(DONE)],
       params=SCENE, backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(161, 7, "a reply cut short by its token limit fails as truncated.",
       [expect(FAILED, message("Object detection generator failed: truncated."))],
       params=SCENE, backend=[{"path": CHAT, "status": 200, "body": {"object": "chat.completion", "choices": [
           {"index": 0, "finish_reason": "length", "message": {"role": "assistant", "content": "[{\"label\""}}]}}])
S.case(162, 7, "a reference to no file in the media directory cannot start the model call.",
       [expect(FAILED, message("ObjectDetection could not run (error.execution)"))],
       params=[("image", "'media/missing.png'")], backend=[])

# 8: the output schema.
S.case(170, 8, "more than 32 objects, when max_objects allows them, is an output its schema refuses.",
       [expect(FAILED, message("Object detection produced output that does not match its output schema.")),
        expect(DONE)],
       params=SCENE + [("max_objects", "40")],
       backend=replying(reply([item(f"o{n}", None, box(0, 0, 1, 1)) for n in range(33)])))
