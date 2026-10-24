//! Phase 4 spikes: the scxml.rs behavior the bot core relies on,
//! pinned through `scx run` before any ability is written. The documents
//! are `tests/fixtures/phase4/spikes/`; every event they use is a
//! test-only `spike.*` name.
//!
//! * (a) `invoke_tree`: a root invokes `../x/child.scx`, which invokes
//!   `grandchild.scx`, which alone invokes the `generate` service with
//!   `env:` slots. Every literal document is prefetched and listed, each
//!   relative URI resolves against the document that names it, the env
//!   listing reaches the grandchild's `<invoke>`, the service only the
//!   grandchild names is resolved for the run, and leaving the root's
//!   invoking state mid-request cancels the whole tree (exit handlers run
//!   root first) and closes the request's connection.
//! * (c) `replay`: events deferred into the data model and replayed with
//!   `<send target="#_internal">` in `<foreach>` run before an external
//!   event already pending, and lose `_event.invokeid` (the replay carries
//!   it as `from`).
//! * (d) `schema_file`: an invoked child whose transition references an
//!   `event:schema` file starts and handles a schema-valid event (gap G1
//!   closed by scxml.rs `EmbeddedDocument::event_schemas`), and `scx
//!   validate` judges a `<send>` to the child against that file (`EVENT011`).
//! * (e) `reentry`: a run-tagged `<send delay>` guard tells a stale timer
//!   from the current one across self re-entry of a state holding invokes.
//! * (f) `start_error`: the `error.execution` of an invoke whose `src` does
//!   not exist arrives before a `delay="0s"` send armed in the same entry.
//!
//! The ignored `live_invoke_tree_generates_through_the_grandchild` runs
//! spike (a) against a real backend: `FLOPPY_LIVE_ENV_FILE=<paths>
//! cargo test -p floppy-tests --test phase4_spikes -- --ignored`. The files
//! must set `FLOPPY_API_KEY` and `FLOPPY_BASE_URL`; the model is
//! `FLOPPY_LIVE_MODEL`, else `Qwen/Qwen3.8-Flash-Next`. It prints pass or
//! fail only.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use support::{Run, labelled, logs, scratch, service_uri, workspace};

/// The fake key (not a secret): the mock checks it arrives as the bearer.
const FAKE_KEY: &str = "sk-floppy-spike-fake";

/// A spike fixture's path.
fn fixture(path: &str) -> PathBuf {
    workspace().join("tests/fixtures/phase4/spikes").join(path)
}

/// One request the mock received, or the close of a held one.
#[derive(Clone, Debug, PartialEq)]
enum Seen {
    Request {
        target: String,
        authorization: Option<String>,
        body: Json,
    },
    /// The client closed a held (`/hold`) connection.
    Closed,
}

/// A non-streamed Chat Completions answer carrying `content`, as the
/// generate service asks for one by default.
fn completion(content: &str) -> String {
    json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": content},
        }],
    })
    .to_string()
}

/// A deterministic fake backend on 127.0.0.1: `/text/...` streams a Chat
/// Completions reply, and `/hold/...` never answers and records when the
/// client closes the connection.
struct Mock {
    origin: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        Self { origin, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("the log lock").clone()
    }

