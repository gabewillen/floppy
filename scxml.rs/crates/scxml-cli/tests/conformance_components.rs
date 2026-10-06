#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `conformance run` executes only through the shipped components: the
//! component release is required, and a build or release that cannot link
//! them fails closed before any case runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/w3c-irp/prepared/xpath")
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scxml-cli"))
        .args(["conformance", "run", "--corpus"])
        .arg(corpus())
        .args(args)
        .output()
        .expect("scxml-cli must spawn")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn the_component_release_is_required() {
    let output = run(&["--id", "144"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("requires --components DIR"));
    let output = run(&["--id", "144", "--components", "release"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("requires --components-manifest-sha256"));
}

#[test]
fn an_unlinkable_release_fails_closed_before_any_case() {
    let output = run(&[
        "--id",
        "144",
        "--components",
        "missing-components",
        "--components-manifest-sha256",
        DIGEST,
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    let expected = if cfg!(feature = "wasmtime") {
        "components-rejected"
    } else {
        "wasmtime-disabled"
    };
    assert!(
        text(&output.stderr).contains(expected),
        "{}",
        text(&output.stderr)
    );
    assert!(
        text(&output.stdout).is_empty(),
        "no case may be reported: {}",
        text(&output.stdout)
    );
}
