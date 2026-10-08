#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! Repository namespaces end to end through the CLI (`wasmtime` feature): a
//! document binds `https://github.com/stateforward/scxml.rs/namespaces/example@v0.2.0`
//! and nothing else, and `validate` and `snapshot run` resolve it from a mock
//! GitHub this suite serves on 127.0.0.1 (`--github-api-base`,
//! `--github-raw-base`, `--github-download-base`): the tag through the API,
//! the manifest and schema at the commit, and, before the session starts, the
//! release asset authenticated by the release's `SHA256SUMS`, all cached
//! content-addressed and pinned. No test touches the real network.
//!
//! The release serves the shipped example extension
//! (`components/scxml-extension-example`): its package with the tagless
//! repository URI as its namespace (the document pins `@v0.2.0`, which is
//! never part of the namespace) (the schema's `targetNamespace` and the manifest's
//! `schema-sha256` follow), and its component bytes as the release asset.
//! The component answers its own fixed namespace URI, not the repository
//! URI, so a session gets as far as the host's `namespace-uri` check of the
//! bound, authenticated component and then fails closed
//! (`extension-component-uri-mismatch`): that check is the proof the
//! release's bytes were bound to the repository URI, as for repository
//! services (`repository_services.rs`).

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest as _, Sha256};

const REPOSITORY_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/example@v0.2.0";
/// The package's identity: the tagless URI its release's manifest and
/// schema name, and the namespace every pinned use of it is in.
const PACKAGE_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/example";
const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";
const ENV_NAMESPACE: &str = "https://github.com/stateforward/scxml.rs/namespaces/env";
const ASSET: &str = "scxml_extension_example.virtualized.wasm";
/// The host's refusal of a component that answers another namespace URI.
const MISMATCH: &str = "extension-component-uri-mismatch";
/// The commit the mock tag names (through an annotated tag object).
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const TAG_OBJECT: &str = "fedcba9876543210fedcba9876543210fedcba98";
const REPO: &str = "stateforward/scxml.rs";

/// The session components plus the example extension component the mock
/// release serves.
const COMPONENTS: &[&str] = &[
    "scxml_clock",
    "scxml_data_model_ecmascript",
    "scxml_data_model_null",
    "scxml_data_model_xpath",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_extension_example",
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
    example: Vec<u8>,
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
        Release {
            dir: dir.display().to_string(),
            manifest_sha256: format!("{:x}", Sha256::digest(manifest.as_bytes())),
            example: fs::read(dir.join(ASSET)).expect("the example component"),
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

/// One mock answer.
#[derive(Clone)]
enum Route {
    Body(Vec<u8>),
    Redirect(String),
    /// A 200 that declares this `content-length` and sends no body.
    Declared(u64),
}

/// A mock GitHub (API, raw files, releases) on 127.0.0.1. Routes are exact paths; every request line is recorded.
struct Server {
    origin: String,
    routes: Arc<Mutex<BTreeMap<String, Route>>>,
    requests: Arc<Mutex<Vec<String>>>,
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
        let (served, log) = (Arc::clone(&routes), Arc::clone(&requests));
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
        }
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
}

/// The example package re-published under the repository URI: its schema
/// with that `targetNamespace`, and its manifest naming that URI and the
/// schema's SHA-256.
fn package_file(name: &str) -> Vec<u8> {
    let source = workspace().join("components/scxml-extension-example/package");
    let original_schema = fs::read(source.join("example.xsd")).expect("the example schema");
    let schema = String::from_utf8(original_schema.clone())
        .expect("UTF-8")
        .replace(
            &format!("targetNamespace=\"{EXAMPLE_URI}\""),
            &format!("targetNamespace=\"{PACKAGE_URI}\""),
        )
        .into_bytes();
    match name {
        "example.xsd" => schema,
        "example.extension.json" => String::from_utf8(
            fs::read(source.join("example.extension.json")).expect("the example manifest"),
        )
        .expect("UTF-8")
        .replace(EXAMPLE_URI, PACKAGE_URI)
        .replace(&sha256(&original_schema), &sha256(&schema))
        .into_bytes(),
        other => panic!("no package file {other}"),
    }
}

fn raw_path(file: &str) -> String {
    format!("/raw/{REPO}/{COMMIT}/namespaces/example/package/{file}")
}

fn download_path(asset: &str) -> String {
    format!("/dl/{REPO}/releases/download/v0.2.0/{asset}")
}

/// A mock GitHub serving the repository element exactly as a release would:
/// `v0.2.0` is an annotated tag of [`COMMIT`], the package files sit at the
/// commit, and the release serves the component (through a redirect, like
/// GitHub's asset host) and its `SHA256SUMS`.
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
        &raw_path("example.extension.json"),
        package_file("example.extension.json"),
    );
    server.body(&raw_path("example.xsd"), package_file("example.xsd"));
    server.body(
        &download_path("SHA256SUMS"),
        format!(
            "{}  {ASSET}\n{}  manifest.tsv\n",
            sha256(&release.example),
            "0".repeat(64)
        ),
    );
    server.route(
        &download_path(ASSET),
        Route::Redirect(format!("{}/assets/example", server.origin)),
    );
    server.body("/assets/example", release.example.clone());
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

