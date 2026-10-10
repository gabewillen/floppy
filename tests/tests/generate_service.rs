//! Integration test of the `generate` service through `scx run`: a
//! session binds the real `floppy_service_generate` component (resolved
//! from its repository URI) and invokes
//! it (`<invoke type="https://github.com/stateforward/floppy/services/generate">`)
//! against a deterministic fake Chat Completions backend on 127.0.0.1
//! (nothing reaches the network). The request runs off the session thread
//! (the service awaits `outbound-http.fetch`, an `async func`, which
//! suspends only its own task); the service returns `generate.result` or
//! `generate.failed` on the stream `invoke` returned, then completes. The key and base URL reach the run only through an
//! `--env-file`, and only the `<invoke>`s whose `env` attributes name them
//! receive them; the component holds slot references and the host
//! substitutes the values into the admitted request.
//!
//! The ignored `live_generate_through_an_env_file` test makes one real call:
//! `FLOPPY_LIVE_ENV_FILE=<path> cargo test -p floppy-tests --test
//! generate_service -- --ignored`. The files must set `FLOPPY_API_KEY` and
//! `FLOPPY_BASE_URL` (the service has no default backend). The model is
//! `FLOPPY_LIVE_MODEL`, else `Qwen/Qwen3.8-Flash-Next`. It reports pass
//! or fail only; no value is ever printed.

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

use support::{Finished, Run, labelled, logs, run_document, scratch, service_uri};

/// The fake key (not a secret): the mock checks it arrives as the bearer.
const FAKE_KEY: &str = "sk-floppy-fake";

fn generate() -> String {
    service_uri("generate")
}

fn decision() -> String {
    service_uri("decision")
}

/// The flags of a run whose values come from `env_file`: the document's
/// `env` attributes scope them, and its repository URIs bind the packages.
fn args(env_file: &Path) -> Vec<String> {
    vec!["--env-file".to_owned(), env_file.display().to_string()]
}

/// Runs `document` with `args` to its end, checking the consent listing
/// names the generate package, that it declares no origin (the `baseurl`
/// value supplies the one it reaches), and its slots.
fn open(args: &[String], document: &str) -> Finished {
    open_with(args, document, &[], Duration::from_secs(60))
}

