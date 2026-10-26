# The seeing's statements and cases (tests/irp/harness.py writes them).
#
# Most cases run on the logical clock (no backend.json, or clock="logical"):
# every timer fires as soon as nothing else is pending, so each view's
# age_ms is exact. The vision service answers within its <invoke>, and the
# logical clock never moves past a model call in flight, so a case that
# times a stage out runs on the wall clock with a 1 ms bound, which the
# stage child's model call (a component started and a request answered)
# always outlasts.

import json

S = suite
# Images: a.png, b.png and c.png are white PNGs of different sizes (other
# bytes, the same gray); gray240, gray225 and black are 8x8 grays. Every
# image is at least 8 pixels a side, the least the vision service's reduced
# decode reads (OpenCV's PNG decode at an eighth).
S.media({
    "a.png": png(8, 8),
    "b.png": png(16, 16),
    "c.png": png(24, 24),
    "gray240.png": png(8, 8, shade=240),
    "gray225.png": png(8, 8, shade=225),
    "black.png": png(8, 8, shade=0),
    "text.png": b"not an image\n",
})
VISUAL = "environment.visual"
LOOK = "bot.ability.seeing.look"
INPUT = "bot.ability.cognition.input"
SIGHT = "bot.ability.seeing.sight"
CHAT = "/chat/completions"
FRAME_DIFF = ("measure", "'frame_diff'")
P1 = {"ref": "person-1", "role": "source"}
P2 = {"ref": "person-2", "role": "source"}

READY = expect("bot.ability.ready")
# The offer: look alone, with its effort (1), its schema, read from its live
# session (decisions D242, D250).
OFFERED = offers("offers.length === 1 && offers[0].event === 'bot.ability.seeing.look' && offers[0].effort === 1"
                 " && offers[0].target === undefined && offers[0].schema.additionalProperties === false")


def canonical(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=False)


def visual(image, source=None, **fields):
    data = {"image": f"media/{image}", "media_type": "image/png"}
    if source is not None:
        data["source"] = source
    data.update(fields)
    return js(data)


def look():
    return send(LOOK, "({})")


def view(image, change=None, age=0, kind=None, participants=(), objects=None, description=None,
         media_type="image/png"):
    return {"image": f"media/{image}", "media_type": media_type, "kind": kind, "participants": list(participants),
            "change": change, "age_ms": age, "objects": objects, "description": description}


def sight(cause, source, views, participants=()):
    """The body's handoff of one sight, exactly."""
    data = {"cause": cause, "views": views, "participants": list(participants)}
    handoff = {"stimulus": {"event": SIGHT, "data": data, "source": source}, "participants": list(participants)}
    return expect(INPUT, f"JSON.stringify(_event.data) === {js(canonical(handoff))}")


def changed(image, low, high, age):
    """The handoff of one changed view whose change is in (low, high)."""
    return expect(INPUT, "_event.data.stimulus.data.cause === 'change'"
                         f" && _event.data.stimulus.data.views[0].image === {js('media/' + image)}"
                         f" && _event.data.stimulus.data.views[0].change > {low}"
                         f" && _event.data.stimulus.data.views[0].change < {high}"
                         f" && _event.data.stimulus.data.views[0].age_ms === {age}")


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def replying(*contents):
    return [{"path": CHAT, "status": 200, "body": completion(content)} for content in contents]


def detections(*found):
    # The object detection child's detections call arguments (the fake
    # backend sends a scripted reply as the call the request requires).
    return json.dumps({"objects": list(found)})


def box(x, y, width, height):
    return {"x": x, "y": y, "width": width, "height": height}


CUP = {"label": "cup", "confidence": 0.5, "box": box(0.25, 0.5, 0.25, 0.25)}
PERSON = {"label": "person", "confidence": 0.75, "box": box(0, 0, 0.5, 1)}
LAMP = {"label": "lamp", "confidence": None, "box": box(0.75, 0.25, 0.25, 0.5)}
DESCRIBED = "A white square."

