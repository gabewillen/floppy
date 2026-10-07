#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! Repository services end to end through the CLI (`wasmtime` feature): a
//! document invokes `https://github.com/stateforward/nemobot/services/echo@v0.1.0`
//! and binds nothing, and `validate` and `snapshot run` resolve it from a
//! mock GitHub this suite serves on 127.0.0.1 (`--github-api-base`,
//! `--github-raw-base`, `--github-download-base`) exactly like a repository
//! namespace: the tag through the API, the manifest and its event schema at
//! the commit, and, when a run binds it, the release asset authenticated by
//! the release's `SHA256SUMS`, all cached content-addressed and pinned. No
//! test touches the real network.
//!
//! The release asset is the shipped echo example component. It answers its
//! own fixed type URI, not the repository URI, so a run gets as far as the
//! host's `type-uri` check of the bound, authenticated component and then
//! fails closed: that check is the proof the release's bytes were bound to
//! the repository URI.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest as _, Sha256};

const SERVICE_URI: &str = "https://github.com/stateforward/nemobot/services/echo@v0.1.0";
const ASSET: &str = "scxml_service_example.virtualized.wasm";
/// The commit the mock tag names (a lightweight tag).
const COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";
const REPO: &str = "stateforward/nemobot";

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
    echo: Vec<u8>,
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let workspace = workspace();
        let target = workspace.join("target/cli-components");
        let dir = target.join("wasm-artifacts/repository-services");
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
            echo: fs::read(dir.join(ASSET)).expect("the echo component"),
        }
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn scratch(name: &str) -> PathBuf {
    let directory = workspace()
        .join("target/cli-repository-services")
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

/// One mock answer: status, body, and an optional `location`.
type Answer = (u16, Vec<u8>, Option<String>);
/// The mock's routes by exact path.
type Routes = Arc<Mutex<BTreeMap<String, Answer>>>;

/// A mock GitHub on 127.0.0.1: exact-path routes; every request line is
/// recorded.
struct Server {
    origin: String,
    routes: Routes,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let routes: Routes = Arc::default();
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
                    Some((status, body, location)) => {
                        let location = location
                            .map(|location| format!("location: {location}\r\n"))
                            .unwrap_or_default();
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status} X\r\n{location}content-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        stream.write_all(&body)
                    }
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

    fn body(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.routes
            .lock()
            .expect("the route lock")
            .insert(path.to_owned(), (200, body.into(), None));
    }

    fn redirect(&self, path: &str, location: String) {
        self.routes
            .lock()
            .expect("the route lock")
            .insert(path.to_owned(), (302, Vec::new(), Some(location)));
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the log lock").clone()
    }
}

fn raw_path(file: &str) -> String {
    format!("/raw/{REPO}/{COMMIT}/services/echo/package/{file}")
}

fn download_path(asset: &str) -> String {
    format!("/dl/{REPO}/releases/download/v0.1.0/{asset}")
}

/// The repository service's manifest, naming `service_uri`.
fn manifest(service_uri: &str) -> String {
    format!(
        r#"{{"format":"stateforward.scxml.service-package/v1","service-uri":"{service_uri}","package-version":"0.1.0","service-artifact":"{ASSET}","schema-artifact":null,"schema-sha256":null,"wit-world":"stateforward:scxml/invoked-service","wit-version":"0.2.0","http-origins":[],"env-slots":["endpoint"]}}"#
    )
}

/// A mock GitHub serving the echo service as a release would: `v0.1.0` is a
/// lightweight tag of [`COMMIT`], the package files sit at the commit, and
/// the release serves the component (through a redirect) and its
/// `SHA256SUMS`.
fn github(manifest_text: &str) -> Server {
    let release = release();
    let server = Server::start();
    server.body(
        &format!("/api/repos/{REPO}/git/ref/tags/v0.1.0"),
        format!(r#"{{"ref":"refs/tags/v0.1.0","object":{{"type":"commit","sha":"{COMMIT}"}}}}"#),
    );
    server.body(&raw_path("echo.service.json"), manifest_text.to_owned());
    server.body(
        &download_path("SHA256SUMS"),
        format!("{}  {ASSET}\n", sha256(&release.echo)),
    );
    server.redirect(
        &download_path(ASSET),
        format!("{}/assets/echo", server.origin),
    );
    server.body("/assets/echo", release.echo.clone());
    server
}

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

/// A document invoking the repository service, written to `directory`.
fn document(directory: &Path) -> String {
    let path = directory.join("service.scxml");
    fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><invoke type="{SERVICE_URI}" id="echo"/><transition event="echo.tick" event:schema='{{"type":"integer"}}'/><transition event="done.invoke.echo" target="f"/></state><final id="f"/></scxml>"#
        ),
    )
    .expect("the document is written");
    path.display().to_string()
}

