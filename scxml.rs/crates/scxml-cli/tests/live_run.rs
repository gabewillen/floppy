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
//! already verified release instead (it must hold
//! `scxml_service_example.virtualized.wasm` too).

use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

/// The release components the Wasmtime session host links, and the echo
/// and files invoked-service examples the service tests bind.
const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_service_example",
    "scxml_service_files",
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

const ECHO: &str = "https://stateforward.org/scxml/services/echo";

/// A recorded run binds its invoked services' component digests: the same
/// binding replays, and a different `--service-component-sha256` is refused
/// before anything runs.
#[test]
fn a_replay_with_a_different_service_component_is_refused() {
    let directory = scratch("replay-service");
    let doc = document(
        &directory,
        &format!(
            r##"<state id="s0"><invoke type="{ECHO}" id="echo"/><transition event="ping"><send event="ping" target="#_echo"/></transition><transition event="echo.ping"><log label="echoed" expr="_event.name"/></transition><transition event="stop"><send event="stop" target="#_echo"/></transition><transition event="done.invoke.echo" target="done"/></state><final id="done"/>"##
        ),
    );
    let (dir, _) = release();
    let component = Path::new(dir).join("scxml_service_example.virtualized.wasm");
    let digest = format!(
        "{:x}",
        Sha256::digest(fs::read(&component).expect("the echo component is readable"))
    );
    let manifest = workspace().join("components/scxml-service-example/package/echo.service.json");
    let binding = |sha256: &str| -> Vec<String> {
        vec![
            "--clock".into(),
            "logical".into(),
            "--service-package".into(),
            manifest.display().to_string(),
            "--service-component".into(),
            format!("{ECHO}={}", component.display()),
            "--service-component-sha256".into(),
            format!("{ECHO}={sha256}"),
        ]
    };
    let log = directory.join("session.ndjson");
    let log_arg = log.display().to_string();
    let recorded_args = [binding(&digest), vec!["--record".into(), log_arg.clone()]].concat();
    let recorded_args: Vec<&str> = recorded_args.iter().map(String::as_str).collect();
    let input = "{\"name\":\"ping\"}\n{\"name\":\"stop\"}\n";
    let recorded = run(&doc, &recorded_args, input);
    assert_eq!(recorded.status.code(), Some(0), "{}", stderr(&recorded));
    let header: serde_json::Value = serde_json::from_str(
        fs::read_to_string(&log)
            .expect("the log is readable")
            .lines()
            .next()
            .expect("a header line"),
    )
    .expect("the header is JSON");
    assert_eq!(
        header["services"],
        serde_json::json!([{"uri": ECHO, "service_component_sha256": digest}])
    );

    let replay_args = |sha256: &str| -> Vec<String> {
        let mut args = binding(sha256);
        // A replay takes its clock from the log.
        args.drain(0..2);
        args.extend(["--replay".into(), log_arg.clone()]);
        args
    };
    let same = replay_args(&digest);
    let same: Vec<&str> = same.iter().map(String::as_str).collect();
    let replayed = run(&doc, &same, "");
    assert_eq!(replayed.status.code(), Some(0), "{}", stderr(&replayed));
    assert_eq!(parse_records(&recorded)[1..], parse_records(&replayed)[1..]);

    let other = replay_args(&"c".repeat(64));
    let other: Vec<&str> = other.iter().map(String::as_str).collect();
    let refused = run(&doc, &other, "");
    assert_eq!(refused.status.code(), Some(2), "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("[replay-identity-mismatch]")
            && stderr(&refused).contains("invoked service components"),
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

/// The echo service's `endpoint` slot bound to `endpoint`, by digest, on
/// the logical clock.
fn fetch_binding(endpoint: &str) -> Vec<String> {
    let (dir, _) = release();
    let component = Path::new(dir).join("scxml_service_example.virtualized.wasm");
    let digest = format!(
        "{:x}",
        Sha256::digest(fs::read(&component).expect("the echo component is readable"))
    );
    let manifest = workspace().join("components/scxml-service-example/package/echo.service.json");
    vec![
        "--clock".into(),
        "logical".into(),
        "--service-package".into(),
        manifest.display().to_string(),
        "--service-component".into(),
        format!("{ECHO}={}", component.display()),
        "--service-component-sha256".into(),
        format!("{ECHO}={digest}"),
        "--env".into(),
        format!("ENDPOINT@{ECHO}={endpoint}"),
    ]
}

/// `s0` invokes the echo service with its `endpoint` slot; `fetch` makes it
/// GET the slot's URL with `outbound-http.fetch`, `ping` is echoed, `leave`
/// exits `s0` (cancelling the invocation, SCXML 6.4), and the 1 s timer
/// `fetch` arms fires on the logical clock only once no request is in
/// flight.
fn fetch_document(directory: &Path) -> PathBuf {
    document(
        directory,
        &format!(
            r##"<state id="s0" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"><invoke type="{ECHO}" id="echo" env:endpoint="ENDPOINT"/><transition event="fetch"><send event="fetch" target="#_echo"/><send event="timer" delay="1s"/></transition><transition event="ping"><send event="ping" target="#_echo"/></transition><transition event="echo.ping"><log label="ping" expr="_event.name"/></transition><transition event="echo.fetched"><log label="fetched" expr="_event.data"/></transition><transition event="timer" target="done"><log label="timer" expr="'fired'"/></transition><transition event="leave" target="away"/></state><state id="away"><transition event="timer"><log label="timer" expr="'fired'"/></transition></state><final id="done"/>"##
        ),
    )
}

/// Reads one HTTP request head from `stream`.
fn read_head(stream: &mut std::net::TcpStream) {
    use std::io::Read as _;
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).expect("the head is read"), 1);
        head.push(byte[0]);
    }
}

