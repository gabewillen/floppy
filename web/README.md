# floppy web

The website (parts register) and the workshop (Build, Workbench, TestBench,
Observe). Both are plain web components on
[hsm.ts](https://github.com/stateforward/hsm.ts), loaded from jsDelivr; there is
no bundler step for the pages themselves.

- `src/site.html`, `src/site-data.js`, `src/site-sheet.js`: the website.
- `src/workshop.html`: the workshop. Its blocks come from `src/extract.py`,
  which reads the SCXML and namespace XSDs in this repository at build time.
- `src/critter.bundle.js`: the animated robot (`<floppy-bot>`), an esbuild
  0.24.0 bundle with hsm.ts kept external.

Build with `python3 web/build.py`, then open `web/dist/index.html`.
`.github/workflows/pages.yml` builds and deploys `web/dist` to GitHub Pages on
every push to `main` that touches `web/`.
