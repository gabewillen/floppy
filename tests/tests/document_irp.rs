//! The implementation report plans of floppy's bot and ability documents
//! (`bots/<name>/irp`, `abilities/<name>/irp`): the W3C IRP's manifest
//! format and `Outcome` oracle over each document's numbered statements,
//! run through `scx run` with the real invoked services.
//!
//! scxml.rs's conformance runner (`scx conformance run`, which runs the
//! namespace suites, `tests/tests/namespace_irp.rs`) binds extension
//! namespaces but no invoked service, and nearly every floppy document
//! invokes one (memory's sql, generate, decision, speech). So a document
//! suite keeps the IRP's shape and runs here instead:
//!
//! * `irp/README.md` holds the document's numbered statements
//!   (`<a id="<name>-<n>">`, under "Statements"), and `irp/manifest.xml`
//!   the W3C `assertions`/`assert`/`test`/`start` manifest, one assertion
//!   per statement, every test mandatory and unattended;
//! * `irp/sources/<id>/test<id>.scxml` is a test document, already in
//!   ECMAScript (floppy's only data model, so there is nothing to
//!   specialize): it invokes the document under test by its literal
//!   relative path, drives it, and ends in its final state `pass`, whose
//!   entry logs `Outcome` `pass`, or `fail`, as a prepared W3C case does.
//!   The `.scxml` extension keeps test documents out of the checks every
//!   floppy `.scx` document is held to (`contracts.rs`, `defaults.rs`);
//! * `irp/sources/<id>/backend.json` (optional) scripts the fake HTTP
//!   backend of the services the run reaches: `{"clock": "wall" |
//!   "logical", "answers": [{"path": SUFFIX, "status": N, "body": JSON},
//!   ...]}`. A request takes the first unused answer whose `path` ends its
//!   target (the last one for a path repeats); a request no answer matches
//!   is answered 404; a 200 completion answers a streamed request
//!   (`"stream": true`) as its server-sent-event stream
//!   ([`support::stream_if_asked`]). Every service's base URL points at it;
//! * `irp/sources/<id>/media/` (optional) is the media the generate and
//!   speech services find at `/media` (a fresh copy per case; an empty
//!   directory without it).
//!
//! A case passes when `scx run` ends with the root finished (exit 0) and
//! the run logged `Outcome` `pass`. The run grants each service the
//! document tree binds a fresh scratch directory, as the integration tests
//! do. `FLOPPY_IRP_SUITE=<name>` runs only that document's suite.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value as Json;
use support::{Run, binds, logs, unique_scratch, workspace};

/// The longest one case may run.
const LIMIT: Duration = Duration::from_secs(180);
/// The fake keys (not secrets).
const FAKE_KEYS: &str = "FLOPPY_API_KEY=floppy-fake\nTYPESAFE_API_KEY=ts-floppy-fake\nFLOPPY_SPEECH_API_KEY=floppy-fake\nFLOPPY_VOICE_API_KEY=floppy-fake\n";

/// Every document a suite can test: `bots/<name>/<name>.scx` and
/// `abilities/<name>/<name>.scx`, as `(kind directory, name)`.
fn documents() -> Vec<(&'static str, String)> {
    let mut found = Vec::new();
    for kind in ["bots", "abilities"] {
        let mut names: Vec<String> = std::fs::read_dir(workspace().join(kind))
            .expect("the directory is listable")
            .filter_map(|entry| {
                let path = entry.expect("the entry is readable").path();
                let name = path.file_name()?.to_string_lossy().into_owned();
                path.join(format!("{name}.scx")).is_file().then_some(name)
            })
            .collect();
        names.sort();
        found.extend(names.into_iter().map(|name| (kind, name)));
    }
    found
}