S.statement(1, "On start it sends its parent `bot.ability.ready`, once and with no data (decision D242). What it offers, read from its live session (`<decision:offers>`), is the one model-callable `bot.ability.seeing.look` with its effort (`look_effort`, 1; decision D250), with its schema (`{}` with no other property), no target.")
S.statement(2, "An `environment.visual` with a `source` is held as that source's view and, as the source's first view, measured and handed on: the parent gets `bot.ability.cognition.input` `{stimulus: {event: \"bot.ability.seeing.sight\", data: {cause: \"change\", views: [view], participants}, source: <the view's source>}, participants}`, the view `{image, media_type, kind, participants, change: null, age_ms, objects: null, description: null}` copied from the stimulus (`media_type` and `kind` null when it names none) and `participants` the view's stamped participants.")
S.statement(3, "An `environment.visual` without a `source` is dropped: nothing is held, measured or handed on, and a look then sees nothing (`{cause: \"look\", views: [], participants: []}`, source `\"seeing\"`).")
S.statement(4, "A later view of a source is measured against the signature of the last view of that source handed on, by `measure` (`byte_identity` by default: 0 for the same bytes, 1 for others; or `frame_diff`, the mean difference of the views' blurred 32x32 grays): a change of at least `change_threshold` (0.1) is handed on with its change; a smaller change is not.")
S.statement(5, "A view whose change is below the threshold keeps the signature handed on before, so drift accumulates against it: views that each differ from the one before by less than the threshold are handed on once they differ enough from the last view handed on.")
S.statement(6, "`bot.ability.seeing.look` `{}` hands on every fresh view, newest first (views that arrived together in the order they arrived), each with `change: null` and its `age_ms`, as `{cause: \"look\", views, participants}` from source `\"seeing\"`; `participants` are the views' stamped participants, first occurrence first.")
S.statement(7, "A look is served at the next opportunity, ahead of any measurement, and every sight, change or look, is followed by a full `settle_after_ms` (5000): a look that arrives while the eye measures, interprets or settles waits for that, and looks waiting for the same opportunity are one look.")
S.statement(8, "At most one measurement runs per `measure_every_ms` (500): after a measurement that hands nothing on the eye paces, and a frame that arrives meanwhile is measured when the pace ends; a look while pacing is served at once.")
S.statement(9, "Sources are measured round robin: of the sources with an unmeasured fresh view, the least recently measured goes first, a source never measured before all others, ties to the oldest arrival.")
S.statement(10, "Each source's newest view replaces its older one (latest wins); at most four views are held, the least recently received evicted past four; a view older than `max_view_age_ms` (60000) is not fresh (a look does not see it, nothing measures it) and is forgotten with its signature at the next hold, so that source's next view is a first view again.")
S.statement(11, "A view the vision service cannot measure (an unreadable reference, or for `frame_diff` a file OpenCV cannot decode, no PNG, JPEG or WebP it reads) is not handed on and no other measure is asked; it counts as measured, the eye paces, and a look still sees the view.")
S.statement(12, "A measurement is bounded by `measure_timeout_ms` (2000): when it passes first nothing is handed on and the eye paces. The vision service answers within its `<invoke>`, so no positive bound passes first here: `tests/tests/seeing.rs` drives the timeout with a vision stand-in that never answers.")
S.statement(13, "With `detect` on, each view handed on is given to the object detection child (abilities/object_detection, with the view's image and media type and the model), and its `bot.ability.vision.detection.output` fills the view's `objects`; with `describe` on the description child's `bot.ability.vision.description.output` fills its `description`. A view's objects stage runs before its description stage, and a sight's views are interpreted one after another, in order.")
S.statement(14, "A stage that fails (its child answers `bot.ability.failed`) or does not answer within `stage_timeout_ms` (30000) leaves only its own field null; the sight is still handed on.")
S.statement(15, "A stimulus its schema refuses is never routed here: an `environment.visual` without an image, or a look with data, raises `error.execution` in the sender.")
S.statement(16, "Its parameters are checked before it perceives: a `change_threshold` outside (0, 1], a `measure_every_ms`, `measure_timeout_ms`, `settle_after_ms`, `max_view_age_ms` or `stage_timeout_ms` that is not positive, a `measure_every_ms` longer than `settle_after_ms`, or a `measure` other than `byte_identity` and `frame_diff` ends it in its final state `degraded`, logging `seeing.degraded` with the first refusal's reason, in that order: it is never ready, and its invoker gets `done.invoke.seeing`.")