fn open_with(args: &[String], document: &str, env: &[(&str, &str)], limit: Duration) -> Finished {
    let finished = run_document("generate.scx", document, args, env, limit);
    let consent = &finished.stderr;
    assert!(
        consent.contains(&format!("service `{}`", generate()))
            && consent.contains("outbound HTTP: none")
            && consent.contains("env slots: apikey, baseurl"),
        "the consent listing names the package, no origin, and its slots: {consent}"
    );
    assert!(!consent.contains(FAKE_KEY), "names only: {consent}");
    finished
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    request_line: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// A streamed Chat Completions answer (server-sent events, as the generate
/// service asks for) carrying `content`, split in two deltas.
fn streamed(content: &str) -> String {
    let middle = content.chars().count() / 2;
    let head: String = content.chars().take(middle).collect();
    let tail: String = content.chars().skip(middle).collect();
    let chunk = |delta: serde_json::Value, finish: Option<&str>| {
        serde_json::json!({
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
    };
    [
        chunk(
            serde_json::json!({"role": "assistant", "content": ""}),
            None,
        ),
        chunk(serde_json::json!({"content": head}), None),
        chunk(serde_json::json!({"content": tail}), None),
        chunk(serde_json::json!({}), Some("stop")),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .chain(std::iter::once("data: [DONE]\n\n".to_owned()))
    .collect()
}

/// A deterministic fake Chat Completions backend on 127.0.0.1: the first
/// path segment picks the scripted answer (`/text`, `/json`, `/mismatch`,
/// `/fail` answers 500); every request is recorded.
struct Mock {
    origin: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
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
                    continue;
                }
                let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                log.lock().expect("the log lock").push(Received {
                    request_line: request_line.trim_end().to_owned(),
                    authorization: header("authorization"),
                    body,
                });
                let (status, answer) = match target.as_str() {
                    "/text/v1/chat/completions" => {
                        ("200 OK", streamed("Hello from the fake backend!"))
                    }
                    "/json/v1/chat/completions" => {
                        ("200 OK", streamed(r#"{"score":41,"answer":"yes"}"#))
                    }
                    "/mismatch/v1/chat/completions" => ("200 OK", streamed(r#"{"answer":"yes"}"#)),
                    "/fail/v1/chat/completions" => (
                        "500 Internal Server Error",
                        r#"{"error":{"message":"boom"}}"#.to_owned(),
                    ),
                    _ => ("404 Not Found", "{}".to_owned()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Self { origin, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log lock").clone()
    }
}

/// Writes an env file holding the fake key and `base_url` for one test.
fn env_file(test: &str, base_url: &str) -> PathBuf {
    env_file_with(
        test,
        &format!("FLOPPY_API_KEY={FAKE_KEY}\nFLOPPY_BASE_URL={base_url}\n"),
    )
}

/// An env file of exactly `text`.
fn env_file_with(test: &str, contents: &str) -> PathBuf {
    let path = scratch("generate-env").join(format!("{test}.env"));
    std::fs::write(&path, contents).expect("the env file is writable");
    path
}

/// A document that invokes the generate service with `params` (`<param>`
/// elements): its result is logged by `log_expr` (over `_event.data`), its
/// failure code as `failed`, and the machine finishes in `done` on
/// `done.invoke`, or in `rejected` on `error.execution` (a rejected start).
fn document(params: &str, log_expr: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="ask">
  <state id="ask">
    <invoke id="gen" type="{service}" env:apikey="FLOPPY_API_KEY" env:baseurl="FLOPPY_BASE_URL">
      {params}
    </invoke>
    <transition event="generate.result"><log label="reply" expr="{log_expr}"/></transition>
    <transition event="generate.failed"><log label="failed" expr="_event.data.code"/></transition>
    <transition event="done.invoke.gen" target="done"/>
    <transition event="error.execution" target="rejected"/>
    <transition event="error.communication" target="rejected"/>
  </state>
  <final id="done"><onentry><log label="final" expr="'done'"/></onentry></final>
  <final id="rejected"><onentry><log label="final" expr="'rejected'"/></onentry></final>
</scxml>"#,
        service = generate()
    )
}

const TEXT_PARAMS: &str = r#"<param name="model" expr="'mock-model'"/>
      <param name="system" expr="'Be brief.'"/>
      <param name="prompt" expr="'Say hello.'"/>"#;

const STRUCTURED_PARAMS: &str = r#"<param name="model" expr="'mock-model'"/>
      <param name="prompt" expr="'Decide.'"/>
      <param name="schema" expr="({type: 'object', properties: {answer: {type: 'string'}, score: {type: 'integer'}}, required: ['answer', 'score'], additionalProperties: false})"/>
      <param name="schemaname" expr="'verdict'"/>
      <param name="strict" expr="true"/>"#;

#[test]
fn a_text_generation_posts_the_canonical_request_and_returns_the_reply() {
    let mock = Mock::start();
    let env = env_file("text", &format!("{}/text/v1", mock.origin));
    let args = args(&env);
    let finished = open(&args, &document(TEXT_PARAMS, "_event.data.text"));
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("reply", "Hello from the fake backend!"), ("final", "done")]),
        "{records:?}"
    );
    assert!(support::finished(records), "{}", finished.stderr);
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request");
    assert_eq!(
        received[0].request_line,
        "POST /text/v1/chat/completions HTTP/1.1"
    );
    assert_eq!(
        received[0].authorization.as_deref(),
        Some(format!("Bearer {FAKE_KEY}").as_str()),
        "the host substitutes the env-file key into the bearer header"
    );
    assert_eq!(
        String::from_utf8(received[0].body.clone()).expect("UTF-8"),
        r#"{"messages":[{"content":"Be brief.","role":"system"},{"content":"Say hello.","role":"user"}],"model":"mock-model","reasoning_effort":"low","stream":true}"#,
        "the request body is canonical JSON"
    );
}

#[test]
fn a_structured_generation_returns_the_schema_valid_object() {
    let mock = Mock::start();
    let env = env_file("json", &format!("{}/json/v1", mock.origin));
    let args = args(&env);
    let finished = open(
        &args,
        &document(
            STRUCTURED_PARAMS,
            "_event.data.json.answer + ':' + (_event.data.json.score + 1)",
        ),
    );
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("reply", "yes:42"), ("final", "done")]),
        "{records:?}"
    );
    let received = mock.received();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("the body is JSON");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "verdict");
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
}

#[test]
fn a_reply_that_mismatches_the_schema_is_never_returned() {
    let mock = Mock::start();
    let env = env_file("mismatch", &format!("{}/mismatch/v1", mock.origin));
    let args = args(&env);
    let finished = open(&args, &document(STRUCTURED_PARAMS, "'unexpected'"));
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("failed", "schema-mismatch"), ("final", "done")]),
        "the result is validated before it is returned: {records:?}"
    );
    assert_eq!(mock.received().len(), 1);
}

