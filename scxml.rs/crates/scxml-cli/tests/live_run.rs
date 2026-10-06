#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `scxml-cli run` end to end through the shipped components (`wasmtime`
//! feature): the binary is spawned, newline-delimited JSON goes to its
//! standard input, and its standard-output records and exit status are
//! checked, including end of input, real delayed sends, record and replay
//! (of the `BasicHTTP` network too), and (on Unix) termination signals.
//!
//! The release components are built and verified once by
//! `scripts/verify-wasm-capabilities.sh` into `target/cli-components`; set
//! `SCXML_COMPONENTS_DIR` and `SCXML_COMPONENTS_MANIFEST_SHA256` to use an
//! already verified release instead.

use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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
        let dir = target.join("wasm-artifacts/live-run");
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

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-live-run")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn document(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("doc.scxml");
    fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0">{body}</scxml>"#
        ),
    )
    .expect("the document is writable");
    path
}

fn command(document: &Path, extra: &[&str]) -> Command {
    let (dir, digest) = release();
    let mut command = Command::new(env!("CARGO_BIN_EXE_scxml-cli"));
    command
        .arg("run")
        .arg(document)
        .args(["--components", dir, "--components-manifest-sha256", digest])
        .args(extra)
        .env("NO_COLOR", "1");
    command
}

/// Runs to completion with `input` on standard input.
fn run(document: &Path, extra: &[&str], input: &str) -> Output {
    let mut child = command(document, extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scxml-cli must start");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("input is writable");
    child.wait_with_output().expect("scxml-cli must finish")
}

fn parse_records(output: &Output) -> Vec<serde_json::Value> {
    String::from_utf8(output.stdout.clone())
        .expect("stdout is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{line}: {error}")))
        .collect()
}

fn kinds(records: &[serde_json::Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            record["type"]
                .as_str()
                .expect("every record has a type")
                .to_owned()
        })
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const TWO_STEPS: &str = r#"<datamodel><data id="seen" expr="null"/></datamodel><state id="s0"><transition event="go" target="s1"><assign location="seen" expr="_event.data"/><log label="go" expr="JSON.stringify(_event.data)"/></transition></state><state id="s1"><transition event="stop" target="done"/></state><final id="done"><donedata><content expr="seen"/></donedata></final>"#;

