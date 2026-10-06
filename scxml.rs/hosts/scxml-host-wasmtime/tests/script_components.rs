//! Wasm script components (`<script src>`, SCXML 1.0 section 5.8) under
//! Wasmtime, in both resolution modes.
//!
//! `scripts/verify-wasm-capabilities.sh scxml_machine scxml_script_example`
//! builds and virtualizes the generic machine and the example script
//! component, then uses the CLI to compile `tests/fixtures/script-machine.scxml`
//! three ways: with the script pinned from a script package (compile-time
//! resolution), unpinned (runtime resolution), and packaged as
//! `component-wasm` with the pinned script bundled. These tests run those
//! exact bytes:
//!
//! * compile-time, packaged: the packaged machine serves the pinned script
//!   from its own `script-bundle` (no loader linked);
//! * compile-time, runtime IR: the generic machine with the pinned IR, the
//!   script resolved by the embedder's loader and checked against the pin;
//! * runtime: the generic machine with unpinned IR, the script resolved and
//!   allowlisted by digest at document load;
//! * runtime, data-model text: the same URI resolved to ECMAScript text,
//!   dispatched by media type to the data model's own `execute`;
//!
//! and require the identical observed trace. Hostile loads (digest mismatch,
//! unknown media type, non-component bytes, world mismatch, oversized, no
//! loader, not allowlisted, `src` plus inline body) must reject the document
//! with typed codes.

#![cfg(feature = "wasmtime")]

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, ComponentSource, ComponentStatus, DataModelFailure,
    HostError, MachineDataModel, MachineIrSource, MachineScripts, MachineSessionTrace,
    ResolvedScript, ScriptPolicy, ScriptResolveError, ScriptResolver, TelemetryProviders,
    TrustedComponentDigest, TrustedManifestDigest, WasmtimeHostConfig, authenticate_component,
    authenticate_packaged_machine, execute_machine_session_with_scripts,
};
use scxml_reporting::{ArtifactPayload, decode_canonical};
use scxml_types::{AssignPlacement, AssignSource, DataValue, Event};
use sha2::{Digest as _, Sha256};

const EVENTS: [&str; 3] = ["bump", "bad", "finish"];
const SCRIPT_URI: &str = "example.wasm";

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Artifacts {
    template: AuthenticatedComponent,
    template_bytes: Vec<u8>,
    script: Vec<u8>,
    script_digest: TrustedComponentDigest,
    pinned_ir: Vec<u8>,
    runtime_ir: Vec<u8>,
    packaged: AuthenticatedComponent,
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let target = workspace.join("target/component-proof");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(["scxml_machine", "scxml_script_example"])
            .current_dir(workspace)
            // The Cargo target every host suite builds the release components
            // in, so they compile once; this suite's artifacts stay its own.
            .env(
                "CARGO_TARGET_DIR",
                workspace.join("target/session-host-proof"),
            )
            // This suite's own artifact directory: suites sharing the Cargo
            // target never rewrite a manifest another suite is reading.
            .env(
                "SCXML_WASM_ARTIFACTS_DIR",
                target.join("wasm-artifacts/script-components"),
            )
            .status()
            .expect("the capability script runs");
        assert!(
            status.success(),
            "release capability and script proof must pass"
        );
        let release = target.join("wasm-artifacts/script-components");
        let manifest = read(&release.join("manifest.tsv"));
        let manifest_digest =
            TrustedManifestDigest::from_hex(&hex(&manifest)).expect("manifest digest is canonical");
        let template_bytes = read(&release.join("scxml_machine.virtualized.wasm"));
        let template = authenticate_component(
            &manifest,
            manifest_digest,
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            &template_bytes,
        )
        .expect("the generic machine template authenticates");
        // The script digest is taken from the authenticated manifest line,
        // never from the bytes being loaded.
        let manifest_text = String::from_utf8(manifest.clone()).expect("manifest is UTF-8");
        let script_line = manifest_text
            .lines()
            .find(|line| line.starts_with("artifact\tscxml_script_example\t"))
            .expect("the manifest records the script component");
        let script_hex = script_line
            .split('\t')
            .nth(4)
            .expect("manifest artifact lines carry a digest");
        let script = read(&release.join("scxml_script_example.virtualized.wasm"));
        assert_eq!(hex(&script), script_hex, "script bytes match the manifest");
        let script_digest =
            TrustedComponentDigest::from_hex(script_hex).expect("script digest is canonical");

        let scripted = release.join("scripted");
        let pinned_ir = read(&scripted.join("script-machine.scxml-ir.json"));
        let runtime_ir = read(&scripted.join("script-machine.runtime.scxml-ir.json"));
        let packaged_bytes = read(&scripted.join("script-machine.component.wasm"));
        let report = read(&scripted.join("script-machine.component.report.json"));
        let envelope = decode_canonical(&report).expect("the compiler report is canonical");
        let ArtifactPayload::Compiler { compiler } = envelope.payload else {
            panic!("the packaging report is a compiler report");
        };
        let output = compiler.output.expect("an emitted report carries output");
        let recipe: serde_json::Value =
            serde_json::from_slice(&output.package_recipe).expect("recipe is JSON");
        assert_eq!(
            recipe["linked_extension_components"],
            serde_json::json!([format!("script-sha256:{script_hex}")]),
            "the package recipe pins the exact script component"
        );
        let packaged = authenticate_packaged_machine(
            &packaged_bytes,
            TrustedComponentDigest::from_hex(&output.sha256).expect("report digest is canonical"),
        )
        .expect("the packaged scripted machine matches its compiler report");
        Artifacts {
            template,
            template_bytes,
            script,
            script_digest,
            pinned_ir,
            runtime_ir,
            packaged,
        }
    })
}

