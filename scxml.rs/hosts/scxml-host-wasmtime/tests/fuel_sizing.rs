//! The workloads that size `DEFAULT_FUEL`: the smallest per-call fuel each
//! runs under, found by bisection over the session host's fuel and printed.
//! Every workload is a document `compile` accepts, and each must run with at
//! least `REQUIRED_HEADROOM_PERCENT` of headroom on the default. A
//! measurement, so it is ignored by default. `default_fuel_runaway_call_time`
//! times the runaway documents the default stops, which trap out of fuel:
//!
//! ```console
//! cargo test --release -p scxml-host-wasmtime --features wasmtime \
//!     --test fuel_sizing -- --ignored --nocapture --test-threads 1
//! ```
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies the release
//! components into `target/session-host-proof` (this suite's own artifacts
//! directory inside it).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, DEFAULT_FUEL, DocumentKind, EmbeddedDocument, HostError,
    MachineScripts, RunFailure, ScriptedEvent, SessionEmbedder, SessionHost, SessionHostComponents,
    SessionRun, SnapshotRequest, TelemetryProviders, Terminal, TrustedManifestDigest,
    WasmtimeHostConfig, WasmtimeStoreConfig, authenticate_component,
};
use sha2::{Digest as _, Sha256};

mod valid_document;

const COMPONENTS: [(ArtifactKind, &str); 8] = [
    (ArtifactKind::Machine, "scxml_machine"),
    (ArtifactKind::Compiler, "scxml_compiler"),
    (
        ArtifactKind::DataModelEcmascript,
        "scxml_data_model_ecmascript",
    ),
    (ArtifactKind::DataModelXpath, "scxml_data_model_xpath"),
    (ArtifactKind::DataModelNull, "scxml_data_model_null"),
    (ArtifactKind::IoProcessorScxml, "scxml_ioprocessor_scxml"),
    (
        ArtifactKind::IoProcessorBasicHttp,
        "scxml_ioprocessor_basichttp",
    ),
    (ArtifactKind::Clock, "scxml_clock"),
];

fn components() -> &'static [AuthenticatedComponent] {
    static COMPONENTS_CELL: OnceLock<Vec<AuthenticatedComponent>> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let release = prebuilt::release_dir().unwrap_or_else(|| {
            let target = workspace.join("target/session-host-proof");
            // This suite's own artifact directory: suites sharing the Cargo
            // target never rewrite a manifest another suite is reading.
            let release = target.join("wasm-artifacts/fuel-sizing");
            let status = Command::new("sh")
                .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                .arg("--artifacts-dir")
                .arg(&release)
                .args(COMPONENTS.iter().map(|(_, name)| *name))
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
        COMPONENTS
            .iter()
            .map(|(kind, name)| {
                let path = format!("{name}.virtualized.wasm");
                let bytes = std::fs::read(release.join(&path)).expect("artifact is readable");
                authenticate_component(&manifest, trusted, *kind, &path, &bytes)
                    .expect("artifact authenticates")
            })
            .collect()
    })
}

/// A host whose every host call runs on `fuel`.
fn host_with_fuel(fuel: u64) -> SessionHost {
    let all = components();
    SessionHost::new(
        &SessionHostComponents {
            machine: &all[0],
            compiler: &all[1],
            data_model_ecmascript: &all[2],
            data_model_xpath: &all[3],
            data_model_null: &all[4],
            io_scxml: &all[5],
            io_basichttp: &all[6],
            clock: &all[7],
        },
        &WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        },
        &TelemetryProviders::new(),
    )
    .expect("the verified components link")
}

