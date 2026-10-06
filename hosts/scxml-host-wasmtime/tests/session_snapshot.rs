//! Wasmtime golden harness of the read-only session snapshot (see
//! `docs/session-snapshot.md`): every
//! `tests/fixtures/session-snapshot/<name>.ops.json` is replayed through
//! the shipped components, and each requested checkpoint's document must
//! equal `golden/<name>/NNNN.snapshot.json` byte for byte once its component
//! digests are normalised. The
//! consecutive-checkpoint diffs must equal `diff/<name>/NNNN.diff.{json,txt}`,
//! and every golden must validate against the published JSON Schemas. Any
//! snapshot failure fails the test.
//!
//! Component digests: `snapshot.components[*].sha256` identifies the build
//! that produced the components, and release component bytes differ between
//! build hosts. Before comparing (or blessing), the harness asserts that each
//! produced document's component digest equals the SHA-256 of the release
//! artifact loaded for that role. Then it replaces those digests with a
//! fixed all-zero placeholder, re-derives the envelope `sha256`, and asserts
//! that nothing else changed. The goldens store the placeholder and are identical on every
//! host, and the diffs are computed from the normalised documents.
//!
//! Ops: `{"options":{"exclude":[…],"trace_context":"strip"|"preserve"},
//! "ops":[…]}` with `{"op":"start"}` (exactly once, first),
//! `{"op":"send","event":E}` (a scripted external event to the root),
//! `{"op":"advance","ms":N}` (the browser's clock step; the Wasmtime drain
//! advances the clock itself, so here it only forbids a later `send`), and
//! `{"op":"snapshot","checkpoint":N}` (strictly ascending checkpoint
//! ordinals, as `docs/session-snapshot.md` "When a snapshot can be taken"
//! numbers them). `<invoke src>` documents resolve by exact `src` to files
//! beside the fixture. The root `_sessionid` is `session`.
//!
//! Host coverage: the browser host replays the same ops against the same
//! goldens, taking each snapshot at its op's position. A browser `send`
//! settles fully, so a checkpoint in the middle of a Wasmtime drain (a
//! drain iteration while forwarded, routed, or delayed events are still
//! queued) has no browser counterpart. Such a snapshot op carries
//! `"hosts":"wasmtime"`; the Wasmtime harness snapshots it like any other,
//! and the browser parity test skips it. Without the field an op is shared
//! by both hosts, and every fixture has at least one shared snapshot op. So
//! every fixture proves cross-host byte parity, apart from the two
//! documented host differences (docs/session-snapshot.md, "Parity
//! fixtures"). First, the `BasicHTTP` entries of `system.ioprocessors`: this
//! host links `BasicHTTP` and the browser does not. Second, event
//! `trace_context` carriers under `preserve`: each embedder's telemetry
//! provider mints them. The browser test removes exactly those values,
//! re-derives `sha256`, and compares the remaining bytes exactly.
//!
//! The harness runs on `TelemetryProviders::deterministic()`: under
//! `preserve` a carrier derived from a random root span would make the
//! document run-specific, so the harness pins sequential IDs and the goldens
//! stay byte-identical across runs. Goldens are written only with
//! `SCXML_BLESS=1` (`sh scripts/bless-fixtures.sh --session-snapshot`).
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies the release
//! components into `target/session-host-proof`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;

use scxml_host_wasmtime::scxml_snapshot::{ComponentRole, Options};
use scxml_host_wasmtime::{
    DocumentKind, EmbeddedDocument, MachineScripts, ReleaseComponents, RunFailure, ScriptedEvent,
    SessionEmbedder, SessionHost, SessionHostError, SessionOutcome, SessionRun, SnapshotFailure,
    SnapshotPoints, SnapshotRequest, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

/// The release components every fixture run links.
const COMPONENTS: [&str; 8] = [
    "scxml_machine",
    "scxml_compiler",
    "scxml_data_model_ecmascript",
    "scxml_data_model_xpath",
    "scxml_data_model_null",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_clock",
];

/// Root `_sessionid` of every fixture run (shared with the browser host).
const ROOT_SESSION_ID: &str = "session";
/// Command budget of one fixture run.
const MAX_COMMANDS: u64 = 4_096;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent")
}

