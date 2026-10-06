//! The shipped component release every executing conformance test runs
//! through (`wasmtime` feature).
//!
//! The release components are built and verified once per test binary by
//! `scripts/verify-wasm-capabilities.sh` into the shared Cargo target
//! `target/conformance-components` (Cargo serializes the builds) and this
//! binary's own artifact directory
//! `target/conformance-components/wasm-artifacts/<binary>`, so concurrently
//! running binaries never publish a manifest another binary is reading. Set
//! `SCXML_COMPONENTS_DIR` and `SCXML_COMPONENTS_MANIFEST_SHA256` to use an
//! already verified release instead; it must also hold the example
//! extension component the namespace suites bind, whose trusted SHA-256 is
//! `SCXML_EXTENSION_COMPONENT_SHA256` (else the SHA-256 of the bytes the
//! release holds, a test-only trust anchor).
//!
//! One [`WasmExecutor`] (it is `!Send`) lives on a dedicated worker thread
//! for the whole binary, so the components are compiled once; tests submit
//! runs to it with [`with_executor`], which serializes them. A second one,
//! binding the example extension package, serves the namespace suites that
//! use it ([`with_extension_executor`]).
#![allow(
    dead_code,
    reason = "shared by several test binaries; each uses a subset"
)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use scxml_conformance::{
    ExtensionRelease, LoadedSuite, RunOptions, RunReport, WasmExecutor, load_suite, run_suite,
};
use sha2::{Digest as _, Sha256};

/// Exactly the release components [`WasmExecutor`] links, and the example
/// extension component the namespace suites bind.
const COMPONENTS: &[&str] = &[
    "scxml_extension_example",
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_compiler",
    "scxml_machine",
];

type Job = Box<dyn FnOnce(&WasmExecutor) + Send>;

pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

/// The name of the running test binary (its artifact directory).
fn binary_name() -> String {
    let executable = std::env::current_exe().expect("the test binary path resolves");
    let stem = executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("the test binary has a UTF-8 name");
    // Cargo names test binaries `<target>-<metadata hash>`.
    stem.rsplit_once('-')
        .map_or(stem, |(name, _)| name)
        .to_owned()
}

/// The verified release directory and its trusted manifest digest.
pub fn release() -> &'static (PathBuf, String) {
    static RELEASE: OnceLock<(PathBuf, String)> = OnceLock::new();
    RELEASE.get_or_init(|| {
        if let (Ok(dir), Ok(digest)) = (
            std::env::var("SCXML_COMPONENTS_DIR"),
            std::env::var("SCXML_COMPONENTS_MANIFEST_SHA256"),
        ) {
            return (PathBuf::from(dir), digest);
        }
        let workspace = workspace();
        let target = workspace.join("target/conformance-components");
        let dir = target.join("wasm-artifacts").join(binary_name());
        let status = std::process::Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS)
            .current_dir(&workspace)
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", &dir)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest =
            std::fs::read(dir.join("manifest.tsv")).expect("the script writes a manifest");
        // Test-only trust anchor: the manifest the proof script just verified.
        let digest = format!("{:x}", Sha256::digest(&manifest));
        (dir, digest)
    })
}

/// The trusted SHA-256 of the release's example extension component.
pub fn extension_sha256() -> String {
    std::env::var("SCXML_EXTENSION_COMPONENT_SHA256").unwrap_or_else(|_| {
        let bytes = std::fs::read(release().0.join("scxml_extension_example.virtualized.wasm"))
            .expect("the release holds the example extension component");
        // Test-only trust anchor: the bytes the proof script just verified.
        format!("{:x}", Sha256::digest(&bytes))
    })
}

/// A worker thread owning one executor that binds the extension packages
/// whose manifests (workspace-relative) are `packages`.
fn spawn_worker(packages: &'static [&'static str]) -> Mutex<Sender<Job>> {
    let (dir, digest) = release().clone();
    let extension = if packages.is_empty() {
        String::new()
    } else {
        extension_sha256()
    };
    let (sender, receiver) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        let executor =
            WasmExecutor::from_release_dir(&dir, &digest).expect("the verified release links");
        let executor = if packages.is_empty() {
            executor
        } else {
            let releases = packages
                .iter()
                .map(|manifest| {
                    ExtensionRelease::read(&workspace().join(manifest), &dir, &extension)
                        .expect("the extension package reads")
                })
                .collect();
            executor
                .with_extensions(releases)
                .expect("the namespace packages bind")
        };
        for job in receiver {
            job(&executor);
        }
    });
    Mutex::new(sender)
}

fn worker() -> &'static Mutex<Sender<Job>> {
    static WORKER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    WORKER.get_or_init(|| spawn_worker(&[]))
}

/// The example extension package the namespace suites bind.
const EXAMPLE_PACKAGE: &str = "components/scxml-extension-example/package/example.extension.json";

fn extension_worker() -> &'static Mutex<Sender<Job>> {
    static WORKER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    WORKER.get_or_init(|| spawn_worker(&[EXAMPLE_PACKAGE]))
}

/// Runs `job` against the binary's one component executor and returns its
/// result.
pub fn with_executor<T: Send + 'static>(
    job: impl FnOnce(&WasmExecutor) -> T + Send + 'static,
) -> T {
    submit(worker(), job)
}

/// Runs `job` against the binary's executor that binds the example
/// extension package.
pub fn with_extension_executor<T: Send + 'static>(
    job: impl FnOnce(&WasmExecutor) -> T + Send + 'static,
) -> T {
    submit(extension_worker(), job)
}

/// Runs the namespace suite `name`'s prepared `datamodel` corpus through the
/// executor binding the packages [`scxml_conformance::NAMESPACE_SUITES`]
/// names for it.
pub fn run_namespace_corpus(name: &str, datamodel: &str) -> RunReport {
    let packages = scxml_conformance::NAMESPACE_SUITES
        .iter()
        .find(|(suite, _)| *suite == name)
        .map(|(_, packages)| *packages)
        .expect("a namespace suite");
    let suite = load_suite(
        &workspace()
            .join("namespaces")
            .join(name)
            .join("irp/prepared")
            .join(datamodel),
    )
    .expect("the prepared corpus loads");
    let job = move |executor: &WasmExecutor| {
        run_suite(&suite, &RunOptions::default(), executor).expect("the report identity encodes")
    };
    match packages {
        [] => with_executor(job),
        [EXAMPLE_PACKAGE] => with_extension_executor(job),
        _ => panic!("the support binds only the example extension package"),
    }
}

fn submit<T: Send + 'static>(
    worker: &'static Mutex<Sender<Job>>,
    job: impl FnOnce(&WasmExecutor) -> T + Send + 'static,
) -> T {
    let (sender, receiver) = mpsc::channel();
    worker
        .lock()
        .expect("the executor worker lock is not poisoned")
        .send(Box::new(move |executor| {
            // The receiver outlives the job unless the test already failed.
            let _ = sender.send(job(executor));
        }))
        .expect("the executor worker is running");
    receiver
        .recv()
        .expect("the executor worker completes the run")
}

/// Runs `suite` through the shipped components.
pub fn run(suite: LoadedSuite, options: RunOptions) -> RunReport {
    with_executor(move |executor| {
        run_suite(&suite, &options, executor).expect("the report identity encodes")
    })
}

/// Loads and runs the prepared corpus at `path` through the shipped
/// components.
pub fn run_corpus(path: &Path, options: RunOptions) -> RunReport {
    run(
        load_suite(path).expect("the prepared corpus loads"),
        options,
    )
}
