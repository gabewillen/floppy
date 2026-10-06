//! nemobot's integration tests.
//!
//! nemobot has no runtime of its own (decision D14 in `docs/decisions.md`):
//! its bots are SCXML documents and its namespaces and services are Wasm
//! component packages, and all of them run on scxml.rs. The tests under
//! `tests/` therefore drive `scxml-cli run`, built from the `scxml.rs/`
//! subtree, with the components `scripts/build-components.sh` builds and
//! the argument files it and the bots ship (`components.args`,
//! `bots/<name>/<name>.args`). This library holds no code; it exists so the
//! tests are a workspace member that `cargo test --workspace` runs.

#![forbid(unsafe_code)]