fn fixtures() -> PathBuf {
    workspace().join("tests/fixtures/session-snapshot")
}

fn components() -> &'static ReleaseComponents {
    static COMPONENTS_CELL: OnceLock<ReleaseComponents> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = workspace();
        let target = workspace.join("target/session-host-proof");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS)
            .current_dir(workspace)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let release = target.join("wasm-artifacts/release");
        let manifest = std::fs::read(release.join("manifest.tsv")).expect("manifest is readable");
        let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical");
        ReleaseComponents::load(&release, trusted)
            .unwrap_or_else(|error| panic!("the release components load: {error}"))
    })
}

/// The SHA-256 of each snapshot role's release artifact, hashed from the
/// file [`components`] loaded for that role. A produced document's
/// `components[*].sha256` must equal these before normalization replaces
/// them, so a swapped role-to-digest mapping fails the harness.
fn release_digests() -> &'static BTreeMap<ComponentRole, String> {
    static DIGESTS_CELL: OnceLock<BTreeMap<ComponentRole, String>> = OnceLock::new();
    DIGESTS_CELL.get_or_init(|| {
        components();
        let release = workspace().join("target/session-host-proof/wasm-artifacts/release");
        [
            (ComponentRole::Clock, "scxml_clock"),
            (
                ComponentRole::DataModelEcmascript,
                "scxml_data_model_ecmascript",
            ),
            (ComponentRole::DataModelNull, "scxml_data_model_null"),
            (ComponentRole::DataModelXpath, "scxml_data_model_xpath"),
            (ComponentRole::IoProcessorScxml, "scxml_ioprocessor_scxml"),
            (ComponentRole::Machine, "scxml_machine"),
        ]
        .into_iter()
        .map(|(role, name)| {
            let bytes = std::fs::read(release.join(format!("{name}.virtualized.wasm")))
                .unwrap_or_else(|error| panic!("{name}: the release artifact reads: {error}"));
            (role, format!("{:x}", Sha256::digest(&bytes)))
        })
        .collect()
    })
}

fn host() -> SessionHost {
    SessionHost::new(
        &components().session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::deterministic(),
    )
    .expect("the verified components link")
}

/// `<invoke src>` documents: files beside the fixture, by exact `src`.
struct FixtureDocuments;

impl SessionEmbedder for FixtureDocuments {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        let valid = !uri.is_empty()
            && uri
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && !uri.starts_with('.');
        valid
            .then(|| std::fs::read_to_string(fixtures().join(uri)).ok())
            .flatten()
            .map(EmbeddedDocument::from)
    }

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

/// One parsed `<name>.ops.json`.
#[derive(Debug)]
struct Fixture {
    name: String,
    options: Options,
    events: Vec<String>,
    checkpoints: Vec<u32>,
}