#[test]
fn events_stream_records_and_a_final_state_exits_zero() {
    let directory = scratch("events");
    let doc = document(&directory, TWO_STEPS);
    let output = run(
        &doc,
        &["--clock", "logical"],
        "{\"name\":\"go\",\"data\":{\"x\":1}}\nnot json\n{\"name\":\"stop\"}\n",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(
        kinds(&records),
        [
            "opened", "call", "log", "call", "error", "done", "call", "closed"
        ]
    );
    assert_eq!(records[0]["session"], "session");
    assert_eq!(
        records[0]["ioprocessors"]["http://www.w3.org/TR/scxml/#SCXMLEventProcessor"]["location"],
        "#_scxml_session"
    );
    assert_eq!(records[1]["cause"], "start");
    assert_eq!(records[1]["configuration"], serde_json::json!(["s0"]));
    assert_eq!(records[2]["label"], "go");
    assert_eq!(records[2]["text"], "{\"x\":1}");
    assert_eq!(records[3]["configuration"], serde_json::json!(["s1"]));
    assert_eq!(records[4]["code"], "input-invalid");
    assert_eq!(records[4]["fatal"], false);
    assert_eq!(records[4]["line"], 2);
    // SCXML 5.5: the root's <donedata> is returned to the environment.
    assert_eq!(records[5]["data"], serde_json::json!({ "x": 1 }));
    assert_eq!(records[6]["status"], "finished");
    assert_eq!(records[7]["reason"], "done");
    assert!(
        stderr(&output).contains("warning[input-invalid]: stdin line 2:"),
        "{}",
        stderr(&output)
    );
}

/// Resource limits apply to the Wasm components only: `run` reads an input
/// line of any length. A line over 1 MiB (JSON whitespace around a small
/// event) is one event like any other.
#[test]
fn an_input_line_over_one_mib_is_accepted() {
    let directory = scratch("long-line");
    let doc = document(&directory, TWO_STEPS);
    let padding = " ".repeat(1024 * 1024 + 1);
    let input = format!("{{\"name\":\"go\",{padding}\"data\":1}}\n{{\"name\":\"stop\"}}\n");
    let output = run(&doc, &["--clock", "logical"], &input);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(
        kinds(&records),
        ["opened", "call", "log", "call", "done", "call", "closed"]
    );
    assert_eq!(records[2]["text"], "1");
}

const DELAYED: &str = r#"<state id="s0"><onentry><send event="tick" delay="60ms"/></onentry><transition event="tick" target="done"/></state><final id="done"/>"#;

#[test]
fn end_of_input_waits_for_a_real_delayed_send_or_closes() {
    let directory = scratch("eof");
    let doc = document(&directory, DELAYED);
    let started = Instant::now();
    let output = run(&doc, &[], "");
    let elapsed = started.elapsed();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(
        kinds(&records),
        ["opened", "call", "done", "call", "closed"]
    );
    assert_eq!(records[1]["next_deadline_ms"], 60);
    assert_eq!(records[3]["cause"], "timer");
    assert!(
        records[3]["at_ms"]
            .as_u64()
            .is_some_and(|at| (60..30_000).contains(&at))
    );
    assert!(elapsed >= Duration::from_millis(60), "{elapsed:?}");

    // A deadline far enough out that end of input always arrives first.
    let far = scratch("eof-close");
    let doc = document(&far, &DELAYED.replace("60ms", "20000ms"));
    let output = run(&doc, &["--on-eof", "close"], "");
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(kinds(&records), ["opened", "call", "closed"]);
    assert_eq!(records[2]["reason"], "eof");
    assert_eq!(records[2]["status"], "running");
    assert_eq!(records[2]["cancelled_sends"], 1);
}

#[test]
fn a_failing_step_fails_the_session_with_exit_one() {
    let directory = scratch("failure");
    let doc = document(
        &directory,
        r#"<state id="s0"><transition event="spin" target="s0"><raise event="spin"/></transition></state>"#,
    );
    let output = run(
        &doc,
        &["--clock", "logical", "--max-commands", "4"],
        "{\"name\":\"spin\"}\n{\"name\":\"spin\"}\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let records = parse_records(&output);
    let error = records
        .iter()
        .find(|record| record["type"] == "error")
        .expect("an error record");
    assert_eq!(error["fatal"], true);
    let last = records.last().expect("records");
    assert_eq!(last["type"], "closed");
    assert_eq!(last["reason"], "failed");
    assert!(stderr(&output).contains("error["), "{}", stderr(&output));
}

#[test]
fn a_recorded_run_replays_its_records_and_snapshots_byte_for_byte() {
    let directory = scratch("replay");
    let doc = document(
        &directory,
        r#"<datamodel><data id="n" expr="0"/></datamodel><state id="s0"><onentry><send event="tick" delay="40ms"/></onentry><transition event="ping"><assign location="n" expr="n + 1"/><log label="ping" expr="n"/></transition><transition event="tick" target="s1"/></state><state id="s1"><transition event="stop" target="done"/></state><final id="done"/>"#,
    );
    let log = directory.join("session.ndjson");
    let log_arg = log.display().to_string();
    let mut child = command(&doc, &["--snapshot-on", "call", "--record", &log_arg])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scxml-cli must start");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(b"{\"name\":\"ping\"}\n{\"type\":\"snapshot\"}\n")
        .expect("input is writable");
    // Let the delayed send fire on the wall clock before the last event.
    std::thread::sleep(Duration::from_millis(250));
    stdin
        .write_all(b"{\"name\":\"stop\"}\n")
        .expect("input is writable");
    drop(stdin);
    let live = child.wait_with_output().expect("scxml-cli must finish");
    assert_eq!(live.status.code(), Some(0), "{}", stderr(&live));
    let live_records = parse_records(&live);
    assert!(
        live_records
            .iter()
            .any(|record| record["type"] == "call" && record["cause"] == "timer"),
        "the delayed send fired live: {live_records:?}"
    );

    let replay = run(&doc, &["--replay", &log_arg], "");
    assert_eq!(replay.status.code(), Some(0), "{}", stderr(&replay));
    let replay_records = parse_records(&replay);
    // Everything after `opened` (whose clock differs) is identical.
    assert_eq!(live_records[1..], replay_records[1..]);
    let snapshots = |output: &Output| -> Vec<String> {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.starts_with("{\"type\":\"snapshot\""))
            .map(str::to_owned)
            .collect()
    };
    let live_snapshots = snapshots(&live);
    assert!(live_snapshots.len() >= 4, "{live_snapshots:?}");
    assert_eq!(
        live_snapshots,
        snapshots(&replay),
        "snapshot bytes replay exactly"
    );

    // A different document is refused.
    let other = scratch("replay-other");
    let other_doc = document(&other, DELAYED);
    let refused = run(&other_doc, &["--replay", &log_arg], "");
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        stderr(&refused).contains("[replay-identity-mismatch]"),
        "{}",
        stderr(&refused)
    );
}

