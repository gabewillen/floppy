//! What every test shares: the components, the `scx` binary, and one
//! `scx run` process driven over its standard input and output (scxml.rs
//! `docs/live-session.md`, "The `run` command").
//!
//! * [`components`] runs `scripts/build-components.sh` once per test binary
//!   into `target/test-components`: the session components, every namespace
//!   and service component, and the packages published into the build's dev
//!   repository-package cache (`target/test-components/package-cache`). The
//!   build runs with `XDG_CACHE_HOME` pointing at an empty directory, so a
//!   test can check it never wrote the release cache. Test binaries run one
//!   after another, so they share the directory.
//! * [`cli`] builds `scx` once per test binary from the `scxml.rs/` subtree
//!   into `target/scx`, embedding the session components just built
//!   (`SCXML_WASM_ARTIFACTS_DIR`).
//! * [`command`] runs `scx` from the workspace root; `run`, `validate`, and
//!   `compile` get `--cache-dir` (the test cache) and `--offline`, so every
//!   document's repository URIs resolve from what the build published and
//!   nothing is fetched.
//! * [`Run`] is one `scx run` process: inputs go to its standard input as
//!   JSON lines, and its records come back one per line.

#![allow(
    dead_code,
    reason = "each test binary uses the part of the shared support it needs"
)]

pub mod hearing;
pub mod trace;

use std::cell::RefCell;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value as Json;
use sha2::{Digest as _, Sha256};

/// The variables the bots' documents name. The child process never inherits
/// them, so a test supplies values only through its env file (or an
/// explicit `--env NAME=VALUE`).
pub const GRANTED: [&str; 8] = [
    "FLOPPY_API_KEY",
    "FLOPPY_BASE_URL",
    "TYPESAFE_API_KEY",
    "TYPESAFE_BASE_URL",
    "FLOPPY_SPEECH_API_KEY",
    "FLOPPY_SPEECH_BASE_URL",
    "FLOPPY_VOICE_API_KEY",
    "FLOPPY_VOICE_BASE_URL",
];

/// The model floppy ships (README.md, "Defaults"): every document's `model`
/// `<data>`, checked by `tests/tests/defaults.rs`.
pub const DEFAULT_MODEL: &str = "Qwen/Qwen3.8-Flash-Next";

/// The model of a live test: `FLOPPY_LIVE_MODEL`, else [`DEFAULT_MODEL`].
pub fn live_model() -> String {
    std::env::var("FLOPPY_LIVE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_owned())
}

/// The env files of a live test: `FLOPPY_LIVE_ENV_FILE`, one path or
/// several joined like `PATH` (`:` on Unix), each given to `scx run` as an
/// `--env-file` in order (a variable takes its last file's entry), so keys
/// kept in separate files reach one run without being copied.
pub fn live_env_files() -> Vec<PathBuf> {
    let Some(paths) = std::env::var_os("FLOPPY_LIVE_ENV_FILE") else {
        panic!("set FLOPPY_LIVE_ENV_FILE to the env file (or files) holding the test's keys");
    };
    std::env::split_paths(&paths).collect()
}

/// The floppy workspace root.
pub fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tests crate sits in the floppy workspace")
}

/// The SHA-256 of a file, lowercase hex.
pub fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("the artifact is readable");
    format!("{:x}", Sha256::digest(&bytes))
}

/// The verified components.
pub struct Components {
    /// The artifacts directory.
    pub dir: PathBuf,
    /// The session components' directory (the release `scx` embeds).
    pub session: PathBuf,
    /// The dev repository-package cache the build published every package
    /// into.
    pub cache: PathBuf,
    /// The `XDG_CACHE_HOME` the build ran with: the release cache would be
    /// `scxml/extensions` below it.
    pub release_home: PathBuf,
}

impl Components {
    /// The flags binding one package explicitly (for packages that are not
    /// repository packages): `kind` is `extension` or `service`, `manifest`
    /// the manifest path, `uri` its exact URI, and `component` the component
    /// path (its SHA-256 is computed here).
    pub fn package_args(kind: &str, manifest: &Path, uri: &str, component: &Path) -> Vec<String> {
        vec![
            format!("--{kind}-package"),
            manifest.display().to_string(),
            format!("--{kind}-component"),
            format!("{uri}={}", component.display()),
            format!("--{kind}-component-sha256"),
            format!("{uri}={}", sha256_file(component)),
        ]
    }
}

