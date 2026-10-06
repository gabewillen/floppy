#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! Repository namespaces end to end through the CLI (`wasmtime` feature): a
//! document binds `https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0`
//! and nothing else, and `validate` and `snapshot run` resolve it from a mock
//! GitHub this suite serves on 127.0.0.1 (`--github-api-base`,
//! `--github-raw-base`, `--github-download-base`): the tag through the API,
//! the manifest and schema at the commit, and, when the session starts, the
//! release asset authenticated by the release's `SHA256SUMS`. The same server
//! answers the element's OpenAI-compatible requests. No test touches the real
//! network.
//!
//! The release components are built and verified once by
//! `scripts/verify-wasm-capabilities.sh` into this suite's own artifact
//! directory; the openai component bytes the mock release serves are the ones
//! it verified.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest as _, Sha256};

const OPENAI_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0";
const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";
const ENV_NAMESPACE: &str = "https://github.com/stateforward/scxml.rs/namespaces/env";
const ASSET: &str = "scxml_extension_openai.virtualized.wasm";
/// The commit the mock tag names (through an annotated tag object).
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const TAG_OBJECT: &str = "fedcba9876543210fedcba9876543210fedcba98";
const REPO: &str = "stateforward/scxml.rs";

/// The session components plus the extension components the mock release
/// serves (openai) and the suite binds explicitly (example).
const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_extension_example",
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
    example_sha256: String,
    openai: Vec<u8>,
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let workspace = workspace();
        let target = workspace.join("target/cli-components");
        let dir = target.join("wasm-artifacts/repository-namespaces");
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
        let example_sha256 = manifest
            .lines()
            .find(|line| line.starts_with("artifact\tscxml_extension_example\t"))
            .and_then(|line| line.split('\t').nth(4))
            .expect("the manifest records the example component")
            .to_owned();
        Release {
            dir: dir.display().to_string(),
            manifest_sha256: format!("{:x}", Sha256::digest(manifest.as_bytes())),
            example_sha256,
            openai: fs::read(dir.join(ASSET)).expect("the openai component"),
        }
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-repository-namespaces")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn cli(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scxml-cli"))
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .output()
        .expect("scxml-cli must run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// One mock answer.
#[derive(Clone)]
enum Route {
    Body(Vec<u8>),
    Redirect(String),
    /// A 200 that declares this `content-length` and sends no body.
    Declared(u64),
}

/// A mock GitHub (API, raw files, releases) and OpenAI-compatible service on
/// 127.0.0.1. Routes are exact paths; every request line is recorded.
struct Server {
    origin: String,
    routes: Arc<Mutex<BTreeMap<String, Route>>>,
    requests: Arc<Mutex<Vec<String>>>,
    /// Every `authorization` header value received, in order.
    authorizations: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let routes: Arc<Mutex<BTreeMap<String, Route>>> = Arc::default();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let authorizations: Arc<Mutex<Vec<String>>> = Arc::default();
        let (served, log) = (Arc::clone(&routes), Arc::clone(&requests));
        let seen = Arc::clone(&authorizations);
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
                let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                log.lock()
                    .expect("the log lock")
                    .push(request_line.trim_end().to_owned());
                let route = served.lock().expect("the route lock").get(&target).cloned();
                let _ = match route {
                    Some(Route::Body(body)) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        stream.write_all(&body)
                    }
                    Some(Route::Declared(length)) => write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
                    ),
                    Some(Route::Redirect(location)) => write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    ),
                    None => write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    ),
                };
            }
        });
        Self {
            origin,
            routes,
            requests,
            authorizations,
        }
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().expect("the header lock").clone()
    }

    fn echoes(&self) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.starts_with("GET /echo"))
            .count()
    }

    fn route(&self, path: &str, route: Route) {
        self.routes
            .lock()
            .expect("the route lock")
            .insert(path.to_owned(), route);
    }

    fn body(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.route(path, Route::Body(body.into()));
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the log lock").clone()
    }

    /// Requests to the mock GitHub (everything but the `OpenAI` API).
    fn github_requests(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|request| !request.contains("/v1/responses") && !request.contains("/echo"))
            .collect()
    }

    fn generations(&self) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.starts_with("POST /v1/responses"))
            .count()
    }
}

