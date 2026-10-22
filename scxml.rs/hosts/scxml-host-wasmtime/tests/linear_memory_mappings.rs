//! Memory mappings per live session under the Wasmtime session host, on
//! Linux.
//!
//! Linux limits the memory mappings of one process (`vm.max_map_count`:
//! 65,530 by default), and a session is several component instances, each
//! with its own linear memories. SCXML 6.4 sets no depth for invoked
//! sessions, so the host's engine must keep a live session's mappings few:
//! a chain of `DEPTH` waiting sessions counts the process's mappings
//! (`/proc/self/maps`) when its first and its last level load their
//! documents, while every level above is live, and the levels in between
//! add fewer than one mapping per `SESSIONS_PER_MAPPING` sessions.
//!
//! This suite is its own test binary, so no other test's sessions run in
//! its process while it counts. `scripts/verify-wasm-capabilities.sh`
//! builds and verifies the release components into
//! `target/session-host-proof` (this suite's own artifacts directory inside
//! it).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(all(feature = "wasmtime", target_os = "linux"))]

mod prebuilt;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;

use scxml_host_wasmtime::{
    DocumentKind, EmbeddedDocument, MachineScripts, ReleaseComponents, ScriptedEvent,
    SessionEmbedder, SessionHost, SessionRun, SnapshotRequest, TelemetryProviders, Terminal,
    TrustedManifestDigest, WasmtimeHostConfig, WasmtimeStoreConfig,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

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

/// Levels of the chain.
const DEPTH: usize = 400;

/// The first level whose document load is counted; the levels below it
/// warm the engine's caches.
const FIRST_COUNTED: usize = 50;

/// Fuel of the root's start, which starts the whole chain in one host call:
/// ten million per level (a level's start compiles its document, loads its
/// session, and runs its start).
const CHAIN_FUEL: u64 = 10_000_000 * DEPTH as u64;

/// Live waiting sessions per memory mapping they may add. A session's
/// linear memories are each one read-write mapping that merges with its
/// neighbors, so the 350 counted sessions add none (measured in Docker,
/// linux/amd64: 242 mappings before and after, 8,768 KiB of address space
/// and 301 KiB resident a session, printed by this test). One unmerged
/// mapping a session would add 350; a guard region, a growth reservation,
/// or a copy-on-write memory image after or inside each memory makes it
/// three or four mappings that never merge, for every memory of every
/// session.
const SESSIONS_PER_MAPPING: usize = 50;

fn components() -> ReleaseComponents {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent");
    let release = prebuilt::release_dir().unwrap_or_else(|| {
        let target = workspace.join("target/session-host-proof");
        let release = target.join("wasm-artifacts/linear-memory-mappings");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .arg("--artifacts-dir")
            .arg(&release)
            .args(COMPONENTS)
            .current_dir(workspace)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        release
    });
    let manifest = std::fs::read(release.join("manifest.tsv")).expect("manifest is readable");
    let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
        .expect("manifest digest is canonical");
    ReleaseComponents::load(&release, trusted)
        .unwrap_or_else(|error| panic!("the release components load: {error}"))
}

fn scxml(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0">{body}</scxml>"#
    )
}

/// The process's memory mappings and its address space and resident
/// memory in KiB (`VmSize`, `VmRSS`).
#[derive(Clone, Copy, Debug)]
struct Sample {
    mappings: usize,
    address_kib: usize,
    resident_kib: usize,
}

fn sample() -> Sample {
    let status =
        std::fs::read_to_string("/proc/self/status").expect("/proc/self/status is readable");
    let kib = |field: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or_else(|| panic!("{field} is in /proc/self/status"))
    };
    Sample {
        mappings: std::fs::read_to_string("/proc/self/maps")
            .expect("/proc/self/maps is readable")
            .lines()
            .count(),
        address_kib: kib("VmSize:"),
        resident_kib: kib("VmRSS:"),
    }
}

/// Level `n` of a chain of `DEPTH` waiting sessions, each invoking the
/// next; the process's [`Sample`] when levels `FIRST_COUNTED` and `DEPTH`
/// load.
struct Chain {
    counted: RefCell<BTreeMap<usize, Sample>>,
}

impl SessionEmbedder for Chain {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        let level: usize = uri
            .strip_prefix("level")?
            .strip_suffix(".scxml")?
            .parse()
            .ok()?;
        if level == FIRST_COUNTED || level == DEPTH {
            self.counted.borrow_mut().insert(level, sample());
        }
        let body = if level == DEPTH {
            r#"<state id="c0"/>"#.to_owned()
        } else {
            format!(
                r#"<state id="c0"><invoke type="scxml" src="level{}.scxml"/></state>"#,
                level + 1
            )
        };
        Some(scxml(&body).into())
    }

    fn scripts(&self, _base_uri: Option<&str>) -> MachineScripts {
        MachineScripts::none()
    }
}

#[test]
fn a_live_waiting_session_adds_few_memory_mappings() {
    let components = components();
    let host = SessionHost::new(
        &components.session_components(),
        &WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel: CHAIN_FUEL,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        },
        &TelemetryProviders::new(),
    )
    .expect("the verified components link");
    let root = scxml(r#"<state id="c0"><invoke type="scxml" src="level1.scxml"/></state>"#);
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "root.scxml",
        root.as_str(),
    ))
    .expect("the root parses");
    let ir = scxml_compiler_core::lower(&document).expect("the root lowers");
    let chain = Rc::new(Chain {
        counted: RefCell::new(BTreeMap::new()),
    });
    let events: [ScriptedEvent; 0] = [];
    let outcome = host
        .run(&SessionRun {
            machine_ir: &ir.to_json().expect("the root IR encodes"),
            data_model: ir.data_model.as_deref(),
            session_id: "root",
            events: &events,
            external_client: None,
            embedder: Rc::clone(&chain) as Rc<dyn SessionEmbedder>,
            max_commands: u64::MAX,
            snapshots: SnapshotRequest::none(),
        })
        .expect("the chain runs");
    assert_eq!(outcome.terminal, Terminal::Running);
    let counted = chain.counted.borrow();
    let (Some(first), Some(last)) = (counted.get(&FIRST_COUNTED), counted.get(&DEPTH)) else {
        panic!("both levels loaded: {counted:?}");
    };
    let sessions = DEPTH - FIRST_COUNTED;
    let added = last.mappings.saturating_sub(first.mappings);
    eprintln!(
        "{sessions} live sessions: {added} mappings ({} -> {}), {} KiB address space and {} KiB resident a session",
        first.mappings,
        last.mappings,
        last.address_kib.saturating_sub(first.address_kib) / sessions,
        last.resident_kib.saturating_sub(first.resident_kib) / sessions,
    );
    assert!(
        added < sessions / SESSIONS_PER_MAPPING,
        "{sessions} live sessions added {added} mappings ({first:?} -> {last:?})"
    );
}