// ---------------------------------------------------------------------------
// The active ECMAScript data model the script component is linked to.

struct InQuery<'a>(&'a [String]);

impl ExecutionHost for InQuery<'_> {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        Ok(self.0.iter().any(|state| state == state_id))
    }
}

struct Ecma {
    model: EcmaScriptDataModel,
    configuration: Vec<String>,
}

impl Ecma {
    fn boxed() -> Box<dyn MachineDataModel> {
        let mut model = EcmaScriptDataModel::new();
        model
            .initialize(
                "",
                SystemContext {
                    session_id: "machine-session".into(),
                    machine_name: Some("scripted".into()),
                    io_processors: DataValue::canonical_json_object("{}")
                        .expect("empty I/O processor object is canonical"),
                },
            )
            .expect("the ECMAScript data model initializes");
        Box::new(Self {
            model,
            configuration: Vec::new(),
        })
    }
}

const fn failure(error: DataModelError) -> DataModelFailure {
    match error {
        DataModelError::InvalidInput | DataModelError::UnsupportedValue => {
            DataModelFailure::InvalidInput
        }
        DataModelError::Execution => DataModelFailure::Execution,
        DataModelError::NotInitialized | DataModelError::Host => DataModelFailure::Platform,
    }
}

impl MachineDataModel for Ecma {
    fn name(&self) -> Option<&str> {
        Some("ecmascript")
    }

    fn update_active_configuration(
        &mut self,
        exited: &[String],
        entered: &[String],
    ) -> Result<(), DataModelFailure> {
        self.configuration.retain(|state| !exited.contains(state));
        self.configuration.extend_from_slice(entered);
        Ok(())
    }

    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, DataModelFailure> {
        let mut host = InQuery(&self.configuration);
        match event {
            Some(event) => self.model.evaluate_condition(expression, event, &mut host),
            None => self
                .model
                .evaluate_condition_without_event(expression, &mut host),
        }
        .map_err(failure)
    }

