#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `env` values end to end through the CLI (`wasmtime` feature), through the
//! echo service (`components/scxml-service-example`, whose package declares
//! the `endpoint` and `token` slots) against a loopback server: a value
//! reaches only requests to an origin the service's package declares or one
//! a value supplies entirely, a variable goes only to the `<invoke>`s whose
//! `env` attributes name it, an extension element has no slots (`SCXML084`),
//! and `--env-file` values are masked in what the CLI writes. No test
//! touches the real network.

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest as _, Sha256};

const ECHO_URI: &str = "https://stateforward.org/scxml/services/echo";
const ENV_NAMESPACE: &str = "https://github.com/stateforward/scxml.rs/namespaces/env";

const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_service_example",
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
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let workspace = workspace();
        let target = workspace.join("target/cli-components");
        let dir = target.join("wasm-artifacts/env-values");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS)
            .current_dir(&workspace)
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", &dir)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest = fs::read(dir.join("manifest.tsv")).expect("manifest");
        Release {
            dir: dir.display().to_string(),
            manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
        }
    })
}

fn component_sha256(file: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(Path::new(&release().dir).join(file)).expect("the component"))
    )
}

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-env-values")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn cli(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scx"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("scx must run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// A loopback server answering `200` to every request; it records each
/// request line and `authorization` header.
struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    authorizations: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
        let origin = format!("http://{}", listener.local_addr().expect("an address"));
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let authorizations: Arc<Mutex<Vec<String>>> = Arc::default();
        let (log, seen) = (Arc::clone(&requests), Arc::clone(&authorizations));
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
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap_or(0);
                        } else if name.eq_ignore_ascii_case("authorization") {
                            seen.lock()
                                .expect("the header lock")
                                .push(value.trim().to_owned());
                        }
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                log.lock()
                    .expect("the log lock")
                    .push(request_line.trim_end().to_owned());
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
            }
        });
        Self {
            origin,
            requests,
            authorizations,
        }
    }

    fn echoes(&self) -> usize {
        self.requests.lock().expect("the log lock").len()
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("the header lock").clone()
    }
}