/// A document whose one element is the repository namespace's `raise`,
/// with `attributes`, written in the test's own `directory` so no other
/// test rewrites it while an `scx` reads it.
fn document_with(directory: &Path, attributes: &str) -> String {
    let path = directory.join(format!("{}.scxml", sha256(attributes.as_bytes())));
    fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:repo="{REPOSITORY_URI}" xmlns:env="{ENV_NAMESPACE}" version="1.0" datamodel="ecmascript" initial="s0">
  <datamodel><data id="outcome" expr="''"/></datamodel>
  <state id="s0">
    <onentry><repo:raise {attributes}/></onentry>
    <transition event="example.raised" target="fetched"/>
    <transition event="error.execution" target="refused"/>
  </state>
  <final id="fetched"/>
  <final id="refused"/>
</scxml>"#
        ),
    )
    .expect("the document");
    path.display().to_string()
}

fn hello(directory: &Path) -> String {
    document_with(directory, r#"event="example.raised""#)
}

/// `snapshot run` of `document` with `extra` flags.
fn snapshot_run(document: &str, output: &Path, extra: &[String]) -> Output {
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
    cli(&args)
}

fn validate(document: &str, extra: &[String]) -> Output {
    let mut args = strings(&["validate", document]);
    args.extend_from_slice(extra);
    cli(&args)
}

/// The run failed closed at the host's check of the bound component (it was
/// resolved, authenticated, and bound to the repository URI first).
fn assert_bound_then_refused(run: &Output, output: &Path) {
    assert!(!run.status.success());
    assert!(
        text(&run.stderr).contains(MISMATCH),
        "{}",
        text(&run.stderr)
    );
    assert!(!output.exists(), "no snapshot is written");
}

#[test]
fn a_repository_element_loads_before_the_session_and_then_from_the_cache_offline() {
    let server = github();
    let directory = scratch("happy");
    let cache = directory.join("cache");
    let output = directory.join("final.snapshot.json");
    let flags = repository(&server, &cache);

    let first = snapshot_run(&hello(&directory), &output, &flags);
    assert_bound_then_refused(&first, &output);
    let fetched = server.requests();
    assert_eq!(
        fetched,
        [
            format!("GET /api/repos/{REPO}/git/ref/tags/v0.2.0 HTTP/1.1"),
            format!("GET /api/repos/{REPO}/git/tags/{TAG_OBJECT} HTTP/1.1"),
            format!("GET {} HTTP/1.1", raw_path("example.extension.json")),
            format!("GET {} HTTP/1.1", raw_path("example.xsd")),
            format!("GET {} HTTP/1.1", download_path("SHA256SUMS")),
            format!("GET {} HTTP/1.1", download_path(ASSET)),
            "GET /assets/example HTTP/1.1".to_owned(),
        ],
        "the tag resolves to its commit, the package comes from that commit (never the tag), and the component from the release"
    );
    // The exact bytes are cached by digest, and the URI is pinned.
    assert!(
        cache
            .join("sha256")
            .join(sha256(&release().example))
            .is_file()
    );
    let index = cache
        .join("index")
        .join(format!("{}.json", sha256(REPOSITORY_URI.as_bytes())));
    let pinned = fs::read_to_string(&index).expect("the URI is pinned");
    assert!(pinned.contains(COMMIT), "{pinned}");

    // Offline: the same binding from the cache alone.
    let mut offline = flags.clone();
    offline.push("--offline".into());
    let second = snapshot_run(&hello(&directory), &output, &offline);
    assert_bound_then_refused(&second, &output);
    assert_eq!(server.requests(), fetched, "offline touches no GitHub URL");

    // Online again: the pin holds even when the tag moves.
    server.body(
        &format!("/api/repos/{REPO}/git/tags/{TAG_OBJECT}"),
        format!(
            r#"{{"object":{{"type":"commit","sha":"{}"}}}}"#,
            "f".repeat(40)
        ),
    );
    let third = snapshot_run(&hello(&directory), &output, &flags);
    assert_bound_then_refused(&third, &output);
    assert_eq!(server.requests(), fetched, "a pinned URI asks no network");
}

#[test]
fn validate_resolves_the_schema_and_never_the_component() {
    let server = github();
    let directory = scratch("validate");
    let flags = repository(&server, &directory.join("cache"));
    let valid = validate(&hello(&directory), &flags);
    assert!(valid.status.success(), "{}", text(&valid.stderr));
    let invalid = validate(
        &document_with(&directory, r#"event="example.raised" stream="true""#),
        &flags,
    );
    assert_eq!(
        invalid.status.code(),
        Some(1),
        "the package schema rejects an undeclared attribute: {}",
        text(&invalid.stderr)
    );
    assert!(
        !server
            .requests()
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
            String::from_utf8(package_file("example.extension.json"))
                .expect("UTF-8")
                .replace(
                    PACKAGE_URI,
                    "https://github.com/stateforward/scxml.rs/namespaces/other",
                ),
            package_file("example.xsd"),
            "is `https://github.com/stateforward/scxml.rs/namespaces/other`, not `https://github.com/stateforward/scxml.rs/namespaces/example`",
        ),
        (
            "pinned-uri",
            String::from_utf8(package_file("example.extension.json"))
                .expect("UTF-8")
                .replace(PACKAGE_URI, REPOSITORY_URI),
            package_file("example.xsd"),
            "names the pinned URI `https://github.com/stateforward/scxml.rs/namespaces/example@v0.2.0`",
        ),
        (
            "schema-digest",
            String::from_utf8(package_file("example.extension.json")).expect("UTF-8"),
            [package_file("example.xsd"), b"<!-- changed -->".to_vec()].concat(),
            "schema-sha256 does not match",
        ),
        {
            let schema = String::from_utf8(package_file("example.xsd"))
                .expect("UTF-8")
                .replace(
                    &format!("targetNamespace=\"{PACKAGE_URI}\""),
                    "targetNamespace=\"https://example.org/other\"",
                )
                .into_bytes();
            let manifest = String::from_utf8(package_file("example.extension.json"))
                .expect("UTF-8")
                .replace(&sha256(&package_file("example.xsd")), &sha256(&schema));
            ("target-namespace", manifest, schema, "targetNamespace")
        },
    ] {
        let server = github();
        server.body(&raw_path("example.extension.json"), manifest);
        server.body(&raw_path("example.xsd"), schema);
        let directory = scratch(name);
        let cache = directory.join("cache");
        let rejected = validate(&hello(&directory), &repository(&server, &cache));
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
fn a_component_its_release_does_not_vouch_for_fails_before_the_session() {
    let release = release();
    for (name, sums, expected) in [
        (
            "missing",
            format!("{}  other.wasm\n", sha256(&release.example)),
            "lists no single SHA-256",
        ),
        (
            "mismatch",
            format!("{}  {ASSET}\n", "0".repeat(64)),
            "does not match the SHA-256",
        ),
        (
            "twice",
            format!("{0}  {ASSET}\n{0}  {ASSET}\n", sha256(&release.example)),
            "lists no single SHA-256",
        ),
    ] {
        let server = github();
        server.body(&download_path("SHA256SUMS"), sums);
        let directory = scratch(name);
        let output = directory.join("final.snapshot.json");
        let run = snapshot_run(
            &hello(&directory),
            &output,
            &repository(&server, &directory.join("cache")),
        );
        assert!(!run.status.success(), "{name}");
        let stderr = text(&run.stderr);
        assert!(stderr.contains(expected), "{name}: {stderr}");
        assert!(!stderr.contains(MISMATCH), "{name}: nothing was bound");
        assert!(!output.exists(), "{name}: no snapshot is written");
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
                    Route::Redirect("http://127.0.0.2:9/example".into()),
                );
            }) as Box<dyn Fn(&Server)>,
            "not a GitHub or configured repository origin",
        ),
        (
            "https-elsewhere",
            Box::new(|server: &Server| {
                server.route(
                    &download_path(ASSET),
                    Route::Redirect("https://example.org/example".into()),
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
                server.body(&raw_path("example.extension.json"), vec![b' '; 70 * 1024]);
            }),
            "larger than its 65536-byte bound",
        ),
        (
            // GitHub's release-asset limit is under 2 GiB, so the component
            // bound is 2 GiB − 1: a declared length one past it fails before
            // any body byte is read or buffered.
            "declared-oversized-component",
            Box::new(|server: &Server| {
                server.route("/assets/example", Route::Declared(2 << 30));
            }),
            "larger than its 2147483647-byte bound",
        ),
    ] {
        let server = github();
        setup(&server);
        let directory = scratch(name);
        let output = directory.join("final.snapshot.json");
        let run = snapshot_run(
            &hello(&directory),
            &output,
            &repository(&server, &directory.join("cache")),
        );
        assert!(!run.status.success(), "{name}");
        assert!(
            text(&run.stderr).contains(expected),
            "{name}: {}",
            text(&run.stderr)
        );
        assert!(!output.exists(), "{name}: no snapshot is written");
    }
}

