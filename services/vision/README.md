# vision

The `vision` invoked service: measures how much a view changed since the
last one handed on, as a Wasm component
(`https://github.com/stateforward/floppy/services/vision`, component
`floppy_service_vision`). The seeing ability
([`abilities/seeing/seeing.scx`](../../abilities/seeing/seeing.scx))
invokes it once per measured view. Its crate documentation
([src/lib.rs](src/lib.rs)) is the full contract.

- **Model.** `measure` is `byte_identity` (a SHA-256 of the file) or
  `frame_diff` (a 32-cell-a-side, blurred grayscale grid of the image
  decoded at an eighth of each side, computed in the component as OpenCV 5
  computes it: to the bit for PNG, JPEG and lossless WebP, within a step in
  a few cells for lossy WebP), `image` the view's media reference, and
  `previous` the signature of the last view handed on (`null` or absent
  for a first view). Its result is the done data `{"change": C,
  "signature": S}`, `C` in 0..1 or `null` for a first view, and no event.
  An invalid parameter, a `previous` of another measure or size, an
  unreadable, empty or over-bound file (4 MiB), or, for `frame_diff`, an
  image it does not decode (not a PNG, JPEG or WebP, a PNG or WebP under 8
  pixels a side, a JPEG whose data runs into the file's end, an
  arithmetic-coded JPEG, or one whose gray needs a subsampled component)
  rejects the start (`error.execution`), and the measurement fails.
- **Grant.** The media directory is the run's
  (`--dir https://github.com/stateforward/floppy/services/vision=HOST::/media`);
  a run that binds the service without it is refused before it starts.
  References resolve as the speech service's do: `media/a.png`,
  `/media/a.png` and `file:///media/a.png` all name `/media/a.png`, and a
  reference with another scheme, a query, a fragment, or a `.` or `..`
  segment is refused.
- **WASI.** The component's `wasi:cli` imports are satisfied by its
  composed deny-all provider ([wasi-cli-deny.wat](wasi-cli-deny.wat)), and
  it keeps exactly the filesystem it imports for the host.