fn parse_ops(name: &str, text: &str) -> Fixture {
    let document: serde_json::Value =
        serde_json::from_str(text).unwrap_or_else(|error| panic!("{name}: ops JSON: {error}"));
    let object = document
        .as_object()
        .unwrap_or_else(|| panic!("{name}: ops is an object"));
    assert_eq!(
        object.keys().collect::<BTreeSet<_>>(),
        BTreeSet::from([&"ops".to_owned(), &"options".to_owned()]),
        "{name}: ops has exactly `options` and `ops`"
    );
    let options: Options = serde_json::from_value(object["options"].clone())
        .unwrap_or_else(|error| panic!("{name}: options: {error}"));
    let ops = object["ops"]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: ops is an array"));
    let mut events = Vec::new();
    let mut checkpoints: Vec<u32> = Vec::new();
    let mut shared_snapshots = 0_usize;
    let mut advanced = false;
    for (index, op) in ops.iter().enumerate() {
        let fields = op
            .as_object()
            .unwrap_or_else(|| panic!("{name}: op {index} is an object"));
        let keys = fields.keys().map(String::as_str).collect::<BTreeSet<_>>();
        match (index, fields.get("op").and_then(serde_json::Value::as_str)) {
            (0, Some("start")) => assert_eq!(keys, BTreeSet::from(["op"]), "{name}: op {index}"),
            (0, _) | (_, Some("start")) => panic!("{name}: `start` is exactly the first op"),
            (_, Some("send")) => {
                assert_eq!(keys, BTreeSet::from(["event", "op"]), "{name}: op {index}");
                assert!(
                    !advanced,
                    "{name}: op {index}: Wasmtime scripts every send before the drain, so no send may follow an advance"
                );
                events.push(
                    fields["event"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{name}: op {index}: event is a string"))
                        .to_owned(),
                );
            }
            (_, Some("advance")) => {
                assert_eq!(keys, BTreeSet::from(["ms", "op"]), "{name}: op {index}");
                assert!(
                    fields["ms"].as_u64().is_some(),
                    "{name}: op {index}: ms is an unsigned integer"
                );
                advanced = true;
            }
            (_, Some("snapshot")) => {
                let shared = match fields.get("hosts") {
                    None => true,
                    Some(serde_json::Value::String(hosts)) if hosts == "wasmtime" => false,
                    Some(other) => {
                        panic!("{name}: op {index}: `hosts` is \"wasmtime\", got {other}")
                    }
                };
                let expected_keys = if shared {
                    BTreeSet::from(["checkpoint", "op"])
                } else {
                    BTreeSet::from(["checkpoint", "hosts", "op"])
                };
                assert_eq!(keys, expected_keys, "{name}: op {index}");
                shared_snapshots += usize::from(shared);
                let checkpoint = fields["checkpoint"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value <= 9_999)
                    .unwrap_or_else(|| panic!("{name}: op {index}: checkpoint is 0..=9999"));
                assert!(
                    checkpoints.last().is_none_or(|last| *last < checkpoint),
                    "{name}: op {index}: checkpoints are strictly ascending"
                );
                checkpoints.push(checkpoint);
            }
            (_, other) => panic!("{name}: op {index}: unknown op {other:?}"),
        }
    }
    assert!(!ops.is_empty(), "{name}: ops starts with `start`");
    assert!(
        shared_snapshots > 0,
        "{name}: at least one snapshot op both hosts take (no `hosts` field)"
    );
    Fixture {
        name: name.to_owned(),
        options,
        events,
        checkpoints,
    }
}

fn load_fixtures() -> Vec<(Fixture, String)> {
    let mut names = std::fs::read_dir(fixtures())
        .expect("the fixture directory is readable")
        .map(|entry| entry.expect("a fixture entry").file_name())
        .filter_map(|name| {
            name.to_str()
                .and_then(|name| name.strip_suffix(".ops.json"))
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    names.sort();
    assert!(!names.is_empty(), "session snapshot fixtures exist");
    names
        .into_iter()
        .map(|name| {
            let ops = std::fs::read_to_string(fixtures().join(format!("{name}.ops.json")))
                .expect("ops are readable");
            let source = std::fs::read_to_string(fixtures().join(format!("{name}.scx")))
                .unwrap_or_else(|error| panic!("{name}.scx: {error}"));
            (parse_ops(&name, &ops), source)
        })
        .collect()
}

fn lower(name: &str, source: &str) -> (Vec<u8>, Option<String>) {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        format!("{name}.scx"),
        source,
    ))
    .unwrap_or_else(|_| panic!("{name}: the fixture parses"));
    let ir = scxml_compiler_core::lower(&document)
        .unwrap_or_else(|_| panic!("{name}: the fixture lowers"));
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn run(
    host: &SessionHost,
    fixture: &Fixture,
    ir: &[u8],
    data_model: Option<&str>,
    points: SnapshotPoints,
) -> Result<SessionOutcome, SessionHostError> {
    let events = fixture
        .events
        .iter()
        .map(ScriptedEvent::named)
        .collect::<Vec<_>>();
    host.run(&SessionRun {
        machine_ir: ir,
        data_model,
        session_id: ROOT_SESSION_ID,
        events: &events,
        external_client: None,
        embedder: Rc::new(FixtureDocuments),
        max_commands: MAX_COMMANDS,
        snapshots: SnapshotRequest {
            points,
            options: fixture.options.clone(),
        },
    })
}

fn golden_dir(name: &str) -> PathBuf {
    fixtures().join("golden").join(name)
}

fn golden_name(checkpoint: u32) -> String {
    format!("{checkpoint:04}.snapshot.json")
}

/// The `diff/<name>` directory of consecutive-checkpoint diff goldens.
fn diff_dir(name: &str) -> PathBuf {
    fixtures().join("diff").join(name)
}

fn diff_name(checkpoint: u32) -> String {
    format!("{checkpoint:04}.diff")
}

/// Compares (or, under `SCXML_BLESS=1`, writes) a set of named byte files in
/// `directory`; stray files fail outside a bless run. Returns the failures.
fn check_named_files(
    label: &str,
    directory: &Path,
    files: &BTreeMap<String, Vec<u8>>,
) -> Vec<String> {
    if std::env::var_os("SCXML_BLESS").is_some_and(|value| value == "1") {
        std::fs::create_dir_all(directory).expect("the golden directory is creatable");
        for entry in std::fs::read_dir(directory).expect("the golden directory is readable") {
            let entry = entry.expect("a golden entry");
            let file = entry.file_name().to_string_lossy().into_owned();
            if !files.contains_key(&file) {
                std::fs::remove_file(entry.path()).expect("a stray golden is removable");
            }
        }
        for (name, bytes) in files {
            std::fs::write(directory.join(name), bytes).expect("the golden is writable");
        }
        return Vec::new();
    }
    let mut failures = Vec::new();
    let present = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .map(|entry| {
                    entry
                        .expect("a golden entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    for stray in present.difference(&files.keys().cloned().collect::<BTreeSet<_>>()) {
        failures.push(format!("{label}: stray golden {stray}"));
    }
    for (name, bytes) in files {
        let path = directory.join(name);
        match std::fs::read(&path) {
            Err(_) => failures.push(format!(
                "{label}: missing golden {} (bless with SCXML_BLESS=1)",
                path.display()
            )),
            Ok(golden) if golden != *bytes => failures.push(format!(
                "{label}: {name} differs\n--- golden ---\n{}\n--- produced ---\n{}\n--- end ---",
                String::from_utf8_lossy(&golden),
                String::from_utf8_lossy(bytes),
            )),
            Ok(_) => {}
        }
    }
    failures
}

/// The value every `snapshot.components[*].sha256` is replaced with before a
/// document is compared or blessed (docs/session-snapshot.md, "Parity
/// fixtures"). Component digests identify the build that produced the
/// components, not the snapshot content, and release component bytes
/// differ between build hosts, so the goldens carry this placeholder
/// instead and stay identical on every host.
const COMPONENT_DIGEST_PLACEHOLDER: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The JSON pointers at which two parsed values differ.
fn differing_paths(left: &serde_json::Value, right: &serde_json::Value, path: &str) -> Vec<String> {
    use serde_json::Value;
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => left
            .keys()
            .chain(right.keys().filter(|key| !left.contains_key(*key)))
            .flat_map(|key| {
                differing_paths(
                    left.get(key).unwrap_or(&Value::Null),
                    right.get(key).unwrap_or(&Value::Null),
                    &format!("{path}/{key}"),
                )
            })
            .collect(),
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => left
            .iter()
            .zip(right)
            .enumerate()
            .flat_map(|(index, (left, right))| {
                differing_paths(left, right, &format!("{path}/{index}"))
            })
            .collect(),
        _ if left == right => Vec::new(),
        _ => vec![path.to_owned()],
    }
}

/// Whether `path` is `/snapshot/components/<index>/sha256`.
fn is_component_digest(path: &str) -> bool {
    path.strip_prefix("/snapshot/components/")
        .and_then(|rest| rest.strip_suffix("/sha256"))
        .is_some_and(|index| index.parse::<usize>().is_ok())
}

/// A stored snapshot golden's codec failures: `check` (what
/// `scx snapshot check` runs) must accept it, since its `sha256` was
/// re-derived, and every component digest must be the placeholder.
fn stored_golden_failures(label: &str, bytes: &[u8], instance: &serde_json::Value) -> Vec<String> {
    let mut failures = Vec::new();
    match scxml_host_wasmtime::scxml_snapshot::check(bytes) {
        Ok(issues) if issues.is_empty() => {}
        Ok(issues) => failures.push(format!("{label}: {issues:?}")),
        Err(error) => failures.push(format!("{label}: {error}")),
    }
    let normalized = instance["snapshot"]["components"]
        .as_array()
        .is_some_and(|components| {
            components
                .iter()
                .all(|component| component["sha256"] == COMPONENT_DIGEST_PLACEHOLDER)
        });
    if !normalized {
        failures.push(format!(
            "{label}: every component digest must be the placeholder"
        ));
    }
    failures
}

/// One snapshot document with every component digest replaced by
/// [`COMPONENT_DIGEST_PLACEHOLDER`] and the envelope `sha256` re-derived
/// over the canonical `snapshot` bytes. Panics unless the input is a valid
/// document whose every component digest is the release digest of its role
/// ([`release_digests`]), the result is a valid document, and only those
/// digests changed.
fn normalize_component_digests(label: &str, bytes: &[u8]) -> Vec<u8> {
    let original = scxml_host_wasmtime::scxml_snapshot::decode(bytes)
        .unwrap_or_else(|error| panic!("{label}: the document decodes: {error}"));
    let expected = release_digests();
    for component in &original.snapshot.components {
        assert_eq!(
            Some(&component.sha256),
            expected.get(&component.role),
            "{label}: the {:?} digest must be that of the component loaded for the role",
            component.role
        );
    }
    let mut document = original;
    for component in &mut document.snapshot.components {
        COMPONENT_DIGEST_PLACEHOLDER.clone_into(&mut component.sha256);
    }
    let snapshot = serde_json::to_vec(&document.snapshot).expect("the snapshot serializes");
    document.sha256 = format!("{:x}", Sha256::digest(&snapshot));
    let normalized = serde_json::to_vec(&document).expect("the document serializes");
    scxml_host_wasmtime::scxml_snapshot::decode(&normalized)
        .unwrap_or_else(|error| panic!("{label}: the normalized document decodes: {error}"));
    let before: serde_json::Value = serde_json::from_slice(bytes).expect("a JSON document");
    let after: serde_json::Value = serde_json::from_slice(&normalized).expect("a JSON document");
    let unexpected = differing_paths(&before, &after, "")
        .into_iter()
        .filter(|path| path != "/sha256" && !is_component_digest(path))
        .collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "{label}: normalization changes only the component digests and sha256, not {unexpected:?}"
    );
    normalized
}

/// Compares (or, under `SCXML_BLESS=1`, writes) one fixture's snapshot
/// goldens; returns the failures.
fn check_goldens(fixture: &Fixture, documents: &[(u32, Vec<u8>)]) -> Vec<String> {
    let files = documents
        .iter()
        .map(|(checkpoint, bytes)| (golden_name(*checkpoint), bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    check_named_files(&fixture.name, &golden_dir(&fixture.name), &files)
}

/// Compares (or, under `SCXML_BLESS=1`, writes) one fixture's
/// consecutive-checkpoint diff goldens, `<k>.diff.json` + `<k>.diff.txt`
/// named by the later checkpoint of each pair. The diff
/// uses the CLI's documented default: trace contexts are ignored, so a
/// `preserve` carrier change between checkpoints is not a difference.
fn check_diff_goldens(fixture: &Fixture, documents: &[(u32, Vec<u8>)]) -> Vec<String> {
    let options = scxml_host_wasmtime::scxml_snapshot::DiffOptions {
        include_trace_context: false,
    };
    let mut files = BTreeMap::new();
    for pair in documents.windows(2) {
        let (before_checkpoint, before_bytes) = (&pair[0].0, &pair[0].1);
        let (after_checkpoint, after_bytes) = (&pair[1].0, &pair[1].1);
        let document = scxml_host_wasmtime::scxml_snapshot::diff_documents(
            &scxml_host_wasmtime::scxml_snapshot::decode(before_bytes).unwrap_or_else(|error| {
                panic!("{}: {before_checkpoint} decodes: {error}", fixture.name)
            }),
            &scxml_host_wasmtime::scxml_snapshot::decode(after_bytes).unwrap_or_else(|error| {
                panic!("{}: {after_checkpoint} decodes: {error}", fixture.name)
            }),
            options,
        );
        files.insert(
            format!("{}.json", diff_name(*after_checkpoint)),
            serde_json::to_vec(&document).expect("the diff document serializes"),
        );
        files.insert(
            format!("{}.txt", diff_name(*after_checkpoint)),
            scxml_host_wasmtime::scxml_snapshot::render_diff(
                &document,
                &golden_name(*before_checkpoint),
                &golden_name(*after_checkpoint),
                scxml_host_wasmtime::scxml_snapshot::Color::Never,
            )
            .into_bytes(),
        );
    }
    check_named_files(
        &format!("{} (diff)", fixture.name),
        &diff_dir(&fixture.name),
        &files,
    )
}

#[test]
fn session_snapshot_goldens() {
    let host = host();
    let mut failures = Vec::new();
    for (fixture, source) in load_fixtures() {
        let (ir, data_model) = lower(&fixture.name, &source);
        let data_model = data_model.as_deref();
        let baseline = run(&host, &fixture, &ir, data_model, SnapshotPoints::None)
            .unwrap_or_else(|error| panic!("{}: the run without snapshots: {error}", fixture.name));
        let outcome = run(
            &host,
            &fixture,
            &ir,
            data_model,
            SnapshotPoints::At(fixture.checkpoints.clone()),
        )
        .unwrap_or_else(|error| panic!("{}: snapshot run failed: {error}", fixture.name));
        assert_eq!(
            (
                outcome.terminal,
                &outcome.configuration,
                outcome.commands,
                &outcome.trace,
                &outcome.root_logs,
            ),
            (
                baseline.terminal,
                &baseline.configuration,
                baseline.commands,
                &baseline.trace,
                &baseline.root_logs,
            ),
            "{}: snapshots must not perturb the run",
            fixture.name
        );
        let documents = outcome
            .snapshots
            .into_iter()
            .map(|snapshot| {
                let label = format!("{}: checkpoint {}", fixture.name, snapshot.checkpoint);
                (
                    snapshot.checkpoint,
                    normalize_component_digests(&label, &snapshot.bytes),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            documents
                .iter()
                .map(|(checkpoint, _)| *checkpoint)
                .collect::<Vec<_>>(),
            fixture.checkpoints,
            "{}: one document per requested checkpoint",
            fixture.name
        );
        failures.extend(check_goldens(&fixture, &documents));
        failures.extend(check_diff_goldens(&fixture, &documents));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ops_files_are_well_formed_and_name_their_documents() {
    // Parsing panics on any malformed op; lowering proves each document.
    for (fixture, source) in load_fixtures() {
        lower(&fixture.name, &source);
    }
}

/// Every checked-in golden — snapshot and diff — must validate against the
/// published JSON Schema of its kind.
#[test]
fn goldens_match_the_published_schemas() {
    let snapshot_schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            workspace().join("crates/scxml-snapshot/schema/scxml.session-snapshot.v2.schema.json"),
        )
        .expect("the snapshot schema is readable"),
    )
    .expect("the snapshot schema parses");
    let diff_schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            workspace()
                .join("crates/scxml-snapshot/schema/scxml.session-snapshot-diff.v2.schema.json"),
        )
        .expect("the diff schema is readable"),
    )
    .expect("the diff schema parses");
    // Both schemas' `urn:` `$id`s are registered as resources so internal
    // and cross-schema references resolve without a retrieval scheme.
    let registry = jsonschema::Registry::new()
        .add(
            snapshot_schema["$id"].as_str().expect("snapshot $id"),
            snapshot_schema.clone(),
        )
        .expect("the snapshot schema registers")
        .add(
            diff_schema["$id"].as_str().expect("diff $id"),
            diff_schema.clone(),
        )
        .expect("the diff schema registers")
        .prepare()
        .expect("the schema registry prepares");
    let snapshot_validator = jsonschema::options()
        .with_registry(&registry)
        .build(&snapshot_schema)
        .expect("the snapshot schema compiles");
    let diff_validator = jsonschema::options()
        .with_registry(&registry)
        .build(&diff_schema)
        .expect("the diff schema compiles");
    let mut checked = 0u32;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(fixtures().join("golden")).expect("the golden root is readable")
    {
        let fixture = entry.expect("a golden fixture").file_name().into_string();
        let Ok(fixture) = fixture else {
            failures.push("a golden fixture name is not UTF-8".to_owned());
            continue;
        };
        for file in
            std::fs::read_dir(golden_dir(&fixture)).expect("the golden directory is readable")
        {
            let path = file.expect("a golden file").path();
            let bytes = std::fs::read(&path).expect("the golden is readable");
            let instance: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{fixture}: {path:?}: {error}"));
            if let Err(error) = snapshot_validator.validate(&instance) {
                failures.push(format!("{fixture}: {path:?}: {error}"));
            }
            failures.extend(stored_golden_failures(
                &format!("{fixture}: {path:?}"),
                &bytes,
                &instance,
            ));
            checked += 1;
        }
    }
    if let Ok(diff_entries) = std::fs::read_dir(fixtures().join("diff")) {
        for entry in diff_entries {
            let fixture = entry.expect("a diff fixture").file_name().into_string();
            let Ok(fixture) = fixture else {
                failures.push("a diff fixture name is not UTF-8".to_owned());
                continue;
            };
            for file in
                std::fs::read_dir(diff_dir(&fixture)).expect("the diff directory is readable")
            {
                let path = file.expect("a diff golden file").path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let instance: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(&path).expect("the diff golden is readable"),
                )
                .unwrap_or_else(|error| panic!("{fixture}: {path:?}: {error}"));
                if let Err(error) = diff_validator.validate(&instance) {
                    failures.push(format!("{fixture}: {path:?}: {error}"));
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "at least one golden is schema-checked");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_run_without_snapshots_reports_none() {
    let host = host();
    for (fixture, source) in load_fixtures() {
        let (ir, data_model) = lower(&fixture.name, &source);
        let outcome = run(
            &host,
            &fixture,
            &ir,
            data_model.as_deref(),
            SnapshotPoints::None,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
        assert!(outcome.snapshots.is_empty(), "{}", fixture.name);
    }
}

/// The `event` attribute vocabulary is metadata only: the fixture that
/// carries it on transitions, a `<raise>`, a `<send>`, and a `<donedata>`
/// runs exactly as the same document without it (configuration, logs,
/// trace, terminal state, and engine commands), and only its snapshots'
/// transitions show the schemas.
#[test]
fn event_schemas_never_change_execution() {
    let host = host();
    let (fixture, source) = load_fixtures()
        .into_iter()
        .find(|(fixture, _)| fixture.name == "transition-event-schemas")
        .expect("the transition-event-schemas fixture exists");
    let plain = strip_event_vocabulary(&source);
    assert!(
        !plain.contains(" event:schema=") && !plain.contains(" event:schemaexpr="),
        "every event attribute is removed"
    );
    let mut outcomes = Vec::new();
    for document in [&source, &plain] {
        let (ir, data_model) = lower(&fixture.name, document);
        let outcome = run(
            &host,
            &fixture,
            &ir,
            data_model.as_deref(),
            SnapshotPoints::None,
        )
        .expect("the fixture runs");
        outcomes.push(outcome);
    }
    let [described, plain] = outcomes.as_slice() else {
        panic!("two runs");
    };
    assert_eq!(described.terminal, plain.terminal);
    assert_eq!(described.configuration, plain.configuration);
    assert_eq!(described.commands, plain.commands);
    assert_eq!(described.root_logs, plain.root_logs);
    assert_eq!(described.trace, plain.trace);
    assert!(!described.trace.is_empty() && !described.root_logs.is_empty());
}

/// `source` without the `event` namespace declaration and its attributes
/// (each `event:NAME='...'` or `event:NAME="..."`).
fn strip_event_vocabulary(source: &str) -> String {
    let mut out = source.replace(
        r#"xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event""#,
        "",
    );
    while let Some(start) = out.find(" event:") {
        let rest = &out[start..];
        let equals = rest.find('=').expect("an attribute has a value");
        let quote = rest[equals + 1..].chars().next().expect("a quoted value");
        let close = rest[equals + 2..].find(quote).expect("the value is closed");
        out.replace_range(start..=start + equals + 2 + close, "");
    }
    out
}

#[test]
fn an_unreached_checkpoint_fails_the_run() {
    let host = host();
    let (fixture, source) = load_fixtures()
        .into_iter()
        .find(|(fixture, _)| fixture.name == "history")
        .expect("the history fixture exists");
    let (ir, data_model) = lower(&fixture.name, &source);
    // History has six checkpoints (0..=5); 6 is never reached.
    let error = run(
        &host,
        &fixture,
        &ir,
        data_model.as_deref(),
        SnapshotPoints::At(vec![6]),
    )
    .expect_err("checkpoint 6 is never reached");
    assert!(
        matches!(
            error.failure,
            RunFailure::Snapshot(SnapshotFailure::Unreached(6))
        ),
        "{error}"
    );
}

/// A live session on the logical clock replays each fixture's ops the way
/// an embedder would (`start` opens, `send` processes one event to
/// a stable configuration, `advance` moves the clock), and every snapshot both hosts
/// take equals the batch run's document at that checkpoint byte for byte.
/// Fixtures under `trace_context: preserve` are skipped: their carriers
/// come from span ids the two drivers mint in different orders.
#[test]
fn live_sessions_reproduce_every_shared_checkpoint_document() {
    use scxml_host_wasmtime::scxml_snapshot::TraceContextPolicy;
    use scxml_host_wasmtime::{LiveClock, LiveHttp, SessionOpen};

    let host = host();
    let mut compared = 0;
    for (fixture, source) in load_fixtures() {
        if fixture.options.trace_context == TraceContextPolicy::Preserve {
            continue;
        }
        let name = &fixture.name;
        let (ir, data_model) = lower(name, &source);
        let batch = run(
            &host,
            &fixture,
            &ir,
            data_model.as_deref(),
            SnapshotPoints::Every,
        )
        .unwrap_or_else(|error| panic!("{name}: the batch run completes: {error}"));
        let ops: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures().join(format!("{name}.ops.json")))
                .expect("ops are readable"),
        )
        .expect("ops are JSON");
        let mut live = None;
        for op in ops["ops"].as_array().expect("ops is an array") {
            match op["op"].as_str() {
                Some("start") => {
                    live = Some(
                        host.open(SessionOpen {
                            machine_ir: &ir,
                            data_model: data_model.as_deref(),
                            session_id: ROOT_SESSION_ID,
                            embedder: Rc::new(FixtureDocuments),
                            max_commands: MAX_COMMANDS,
                            clock: LiveClock::Logical,
                            record: false,
                            replay: None,
                            http: LiveHttp::default(),
                        })
                        .unwrap_or_else(|error| panic!("{name}: the session opens: {error}")),
                    );
                }
                Some("send") => {
                    let session = live.as_mut().expect("start is first");
                    let event = op["event"].as_str().expect("event is a string");
                    session
                        .send(&ScriptedEvent::named(event))
                        .unwrap_or_else(|error| panic!("{name}: send {event}: {error}"));
                }
                Some("advance") => {
                    let session = live.as_mut().expect("start is first");
                    let now = session.now_ms().expect("the clock reads");
                    let ms = op["ms"].as_u64().expect("ms is an integer");
                    session
                        .advance_to(now + ms)
                        .unwrap_or_else(|error| panic!("{name}: advance {ms}: {error}"));
                }
                Some("snapshot") if op.get("hosts").is_none() => {
                    let session = live.as_mut().expect("start is first");
                    let checkpoint = u32::try_from(op["checkpoint"].as_u64().expect("checkpoint"))
                        .expect("a u32 ordinal");
                    let expected = batch
                        .snapshots
                        .iter()
                        .find(|snapshot| snapshot.checkpoint == checkpoint)
                        .unwrap_or_else(|| panic!("{name}: batch checkpoint {checkpoint}"));
                    let produced = session
                        .snapshot(&fixture.options)
                        .unwrap_or_else(|error| panic!("{name}: live snapshot: {error}"));
                    assert!(
                        produced == expected.bytes,
                        "{name}: checkpoint {checkpoint} differs\n--- batch ---\n{}\n--- live ---\n{}",
                        String::from_utf8_lossy(&expected.bytes),
                        String::from_utf8_lossy(&produced),
                    );
                    compared += 1;
                }
                Some("snapshot") => {}
                other => panic!("{name}: unknown op {other:?}"),
            }
        }
        live.expect("every fixture starts")
            .close()
            .unwrap_or_else(|error| panic!("{name}: the session closes: {error}"));
    }
    assert!(compared >= 10, "the parity covers the shared checkpoints");
}