    /// Waits at most `limit` for `count` entries.
    fn wait_for(&self, count: usize, limit: Duration) -> Vec<Seen> {
        let deadline = Instant::now() + limit;
        loop {
            let seen = self.seen();
            if seen.len() >= count || Instant::now() >= deadline {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn serve(mut stream: std::net::TcpStream, log: &Mutex<Vec<Seen>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let header = |wanted: &str| {
        headers
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.clone())
    };
    let length = header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
    let body: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let answer = if target.starts_with("/text/") {
        Some((
            "application/json",
            completion("Hello from the spike backend!"),
        ))
    } else {
        None
    };
    log.lock().expect("the log lock").push(Seen::Request {
        target: target.clone(),
        authorization: header("authorization"),
        body,
    });
    if target.starts_with("/hold/") {
        // Never answer: wait for the client to close the connection.
        let mut byte = [0_u8; 1];
        while matches!(reader.read(&mut byte), Ok(1..)) {}
        log.lock().expect("the log lock").push(Seen::Closed);
        return;
    }
    let (status, kind, text) = match answer {
        Some((kind, text)) => ("200 OK", kind, text),
        None => ("404 Not Found", "application/json", "{}".to_owned()),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    );
}

/// An env file of exactly `contents`.
fn env_file(test: &str, contents: &str) -> PathBuf {
    let path = scratch("phase4-spikes-env").join(format!("{test}.env"));
    std::fs::write(&path, contents).expect("the env file is writable");
    path
}

/// The arguments running `document` with the env files `env`.
fn args(env: &[PathBuf], document: &Path) -> Vec<String> {
    let mut args: Vec<String> = env
        .iter()
        .flat_map(|path| ["--env-file".to_owned(), path.display().to_string()])
        .collect();
    args.push(document.display().to_string());
    args
}

fn is_log(label: &'static str) -> impl Fn(&Json) -> bool {
    move |record| record["type"] == "log" && record["label"] == label
}

fn is_closed(record: &Json) -> bool {
    record["type"] == "closed"
}

// ---------------------------------------------------------------- spike (a)

fn ask(run: &mut Run, model: &str, prompt: &str) {
    run.event("spike.ask", &json!({"model": model, "prompt": prompt}));
}

#[test]
fn an_invoke_tree_prefetches_resolves_relative_uris_and_a_service_only_the_grandchild_names() {
    let mock = Mock::start();
    let env = env_file(
        "tree-text",
        &format!(
            "FLOPPY_API_KEY={FAKE_KEY}\nFLOPPY_BASE_URL={}/text/v1\n",
            mock.origin
        ),
    );
    let mut run = Run::start(&args(&[env], &fixture("invoke_tree/root/root.scx")), &[]);
    ask(&mut run, "mock-model", "Say hello.");
    let records = run.until(Duration::from_secs(120), is_closed);
    let finished = run.finish(Duration::from_secs(30));
    assert!(support::finished(&records), "{records:?}");
    assert_eq!(
        logs(&records),
        labelled(&[
            ("grandchild.exit", "generating"),
            ("child.exit", "relaying"),
            ("root.exit", "asking"),
            ("reply", "Hello from the spike backend!"),
        ]),
        "{records:?}"
    );
    let listing = &finished.stderr;
    let spikes = fixture("invoke_tree");
    let child = format!(
        "document `file://{}`\n  = for: <invoke src=\"../x/child.scx\">",
        spikes.join("x/child.scx").display()
    );
    let grandchild = format!(
        "document `file://{}`\n  = for: <invoke src=\"grandchild.scx\">",
        spikes.join("x/grandchild.scx").display()
    );
    assert!(
        listing.contains(&child) && listing.contains(&grandchild),
        "both literal documents are prefetched, each relative URI against the document naming it: {listing}"
    );
    let generate = service_uri("generate");
    for (variable, slot) in [("FLOPPY_API_KEY", "apikey"), ("FLOPPY_BASE_URL", "baseurl")] {
        assert!(
            listing.contains(&format!(
                "env `{variable}`\n  = to: <invoke type=\"{generate}\"> env:{slot}"
            )),
            "the env listing reaches the grandchild's invoke: {listing}"
        );
    }
    assert!(
        listing.contains(&format!("service `{generate}`")),
        "the service only the grandchild names is resolved for the run: {listing}"
    );
    assert!(!listing.contains(FAKE_KEY), "names only: {listing}");
    let seen = mock.seen();
    let [
        Seen::Request {
            target,
            authorization,
            body,
        },
    ] = seen.as_slice()
    else {
        panic!("exactly one request: {seen:?}");
    };
    assert_eq!(target, "/text/v1/chat/completions");
    assert_eq!(
        authorization.as_deref(),
        Some(format!("Bearer {FAKE_KEY}").as_str())
    );
    assert_eq!(
        body["model"], "mock-model",
        "the params reach the grandchild"
    );
}

#[test]
fn leaving_the_roots_state_mid_request_cancels_the_tree_and_closes_the_connection() {
    let mock = Mock::start();
    let env = env_file(
        "tree-hold",
        &format!(
            "FLOPPY_API_KEY={FAKE_KEY}\nFLOPPY_BASE_URL={}/hold/v1\n",
            mock.origin
        ),
    );
    let mut run = Run::start(&args(&[env], &fixture("invoke_tree/root/root.scx")), &[]);
    ask(&mut run, "mock-model", "Say hello.");
    let seen = mock.wait_for(1, Duration::from_secs(120));
    assert!(
        matches!(seen.as_slice(), [Seen::Request { target, .. }] if target == "/hold/v1/chat/completions"),
        "the grandchild's request is in flight: {seen:?}"
    );
    run.event("spike.leave", &Json::Null);
    let records = run.until(Duration::from_secs(60), is_closed);
    assert!(support::finished(&records), "{records:?}");
    assert_eq!(
        logs(&records),
        labelled(&[
            ("root.exit", "asking"),
            ("child.exit", "relaying"),
            ("grandchild.exit", "generating"),
            ("final", "left"),
        ]),
        "the cancel cascades root first, and every exit handler runs: {records:?}"
    );
    let seen = mock.wait_for(2, Duration::from_secs(30));
    assert_eq!(
        seen.last(),
        Some(&Seen::Closed),
        "cancelling the grandchild's invocation closes its connection: {seen:?}"
    );
    drop(run);
}

/// Spike (a) against a real backend through `FLOPPY_LIVE_ENV_FILE`: the
/// grandchild's generate call answers and the reply reaches the root.
#[test]
#[ignore = "makes one real model call; needs FLOPPY_LIVE_ENV_FILE"]
fn live_invoke_tree_generates_through_the_grandchild() {
    let model = support::live_model();
    let mut run = Run::start(
        &args(
            &support::live_env_files(),
            &fixture("invoke_tree/root/root.scx"),
        ),
        &[],
    );
    ask(&mut run, &model, "Say hello.");
    let records = run.until(Duration::from_secs(300), is_closed);
    let pass = support::finished(&records)
        && logs(&records)
            .iter()
            .any(|(label, text)| label == "reply" && !text.trim().is_empty());
    println!("live invoke tree: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live invoke tree did not pass");
}

// ---------------------------------------------------------------- spike (b)

#[test]
fn replayed_internal_events_run_before_a_pending_external_one_and_lose_their_invokeid() {
    let mut run = Run::start(&args(&[], &fixture("replay/root.scx")), &[]);
    let mut records = run.until(Duration::from_secs(60), |record| {
        is_log("deferred")(record) && record["text"] == "w2"
    });
    run.event("spike.release", &Json::Null);
    records.extend(run.until(Duration::from_secs(60), |record| {
        is_log("handled")(record)
            && record["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("ext"))
    }));
    run.event("spike.finish", &Json::Null);
    records.extend(run.until(Duration::from_secs(60), is_closed));
    assert!(support::finished(&records), "{records:?}");
    assert_eq!(
        logs(&records),
        labelled(&[
            ("deferred", "w1"),
            ("deferred", "w2"),
            ("handled", "w1 type=internal invokeid=undefined from=worker"),
            ("handled", "w2 type=internal invokeid=undefined from=worker"),
            (
                "handled",
                "ext type=external invokeid=undefined from=undefined"
            ),
        ]),
        "{records:?}"
    );
}

// ---------------------------------------------------------------- spike (d)

#[test]
fn an_invoked_child_referencing_an_event_schema_file_starts_with_it() {
    let child = fixture("schema_file/child.scx");
    let validated = support::output(&["validate".to_owned(), child.display().to_string()]);
    assert!(
        validated.status.success() && validated.stderr.contains("0 error(s), 0 warning(s)"),
        "the child is valid on its own, schema file included: {}",
        validated.stderr
    );
    let root = fixture("schema_file/root.scx");
    let validated = support::output(&["validate".to_owned(), root.display().to_string()]);
    assert!(
        validated.status.success() && validated.stderr.contains("0 error(s), 0 warning(s)"),
        "the root's ping is valid against the child's schema file: {}",
        validated.stderr
    );
    let mut run = Run::start(&args(&[], &root), &[]);
    run.end_input();
    let finished = run.finish(Duration::from_secs(60));
    assert_eq!(
        logs(&finished.records),
        labelled(&[("ready", "child"), ("done", "child")]),
        "the child compiles with its schema file, starts, and takes the ping: {:?}",
        finished.records
    );
}

/// `scx validate` reads the literal child with its own schema file and
/// warns `EVENT011` for a `<send>` whose data that file rejects.
#[test]
fn a_send_the_childs_schema_file_rejects_is_event011() {
    let root = fixture("schema_file/root_rejected_ping.scx");
    let validated = support::output(&["validate".to_owned(), root.display().to_string()]);
    assert!(
        validated.status.success()
            && validated.stderr.contains("warning[EVENT011]")
            && validated.stderr.contains("0 error(s), 1 warning(s)"),
        "the rejected ping is EVENT011: {}",
        validated.stderr
    );
}

// ---------------------------------------------------------------- spike (e)

#[test]
fn a_run_tagged_timer_guard_survives_self_reentry_of_a_state_holding_invokes() {
    // The wall clock: the logical clock would fire the first timer before
    // the reboot arrives. Each timer is armed 30 s before it fires, when
    // its member announces itself, and the reboot is sent as soon as the
    // first announcement is read, so a loaded machine still takes the
    // reboot first (10 s was not enough under a full parallel test run).
    let mut run = Run::start(&args(&[], &fixture("reentry/root.scx")), &[]);
    let mut records = run.until(Duration::from_secs(60), is_log("member"));
    run.event("spike.reboot", &Json::Null);
    records.extend(run.until(Duration::from_secs(60), is_closed));
    assert!(support::finished(&records), "{records:?}");
    let mut labels: Vec<(String, String)> = logs(&records);
    // The second member's start (its compile) and the stale first timer
    // race: either may come first, and only the stale timer's place before
    // the second timer is the spike's claim.
    if labels.get(4..6) == Some(&labelled(&[("stale", "1"), ("member", "up")])[..]) {
        labels.swap(4, 5);
    }
    assert_eq!(
        labels,
        labelled(&[
            ("entered", "1"),
            ("member", "up"),
            ("member.exit", "up"),
            ("entered", "2"),
            ("member", "up"),
            ("stale", "1"),
            ("member.exit", "up"),
            ("timer", "2"),
        ]),
        "{records:?}"
    );
    let members: Vec<&str> = support::of_type(&records, "log")
        .into_iter()
        .filter(|record| record["label"] == "member.exit")
        .filter_map(|record| record["session"].as_str())
        .collect();
    // A child's `_sessionid` is `<parent _sessionid>.<invokeid>`: the new
    // member, invoked again under the same invoke id, has the first one's
    // address (the `entered` labels above tell the two apart).
    assert_eq!(
        members,
        ["session.member", "session.member"],
        "re-entry cancels the first member and invokes a new one at the same address"
    );
}

// ---------------------------------------------------------------- spike (f)

fn start_error(clock: &str) {
    let mut full = vec!["--clock".to_owned(), clock.to_owned()];
    full.extend(args(&[], &fixture("start_error/root.scx")));
    let mut run = Run::start(&full, &[]);
    let mut records = run.until(Duration::from_secs(60), is_log("tick"));
    run.event("spike.finish", &Json::Null);
    records.extend(run.until(Duration::from_secs(60), is_closed));
    let finished = run.finish(Duration::from_secs(30));
    assert!(support::finished(&records), "{records:?}");
    assert_eq!(
        logs(&records),
        labelled(&[
            ("error", "error.execution invokeid=undefined"),
            ("tick", "0s"),
        ]),
        "{clock} clock: {records:?}"
    );
    assert!(
        finished
            .stderr
            .contains("= for: <invoke src=\"missing.scx\">")
            && finished.stderr.contains("= unavailable: cannot read"),
        "the missing document is listed as unavailable: {}",
        finished.stderr
    );
}

#[test]
fn an_invoke_start_error_arrives_before_a_zero_delay_send_on_both_clocks() {
    start_error("wall");
    start_error("logical");
}
