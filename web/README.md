# floppy web

The website (parts register) and the workshop (Build, Workbench, TestBench,
Observe). Both are plain web components on
[hsm.ts](https://github.com/stateforward/hsm.ts), loaded from jsDelivr; there is
no bundler step for the pages themselves.

- `src/site.html`, `src/site-data.js`, `src/site-sheet.js`: the website. The
  register's facts (package versions, artifacts, WIT worlds, env slots,
  origins, elements and their attributes, and the documents that use each)
  come from `src/register.py`, which reads every package in this repository
  at build time; `site-data.js` keeps the prose.
- `src/workshop.html`: the workshop. Its blocks come from `src/extract.py`,
  which reads the SCXML and namespace XSDs in this repository at build time.
- `src/runtime.js`: the workshop's runtime. It validates, compiles and runs
  the document in the page with scxml.rs's browser host
  (`scxml.rs/hosts/scxml-host-browser`): the same validator, compiler,
  machine, ECMAScript data model and extension components (`scx` and every
  floppy namespace) that `scx run` uses. The TestBench and Observe show the
  session's snapshot (its configuration, enabled transitions, data model,
  delayed sends and invocations); the page selects no transition. A
  document's scripts, schemas and invoked children load from the site by
  the same relative URIs as under `scx run`, so the workshop opens any bot or
  ability of this repository, or a `.scx` file of your own; each document
  answers with its own URL as its base and with its `event:schema` files,
  so an invoked child resolves its names against itself. The browser host
  binds no invoked service, so a service `<invoke>` raises `error.execution`
  in the page, and a run of more than a handful of sessions (the floppy
  bot) exceeds the browser's limit on live Wasm memories.
- `src/critter.bundle.js`: the animated robot (`<floppy-bot>`), an esbuild
  0.24.0 bundle with hsm.ts kept external.

Build with:

```console
sh web/build-host.sh     # the browser host and its components, into target/web
python3 web/build.py     # the pages, into web/dist
```

then serve `web/dist` over HTTP (the components load with `fetch`, so
`file:` does not work), for example `python3 -m http.server -d web/dist`.
`build-host.sh` needs what scxml.rs's browser host build needs (Node.js
v24.11.1, the pinned `wasm-tools` and `wasi-virt`) and what
`scripts/build-components.sh` needs (wasi-sdk 25 at `WASI_SDK_PATH`).
`.github/workflows/pages.yml` runs both and deploys `web/dist` to GitHub
Pages on every push to `main` that touches the site, the documents, the
namespaces or the subtree.