/// Documents keyed by URI; no scripts.
struct Documents(Vec<(&'static str, String)>);

impl SessionEmbedder for Documents {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        self.0
            .iter()
            .find(|(key, _)| *key == uri)
            .map(|(_, text)| text.clone().into())
    }

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

fn lower(source: &str) -> (Vec<u8>, Option<String>) {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "session-host.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

/// The source id of every measured document.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);
/// The command budget of a measured run.
const MAX_COMMANDS: u64 = 100_000;
/// The lower end of the bisection, which no workload runs under.
const FUEL_FLOOR: u64 = 100_000;
/// The upper end of the bisection, which every workload runs under.
const FUEL_CEILING: u64 = 2 * DEFAULT_FUEL;
/// The bisection stops when its interval is under 1 / `PRECISION` of the
/// fuel that runs (0.5 %).
const PRECISION: u64 = 200;
/// The headroom `DEFAULT_FUEL` keeps over every workload, in percent of the
/// workload's fuel (1.5 times).
const REQUIRED_HEADROOM_PERCENT: u64 = 150;

/// Whether the workload runs to a stable configuration on `fuel` per host call without
/// leaving its first state for `f` (where an invocation failure goes).
fn runs(fuel: u64, ir: &[u8], data_model: Option<&str>, events: &[ScriptedEvent]) -> bool {
    host_with_fuel(fuel)
        .run(&SessionRun {
            machine_ir: ir,
            data_model,
            session_id: "root",
            events,
            external_client: None,
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: MAX_COMMANDS,
            snapshots: SnapshotRequest::none(),
        })
        .is_ok_and(|outcome| {
            !matches!(outcome.terminal, Terminal::Rejected(_))
                && !outcome.configuration.iter().any(|state| state == "f")
        })
}

/// The smallest per-call fuel the workload runs under, within 0.5 %.
fn smallest_fuel(label: &str, source: &str, events: &[&str]) -> u64 {
    valid_document::assert_valid(label, source);
    let (ir, data_model) = lower(source);
    let events: Vec<ScriptedEvent> = events.iter().copied().map(ScriptedEvent::named).collect();
    assert!(
        runs(FUEL_CEILING, &ir, data_model.as_deref(), &events),
        "the workload runs on twice the default fuel"
    );
    let (mut fails, mut passes) = (FUEL_FLOOR, FUEL_CEILING);
    while passes - fails > passes / PRECISION {
        let middle = fails + (passes - fails) / 2;
        if runs(middle, &ir, data_model.as_deref(), &events) {
            passes = middle;
        } else {
            fails = middle;
        }
    }
    passes
}

/// An ECMAScript `<foreach>` with an `<assign>` over `items` items in the
/// start.
fn foreach(items: usize) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="arr" expr="(function(){{var r=[];for(var i=0;i&lt;{items};i++)r.push(i);return r}})()"/><data id="acc" expr="0"/></datamodel><state id="s0"><onentry><foreach array="arr" item="it"><assign location="acc" expr="acc + it"/></foreach><log label="sum" expr="acc"/></onentry></state></scxml>"#
    )
}

/// Eight non-assignment `<script>`s in one transition.
fn eight_scripts() -> String {
    let scripts = "<script>[1, 2, 3].map(function (v) { return v * 2; });</script>".repeat(8);
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><transition event="go" target="s1">{scripts}</transition></state><state id="s1"/></scxml>"#
    )
}

/// A `regions`-region parallel state whose regions all transition on `t`.
fn wide_parallel(regions: usize) -> String {
    let mut body = String::new();
    for region in 0..regions {
        write!(
            body,
            r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition event="t" target="r{region}b"/></state><state id="r{region}b"><transition event="t" target="r{region}a"/></state></state>"#
        )
        .expect("writing to a string succeeds");
    }
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="wide"><parallel id="wide">{body}</parallel></scxml>"#
    )
}

/// An ECMAScript root invoking an inline child of `states` states.
fn large_inline_child(states: usize) -> String {
    let mut body = String::new();
    for state in 0..states {
        let next = (state + 1) % states;
        write!(
            body,
            r#"<state id="c{state}"><transition event="e{state}" target="c{next}"/></state>"#
        )
        .expect("writing to a string succeeds");
    }
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c0">{body}</scxml></content></invoke><transition event="error.*" target="f"/></state><final id="f"/></scxml>"#
    )
}

/// Items of the `<foreach>` workloads.
const FOREACH_ITEMS: [usize; 3] = [60, 90, 1_000];
/// Regions of the wide parallel workload.
const WIDE_REGIONS: usize = 64;
/// States of the large inline child workload.
const CHILD_STATES: usize = 4_000;

