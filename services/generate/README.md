# generate

The `generate` invoked service: one model generation per `<invoke>` on the
`OpenAI`-compatible Chat Completions API, as a Wasm component
(`https://github.com/stateforward/floppy/services/generate`). Its crate
documentation ([src/lib.rs](src/lib.rs)) is the full contract: parameters,
events, the request, the reply, bounds, and telemetry.

## Images

A `messages` entry's `content` may be an array of the original's reading parts,
`{"kind": "text", "content": T}` and, in a `user` message,
`{"kind": "image", "content": R}`, so a vision model reads an image as
The original's `ImageDecoder` does:

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
`image_url` part with a `data:` URL (PNG, JPEG, GIF, or WebP, by its
signature). The reply is the usual `generate.result` `{"text": T}`.

A reference that names no readable file in a granted directory (a symlink
out of it included), a file that is not such an image, or images over
their bound (2 MiB together) reject the start: the `<invoke>` raises
`error.execution` and nothing is sent.

## Grants

The component imports `wasi:filesystem`, so `scx run` refuses a run that
binds it without a `--dir` grant, even one that sends no image:

```console
mkdir -p media
scx run --env-file .env \
  --dir "https://github.com/stateforward/floppy/services/generate=$PWD/media::/media" \
  <document>
```

The run also needs `FLOPPY_BASE_URL` and `FLOPPY_API_KEY` (no default
backend, decision D31).
