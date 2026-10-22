# Review rules

[Open Code Review](https://github.com/alibaba/open-code-review) (`ocr`) reviews
pull requests against floppy's own rules, through `.github/workflows/review.yml`.

- `rule.json` picks one rule file per changed file by path glob (first match
  wins), adds the file types `ocr` would otherwise skip (`.scx`, `.scxml`,
  `.md`, `.xsd`), and excludes the `scxml.rs/` subtree, lockfiles and generated
  output. Entries with `merge_system_rule` keep `ocr`'s built-in checks for that
  language beside floppy's.
- `rules/*.md` hold the per-area rules: SCXML documents, their ECMAScript,
  event schemas, Rust components, packaging and scripts, tests, and docs.
- `background.md` holds the rules for every file and is passed as review
  context (`--background-file`).

AGENTS.md stays the normative source; these files restate it for the reviewer
and cite it, so a rule change there updates the matching rule here.

Run a review locally against `main`:

```sh
npm install -g @alibaba-group/open-code-review
ocr config provider && ocr config model
ocr review --from main --to HEAD --background-file .opencodereview/background.md
```

`ocr rules check <path>` shows which rule a file gets, and
`ocr delegate preview --from main --to HEAD` lists which files would be reviewed.