S.case(100, 1, "the offer is look alone.", [READY, OFFERED])
S.case(101, 1, "it is ready once: the first sight is the next thing the parent gets.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")])])

S.case(110, 2, "a first view is handed on with the stimulus's fields and participants.",
       [READY, send(VISUAL, visual("a.png", "camera-1", kind="camera", participants=[P1])),
        sight("change", "camera-1", [view("a.png", kind="camera", participants=[P1])], [P1])])
S.case(111, 2, "a view without media type, kind or participants hands them on as null and empty.",
       [READY, send(VISUAL, js({"image": "media/a.png", "source": "camera-1"})),
        sight("change", "camera-1", [view("a.png", media_type=None)])])

S.case(120, 3, "a view without a source is dropped; a look sees nothing.",
       [READY, send(VISUAL, visual("a.png")), look(), sight("look", "seeing", [])])
S.case(121, 3, "a view whose source is null is dropped too.",
       [READY, send(VISUAL, js({"image": "media/a.png", "media_type": "image/png", "source": None})), look(), sight("look", "seeing", [])])

S.case(130, 4, "byte identity: the same bytes are not handed on (pacing), other bytes are, with change 1.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), send(VISUAL, visual("a.png", "camera-1")), send(VISUAL, visual("b.png", "camera-1")),
        sight("change", "camera-1", [view("b.png", change=1, age=500)])])
S.case(131, 4, "frame difference: the same gray at another size is not handed on, black after white is.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), send(VISUAL, visual("b.png", "camera-1")), send(VISUAL, visual("black.png", "camera-1")),
        sight("change", "camera-1", [view("black.png", change=1, age=500)])],
       params=[FRAME_DIFF])
S.case(132, 4, "a change exactly at the threshold is handed on.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), send(VISUAL, visual("b.png", "camera-1")),
        sight("change", "camera-1", [view("b.png", change=1)])],
       params=[("change_threshold", "1")])

S.case(140, 5, "white, then 240 (below 0.1 from white), then 225 (past 0.1 from white, not from 240): 225 is handed on.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), send(VISUAL, visual("gray240.png", "camera-1")), send(VISUAL, visual("gray225.png", "camera-1")),
        changed("gray225.png", 0.11, 0.12, 500)],
       params=[FRAME_DIFF])

S.case(150, 6, "a look sees every fresh view newest first, with the views' participants merged.",
       [READY, send(VISUAL, visual("a.png", "camera-1", participants=[P1])),
        sight("change", "camera-1", [view("a.png", participants=[P1])], [P1]),
        wait("10s"), send(VISUAL, visual("b.png", "camera-2", participants=[P2, P1])),
        sight("change", "camera-2", [view("b.png", participants=[P2, P1])], [P2, P1]),
        wait("10s"), look(),
        sight("look", "seeing", [view("b.png", age=10000, participants=[P2, P1]),
                                 view("a.png", age=20000, participants=[P1])], [P2, P1])])

S.case(160, 7, "a look sent with two views waits for the first one's sight and its settle, goes ahead of the second view's measurement (views that arrived together in arrival order), and is settled after too.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), send(VISUAL, visual("b.png", "camera-2")), look(),
        sight("change", "camera-1", [view("a.png")]),
        sight("look", "seeing", [view("a.png", age=5000), view("b.png", age=5000)]),
        sight("change", "camera-2", [view("b.png", age=10000)])])
S.case(161, 7, "looks waiting for the same opportunity are one look.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), look(), look(), look(),
        sight("change", "camera-1", [view("a.png")]),
        sight("look", "seeing", [view("a.png", age=5000)]),
        send(VISUAL, visual("b.png", "camera-2")),
        sight("change", "camera-2", [view("b.png", age=5000)])])

S.case(170, 8, "a frame that arrives while the eye paces is measured when the pace ends.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), send(VISUAL, visual("a.png", "camera-1")), wait("100ms"),
        send(VISUAL, visual("b.png", "camera-1")), sight("change", "camera-1", [view("b.png", change=1, age=400)])])
