//! The deferred-request slot never outlives the call that filled it: a
//! child constructed for a start the host never carried out would hold the
//! run's shared state, which holds the slot.

use std::path::Path;
use std::process::Command;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;

use opentelemetry::Context;
use sha2::{Digest as _, Sha256};

use super::{BATCH_START_EPOCH_MS, Opened, RootSpec, RunEngine, RunMode, RunShared};
use crate::session_host::network::RunHttp;
use crate::{
    DocumentKind, EmbeddedDocument, HostError, MachineScripts, ReleaseComponents, RunFailure,
    SessionEmbedder, SessionHost, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig,
};
use scxml_scheduler::SessionKey;

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const COMPONENTS: &[&str] = &[
    "scxml_machine",
    "scxml_compiler",
    "scxml_data_model_ecmascript",
    "scxml_data_model_xpath",
    "scxml_data_model_null",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_clock",
];

fn components() -> &'static ReleaseComponents {
    static COMPONENTS_CELL: OnceLock<ReleaseComponents> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let target = workspace.join("target/session-host-proof");
        let release = target.join("wasm-artifacts/deferred-request");
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
        let manifest = std::fs::read(release.join("manifest.tsv")).expect("manifest is readable");
        let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical");
        ReleaseComponents::load(&release, trusted)
            .unwrap_or_else(|error| panic!("the release components load: {error}"))
    })
}

/// The child document every `<invoke>` here loads.
struct Child;

impl SessionEmbedder for Child {
    fn document(&self, _kind: DocumentKind, _uri: &str) -> Option<EmbeddedDocument> {
        Some(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"/></scxml>"#
                .to_owned()
                .into(),
        )
    }

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

/// An engine whose root's start invokes one child, with a weak reference
/// to its shared state.
fn engine() -> (Box<RunEngine>, Weak<RunShared>) {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="r0"><state id="r0"><invoke id="k" type="scxml" src="child.scxml"/></state></scxml>"#;
    let document =
        scxml_syntax::parse_xml(scxml_syntax::SourceText::new(SOURCE, "root.scxml", source))
            .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document)
        .expect("fixture lowers")
        .to_json()
        .expect("fixture IR encodes");
    let host = SessionHost::new(
        &components().session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the verified components link");
    let shared = Rc::new(RunShared::new(
        64,
        RunMode::Batch,
        RunHttp::batch(),
        BATCH_START_EPOCH_MS,
    ));
    let embedder: Rc<dyn SessionEmbedder> = Rc::new(Child);
    let root = RootSpec {
        machine_ir: &ir,
        data_model: Some("null"),
        session_id: "root",
        embedder: &embedder,
    };
    let Ok(Opened::Ready(engine)) = RunEngine::open(&host.core, &shared, root, &Context::new())
    else {
        panic!("the root opens");
    };
    (engine, Rc::downgrade(&shared))
}

/// A call that defers a child's start and then traps instead of
/// suspending: the request is dropped with the child and its
/// registration, and once the engine is gone nothing keeps the run's
/// shared state alive.
#[test]
fn a_start_deferred_by_a_call_that_then_traps_is_dropped() {
    let (mut engine, shared) = engine();
    let RunEngine {
        sessions,
        scheduler,
        ..
    } = &mut *engine;
    let failed = sessions.with_invoked_sessions(scheduler.invoked_sessions_mut(), |sessions| {
        sessions.command(SessionKey::ROOT, |handle| {
            let started = handle.start();
            assert!(
                matches!(&started, Ok(Ok(step)) if step.suspended),
                "the root's start defers its child's start"
            );
            Err(HostError::Wasmtime(wasmtime::Trap::OutOfFuel.into()))
        })
    });
    assert!(matches!(failed, Err(RunFailure::Host(_))));
    let upgraded = shared.upgrade().expect("the engine holds the shared state");
    assert!(
        upgraded.deferred.borrow().is_none(),
        "the request was dropped"
    );
    assert_eq!(
        scheduler.invoked_sessions().len(),
        1,
        "the child was deregistered"
    );
    drop(upgraded);
    drop(engine);
    assert!(
        shared.upgrade().is_none(),
        "the run's shared state was freed"
    );
}

/// An engine that ends while a request is still deferred (its call
/// suspended, the host never carried the request out) empties the slot,
/// so the child left there does not keep the run's shared state alive.
#[test]
fn an_engine_dropped_with_a_deferred_start_frees_the_run() {
    let (mut engine, shared) = engine();
    let started = engine
        .sessions
        .session(SessionKey::ROOT)
        .expect("the root is present")
        .start();
    assert!(matches!(&started, Ok(Ok(step)) if step.suspended));
    assert!(
        shared
            .upgrade()
            .is_some_and(|shared| shared.deferred.borrow().is_some()),
        "the child's start is deferred"
    );
    drop(engine);
    assert!(
        shared.upgrade().is_none(),
        "the run's shared state was freed"
    );
}
