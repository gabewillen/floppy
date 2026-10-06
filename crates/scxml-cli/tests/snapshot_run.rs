#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `scx snapshot run` end to end through the shipped components
//! (`wasmtime` feature): the success path against the shared session
//! snapshot fixtures, and every failure exit. Also pins that
//! `conformance run --snapshot-every-checkpoint` still applies the plain
//! run's gates.
//!
//! The release components are built and verified once by
//! `scripts/verify-wasm-capabilities.sh` into `target/cli-components`; set
//! `SCXML_COMPONENTS_DIR` and `SCXML_COMPONENTS_MANIFEST_SHA256` to use an
//! already verified release instead.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

/// Exactly the release components the Wasmtime session host links.
const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_compiler",
    "scxml_machine",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

/// The verified release directory and its trusted manifest digest.
fn release() -> &'static (String, String) {
    static RELEASE: OnceLock<(String, String)> = OnceLock::new();
    RELEASE.get_or_init(|| {
        if let (Ok(dir), Ok(digest)) = (
            std::env::var("SCXML_COMPONENTS_DIR"),
            std::env::var("SCXML_COMPONENTS_MANIFEST_SHA256"),
        ) {
            return (dir, digest);
        }
        let workspace = workspace();
        let target = workspace.join("target/cli-components");
        let dir = target.join("wasm-artifacts/snapshot-run");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS)
            .current_dir(&workspace)
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", &dir)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest = fs::read(dir.join("manifest.tsv")).expect("the script writes a manifest");
        // Test-only trust anchor: the manifest the proof script just verified.
        let digest = format!("{:x}", Sha256::digest(&manifest));
        (dir.display().to_string(), digest)
    })
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scx"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("scx must run")
}