#[test]
fn usage_errors_exit_two_before_anything_runs() {
    let directory = scratch("usage");
    let doc = document(&directory, DELAYED);
    let output = Command::new(env!("CARGO_BIN_EXE_scxml-cli"))
        .arg("run")
        .arg(&doc)
        .output()
        .expect("scxml-cli must run");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("run requires --components DIR"),
        "{}",
        stderr(&output)
    );
    assert!(output.stdout.is_empty());
    let output = run(&doc, &["--clock", "sundial"], "");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--clock requires wall or logical"));
}

#[cfg(unix)]
#[test]
fn sigterm_closes_the_session_gracefully() {
    let directory = scratch("signal");
    let doc = document(
        &directory,
        r#"<state id="s0"><onentry><send event="later" delay="60000ms"/></onentry><onexit><log label="exit"/></onexit></state>"#,
    );
    let mut child = command(&doc, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scxml-cli must start");
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    // Wait until the session is open and its start step is reported.
    for _ in 0..2 {
        lines
            .next()
            .expect("a record")
            .expect("the record is readable");
    }
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success());
    let rest: Vec<serde_json::Value> = lines
        .map(|line| serde_json::from_str(&line.expect("readable")).expect("JSON"))
        .collect();
    let status = child.wait().expect("scxml-cli exits");
    assert_eq!(status.code(), Some(143));
    assert_eq!(kinds(&rest), ["log", "closed"]);
    assert_eq!(rest[0]["label"], "exit");
    assert_eq!(rest[1]["reason"], "signal");
    assert_eq!(rest[1]["cancelled_sends"], 1);
    // stdin stays open the whole time: the signal, not end of input, closed it.
    drop(child.stdin.take());
}

const BASICHTTP: &str = "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor";

/// A free loopback port (bound, read, and released).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

