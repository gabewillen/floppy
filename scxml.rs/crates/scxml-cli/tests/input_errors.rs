//! `scx validate` on input it cannot read as a document: a missing
//! file names its path, and bytes that are not UTF-8 are one located
//! `XML001` diagnostic in every output format (an agent that parses
//! `--format json` or `sarif` always receives a report).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use scxml_reporting::decode_canonical;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root must resolve")
}

fn validate(root: &Path, relative: &str, format: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scx"))
        .current_dir(root)
        .args(["validate", relative, "--format", format])
        .output()
        .expect("scx must run")
}

#[test]
fn invalid_utf8_is_a_located_diagnostic_in_every_format() {
    let root = workspace_root();
    let directory = root.join("target/cli-input-errors");
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    // `é` saved as Windows-1252 (0xE9) on line 3, after two ASCII columns.
    let source = b"<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\">\n  <final id=\"done\">\n  <!-- caf\xE9 -->\n  </final>\n</scxml>\n";
    fs::write(directory.join("latin1.scxml"), source).expect("fixture must be writable");
    let relative = "target/cli-input-errors/latin1.scxml";

    let json = validate(&root, relative, "json");
    assert_eq!(
        json.status.code(),
        Some(1),
        "invalid input is an error report"
    );
    assert!(json.stderr.is_empty(), "json keeps stderr clean");
    let artifact = decode_canonical(&json.stdout).expect("json is a canonical report");
    let payload = artifact.validator_payload().expect("validator payload");
    assert_eq!(payload.diagnostics.len(), 1);
    let diagnostic = &payload.diagnostics[0];
    assert_eq!(diagnostic.code, "XML001");
    assert_eq!(diagnostic.message, "the document is not valid UTF-8");
    let start = usize::try_from(diagnostic.primary.span.start).expect("span fits usize");
    let end = usize::try_from(diagnostic.primary.span.end).expect("span fits usize");
    assert_eq!(payload.source_text.get(start..end), Some("\u{FFFD}"));
    assert!(
        diagnostic
            .primary
            .message
            .as_deref()
            .is_some_and(|label| label.contains("0xE9")),
        "{diagnostic:?}"
    );

    let sarif = validate(&root, relative, "sarif");
    assert_eq!(sarif.status.code(), Some(1));
    let sarif: serde_json::Value = serde_json::from_slice(&sarif.stdout).expect("sarif is JSON");
    let region = &sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
    assert_eq!(region["startLine"], 3, "{region}");
    assert_eq!(region["startColumn"], 11, "{region}");

    let human = validate(&root, relative, "human");
    assert_eq!(human.status.code(), Some(1));
    let stderr = String::from_utf8(human.stderr).expect("human output is UTF-8");
    assert!(
        stderr.contains(&format!("{relative}:3:11")),
        "the path and line:column are shown: {stderr}"
    );
}

/// The native CLI reads and validates a source of any size; the 8 MiB
/// source bound is a wasm-build bound only.
#[test]
fn a_source_past_the_wasm_size_bound_validates() {
    let root = workspace_root();
    let directory = root.join("target/cli-input-errors");
    fs::create_dir_all(&directory).expect("fixture directory must be creatable");
    let comment = "x".repeat(9 * 1024 * 1024);
    fs::write(
        directory.join("large.scxml"),
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><!--{comment}--><final id="f"/></scxml>"#
        ),
    )
    .expect("fixture must be writable");
    let output = validate(&root, "target/cli-input-errors/large.scxml", "json");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_missing_file_names_its_path() {
    let root = workspace_root();
    let output = validate(
        &root,
        "target/cli-input-errors/does-not-exist.scxml",
        "json",
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "I/O failure is a usage failure"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(
        stderr.starts_with("error: cannot read `target/cli-input-errors/does-not-exist.scxml`:"),
        "{stderr}"
    );
}
