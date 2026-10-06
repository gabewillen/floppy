//! nemobot-host integration test of the `generate` namespace: a live session
//! binds the real `nemobot_generate` extension component and runs
//! `<gen:generate>` against a deterministic fake Chat Completions backend on
//! 127.0.0.1 (nothing reaches the network). The key and base URL reach the
//! run only through an env file the config names; the component holds slot
//! references and the host substitutes the values into the admitted request.
//!
//! `scripts/build-components.sh` builds and verifies the release session
//! components and the namespace components (import allowlists included) into
//! `target/test-generate-namespace/components`.
//!
//! The ignored `live_generate_through_an_env_file` test makes one real call:
//! `NEMOBOT_LIVE_ENV_FILE=<path> cargo test -p nemobot-host --test
//! generate_namespace -- --ignored`. It grants `OPENAI_API_KEY` (the file
//! must set it) and `OPENAI_BASE_URL` (the file's value, else the config's
//! `env-defaults` `OpenAI` URL: no backend URL is compiled into the
//! component), and reports pass or fail only; no value is ever printed.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nemobot_host::compile::compile_source;
use nemobot_host::config::{CONFIG_FORMAT, Config};
use nemobot_host::env_file::Values;
use nemobot_host::packages::Packages;
use nemobot_host::runtime::{ClockMode, Egress, SessionHandle, SessionSpec, SessionStatus};
use scxml_host_wasmtime::SessionOutput;
use sha2::{Digest as _, Sha256};

const NAMESPACE_URI: &str = "https://github.com/stateforward/nemobot/namespaces/generate@v0.1.0";
/// The fake key (not a secret): the mock checks it arrives as the bearer.
const FAKE_KEY: &str = "sk-nemobot-fake";

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the host crate sits in the nemobot workspace")
}

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("the artifact is readable");
    format!("{:x}", Sha256::digest(&bytes))
}

/// The verified artifacts: the session components' directory and manifest
/// digest, and the generate component and its digest.
struct Artifacts {
    components: PathBuf,
    manifest_sha256: String,
    component: PathBuf,
    component_sha256: String,
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let dir = workspace().join("target/test-generate-namespace/components");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--artifacts-dir")
            .arg(&dir)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the component proof must pass");
        let components = dir.join("scxml");
        let component = dir.join("namespaces/nemobot_generate.virtualized.wasm");
        Artifacts {
            manifest_sha256: sha256_file(&components.join("manifest.tsv")),
            component_sha256: sha256_file(&component),
            components,
            component,
        }
    })
}

/// A config binding the generate package, granting `env` from `env_file`.
fn config(env_file: &Path, env: &[&str]) -> Config {
    config_with_defaults(env_file, env, &serde_json::json!({}))
}

/// [`config`] with non-secret `env-defaults`.
fn config_with_defaults(env_file: &Path, env: &[&str], defaults: &serde_json::Value) -> Config {
    let artifacts = artifacts();
    let package = workspace().join("namespaces/generate/package/generate.extension.json");
    let text = serde_json::json!({
        "format": CONFIG_FORMAT,
        "components": {
            "dir": artifacts.components.display().to_string(),
            "manifest-sha256": artifacts.manifest_sha256,
        },
        "env-file": env_file.display().to_string(),
        "env": env,
        "env-defaults": defaults,
        "extensions": [{
            "manifest": package.display().to_string(),
            "component": artifacts.component.display().to_string(),
            "component-sha256": artifacts.component_sha256,
        }],
        "outbound-http": true,
    })
    .to_string();
    Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the test config is valid")
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    request_line: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

const TEXT_REPLY: &str = r#"{"id":"c","object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Hello from the fake backend!"}}]}"#;
const JSON_REPLY: &str = r#"{"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"{\"score\":41,\"answer\":\"yes\"}"}}]}"#;
const MISMATCH_REPLY: &str = r#"{"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"{\"answer\":\"yes\"}"}}]}"#;

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
                    "/text/v1/chat/completions" => ("200 OK", TEXT_REPLY),
                    "/json/v1/chat/completions" => ("200 OK", JSON_REPLY),
                    "/mismatch/v1/chat/completions" => ("200 OK", MISMATCH_REPLY),
                    "/fail/v1/chat/completions" => (
                        "500 Internal Server Error",
                        r#"{"error":{"message":"boom"}}"#,
                    ),
                    _ => ("404 Not Found", "{}"),
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
    let dir = workspace().join("target/test-generate-namespace/env");
    std::fs::create_dir_all(&dir).expect("the env directory is creatable");
    let path = dir.join(format!("{test}.env"));
    std::fs::write(
        &path,
        format!("OPENAI_API_KEY={FAKE_KEY}\nOPENAI_BASE_URL={base_url}\n"),
    )
    .expect("the env file is writable");
    path
}