/// The echo service bound with `--service-package`: its manifest declares
/// `declared` as its outbound origins (written to `directory`).
fn echo_package(directory: &Path, declared: &[&str]) -> Vec<String> {
    let source = workspace().join("components/scxml-service-example/package");
    let package = directory.join(format!("echo-{}", declared.len()));
    fs::create_dir_all(&package).expect("the package directory");
    let manifest = fs::read_to_string(source.join("echo.service.json"))
        .expect("the manifest")
        .replace(
            r#""http-origins": []"#,
            &format!(
                r#""http-origins": [{}]"#,
                declared
                    .iter()
                    .map(|origin| format!(r#""{origin}""#))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    let manifest_path = package.join("echo.service.json");
    fs::write(&manifest_path, manifest).expect("the manifest");
    strings(&[
        "--service-package",
        &manifest_path.display().to_string(),
        "--service-component",
        &format!(
            "{ECHO_URI}={}/scxml_service_example.virtualized.wasm",
            release().dir
        ),
        "--service-component-sha256",
        &format!(
            "{ECHO_URI}={}",
            component_sha256("scxml_service_example.virtualized.wasm")
        ),
    ])
}

/// A document that invokes the echo service with `attributes` and has it
/// GET its `endpoint` slot's URL followed by `path`: it ends in `fetched`
/// (the status in `status`) or, when the request is refused or fails, in
/// `refused`. `extra` goes into the invoking state.
fn echo_document(directory: &Path, attributes: &str, path: &str, extra: &str) -> String {
    let document = directory.join(format!(
        "echo-{:x}.scxml",
        Sha256::digest(format!("{attributes}{path}{extra}").as_bytes())
    ));
    fs::write(
        &document,
        format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="{ENV_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="a">
  <datamodel><data id="status" expr="''"/></datamodel>
  <state id="a">
    <onentry><send event="go"/></onentry>
    <invoke type="{ECHO_URI}" id="echo" {attributes}/>
    {extra}
    <transition event="go"><send event="fetch" target="#_echo"><content expr="'{path}'"/></send></transition>
    <transition event="echo.fetched" target="fetched"><assign location="status" expr="_event.data"/></transition>
    <transition event="error.communication" target="refused"/>
  </state>
  <final id="fetched"/>
  <final id="refused"/>
</scxml>"##
        ),
    )
    .expect("the document");
    document.display().to_string()
}

/// `snapshot run` of `document`; the rendered final snapshot when it
/// succeeded.
fn snapshot_run(document: &str, output: &Path, extra: &[String]) -> (Output, String) {
    let release = release();
    let output = output.display().to_string();
    let mut args = strings(&[
        "snapshot",
        "run",
        document,
        "--components",
        &release.dir,
        "--components-manifest-sha256",
        &release.manifest_sha256,
        "--output",
        &output,
    ]);
    args.extend_from_slice(extra);
    let run = cli(&args);
    let shown = if run.status.success() {
        text(&cli(&strings(&["snapshot", "show", &output])).stdout)
    } else {
        String::new()
    };
    (run, shown)
}

/// A value is substituted only into a request to an origin the service's
/// package declares or the run supplied entirely (the `endpoint` value): a
/// path that extends the authority a value starts is refused before it is
/// sent, unless the package declares the resulting origin.
#[test]
fn values_reach_only_admitted_origins() {
    let server = Server::start();
    let directory = scratch("admission");
    let output = directory.join("final.snapshot.json");
    let origin = server.origin.clone();
    let port = origin.rsplit(':').next().expect("a port").to_owned();
    let slots = r#"env:endpoint="BASE" env:token="KEY""#;
    let run = |base: &str, path: &str, declared: &[&str]| {
        let mut flags = strings(&["--env", "KEY=key-test", "--env", &format!("BASE={base}")]);
        flags.extend(echo_package(&directory, declared));
        snapshot_run(&echo_document(&directory, slots, path, ""), &output, &flags)
    };

    // The value is a whole origin: admitted though nothing declares it,
    // carrying the token.
    let (admitted, shown) = run(&origin, "/echo", &[]);
    assert!(admitted.status.success(), "{}", text(&admitted.stderr));
    assert!(shown.contains("configuration: atomic [fetched]"), "{shown}");
    assert!(shown.contains("status = 200"), "{shown}");
    assert_eq!(server.authorizations(), ["Bearer key-test"]);
    assert_eq!(server.echoes(), 1);

    // A literal that extends the authority a value starts (the port): no
    // run-supplied origin, refused before anything is sent.
    let (refused, shown) = run("http://127.0.0.1", &format!(":{port}/echo"), &[]);
    assert!(refused.status.success(), "{}", text(&refused.stderr));
    assert!(shown.contains("configuration: atomic [refused]"), "{shown}");
    assert_eq!(server.echoes(), 1, "the request never left the host");

    // The same request with the origin declared by the package: admitted.
    let (declared, shown) = run("http://127.0.0.1", &format!(":{port}/echo"), &[&origin]);
    assert!(declared.status.success(), "{}", text(&declared.stderr));
    assert!(shown.contains("configuration: atomic [fetched]"), "{shown}");
    assert_eq!(server.echoes(), 2);
    assert_eq!(
        server.authorizations().last().map(String::as_str),
        Some("Bearer key-test"),
        "the host substituted the value into the admitted request"
    );
}

/// A variable goes only to the `<invoke>`s whose `env` attributes name it:
/// the token slot names `OTHER`, which the run does not hold, so it is
/// absent, and the listing shows each variable and where it goes. An `env`
/// attribute on an extension element is rejected (`SCXML084`): executable
/// content has no slots.
#[test]
fn a_variable_goes_only_to_the_invokes_naming_it() {
    let server = Server::start();
    let directory = scratch("scoped");
    let output = directory.join("final.snapshot.json");
    let mut flags = strings(&["--env", &format!("KEY={}", server.origin)]);
    flags.extend(echo_package(&directory, &[]));
    let document = echo_document(
        &directory,
        r#"env:endpoint="KEY" env:token="OTHER""#,
        "/echo",
        "",
    );
    let (out_of_scope, shown) = snapshot_run(&document, &output, &flags);
    let stderr = text(&out_of_scope.stderr);
    assert!(out_of_scope.status.success(), "{stderr}");
    assert!(shown.contains("configuration: atomic [fetched]"), "{shown}");
    assert!(
        server.authorizations().is_empty(),
        "no request carried a token"
    );
    assert!(
        stderr.contains(&format!(
            "env `KEY`\n  = to: <invoke type=\"{ECHO_URI}\"> env:endpoint"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "env `OTHER` (no value)\n  = to: <invoke type=\"{ECHO_URI}\"> env:token"
        )),
        "{stderr}"
    );
    assert!(!stderr.contains(&server.origin), "never a value: {stderr}");

    let element = directory.join("element.scxml");
    fs::write(
        &element,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:scx="https://github.com/stateforward/scxml.rs/namespaces/scx" xmlns:env="{ENV_NAMESPACE}" version="1.0" initial="a"><state id="a"><onentry><scx:validate location="r" env:token="KEY"><scx:content>x</scx:content></scx:validate></onentry></state></scxml>"#
        ),
    )
    .expect("the document");
    let rejected = cli(&strings(&["validate", &element.display().to_string()]));
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        text(&rejected.stderr)
    );
    assert!(
        text(&rejected.stderr).contains("SCXML084"),
        "{}",
        text(&rejected.stderr)
    );
}

/// Values from `--env-file` (Docker's format, CRLF included), masked in what
/// the CLI writes as text (here a live session's log record), and a missing
/// one listed as holding no value.
#[test]
fn env_files_supply_values_that_are_masked_in_cli_output() {
    let release = release();
    let server = Server::start();
    let directory = scratch("env-file");
    let env_file = directory.join(".env");
    fs::write(
        &env_file,
        format!(
            "# the key\r\nKEY=sk-file-secret-1\r\nBASE={}\r\n",
            server.origin
        ),
    )
    .expect("the env file");
    let document = echo_document(
        &directory,
        r#"env:endpoint="BASE" env:token="KEY""#,
        "/echo",
        r#"<onentry><log label="leak" expr="'echo sk-file-secret-1'"/></onentry>"#,
    );
    let mut args = strings(&[
        "run",
        &document,
        "--components",
        &release.dir,
        "--components-manifest-sha256",
        &release.manifest_sha256,
        "--clock",
        "logical",
        "--on-eof",
        "wait",
        "--env-file",
        &env_file.display().to_string(),
    ]);
    args.extend(echo_package(&directory, &[]));
    let live = cli(&args);
    let stdout = text(&live.stdout);
    assert_eq!(
        live.status.code(),
        Some(0),
        "{}{stdout}",
        text(&live.stderr)
    );
    assert!(stdout.contains(r#""text":"echo [masked]""#), "{stdout}");
    assert!(!stdout.contains("sk-file-secret-1"), "{stdout}");
    assert!(!text(&live.stderr).contains("sk-file-secret-1"));
    assert_eq!(
        server.authorizations(),
        ["Bearer sk-file-secret-1"],
        "the value reached the admitted request as a header"
    );

    // Without the file the key has no value: the listing says so.
    let output = directory.join("final.snapshot.json");
    let mut flags = strings(&["--env", &format!("BASE={}", server.origin)]);
    flags.extend(echo_package(&directory, &[]));
    let (missing, shown) = snapshot_run(&document, &output, &flags);
    assert!(missing.status.success(), "{}", text(&missing.stderr));
    assert!(shown.contains("configuration: atomic [fetched]"), "{shown}");
    assert!(
        text(&missing.stderr).contains("env `KEY` (no value)"),
        "{}",
        text(&missing.stderr)
    );
}
