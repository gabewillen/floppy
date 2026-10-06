#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! `env` values end to end through the CLI (`wasmtime` feature), through the
//! example extension's `<example:fetch>` (`components/scxml-extension-example`,
//! whose package declares the `url` and `token` slots) against a loopback
//! server: each element receives references for its own slots only, a value
//! reaches only requests to an origin its package declares or one a value
//! supplies entirely, a variable goes only to the elements and `<invoke>`s
//! whose `env` attributes name it, and `--env-file` values are masked in what
//! the CLI writes. No test touches
//! the real network.

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest as _, Sha256};

const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";
const ECHO_URI: &str = "https://stateforward.org/scxml/services/echo";
const ENV_NAMESPACE: &str = "https://github.com/stateforward/scxml.rs/namespaces/env";

const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_extension_example",
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

/// The example element bound with `--extension-package`: its manifest
/// declares `declared` as its outbound origins (written to `directory`).
fn example_package(directory: &Path, declared: &[&str]) -> Vec<String> {
    let source = workspace().join("components/scxml-extension-example/package");
    let package = directory.join(format!("example-{}", declared.len()));
    fs::create_dir_all(&package).expect("the package directory");
    fs::copy(source.join("example.xsd"), package.join("example.xsd")).expect("the schema");
    let manifest = fs::read_to_string(source.join("example.extension.json"))
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
    let manifest_path = package.join("example.extension.json");
    fs::write(&manifest_path, manifest).expect("the manifest");
    strings(&[
        "--extension-package",
        &manifest_path.display().to_string(),
        "--extension-component",
        &format!(
            "{EXAMPLE_URI}={}/scxml_extension_example.virtualized.wasm",
            release().dir
        ),
        "--extension-component-sha256",
        &format!(
            "{EXAMPLE_URI}={}",
            component_sha256("scxml_extension_example.virtualized.wasm")
        ),
    ])
}

