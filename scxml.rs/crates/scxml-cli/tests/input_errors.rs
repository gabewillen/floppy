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
    scx_within(root, &["validate", relative, "--format", "json"], bound)
}

/// Runs `scx` with `args` and kills it after a bound, so a read that never
/// ends fails the test instead of hanging it; `None` when it did not finish.
#[cfg(unix)]
fn scx_within(root: &Path, args: &[&str], bound: std::time::Duration) -> Option<Output> {
    use std::process::Stdio;
    use std::time::Instant;
    let mut child = Command::new(env!("CARGO_BIN_EXE_scx"))
        .current_dir(root)
        .args(args)
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

/// Every input file `scx` reads besides the source is refused as not a
/// regular file when it is a FIFO, instead of blocking on the open: the
/// machine template, a script, an extension package manifest, a snapshot
/// document, and a replay log.
#[cfg(unix)]
#[test]
fn a_fifo_input_file_is_refused_without_blocking() {
    let root = workspace_root();
    let base = "target/cli-input-errors/fifo-inputs";
    let directory = root.join(base);
    fs::create_dir_all(&directory).expect("fixture directory must be creatable");
    fs::write(
        directory.join("doc.scxml"),
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"><final id="s"/></scxml>"#,
    )
    .expect("fixture must be writable");
    for name in [
        "template.wasm",
        "script.wasm",
        "package.json",
        "snap.json",
        "replay.jsonl",
    ] {
        fifo(&directory.join(name));
    }
    let cases = fifo_cases(base);
    // The cases run at once, so every input that blocks is named in one run.
    let failures = std::thread::scope(|scope| {
        let runs = cases
            .iter()
            .map(|(input, args, expected)| {
                let root = &root;
                scope.spawn(move || {
                    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
                    let Some(output) = scx_within(root, &args, std::time::Duration::from_secs(120))
                    else {
                        return Some(format!("{input}: scx blocked on the FIFO"));
                    };
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    (output.status.success()
                        || !stderr.contains(expected)
                        || !stderr.contains("regular file"))
                    .then(|| format!("{input}: {stderr}"))
                })
            })
            .collect::<Vec<_>>();
        runs.into_iter()
            .filter_map(|run| run.join().expect("a case thread must not panic"))
            .collect::<Vec<_>>()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Each FIFO input of [`a_fifo_input_file_is_refused_without_blocking`] under
/// `base`: what it is, the `scx` arguments that read it, and the text its
/// refusal names.
#[cfg(unix)]
fn fifo_cases(base: &str) -> [(&'static str, Vec<String>, &'static str); 5] {
    let digest = "0".repeat(64);
    let path = |name: &str| format!("{base}/{name}");
    [
        (
            "machine template",
            vec![
                "compile".into(),
                path("doc.scxml"),
                "--artifact".into(),
                "component-wasm".into(),
                "--machine-template".into(),
                path("template.wasm"),
                "--machine-template-sha256".into(),
                digest.clone(),
                "--output".into(),
                path("out.wasm"),
            ],
            "machine template must be a regular file",
        ),
        (
            "script",
            vec![
                "compile".into(),
                path("doc.scxml"),
                "--script".into(),
                format!("s.wasm={}", path("script.wasm")),
                "--script-sha256".into(),
                format!("s.wasm={digest}"),
                "--output".into(),
                path("out.json"),
            ],
            "script must be a regular file",
        ),
        (
            "extension package",
            vec![
                "validate".into(),
                path("doc.scxml"),
                "--extension-package".into(),
                path("package.json"),
            ],
            "not a regular file",
        ),
        (
            "snapshot document",
            vec!["snapshot".into(), "check".into(), path("snap.json")],
            "not a regular file",
        ),
        (
            "replay log",
            vec![
                "run".into(),
                path("doc.scxml"),
                "--components".into(),
                base.into(),
                "--components-manifest-sha256".into(),
                digest.clone(),
                "--replay".into(),
                path("replay.jsonl"),
            ],
            "[replay-unreadable]",
        ),
    ]
}

/// A replay log that is a device is refused before it is read: reading
/// `/dev/zero` never ends and grows memory by gigabytes a second, so the bound
/// is short.
#[cfg(unix)]
#[test]
fn a_device_replay_log_is_refused_before_it_is_read() {
    let root = workspace_root();
    let base = "target/cli-input-errors/device-replay";
    let directory = root.join(base);
    fs::create_dir_all(&directory).expect("fixture directory must be creatable");
    let document = format!("{base}/doc.scxml");
    fs::write(
        root.join(&document),
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"><final id="s"/></scxml>"#,
    )
    .expect("fixture must be writable");
    let digest = "0".repeat(64);
    let output = scx_within(
        &root,
        &[
            "run",
            &document,
            "--components",
            base,
            "--components-manifest-sha256",
            &digest,
            "--replay",
            "/dev/zero",
        ],
        std::time::Duration::from_secs(20),
    )
    .expect("scx must not read `/dev/zero` as a replay log");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("[replay-unreadable]") && stderr.contains("not a regular file"),
        "{stderr}"
    );
}