#[test]
#[ignore = "a measurement that bisects each workload's fuel; run it with --ignored --nocapture"]
fn default_fuel_sizing_workloads() {
    let foreach_workloads = FOREACH_ITEMS.map(|items| {
        (
            format!("<foreach> over {items} items"),
            foreach(items),
            Vec::new(),
        )
    });
    let workloads = foreach_workloads.into_iter().chain([
        (
            "eight <script>s in one transition".to_owned(),
            eight_scripts(),
            vec!["go"],
        ),
        (
            "representative machine start".to_owned(),
            include_str!("../../../tests/fixtures/representative-machine.scx").to_owned(),
            Vec::new(),
        ),
        (
            format!("{WIDE_REGIONS}-region parallel start and event"),
            wide_parallel(WIDE_REGIONS),
            vec!["t"],
        ),
        (
            format!("{CHILD_STATES}-state inline child start"),
            large_inline_child(CHILD_STATES),
            Vec::new(),
        ),
    ]);
    for (label, source, events) in workloads {
        let fuel = smallest_fuel(&label, &source, &events);
        let hundredths = DEFAULT_FUEL * 100 / fuel;
        eprintln!(
            "{label}: {fuel} fuel, {}.{:02} times headroom on DEFAULT_FUEL",
            hundredths / 100,
            hundredths % 100
        );
        assert!(
            fuel * REQUIRED_HEADROOM_PERCENT <= DEFAULT_FUEL * 100,
            "{label} runs with the required headroom on the default fuel"
        );
    }
}

/// Ten `<foreach>` loops over 200 items, each running a heavy `<script>`, in
/// one `<onentry>`: the start never ends within any practical fuel.
fn heavy_nested_start() -> String {
    let loops = r#"<foreach array="arr" item="it"><script>for(var j=0;j&lt;5;j++){acc+=burn()}</script></foreach>"#
        .repeat(10);
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="arr" expr="(function(){{var r=[];for(var i=0;i&lt;200;i++)r.push(i);return r}})()"/><data id="acc" expr="0"/></datamodel><script>function burn(){{var x=0;for(var i=0;i&lt;99000;i++){{x+=i}}return x}}</script><state id="s0"><onentry>{loops}<log label="done"/></onentry></state></scxml>"#
    )
}

/// A `regions`-region parallel state whose regions all toggle on every
/// eventless microstep, so it never reaches a stable configuration.
fn wide_eventless(regions: usize) -> String {
    let mut body = String::new();
    for region in 0..regions {
        write!(
            body,
            r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition target="r{region}b"/></state><state id="r{region}b"><transition target="r{region}a"/></state></state>"#
        )
        .expect("writing to a string succeeds");
    }
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="wide"><parallel id="wide">{body}</parallel></scxml>"#
    )
}

/// Regions of the wide eventless runaway: wide enough that it traps out of
/// fuel well before the machine's per-call command bound.
const RUNAWAY_REGIONS: usize = 128;
/// Timed runs of each runaway.
const RUNAWAY_RUNS: usize = 3;

#[test]
#[ignore = "a measurement that times each runaway's trap; run it with --ignored --nocapture"]
fn default_fuel_runaway_call_time() {
    let runaways = [
        (
            "heavy nested <foreach>/<script> start".to_owned(),
            heavy_nested_start(),
        ),
        (
            format!("{RUNAWAY_REGIONS}-region eventless parallel"),
            wide_eventless(RUNAWAY_REGIONS),
        ),
    ];
    let host = host_with_fuel(DEFAULT_FUEL);
    for (label, source) in runaways {
        valid_document::assert_valid(&label, &source);
        let (ir, data_model) = lower(&source);
        let run = || {
            host.run(&SessionRun {
                machine_ir: &ir,
                data_model: data_model.as_deref(),
                session_id: "root",
                events: &[],
                external_client: None,
                embedder: Rc::new(Documents(Vec::new())),
                max_commands: MAX_COMMANDS,
                snapshots: SnapshotRequest::none(),
            })
            .expect_err("the runaway never reaches a stable configuration")
        };
        // The first run also compiles what the host caches; it is not timed.
        run();
        let mut times = Vec::new();
        let mut commands = 0;
        for _ in 0..RUNAWAY_RUNS {
            let started = Instant::now();
            let error = run();
            times.push(started.elapsed());
            assert!(
                matches!(
                    &error.failure,
                    RunFailure::Host(HostError::Wasmtime(trap))
                        if trap.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
                ),
                "{label} traps out of fuel: {:?}",
                error.failure
            );
            commands = error.commands;
        }
        eprintln!("{label}: trapped on DEFAULT_FUEL after {commands} commands, in {times:?}");
    }
}