fn validate(document: &str, extra: &[String]) -> Output {
    let mut args = vec!["validate".to_owned(), document.to_owned()];
    args.extend_from_slice(extra);
    cli(&args)
}

fn snapshot_run(document: &str, output: &Path, extra: &[String]) -> Output {
    let release = release();
    let mut args = vec![
        "snapshot".to_owned(),
        "run".to_owned(),
        document.to_owned(),
        "--components".to_owned(),
        release.dir.clone(),
        "--components-manifest-sha256".to_owned(),
        release.manifest_sha256.clone(),
        "--output".to_owned(),
        output.display().to_string(),
    ];
    args.extend_from_slice(extra);
    cli(&args)
}

#[test]
fn validate_resolves_the_package_at_its_commit_and_then_runs_offline() {
    let server = github(&manifest(SERVICE_URI));
    let directory = scratch("validate");
    let cache = directory.join("cache");
    let source = document(&directory);
    let flags = repository(&server, &cache);
    let first = validate(&source, &flags);
    let stderr = text(&first.stderr);
    // The package resolved (no SCXML094) and lists its consent.
    assert!(first.status.success(), "{stderr}");
    assert!(!stderr.contains("SCXML094"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "service `{SERVICE_URI}`\n  = version: release v0.1.0 (commit {COMMIT}), package 0.1.0"
        )),
        "{stderr}"
    );
    assert_eq!(
        server.requests(),
        [
            format!("GET /api/repos/{REPO}/git/ref/tags/v0.1.0 HTTP/1.1"),
            format!("GET {} HTTP/1.1", raw_path("echo.service.json")),
        ],
        "the tag resolves to its commit, the package comes from that commit, and validation never reaches the release"
    );
    // The URI is pinned and its bytes cached by digest: --offline needs no
    // network.
    let index = cache
        .join("index")
        .join(format!("{}.json", sha256(SERVICE_URI.as_bytes())));
    let entry = fs::read_to_string(&index).expect("the URI is pinned");
    assert!(
        entry.contains("\"format\": \"stateforward.scxml.service-cache/v1\""),
        "{entry}"
    );
    assert!(
        entry.contains(&format!("\"commit\": \"{COMMIT}\"")),
        "{entry}"
    );
    let before = server.requests().len();
    let mut offline = flags.clone();
    offline.push("--offline".into());
    let again = validate(&source, &offline);
    assert_eq!(text(&again.stderr), stderr);
    assert_eq!(server.requests().len(), before, "offline uses no network");
}

#[test]
fn a_run_binds_the_release_component_authenticated_by_its_sha256sums() {
    let server = github(&manifest(SERVICE_URI));
    let directory = scratch("run");
    let cache = directory.join("cache");
    let source = document(&directory);
    let output = directory.join("final.snapshot.json");
    let flags = repository(&server, &cache);

    let run = snapshot_run(&source, &output, &flags);
    let stderr = text(&run.stderr);
    // The release's bytes were authenticated and bound to the repository
    // URI, and the session ran (the example component rejects a type URI
    // other than its own when `invoke` runs, which raises error.execution
    // in the session: invocation is a runtime action).
    assert!(run.status.success(), "{stderr}");
    let snapshot = fs::read(&output).expect("the final snapshot is written");
    let requests = server.requests();
    assert!(
        requests.ends_with(&[
            format!("GET {} HTTP/1.1", download_path("SHA256SUMS")),
            format!("GET {} HTTP/1.1", download_path(ASSET)),
            "GET /assets/echo HTTP/1.1".to_owned(),
        ]),
        "{requests:?}"
    );
    assert!(cache.join("sha256").join(sha256(&release().echo)).exists());

    // Pinned: the offline run runs the same session from the cache alone.
    let before = server.requests().len();
    let mut offline = flags.clone();
    offline.push("--offline".into());
    let again = snapshot_run(&source, &output, &offline);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert_eq!(fs::read(&output).expect("written again"), snapshot);
    assert_eq!(server.requests().len(), before);

    // A tampered cached component fails closed before anything runs.
    let object = cache.join("sha256").join(sha256(&release().echo));
    let mut bytes = fs::read(&object).expect("the cached component");
    if let Some(last) = bytes.last_mut() {
        *last ^= 1;
    }
    fs::write(&object, bytes).expect("tamper");
    let tampered = snapshot_run(&source, &output, &offline);
    assert!(
        text(&tampered.stderr).contains("does not match its SHA-256 (it was modified)"),
        "{}",
        text(&tampered.stderr)
    );
}

