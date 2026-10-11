//! Pinned event traces: what a phase 4 test's bot did, saved under
//! `tests/fixtures/phase4/traces/<test>.ndjson` and compared byte for byte,
//! so a restructure that must not change the bot's events proves it.
//!
//! A trace is NDJSON, one line per item, in order:
//!
//! * every `log`, `done` and `error` record of the run (every session's,
//!   with its `session` and `parent`), as `scx run` wrote it;
//! * then every request a fake backend received, as
//!   `{"type":"request","target":…,"body":…}` (the choices, strict
//!   schemas and prompts the services sent).
//!
//! Values that differ per run are normalized: the test's scratch directory
//! (`target/test-scratch/<pid>-<name>-<n>`) becomes `<scratch>`, the
//! workspace root `<workspace>`, and a UTC timestamp
//! (`YYYY-MM-DDTHH:MM:SSZ`) `<timestamp>`. Each record is written with its keys
//! sorted (`serde_json` without `preserve_order`).
//!
//! [`assert_trace`] rewrites the file instead of comparing when
//! `FLOPPY_TRACE_BLESS=1` [floppy name] is set.

use std::path::PathBuf;

use serde_json::{Value as Json, json};

/// The variable that rewrites the traces instead of comparing them.
pub const BLESS: &str = "FLOPPY_TRACE_BLESS";

/// The directory of the pinned traces.
pub fn dir() -> PathBuf {
    super::workspace().join("tests/fixtures/phase4/traces")
}

/// The record types a trace keeps.
const KEPT: [&str; 3] = ["log", "done", "error"];

/// `text` with the per-run scratch directories and the workspace root
/// replaced.
fn normalized(text: &str) -> String {
    let scratch = format!("{}/target/test-scratch/", super::workspace().display());
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&scratch) {
        out.push_str(&rest[..at]);
        out.push_str("<scratch>");
        let after = &rest[at + scratch.len()..];
        // The directory name runs to the next path separator (or the end
        // of the path's string).
        let end = after
            .find(|c: char| c == '/' || c == '"' || c == '\\' || c.is_whitespace())
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    timestamps_normalized(&out.replace(&super::workspace().display().to_string(), "<workspace>"))
}

/// `text` with each UTC timestamp in the original's form
/// (`YYYY-MM-DDTHH:MM:SSZ`, the rows' `last_used_at` and the like, which
/// the wall clock sets) replaced by `<timestamp>`.
fn timestamps_normalized(text: &str) -> String {
    const SHAPE: &[u8] = b"dddd-dd-ddTdd:dd:ddZ";
    let bytes = text.as_bytes();
    let matches_at = |at: usize| {
        bytes.get(at..at + SHAPE.len()).is_some_and(|window| {
            window.iter().zip(SHAPE).all(|(byte, shape)| match shape {
                b'd' => byte.is_ascii_digit(),
                other => byte == other,
            })
        })
    };
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        if matches_at(at) {
            out.push_str(&text[copied..at]);
            out.push_str("<timestamp>");
            at += SHAPE.len();
            copied = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// The trace of `records` (a run's records, in order) and `requests`
/// (`(target, body)` of each backend request, in order).
pub fn render(records: &[Json], requests: &[(String, Json)]) -> String {
    let mut text = String::new();
    let records = records
        .iter()
        .filter(|record| record["type"].as_str().is_some_and(|t| KEPT.contains(&t)));
    let requests = requests
        .iter()
        .map(|(target, body)| json!({"type": "request", "target": target, "body": body}));
    for item in records.cloned().chain(requests) {
        text.push_str(&normalized(&item.to_string()));
        text.push('\n');
    }
    text
}

/// The number of backend requests in the pinned trace of `test`, or
/// `None` when [`BLESS`] is `1` (the run is about to rewrite it) or no
/// trace is pinned.
pub fn pinned_requests(test: &str) -> Option<usize> {
    if std::env::var(BLESS).is_ok_and(|value| value == "1") {
        return None;
    }
    let pinned = std::fs::read_to_string(dir().join(format!("{test}.ndjson"))).ok()?;
    Some(
        pinned
            .lines()
            .filter(|line| {
                serde_json::from_str::<Json>(line).is_ok_and(|item| item["type"] == "request")
            })
            .count(),
    )
}

/// Asserts that `records` and `requests` render to the pinned trace of
/// `test`, or rewrites it when [`BLESS`] is `1`.
pub fn assert_trace(test: &str, records: &[Json], requests: &[(String, Json)]) {
    let actual = render(records, requests);
    let path = dir().join(format!("{test}.ndjson"));
    if std::env::var(BLESS).is_ok_and(|value| value == "1") {
        std::fs::create_dir_all(dir()).expect("the traces directory is creatable");
        std::fs::write(&path, &actual).expect("the trace is writable");
        return;
    }
    let pinned = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "no pinned trace {} ({error}); bless it with {BLESS}=1",
            path.display()
        )
    });
    if actual == pinned {
        return;
    }
    let line = actual
        .lines()
        .zip(pinned.lines())
        .position(|(a, p)| a != p)
        .unwrap_or_else(|| actual.lines().count().min(pinned.lines().count()));
    panic!(
        "the trace of {test} differs from {} at line {}:\n  pinned: {}\n  actual: {}\n(rerun with {BLESS}=1 only when the change is intended)",
        path.display(),
        line + 1,
        pinned.lines().nth(line).unwrap_or("<end>"),
        actual.lines().nth(line).unwrap_or("<end>"),
    );
}
