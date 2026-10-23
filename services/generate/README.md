# generate

The `generate` invoked service: one model generation per `<invoke>` on the
`OpenAI`-compatible Chat Completions API, as a Wasm component
(`https://github.com/stateforward/floppy/services/generate`). Its crate
documentation ([src/lib.rs](src/lib.rs)) is the full contract: parameters,
events, the request, the reply, bounds, and telemetry.

## Events

`generate.result` is the reply's content and tool calls with what the
request cost (decision D126):

```json
{"content": "…", "tool_calls": [{"id": "call-1", "name": "dispatch", "args": {…}}],
 "request_bytes": 2048,
 "usage": {"input_tokens": 812, "output_tokens": 40, "reasoning_tokens": 24, "total_tokens": 852}}
```

`request_bytes` is the UTF-8 length of the request body without its
streaming members (`stream`, `stream_options`), so a streamed and an
unstreamed call of the same request count the same;
`usage` is what the backend reported (a non-streamed completion's `usage`,
or a stream's last chunk, which a streamed call asks for with
`stream_options.include_usage`), each count present only when reported and
the whole member absent when none is.

`generate.failed` is `{"code": C}`; a call refused as `not-json` or
`schema-mismatch` also carries its rejection,
`"rejection": {"key": K, "message": M}`, so an invoking document can ask its
model again with the diagnostic and stop when the same key repeats:

| Refusal | `key` | `message` (example) |
| --- | --- | --- |
| Arguments that are not a JSON object | `not-json` | `the arguments are not JSON: EOF while parsing a value at line 1 column 0` |
| A call of a function the request did not offer | `schema-mismatch:name` | `the call names a function the request did not offer: expected 'dispatch', got 'speak'` |
| Arguments the parameters do not admit | `schema-mismatch:<P>`, `P` the JSON pointer of the first failure | `/events/0/data: additional properties 'reason' not allowed` |

Neither repeats a value of the arguments, and each is one line of at most
200 characters.

## Tool choice, sampling and thinking

These optional `<param>`s shape the request (decision D128):

| Parameter | Value | Sent as |
| --- | --- | --- |
| `tool_choice` | `required` (the default) or `auto`; needs a `schema` | `tool_choice`. Under `auto` a reply with no call is read from its content as one call (`id` `content`) of the tool, its arguments the content's JSON object, checked as any call's; blank content is a result with no call |
| `temperature` | a number from 0 to 2 | `temperature` |
| `max_completion_tokens` | a whole number from 1 to 1048576 | `max_completion_tokens` |
| `thinking_token_budget` | a whole number from 1 to 1048576 | `thinking_token_budget` (vLLM's bound on a reasoning model's thinking) |

A call's arguments are read leniently (decision D128): arguments sent as
a JSON string of the object are decoded once more, and so is each string
where the tool's schema takes only an array or an object (through `$ref`,
and the first `anyOf` branch that admits the decoded value); such a string
that is not JSON, or holds neither, fails `not-json`. A chain of `$ref`s
with no property or item between them is followed at most 64 deep (a
schema that refers to itself ends there), and on wasm builds arguments
that take more than 65536 schemas to decode (each `$ref`, `anyOf` branch,
property and item counting one) fail `not-json`.

## Images

A `messages` entry's `content` may be an array of reading parts,
`{"kind": "text", "content": T}` and, in a `user` message,
`{"kind": "image", "content": R}`, so a vision model reads an image:

```xml
<invoke id="read" type="https://github.com/stateforward/floppy/services/generate"
        env:apikey="FLOPPY_API_KEY" env:baseurl="FLOPPY_BASE_URL">
  <param name="model" expr="model"/>
  <param name="messages" expr="[{role: 'user', content: [
      {kind: 'text', content: 'Read all visible text in this image. Return only the text.'},
      {kind: 'image', content: 'media/page.png'}]}]"/>
</invoke>
```

Media moves by reference, never as bytes in a document. `R` is a path-only
URI reference (no scheme, query, fragment, or `.`/`..` segment) resolved
against the guest root, so `media/page.png` names `/media/page.png`. The
service reads it from the directory the run grants and sends it as an
`image_url` part with a `data:` URL (PNG, JPEG, or WebP, by its
signature). The reply is the usual `generate.result` `{"content": T, "tool_calls": [], "request_bytes": B}`.

A reference that names no readable file in a granted directory (a symlink
out of it included), a file that is not such an image, or images over
their bound (2 MiB together) reject the start: the `<invoke>` raises
`error.execution` and nothing is sent. The host bounds a request body to
4 MiB, and an image's base64 `data:` URL is a third larger than its bytes,
so 2 MiB of images in all is the bound, and no image over it is sent.

## Grants

The component imports `wasi:filesystem`, so `scx run` refuses a run that
binds it without a `--dir` grant, even one that sends no image:

```console
mkdir -p media
scx run --env-file .env \
  --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
  <document>
```

The run also needs `FLOPPY_BASE_URL` (no default backend, decision D31),
and `FLOPPY_API_KEY` unless the backend is keyless: without a held key the
request carries no `authorization` header (decision D127).