fn log_lines(records: &[serde_json::Value]) -> Vec<String> {
    records
        .iter()
        .filter(|record| record["type"] == "log")
        .map(|record| {
            format!(
                "{}: {}",
                record["label"].as_str().unwrap_or_default(),
                record["text"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// Serves one request on a loopback port, answering `204` once `answer`
/// receives (or at once when it is `None`).
fn answer_once(
    answer: Option<std::sync::mpsc::Receiver<()>>,
) -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
    let port = listener.local_addr().expect("an address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the service connects");
        read_head(&mut stream);
        if let Some(answer) = answer {
            answer
                .recv_timeout(Duration::from_secs(30))
                .expect("the ping is answered while the request is in flight");
        }
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .expect("the answer is written");
    });
    (port, server)
}

/// A service's `fetch` suspends only its own task: a slow request never
/// blocks the session (the `ping` sent while it is in flight is answered
/// first), its response arrives in an `http-response` call, and the
/// logical clock fires the 1 s timer only after it.
#[test]
fn a_service_fetch_never_blocks_the_session() {
    let (pinged_tx, pinged) = std::sync::mpsc::channel::<()>();
    let (port, server) = answer_once(Some(pinged));
    let directory = scratch("fetch");
    let doc = fetch_document(&directory);
    let args = fetch_binding(&format!("http://127.0.0.1:{port}/slow"));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = command(&doc, &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scxml-cli must start");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    stdin
        .write_all(b"{\"name\":\"fetch\"}\n{\"name\":\"ping\"}\n")
        .expect("input is writable");
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).expect("stdout is readable") > 0,
            "the ping is logged before the run ends: {lines:?}"
        );
        let pinged_now = line.contains("\"label\":\"ping\"");
        lines.push(line);
        if pinged_now {
            break;
        }
    }
    pinged_tx.send(()).expect("the server waits");
    drop(stdin);
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut rest).expect("stdout is readable");
    let output = child.wait_with_output().expect("scxml-cli must finish");
    server.join().expect("the server ends");
    let text = format!("{}{rest}", lines.concat());
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{line}: {error}")))
        .collect();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        log_lines(&records),
        ["ping: echo.ping", "fetched: 204", "timer: fired"]
    );
    assert!(
        records
            .iter()
            .any(|record| record["type"] == "call" && record["cause"] == "http-response"),
        "{text}"
    );
}