#[test]
fn a_failing_backend_returns_generate_failed() {
    let mock = Mock::start();
    let env = env_file("fail", &format!("{}/fail/v1", mock.origin));
    let args = args(&env);
    let finished = open(&args, &document(TEXT_PARAMS, "_event.data.text"));
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("failed", "status"), ("final", "done")]),
        "{records:?}"
    );
}

/// A PNG signature and two bytes: the smallest file the service sends as
/// `image/png`.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nab";

/// `--dir` granting the generate service `dir` at `/media`.
fn media(dir: &Path) -> Vec<String> {
    vec![
        "--dir".to_owned(),
        format!("{}={}::/media", generate(), dir.display()),
    ]
}

/// The parameters of mosfet's `ImageDecoder` reading the image `reference`.
fn image_params(reference: &str) -> String {
    format!(
        r#"<param name="model" expr="'mock-model'"/>
      <param name="messages" expr="[{{role: 'user', content: [{{kind: 'text', content: 'Read all visible text in this image. Return only the text.'}}, {{kind: 'image', content: '{reference}'}}]}}]"/>"#
    )
}

#[test]
fn an_image_part_reaches_the_backend_as_a_data_url() {
    let mock = Mock::start();
    let env = env_file("image", &format!("{}/text/v1", mock.origin));
    let granted = support::unique_scratch("generate-media");
    std::fs::write(granted.join("page.png"), PNG).expect("the image is writable");
    let mut args = args(&env);
    args.extend(media(&granted));
    let finished = open(
        &args,
        &document(&image_params("media/page.png"), "_event.data.text"),
    );
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("reply", "Hello from the fake backend!"), ("final", "done")]),
        "{records:?}"
    );
    assert!(
        finished.stderr.contains("  = WASI resources: --dir "),
        "the consent listing shows the grant: {}",
        finished.stderr
    );
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request");
    assert_eq!(
        String::from_utf8(received[0].body.clone()).expect("UTF-8"),
        r#"{"messages":[{"content":[{"text":"Read all visible text in this image. Return only the text.","type":"text"},{"image_url":{"url":"data:image/png;base64,iVBORw0KGgphYg=="},"type":"image_url"}],"role":"user"}],"model":"mock-model","reasoning_effort":"low","stream":true}"#,
        "the image is sent as a data URL of the granted file's bytes"
    );
}

#[test]
fn an_image_reference_outside_the_grant_rejects_the_start_and_sends_nothing() {
    let mock = Mock::start();
    let env = env_file("image-outside", &format!("{}/text/v1", mock.origin));
    let granted = support::unique_scratch("generate-media");
    let outside = support::unique_scratch("generate-outside");
    std::fs::write(outside.join("page.png"), PNG).expect("the image is writable");
    std::fs::write(granted.join("note.txt"), "not an image").expect("the file is writable");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("page.png"), granted.join("escape.png"))
        .expect("the symlink is creatable");
    let mut args = args(&env);
    args.extend(media(&granted));
    for reference in [
        // A directory the run never granted.
        "elsewhere/page.png",
        // The granted directory's parent, by a dot segment.
        "media/../page.png",
        // A file the grant does not hold.
        "media/missing.png",
        // A file that is not an image.
        "media/note.txt",
        // A symlink inside the grant to a file outside it.
        "media/escape.png",
    ] {
        let finished = open(
            &args,
            &document(&image_params(reference), "_event.data.text"),
        );
        assert_eq!(
            logs(&finished.records),
            labelled(&[("final", "rejected")]),
            "{reference}: {:?}",
            finished.records
        );
    }
    assert!(mock.received().is_empty(), "nothing is sent");
}

/// The service imports `wasi:filesystem`, so a run that binds it without
/// a `--dir` grant is refused before anything runs.
#[test]
fn a_run_without_a_media_grant_is_refused() {
    let env = env_file("no-grant", "http://127.0.0.1:9/v1");
    let path = support::write(
        &support::unique_scratch("generate-no-grant"),
        "generate.scx",
        &document(TEXT_PARAMS, "_event.data.text"),
    );
    let output = support::output(&[
        "run".to_owned(),
        "--env-file".to_owned(),
        env.display().to_string(),
        path.display().to_string(),
    ]);
    let stderr = &output.stderr;
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("whose resource the run does not grant"),
        "{stderr}"
    );
}

