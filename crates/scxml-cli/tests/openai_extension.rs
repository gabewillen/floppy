#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! The `openai` executable-content extension package (`namespaces/openai`) end to
//! end through the CLI (`wasmtime` feature): `validate` checks the example
//! against the package schema, and `snapshot run` executes the shipped behavior
//! component against an OpenAI-compatible mock this suite serves on 127.0.0.1,
//! reachable because the run supplies its base URL through the `baseurl` slot.
//!
//! The release components are built and verified once by
//! `scripts/verify-wasm-capabilities.sh` into this suite's own artifact
//! directory.

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest as _, Sha256};

const OPENAI_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0";

/// The session components plus the `openai` extension component.
const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_extension_openai",
    "scxml_compiler",
    "scxml_machine",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

struct Release {
    dir: String,
    manifest_sha256: String,
    openai_sha256: String,
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let workspace = workspace();
        let target = workspace.join("target/cli-components");
        let dir = target.join("wasm-artifacts/openai-namespace");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS)
            .current_dir(&workspace)
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", &dir)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest = fs::read_to_string(dir.join("manifest.tsv")).expect("manifest");
        // A repository namespace: the release manifest does not list it, so the
        // test pins the SHA-256 of the bytes the capability script verified.
        let openai = fs::read(dir.join("scxml_extension_openai.virtualized.wasm"))
            .expect("the openai component");
        let openai_sha256 = format!("{:x}", Sha256::digest(&openai));
        Release {
            dir: dir.display().to_string(),
            manifest_sha256: format!("{:x}", Sha256::digest(manifest.as_bytes())),
            openai_sha256,
        }
    })
}

fn package() -> String {
    workspace()
        .join("namespaces/openai/package/openai.extension.json")
        .display()
        .to_string()
}

fn example() -> String {
    example_named("hello")
}