S.case(171, 8, "a look while the eye paces is served at once.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), send(VISUAL, visual("a.png", "camera-1")), wait("100ms"), look(),
        sight("look", "seeing", [view("a.png", age=100)])])

S.case(180, 9, "the least recently measured source goes first, though another source's view arrived first.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), send(VISUAL, visual("a.png", "camera-2")),
        sight("change", "camera-1", [view("a.png")]), sight("change", "camera-2", [view("a.png", age=5000)]),
        wait("1s"), send(VISUAL, visual("b.png", "camera-2")), send(VISUAL, visual("b.png", "camera-1")),
        sight("change", "camera-1", [view("b.png", change=1, age=4000)]),
        sight("change", "camera-2", [view("b.png", change=1, age=9000)])])
S.case(181, 9, "a source never measured goes before one measured before.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("1s"), send(VISUAL, visual("b.png", "camera-1")), send(VISUAL, visual("a.png", "camera-3")),
        sight("change", "camera-3", [view("a.png", age=4000)]),
        sight("change", "camera-1", [view("b.png", change=1, age=9000)])])

S.case(190, 10, "a source's newest view replaces the one before it: only the last is measured and seen.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("1s"), send(VISUAL, visual("b.png", "camera-1")), send(VISUAL, visual("c.png", "camera-1")),
        sight("change", "camera-1", [view("c.png", change=1, age=4000)]), look(),
        sight("look", "seeing", [view("c.png", age=9000)])])
S.case(191, 10, "past four views the least recently received is evicted: a look sees the four newest.",
       [READY] + [send(VISUAL, visual("a.png", f"camera-{n}")) for n in range(1, 6)] + [look(),
        sight("change", "camera-1", [view("a.png")]),
        sight("look", "seeing", [view("a.png", age=5000) for _ in range(4)])])
S.case(192, 10, "a view older than max_view_age_ms is not seen, and its source's next view is a first view again.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")]),
        wait("6s"), look(), sight("look", "seeing", []),
        wait("6s"), send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")])],
       params=[("max_view_age_ms", "1000")])

S.case(200, 11, "an unreadable reference is not handed on: the eye paces (another source's frame waits for the pace), and a look still sees it.",
       [READY, send(VISUAL, visual("missing.png", "camera-1")), wait("100ms"), send(VISUAL, visual("a.png", "camera-2")),
        sight("change", "camera-2", [view("a.png", age=400)]), look(),
        sight("look", "seeing", [view("a.png", age=5400), view("missing.png", age=5500)])])
S.case(201, 11, "frame difference: a file that is no image is not handed on; a look still sees it.",
       [READY, send(VISUAL, visual("text.png", "camera-1")), wait("1s"), look(),
        sight("look", "seeing", [view("text.png", age=1000)])],
       params=[FRAME_DIFF])

S.case(210, 12, "the vision service answers within its <invoke>: even a 1 ms bound does not pass first, so the first view is handed on before the look sees it.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), look(),
        sight("change", "camera-1", [view("a.png")]),
        sight("look", "seeing", [view("a.png", age=5000)])],
       params=[("measure_timeout_ms", "1")], logs={"seeing.measure": ["first"]})

S.case(220, 13, "with detect on, the view's objects are the detection child's.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png", objects=[CUP])])],
       params=[("detect", "true")], backend=replying(detections(CUP)), clock="logical")
S.case(221, 13, "with describe on, the view's description is the description child's.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png", description=DESCRIBED)])],
       params=[("describe", "true")], backend=replying(DESCRIBED), clock="logical")
S.case(222, 13, "with both on, the objects stage is asked first, then the description stage.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png", objects=[PERSON, CUP], description=DESCRIBED)])],
       params=[("detect", "true"), ("describe", "true")],
       backend=replying(detections(CUP, PERSON), DESCRIBED), clock="logical",
       logs={"seeing.stage": ["objects ok", "description ok"]})