#[test]
fn parameters_the_package_validator_sees_refuse_the_run_and_send_nothing() {
    let mock = Mock::start();
    let env = env_file("malformed", &format!("{}/text/v1", mock.origin));
    // No model, and a parameter the service does not define: the generate
    // package validator reports both before the session starts.
    let finished = support::run_document(
        "generate.scx",
        &document(
            r#"<param name="prompt" expr="'Say hello.'"/><param name="temperature" expr="1"/>"#,
            "_event.data.text",
        ),
        &args(&env),
        &[],
        Duration::from_secs(60),
    );
    assert!(!finished.status.success());
    assert!(finished.records.is_empty(), "{:?}", finished.records);
    for code in ["generate::missing_param", "generate::unknown_param"] {
        assert!(finished.stderr.contains(code), "{}", finished.stderr);
    }
    assert!(mock.received().is_empty(), "nothing is sent");
}

#[test]
fn malformed_parameters_reject_the_start_and_send_nothing() {
    let mock = Mock::start();
    let env = env_file("malformed", &format!("{}/text/v1", mock.origin));
    let args = args(&env);
    // A value only the run knows (the package validator checks literal
    // values only) that the service rejects when the invocation starts.
    let finished = open(
        &args,
        &document(
            r#"<param name="model" expr="'mock-model'"/><param name="prompt" expr="'Say hello.'"/><param name="reasoning_effort" expr="'max'.toUpperCase()"/>"#,
            "_event.data.text",
        ),
    );
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("final", "rejected")]),
        "{records:?}"
    );
    assert!(mock.received().is_empty(), "nothing is sent");
}

#[test]
fn without_a_granted_key_nothing_is_sent() {
    let mock = Mock::start();
    let env = env_file_with(
        "nokey",
        &format!("FLOPPY_BASE_URL={}/text/v1\n", mock.origin),
    );
    // Only the base URL has a value: the apikey slot stays absent.
    let args = args(&env);
    let finished = open(&args, &document(TEXT_PARAMS, "_event.data.text"));
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("failed", "unconfigured"), ("final", "done")]),
        "{records:?}"
    );
    assert!(mock.received().is_empty(), "no request without a key");
}

/// The package declares no `http-origins`, so the one origin a request may
/// reach is the one the run's `FLOPPY_BASE_URL` value supplies (here the
/// loopback mock, which no manifest names): the host admits it because the
/// run chose it.
#[test]
fn a_held_base_url_outside_the_declared_origins_is_the_one_the_request_reaches() {
    let mock = Mock::start();
    let env = env_file("held-base", &format!("{}/text/v1", mock.origin));
    let finished = open(&args(&env), &document(TEXT_PARAMS, "_event.data.text"));
    assert_eq!(
        logs(&finished.records),
        labelled(&[("reply", "Hello from the fake backend!"), ("final", "done")]),
        "{:?}",
        finished.records
    );
    let received = mock.received();
    assert_eq!(received.len(), 1, "exactly one request, to the held origin");
    assert_eq!(
        received[0].request_line,
        "POST /text/v1/chat/completions HTTP/1.1"
    );
}

/// No default backend: without a held `FLOPPY_BASE_URL` the invocation
/// fails fast with `unconfigured` and sends nothing, and the consent
/// listing shows the variable has no value.
#[test]
fn without_a_held_base_url_nothing_is_sent_and_the_generation_is_unconfigured() {
    let mock = Mock::start();
    let env = env_file_with("nobase", &format!("FLOPPY_API_KEY={FAKE_KEY}\n"));
    let finished = open(&args(&env), &document(TEXT_PARAMS, "_event.data.text"));
    assert_eq!(
        logs(&finished.records),
        labelled(&[("failed", "unconfigured"), ("final", "done")]),
        "{:?}",
        finished.records
    );
    assert!(
        finished.stderr.contains("env `FLOPPY_BASE_URL` (no value)"),
        "{}",
        finished.stderr
    );
    assert!(mock.received().is_empty(), "no request without a base URL");
}