/// `snapshot run DOC <release flags> <extra>`.
fn run(document: &Path, extra: &[&str]) -> Output {
    let (dir, digest) = release();
    let document = document.display().to_string();
    let mut args = vec![
        "snapshot",
        "run",
        document.as_str(),
        "--components",
        dir.as_str(),
        "--components-manifest-sha256",
        digest.as_str(),
    ];
    args.extend_from_slice(extra);
    cli(&args)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn fixtures() -> PathBuf {
    workspace().join("tests/fixtures/session-snapshot")
}

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-snapshot-run")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

/// The session content of a document: everything but the component
/// digests (which bind the release build) and the document digest.
fn content(bytes: &[u8]) -> serde_json::Value {
    let document: serde_json::Value = serde_json::from_slice(bytes).expect("a JSON document");
    serde_json::json!({
        "version": document["version"],
        "options": document["options"],
        "clock": document["snapshot"]["clock"],
        "root": document["snapshot"]["root"],
    })
}

fn golden(checkpoint: &str) -> Vec<u8> {
    fs::read(fixtures().join(format!("golden/delayed-send/{checkpoint}.snapshot.json")))
        .expect("the golden is readable")
}

#[test]
fn run_writes_the_requested_checkpoints() {
    let document = fixtures().join("delayed-send.scx");
    let directory = scratch("every");
    let out = directory.join("docs");
    let out_text = out.display().to_string();
    let output = run(
        &document,
        &[
            "--event",
            "go",
            "--at",
            "3",
            "--at",
            "0",
            "--at",
            "1",
            "--output-dir",
            &out_text,
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let mut written: Vec<String> = fs::read_dir(&out)
        .expect("the output directory exists")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    written.sort();
    assert_eq!(
        written,
        [
            "0000.snapshot.json",
            "0001.snapshot.json",
            "0003.snapshot.json"
        ]
    );
    for checkpoint in ["0000", "0001", "0003"] {
        let bytes = fs::read(out.join(format!("{checkpoint}.snapshot.json"))).expect("written");
        let path = out.join(format!("{checkpoint}.snapshot.json"));
        let check = cli(&["snapshot", "check", &path.display().to_string()]);
        assert_eq!(check.status.code(), Some(0), "{}", text(&check.stderr));
        // The CLI runs the shared fixture exactly as the host golden
        // harness does (`go`, then drain), so the sessions are identical.
        assert_eq!(
            content(&bytes),
            content(&golden(checkpoint)),
            "{checkpoint}"
        );
    }

    // One checkpoint goes to stdout, or to --output FILE; the default is
    // the final checkpoint.
    let stdout = run(&document, &["--event", "go", "--at", "1"]);
    assert_eq!(stdout.status.code(), Some(0), "{}", text(&stdout.stderr));
    assert_eq!(
        stdout.stdout,
        fs::read(out.join("0001.snapshot.json")).expect("written")
    );
    let file = directory.join("final.json");
    let file_text = file.display().to_string();
    let last = run(&document, &["--event", "go", "--output", &file_text]);
    assert_eq!(last.status.code(), Some(0), "{}", text(&last.stderr));
    assert!(last.stdout.is_empty());
    assert_eq!(
        fs::read(&file).expect("written"),
        fs::read(out.join("0003.snapshot.json")).expect("written")
    );

    // Every checkpoint, twice: byte-identical documents.
    let every = directory.join("every");
    let again = directory.join("again");
    for dir in [&every, &again] {
        let dir_text = dir.display().to_string();
        let output = run(
            &document,
            &["--event", "go", "--every", "--output-dir", &dir_text],
        );
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    }
    for entry in fs::read_dir(&every).expect("written") {
        let name = entry.expect("entry").file_name();
        assert_eq!(
            fs::read(every.join(&name)).expect("written"),
            fs::read(again.join(&name)).expect("written"),
            "{name:?}"
        );
    }
    fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn run_failures_exit_2() {
    let document = fixtures().join("delayed-send.scx");
    let directory = scratch("failures");

    let unreached = run(&document, &["--event", "go", "--at", "99"]);
    assert_eq!(unreached.status.code(), Some(2));
    assert!(
        text(&unreached.stderr).contains("was never reached"),
        "{}",
        text(&unreached.stderr)
    );
    assert!(unreached.stdout.is_empty());

    let several = run(&document, &["--every"]);
    assert_eq!(several.status.code(), Some(2));
    assert!(text(&several.stderr).contains("several checkpoints need --output-dir DIR"));

    let (dir, _) = release();
    let rejected = cli(&[
        "snapshot",
        "run",
        &document.display().to_string(),
        "--components",
        dir,
        "--components-manifest-sha256",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(rejected.stdout.is_empty());

    let invalid = directory.join("invalid.scx");
    fs::write(
        &invalid,
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="missing"><state id="a"/></scxml>"#,
    )
    .expect("writable");
    let invalid = run(&invalid, &[]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        text(&invalid.stderr).contains("error[SCXML"),
        "{}",
        text(&invalid.stderr)
    );
    assert!(invalid.stdout.is_empty());
    fs::remove_dir_all(&directory).expect("cleanup");
}

/// The snapshot proof still applies the plain run's gates: a `--baseline`
/// that cannot be read fails the run.
#[test]
fn the_snapshot_proof_honours_the_baseline() {
    let (dir, digest) = release();
    let corpus = workspace().join("tests/conformance/w3c-irp/prepared/xpath");
    // The proof writes its summary under the current directory.
    let directory = scratch("proof");
    let output = Command::new(env!("CARGO_BIN_EXE_scx"))
        .current_dir(&directory)
        .args([
            "conformance",
            "run",
            "--corpus",
            &corpus.display().to_string(),
            "--id",
            "144",
            "--components",
            dir,
            "--components-manifest-sha256",
            digest,
            "--snapshot-every-checkpoint",
            "--baseline",
            "missing-baseline.json",
            "--fail-on-regression",
        ])
        .output()
        .expect("scx must run");
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("snapshot-every-checkpoint: 1 executed cases"),
        "{stdout}"
    );
    assert!(
        stdout.contains("144 "),
        "the plain case line is printed: {stdout}"
    );
    assert!(directory.join("target/snapshot-proof/xpath.json").is_file());
    fs::remove_dir_all(&directory).expect("cleanup");
}