/// A body of exactly its bound is admitted: the manifest padded with JSON
/// white space to 65,536 bytes resolves.
#[test]
fn a_download_of_exactly_its_bound_is_admitted() {
    let server = github();
    let mut manifest = package_file("example.extension.json");
    assert!(
        manifest.len() < 64 * 1024,
        "the fixture manifest has room to pad"
    );
    manifest.resize(64 * 1024, b' ');
    server.body(&raw_path("example.extension.json"), manifest);
    let directory = scratch("exact-bound");
    let valid = validate(
        &hello(&directory),
        &repository(&server, &directory.join("cache")),
    );
    assert!(valid.status.success(), "{}", text(&valid.stderr));
}

#[test]
fn offline_without_the_cache_fails_closed_before_the_first_macrostep() {
    let server = github();
    let directory = scratch("offline");
    let cache = directory.join("cache");
    let output = directory.join("final.snapshot.json");
    let mut flags = repository(&server, &cache);
    flags.push("--offline".into());

    let empty = snapshot_run(&hello(&directory), &output, &flags);
    assert!(!empty.status.success());
    assert!(
        text(&empty.stderr).contains("--offline forbids fetching"),
        "{}",
        text(&empty.stderr)
    );
    assert!(server.requests().is_empty(), "offline uses no network");

    // The package cached (validation needs only it), the component not: the
    // run resolves the element before its session starts, and fails closed
    // without running anything.
    let online = validate(&hello(&directory), &repository(&server, &cache));
    assert!(online.status.success(), "{}", text(&online.stderr));
    let before = server.requests().len();
    let start = snapshot_run(&hello(&directory), &output, &flags);
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
    let flags = repository(&server, &cache);
    let first = snapshot_run(&hello(&directory), &output, &flags);
    assert_bound_then_refused(&first, &output);
    let object = cache.join("sha256").join(sha256(&release().example));
    let mut bytes = fs::read(&object).expect("the cached component");
    if let Some(last) = bytes.last_mut() {
        *last ^= 1;
    }
    fs::write(&object, bytes).expect("tamper");
    let tampered = snapshot_run(&hello(&directory), &output, &flags);
    assert!(!tampered.status.success());
    let stderr = text(&tampered.stderr);
    assert!(
        stderr.contains("does not match its SHA-256 (it was modified)"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(MISMATCH),
        "the tampered bytes were never bound"
    );
    assert!(!output.exists());
}

/// An `env` attribute on a repository element is the validator's
/// `ENV002`: executable content has no slots.
#[test]
fn an_env_attribute_on_a_repository_element_is_rejected() {
    let server = github();
    let directory = scratch("env");
    let rejected = validate(
        &document_with(
            &directory,
            r#"event="example.raised" env:token="REPO_TOKEN""#,
        ),
        &repository(&server, &directory.join("cache")),
    );
    assert_eq!(
        rejected.status.code(),
        Some(1),
        "{}",
        text(&rejected.stderr)
    );
    let stderr = text(&rejected.stderr);
    assert!(stderr.contains("error[ENV002]"), "{stderr}");
}

/// An invoked child session that uses the repository namespace: the parent's
/// document names it, so the run resolves and binds it before the parent
/// starts (and fails closed at the bound component's check before anything
/// runs).
#[test]
fn an_invoked_session_binds_the_repository_element_before_the_parent_starts() {
    let server = github();
    let directory = scratch("invoked");
    let document = directory.join("parent.scxml");
    fs::write(
        &document,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="p">
  <state id="p">
    <invoke type="scxml">
      <content>
        <scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:repo="{REPOSITORY_URI}" version="1.0" datamodel="ecmascript" initial="ask">
          <state id="ask">
            <onentry><repo:raise event="asked"/></onentry>
            <transition event="asked" target="answered"/>
          </state>
          <final id="answered"/>
        </scxml>
      </content>
    </invoke>
    <transition event="done.invoke" target="done"/>
  </state>
  <final id="done"/>
</scxml>"#
        ),
    )
    .expect("the document");
    let output = directory.join("final.snapshot.json");
    let run = snapshot_run(
        &document.display().to_string(),
        &output,
        &repository(&server, &directory.join("cache")),
    );
    assert_bound_then_refused(&run, &output);
    assert!(
        server
            .requests()
            .contains(&"GET /assets/example HTTP/1.1".to_owned()),
        "the run fetched the child's element"
    );
}