#[test]
fn a_package_that_does_not_name_itself_exactly_or_an_unlisted_component_fails_closed() {
    // The manifest names another URI.
    let server = github(&manifest(
        "https://github.com/stateforward/nemobot/services/echo@v0.2.0",
    ));
    let directory = scratch("uri");
    let source = document(&directory);
    let invalid = validate(&source, &repository(&server, &directory.join("cache")));
    assert!(!invalid.status.success());
    assert!(
        text(&invalid.stderr).contains("its manifest binds"),
        "{}",
        text(&invalid.stderr)
    );
    assert!(!directory.join("cache/index").exists(), "nothing is pinned");

    // The release does not list the component.
    let server = github(&manifest(SERVICE_URI));
    server.body(
        &download_path("SHA256SUMS"),
        format!("{}  other.wasm\n", "0".repeat(64)),
    );
    let directory = scratch("sums");
    let source = document(&directory);
    let run = snapshot_run(
        &source,
        &directory.join("final.snapshot.json"),
        &repository(&server, &directory.join("cache")),
    );
    assert!(!run.status.success());
    assert!(
        text(&run.stderr).contains("lists no single SHA-256"),
        "{}",
        text(&run.stderr)
    );
}

#[test]
fn offline_without_the_cache_fails_closed_and_a_bound_uri_is_never_fetched() {
    let server = github(&manifest(SERVICE_URI));
    let directory = scratch("offline");
    let source = document(&directory);
    let mut flags = repository(&server, &directory.join("cache"));
    flags.push("--offline".into());
    let offline = validate(&source, &flags);
    assert!(!offline.status.success());
    assert!(
        text(&offline.stderr).contains("repository service")
            && text(&offline.stderr).contains("--offline forbids fetching"),
        "{}",
        text(&offline.stderr)
    );
    assert!(server.requests().is_empty());

    // A `--service-package` that binds the URI wins: nothing is fetched.
    let package = directory.join("package");
    fs::create_dir_all(&package).expect("package directory");
    fs::write(package.join("echo.service.json"), manifest(SERVICE_URI)).expect("manifest");
    let mut bound = repository(&server, &directory.join("cache"));
    bound.push("--service-package".into());
    bound.push(package.join("echo.service.json").display().to_string());
    let validated = validate(&source, &bound);
    assert!(
        text(&validated.stderr).contains("from --service-package"),
        "{}",
        text(&validated.stderr)
    );
    assert!(server.requests().is_empty());
}

/// The tagless URI of the echo service: the repository's latest release.
const TAGLESS_URI: &str = "https://github.com/stateforward/nemobot/services/echo";
/// The commit the latest release's tag (`v0.2.0`) names.
const LATEST_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn latest_raw_path(file: &str) -> String {
    format!("/raw/{REPO}/{LATEST_COMMIT}/services/echo/package/{file}")
}

fn latest_download_path(asset: &str) -> String {
    format!("/dl/{REPO}/releases/download/v0.2.0/{asset}")
}

/// [`github`] plus a latest release `v0.2.0` (a lightweight tag of
/// [`LATEST_COMMIT`]) whose manifest names the tagless URI.
fn github_with_latest() -> Server {
    let release = release();
    let server = github(&manifest(SERVICE_URI));
    server.body(
        &format!("/api/repos/{REPO}/releases/latest"),
        r#"{"tag_name":"v0.2.0","name":"v0.2.0"}"#,
    );
    server.body(
        &format!("/api/repos/{REPO}/git/ref/tags/v0.2.0"),
        format!(
            r#"{{"ref":"refs/tags/v0.2.0","object":{{"type":"commit","sha":"{LATEST_COMMIT}"}}}}"#
        ),
    );
    server.body(&latest_raw_path("echo.service.json"), manifest(TAGLESS_URI));
    server.body(
        &latest_download_path("SHA256SUMS"),
        format!("{}  {ASSET}\n", sha256(&release.echo)),
    );
    server.body(&latest_download_path(ASSET), release.echo.clone());
    server
}

