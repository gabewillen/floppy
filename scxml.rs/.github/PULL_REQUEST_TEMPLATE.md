## Summary

<!-- What changes and why. Link the issue it resolves, if any. -->

## Proof

<!--
CONTRIBUTING.md, "Required workspace proof", lists the commands; CI runs
the same steps. List what you ran locally and its result, and anything
you could not run (for example the Docker canonical build or headless
Chromium) and why.
-->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --workspace --all-targets --locked`
- [ ] The wasm32-wasip2 clippy list and `sh scripts/verify-wasm-capabilities.sh`, if a component, WIT package, or guest dependency changed
- [ ] The affected Wasmtime host, CLI, and browser-host suites
- [ ] `cargo deny --offline check licenses`, if dependencies changed

## Conformance and artifacts

- [ ] No IRP disposition changed, or the change is explained and the baselines were regenerated from the canonical release (`scripts/build-canonical-components.sh`) with their published digests updated
- [ ] Blessed fixtures (`sh scripts/bless-fixtures.sh`) were regenerated only for intended output changes, and their diff reviewed
- [ ] Documentation, WIT, schemas, and examples match the change, including the current-gaps list in `docs/limitations.md` and its README summary
- [ ] `CHANGELOG.md` `[Unreleased]` records any user-visible change