/// A repository namespace only an `<invoke src>` child document uses: the
/// run loads the child before the session and resolves and binds its
/// namespace with the root's, so the child compiles against it.
#[test]
fn an_invoke_src_child_document_has_its_repository_element_resolved() {
    let server = github();
    let directory = scratch("invoke-src");
    fs::write(
        directory.join("child.scx"),
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:repo="{REPOSITORY_URI}" version="1.0" datamodel="ecmascript" initial="ask">
  <state id="ask">
    <onentry><repo:raise event="asked"/></onentry>
    <transition event="asked" target="answered"/>
  </state>
  <final id="answered"/>
</scxml>"#
        ),
    )
    .expect("the child");
    let document = directory.join("parent.scx");
    fs::write(
        &document,
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="p"><state id="p"><invoke type="scxml" src="child.scx"/><transition event="done.invoke" target="done"/></state><final id="done"/></scxml>"#,
    )
    .expect("the document");
    let output = directory.join("final.snapshot.json");
    let run = snapshot_run(
        &document.display().to_string(),
        &output,
        &repository(&server, &directory.join("cache")),
    );
    assert_bound_then_refused(&run, &output);
    assert!(
        text(&run.stderr).contains(&format!("element `{PACKAGE_URI}`")),
        "the child's element is listed: {}",
        text(&run.stderr)
    );
}