/// The shipped openai package files.
fn package_file(name: &str) -> Vec<u8> {
    fs::read(workspace().join("namespaces/openai/package").join(name)).expect("package file")
}

fn raw_path(file: &str) -> String {
    format!("/raw/{REPO}/{COMMIT}/namespaces/openai/package/{file}")
}

fn download_path(asset: &str) -> String {
    format!("/dl/{REPO}/releases/download/v0.2.0/{asset}")
}

/// A mock GitHub serving the openai element exactly as a release would:
/// `v0.2.0` is an annotated tag of [`COMMIT`], the package files sit at the
/// commit, and the release serves the component (through a redirect, like
/// GitHub's asset host) and its `SHA256SUMS`. It also answers the element's
/// generation request.
fn github() -> Server {
    let release = release();
    let server = Server::start();
    server.body(
        &format!("/api/repos/{REPO}/git/ref/tags/v0.2.0"),
        format!(r#"{{"ref":"refs/tags/v0.2.0","object":{{"type":"tag","sha":"{TAG_OBJECT}"}}}}"#),
    );
    server.body(
        &format!("/api/repos/{REPO}/git/tags/{TAG_OBJECT}"),
        format!(r#"{{"object":{{"type":"commit","sha":"{COMMIT}"}}}}"#),
    );
    server.body(
        &raw_path("openai.extension.json"),
        package_file("openai.extension.json"),
    );
    server.body(&raw_path("openai.xsd"), package_file("openai.xsd"));
    server.body(
        &download_path("SHA256SUMS"),
        format!(
            "{}  {ASSET}\n{}  manifest.tsv\n",
            sha256(&release.openai),
            "0".repeat(64)
        ),
    );
    server.route(
        &download_path(ASSET),
        Route::Redirect(format!("{}/assets/openai", server.origin)),
    );
    server.body("/assets/openai", release.openai.clone());
    server.body("/echo", "echo");
    server.body(
        "/v1/responses",
        r#"{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello, machine."}]}]}"#,
    );
    server
}

/// The repository flags pointing at `server`, with `cache` as the cache.
fn repository(server: &Server, cache: &Path) -> Vec<String> {
    vec![
        "--github-api-base".into(),
        format!("{}/api", server.origin),
        "--github-raw-base".into(),
        format!("{}/raw", server.origin),
        "--github-download-base".into(),
        format!("{}/dl", server.origin),
        "--cache-dir".into(),
        cache.display().to_string(),
    ]
}

/// The values the openai element's slots name: its key, and a base URL on
/// `server` (which supplies the request's origin).
fn openai_grants(server: &Server) -> Vec<String> {
    vec![
        "--env".into(),
        format!("OPENAI_API_KEY@{OPENAI_URI}=sk-test"),
        "--env".into(),
        format!("OPENAI_BASE_URL@{OPENAI_URI}={}/v1", server.origin),
    ]
}

fn hello() -> String {
    workspace()
        .join("namespaces/openai/examples/hello.scxml")
        .display()
        .to_string()
}

/// `snapshot run` of `document` with `extra` flags; the rendered final
/// snapshot when it succeeded.
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

fn validate(document: &str, extra: &[String]) -> Output {
    let mut args = strings(&["validate", document]);
    args.extend_from_slice(extra);
    cli(&args)
}

#[test]
fn a_repository_element_loads_at_session_start_and_then_runs_offline_from_the_cache() {
    let server = github();
    let directory = scratch("happy");
    let cache = directory.join("cache");
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &cache);
    flags.extend(openai_grants(&server));

    let (first, shown) = snapshot_run(&hello(), &output, &flags);
    assert!(first.status.success(), "{}", text(&first.stderr));
    assert!(
        shown.contains("configuration: atomic [answered]"),
        "{shown}"
    );
    assert!(shown.contains(r#"reply = "Hello, machine.""#), "{shown}");
    let fetched = server.github_requests();
    assert_eq!(
        fetched,
        [
            format!("GET /api/repos/{REPO}/git/ref/tags/v0.2.0 HTTP/1.1"),
            format!("GET /api/repos/{REPO}/git/tags/{TAG_OBJECT} HTTP/1.1"),
            format!("GET {} HTTP/1.1", raw_path("openai.extension.json")),
            format!("GET {} HTTP/1.1", raw_path("openai.xsd")),
            format!("GET {} HTTP/1.1", download_path("SHA256SUMS")),
            format!("GET {} HTTP/1.1", download_path(ASSET)),
            "GET /assets/openai HTTP/1.1".to_owned(),
        ],
        "the tag resolves to its commit, the package comes from that commit (never the tag), and the component from the release"
    );
    // The exact bytes are cached by digest, and the URI is pinned.
    assert!(
        cache
            .join("sha256")
            .join(sha256(&release().openai))
            .is_file()
    );
    let index = cache
        .join("index")
        .join(format!("{}.json", sha256(OPENAI_URI.as_bytes())));
    let pinned = fs::read_to_string(&index).expect("the URI is pinned");
    assert!(pinned.contains(COMMIT), "{pinned}");

    // Offline: the same run from the cache alone.
    let mut offline = flags.clone();
    offline.push("--offline".into());
    let (second, shown) = snapshot_run(&hello(), &output, &offline);
    assert!(second.status.success(), "{}", text(&second.stderr));
    assert!(
        shown.contains("configuration: atomic [answered]"),
        "{shown}"
    );
    assert_eq!(
        server.github_requests(),
        fetched,
        "offline touches no GitHub URL"
    );

    // Online again: the pin holds even when the tag moves.
    server.body(
        &format!("/api/repos/{REPO}/git/tags/{TAG_OBJECT}"),
        format!(
            r#"{{"object":{{"type":"commit","sha":"{}"}}}}"#,
            "f".repeat(40)
        ),
    );
    let (third, _) = snapshot_run(&hello(), &output, &flags);
    assert!(third.status.success(), "{}", text(&third.stderr));
    assert_eq!(
        server.github_requests(),
        fetched,
        "a pinned URI asks no network"
    );
    assert_eq!(server.generations(), 3);
}

#[test]
fn validate_resolves_the_schema_and_never_the_component() {
    let server = github();
    let directory = scratch("validate");
    let flags = repository(&server, &directory.join("cache"));
    let valid = validate(&hello(), &flags);
    assert!(valid.status.success(), "{}", text(&valid.stderr));
    let invalid_document = directory.join("invalid.scxml");
    fs::write(
        &invalid_document,
        fs::read_to_string(hello()).expect("the example").replace(
            "max-output-tokens=\"64\"",
            "max-output-tokens=\"64\" stream=\"true\"",
        ),
    )
    .expect("the invalid copy");
    let invalid = validate(&invalid_document.display().to_string(), &flags);
    assert_eq!(
        invalid.status.code(),
        Some(1),
        "the package schema rejects an undeclared attribute: {}",
        text(&invalid.stderr)
    );
    assert!(
        !server
            .github_requests()
            .iter()
            .any(|request| request.contains("/releases/")),
        "validation needs only the manifest and schema"
    );
}

#[test]
fn a_package_that_does_not_name_itself_exactly_fails_closed() {
    for (name, manifest, schema, expected) in [
        (
            "uri",
            String::from_utf8(package_file("openai.extension.json"))
                .expect("UTF-8")
                .replace("@v0.2.0", "@v0.1.0"),
            package_file("openai.xsd"),
            "not this URI",
        ),
        (
            "schema-digest",
            String::from_utf8(package_file("openai.extension.json")).expect("UTF-8"),
            [package_file("openai.xsd"), b"<!-- changed -->".to_vec()].concat(),
            "schema-sha256 does not match",
        ),
        {
            let schema = String::from_utf8(package_file("openai.xsd"))
                .expect("UTF-8")
                .replace(
                    &format!("targetNamespace=\"{OPENAI_URI}\""),
                    "targetNamespace=\"https://example.org/other\"",
                )
                .into_bytes();
            let manifest = String::from_utf8(package_file("openai.extension.json"))
                .expect("UTF-8")
                .replace(&sha256(&package_file("openai.xsd")), &sha256(&schema));
            ("target-namespace", manifest, schema, "targetNamespace")
        },
    ] {
        let server = github();
        server.body(&raw_path("openai.extension.json"), manifest);
        server.body(&raw_path("openai.xsd"), schema);
        let cache = scratch(name).join("cache");
        let rejected = validate(&hello(), &repository(&server, &cache));
        assert_eq!(rejected.status.code(), Some(2), "{name}");
        assert!(
            text(&rejected.stderr).contains(expected),
            "{name}: {}",
            text(&rejected.stderr)
        );
        assert!(!cache.join("index").exists(), "{name}: nothing is pinned");
    }
}

#[test]
fn a_component_its_release_does_not_vouch_for_fails_the_session_start() {
    let release = release();
    for (name, sums, expected) in [
        (
            "missing",
            format!("{}  other.wasm\n", sha256(&release.openai)),
            "lists no single SHA-256",
        ),
        (
            "mismatch",
            format!("{}  {ASSET}\n", "0".repeat(64)),
            "does not match the SHA-256",
        ),
        (
            "twice",
            format!("{0}  {ASSET}\n{0}  {ASSET}\n", sha256(&release.openai)),
            "lists no single SHA-256",
        ),
    ] {
        let server = github();
        server.body(&download_path("SHA256SUMS"), sums);
        let directory = scratch(name);
        let output = directory.join("final.snapshot.json");
        let mut flags = repository(&server, &directory.join("cache"));
        flags.extend(openai_grants(&server));
        let (run, _) = snapshot_run(&hello(), &output, &flags);
        assert!(!run.status.success(), "{name}");
        assert!(
            text(&run.stderr).contains(expected),
            "{name}: {}",
            text(&run.stderr)
        );
        assert!(!output.exists(), "{name}: no snapshot is written");
        assert_eq!(server.generations(), 0, "{name}: nothing executed");
    }
}

#[test]
fn downloads_stay_on_github_origins_and_within_their_bounds() {
    for (name, setup, expected) in [
        (
            "foreign-redirect",
            Box::new(|server: &Server| {
                server.route(
                    &download_path(ASSET),
                    Route::Redirect("http://127.0.0.2:9/openai".into()),
                );
            }) as Box<dyn Fn(&Server)>,
            "not a GitHub or configured repository origin",
        ),
        (
            "https-elsewhere",
            Box::new(|server: &Server| {
                server.route(
                    &download_path(ASSET),
                    Route::Redirect("https://example.org/openai".into()),
                );
            }),
            "not a GitHub or configured repository origin",
        ),
        (
            "redirect-loop",
            Box::new(|server: &Server| {
                server.route(&download_path(ASSET), Route::Redirect(download_path(ASSET)));
            }),
            "redirects more than 5 times",
        ),
        (
            "oversized-manifest",
            Box::new(|server: &Server| {
                server.body(&raw_path("openai.extension.json"), vec![b' '; 70 * 1024]);
            }),
            "larger than its 65536-byte bound",
        ),
        (
            // GitHub's release-asset limit is under 2 GiB, so the component
            // bound is 2 GiB − 1: a declared length one past it fails before
            // any body byte is read or buffered.
            "declared-oversized-component",
            Box::new(|server: &Server| {
                server.route("/assets/openai", Route::Declared(2 << 30));
            }),
            "larger than its 2147483647-byte bound",
        ),
    ] {
        let server = github();
        setup(&server);
        let directory = scratch(name);
        let output = directory.join("final.snapshot.json");
        let mut flags = repository(&server, &directory.join("cache"));
        flags.extend(openai_grants(&server));
        let (run, _) = snapshot_run(&hello(), &output, &flags);
        assert!(!run.status.success(), "{name}");
        assert!(
            text(&run.stderr).contains(expected),
            "{name}: {}",
            text(&run.stderr)
        );
        assert_eq!(server.generations(), 0, "{name}");
    }
}

/// A body of exactly its bound is admitted: the manifest padded with JSON
/// white space to 65,536 bytes resolves.
#[test]
fn a_download_of_exactly_its_bound_is_admitted() {
    let server = github();
    let mut manifest = package_file("openai.extension.json");
    assert!(
        manifest.len() < 64 * 1024,
        "the fixture manifest has room to pad"
    );
    manifest.resize(64 * 1024, b' ');
    server.body(&raw_path("openai.extension.json"), manifest);
    let directory = scratch("exact-bound");
    let valid = validate(&hello(), &repository(&server, &directory.join("cache")));
    assert!(valid.status.success(), "{}", text(&valid.stderr));
}

#[test]
fn offline_without_the_cache_fails_closed_before_the_first_macrostep() {
    let server = github();
    let directory = scratch("offline");
    let cache = directory.join("cache");
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &cache);
    flags.extend(openai_grants(&server));
    flags.push("--offline".into());

    let (empty, _) = snapshot_run(&hello(), &output, &flags);
    assert!(!empty.status.success());
    assert!(
        text(&empty.stderr).contains("--offline forbids fetching"),
        "{}",
        text(&empty.stderr)
    );
    assert!(server.requests().is_empty(), "offline uses no network");

    // The package cached (validation needs only it), the component not: the
    // machine asks for the element when its session starts, and that start
    // fails closed without running anything.
    let online = validate(&hello(), &repository(&server, &cache));
    assert!(online.status.success(), "{}", text(&online.stderr));
    let before = server.requests().len();
    let (start, _) = snapshot_run(&hello(), &output, &flags);
    assert!(!start.status.success());
    let stderr = text(&start.stderr);
    assert!(
        stderr.contains("its extension component digest is not in the extension cache"),
        "{stderr}"
    );
    assert!(!output.exists());
    assert_eq!(server.requests().len(), before);
}

#[test]
fn a_tampered_cache_fails_closed() {
    let server = github();
    let directory = scratch("tampered");
    let cache = directory.join("cache");
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &cache);
    flags.extend(openai_grants(&server));
    let (first, _) = snapshot_run(&hello(), &output, &flags);
    assert!(first.status.success(), "{}", text(&first.stderr));
    let object = cache.join("sha256").join(sha256(&release().openai));
    let mut bytes = fs::read(&object).expect("the cached component");
    if let Some(last) = bytes.last_mut() {
        *last ^= 1;
    }
    fs::write(&object, bytes).expect("tamper");
    fs::remove_file(&output).expect("a fresh output");
    let (tampered, _) = snapshot_run(&hello(), &output, &flags);
    assert!(!tampered.status.success());
    assert!(
        text(&tampered.stderr).contains("does not match its SHA-256 (it was modified)"),
        "{}",
        text(&tampered.stderr)
    );
    assert!(!output.exists());
}

/// The example element bound with `--extension-package`: its manifest
/// declares `declared` as its outbound origins (written to `directory`).
fn example_package(directory: &Path, declared: &[&str]) -> Vec<String> {
    let release = release();
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
            release.dir
        ),
        "--extension-component-sha256",
        &format!("{EXAMPLE_URI}={}", release.example_sha256),
    ])
}