/// A document whose first element (A) fetches with `a_attributes`, then
/// whose second element (B) fetches with `b_attributes`, assigning their
/// outcomes to `first` and `outcome`. `invoke` is placed in the first state.
fn two_elements(directory: &Path, a_attributes: &str, b_attributes: &str, invoke: &str) -> String {
    let document = directory.join(format!(
        "two-{:x}.scxml",
        Sha256::digest(format!("{a_attributes}{b_attributes}{invoke}").as_bytes())
    ));
    fs::write(
        &document,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" xmlns:env="{ENV_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="a">
  <datamodel><data id="first" expr="''"/><data id="outcome" expr="''"/></datamodel>
  <state id="a">
    {invoke}
    <onentry><example:fetch {a_attributes} location="first"/></onentry>
    <transition event="example.fetched" target="b"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <state id="b">
    <onentry><example:fetch {b_attributes} location="outcome"/></onentry>
    <transition event="example.fetched" target="fetched"/>
    <transition event="error.execution" target="refused"/>
  </state>
  <final id="fetched"/>
  <final id="refused"/>
  <final id="failed"/>
</scxml>"#
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

/// Each element receives references for its own slots only, and a value is
/// substituted only into a request to an origin the element's package
/// declares or the run supplied. The component never holds a value: it
/// reports only the slot names it was handed.
#[test]
fn values_reach_only_their_element_and_only_admitted_origins() {
    let server = Server::start();
    let directory = scratch("isolation");
    let output = directory.join("final.snapshot.json");
    let origin = server.origin.clone();
    let echo = format!("{origin}/echo");
    let grants = ["KEY=key-test".to_owned(), format!("A_BASE={origin}")];
    // A's url slot names a value that is a whole origin, so its request is
    // admitted though nothing declares it, carrying A's token.
    let a = r#"env:url="A_BASE" path="/echo" env:token="KEY""#;
    let run = |document: &str, declared: &[&str], extra: &[&str]| {
        let mut flags = strings(&["--env", &grants[0], "--env", &grants[1]]);
        flags.extend(example_package(&directory, declared));
        flags.extend(strings(extra));
        snapshot_run(document, &output, &flags)
    };

    // B binds no slot: it gets no reference, though A bound KEY, and its
    // request to a literal origin nothing declares is refused.
    let (refused, shown) = run(
        &two_elements(&directory, a, &format!(r#"url="{echo}""#), ""),
        &[],
        &[],
    );
    assert!(refused.status.success(), "{}", text(&refused.stderr));
    assert!(shown.contains("configuration: atomic [refused]"), "{shown}");
    assert!(
        shown.contains(r#"first = "status=200 slots=url,token""#),
        "{shown}"
    );
    assert!(
        shown.contains(r#"outcome = "status=refused slots=""#),
        "{shown}"
    );
    assert_eq!(
        server.authorizations(),
        ["Bearer key-test"],
        "A's request carried its value"
    );
    assert_eq!(server.echoes(), 1, "B's request never left the host");
    // The origin is A_BASE's value, so the note masks it too.
    assert!(
        text(&refused.stderr).contains(&format!(
            "`{EXAMPLE_URI}` was refused outbound HTTP to `[masked]`"
        )),
        "{}",
        text(&refused.stderr)
    );

    // B names KEY for its own token slot toward an undeclared origin:
    // refused before anything is sent, so the key never leaves through B.
    let toward = two_elements(
        &directory,
        a,
        &format!(r#"url="{echo}" env:token="KEY""#),
        "",
    );
    let (refused, shown) = run(&toward, &[], &[]);
    assert!(refused.status.success(), "{}", text(&refused.stderr));
    assert!(
        shown.contains(r#"outcome = "status=refused slots=token""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 2, "only A's request was sent");

    // The same element whose package declares the origin: admitted, the
    // reference substituted by the host.
    let (admitted, shown) = run(&toward, &[&origin], &[]);
    assert!(admitted.status.success(), "{}", text(&admitted.stderr));
    assert!(shown.contains("configuration: atomic [fetched]"), "{shown}");
    assert!(
        shown.contains(r#"outcome = "status=200 slots=token""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 4);
    assert_eq!(
        server.authorizations().last().map(String::as_str),
        Some("Bearer key-test"),
        "the host substituted the value into the admitted request"
    );

    // A literal that extends the authority a value starts (the port here)
    // is no run-supplied origin: refused.
    let port = origin.rsplit(':').next().expect("a port").to_owned();
    let (split, shown) = run(
        &two_elements(
            &directory,
            a,
            &format!(r#"path=":{port}/echo" env:url="HOST_ONLY""#),
            "",
        ),
        &[],
        &["--env", "HOST_ONLY=http://127.0.0.1"],
    );
    assert!(split.status.success(), "{}", text(&split.stderr));
    assert!(
        shown.contains(r#"outcome = "status=refused slots=url""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 5, "only A's request was sent");
}

/// A variable goes only to the elements and `<invoke>`s whose `env`
/// attributes name it: the echo service's `<invoke>` names `KEY`, the
/// example element names `OTHER` (which the run does not hold) for its
/// token, so the example element's slot is absent even toward an origin its
/// package declares, and the listing shows each variable and where it goes.
#[test]
fn a_variable_goes_only_to_the_elements_naming_it() {
    let server = Server::start();
    let directory = scratch("scoped");
    let output = directory.join("final.snapshot.json");
    let echo_manifest =
        workspace().join("components/scxml-service-example/package/echo.service.json");
    let mut flags = strings(&[
        "--env",
        "KEY=key-test",
        "--service-package",
        &echo_manifest.display().to_string(),
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
    ]);
    flags.extend(example_package(&directory, &[&server.origin]));
    let document = two_elements(
        &directory,
        &format!(r#"url="{}" path="/echo" env:token="OTHER""#, server.origin),
        &format!(r#"url="{}/echo""#, server.origin),
        &format!(r#"<invoke type="{ECHO_URI}" id="echo" env:endpoint="KEY"/>"#),
    );
    let (out_of_scope, shown) = snapshot_run(&document, &output, &flags);
    let stderr = text(&out_of_scope.stderr);
    assert!(out_of_scope.status.success(), "{stderr}");
    assert!(shown.contains(r#"first = "status=200 slots=""#), "{shown}");
    assert!(
        server.authorizations().is_empty(),
        "no request carried the value"
    );
    assert!(stderr.contains("token <- OTHER (no value)"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "env `KEY`\n  = to: <invoke type=\"{ECHO_URI}\"> env:endpoint"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "env `OTHER` (no value)\n  = to: <fetch> of `{EXAMPLE_URI}` env:token"
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("key-test"), "never a value: {stderr}");
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
    fs::write(&env_file, "# the key\r\nKEY=sk-file-secret-1\r\n").expect("the env file");
    let document = directory.join("leak.scxml");
    fs::write(
        &document,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" xmlns:env="{ENV_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="a">
  <state id="a">
    <onentry><example:fetch url="{}" path="/echo" env:token="KEY"/><log label="leak" expr="'echo sk-file-secret-1'"/></onentry>
    <transition event="example.fetched" target="done"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="done"/>
  <final id="failed"/>
</scxml>"#,
            server.origin
        ),
    )
    .expect("the document");
    let mut args = strings(&[
        "run",
        &document.display().to_string(),
        "--components",
        &release.dir,
        "--components-manifest-sha256",
        &release.manifest_sha256,
        "--clock",
        "logical",
        "--on-eof",
        "close",
        "--env-file",
        &env_file.display().to_string(),
    ]);
    args.extend(example_package(&directory, &[&server.origin]));
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
    let mut flags = strings(&["--env", "OTHER=x-value"]);
    flags.extend(example_package(&directory, &[&server.origin]));
    let (missing, shown) = snapshot_run(&document.display().to_string(), &output, &flags);
    assert!(missing.status.success(), "{}", text(&missing.stderr));
    assert!(shown.contains("configuration: atomic [done]"), "{shown}");
    assert!(
        text(&missing.stderr).contains("token <- KEY (no value)"),
        "{}",
        text(&missing.stderr)
    );
}
