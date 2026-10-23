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
    # The detections call's arguments: the fake backend sends a scripted
    # reply as the call of the function the request requires.
    import json
    return json.dumps({"objects": items})


def calls(*arguments):
    # A completion of several detections calls, each with its arguments.
    import json
    return [{"path": CHAT, "status": 200, "body": {"object": "chat.completion", "choices": [{
        "index": 0, "finish_reason": "tool_calls", "message": {"role": "assistant", "content": None, "tool_calls": [
            {"id": f"call-{n}", "type": "function", "function": {"name": "detections", "arguments": json.dumps(value)}}
            for n, value in enumerate(arguments)]}}]}}]


def objects(found):
    return f"JSON.stringify(_event.data) === {js(json_text({'objects': found}))}"


def message(value):
    return f"JSON.stringify(_event.data) === {js(json_text({'message': value}))}"


CUP = item("cup", 0.87, box(0.4, 0.5, 0.1, 0.15))
PERSON = item("person", 0.5, box(0, 0, 0.5, 1))
LAMP = item("lamp", None, box(0.75, 0.25, 0.25, 0.5))

S.statement(1, "Its input is its `<param>`s: it sends itself `bot.ability.vision.detection.input` `{image, media_type}` (the image by reference), which its schema checks, so an `image` that is missing or empty sends nothing to the model and answers `bot.ability.failed` `{message: \"ObjectDetection could not run (error.execution)\"}`; nothing reaches the parent before its one answer.")
S.statement(2, "It asks the generate service once, with one user message (its instructions, which hold no JSON, as a text part and the image's reference as an image part, the image read from the granted media directory) and the detections tool (decision D201), and answers its parent `bot.ability.vision.detection.output` `{objects}`, each object `{label, confidence, box: {x, y, width, height}}` as the call gave it; a call with no objects is `{objects: []}`; then it ends (`done.invoke.<id>` follows the answer).")
S.statement(3, "The objects of every detections call of the reply are read, in order, as one list.")
S.statement(4, "The detections tool's schema admits an object only as `label` a string of 1..100 characters, `confidence` 0..1 or `null` (required), `box` a box (`x`, `y` in 0..1, `width`, `height` in (0, 1]), its numbers JSON numbers (a boolean or a numeric string is refused), and no other keys; an object whose box passes the image's edge (`x + width` or `y + height` over 1 + 1e-6) is skipped, never failing the detection.")
S.statement(5, "The first `max_objects` (32 by default, a `<param>`) objects, in reply order, are kept, then ordered by descending confidence, unrated last, equally rated objects keeping the reply's order.")
S.statement(6, "Arguments the detections tool's schema refuses (an object it does not admit, or no `objects` array) fail the detection: `bot.ability.failed` `{message: \"Object detection generator failed: schema-mismatch.\"}` (never the arguments themselves), and arguments that are not JSON `{message: \"Object detection generator failed: not-json.\"}`.")
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
S.case(110, 2, "the call's objects come back, and the detection then ends.",
       [expect(OUTPUT, objects([CUP, PERSON])), expect(DONE)],
       params=SCENE, backend=replying(reply([CUP, PERSON])))
S.case(111, 2, "an absolute reference to the granted media directory, with no media type, is read too.",
       [expect(OUTPUT, objects([CUP]))],
       params=[("image", "'/media/scene.png'")], backend=replying(reply([CUP])))
S.case(112, 2, "a call with no objects is no objects, not a failure.",
       [expect(OUTPUT, objects([])), expect(DONE)],
       params=SCENE, backend=replying(reply([])))

# 3: every call.
S.case(120, 3, "the objects of several calls are read in order, as one list.",
       [expect(OUTPUT, objects([CUP, PERSON, LAMP]))],
       params=SCENE, backend=calls({"objects": [PERSON]}, {"objects": []}, {"objects": [CUP, LAMP]}))