fn open(config: &Config, document: &str) -> SessionHandle {
    let packages = Packages::load(config).expect("the generate package binds");
    let consent = packages.consent(&Values::none());
    assert!(
        consent.contains(NAMESPACE_URI)
            && consent.contains("http-origins: https://api.openai.com")
            && consent.contains("env-slots: apikey, baseurl"),
        "the consent listing names the package, its origin, and its slots: {consent}"
    );
    let values = Values::granted(
        config
            .env_file
            .as_deref()
            .expect("the config names an env file"),
        &config.env,
        &config.env_defaults,
        &|_| None,
    )
    .expect("the env file grants the variables");
    let compiled = compile_source(
        "generate.scxml",
        document.to_owned(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("the document compiles with the generate package");
    SessionHandle::open(
        config,
        packages,
        values,
        SessionSpec {
            compiled,
            session_id: "generate-namespace".into(),
            clock: ClockMode::Logical,
            documents: Vec::new(),
        },
    )
    .expect("the session opens")
}

/// Egress records until the session closes, at most `limit`.
fn until_closed(handle: &SessionHandle, limit: Duration) -> Vec<Egress> {
    let deadline = Instant::now() + limit;
    let mut records = Vec::new();
    while Instant::now() < deadline {
        if let Some(record) = handle.recv_timeout(Duration::from_millis(100)) {
            let closed = matches!(record, Egress::Closed { .. });
            records.push(record);
            if closed {
                return records;
            }
        }
    }
    panic!(
        "the session did not close in time; got {} records",
        records.len()
    );
}

fn logs(records: &[Egress]) -> Vec<(String, String)> {
    records
        .iter()
        .filter_map(|record| match record {
            Egress::Outputs(outputs) => Some(outputs),
            _ => None,
        })
        .flatten()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => {
                Some((entry.label.clone().unwrap_or_default(), entry.text.clone()))
            }
            SessionOutput::Done { .. } => None,
        })
        .collect()
}

fn finished(records: &[Egress]) -> bool {
    matches!(
        records.last(),
        Some(Egress::Closed {
            status: SessionStatus::Finished,
            ..
        })
    )
}

/// A document around one `<gen:generate>` element: the reply is logged by
/// `log_expr`, and the machine finishes in `done` once `reply` is assigned
/// or in `failed` on `error.execution`.
fn document(element: &str, log_expr: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"
       xmlns:gen="{NAMESPACE_URI}"
       xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env"
       version="1.0" datamodel="ecmascript" initial="ask">
  <datamodel><data id="reply"/></datamodel>
  <state id="ask">
    <onentry>
      {element}
      <log label="reply" expr="{log_expr}"/>
    </onentry>
    <transition cond="reply !== undefined" target="done"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="done"><onentry><log label="final" expr="'done'"/></onentry></final>
  <final id="failed"><onentry><log label="final" expr="reply === undefined ? 'failed-unassigned' : 'failed-assigned'"/></onentry></final>
</scxml>"#
    )
}

const TEXT_ELEMENT: &str = r#"<gen:generate location="reply" model="mock-model" system="Be brief."
                    env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL">
        <gen:content>Say hello.</gen:content>
      </gen:generate>"#;

const STRUCTURED_ELEMENT: &str = r#"<gen:generate location="reply" model="mock-model" schemaname="verdict" strict="true"
                    env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL">
        <gen:content>Decide.</gen:content>
        <gen:schema>{"type":"object","properties":{"answer":{"type":"string"},"score":{"type":"integer"}},"required":["answer","score"],"additionalProperties":false}</gen:schema>
      </gen:generate>"#;

fn labelled(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(label, text)| ((*label).to_owned(), (*text).to_owned()))
        .collect()
}

#[test]
fn a_text_generation_posts_the_canonical_request_and_assigns_the_reply() {
    let mock = Mock::start();
    let env = env_file("text", &format!("{}/text/v1", mock.origin));
    let config = config(&env, &["OPENAI_API_KEY", "OPENAI_BASE_URL"]);
    let handle = open(&config, &document(TEXT_ELEMENT, "reply"));
    let records = until_closed(&handle, Duration::from_secs(60));
    assert_eq!(
        logs(&records),
        labelled(&[("reply", "Hello from the fake backend!"), ("final", "done")]),
        "{records:?}"
    );
    assert!(finished(&records));
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
        r#"{"messages":[{"content":"Be brief.","role":"system"},{"content":"Say hello.","role":"user"}],"model":"mock-model"}"#,
        "the request body is canonical JSON"
    );
}