/// The commit a second release, `v0.3.0`, names (a lightweight tag).
const COMMIT_V3: &str = "89abcdef0123456789abcdef0123456789abcdef";

/// Adds the repository's latest release (`v0.2.0`) and a second release,
/// `v0.3.0`, serving the same package and component, to `server`.
fn more_releases(server: &Server) {
    let release = release();
    server.body(
        &format!("/api/repos/{REPO}/releases/latest"),
        r#"{"tag_name":"v0.2.0"}"#,
    );
    server.body(
        &format!("/api/repos/{REPO}/git/ref/tags/v0.3.0"),
        format!(r#"{{"ref":"refs/tags/v0.3.0","object":{{"type":"commit","sha":"{COMMIT_V3}"}}}}"#),
    );
    for file in ["example.extension.json", "example.xsd"] {
        server.body(
            &format!("/raw/{REPO}/{COMMIT_V3}/namespaces/example/package/{file}"),
            package_file(file),
        );
    }
    server.body(
        &format!("/dl/{REPO}/releases/download/v0.3.0/SHA256SUMS"),
        format!("{}  {ASSET}\n", sha256(&release.example)),
    );
    server.route(
        &format!("/dl/{REPO}/releases/download/v0.3.0/{ASSET}"),
        Route::Redirect(format!("{}/assets/example-v3", server.origin)),
    );
    server.body("/assets/example-v3", release.example.clone());
}

/// A document of the repository namespace bound as `uri` (tagless or
/// pinned): the same element either way.
fn document_binding(directory: &Path, name: &str, uri: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:repo="{uri}" version="1.0" datamodel="ecmascript" initial="ask">
  <state id="ask">
    <onentry><repo:raise event="asked"/></onentry>
    <transition event="asked" target="answered"/>
  </state>
  <final id="answered"/>
</scxml>"#
        ),
    )
    .expect("the document");
    path
}