/// Leaving the invoking state cancels the invocation (SCXML 6.4): `run`
/// closes its request's connection at once, while the process and its
/// session keep running.
#[test]
fn leaving_the_invoking_state_closes_the_request_connection() {
    use std::io::Read as _;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
    let port = listener.local_addr().expect("an address").port();
    let (requested_tx, requested) = std::sync::mpsc::channel::<()>();
    let (closed_tx, closed) = std::sync::mpsc::channel::<Instant>();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the service connects");
        read_head(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n")
            .expect("the head is written");
        requested_tx.send(()).expect("the test waits");
        let mut writer = stream.try_clone().expect("the stream clones");
        let streaming = std::thread::spawn(move || {
            while writer.write_all(b"8\r\ndata: x\n\r\n").is_ok() {
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let mut rest = [0_u8; 64];
        while matches!(stream.read(&mut rest), Ok(read) if read > 0) {}
        closed_tx.send(Instant::now()).expect("the test waits");
        streaming.join().expect("the streaming thread ends");
    });
    let directory = scratch("fetch-cancel");
    let doc = fetch_document(&directory);
    let args = fetch_binding(&format!("http://127.0.0.1:{port}/stream"));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = command(&doc, &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scxml-cli must start");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(b"{\"name\":\"fetch\"}\n")
        .expect("input is writable");
    requested
        .recv_timeout(Duration::from_secs(30))
        .expect("the request reaches the server");
    std::thread::sleep(Duration::from_millis(100));
    let left = Instant::now();
    stdin
        .write_all(b"{\"name\":\"leave\"}\n")
        .expect("input is writable");
    let closed_at = closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the server sees the connection close while the run goes on");
    assert!(closed_at.duration_since(left) < Duration::from_secs(2));
    assert!(
        child.try_wait().expect("the child is waitable").is_none(),
        "the run is still open: the cancel, not the exit, closed the connection"
    );
    stdin
        .write_all(b"{\"type\":\"close\"}\n")
        .expect("input is writable");
    drop(stdin);
    let output = child.wait_with_output().expect("scxml-cli must finish");
    server.join().expect("the server ends");
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let records = parse_records(&output);
    assert_eq!(log_lines(&records), ["timer: fired"]);
}

/// A service's HTTP responses are recorded inputs: `--record` performs the
/// request and writes its response, and `--replay` reproduces the session
/// byte for byte with nothing listening.
#[test]
fn a_recorded_service_fetch_replays_without_the_network() {
    let (port, server) = answer_once(None);
    let directory = scratch("fetch-record");
    let doc = fetch_document(&directory);
    let log = directory.join("session.ndjson");
    let log_arg = log.display().to_string();
    let binding = fetch_binding(&format!("http://127.0.0.1:{port}/once"));
    let recorded_args = [binding.clone(), vec!["--record".into(), log_arg.clone()]].concat();
    let recorded_args: Vec<&str> = recorded_args.iter().map(String::as_str).collect();
    let recorded = run(&doc, &recorded_args, "{\"name\":\"fetch\"}\n");
    server.join().expect("the server ends");
    assert_eq!(recorded.status.code(), Some(0), "{}", stderr(&recorded));
    assert_eq!(
        log_lines(&parse_records(&recorded)),
        ["fetched: 204", "timer: fired"]
    );
    assert!(
        fs::read_to_string(&log)
            .expect("the log is readable")
            .contains("\"http-response\""),
        "the response is a recorded input"
    );
    // A replay takes its clock from the log; nothing listens on the port.
    let mut replay_args = binding;
    replay_args.drain(0..2);
    replay_args.extend(["--replay".into(), log_arg]);
    let replay_args: Vec<&str> = replay_args.iter().map(String::as_str).collect();
    let replayed = run(&doc, &replay_args, "");
    assert_eq!(replayed.status.code(), Some(0), "{}", stderr(&replayed));
    assert_eq!(parse_records(&recorded)[1..], parse_records(&replayed)[1..]);
}

const FILES: &str = "https://stateforward.org/scxml/services/files";

/// The files service imports `wasi:filesystem`: `run` binds it only with a
/// `--dir` grant (wasmtime's `--dir HOST::GUEST`, scoped to its type URI),
/// and then it writes into that directory.
#[test]
fn a_wasi_service_runs_only_with_its_dir_granted() {
    let directory = scratch("files");
    let workspace_dir = directory.join("workspace");
    fs::create_dir_all(&workspace_dir).expect("the workspace is creatable");
    let doc = document(
        &directory,
        &format!(
            r#"<state id="s0"><invoke type="{FILES}" id="files"><param name="name" expr="'out.txt'"/><param name="text" expr="'hi'"/></invoke><transition event="done.invoke.files" target="done"><log label="done" expr="JSON.stringify(_event.data)"/></transition></state><final id="done"/>"#
        ),
    );
    let (dir, _) = release();
    let component = Path::new(dir).join("scxml_service_files.virtualized.wasm");
    let digest = format!(
        "{:x}",
        Sha256::digest(fs::read(&component).expect("the files component is readable"))
    );
    let manifest = workspace().join("components/scxml-service-files/package/files.service.json");
    let binding = vec![
        "--clock".to_owned(),
        "logical".into(),
        "--service-package".into(),
        manifest.display().to_string(),
        "--service-component".into(),
        format!("{FILES}={}", component.display()),
        "--service-component-sha256".into(),
        format!("{FILES}={digest}"),
    ];
    let args: Vec<&str> = binding.iter().map(String::as_str).collect();
    let ungranted = run(&doc, &args, "");
    assert_ne!(ungranted.status.code(), Some(0), "{}", stderr(&ungranted));
    assert!(
        stderr(&ungranted).contains("whose resource the run does not grant"),
        "{}",
        stderr(&ungranted)
    );
    assert!(!workspace_dir.join("out.txt").exists());

    let granted = [
        binding,
        vec![
            "--dir".into(),
            format!("{FILES}={}::/workspace", workspace_dir.display()),
        ],
    ]
    .concat();
    let args: Vec<&str> = granted.iter().map(String::as_str).collect();
    let output = run(&doc, &args, "");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("= WASI imports: ")
            && stderr(&output).contains("wasi:filesystem/preopens")
            && stderr(&output).contains("= WASI resources: --dir "),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        log_lines(&parse_records(&output)),
        [r#"done: {"seed":null,"wrote":2}"#]
    );
    assert_eq!(
        fs::read_to_string(workspace_dir.join("out.txt")).expect("the file is written"),
        "hi"
    );
}
