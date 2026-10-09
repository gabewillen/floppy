//! Session hosts constructed from rayon tasks never deadlock on the
//! process-wide compiled-component cache.
//!
//! Wasmtime's parallel compilation runs Cranelift on rayon's global pool.
//! While a compiling thread waits for that work it may run other queued
//! global-pool jobs, including an embedder's own host construction for the
//! same component; that construction must never block on the in-flight
//! compilation. This file is its own test binary so its global pool is the
//! small one configured here and the cache starts cold.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies the release
//! components into `target/session-host-proof` (this suite's own artifacts
//! directory inside it).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rayon::prelude::*;
use scxml_host_wasmtime::{
    ReleaseComponents, SessionHost, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig,
};
use sha2::{Digest as _, Sha256};

const COMPONENTS: [&str; 8] = [
    "scxml_machine",
    "scxml_compiler",
    "scxml_data_model_ecmascript",
    "scxml_data_model_xpath",
    "scxml_data_model_null",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_clock",
];

/// Host constructions raced from the pool (and, in total, from plain threads).
const HOSTS: usize = 32;
/// Plain (non-pool) threads constructing hosts beside the pool.
const PLAIN_THREADS: usize = 4;
/// Generous bound (about 95 s in a debug build, 12 s in release) for the
/// constructions; a deadlock leaves every thread idle and never finishes, so
/// any bound separates the two.
const WATCHDOG: Duration = Duration::from_mins(15);

fn release_components() -> ReleaseComponents {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent");
    let release = prebuilt::release_dir().unwrap_or_else(|| {
        let target = workspace.join("target/session-host-proof");
        // This suite's own artifact directory: suites sharing the Cargo
        // target never rewrite a manifest another suite is reading.
        let release = target.join("wasm-artifacts/session-host-rayon");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .arg("--artifacts-dir")
            .arg(&release)
            .args(COMPONENTS)
            .current_dir(workspace)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        release
    });
    let manifest = std::fs::read(release.join("manifest.tsv")).expect("manifest is readable");
    let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
        .expect("manifest digest is canonical");
    ReleaseComponents::load(&release, trusted)
        .unwrap_or_else(|error| panic!("the release components load: {error}"))
}

fn construct(release: &ReleaseComponents) {
    let host = SessionHost::new(
        &release.session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the verified components link");
    drop(host);
}

/// Pool tasks and plain threads construct hosts at once, every host dropped
/// at once so the (weak) cache keeps going cold and compiling again. A
/// plain thread's compilation runs on the global pool; a pool worker that
/// executes it may pick up a queued host construction for the same
/// component, which must compile rather than wait for that very
/// compilation.
#[test]
fn hosts_constructed_from_rayon_tasks_complete() {
    let release = Arc::new(release_components());
    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build_global()
        .expect("this test binary configures the global pool first");
    let (done, finished) = mpsc::channel();
    let pool_release = Arc::clone(&release);
    let pool_done = done.clone();
    let pool = std::thread::spawn(move || {
        (0..HOSTS)
            .into_par_iter()
            .for_each(|_| construct(&pool_release));
        pool_done.send(()).expect("the watchdog is listening");
    });
    let plain: Vec<_> = (0..PLAIN_THREADS)
        .map(|_| {
            let release = Arc::clone(&release);
            let done = done.clone();
            std::thread::spawn(move || {
                for _ in 0..HOSTS / PLAIN_THREADS {
                    construct(&release);
                }
                done.send(()).expect("the watchdog is listening");
            })
        })
        .collect();
    for _ in 0..=PLAIN_THREADS {
        finished
            .recv_timeout(WATCHDOG)
            .expect("every host construction finishes (no deadlock)");
    }
    pool.join().expect("the pool constructions succeed");
    for thread in plain {
        thread
            .join()
            .expect("the plain-thread constructions succeed");
    }
}