S.case(223, 13, "a look's views are interpreted one after another, in order.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), send(VISUAL, visual("b.png", "camera-2")), look(),
        sight("change", "camera-1", [view("a.png", objects=[CUP])]),
        sight("look", "seeing", [view("a.png", age=5000, objects=[PERSON]), view("b.png", age=5000, objects=[LAMP])]),
        sight("change", "camera-2", [view("b.png", age=10000, objects=[])])],
       params=[("detect", "true")],
       backend=replying(detections(CUP), detections(PERSON), detections(LAMP), detections()), clock="logical")

S.case(230, 14, "a failed objects stage leaves objects null; the description is still filled.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png", description=DESCRIBED)])],
       params=[("detect", "true"), ("describe", "true")],
       backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}] + replying(DESCRIBED),
       clock="logical", logs={"seeing.stage": ["objects failed", "description ok"]})
S.case(231, 14, "a failed description leaves description null; the objects are still filled.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png", objects=[CUP])])],
       params=[("detect", "true"), ("describe", "true")],
       backend=replying(detections(CUP), "   "), clock="logical",
       logs={"seeing.stage": ["objects ok", "description failed"]})
S.case(232, 14, "an objects stage that does not answer within stage_timeout_ms leaves objects null.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png")])],
       params=[("detect", "true"), ("stage_timeout_ms", "1")],
       backend=replying(detections(CUP)), clock="wall", logs={"seeing.stage": ["objects timeout"]})
S.case(233, 14, "a description stage that does not answer within stage_timeout_ms leaves description null.",
       [READY, send(VISUAL, visual("a.png", "camera-1")),
        sight("change", "camera-1", [view("a.png")])],
       params=[("describe", "true"), ("stage_timeout_ms", "1")],
       backend=replying(DESCRIBED), clock="wall", logs={"seeing.stage": ["description timeout"]})

S.case(240, 15, "a visual without an image is never routed: the sender gets error.execution.",
       [READY, send(VISUAL, js({"source": "camera-1"})), expect("error.execution"),
        send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")])])
S.case(241, 15, "a look with data is never routed: the sender gets error.execution.",
       [READY, send(LOOK, js({"now": True})), expect("error.execution"), look(), sight("look", "seeing", [])])


def refused(test_id, comment, params, reason):
    """A case whose parameters the eye refuses: it ends at once, never ready."""
    S.case(test_id, 16, comment, [expect("done.invoke.seeing")], params=params, logs={"seeing.degraded": [reason]})


refused(250, "a change_threshold of 0 is refused.", [("change_threshold", "0")], "change_threshold must be in (0, 1]")
refused(251, "a change_threshold over 1 is refused.", [("change_threshold", "1.5")], "change_threshold must be in (0, 1]")
refused(252, "a measure_every_ms of 0 is refused.", [("measure_every_ms", "0")], "measure_every_ms must be positive")
refused(253, "a negative measure_timeout_ms is refused.", [("measure_timeout_ms", "-1")], "measure_timeout_ms must be positive")
refused(254, "a settle_after_ms of 0 is refused.", [("settle_after_ms", "0")], "settle_after_ms must be positive")
refused(255, "a max_view_age_ms of 0 is refused.", [("max_view_age_ms", "0")], "max_view_age_ms must be positive")
refused(256, "a stage_timeout_ms of 0 is refused.", [("stage_timeout_ms", "0")], "stage_timeout_ms must be positive")
refused(257, "a measure_every_ms longer than settle_after_ms is refused.", [("measure_every_ms", "5001")],
        "measure_every_ms cannot be longer than settle_after_ms")
refused(258, "a measure other than byte_identity and frame_diff is refused.", [("measure", "'embedding'")],
        "measure must be byte_identity or frame_diff")
refused(259, "only the first refusal is logged: the threshold's, before the measure's.",
        [("change_threshold", "0"), ("measure", "'embedding'")], "change_threshold must be in (0, 1]")
S.case(260, 16, "a measure_every_ms equal to settle_after_ms is accepted: the eye is ready and sees.",
       [READY, send(VISUAL, visual("a.png", "camera-1")), sight("change", "camera-1", [view("a.png")])],
       params=[("measure_every_ms", "5000")])
