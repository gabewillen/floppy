//! The run's compiler store takes its memory growth copies from the host
//! call's copy allowance, like every other store.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

use super::{ChildCompile, CompilerRuntime};
use crate::{
    ReleaseComponents, SessionHost, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig,
};

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

/// States in the compiled child document: enough that compiling it grows
/// the compiler's memory.
const CHILD_STATES: usize = 2_000;

fn components() -> &'static ReleaseComponents {
    static COMPONENTS_CELL: OnceLock<ReleaseComponents> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let target = workspace.join("target/session-host-proof");
        let release = target.join("wasm-artifacts/compile-growth");
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

#[test]
fn compiling_a_child_document_takes_its_growth_copies_from_the_allowance() {
    let host = SessionHost::new(
        &components().session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the verified components link");
    let mut source = String::from(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0">"#,
    );
    for state in 0..CHILD_STATES {
        let next = (state + 1) % CHILD_STATES;
        write!(
            source,
            r#"<state id="c{state}"><transition event="e{state}" target="c{next}"/></state>"#
        )
        .expect("writing to a string succeeds");
    }
    source.push_str("</scxml>");
    let mut compiler = CompilerRuntime::instantiate(&host.core).expect("the compiler instantiates");
    let before = host.core.call_budget.spent();
    let outcome = compiler
        .compile(
            "child.scxml".to_owned(),
            source,
            std::iter::empty(),
            std::collections::BTreeMap::new(),
        )
        .expect("the compile call completes");
    assert!(
        matches!(outcome, ChildCompile::Lowered(_)),
        "the child lowers"
    );
    let spent = host.core.call_budget.spent().since(before);
    assert!(spent.wasm > 0, "the compile runs Wasm");
    assert!(
        spent.memory_growth_bytes > 0,
        "the compiler's growth copies come out of the copy allowance: {spent:?}"
    );
}
