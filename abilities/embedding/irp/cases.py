# The embedding's statements and cases (tests/irp/harness.py writes them).

S = suite
INPUT = "bot.ability.embedding.input"
OUTPUT = "bot.ability.embedding.output"
FAILED = "bot.ability.embedding.failed"
EMBEDDINGS = "/embeddings"
# A space of two dimensions, so the scripted rows stay short. A loaded
# host can take longer than the 10 s bound to start the generate service:
# the cases that reach it wait longer (case 122 tests the bound itself).
TWO = [("dimensions", "2"), ("timeout_ms", "600000")]


def request(role, *texts, id="embedding-1"):
    """An input of one item per text, each of one text part, with the
    requester's id."""
    return js({"id": id, "role": role, "items": [{"parts": [{"kind": "text", "text": text}]} for text in texts]})


def rows(*vectors, order=None):
    """An Embeddings reply of one row per vector, listed in `order` (the
    indices) or in order."""
    indices = order if order is not None else range(len(vectors))
    return {"path": EMBEDDINGS, "status": 200,
            "body": {"object": "list", "data": [{"object": "embedding", "index": index, "embedding": vectors[index]}
                                                for index in indices]}}


def answered(role, *vectors, model="embeddinggemma-2", dimensions=2, id="embedding-1"):
    values = js(js([list(v) for v in vectors]).replace(" ", ""))
    return expect(OUTPUT, f"_event.data.id === {js(id)} && _event.data.role === {js(role)} && _event.data.space.model === {js(model)}"
                          f" && _event.data.space.dimensions === {dimensions}"
                          " && JSON.stringify(_event.data.vectors.map(function (v) { return v.values; }))"
                          f" === {values}")


def failed(kind, message, id="embedding-1"):
    return expect(FAILED, f"_event.data.id === {js(id)} && _event.data.kind === {js(kind)} && _event.data.message === {js(message)}")


S.statement(1, "`bot.ability.embedding.input` `{id, role, items}` is embedded through the generate service's embeddings mode (`POST {base}/embeddings`, with its `model` and `dimensions`, `<param>`s defaulting to `embeddinggemma-2` and 256) and answered to its sender (`_event.origin`) `bot.ability.embedding.output` `{id, space: {model, dimensions}, role, vectors: [{values}]}`: the id and role echoed, one unit vector per item, in the items' order whatever the order of the reply's rows.")
S.statement(2, "Requests that arrive while one is in flight are held and served in order, each answered once.")
S.statement(3, "A request the embedder cannot serve is answered `bot.ability.embedding.failed` `{id, kind, message}`, and the next request is served: an error status is `embed_failed` (`The embedder failed (status).`), a reply that is not one vector per item `invalid_response` (`The embedder failed (malformed).`), and no answer within `timeout_ms` (10 s, a `<param>`) `timeout` (`The embedder did not answer within <s>s.`).")
S.statement(4, "An input its schema refuses (no items, a part without text, no id) never arrives: the send raises `error.execution` in the sender, and the next request is served.")

S.case(100, 1, "a query of two items is two unit vectors in the items' order, though the reply lists them reversed.",
       [send(INPUT, request("query", "a red circle", "a blue square")),
        answered("query", (0.6, 0.8), (0, 1))],
       params=TWO, backend=[rows([3, 4], [0, 2], order=[1, 0])])
S.case(101, 1, "a document's vector is answered in the space its model and dimensions name.",
       [send(INPUT, request("document", "Gabe lives in Lisbon")), answered("document", (1, 0), model="other-model")],
       params=TWO + [("model", "'other-model'")], backend=[rows([5, 0])])
S.case(102, 1, "by default the space is embeddinggemma-2 at 256 dimensions, a row cut to its first 256.",
       [send(INPUT, request("query", "hello")),
        expect(OUTPUT, "_event.data.id === 'embedding-1' && _event.data.space.model === 'embeddinggemma-2' && _event.data.space.dimensions === 256"
                       " && _event.data.vectors.length === 1 && _event.data.vectors[0].values.length === 256"
                       " && _event.data.vectors[0].values[0] === 1 && _event.data.vectors[0].values[255] === 0")],
       params=[("timeout_ms", "600000")], backend=[rows([2] + [0] * 255 + [7])])

S.case(110, 2, "a request sent while one is in flight is served after it, in order.",
       [send(INPUT, request("query", "first", id="first")), send(INPUT, request("document", "second", id="second")),
        answered("query", (1, 0), id="first"), answered("document", (0, 1), id="second")],
       params=TWO, backend=[rows([1, 0]), rows([0, 1])])

S.case(120, 3, "an error status fails the request as embed_failed; the next is served.",
       [send(INPUT, request("query", "boom")), failed("embed_failed", "The embedder failed (status)."),
        send(INPUT, request("query", "again")), answered("query", (0, 1))],
       params=TWO, backend=[{"path": EMBEDDINGS, "status": 500, "body": {"error": {"message": "boom"}}}, rows([0, 3])])
S.case(121, 3, "a reply with fewer rows than items fails the request as invalid_response.",
       [send(INPUT, request("query", "one", "two")), failed("invalid_response", "The embedder failed (malformed).")],
       params=TWO, backend=[rows([1, 0])])
S.case(122, 3, "an embedder that does not answer within timeout_ms fails the request as timeout.",
       [send(INPUT, request("query", "slow")), failed("timeout", "The embedder did not answer within 0.001s.")],
       params=[("dimensions", "2"), ("timeout_ms", "1")], backend=[rows([1, 0])], clock="wall")

S.case(130, 4, "inputs the schema refuses raise error.execution at the send; the next is served.",
       [send(INPUT, js({"id": "embedding-1", "role": "query", "items": []})), expect("error.execution"),
        send(INPUT, js({"id": "embedding-1", "role": "query", "items": [{"parts": [{"kind": "text"}]}]})), expect("error.execution"),
        send(INPUT, js({"role": "query", "items": [{"parts": [{"kind": "text", "text": "no id"}]}]})), expect("error.execution"),
        send(INPUT, request("query", "fine")), answered("query", (1, 0))],
       params=TWO, backend=[rows([4, 0])])