/// A document whose generate invocation names only the base URL and whose
/// decision invocation (in a state the run never enters) names the key:
/// each variable goes only to the `<invoke>` whose `env` attribute names it.
fn two_services(params: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="ask">
  <state id="ask">
    <invoke id="gen" type="{generate}" env:baseurl="FLOPPY_BASE_URL">
      {params}
    </invoke>
    <transition event="generate.result"><log label="reply" expr="_event.data.text"/></transition>
    <transition event="generate.failed"><log label="failed" expr="_event.data.code"/></transition>
    <transition event="done.invoke.gen" target="done"/>
    <transition event="error.execution" target="rejected"/>
    <transition event="error.communication" target="rejected"/>
  </state>
  <state id="elsewhere">
    <invoke id="other" type="{decision}" env:apikey="FLOPPY_API_KEY"/>
  </state>
  <final id="done"><onentry><log label="final" expr="'done'"/></onentry></final>
  <final id="rejected"><onentry><log label="final" expr="'rejected'"/></onentry></final>
</scxml>"#,
        generate = generate(),
        decision = decision(),
    )
}

#[test]
fn the_consent_lists_each_service_and_where_each_variable_goes_and_never_a_value() {
    let env = env_file("listing", "http://127.0.0.1:9/v1");
    let finished = open(&args(&env), &two_services(TEXT_PARAMS));
    let consent = &finished.stderr;
    assert!(
        consent.contains(&format!("service `{}`", decision()))
            && consent.contains("outbound HTTP: https://api.typesafe.ai"),
        "{consent}"
    );
    assert!(
        consent.contains(&format!(
            "env `FLOPPY_API_KEY`\n  = to: <invoke type=\"{}\"> env:apikey",
            decision()
        )) && consent.contains(&format!(
            "env `FLOPPY_BASE_URL`\n  = to: <invoke type=\"{}\"> env:baseurl",
            generate()
        )),
        "the listing shows each variable and the invoke it goes to: {consent}"
    );
}

#[test]
fn a_key_named_only_by_another_invoke_is_absent_and_nothing_is_sent() {
    let mock = Mock::start();
    let env = env_file("out-of-scope", &format!("{}/text/v1", mock.origin));
    // Only the decision invocation names the key: the generate invocation
    // gets no reference for it and its apikey slot is absent, exactly as if
    // the run held no value.
    let finished = open(&args(&env), &two_services(TEXT_PARAMS));
    let records = &finished.records;
    assert_eq!(
        logs(records),
        labelled(&[("failed", "unconfigured"), ("final", "done")]),
        "{records:?}"
    );
    assert!(
        mock.received().is_empty(),
        "no request without an in-scope key"
    );
}

/// A backend that streams its answer forever: it accepts one connection,
/// reads the request, answers a chunked `text/event-stream` head, writes one
/// chunk every 20 ms until a write fails, and reports when it saw the
/// client close the connection.
struct Stall {
    origin: String,
    requested: std::sync::mpsc::Receiver<()>,
    closed: std::sync::mpsc::Receiver<Instant>,
}

impl Stall {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the stall binds a loopback port");
        let origin = format!("http://{}", listener.local_addr().expect("an address"));
        let (requested_tx, requested) = std::sync::mpsc::channel();
        let (closed_tx, closed) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the service connects");
            let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.trim_end().split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            reader
                .read_exact(&mut body)
                .expect("the request body is read");
            let mut writer = stream.try_clone().expect("the stream clones");
            writer
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n")
                .expect("the head is written");
            requested_tx.send(()).expect("the test waits");
            let streaming = std::thread::spawn(move || {
                let event =
                    b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"token \"}}]}\n\n";
                let framed = [format!("{:x}\r\n", event.len()).as_bytes(), event, b"\r\n"].concat();
                while writer.write_all(&framed).is_ok() {
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
            // The client sends nothing more: the next read ends when it
            // closes the connection.
            let mut rest = [0_u8; 64];
            while matches!(reader.read(&mut rest), Ok(read) if read > 0) {}
            closed_tx.send(Instant::now()).expect("the test waits");
            streaming.join().expect("the streaming thread ends");
        });
        Self {
            origin,
            requested,
            closed,
        }
    }
}