fn example_named(name: &str) -> String {
    workspace()
        .join(format!("namespaces/openai/examples/{name}.scxml"))
        .display()
        .to_string()
}

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-openai-namespace")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn cli(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scxml-cli"));
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL");
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("scxml-cli must run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An OpenAI-compatible mock on 127.0.0.1: `POST /v1/responses` answers one
/// assistant message, `POST /call/v1/responses` one call of
/// `send_answer_yes`, `POST /fail/v1/responses` answers 500. Every request
/// line is recorded, and every request body in `bodies`.
struct Mock {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let body_log = Arc::clone(&bodies);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
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
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                log.lock()
                    .expect("the log lock")
                    .push(request_line.trim_end().to_owned());
                body_log.lock().expect("the body lock").push(body);
                let (status, answer) = match target.as_str() {
                    "/v1/responses" => (
                        "200 OK",
                        r#"{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello, machine."}]}]}"#,
                    ),
                    "/call/v1/responses" => (
                        "200 OK",
                        r#"{"output":[{"type":"function_call","call_id":"call_1","name":"send_answer_yes","arguments":"{}"}]}"#,
                    ),
                    "/fail/v1/responses" => ("500 Internal Server Error", r#"{"error":{}}"#),
                    _ => ("404 Not Found", "{}"),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Self {
            origin,
            requests,
            bodies,
        }
    }

    fn bodies(&self) -> Vec<serde_json::Value> {
        self.bodies
            .lock()
            .expect("the body lock")
            .iter()
            .map(|body| serde_json::from_slice(body).expect("a JSON request body"))
            .collect()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the log lock").clone()
    }
}

/// `snapshot run` of the example with the openai package bound, then the
/// rendered final snapshot (`snapshot show`).
fn run_example(name: &str, extra: &[&str], env: &[(&str, &str)]) -> (Output, String) {
    run_named_example("hello", name, extra, env)
}

fn run_named_example(
    example: &str,
    name: &str,
    extra: &[&str],
    env: &[(&str, &str)],
) -> (Output, String) {
    let release = release();
    let behavior = format!(
        "{OPENAI_URI}={}/scxml_extension_openai.virtualized.wasm",
        release.dir
    );
    let digest = format!("{OPENAI_URI}={}", release.openai_sha256);
    let package = package();
    let example = example_named(example);
    let output = scratch(name).join("final.snapshot.json");
    let output = output.display().to_string();
    let mut args = vec![
        "snapshot",
        "run",
        example.as_str(),
        "--components",
        release.dir.as_str(),
        "--components-manifest-sha256",
        release.manifest_sha256.as_str(),
        "--extension-package",
        package.as_str(),
        "--extension-component",
        behavior.as_str(),
        "--extension-component-sha256",
        digest.as_str(),
        "--output",
        output.as_str(),
    ];
    args.extend_from_slice(extra);
    let run = cli(&args, env);
    let shown = if run.status.success() {
        text(&cli(&["snapshot", "show", output.as_str()], &[]).stdout)
    } else {
        String::new()
    };
    (run, shown)
}

#[test]
fn the_example_validates_against_the_package_schema_and_unsupported_attributes_do_not() {
    let package = package();
    let valid = cli(
        &[
            "validate",
            example().as_str(),
            "--extension-package",
            package.as_str(),
        ],
        &[],
    );
    assert!(valid.status.success(), "{}", text(&valid.stderr));
    let directory = scratch("schema");
    let streaming = directory.join("stream.scxml");
    fs::write(
        &streaming,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:openai="{OPENAI_URI}" version="1.0" datamodel="ecmascript"><state id="s"><onentry><openai:generate model="m" prompt="p" location="x" stream="true"/></onentry></state></scxml>"#
        ),
    )
    .expect("fixture is writable");
    let rejected = cli(
        &[
            "validate",
            streaming.to_str().expect("UTF-8 path"),
            "--extension-package",
            package.as_str(),
        ],
        &[],
    );
    assert!(
        !rejected.status.success(),
        "stream is not part of this package version"
    );
}

#[test]
fn snapshot_run_stores_the_generated_reply_and_transitions_on_it() {
    let mock = Mock::start();
    let base = format!("{}/v1", mock.origin);
    let (run, shown) = run_example(
        "success",
        &["--env", "OPENAI_API_KEY", "--env", "OPENAI_BASE_URL"],
        &[("OPENAI_API_KEY", "sk-test"), ("OPENAI_BASE_URL", &base)],
    );
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(
        shown.contains("configuration: atomic [answered]"),
        "{shown}"
    );
    assert!(shown.contains(r#"reply = "Hello, machine.""#), "{shown}");
    assert_eq!(mock.requests(), ["POST /v1/responses HTTP/1.1"]);
}

/// Function-calling mode end to end: the request carries the session's
/// pruned snapshot and the events `ask` accepts, and the model's call of
/// `send_answer_yes` moves the machine to `yes`.
#[test]
fn snapshot_run_sends_the_event_the_model_called() {
    let mock = Mock::start();
    let base = format!("{}/call/v1", mock.origin);
    let (run, shown) = run_named_example(
        "decide",
        "decide",
        &["--env", "OPENAI_API_KEY", "--env", "OPENAI_BASE_URL"],
        &[("OPENAI_API_KEY", "sk-test"), ("OPENAI_BASE_URL", &base)],
    );
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(shown.contains("configuration: atomic [yes]"), "{shown}");
    let [body] = mock.bodies().try_into().expect("exactly one request");
    assert_eq!(body["tool_choice"], "required");
    let tools: Vec<&str> = body["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        tools,
        ["send_answer_yes", "send_answer_no", "send_error_execution"]
    );
    let system: serde_json::Value =
        serde_json::from_str(body["input"][0]["content"].as_str().expect("system text"))
            .expect("the system message is a snapshot document");
    assert_eq!(system["version"], "scxml.session-snapshot/v2");
    assert_eq!(system["options"]["point"], "executable-content");
    assert_eq!(
        system["snapshot"]["root"]["configuration"]["atomic"],
        serde_json::json!(["ask"])
    );
}

#[test]
fn snapshot_run_raises_error_execution_for_every_failure() {
    let mock = Mock::start();
    let ok_base = format!("{}/v1", mock.origin);
    let fail_base = format!("{}/fail/v1", mock.origin);
    // No scheme: the value supplies no origin, so the request is refused
    // before anything is sent.
    let not_a_url = ok_base.trim_start_matches("http://").to_owned();
    for (name, base, key, sent) in [
        ("status", fail_base.as_str(), true, 1),
        ("not-a-url", not_a_url.as_str(), true, 0),
        ("no-key", ok_base.as_str(), false, 0),
    ] {
        let before = mock.requests().len();
        let extra = ["--env", "OPENAI_API_KEY", "--env", "OPENAI_BASE_URL"];
        let mut env = vec![("OPENAI_BASE_URL", base)];
        if key {
            env.push(("OPENAI_API_KEY", "sk-test"));
        }
        let (run, shown) = run_example(name, &extra, &env);
        assert!(run.status.success(), "{name}: {}", text(&run.stderr));
        assert!(
            shown.contains("configuration: atomic [failed]"),
            "{name}: {shown}"
        );
        assert!(shown.contains(r#"reply = """#), "{name}: {shown}");
        assert_eq!(mock.requests().len() - before, sent, "{name}");
    }
}

#[test]
fn the_run_lists_what_the_element_consents_to_and_masks_the_values() {
    let mock = Mock::start();
    let base = format!("{}/v1", mock.origin);
    let (run, shown) = run_example(
        "summary",
        &[
            "--env",
            "OPENAI_API_KEY=sk-masked-key-123",
            "--env",
            "OPENAI_BASE_URL",
        ],
        &[("OPENAI_BASE_URL", &base)],
    );
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(
        shown.contains("configuration: atomic [answered]"),
        "{shown}"
    );
    let stderr = text(&run.stderr);
    assert!(
        stderr.contains(&format!("element `{OPENAI_URI}`")),
        "{stderr}"
    );
    assert!(
        stderr.contains("outbound HTTP: https://api.openai.com"),
        "{stderr}"
    );
    assert!(
        stderr.contains("apikey <- OPENAI_API_KEY")
            && stderr.contains("baseurl <- OPENAI_BASE_URL"),
        "{stderr}"
    );
    assert!(!stderr.contains("sk-masked-key-123"), "{stderr}");
    let [body] = mock.bodies().try_into().expect("one request");
    assert!(
        !body.to_string().contains("sk-masked-key-123"),
        "the key is a header, never the body"
    );
}

#[test]
fn a_missing_value_names_the_env_flag_and_removed_flags_are_refused() {
    let (run, shown) = run_example("missing", &[], &[]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(shown.contains("configuration: atomic [failed]"), "{shown}");
    assert!(
        text(&run.stderr).contains("apikey <- OPENAI_API_KEY (no value: add --env OPENAI_API_KEY)"),
        "{}",
        text(&run.stderr)
    );
    for removed in [
        vec!["--extension-http-allow", "https://api.openai.com"],
        vec!["--inherit-env"],
        vec!["--env", &format!("{OPENAI_URI}=OPENAI_API_KEY")],
    ] {
        let (refused, _) = run_example("removed", &removed, &[]);
        assert_eq!(refused.status.code(), Some(2), "{removed:?}");
    }
}