/// A document whose openai element answers first, then whose example element
/// fetches with `fetch_attributes`, assigning its outcome to `outcome`.
fn two_elements(directory: &Path, fetch_attributes: &str) -> String {
    let document = directory.join(format!("two-{}.scxml", sha256(fetch_attributes.as_bytes())));
    fs::write(
        &document,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:openai="{OPENAI_URI}" xmlns:example="{EXAMPLE_URI}" xmlns:env="{ENV_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="ask">
  <datamodel><data id="reply" expr="''"/><data id="outcome" expr="''"/></datamodel>
  <state id="ask">
    <onentry>
      <openai:generate env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL" model="gpt-4o-mini" location="reply"><openai:prompt>Hi</openai:prompt></openai:generate>
    </onentry>
    <transition cond="reply.length > 0" target="fetch"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <state id="fetch">
    <onentry><example:fetch {fetch_attributes} location="outcome"/></onentry>
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

/// Each element receives references for its own slots only, and a value is
/// substituted only into a request to an origin the element's package
/// declares or the run supplied. The example element never holds a value:
/// it reports only the slot names it was handed.
#[test]
fn values_reach_only_their_element_and_only_admitted_origins() {
    let server = github();
    let directory = scratch("isolation");
    let output = directory.join("final.snapshot.json");
    let cache = directory.join("cache");
    let run = |document: &str, declared: &[&str], extra: &[&str]| {
        let mut flags = repository(&server, &cache);
        flags.extend(openai_grants(&server));
        flags.extend(example_package(&directory, declared));
        flags.extend(strings(extra));
        snapshot_run(document, &output, &flags)
    };
    let echo = format!("{}/echo", server.origin);

    // B binds no slot: it gets no reference, even though the run holds A's
    // key, and its undeclared literal origin is refused.
    let (refused, shown) = run(
        &two_elements(&directory, &format!(r#"url="{echo}""#)),
        &[],
        &[],
    );
    assert!(refused.status.success(), "{}", text(&refused.stderr));
    assert!(shown.contains("configuration: atomic [refused]"), "{shown}");
    assert!(
        shown.contains(r#"outcome = "status=refused slots=""#),
        "{shown}"
    );
    assert_eq!(server.generations(), 1, "A's own request went out");
    assert_eq!(server.echoes(), 0, "B's request never left the host");
    assert!(
        text(&refused.stderr).contains(&format!(
            "`{EXAMPLE_URI}` was refused outbound HTTP to `{}`",
            server.origin
        )),
        "{}",
        text(&refused.stderr)
    );

    let toward_undeclared = two_elements(
        &directory,
        &format!(r#"url="{echo}" env:token="OPENAI_API_KEY""#),
    );
    let also_b = format!("OPENAI_API_KEY@{EXAMPLE_URI}");
    let also_b: &[&str] = &["--env", &also_b];

    // B names A's key (granted to B as well) for its own token slot, toward
    // an undeclared origin: refused before anything is sent, so the key
    // never leaves.
    let (refused, shown) = run(&toward_undeclared, &[], also_b);
    assert!(refused.status.success(), "{}", text(&refused.stderr));
    assert!(
        shown.contains(r#"outcome = "status=refused slots=token""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 0);
    let before = server.authorizations().len();

    // The same element whose package declares the origin: admitted, the
    // reference substituted by the host, the component still holding only
    // the slot's name.
    let (admitted, shown) = run(&toward_undeclared, &[&server.origin], also_b);
    assert!(admitted.status.success(), "{}", text(&admitted.stderr));
    assert!(shown.contains("configuration: atomic [fetched]"), "{shown}");
    assert!(
        shown.contains(r#"outcome = "status=200 slots=token""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 1);
    assert_eq!(
        server.authorizations()[before..].last().map(String::as_str),
        Some("Bearer sk-test"),
        "the host substituted the value into the admitted request"
    );

    // A run-supplied origin: B's url slot names a value that is a whole
    // origin, so the request is admitted though nothing declares it.
    let (supplied, shown) = run(
        &two_elements(&directory, r#"path="/echo" env:url="ECHO_BASE""#),
        &[],
        &[
            "--env",
            &format!("ECHO_BASE@{EXAMPLE_URI}={}", server.origin),
        ],
    );
    assert!(supplied.status.success(), "{}", text(&supplied.stderr));
    assert!(
        shown.contains(r#"outcome = "status=200 slots=url""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 2);

    // A literal that extends the authority the value starts (the port here)
    // is no run-supplied origin: refused.
    let port = server.origin.rsplit(':').next().expect("a port").to_owned();
    let (split, shown) = run(
        &two_elements(
            &directory,
            &format!(r#"path=":{port}/echo" env:url="HOST_ONLY""#),
        ),
        &[],
        &[
            "--env",
            &format!("HOST_ONLY@{EXAMPLE_URI}=http://127.0.0.1"),
        ],
    );
    assert!(split.status.success(), "{}", text(&split.stderr));
    assert!(
        shown.contains(r#"outcome = "status=refused slots=url""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 2, "nothing more was sent");
}

/// A value granted only to A's namespace never reaches B: B's slot naming it
/// is absent, exactly like an unheld value, even toward an origin B's
/// package declares, and the listing names the flag that would grant it.
#[test]
fn a_value_granted_to_another_namespace_is_absent() {
    let server = github();
    let directory = scratch("scoped");
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &directory.join("cache"));
    flags.extend(openai_grants(&server));
    flags.extend(example_package(&directory, &[&server.origin]));
    let document = two_elements(
        &directory,
        &format!(r#"url="{}/echo" env:token="OPENAI_API_KEY""#, server.origin),
    );
    let (out_of_scope, shown) = snapshot_run(&document, &output, &flags);
    assert!(
        out_of_scope.status.success(),
        "{}",
        text(&out_of_scope.stderr)
    );
    assert!(
        shown.contains(r#"outcome = "status=200 slots=""#),
        "{shown}"
    );
    assert_eq!(server.echoes(), 1, "B's request carried no reference");
    assert!(
        text(&out_of_scope.stderr).contains(&format!(
            "token <- OPENAI_API_KEY (no value granted: add --env OPENAI_API_KEY@{EXAMPLE_URI})"
        )),
        "{}",
        text(&out_of_scope.stderr)
    );
}

/// A misspelled slot is the validator's `SCXML083`, with the fix.
#[test]
fn a_misspelled_slot_is_caught_by_the_validator() {
    let server = github();
    let directory = scratch("typo");
    let document = directory.join("typo.scxml");
    fs::write(
        &document,
        fs::read_to_string(hello())
            .expect("the example")
            .replace("env:apikey=", "env:apikye="),
    )
    .expect("the document");
    let rejected = validate(
        &document.display().to_string(),
        &repository(&server, &directory.join("cache")),
    );
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        text(&rejected.stderr)
    );
    let stderr = text(&rejected.stderr);
    assert!(stderr.contains("error[SCXML083]"), "{stderr}");
    assert!(stderr.contains("did you mean `apikey`?"), "{stderr}");
}

/// Values from `--env-file` (Docker's format, CRLF included), masked in what
/// the CLI writes as text (here a live session's log record), and a missing
/// one named by the flag that adds it.
#[test]
fn env_files_supply_values_that_are_masked_in_cli_output() {
    let release = release();
    let server = github();
    let directory = scratch("env-file");
    let env_file = directory.join(".env");
    fs::write(
        &env_file,
        format!(
            "# the key\r\nOPENAI_API_KEY=sk-file-secret-1\r\nOPENAI_BASE_URL={}/v1\r\n",
            server.origin
        ),
    )
    .expect("the env file");
    let document = directory.join("leak.scxml");
    fs::write(
        &document,
        fs::read_to_string(hello()).expect("the example").replace(
            r#"<log label="reply" expr="reply"/>"#,
            r#"<log label="reply" expr="reply"/><log label="leak" expr="'echo sk-file-secret-1'"/>"#,
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
        "--env",
        &format!("OPENAI_API_KEY@{OPENAI_URI}"),
        "--env",
        &format!("OPENAI_BASE_URL@{OPENAI_URI}"),
    ]);
    args.extend(repository(&server, &directory.join("cache")));
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
        server.generations(),
        1,
        "the key reached the service as a header"
    );

    // Without the file the key has no value: the run says which flag adds it.
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &directory.join("cache"));
    flags.extend(strings(&[
        "--env",
        &format!("OPENAI_BASE_URL@{OPENAI_URI}={}/v1", server.origin),
    ]));
    let (missing, shown) = snapshot_run(&hello(), &output, &flags);
    assert!(missing.status.success(), "{}", text(&missing.stderr));
    assert!(shown.contains("configuration: atomic [failed]"), "{shown}");
    assert!(
        text(&missing.stderr).contains(&format!(
            "apikey <- OPENAI_API_KEY (no value granted: add --env OPENAI_API_KEY@{OPENAI_URI})"
        )),
        "{}",
        text(&missing.stderr)
    );
}

/// An invoked child session that uses the repository namespace: the parent's
/// document names it, so the parent's session start loads it, and the
/// child's start finds it bound and instantiates it with its grants.
#[test]
fn an_invoked_session_uses_the_repository_element() {
    let server = github();
    let directory = scratch("invoked");
    let document = directory.join("parent.scxml");
    fs::write(
        &document,
        format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="p">
  <state id="p">
    <invoke type="scxml">
      <content>
        <scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:openai="{OPENAI_URI}" xmlns:env="{ENV_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="ask">
          <datamodel><data id="reply" expr="''"/></datamodel>
          <state id="ask">
            <onentry><openai:generate env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL" model="gpt-4o-mini" location="reply"><openai:prompt>Hi</openai:prompt></openai:generate></onentry>
            <transition cond="reply.length > 0" target="answered"/>
            <transition event="error.execution" target="failed"/>
          </state>
          <final id="answered"><onentry><send target="#_parent" event="answered"/></onentry></final>
          <final id="failed"/>
        </scxml>
      </content>
    </invoke>
    <transition event="answered" target="done"/>
    <transition event="done.invoke" target="lost"/>
  </state>
  <final id="done"/>
  <final id="lost"/>
</scxml>"##
        ),
    )
    .expect("the document");
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &directory.join("cache"));
    flags.extend(openai_grants(&server));
    let (run, shown) = snapshot_run(&document.display().to_string(), &output, &flags);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(shown.contains("configuration: atomic [done]"), "{shown}");
    assert_eq!(server.generations(), 1);
}