/// The exact type URI of the floppy service `name`.
pub fn service_uri(name: &str) -> String {
    format!("https://github.com/stateforward/floppy/services/{name}")
}

/// The components `scripts/build-components.sh` builds and verifies.
pub fn components() -> &'static Components {
    static COMPONENTS: OnceLock<Components> = OnceLock::new();
    COMPONENTS.get_or_init(|| {
        let dir = workspace().join("target/test-components");
        let release_home = dir.join("release-home");
        let _ = std::fs::remove_dir_all(&release_home);
        std::fs::create_dir_all(&release_home).expect("the release home is creatable");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--artifacts-dir")
            .arg(&dir)
            .env("XDG_CACHE_HOME", &release_home)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the component proof must pass");
        Components {
            session: dir.join("scxml"),
            cache: dir.join("package-cache"),
            release_home,
            dir,
        }
    })
}

/// The `scx` binary, built from the subtree with the session components
/// [`components`] built embedded.
pub fn cli() -> &'static Path {
    static CLI: OnceLock<PathBuf> = OnceLock::new();
    CLI.get_or_init(|| {
        let session = &components().session;
        let target = workspace().join("target/scx");
        let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["build", "--locked", "-p", "scxml-cli"])
            .current_dir(workspace().join("scxml.rs"))
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", session)
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .status()
            .expect("cargo runs");
        assert!(status.success(), "scx must build");
        target.join("debug/scx")
    })
}