/// A release whose manifest names the tagless URI serves a document that
/// binds it tagless (the latest release) and one that pins it: the tag is
/// resolution input, never part of the namespace, so both validate against
/// the same schema.
#[test]
fn a_release_naming_its_tagless_uri_serves_tagless_and_pinned_documents() {
    let server = github();
    more_releases(&server);
    let directory = scratch("tagless-and-pinned");
    let flags = repository(&server, &directory.join("cache"));
    for (name, uri) in [
        ("tagless.scx", PACKAGE_URI),
        ("pinned.scx", REPOSITORY_URI),
        ("pinned-v3.scx", &format!("{PACKAGE_URI}@v0.3.0")),
    ] {
        let document = document_binding(&directory, name, uri);
        let valid = validate(&document.display().to_string(), &flags);
        assert!(valid.status.success(), "{name}: {}", text(&valid.stderr));
    }
    let requests = server.requests();
    assert!(
        requests.contains(&format!("GET /api/repos/{REPO}/releases/latest HTTP/1.1")),
        "the tagless document asked for the latest release: {requests:?}"
    );
    assert!(
        requests.contains(&format!(
            "GET /raw/{REPO}/{COMMIT_V3}/namespaces/example/package/example.extension.json HTTP/1.1"
        )),
        "the pinned v0.3.0 document resolved its own release: {requests:?}"
    );
}

