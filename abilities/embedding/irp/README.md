# `embedding` implementation report plan

The implementation report plan of [`embedding.scx`](../embedding.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="embedding-1"></a>**embedding-1**. `bot.ability.embedding.input` `{role, items}` is embedded through the generate service's embeddings mode (`POST {base}/embeddings`, with its `model` and `dimensions`, `<param>`s defaulting to `embeddinggemma-2` and 256) and answered `bot.ability.embedding.output` `{space: {model, dimensions}, role, vectors: [{values}]}`: the role echoed, one unit vector per item, in the items' order whatever the order of the reply's rows.
- <a id="embedding-2"></a>**embedding-2**. Requests that arrive while one is in flight are held and served in order, each answered once.
- <a id="embedding-3"></a>**embedding-3**. A request the embedder cannot serve is answered `bot.ability.embedding.failed` `{kind, message}`, and the next request is served: an error status is `embed_failed` (`The embedder failed (status).`), a reply that is not one vector per item `invalid_response` (`The embedder failed (malformed).`), and no answer within `timeout_ms` (10 s, a `<param>`) `timeout` (`The embedder did not answer within <s>s.`).
- <a id="embedding-4"></a>**embedding-4**. An input its schema refuses (no items, a part without text) never arrives: the send raises `error.execution` in the sender, and the next request is served.