/// Starts `run` with no standard input (a serving session stays open).
fn serve(document: &Path, extra: &[&str]) -> std::process::Child {
    command(document, extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scxml-cli must start")
}

/// Waits for `child` to exit, killing it after `limit`.
fn finish(mut child: std::process::Child, limit: Duration) -> Output {
    let started = Instant::now();
    while child.try_wait().expect("the child can be polled").is_none() {
        if started.elapsed() > limit {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().expect("scxml-cli must finish")
}

#[test]
fn two_run_processes_exchange_events_over_http() {
    let port_b = free_port();
    let directory_a = scratch("http-a");
    // A answers every ping with a pong to the reply address the ping carries.
    let doc_a = document(
        &directory_a,
        &format!(
            r#"<state id="s0"><transition event="ping" target="done"><log label="ping" expr="_event.data.from"/><send event="pong" type="{BASICHTTP}" targetexpr="_event.data.reply"><param name="n" expr="7"/></send></transition><transition event="error.communication" target="lost"/></state><final id="done"/><final id="lost"/>"#
        ),
    );
    let mut a = serve(
        &doc_a,
        &[
            "--http-listen",
            "0",
            "--http-allow",
            &format!("http://127.0.0.1:{port_b}"),
        ],
    );
    let stdout_a = a.stdout.take().expect("stdout is piped");
    let mut lines_a = BufReader::new(stdout_a).lines();
    let opened: serde_json::Value = serde_json::from_str(
        &lines_a
            .next()
            .expect("A opens")
            .expect("the record is readable"),
    )
    .expect("JSON");
    assert_eq!(opened["type"], "opened");
    let location_a = opened["ioprocessors"][BASICHTTP]["location"]
        .as_str()
        .expect("A advertises its BasicHTTP location")
        .to_owned();
    assert!(
        location_a.starts_with("http://127.0.0.1:")
            && location_a.ends_with("/sessions/session/basichttp"),
        "{location_a}"
    );
    assert_eq!(opened["ioprocessors"]["basichttp"]["location"], location_a);

    // B pings A at start, with its own location as the reply address.
    let directory_b = scratch("http-b");
    let doc_b = document(
        &directory_b,
        &format!(
            r#"<state id="s0"><onentry><send event="ping" type="{BASICHTTP}" target="{location_a}"><param name="from" expr="'B'"/><param name="reply" expr="_ioprocessors['basichttp'].location"/></send></onentry><transition event="pong" target="done"><log label="pong" expr="_event.data.n + ' ' + _event.origintype"/></transition><transition event="error.communication" target="lost"/></state><final id="done"/><final id="lost"/>"#
        ),
    );
    let origin_a = location_a
        .split("/sessions/")
        .next()
        .expect("A's origin")
        .to_owned();
    let b = serve(
        &doc_b,
        &[
            "--http-listen",
            &format!("127.0.0.1:{port_b}"),
            "--http-allow",
            &origin_a,
            "--session-id",
            "b",
        ],
    );
    let output_b = finish(b, Duration::from_secs(60));
    let rest_a: Vec<serde_json::Value> = lines_a
        .map(|line| serde_json::from_str(&line.expect("readable")).expect("JSON"))
        .collect();
    let output_a = finish(a, Duration::from_secs(60));
    assert_eq!(output_a.status.code(), Some(0), "{}", stderr(&output_a));
    assert_eq!(output_b.status.code(), Some(0), "{}", stderr(&output_b));
    assert_eq!(
        kinds(&rest_a),
        ["call", "log", "done", "call", "closed"],
        "{rest_a:?}"
    );
    assert_eq!(rest_a[1]["label"], "ping");
    assert_eq!(rest_a[1]["text"], "B");
    assert_eq!(rest_a[3]["cause"], "http");
    assert_eq!(rest_a[3]["configuration"], serde_json::json!(["done"]));
    let records_b = parse_records(&output_b);
    assert_eq!(
        records_b[0]["ioprocessors"][BASICHTTP]["location"],
        format!("http://127.0.0.1:{port_b}/sessions/b/basichttp")
    );
    let pong = records_b
        .iter()
        .find(|record| record["type"] == "log")
        .expect("B logs the pong");
    assert_eq!(pong["label"], "pong");
    assert_eq!(pong["text"], format!("7 {BASICHTTP}"));
    let last_step = records_b
        .iter()
        .rfind(|record| record["type"] == "call")
        .expect("B reports steps");
    assert_eq!(last_step["cause"], "http");
    assert_eq!(last_step["configuration"], serde_json::json!(["done"]));
}

const SEND_TO: &str = r#"<state id="s0"><onentry><send event="notify" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="TARGET"/></onentry><transition event="error.communication" target="lost"/></state><final id="lost"/>"#;

#[test]
fn basichttp_sends_fail_without_an_allowlist_and_late_after_acceptance() {
    let closed = free_port();
    let target = format!("http://127.0.0.1:{closed}/hook");
    let directory = scratch("http-denied");
    let doc = document(&directory, &SEND_TO.replace("TARGET", &target));
    // No --http-allow: the send is undeliverable at once.
    let output = run(&doc, &["--clock", "logical"], "");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(kinds(&records), ["opened", "done", "call", "closed"]);
    assert_eq!(records[2]["cause"], "start");
    assert_eq!(records[2]["configuration"], serde_json::json!(["lost"]));

    // Allowed but nothing listens: accepted, then error.communication later.
    let child = serve(
        &doc,
        &["--http-allow", &format!("http://127.0.0.1:{closed}")],
    );
    let output = finish(child, Duration::from_secs(60));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(
        kinds(&records),
        ["opened", "call", "done", "call", "closed"],
        "{records:?}"
    );
    assert_eq!(records[1]["configuration"], serde_json::json!(["s0"]));
    assert_eq!(records[3]["cause"], "communication-error");
    assert_eq!(records[3]["configuration"], serde_json::json!(["lost"]));
}

#[test]
fn http_flags_are_checked_before_anything_runs() {
    let directory = scratch("http-usage");
    let doc = document(&directory, DELAYED);
    for (extra, expected) in [
        (
            &["--http-public-url", "https://agent.example"][..],
            "give --http-listen too",
        ),
        (
            &["--http-listen", "0.0.0.0:0"][..],
            "needs --http-public-url",
        ),
        (
            &["--http-listen", "localhost:80"][..],
            "--http-listen requires PORT or IP:PORT",
        ),
        (
            &[
                "--http-allow",
                "http://127.0.0.1:1",
                "--replay",
                "log.ndjson",
            ][..],
            "--http-allow, --http-listen, and --http-public-url cannot be given with it",
        ),
        (
            &["--http-allow", "https://user:pw@api.example"][..],
            "[http-allow-invalid] --http-allow `https://user:pw@api.example`: allowlist entry 1 carries userinfo",
        ),
        (
            &["--http-allow", "ftp://api.example"][..],
            "[http-allow-invalid]",
        ),
        (
            &[
                "--http-listen",
                "0",
                "--http-public-url",
                "https://agent.example/?q",
            ][..],
            "[http-public-url-invalid]",
        ),
    ] {
        let output = run(&doc, extra, "");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{extra:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(expected),
            "{extra:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty(), "{extra:?}");
    }
}

/// A one-request-at-a-time HTTP server that answers every request 500.
fn failing_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match std::io::Read::read(&mut stream, &mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    port
}

/// POSTs a form `body` to the `http://` `location`; the response status.
fn post_form(location: &str, body: &str) -> u16 {
    let (authority, path) = location
        .split_once("//")
        .and_then(|(_, rest)| rest.split_once('/'))
        .expect("an http location");
    let mut stream = std::net::TcpStream::connect(authority).expect("connect");
    stream
        .write_all(
            format!(
                "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .expect("the request is writable");
    let mut response = String::new();
    let _ = std::io::Read::read_to_string(&mut stream, &mut response);
    response
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0)
}

#[test]
fn a_recorded_http_session_replays_byte_for_byte_without_the_network() {
    let server = failing_server();
    let directory = scratch("http-replay");
    // A denied send fails at once, an accepted one fails later (the server
    // answers 500); then an inbound `hello` ends the session.
    let doc = document(
        &directory,
        &format!(
            r#"<datamodel><data id="n" expr="0"/></datamodel><state id="s0"><onentry><send id="late-note" event="notify" type="{BASICHTTP}" target="http://127.0.0.1:{server}/hook"/><send id="denied-note" event="denied" type="{BASICHTTP}" target="https://denied.example/"/></onentry><transition event="error.communication" cond="n == 0"><assign location="n" expr="1"/><log label="sync" expr="_event.type + ' ' + _event.sendid"/></transition><transition event="error.communication" cond="n == 1" target="s1"><log label="late" expr="_event.type + ' ' + _event.sendid"/></transition></state><state id="s1"><transition event="hello" target="done"><log label="hello" expr="JSON.stringify(_event.data) + ' ' + _event.origintype"/></transition></state><final id="done"/>"#
        ),
    );
    let log = directory.join("session.ndjson");
    let log_arg = log.display().to_string();
    let mut child = serve(
        &doc,
        &[
            "--http-listen",
            "0",
            "--http-allow",
            &format!("http://127.0.0.1:{server}"),
            "--snapshot-on",
            "call",
            "--record",
            &log_arg,
        ],
    );
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    let mut streamed = Vec::new();
    let mut location = None;
    // Read until the late error.communication ran, then post `hello`.
    for line in lines.by_ref() {
        let line = line.expect("the record is readable");
        let record: serde_json::Value = serde_json::from_str(&line).expect("JSON");
        if record["type"] == "opened" {
            location = record["ioprocessors"][BASICHTTP]["location"]
                .as_str()
                .map(str::to_owned);
        }
        let late = record["type"] == "call" && record["cause"] == "communication-error";
        streamed.push(line);
        if late {
            break;
        }
    }
    let location = location.expect("the session advertises its location");
    assert_eq!(post_form(&location, "_scxmleventname=hello&who=world"), 200);
    streamed.extend(lines.map(|line| line.expect("the record is readable")));
    let output = finish(child, Duration::from_secs(60));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let records: Vec<serde_json::Value> = streamed
        .iter()
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    let logs: Vec<&str> = records
        .iter()
        .filter(|record| record["type"] == "log")
        .filter_map(|record| record["text"].as_str())
        .collect();
    // Each error.communication names its failed send (SCXML 5.10.1).
    assert_eq!(
        logs,
        [
            "platform denied-note",
            "platform late-note",
            &format!(r#"{{"who":"world"}} {BASICHTTP}"#)
        ]
    );
    let causes: Vec<&str> = records
        .iter()
        .filter(|record| record["type"] == "call")
        .filter_map(|record| record["cause"].as_str())
        .collect();
    assert_eq!(causes, ["start", "communication-error", "http"]);
    let log_text = fs::read_to_string(&log).expect("the log is readable");
    for kind in ["\"outbound\"", "\"communication-error\"", "\"http\""] {
        assert!(log_text.contains(&format!("\"type\":{kind}")), "{log_text}");
    }
    // The recorded late failure keeps the send id it replays with.
    assert!(
        log_text.contains(r#""senders":[{"session":"session","path":[],"sendid":"late-note"}]"#),
        "{log_text}"
    );

    // Replay: no listener, no allowlist, no server; the same bytes.
    let replay = run(&doc, &["--replay", &log_arg], "");
    assert_eq!(replay.status.code(), Some(0), "{}", stderr(&replay));
    let replayed: Vec<String> = String::from_utf8(replay.stdout)
        .expect("stdout is UTF-8")
        .lines()
        .map(str::to_owned)
        .collect();
    // Everything after `opened` (whose clock differs) is identical,
    // snapshot documents included.
    assert_eq!(streamed[1..], replayed[1..]);
    assert!(
        replayed[0].contains(&location),
        "replay advertises the recorded location: {}",
        replayed[0]
    );
}