/// Leaving the invoking state cancels the generate invocation (SCXML 6.4),
/// and the host closes its streaming request's connection at once, while
/// the session keeps running: the backend sees the close and stops
/// generating, so no more tokens are produced for a cancelled call.
#[test]
fn leaving_the_invoking_state_closes_the_streaming_model_call() {
    let stall = Stall::start();
    let env = env_file("stall", &format!("{}/v1", stall.origin));
    let mut args = args(&env);
    let document = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="ask">
  <state id="ask">
    <invoke id="gen" type="{generate}" env:apikey="FLOPPY_API_KEY" env:baseurl="FLOPPY_BASE_URL">
      {TEXT_PARAMS}
    </invoke>
    <transition event="generate.result"><log label="reply" expr="_event.data.text"/></transition>
    <transition event="leave" target="away"/>
  </state>
  <state id="away">
    <onentry><log label="away" expr="'left'"/></onentry>
    <transition event="stop" target="done"/>
  </state>
  <final id="done"/>
</scxml>"#,
        generate = generate()
    );
    let path = support::write(&scratch("generate-stall"), "stall.scx", &document);
    args.extend([
        "--clock".to_owned(),
        "logical".to_owned(),
        path.display().to_string(),
    ]);
    let mut run = Run::start(&args, &[]);
    stall
        .requested
        .recv_timeout(Duration::from_secs(60))
        .expect("the model call reaches the backend");
    std::thread::sleep(Duration::from_millis(100));
    let left = Instant::now();
    run.event("leave", &serde_json::Value::Null);
    let closed = stall
        .closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the backend sees the connection close within 5 s of the cancel");
    assert!(closed.duration_since(left) < Duration::from_secs(5));
    run.event("stop", &serde_json::Value::Null);
    run.end_input();
    let finished = run.finish(Duration::from_secs(30));
    let records = &finished.records;
    assert_eq!(logs(records), labelled(&[("away", "left")]), "{records:?}");
    assert!(support::finished(records), "{}", finished.stderr);
}

/// One real structured generation through an env file
/// (`FLOPPY_LIVE_ENV_FILE`; `FLOPPY_LIVE_MODEL`, default
/// `Qwen/Qwen3.8-Flash-Next`).
/// Prints nothing but pass or fail.
#[test]
#[ignore = "makes one real model call; needs FLOPPY_LIVE_ENV_FILE"]
fn live_generate_through_an_env_file() {
    let args: Vec<String> = support::live_env_files()
        .iter()
        .flat_map(|path| args(path))
        .collect();
    let model =
        std::env::var("FLOPPY_LIVE_MODEL").unwrap_or_else(|_| "Qwen/Qwen3.8-Flash-Next".into());
    let params = format!(
        r#"<param name="model" expr="'{model}'"/>
      <param name="system" expr="'Answer in JSON only.'"/>
      <param name="prompt" expr="'Greet a state machine in at most five words.'"/>
      <param name="schema" expr="({{type: 'object', properties: {{greeting: {{type: 'string'}}}}, required: ['greeting'], additionalProperties: false}})"/>
      <param name="schemaname" expr="'greeting'"/>
      <param name="strict" expr="true"/>"#
    );
    // The env files must hold FLOPPY_BASE_URL: the service has no default.
    let finished = open_with(
        &args,
        &document(
            &params,
            "typeof _event.data.json.greeting === 'string' &amp;&amp; _event.data.json.greeting.length &gt; 0",
        ),
        &[],
        Duration::from_secs(180),
    );
    let pass = logs(&finished.records) == labelled(&[("reply", "true"), ("final", "done")]);
    println!("live generate: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live generation did not pass");
}

/// One real image reading through an env file, as mosfet's `ImageDecoder`
/// reads (`FLOPPY_LIVE_ENV_FILE`; `FLOPPY_LIVE_MODEL`, a vision model): the
/// fixture `tests/fixtures/total-due.png` (mosfet's reading example, "Total
/// due: $14.21") is granted at `/media` and read by reference.
/// Prints nothing but pass or fail.
#[test]
#[ignore = "makes one real model call; needs FLOPPY_LIVE_ENV_FILE and a vision model"]
fn live_image_reading_through_an_env_file() {
    let mut args: Vec<String> = support::live_env_files()
        .iter()
        .flat_map(|path| args(path))
        .collect();
    args.extend(media(&support::workspace().join("tests/fixtures")));
    let model =
        std::env::var("FLOPPY_LIVE_MODEL").unwrap_or_else(|_| "Qwen/Qwen3.8-Flash-Next".into());
    let params = image_params("media/total-due.png").replace("'mock-model'", &format!("'{model}'"));
    let finished = open_with(
        &args,
        &document(&params, "_event.data.text.indexOf('14.21') &gt;= 0"),
        &[],
        Duration::from_secs(180),
    );
    let pass = logs(&finished.records) == labelled(&[("reply", "true"), ("final", "done")]);
    println!("live image reading: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live image reading did not pass");
}