#[test]
fn a_structured_generation_assigns_the_schema_valid_object() {
    let mock = Mock::start();
    let env = env_file("json", &format!("{}/json/v1", mock.origin));
    let config = config(&env, &["OPENAI_API_KEY", "OPENAI_BASE_URL"]);
    let handle = open(
        &config,
        &document(STRUCTURED_ELEMENT, "reply.answer + ':' + (reply.score + 1)"),
    );
    let records = until_closed(&handle, Duration::from_secs(60));
    assert_eq!(
        logs(&records),
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
fn a_reply_that_mismatches_the_schema_raises_error_execution_unassigned() {
    let mock = Mock::start();
    let env = env_file("mismatch", &format!("{}/mismatch/v1", mock.origin));
    let config = config(&env, &["OPENAI_API_KEY", "OPENAI_BASE_URL"]);
    let handle = open(&config, &document(STRUCTURED_ELEMENT, "typeof reply"));
    let records = until_closed(&handle, Duration::from_secs(60));
    assert_eq!(
        logs(&records),
        labelled(&[("final", "failed-unassigned")]),
        "the element fails before its log; nothing is assigned: {records:?}"
    );
    assert_eq!(mock.received().len(), 1);
}

#[test]
fn a_failing_backend_raises_error_execution() {
    let mock = Mock::start();
    let env = env_file("fail", &format!("{}/fail/v1", mock.origin));
    let config = config(&env, &["OPENAI_API_KEY", "OPENAI_BASE_URL"]);
    let handle = open(&config, &document(TEXT_ELEMENT, "reply"));
    let records = until_closed(&handle, Duration::from_secs(60));
    assert_eq!(
        logs(&records),
        labelled(&[("final", "failed-unassigned")]),
        "{records:?}"
    );
}

#[test]
fn without_a_granted_key_nothing_is_sent() {
    let mock = Mock::start();
    let env = env_file("nokey", &format!("{}/text/v1", mock.origin));
    // Only the base URL is granted: the apikey slot stays absent.
    let config = config(&env, &["OPENAI_BASE_URL"]);
    let handle = open(&config, &document(TEXT_ELEMENT, "reply"));
    let records = until_closed(&handle, Duration::from_secs(60));
    assert_eq!(
        logs(&records),
        labelled(&[("final", "failed-unassigned")]),
        "{records:?}"
    );
    assert!(mock.received().is_empty(), "no request without a key");
}

/// `OpenAI`'s base URL, the live run's config default (never compiled into
/// the component).
const OPENAI_BASE_URL_DEFAULT: &str = "https://api.openai.com/v1";

/// One real structured generation through an env file
/// (`NEMOBOT_LIVE_ENV_FILE`; `NEMOBOT_LIVE_MODEL`, default `gpt-4o-mini`).
/// Prints nothing but pass or fail.
#[test]
#[ignore = "makes one real model call; needs NEMOBOT_LIVE_ENV_FILE"]
fn live_generate_through_an_env_file() {
    let Some(path) = std::env::var_os("NEMOBOT_LIVE_ENV_FILE").map(PathBuf::from) else {
        panic!("set NEMOBOT_LIVE_ENV_FILE to an env file that sets OPENAI_API_KEY");
    };
    let model = std::env::var("NEMOBOT_LIVE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    // No backend URL is compiled in: the run's config supplies OpenAI's as
    // a non-secret default the file may override. The failure names the
    // kind only, never a value.
    let config = config_with_defaults(
        &path,
        &["OPENAI_API_KEY", "OPENAI_BASE_URL"],
        &serde_json::json!({"OPENAI_BASE_URL": OPENAI_BASE_URL_DEFAULT}),
    );
    if let Err(failure) = Values::granted(&path, &config.env, &config.env_defaults, &|_| None) {
        panic!(
            "the env file must set OPENAI_API_KEY ({:?})",
            failure.kind()
        );
    }
    let element = format!(
        r#"<gen:generate location="reply" model="{model}" schemaname="greeting" strict="true"
                    system="Answer in JSON only."
                    env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL">
        <gen:content>Greet a state machine in at most five words.</gen:content>
        <gen:schema>{{"type":"object","properties":{{"greeting":{{"type":"string"}}}},"required":["greeting"],"additionalProperties":false}}</gen:schema>
      </gen:generate>"#
    );
    let handle = open(
        &config,
        &document(
            &element,
            "typeof reply.greeting === 'string' &amp;&amp; reply.greeting.length &gt; 0",
        ),
    );
    let records = until_closed(&handle, Duration::from_secs(180));
    let pass = logs(&records) == labelled(&[("reply", "true"), ("final", "done")]);
    println!("live generate: {}", if pass { "pass" } else { "fail" });
    assert!(pass, "the live generation did not pass");
}