/// A document at `directory/name` invoking each type URI of `types` and,
/// when given, the child document `child` through `<invoke src>`.
fn document_invoking(directory: &Path, name: &str, types: &[&str], child: Option<&str>) -> String {
    let invokes = types
        .iter()
        .enumerate()
        .map(|(index, uri)| format!(r#"<invoke type="{uri}" id="s{index}"/>"#))
        .collect::<Vec<_>>()
        .concat();
    let child = child
        .map(|src| format!(r#"<invoke type="scxml" src="{src}" id="child"/>"#))
        .unwrap_or_default();
    let path = directory.join(name);
    fs::write(
        &path,
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s">{invokes}{child}<transition event="done.invoke.child" target="f"/></state><final id="f"/></scxml>"#
        ),
    )
    .expect("the document is written");
    path.display().to_string()
}

fn count(requests: &[String], path: &str) -> usize {
    let line = format!("GET {path} HTTP/1.1");
    requests.iter().filter(|request| **request == line).count()
}

#[test]
fn a_tagless_uri_resolves_the_latest_release_and_the_consent_names_it() {
    let server = github_with_latest();
    let directory = scratch("tagless");
    let cache = directory.join("cache");
    let source = document_invoking(&directory, "root.scx", &[TAGLESS_URI], None);
    let flags = repository(&server, &cache);
    let first = validate(&source, &flags);
    let stderr = text(&first.stderr);
    assert!(first.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "service `{TAGLESS_URI}`\n  = version: release v0.2.0 (commit {LATEST_COMMIT}), package 0.1.0"
        )),
        "the consent shows the resolved tag and commit: {stderr}"
    );
    assert_eq!(
        server.requests(),
        [
            format!("GET /api/repos/{REPO}/releases/latest HTTP/1.1"),
            format!("GET /api/repos/{REPO}/git/ref/tags/v0.2.0 HTTP/1.1"),
            format!("GET {} HTTP/1.1", latest_raw_path("echo.service.json")),
        ]
    );
    let entry = fs::read_to_string(
        cache
            .join("index")
            .join(format!("{}.json", sha256(TAGLESS_URI.as_bytes()))),
    )
    .expect("the tagless URI is pinned");
    assert!(entry.contains("\"tag\": \"v0.2.0\""), "{entry}");

    // Offline, the tagless URI uses the release the cache pinned.
    let before = server.requests().len();
    let mut offline = flags.clone();
    offline.push("--offline".into());
    let again = validate(&source, &offline);
    assert_eq!(text(&again.stderr), stderr);
    assert_eq!(server.requests().len(), before);

    // Online again, the latest release is asked once more (once per root
    // session); the same commit needs no other request.
    let third = validate(&source, &flags);
    assert!(third.status.success(), "{}", text(&third.stderr));
    assert_eq!(
        server.requests()[before..],
        [
            format!("GET /api/repos/{REPO}/releases/latest HTTP/1.1"),
            format!("GET /api/repos/{REPO}/git/ref/tags/v0.2.0 HTTP/1.1"),
        ]
    );
}

#[test]
fn a_child_with_the_same_tagless_import_reuses_the_roots_single_fetch() {
    let server = github_with_latest();
    let directory = scratch("shared");
    let cache = directory.join("cache");
    document_invoking(&directory, "child.scx", &[TAGLESS_URI], None);
    let source = document_invoking(&directory, "root.scx", &[TAGLESS_URI], Some("child.scx"));
    let run = snapshot_run(
        &source,
        &directory.join("final.snapshot.json"),
        &repository(&server, &cache),
    );
    let stderr = text(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    let requests = server.requests();
    for path in [
        format!("/api/repos/{REPO}/releases/latest"),
        format!("/api/repos/{REPO}/git/ref/tags/v0.2.0"),
        latest_raw_path("echo.service.json"),
        latest_download_path("SHA256SUMS"),
        latest_download_path(ASSET),
    ] {
        assert_eq!(count(&requests, &path), 1, "{path}: {requests:?}");
    }
    assert_eq!(
        stderr.matches(&format!("service `{TAGLESS_URI}`")).count(),
        1,
        "one binding for the root and the child: {stderr}"
    );
    // The child document was loaded before the session and listed.
    let child = fs::canonicalize(directory.join("child.scx")).expect("the child exists");
    assert!(
        stderr.contains(&format!(
            "document `file://{}`\n  = for: <invoke src=\"child.scx\">",
            child.display()
        )),
        "{stderr}"
    );
}

#[test]
fn a_pinned_child_coexists_with_a_tagless_root_and_is_resolved_for_the_child() {
    let server = github_with_latest();
    let directory = scratch("pinned");
    let cache = directory.join("cache");
    // The root never uses the pinned URI: only its child does.
    document_invoking(&directory, "child.scx", &[SERVICE_URI], None);
    let source = document_invoking(&directory, "root.scx", &[TAGLESS_URI], Some("child.scx"));
    let run = snapshot_run(
        &source,
        &directory.join("final.snapshot.json"),
        &repository(&server, &cache),
    );
    let stderr = text(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "service `{TAGLESS_URI}`\n  = version: release v0.2.0 (commit {LATEST_COMMIT})"
        )) && stderr.contains(&format!(
            "service `{SERVICE_URI}`\n  = version: release v0.1.0 (commit {COMMIT})"
        )),
        "both versions are bound: {stderr}"
    );
    let requests = server.requests();
    for sums in [
        download_path("SHA256SUMS"),
        latest_download_path("SHA256SUMS"),
    ] {
        assert_eq!(count(&requests, &sums), 1, "{sums}: {requests:?}");
    }
    // Both releases list the same component bytes, which the cache holds
    // once (content-addressed): one download serves both.
    assert_eq!(
        count(&requests, &download_path(ASSET)) + count(&requests, &latest_download_path(ASSET)),
        1,
        "{requests:?}"
    );
}
