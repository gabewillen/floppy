# The description's statements and cases (tests/irp/harness.py writes them).

S = suite
# scene.png: a 1x1 white PNG (the fake backend reads it).
S.media({"scene.png": png()})
OUTPUT = "bot.ability.vision.description.output"
FAILED = "bot.ability.failed"
DONE = "done.invoke.description"
CHAT = "/chat/completions"
SCENE = [("image", "'media/scene.png'"), ("media_type", "'image/png'")]
SAID = "A white square fills the frame. Nothing else is in view."


def json_text(value):
    import json
    return json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=False)


def completion(content):
    return {"object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}]}


def replying(content):
    return [{"path": CHAT, "status": 200, "body": completion(content)}]


def described(text):
    return f"JSON.stringify(_event.data) === {js(json_text({'description': text}))}"


def message(value):
    return f"JSON.stringify(_event.data) === {js(json_text({'message': value}))}"


S.statement(1, "Its input is its `<param>`s: it sends itself `bot.ability.vision.description.input` `{image, media_type}` (the original's media.Image, by reference), which its schema checks, so an `image` that is missing or empty sends nothing to the model and answers `bot.ability.failed` `{message: \"Description could not run (error.execution)\"}`; nothing reaches the parent before its one answer.")
S.statement(2, "It asks the generate service once, with one user message (the original's DESCRIBE_INSTRUCTIONS as a text part and the image's reference as an image part, the image read from the granted media directory), and answers its parent `bot.ability.vision.description.output` `{description}`, the reply trimmed of white space; then it ends (`done.invoke.<id>` follows the answer).")
S.statement(3, "A reply that is empty once stripped answers `bot.ability.failed` `{message: \"the generator answered with an empty description\"}`.")
S.statement(4, "A description of at most 4000 characters (the original's MAX_DESCRIPTION_CHARS, counted in characters, not bytes) is answered; a longer one is an output its schema refuses and answers `bot.ability.failed` `{message: \"Description produced output that does not match its output schema.\"}`, the original's invalid-output failure.")
S.statement(5, "A model call that fails answers `bot.ability.failed` `{message: \"Description generator failed: <code>.\"}`, the generate service's failure code in the message, and an image reference the generate service cannot read rejects its start: `{message: \"Description could not run (error.execution)\"}`.")

# 1: the input.
S.case(100, 1, "with no image nothing is asked of the model: the input's schema refuses it.",
       [expect(FAILED, message("Description could not run (error.execution)"))])
S.case(101, 1, "an empty image is refused by the input's schema.",
       [expect(FAILED, message("Description could not run (error.execution)"))],
       params=[("image", "''"), ("media_type", "'image/png'")])
S.case(102, 1, "a media type that is not a string is refused by the input's schema.",
       [expect(FAILED, message("Description could not run (error.execution)"))],
       params=[("image", "'media/scene.png'"), ("media_type", "7")])

# 2: the description.
S.case(110, 2, "the reply, stripped, is the description, and the description then ends.",
       [expect(OUTPUT, described(SAID)), expect(DONE)],
       params=SCENE, backend=replying("\n  " + SAID + "  \n"))
S.case(111, 2, "an absolute reference to the granted media directory, with no media type, is read too.",
       [expect(OUTPUT, described(SAID))],
       params=[("image", "'/media/scene.png'")], backend=replying(SAID))
S.case(112, 2, "Unicode white space and a byte order mark around the reply are trimmed.",
       [expect(OUTPUT, described(SAID))],
       params=SCENE, backend=replying("\t\ufeff" + SAID + "\r\u3000"))

# 3: an empty reply.
S.case(120, 3, "a reply of only line breaks is no description.",
       [expect(FAILED, message("the generator answered with an empty description")), expect(DONE)],
       params=SCENE, backend=replying("\n\n"))
S.case(121, 3, "a reply of only white space is no description.",
       [expect(FAILED, message("the generator answered with an empty description"))],
       params=SCENE, backend=replying(" \n\t\u3000\r\u2003 "))

# 4: the output schema's bound.
S.case(130, 4, "4000 characters, some of them two UTF-8 bytes, are a description.",
       [expect(OUTPUT, described("é" * 2000 + "e" * 2000))],
       params=SCENE, backend=replying("é" * 2000 + "e" * 2000))
S.case(131, 4, "4001 characters are an output its schema refuses.",
       [expect(FAILED, message("Description produced output that does not match its output schema.")), expect(DONE)],
       params=SCENE, backend=replying("e" * 4001))

# 5: the model call fails or cannot start.
S.case(140, 5, "a model that answers 500 fails the description with the status code.",
       [expect(FAILED, message("Description generator failed: status.")), expect(DONE)],
       params=SCENE, backend=[{"path": CHAT, "status": 500, "body": {"error": {"message": "boom"}}}])
S.case(141, 5, "a reply cut short by its token limit fails as truncated.",
       [expect(FAILED, message("Description generator failed: truncated."))],
       params=SCENE, backend=[{"path": CHAT, "status": 200, "body": {"object": "chat.completion", "choices": [
           {"index": 0, "finish_reason": "length", "message": {"role": "assistant", "content": "A white"}}]}}])
S.case(142, 5, "a reference to no file in the media directory cannot start the model call.",
       [expect(FAILED, message("Description could not run (error.execution)"))],
       params=[("image", "'media/missing.png'")], backend=[])
