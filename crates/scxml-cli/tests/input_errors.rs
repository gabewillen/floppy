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

/// Runs `scx validate` and kills it after a bound, so a read that never ends
/// fails the test instead of hanging it; `None` when it did not finish.
#[cfg(unix)]
fn validate_within(root: &Path, relative: &str, bound: std::time::Duration) -> Option<Output> {
    use std::process::Stdio;
    use std::time::Instant;
    let mut child = Command::new(env!("CARGO_BIN_EXE_scx"))
        .current_dir(root)
        .args(["validate", relative, "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scx must run");
    let deadline = Instant::now() + bound;
    while child.try_wait().expect("scx must be waitable").is_none() {
        if Instant::now() > deadline {
            child.kill().expect("a hung scx must be killable");
            child.wait().expect("a killed scx must be reaped");
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Some(
        child
            .wait_with_output()
            .expect("scx output must be readable"),
    )
}

/// A FIFO in the fixture directory, replacing any left by an earlier run.
#[cfg(unix)]
fn fifo(path: &Path) {
    let _ = fs::remove_file(path);
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo must run");
    assert!(status.success(), "mkfifo must create {}", path.display());
}

/// A root source that is a FIFO is refused as not a regular file instead of
/// blocking on the open.
#[cfg(unix)]
#[test]
fn a_fifo_source_is_refused_without_blocking() {
    let root = workspace_root();
    let directory = root.join("target/cli-input-errors/fifo-root");
    fs::create_dir_all(&directory).expect("fixture directory must be creatable");
    fifo(&directory.join("root.scxml"));
    let output = validate_within(
        &root,
        "target/cli-input-errors/fifo-root/root.scxml",
        std::time::Duration::from_secs(120),
    )
    .expect("validate must not block on a FIFO source");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("not a regular file"), "{stderr}");
}

/// A literal `<invoke src>` naming a FIFO or a device is not read: the child
/// is skipped as the run path skips it, and validation finishes.
#[cfg(unix)]
#[test]
fn an_invoked_document_that_is_not_a_regular_file_is_skipped() {
    let root = workspace_root();
    let directory = root.join("target/cli-input-errors/invoke-special");
    fs::create_dir_all(&directory).expect("fixture directory must be creatable");
    fifo(&directory.join("child.scxml"));
    // The bounds are wide because a loaded host can delay a debug `scx` by
    // tens of seconds; an unbounded read of `/dev/zero` grows memory by
    // gigabytes a second, so that case is killed sooner than a blocked open.
    for (name, src, bound) in [
        ("fifo.scxml", "child.scxml", 120),
        ("zero.scxml", "file:///dev/zero", 20),
    ] {
        fs::write(
            directory.join(name),
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"><state id="s"><invoke src="{src}"/></state></scxml>"#
            ),
        )
        .expect("fixture must be writable");
        let relative = format!("target/cli-input-errors/invoke-special/{name}");
        let output = validate_within(&root, &relative, std::time::Duration::from_secs(bound))
            .unwrap_or_else(|| panic!("validate must not block reading `{src}`"));
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        decode_canonical(&output.stdout).expect("json is a canonical report");
    }
}