    fn evaluate_value(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<DataValue, DataModelFailure> {
        let mut host = InQuery(&self.configuration);
        match event {
            Some(event) => self.model.evaluate_value(expression, event, &mut host),
            None => self
                .model
                .evaluate_value_without_event(expression, &mut host),
        }
        .map_err(failure)
    }

    fn assign(
        &mut self,
        location: &str,
        source: &AssignSource,
        placement: &AssignPlacement,
        event: Option<&Event>,
    ) -> Result<(), DataModelFailure> {
        // The data model's own assignment semantics, as the component runs.
        let configuration = self.configuration.clone();
        self.model
            .apply_assign(
                location,
                source,
                placement,
                event,
                &mut InQuery(&configuration),
            )
            .map_err(failure)
    }

    fn execute(&mut self, script: &str, event: Option<&Event>) -> Result<(), DataModelFailure> {
        let configuration = self.configuration.clone();
        let mut host = InQuery(&configuration);
        match event {
            Some(event) => self.model.execute(script, event, &mut host),
            None => self.model.execute_without_event(script, &mut host),
        }
        .map_err(failure)
    }

    fn evaluate_log(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<String, DataModelFailure> {
        let mut host = InQuery(&self.configuration);
        match event {
            Some(event) => self.model.evaluate_log(expression, event, &mut host),
            None => self.model.evaluate_log_without_event(expression, &mut host),
        }
        .map_err(failure)
    }
}

// ---------------------------------------------------------------------------
// Embedder-supplied runtime script loader.

/// Serves exactly one URI with fixed bytes and media type.
struct FixedResolver {
    media_type: &'static str,
    bytes: Vec<u8>,
}

impl ScriptResolver for FixedResolver {
    fn resolve(&mut self, uri: &str) -> Result<ResolvedScript, ScriptResolveError> {
        if uri == SCRIPT_URI {
            Ok(ResolvedScript {
                media_type: self.media_type.to_owned(),
                bytes: self.bytes.clone(),
            })
        } else {
            Err(ScriptResolveError::NotFound)
        }
    }
}

fn loader(media_type: &'static str, bytes: Vec<u8>, policy: ScriptPolicy) -> MachineScripts {
    MachineScripts::runtime(Box::new(FixedResolver { media_type, bytes }), policy)
}

fn trusting(bytes: &[u8]) -> ScriptPolicy {
    ScriptPolicy::new([TrustedComponentDigest::from_hex(&hex(bytes)).expect("digest is canonical")])
}

fn run(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    scripts: MachineScripts,
) -> Result<MachineSessionTrace, HostError> {
    execute_machine_session_with_scripts(
        ComponentSource::Authenticated(component),
        ir,
        &EVENTS,
        Ecma::boxed(),
        scripts,
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
}

/// Status, configuration, and `label: value` log lines per command.
fn observed(trace: &MachineSessionTrace) -> Vec<(ComponentStatus, Vec<String>, Vec<String>)> {
    trace
        .results
        .iter()
        .map(|step| {
            (
                step.status,
                step.active_configuration.clone(),
                step.logs
                    .iter()
                    .map(|record| {
                        format!(
                            "{}: {}",
                            record.label.as_deref().unwrap_or_default(),
                            record.text
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn expected() -> Vec<(ComponentStatus, Vec<String>, Vec<String>)> {
    let run = || vec!["run".to_owned()];
    vec![
        // Entry: the component doubles `input` (21) into `doubled`.
        (ComponentStatus::Running, run(), vec!["doubled: 42".into()]),
        // The component reads the updated `input` from the same data model.
        (ComponentStatus::Running, run(), vec!["doubled: 84".into()]),
        // A non-numeric input fails the script: the rest of the block is
        // skipped and SCXML `error.execution` is delivered.
        (
            ComponentStatus::Running,
            run(),
            vec!["error: error.execution".into()],
        ),
        (
            ComponentStatus::Finished,
            vec!["done".into()],
            vec!["final: 84".into()],
        ),
    ]
}

#[test]
fn compile_time_and_runtime_script_resolution_produce_the_same_trace() {
    let _serial = serial();
    let artifacts = artifacts();
    let expected = expected();

    let packaged = run(
        &artifacts.packaged,
        MachineIrSource::Packaged,
        MachineScripts::none(),
    )
    .expect("the packaged machine serves its pinned script from its own bundle");
    assert_eq!(observed(&packaged), expected, "compile-time, packaged");

    let pinned = run(
        &artifacts.template,
        MachineIrSource::Runtime(&artifacts.pinned_ir),
        loader(
            scxml_script::WASM_MEDIA_TYPE,
            artifacts.script.clone(),
            ScriptPolicy::new([artifacts.script_digest]),
        ),
    )
    .expect("pinned IR resolves the script through the loader");
    assert_eq!(observed(&pinned), expected, "compile-time pin, runtime IR");

    let runtime = run(
        &artifacts.template,
        MachineIrSource::Runtime(&artifacts.runtime_ir),
        loader(
            scxml_script::WASM_MEDIA_TYPE,
            artifacts.script.clone(),
            ScriptPolicy::new([artifacts.script_digest]),
        ),
    )
    .expect("runtime resolution loads the allowlisted script");
    assert_eq!(observed(&runtime), expected, "runtime resolution");

    // The same `src` resolved to the data model's own language runs through
    // the data model's `execute` (dispatch by media type).
    let text =
        b"if (typeof input !== 'number') { throw new Error('not a number'); } doubled = input * 2;"
            .to_vec();
    let text_trace = run(
        &artifacts.template,
        MachineIrSource::Runtime(&artifacts.runtime_ir),
        loader("text/javascript", text.clone(), trusting(&text)),
    )
    .expect("data-model text resolves and runs");
    assert_eq!(observed(&text_trace), expected, "runtime ECMAScript text");
}

/// Script text has no size bound outside a wasm build (resource limits
/// apply to wasm builds only): a `<script src>` resolved to 65 KiB of
/// ECMAScript runs natively, as the same text does inline.
#[test]
fn a_script_src_over_64_kib_of_text_runs_natively() {
    let _serial = serial();
    let artifacts = artifacts();
    let mut text =
        b"if (typeof input !== 'number') { throw new Error('not a number'); } doubled = input * 2;"
            .to_vec();
    text.extend(std::iter::repeat_n(b' ', 65 * 1024));
    let trace = run(
        &artifacts.template,
        MachineIrSource::Runtime(&artifacts.runtime_ir),
        loader("text/javascript", text.clone(), trusting(&text)),
    )
    .expect("65 KiB of script text loads and runs");
    assert_eq!(observed(&trace), expected());
}

fn rejected(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    scripts: MachineScripts,
) -> String {
    match run(component, ir, scripts) {
        Err(HostError::Component(code)) => code,
        other => panic!("the document must be rejected at load, got {other:?}"),
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one table of hostile loads, each with its expected typed code"
)]
fn hostile_script_loads_reject_the_document_with_typed_codes() {
    let _serial = serial();
    let artifacts = artifacts();
    let template = &artifacts.template;
    let runtime_ir = MachineIrSource::Runtime(&artifacts.runtime_ir);
    let pinned_ir = MachineIrSource::Runtime(&artifacts.pinned_ir);
    let wasm = scxml_script::WASM_MEDIA_TYPE;

    // Digest mismatch: the loader serves bytes other than the pinned ones.
    let mut tampered = artifacts.script.clone();
    let last = tampered.last_mut().expect("script is non-empty");
    *last ^= 1;
    assert_eq!(
        rejected(
            template,
            pinned_ir,
            loader(wasm, tampered.clone(), trusting(&tampered))
        ),
        "invalid-input",
        "digest mismatch against the compile-time pin"
    );
    // Not allowlisted: a valid script component whose digest the embedder
    // did not trust.
    assert_eq!(
        rejected(
            template,
            runtime_ir,
            loader(wasm, artifacts.script.clone(), ScriptPolicy::new([]))
        ),
        "invalid-input",
        "runtime scripts must be allowlisted by digest"
    );
    // Unknown media type.
    assert_eq!(
        rejected(
            template,
            runtime_ir,
            loader(
                "application/octet-stream",
                artifacts.script.clone(),
                trusting(&artifacts.script)
            ),
        ),
        "invalid-input"
    );
    // Non-component bytes: a bare core module header.
    let core = b"\0asm\x01\0\0\0".to_vec();
    assert_eq!(
        rejected(
            template,
            runtime_ir,
            loader(wasm, core.clone(), trusting(&core))
        ),
        "invalid-input"
    );
    // World mismatch: a valid component that is not a script (the machine).
    assert_eq!(
        rejected(
            template,
            runtime_ir,
            loader(
                wasm,
                artifacts.template_bytes.clone(),
                trusting(&artifacts.template_bytes)
            ),
        ),
        "invalid-input"
    );
    // Oversized for the embedder's byte bound.
    let tight = trusting(&artifacts.script)
        .with_max_script_bytes(1_024)
        .expect("a positive bound is valid");
    assert_eq!(
        rejected(
            template,
            runtime_ir,
            loader(wasm, artifacts.script.clone(), tight)
        ),
        "payload-too-large"
    );
    // No loader linked: a runtime reference cannot be fetched.
    assert_eq!(
        rejected(template, runtime_ir, MachineScripts::none()),
        "unavailable"
    );
    // The loader does not know the URI.
    let unknown = MachineScripts::runtime(
        Box::new(FixedResolver {
            media_type: "application/wasm",
            bytes: Vec::new(),
        }) as Box<dyn ScriptResolver>,
        ScriptPolicy::new([]),
    );
    let mut renamed: serde_json::Value =
        serde_json::from_slice(&artifacts.runtime_ir).expect("IR is JSON");
    rename_script_sources(&mut renamed, "missing.wasm");
    let renamed = serde_json::to_vec(&renamed).expect("IR encodes");
    assert_eq!(
        rejected(template, MachineIrSource::Runtime(&renamed), unknown),
        "unavailable"
    );
    // `src` together with an inline body is invalid Machine IR (SCXML 1.0
    // section 5.8: exactly one of the two).
    let mut both: serde_json::Value =
        serde_json::from_slice(&artifacts.runtime_ir).expect("IR is JSON");
    set_script_bodies(&mut both, "doubled = 1;");
    let both = serde_json::to_vec(&both).expect("IR encodes");
    assert_eq!(
        rejected(
            template,
            MachineIrSource::Runtime(&both),
            loader(wasm, artifacts.script.clone(), trusting(&artifacts.script)),
        ),
        "invalid-input"
    );
    assert!(
        ScriptPolicy::new([]).with_max_script_bytes(0).is_err()
            && ScriptPolicy::new([])
                .with_max_script_bytes(scxml_script::MAX_SCRIPT_COMPONENT_BYTES + 1)
                .is_err()
            && ScriptPolicy::new([]).with_run_fuel(0).is_err(),
        "unbounded script policies are rejected"
    );
}

/// Visits every `{"kind": "script", "src": {...}}` action in IR JSON.
fn for_each_src_script(
    value: &mut serde_json::Value,
    visit: &mut impl FnMut(&mut serde_json::Value),
) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("kind").and_then(serde_json::Value::as_str) == Some("script")
                && map.get("src").is_some_and(|src| !src.is_null())
            {
                visit(value);
                return;
            }
            for child in map.values_mut() {
                for_each_src_script(child, visit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                for_each_src_script(item, visit);
            }
        }
        _ => {}
    }
}

fn rename_script_sources(ir: &mut serde_json::Value, uri: &str) {
    for_each_src_script(ir, &mut |script| {
        script["src"]["uri"] = serde_json::json!(uri);
    });
}

fn set_script_bodies(ir: &mut serde_json::Value, body: &str) {
    for_each_src_script(ir, &mut |script| script["body"] = serde_json::json!(body));
}

/// Serves the same bytes for every URI.
struct AnyUriResolver {
    bytes: Vec<u8>,
}

impl ScriptResolver for AnyUriResolver {
    fn resolve(&mut self, _uri: &str) -> Result<ResolvedScript, ScriptResolveError> {
        Ok(ResolvedScript {
            media_type: scxml_script::WASM_MEDIA_TYPE.to_owned(),
            bytes: self.bytes.clone(),
        })
    }
}

/// Replaces the first action list holding a `src` script with `copies`
/// copies of that script, each loading its own URI.
fn multiply_src_scripts(value: &mut serde_json::Value, copies: usize) -> bool {
    match value {
        serde_json::Value::Array(items) => {
            let found = items.iter().position(|item| {
                item.get("kind").and_then(serde_json::Value::as_str) == Some("script")
                    && item.get("src").is_some_and(|src| !src.is_null())
            });
            if let Some(index) = found {
                let script = items[index].clone();
                items.splice(
                    index..=index,
                    (0..copies).map(|copy| {
                        let mut script = script.clone();
                        script["src"]["uri"] =
                            serde_json::json!(format!("{SCRIPT_URI}?copy={copy}"));
                        script
                    }),
                );
                return true;
            }
            items
                .iter_mut()
                .any(|item| multiply_src_scripts(item, copies))
        }
        serde_json::Value::Object(map) => map
            .values_mut()
            .any(|child| multiply_src_scripts(child, copies)),
        _ => false,
    }
}

/// The native host sets no bound on the distinct scripts one session
/// loads (resource limits apply to wasm builds only): 65 distinct URIs
/// load and run.
#[test]
fn a_session_loads_any_number_of_distinct_scripts() {
    let _serial = serial();
    let artifacts = artifacts();
    let mut ir: serde_json::Value =
        serde_json::from_slice(&artifacts.runtime_ir).expect("runtime IR is JSON");
    assert!(
        multiply_src_scripts(&mut ir, 65),
        "the fixture has a src script"
    );
    let ir = serde_json::to_vec(&ir).expect("IR encodes");
    let scripts = MachineScripts::runtime(
        Box::new(AnyUriResolver {
            bytes: artifacts.script.clone(),
        }),
        trusting(&artifacts.script),
    );
    let trace = run(&artifacts.template, MachineIrSource::Runtime(&ir), scripts)
        .expect("65 distinct scripts load");
    assert!(!trace.results.is_empty());
}

#[test]
fn scripted_package_is_self_contained_and_ignores_the_runtime_loader() {
    let _serial = serial();
    let artifacts = artifacts();
    // The bundle wins for pinned references: a loader that would serve
    // hostile bytes is never consulted.
    let trace = run(
        &artifacts.packaged,
        MachineIrSource::Packaged,
        loader(
            scxml_script::WASM_MEDIA_TYPE,
            b"hostile".to_vec(),
            ScriptPolicy::new([]),
        ),
    )
    .expect("pinned scripts come from the authenticated package");
    assert_eq!(observed(&trace), expected());
}