/// The manifest's `(assertion id, [test id])` in document order.
fn assertions(suite: &Path) -> Vec<(String, Vec<String>)> {
    let text =
        std::fs::read_to_string(suite.join("manifest.xml")).expect("the manifest is readable");
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("<assert id=\"") {
            let id = rest.split('"').next().expect("an assertion id");
            found.push((id.to_owned(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("<test id=\"") {
            let id = rest.split('"').next().expect("a test id");
            found
                .last_mut()
                .expect("a test sits in an assertion")
                .1
                .push(id.to_owned());
        }
    }
    found
}

/// The statement ids under the suite README's "Statements".
fn statements(suite: &Path, name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(suite.join("README.md")).expect("the README is readable");
    let prefix = format!("<a id=\"{name}-");
    text.match_indices(&prefix)
        .map(|(start, _)| {
            let rest = &text[start + "<a id=\"".len()..];
            rest.split('"').next().expect("a statement id").to_owned()
        })
        .collect()
}

/// One scripted answer of the fake backend.
#[derive(Clone, Debug)]
struct Answer {
    path: String,
    status: u16,
    body: String,
}

/// The fake backend of every service a case reaches.
struct Backend {
    origin: String,
}

impl Backend {
    fn start(answers: Vec<Answer>) -> Self {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("the fake backend binds a loopback port");
        let origin = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("the fake backend has an address")
        );
        let state = Arc::new(Mutex::new((answers, BTreeMap::<usize, bool>::new())));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&state);
                std::thread::spawn(move || serve(stream, &state));
            }
        });
        Self { origin }
    }
}

type Script = Mutex<(Vec<Answer>, BTreeMap<usize, bool>)>;

fn serve(mut stream: TcpStream, state: &Arc<Script>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let request: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let (status, text) = {
        let mut guard = state.lock().expect("the script lock");
        let (answers, used) = &mut *guard;
        let matching: Vec<usize> = (0..answers.len())
            .filter(|index| target.ends_with(&answers[*index].path))
            .collect();
        let chosen = matching
            .iter()
            .copied()
            .find(|index| !used.contains_key(index))
            .or_else(|| matching.last().copied());
        chosen.map_or(
            (
                404,
                r#"{"error":{"message":"no scripted answer"}}"#.to_owned(),
            ),
            |index| {
                used.insert(index, true);
                (answers[index].status, answers[index].body.clone())
            },
        )
    };
    // A streamed request gets its scripted completion as a stream.
    let text = if status == 200 {
        support::stream_if_asked(&request, text)
    } else {
        text
    };
    let _written = write!(
        stream,
        "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        if status == 200 { "OK" } else { "Scripted" },
        text.len()
    );
}

