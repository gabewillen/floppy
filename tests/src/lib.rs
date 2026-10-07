//! floppy's integration tests.
//!
//! floppy has no runtime of its own (decision D14 in `docs/decisions.md`):
//! its bots are SCXML documents and its namespaces and services are Wasm
//! component packages, and all of them run on scxml.rs. The tests under
//! `tests/` therefore drive `scx run`, built from the `scxml.rs/` subtree
//! with the session components `scripts/build-components.sh` builds
//! embedded, and resolve the packages from the cache that script publishes
//! them into. This library holds no code; it exists so the
//! tests are a workspace member that `cargo test --workspace` runs.

#![forbid(unsafe_code)]