/// A root that pins `@v0.2.0` and a tagless `<invoke src>` child: one
/// package, resolved once, the root's release; the tagless child shares it
/// (no latest-release lookup). A child that pins another release
/// (`@v0.3.0`) gets that release for its own sessions, alongside the root's,
/// and the consent listing names it with its document.
#[test]
fn a_tagless_child_shares_the_roots_pin_and_a_pinned_child_binds_its_own() {
    for (child_uri, second_release) in [
        (PACKAGE_URI.to_owned(), false),
        (format!("{PACKAGE_URI}@v0.3.0"), true),
    ] {
        let server = github();
        more_releases(&server);
        let directory = scratch(if second_release {
            "child-v3"
        } else {
            "child-tagless"
        });
        document_binding(&directory, "child.scx", &child_uri);
        let document = directory.join("parent.scx");
        fs::write(
            &document,
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:repo="{REPOSITORY_URI}" version="1.0" datamodel="ecmascript" initial="p"><state id="p"><onentry><repo:raise event="asked"/></onentry><invoke type="scxml" src="child.scx"/><transition event="done.invoke" target="done"/></state><final id="done"/></scxml>"#
            ),
        )
        .expect("the document");
        let output = directory.join("final.snapshot.json");
        let run = snapshot_run(
            &document.display().to_string(),
            &output,
            &repository(&server, &directory.join("cache")),
        );
        assert_bound_then_refused(&run, &output);
        let requests = server.requests();
        let stderr = text(&run.stderr);
        assert!(
            !requests.contains(&format!("GET /api/repos/{REPO}/releases/latest HTTP/1.1")),
            "a tagless use shares the root's pin: {requests:?}"
        );
        assert!(
            requests.contains(&format!("GET {} HTTP/1.1", download_path("SHA256SUMS"))),
            "the root's release is bound before the session: {requests:?}"
        );
        let child_listing = "= for: the sessions of <invoke src> document";
        if second_release {
            assert!(
                requests.contains(&"GET /assets/example-v3 HTTP/1.1".to_owned()),
                "the child's own release is bound before the session: {requests:?}"
            );
            assert!(stderr.contains("release v0.3.0"), "{stderr}");
            assert!(stderr.contains(child_listing), "{stderr}");
        } else {
            assert!(
                !requests.iter().any(|request| request.contains("v0.3.0")),
                "{requests:?}"
            );
            assert!(!stderr.contains(child_listing), "{stderr}");
        }
        assert_eq!(
            stderr.matches(&format!("element `{PACKAGE_URI}`")).count(),
            if second_release { 2 } else { 1 },
            "{stderr}"
        );
    }
}

/// A malformed pin, and one document pinning two releases of a package,
/// fail closed before anything is fetched.
#[test]
fn malformed_and_conflicting_pins_fail_closed() {
    let server = github();
    let directory = scratch("bad-pins");
    let flags = repository(&server, &directory.join("cache"));
    for (name, declarations, expected) in [
        (
            "empty-tag",
            format!(r#"xmlns:repo="{PACKAGE_URI}@""#),
            "malformed tag",
        ),
        (
            "slash-after-tag",
            format!(r#"xmlns:repo="{PACKAGE_URI}@v0.2.0/x""#),
            "malformed tag",
        ),
        (
            "two-releases",
            format!(r#"xmlns:repo="{REPOSITORY_URI}" xmlns:other="{PACKAGE_URI}@v0.3.0""#),
            "is pinned to both `@v0.2.0` and `@v0.3.0`",
        ),
    ] {
        let path = directory.join(format!("{name}.scx"));
        fs::write(
            &path,
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" {declarations} version="1.0" initial="s"><state id="s"/></scxml>"#
            ),
        )
        .expect("the document");
        let rejected = validate(&path.display().to_string(), &flags);
        assert!(!rejected.status.success(), "{name}");
        assert!(
            text(&rejected.stderr).contains(expected),
            "{name}: {}",
            text(&rejected.stderr)
        );
    }
    assert!(server.requests().is_empty(), "nothing is fetched");
}
