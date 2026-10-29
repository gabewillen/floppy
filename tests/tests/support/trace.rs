//! Pinned event traces: what a phase 4 test's bot did, saved under
//! `tests/fixtures/phase4/traces/<test>.ndjson` and compared byte for byte,
//! so a restructure that must not change the bot's events proves it.
//!
//! A trace is NDJSON, one line per item, in order:
//!
//! * every `log`, `done` and `error` record of the run (every session's,
//!   with its `session` and `parent`), as `scx run` wrote it, grouped by
//!   session (by id; each session's own records in order);
//! * then every request a fake backend received, as
//!   `{"type":"request","target":…,"body":…}` (the choices, strict
//!   schemas and prompts the services sent), sorted by their text.
//!
//! Values that differ per run are normalized: the test's scratch directory
//! (`target/test-scratch/<pid>-<name>-<n>`) becomes `<scratch>`, the
//! workspace root `<workspace>`, a UTC timestamp
//! (`YYYY-MM-DDTHH:MM:SSZ`) `<timestamp>`, and a UTC time of day
//! (`HH:MM:SSZ`) `<time>`; a voice ref the interpretation minted
//! (`voice:` and 32 hex digits) is `voice:<n>`, numbered as the trace first
//! names each. Each record is written with its keys
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

/// `text` with each whole-second UTC timestamp
/// (`YYYY-MM-DDTHH:MM:SSZ`, the rows' `last_used_at` and the like, which
/// the wall clock sets) replaced by `<timestamp>`, then each UTC time of day
/// (`HH:MM:SSZ`, a recent turn's `at`) by `<time>`.
fn timestamps_normalized(text: &str) -> String {
    shapes_normalized(
        &shapes_normalized(text, b"dddd-dd-ddTdd:dd:ddZ", "<timestamp>"),
        b"dd:dd:ddZ",
        "<time>",
    )
}

/// `text` with each run of `shape` (`d` a digit, anything else itself)
/// replaced by `replacement`.
fn shapes_normalized(text: &str, shape: &[u8], replacement: &str) -> String {
    let bytes = text.as_bytes();
    let matches_at = |at: usize| {
        bytes.get(at..at + shape.len()).is_some_and(|window| {
            window
                .iter()
                .zip(shape)
                .all(|(byte, expected)| match expected {
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
            out.push_str(replacement);
            at += shape.len();
            copied = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// The trace of `records` (a run's records, in order) and `requests`
/// (`(target, body)` of each backend request, in order): the records
/// grouped by session (sessions by id, each session's own records in
/// order), then the requests sorted by their text. Sessions run side by
/// side (a deliberative tier takes a turn as the decision layer does), so
/// only each session's own order, and which requests were made, are
/// stable from run to run.
pub fn render(records: &[Json], requests: &[(String, Json)]) -> String {
    let mut records: Vec<(String, String)> = records
        .iter()
        .filter(|record| record["type"].as_str().is_some_and(|t| KEPT.contains(&t)))
        .map(|record| {
            let session = record["session"].as_str().unwrap_or_default().to_owned();
            (session, normalized(&record.to_string()))
        })
        .collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let mut requests: Vec<String> = requests
        .iter()
        .map(|(target, body)| {
            normalized(&json!({"type": "request", "target": target, "body": body}).to_string())
        })
        .collect();
    requests.sort();
    let mut text = String::new();
    let mut voices = Vec::new();
    for line in records.into_iter().map(|(_, line)| line).chain(requests) {
        text.push_str(&voices_normalized(&line, &mut voices));
        text.push('\n');
    }
    text
}

/// `text` with each voice ref the interpretation's registry minted
/// (`voice:` and 32 lowercase hex digits, which the clock begins) replaced
/// by `voice:<n>`, `n` its place among the trace's voices (`voices`, in the
/// order each first appears), from 1.
fn voices_normalized(text: &str, voices: &mut Vec<String>) -> String {
    const PREFIX: &str = "voice:";
    const DIGITS: usize = 32;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(PREFIX) {
        let after = &rest[at + PREFIX.len()..];
        let id = after.get(..DIGITS).filter(|id| {
            id.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && !after[DIGITS..].starts_with(|c: char| c.is_ascii_alphanumeric())
        });
        out.push_str(&rest[..at + PREFIX.len()]);
        if let Some(id) = id {
            let n = voices
                .iter()
                .position(|known| known == id)
                .unwrap_or_else(|| {
                    voices.push(id.to_owned());
                    voices.len() - 1
                });
            out.push('<');
            out.push_str(&(n + 1).to_string());
            out.push('>');
            rest = &after[DIGITS..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
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