/// A scratch directory of this test binary.
pub fn scratch(name: &str) -> PathBuf {
    let dir = workspace()
        .join("target/test-scratch")
        .join(format!("{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    dir
}

/// Writes `text` to `name` in the scratch directory `dir` and returns its
/// path.
pub fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("the scratch file is writable");
    path
}

/// A `scx` command run from the workspace root, without the bots'
/// variables from the calling shell; `run`, `validate`, and `compile`
/// resolve repository packages from the test cache only.
pub fn command(args: &[String]) -> Command {
    let mut command = Command::new(cli());
    command.args(args);
    if args
        .first()
        .is_some_and(|first| matches!(first.as_str(), "run" | "validate" | "compile"))
    {
        command
            .arg("--cache-dir")
            .arg(&components().cache)
            .arg("--offline");
    }
    command.current_dir(workspace()).env("NO_COLOR", "1");
    for name in GRANTED {
        command.env_remove(name);
    }
    command
}

/// One finished `scx` command.
pub struct Output {
    /// Its exit status.
    pub status: ExitStatus,
    /// Its standard output.
    pub stdout: String,
    /// Its standard error.
    pub stderr: String,
}

/// Runs `scx` with `args` to completion (no standard input).
pub fn output(args: &[String]) -> Output {
    let output = command(args)
        .stdin(Stdio::null())
        .output()
        .expect("scx runs");
    Output {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// One `scx run` process.
pub struct Run {
    child: Child,
    stdin: Option<ChildStdin>,
    records: Receiver<Json>,
    /// Every record read so far, in order (what [`Finished::all`] starts
    /// with).
    journal: RefCell<Vec<Json>>,
    reader: Option<std::thread::JoinHandle<()>>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

/// One finished `scx run` process.
pub struct Finished {
    /// Every record it wrote that no [`Run::until`] returned, in order.
    pub records: Vec<Json>,
    /// Every record it wrote, in order, including the ones
    /// [`Run::until`] returned.
    pub all: Vec<Json>,
    /// Its exit status.
    pub status: ExitStatus,
    /// Its standard error (diagnostics and the consent listing).
    pub stderr: String,
}

impl Run {
    /// Starts `scx run` with `args` (the document among them) and the
    /// extra environment `env`.
    ///
    /// A run whose document tree binds the sql, the generate or the speech
    /// service (decided by [`binds`]) and that grants it no directory gets
    /// a fresh one ([`memory_grant`], [`media_grant`],
    /// [`speech_media_grant`]): each imports `wasi:filesystem`, and a run
    /// that binds one without a `--dir` grant is refused. One that binds the pgsql service gets the network
    /// ([`pgsql_grant`]).
    pub fn start(args: &[String], env: &[(&str, &str)]) -> Self {
        let mut full = vec!["run".to_owned()];
        full.extend_from_slice(args);
        for (name, grant) in [
            ("sql", memory_grant as fn() -> Vec<String>),
            ("generate", media_grant),
            ("speech", speech_media_grant),
            ("pgsql", pgsql_grant),
        ] {
            let uri = service_uri(name);
            let granted = args.iter().any(|arg| arg.starts_with(&format!("{uri}=")));
            // `scx` runs from the workspace root, so a relative document
            // path is relative to it.
            if !granted && args.iter().any(|arg| binds(&workspace().join(arg), name)) {
                full.splice(1..1, grant());
            }
        }
        let mut command = command(&full);
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("scx run starts");
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let (sender, records) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let record: Json = serde_json::from_str(&line)
                    .unwrap_or_else(|_| panic!("every record is one JSON line: {line}"));
                if sender.send(record).is_err() {
                    break;
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _read = stderr.read_to_string(&mut text);
            text
        });
        Self {
            stdin: child.stdin.take(),
            child,
            records,
            journal: RefCell::new(Vec::new()),
            reader: Some(reader),
            stderr: Some(stderr),
        }
    }

    /// Sends one input line.
    pub fn send(&mut self, input: &Json) {
        let stdin = self.stdin.as_mut().expect("standard input is open");
        writeln!(stdin, "{input}").expect("the session reads its input");
        stdin.flush().expect("the input is flushed");
    }

    /// Sends the external event `name` with `data`.
    pub fn event(&mut self, name: &str, data: &Json) {
        self.send(&serde_json::json!({"name": name, "data": data}));
    }

    /// Closes standard input (the end of input).
    pub fn end_input(&mut self) {
        self.stdin = None;
    }

    /// The number of records read so far (by [`Run::until`]) for which
    /// `matches` holds.
    pub fn count(&self, matches: impl Fn(&Json) -> bool) -> usize {
        self.journal
            .borrow()
            .iter()
            .filter(|record| matches(record))
            .count()
    }

    /// Records until `done` holds for one, at most `limit`; panics on a
    /// timeout.
    pub fn until(&self, limit: Duration, done: impl Fn(&Json) -> bool) -> Vec<Json> {
        let deadline = Instant::now() + limit;
        let mut records = Vec::new();
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self
                .records
                .recv_timeout(left.min(Duration::from_millis(100)))
            {
                Ok(record) => {
                    let finished = done(&record);
                    self.journal.borrow_mut().push(record.clone());
                    records.push(record);
                    if finished {
                        return records;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        panic!("no expected record within {limit:?}; got {records:?}");
    }

    /// Waits at most `limit` for the process to end on its own (its root
    /// finished, or its input ended), returning every record not yet read.
    pub fn finish(mut self, limit: Duration) -> Finished {
        let deadline = Instant::now() + limit;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the child is waitable") {
                break status;
            }
            if Instant::now() >= deadline {
                let _killed = self.child.kill();
                let stderr = self
                    .stderr
                    .take()
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                panic!("scx run did not end within {limit:?}; stderr:\n{stderr}");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let stderr = self
            .stderr
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        // Every record the process wrote is read once its output closes.
        if let Some(reader) = self.reader.take() {
            reader.join().expect("the record reader ends");
        }
        let records: Vec<Json> = self.records.try_iter().collect();
        let mut all = self.journal.take();
        all.extend(records.iter().cloned());
        Finished {
            records,
            all,
            status,
            stderr,
        }
    }

    /// Sends `{"type":"close"}` and finishes.
    pub fn close(mut self, limit: Duration) -> Finished {
        self.send(&serde_json::json!({"type": "close"}));
        self.end_input();
        self.finish(limit)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _killed = self.child.kill();
            let _waited = self.child.wait();
        }
    }
}

/// The records of type `kind`.
pub fn of_type<'a>(records: &'a [Json], kind: &str) -> Vec<&'a Json> {
    records
        .iter()
        .filter(|record| record["type"] == kind)
        .collect()
}

/// `(label, text)` of every `log` record.
pub fn logs(records: &[Json]) -> Vec<(String, String)> {
    of_type(records, "log")
        .into_iter()
        .map(|record| {
            (
                record["label"].as_str().unwrap_or_default().to_owned(),
                record["text"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// `(label, text)` pairs from string pairs.
pub fn labelled(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(label, text)| ((*label).to_owned(), (*text).to_owned()))
        .collect()
}

/// The status of the `closed` record (`finished`, `running`, `failed`).
pub fn closed_status(records: &[Json]) -> Option<String> {
    of_type(records, "closed")
        .last()
        .and_then(|record| record["status"].as_str())
        .map(str::to_owned)
}

/// Whether the session closed with its root in a top-level final state.
pub fn finished(records: &[Json]) -> bool {
    closed_status(records).as_deref() == Some("finished")
}

/// `--dir` granting the sql service a fresh scratch directory at
/// `/memory`, where the bots keep their memory, `/memory/floppy.sqlite`.
pub fn memory_grant() -> Vec<String> {
    vec![
        "--dir".to_owned(),
        format!(
            "{}={}::/memory",
            service_uri("sql"),
            unique_scratch("memory").display()
        ),
    ]
}

/// `--dir` granting the generate service a fresh scratch directory at
/// `/media`, where the images its messages name are read.
pub fn media_grant() -> Vec<String> {
    vec![
        "--dir".to_owned(),
        format!(
            "{}={}::/media",
            service_uri("generate"),
            unique_scratch("media").display()
        ),
    ]
}

/// `--dir` granting the speech service a fresh scratch directory at
/// `/media`, where the audio it decodes and encodes is read and written.
pub fn speech_media_grant() -> Vec<String> {
    vec![
        "--dir".to_owned(),
        format!(
            "{}={}::/media",
            service_uri("speech"),
            unique_scratch("speech-media").display()
        ),
    ]
}

/// `--wasi` letting the pgsql service connect (`inherit-network`): the
/// memory ability names it beside the sql service, so a run that binds one
/// binds both.
pub fn pgsql_grant() -> Vec<String> {
    vec![
        "--wasi".to_owned(),
        format!("{}=inherit-network", service_uri("pgsql")),
    ]
}

/// `--dir` granting the speech service `media` at `/media`, where the
/// audio references (`media/<file>`) of `environment.sound` resolve.
pub fn speech_grant(media: &Path) -> Vec<String> {
    vec![
        "--dir".to_owned(),
        format!("{}={}::/media", service_uri("speech"), media.display()),
    ]
}

/// Whether the `.scx` document at `path`, or a document it loads by a
/// literal relative `<invoke src>` (recursively), names the service `name`'s
/// URI. Anything that is not a readable `.scx` file names none.
pub fn binds(path: &Path, name: &str) -> bool {
    fn visit(path: &Path, uri: &str, seen: &mut Vec<PathBuf>) -> bool {
        let Ok(path) = path.canonicalize() else {
            return false;
        };
        if path.extension().is_none_or(|extension| extension != "scx") || seen.contains(&path) {
            return false;
        }
        seen.push(path.clone());
        let Ok(text) = std::fs::read_to_string(&path) else {
            return false;
        };
        if text.contains(&format!("{uri}\"")) {
            return true;
        }
        let dir = path.parent().unwrap_or(Path::new("/")).to_owned();
        text.split("src=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|src| !src.contains(':'))
            .any(|src| visit(&dir.join(src), uri, seen))
    }
    visit(path, &service_uri(name), &mut Vec::new())
}

/// A scratch directory of its own for each call (tests run in parallel).
pub fn unique_scratch(name: &str) -> PathBuf {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    scratch(&format!("{name}-{sequence}"))
}

/// Writes `document` as `name` in a scratch directory of its own and runs
/// it with `args` on the logical clock with no input, until the process
/// ends (at most `limit`).
pub fn run_document(
    name: &str,
    document: &str,
    args: &[String],
    env: &[(&str, &str)],
    limit: Duration,
) -> Finished {
    let path = write(&unique_scratch("documents"), name, document);
    let mut full = args.to_vec();
    full.extend([
        "--clock".to_owned(),
        "logical".to_owned(),
        path.display().to_string(),
    ]);
    let mut run = Run::start(&full, env);
    run.end_input();
    run.finish(limit)
}

/// Copies the directory `from` to `to`, recursively.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("the directory is creatable");
    for entry in std::fs::read_dir(from).expect("the directory is readable") {
        let path = entry.expect("a directory entry").path();
        let target = to.join(path.file_name().expect("a file name"));
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("the file is copyable");
        }
    }
}

/// Copies the ability `name` (`abilities/<name>/`: its document, its
/// script, its schema twins, and any data beside them) into `abilities`,
/// and returns the path of its copied document. The memory ability comes
/// with the migration ability it invokes, and the conversation with its
/// turn detector.
pub fn copy_ability(name: &str, abilities: &Path) -> PathBuf {
    copy_tree(
        &workspace().join("abilities").join(name),
        &abilities.join(name),
    );
    if name == "memory" {
        copy_ability("migration", abilities);
    }
    if name == "conversation" {
        copy_ability("turn_detector", abilities);
    }
    if name == "autonomy" {
        copy_ability("candidates_runner", abilities);
    }
    abilities.join(name).join(format!("{name}.scx"))
}

/// The listening's abilities (abilities/listening/, its children and its
/// hearing leaves).
pub const LISTENING: [&str; 8] = [
    "listening",
    "sensitivity",
    "interpretation",
    "voice_detection",
    "voice_diarization",
    "voice_identification",
    "sound_classification",
    "speech_decoding",
];

/// Copies the listening's abilities into the layout's `abilities`
/// directory.
pub fn copy_listening(abilities: &Path) {
    for name in LISTENING {
        copy_ability(name, abilities);
    }
}

/// The speech service `<invoke>` opening tag in `abilities/vocal/vocal.scx`.
const SPEECH_INVOKE: &str = r#"<invoke id="speech" type="https://github.com/stateforward/floppy/services/speech"
            env:apikey="FLOPPY_SPEECH_API_KEY" env:baseurl="FLOPPY_SPEECH_BASE_URL">"#;

/// Copies the vocal ability into the laid-out `abilities` directory
/// with its speech service `<invoke>` rewritten to invoke
/// `tests/fixtures/phase6/speech_double.scx` (copied beside it, its `mode`
/// set to the ECMAScript expression `mode`): the double of the speech
/// service's encoding events, until the service lands.
pub fn double_speech(abilities: &Path, mode: &str) {
    let path = copy_ability("vocal", abilities);
    let vocal = std::fs::read_to_string(&path).expect("the vocal ability is readable");
    assert!(
        vocal.contains(SPEECH_INVOKE),
        "abilities/vocal/vocal.scx invokes the speech service"
    );
    std::fs::write(
        &path,
        vocal.replacen(
            SPEECH_INVOKE,
            r#"<invoke id="speech" type="scxml" src="speech_double.scx">"#,
            1,
        ),
    )
    .expect("the vocal copy is writable");
    let double =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase6/speech_double.scx"))
            .expect("the speech double is readable");
    let declared = r#"<data id="mode" expr="'encode'"/>"#;
    assert!(
        double.contains(declared),
        "the speech double declares its mode"
    );
    std::fs::write(
        abilities.join("vocal/speech_double.scx"),
        double.replacen(declared, &format!(r#"<data id="mode" expr="{mode}"/>"#), 1),
    )
    .expect("the speech double copy is writable");
}

/// Points the `<data id="database">` of the document at `path` at
/// `database` (a file path or a Postgres connection URI), so the bot or
/// ability keeps its memory there.
pub fn use_database(path: &Path, database: &str) {
    let mut text = std::fs::read_to_string(path).expect("the document is readable");
    let start = text
        .find(r#"<data id="database" expr=""#)
        .unwrap_or_else(|| panic!("{} declares <data id=\"database\">", path.display()));
    let end = start + text[start..].find("/>").expect("the <data> element closes");
    text.replace_range(
        start..end,
        &format!(r#"<data id="database" expr="'{database}'""#),
    );
    std::fs::write(path, text).expect("the document copy is writable");
}

/// One Postgres server for a Postgres-engine test, fresh for the test and
/// stopped when dropped: `PGlite` (`node $FLOPPY_PGLITE_SERVER --port 0`,
/// the `pglite-server` script of `@electric-sql/pglite-socket`,
/// `dist/scripts/server.js`: a fresh in-memory database on a free loopback
/// port), or, when only `FLOPPY_POSTGRES_BIN` is set (a directory holding
/// Postgres's `initdb` and `postgres`), a Postgres cluster made for the
/// test in a scratch directory with `trust` authentication (run as the
/// `postgres` user when the tests run as root, which Postgres refuses).
pub struct Pglite {
    child: Child,
    /// A Postgres cluster's data directory (its `postmaster.pid`).
    data: Option<PathBuf>,
    /// Its `IP:PORT`.
    pub address: String,
}

impl Pglite {
    /// Starts a server; `None` when neither `FLOPPY_PGLITE_SERVER` nor
    /// `FLOPPY_POSTGRES_BIN` is set.
    pub fn start() -> Option<Self> {
        if let Some(script) = std::env::var_os("FLOPPY_PGLITE_SERVER") {
            return Some(Self::node(&script));
        }
        let bin = PathBuf::from(std::env::var_os("FLOPPY_POSTGRES_BIN")?);
        Some(Self::postgres(&bin))
    }

    fn node(script: &std::ffi::OsStr) -> Self {
        let mut child = Command::new("node")
            .arg(script)
            .args(["--port", "0", "--max-connections", "8"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("node runs the PGlite server");
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut lines = BufReader::new(stdout).lines();
        let address = loop {
            let line = lines
                .next()
                .expect("the PGlite server reports where it listens")
                .expect("the PGlite server's output is text");
            if let Some(listening) = line.split_once("listening on ") {
                let listening: Json =
                    serde_json::from_str(listening.1).expect("the address is JSON");
                break format!(
                    "{}:{}",
                    listening["host"].as_str().expect("a host"),
                    listening["port"]
                );
            }
        };
        // Keep draining its output so the server never blocks on a full
        // pipe.
        std::thread::spawn(move || for _ in lines {});
        Self {
            child,
            data: None,
            address,
        }
    }

    fn postgres(bin: &Path) -> Self {
        let dir = unique_scratch("postgres");
        let data = dir.join("data");
        // Postgres refuses to run as root: run it as `postgres` then.
        let as_root = std::env::var("USER").is_ok_and(|user| user == "root")
            || Command::new("id")
                .arg("-u")
                .output()
                .is_ok_and(|out| out.stdout.trim_ascii() == b"0");
        let command = |program: &str| {
            let mut command = if as_root {
                let mut command = Command::new("runuser");
                command
                    .args(["-u", "postgres", "--"])
                    .arg(bin.join(program));
                command
            } else {
                Command::new(bin.join(program))
            };
            command.stdin(Stdio::null());
            command
        };
        if as_root {
            let status = Command::new("chown")
                .args(["-R", "postgres"])
                .arg(&dir)
                .status()
                .expect("chown runs");
            assert!(
                status.success(),
                "the scratch directory is given to postgres"
            );
        }
        let initialized = command("initdb")
            .args([
                "-A",
                "trust",
                "-U",
                "postgres",
                "-E",
                "UTF8",
                "--no-sync",
                "-D",
            ])
            .arg(&data)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("initdb runs");
        assert!(initialized.success(), "initdb makes the cluster");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let child = command("postgres")
            .arg("-D")
            .arg(&data)
            .args([
                "-p",
                &port.to_string(),
                "-c",
                "listen_addresses=127.0.0.1",
                "-c",
                "fsync=off",
            ])
            .arg("-k")
            .arg(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("postgres starts");
        let address = format!("127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(60);
        while std::net::TcpStream::connect(&address).is_err() {
            assert!(Instant::now() < deadline, "postgres listens on {address}");
            std::thread::sleep(Duration::from_millis(100));
        }
        Self {
            child,
            data: Some(data),
            address,
        }
    }

    /// The Postgres connection URI of its database.
    pub fn database(&self) -> String {
        format!("postgresql://postgres@{}/postgres", self.address)
    }
}

impl Drop for Pglite {
    fn drop(&mut self) {
        // A Postgres cluster stops on SIGINT to its postmaster (named by
        // `postmaster.pid`, since `runuser` may stand between); PGlite's
        // server is the child itself.
        let postmaster = self
            .data
            .as_ref()
            .and_then(|data| std::fs::read_to_string(data.join("postmaster.pid")).ok())
            .and_then(|pid| pid.lines().next().map(str::to_owned));
        match postmaster {
            Some(pid) => {
                let _stopped = Command::new("kill").args(["-INT", &pid]).status();
            }
            None => {
                let _killed = self.child.kill();
            }
        }
        let _waited = self.child.wait();
    }
}

/// `text`, a fake backend's answer to the Chat Completions `request`, as
/// the backend sends it: when the request asks for a streamed reply
/// (`"stream": true`, as every model call of reasoning, reflection and the
/// builder does, decision D101) and `text` is a completion, the
/// server-sent-event stream of the same reply (a role chunk, its content,
/// its finish, then `data: [DONE]`); any other answer unchanged.
pub fn stream_if_asked(request: &Json, text: String) -> String {
    if request["stream"] != true {
        return text;
    }
    let Ok(completion) = serde_json::from_str::<Json>(&text) else {
        return text;
    };
    let choice = &completion["choices"][0];
    let Some(content) = choice["message"]["content"].as_str() else {
        return text;
    };
    let chunk = |delta: Json, finish: Json| {
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            })
        )
    };
    [
        chunk(
            serde_json::json!({"role": "assistant", "content": ""}),
            Json::Null,
        ),
        chunk(serde_json::json!({"content": content}), Json::Null),
        chunk(serde_json::json!({}), choice["finish_reason"].clone()),
        "data: [DONE]\n\n".to_owned(),
    ]
    .concat()
}

/// Answers the Chat Completions `request` on `stream` with `content` the
/// way a slow backend does: a streamed request (`"stream": true`) gets its
/// head at once and then `content` in `pieces` server-sent-event chunks
/// `gap` apart, then its finish and `data: [DONE]`; with `stall`, only the
/// first piece, and then nothing until the client closes the connection
/// (at most two minutes). A request that is not streamed gets the whole
/// completion once all of it is ready, `pieces` × `gap` later, as a
/// backend that answers only when it has finished. The body ends when the
/// connection closes.
pub fn answer_slowly(
    stream: &mut std::net::TcpStream,
    request: &Json,
    content: &str,
    pieces: usize,
    gap: std::time::Duration,
    stall: bool,
) {
    use std::io::Write as _;
    let streamed = request["stream"] == true;
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {}\r\nconnection: close\r\n\r\n",
        if streamed {
            "text/event-stream"
        } else {
            "application/json"
        }
    );
    if !streamed {
        std::thread::sleep(gap * u32::try_from(pieces).unwrap_or(u32::MAX));
        let completion = serde_json::json!({
            "object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
        });
        let _ = write!(stream, "{head}{completion}");
        return;
    }
    let chunk = |delta: Json, finish: Json| {
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            })
        )
    };
    let text: Vec<char> = content.chars().collect();
    let size = text.len().div_ceil(pieces.max(1)).max(1);
    if stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.flush())
        .is_err()
    {
        return;
    }
    for (index, piece) in text.chunks(size).enumerate() {
        if index > 0 {
            if stall {
                std::thread::sleep(std::time::Duration::from_secs(120));
                return;
            }
            std::thread::sleep(gap);
        }
        let piece: String = piece.iter().collect();
        let written = stream
            .write_all(chunk(serde_json::json!({"content": piece}), Json::Null).as_bytes())
            .and_then(|()| stream.flush());
        if written.is_err() {
            return;
        }
    }
    let _ = write!(
        stream,
        "{}data: [DONE]\n\n",
        chunk(serde_json::json!({}), Json::String("stop".to_owned()))
    );
}

/// Answers a streamed request on `stream` as a gateway does while its model
/// says nothing: the head at once, then only keep-alive comments
/// (`: ping`) every `gap`, for at most a minute.
pub fn answer_with_pings(stream: &mut std::net::TcpStream, gap: std::time::Duration) {
    use std::io::Write as _;
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(60) {
        if stream
            .write_all(b": ping\n\n")
            .and_then(|()| stream.flush())
            .is_err()
        {
            return;
        }
        std::thread::sleep(gap);
    }
}