# 4: the schema's objects, and the image's edge.
S.case(130, 4, "an object whose box passes the image's edge is skipped, the others kept.",
       [expect(OUTPUT, objects([CUP, LAMP]))],
       params=SCENE, backend=replying(reply([
           CUP,
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
S.case(132, 4, "boxes reaching the image's right and bottom edges exactly are inside it.",
       [expect(OUTPUT, objects([item("a", 1, box(0, 0, 1, 1)), item("f", 0, box(0.5, 0.5, 0.5, 0.5))]))],
       params=SCENE, backend=replying(reply([item("a", 1, box(0, 0, 1, 1)), item("f", 0, box(0.5, 0.5, 0.5, 0.5))])))

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
S.case(141, 5, "the first max_objects objects are kept before they are ordered.",
       [expect(OUTPUT, objects([CUP, LAMP]))],
       params=SCENE + [("max_objects", "2")],
       backend=replying(reply([item("past", 1, box(0.75, 0, 0.5, 0.5)), LAMP, CUP, PERSON])))
S.case(142, 5, "32 objects at most by default.",
       [expect(OUTPUT, objects([item(f"o{n}", None, box(0, 0, 1, 1)) for n in range(32)]))],
       params=SCENE, backend=replying(reply([item(f"o{n}", None, box(0, 0, 1, 1)) for n in range(40)])))

# 6: arguments the schema refuses.
MISMATCH = message("Object detection generator failed: schema-mismatch.")
S.case(150, 6, "an object the schema does not admit fails the detection as schema-mismatch.",
       [expect(FAILED, MISMATCH), expect(DONE)],
       params=SCENE, backend=replying(reply([CUP, {"label": "cup", "box": box(0, 0, 1, 1)}])))
for test_id, bad in [
    (151, item("", 0.5, box(0, 0, 1, 1))),
    (152, item("x" * 101, 0.5, box(0, 0, 1, 1))),
    (153, item("cup", 1.5, box(0, 0, 1, 1))),
    (154, item("cup", True, box(0, 0, 0.5, 0.5))),
    (155, item("cup", "0.5", box(0, 0, 0.5, 0.5))),
    (156, item("cup", 0.5, box(0, 0, 0, 0.5))),
    (157, item("cup", 0.5, box(-0.1, 0, 0.5, 0.5))),
    (158, item("cup", 0.5, {"x": 0, "y": 0, "width": 1, "height": 1, "depth": 1})),
    (159, {"label": "cup", "confidence": 0.5, "box": box(0, 0, 1, 1), "color": "red"}),
]:
    S.case(test_id, 6, f"the schema refuses {json_text(bad)}.",
           [expect(FAILED, MISMATCH)], params=SCENE, backend=replying(reply([bad])))
S.case(165, 6, "arguments with no objects array are refused.",
       [expect(FAILED, MISMATCH)], params=SCENE, backend=replying(json_text({"found": [CUP]})))
S.case(166, 6, "arguments that are not JSON fail as not-json.",
       [expect(FAILED, message("Object detection generator failed: not-json."))],
       params=SCENE, backend=replying("I see a cup on a table."))

# 7: the model call fails or cannot start.
S.case(170, 7, "a model that answers 500 fails the detection with the status code, and the detection ends.",
       [expect(FAILED, message("Object detection generator failed: status.")), expect(DONE)],
       params=SCENE, backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(171, 7, "a reply cut short by its token limit fails as truncated.",
       [expect(FAILED, message("Object detection generator failed: truncated."))],
       params=SCENE, backend=[{"path": CHAT, "status": 200, "body": {"object": "chat.completion", "choices": [
           {"index": 0, "finish_reason": "length", "message": {"role": "assistant", "content": "{\"objects\""}}]}}])
S.case(172, 7, "a reference to no file in the media directory cannot start the model call.",
       [expect(FAILED, message("ObjectDetection could not run (error.execution)"))],
       params=[("image", "'media/missing.png'")], backend=[])

# 8: the output schema.
S.case(180, 8, "more than 32 objects, when max_objects allows them, is an output its schema refuses.",
       [expect(FAILED, message("Object detection produced output that does not match its output schema.")),
        expect(DONE)],
       params=SCENE + [("max_objects", "40")],
       backend=replying(reply([item(f"o{n}", None, box(0, 0, 1, 1)) for n in range(33)])))