/// The case's backend script, or none.
fn script(case: &Path) -> (bool, Vec<Answer>) {
    let path = case.join("backend.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (false, Vec::new());
    };
    let script: Json = serde_json::from_str(&text).expect("backend.json is JSON");
    let wall = script["clock"] == "wall";
    let answers = script["answers"]
        .as_array()
        .map(|answers| {
            answers
                .iter()
                .map(|answer| Answer {
                    path: answer["path"]
                        .as_str()
                        .expect("an answer's path")
                        .to_owned(),
                    status: answer["status"]
                        .as_u64()
                        .and_then(|status| u16::try_from(status).ok())
                        .unwrap_or(200),
                    body: match &answer["body"] {
                        Json::String(text) => text.clone(),
                        other => other.to_string(),
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    (wall, answers)
}

/// Runs one case; returns why it did not pass, if it did not.
fn run_case(document: &Path, case: &Path, test: &str) -> Option<String> {
    let start = case.join(format!("test{test}.scxml"));
    if !start.is_file() {
        return Some(format!("{} is missing", start.display()));
    }
    let (wall, answers) = script(case);
    let backend = Backend::start(answers);
    let env = unique_scratch("irp-env").join("fake.env");
    std::fs::write(
        &env,
        format!(
            "{FAKE_KEYS}FLOPPY_BASE_URL={origin}/v1\nTYPESAFE_BASE_URL={origin}\nFLOPPY_SPEECH_BASE_URL={origin}/v1\nFLOPPY_VOICE_BASE_URL={origin}/v1\n",
            origin = backend.origin
        ),
    )
    .expect("the env file is writable");
    let mut args = vec!["--env-file".to_owned(), env.display().to_string()];
    if binds(document, "sql") {
        args.extend(support::memory_grant());
    }
    if binds(document, "pgsql") {
        args.extend(support::pgsql_grant());
    }
    // The generate and speech services read and write media at /media: a
    // fresh copy of the case's media/ directory, else an empty directory.
    for name in ["generate", "speech"] {
        if binds(document, name) {
            let media = unique_scratch("irp-media");
            if case.join("media").is_dir() {
                support::copy_tree(&case.join("media"), &media);
            }
            args.extend([
                "--dir".to_owned(),
                format!("{}={}::/media", support::service_uri(name), media.display()),
            ]);
        }
    }
    args.extend([
        "--clock".to_owned(),
        if wall { "wall" } else { "logical" }.to_owned(),
        start
            .strip_prefix(workspace())
            .unwrap_or(&start)
            .display()
            .to_string(),
    ]);
    let run = Run::start(&args, &[]);
    let finished = run.finish(LIMIT);
    let outcome: Vec<String> = logs(&finished.all)
        .into_iter()
        .filter(|(label, _)| label == "Outcome")
        .map(|(_, text)| text)
        .collect();
    (!(finished.status.success() && outcome == ["pass"])).then(|| {
        format!(
            "exit {:?}, Outcome {outcome:?}\nrecords: {}\nstderr: {}",
            finished.status.code(),
            finished
                .all
                .iter()
                .map(Json::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            finished.stderr
        )
    })
}

fn selected(name: &str) -> bool {
    std::env::var("FLOPPY_IRP_SUITE").map_or(true, |only| only == name)
}

#[test]
fn every_statement_is_an_assertion_with_tests() {
    for (kind, name) in documents() {
        let suite = workspace().join(kind).join(&name).join("irp");
        if !suite.join("manifest.xml").is_file() || !selected(&name) {
            continue;
        }
        let assertions = assertions(&suite);
        let asserted: Vec<&str> = assertions.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            asserted,
            statements(&suite, &name),
            "{name}: the manifest's assertions are the README's statements, in order"
        );
        for (id, tests) in &assertions {
            assert!(!tests.is_empty(), "{name}: {id} has a test");
        }
    }
}

#[test]
fn every_document_suite_passes() {
    let mut jobs = Vec::new();
    for (kind, name) in documents() {
        let dir = workspace().join(kind).join(&name);
        let suite = dir.join("irp");
        if !suite.join("manifest.xml").is_file() || !selected(&name) {
            continue;
        }
        let document = dir.join(format!("{name}.scx"));
        for (assertion, tests) in assertions(&suite) {
            for test in tests {
                let case = suite.join("sources").join(&test);
                jobs.push((
                    name.clone(),
                    assertion.clone(),
                    test,
                    document.clone(),
                    case,
                ));
            }
        }
    }
    // The components and scx are built once, before the cases share them.
    let _cli = support::cli();
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(2, |count| count.get().clamp(1, 8));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some((name, assertion, test, document, case)) =
                    jobs.get(next.fetch_add(1, Ordering::Relaxed))
                {
                    if let Some(why) = run_case(document, case, test) {
                        failures
                            .lock()
                            .expect("the failures lock")
                            .push(format!("{name} {test} ({assertion}): {why}"));
                    }
                }
            });
        }
    });
    let mut failures = failures.into_inner().expect("the failures lock");
    failures.sort();
    eprintln!("ran {} document IRP cases", jobs.len());
    assert!(
        failures.is_empty(),
        "{} of {} cases did not pass:\n\n{}",
        failures.len(),
        jobs.len(),
        failures.join("\n\n")
    );
}

#[test]
fn every_document_has_a_suite() {
    let missing: Vec<String> = documents()
        .into_iter()
        .filter(|(kind, name)| {
            !workspace()
                .join(kind)
                .join(name)
                .join("irp/manifest.xml")
                .is_file()
        })
        .map(|(_, name)| name)
        .collect();
    assert!(
        missing.is_empty(),
        "the documents without a suite: {missing:?}"
    );
}
