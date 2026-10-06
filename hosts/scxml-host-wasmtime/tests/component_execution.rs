//! End-to-end Component Model execution, telemetry, and resource-bound proofs.

#![cfg(feature = "wasmtime")]

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use scxml_engine::{
    BatchEnd, Command as EngineCommand, EffectExecutor, EffectFailure, Engine, EngineError,
    EngineStatus, EvaluationError, GuardEvaluator, PlannedEffect, RunStage,
};
use scxml_host_wasmtime::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ComponentSource, ComponentStatus,
    HostError, MachineIrSource, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig,
    authenticate_component, execute_component_start,
};
use sha2::{Digest as _, Sha256};

fn telemetry_test_guard() -> MutexGuard<'static, ()> {
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn component_artifact() -> &'static ComponentArtifact {
    static ARTIFACT: OnceLock<ComponentArtifact> = OnceLock::new();
    ARTIFACT.get_or_init(build_component_artifact)
}

struct ComponentArtifact {
    component: AuthenticatedComponent,
    digest: String,
    artifact_path: std::path::PathBuf,
    manifest: std::path::PathBuf,
    manifest_digest: String,
}

fn hostile_fixture(name: &str) -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent")
        .join("target/host-artifact-hostile-tests")
        .join(format!(
            "{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            name
        ));
    std::fs::create_dir_all(&directory).expect("hostile fixture directory is creatable");
    directory
        .canonicalize()
        .expect("hostile fixture directory canonicalizes")
}

fn write_fixture_manifest(directory: &Path, size: usize, digest: &str) -> std::path::PathBuf {
    let manifest = directory.join("manifest.tsv");
    let text = format!(
        "schema\tstateforward.wasm-artifacts/v1\nprofile\trelease\ntarget\twasm32-wasip2\nartifact-count\t1\nwasm-tools\twasm-tools 1.244.0\nwasi-virt\twasi-virt 0.2.0\nartifact\tscxml_machine\tscxml_machine.virtualized.wasm\t{size}\t{digest}\n"
    );
    std::fs::write(&manifest, text).expect("fixture manifest is writable");
    manifest
        .canonicalize()
        .expect("fixture manifest canonicalizes")
}

fn digest_file(path: &Path) -> TrustedManifestDigest {
    let bytes = std::fs::read(path).expect("fixture is readable");
    TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(bytes)))
        .expect("fixture digest is canonical")
}

fn inspect_script_manifest(manifest: &Path) -> (std::path::PathBuf, String, String) {
    let manifest_size = std::fs::metadata(manifest)
        .unwrap_or_else(|error| {
            panic!(
                "artifact manifest {} must have readable metadata: {error}",
                manifest.display()
            )
        })
        .len();
    assert!(
        manifest_size <= 64 * 1024,
        "artifact manifest exceeds its 64 KiB test boundary"
    );
    let manifest_text = std::fs::read_to_string(manifest).unwrap_or_else(|error| {
        panic!(
            "artifact manifest {} must be readable: {error}",
            manifest.display()
        )
    });
    for header in [
        "schema\tstateforward.wasm-artifacts/v1",
        "profile\trelease",
        "target\twasm32-wasip2",
        "artifact-count\t1",
        "wasm-tools\twasm-tools 1.244.0",
        "wasi-virt\twasi-virt 0.2.0",
    ] {
        assert!(
            manifest_text.lines().any(|line| line == header),
            "artifact manifest must contain exact header {header}"
        );
    }
    let machine_entries = manifest_text
        .lines()
        .filter(|line| line.starts_with("artifact\tscxml_machine\t"))
        .collect::<Vec<_>>();
    assert_eq!(
        machine_entries.len(),
        1,
        "manifest contains one exact machine identity"
    );
    let fields = machine_entries[0].split('\t').collect::<Vec<_>>();
    assert_eq!(fields.len(), 5, "machine manifest entry has five fields");
    assert_eq!(
        fields[2], "scxml_machine.virtualized.wasm",
        "manifest artifact path is deterministic and relative"
    );
    let artifact = manifest
        .parent()
        .expect("manifest has a parent")
        .join(fields[2]);
    let bytes = std::fs::read(&artifact).expect("virtualized component artifact is readable");
    assert_eq!(
        bytes.len(),
        fields[3]
            .parse::<usize>()
            .expect("machine manifest size is a bounded integer"),
        "manifest size matches exact bytes"
    );
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(digest, fields[4], "manifest digest matches exact bytes");
    (artifact, digest, manifest_text)
}

fn build_component_artifact() -> ComponentArtifact {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent");
    let target = workspace.join("target/component-proof");
    let status = Command::new("sh")
        .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
        .arg("scxml_machine")
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
            target.join("wasm-artifacts/component-execution"),
        )
        .status()
        .expect("component proof can invoke the capability script");
    assert!(status.success(), "release capability proof must pass");
    let manifest = target.join("wasm-artifacts/component-execution/manifest.tsv");
    let (artifact, digest, manifest_text) = inspect_script_manifest(&manifest);
    eprintln!(
        "executing artifact={} sha256={} bytes={}",
        artifact.display(),
        digest,
        std::fs::metadata(&artifact)
            .expect("artifact metadata is readable")
            .len()
    );
    let manifest_digest = format!("{:x}", Sha256::digest(manifest_text.as_bytes()));
    let trusted_digest = TrustedManifestDigest::from_hex(&manifest_digest)
        .expect("script manifest digest is valid lowercase SHA-256");
    let artifact_bytes = std::fs::read(&artifact).expect("artifact bytes are readable");
    let component = authenticate_component(
        manifest_text.as_bytes(),
        trusted_digest,
        ArtifactKind::Machine,
        "scxml_machine.virtualized.wasm",
        &artifact_bytes,
    )
    .expect("exact script artifact authenticates before execution");
    ComponentArtifact {
        component,
        digest,
        artifact_path: artifact,
        manifest,
        manifest_digest,
    }
}

fn providers() -> (
    TelemetryProviders,
    InMemorySpanExporter,
    InMemoryMetricExporter,
) {
    let spans = InMemorySpanExporter::default();
    let tracer = SdkTracerProvider::builder()
        .with_simple_exporter(spans.clone())
        .build();
    let metrics = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(metrics.clone()).build();
    let meter = SdkMeterProvider::builder().with_reader(reader).build();
    (
        TelemetryProviders::from_providers(tracer, meter),
        spans,
        metrics,
    )
}

/// Native reference executor for the effect-free example machine: the
/// engine runs `start` to a stable configuration exactly as inside the component.
struct NativeReference;

impl GuardEvaluator for NativeReference {
    fn evaluate_condition(
        &mut self,
        _expression: &str,
        _event: Option<&scxml_types::Event>,
    ) -> Result<bool, EvaluationError> {
        Err(EvaluationError::Unavailable)
    }
}

impl EffectExecutor for NativeReference {
    type Error = EngineError;

    fn admit_command(&mut self) -> Result<(), EngineError> {
        Ok(())
    }

    fn command_completed(&mut self) {}

    fn engine_error(&mut self, error: EngineError, _stage: RunStage) -> EngineError {
        error
    }

    fn update_configuration(
        &mut self,
        _exited: &[scxml_types::StateId],
        _entered: &[scxml_types::StateId],
    ) -> Result<(), EngineError> {
        Ok(())
    }

    fn begin_batch(&mut self, _trigger: Option<&scxml_types::Event>) -> Result<(), EngineError> {
        Ok(())
    }

    fn apply_effect(
        &mut self,
        _engine: &mut Engine,
        _planned: &PlannedEffect,
        _trigger: Option<&scxml_types::Event>,
    ) -> Result<(), EffectFailure<EngineError>> {
        Ok(())
    }

    fn end_batch(&mut self, _end: BatchEnd) {}
}

fn native_start() -> (ComponentStatus, Vec<String>, u32) {
    let machine = common::example_machine();
    let mut engine = Engine::new(machine).expect("canonical native fixture is valid");
    let report = engine
        .run(EngineCommand::Start, &mut NativeReference)
        .expect("native fixture starts");
    assert!(
        report.stable,
        "native fixture start reaches a stable configuration"
    );
    let step = report.result;
    let status = match step.status {
        EngineStatus::Ready => ComponentStatus::Ready,
        EngineStatus::Running => ComponentStatus::Running,
        EngineStatus::Finished => ComponentStatus::Finished,
    };
    let configuration = step
        .leaves
        .iter()
        .map(|state| state.as_str().to_owned())
        .collect::<Vec<_>>();
    (status, configuration, report.committed_effects)
}

#[test]
fn component_links_instantiates_calls_export_and_matches_native() {
    let _test_guard = telemetry_test_guard();
    assert_eq!(
        component_artifact().digest.len(),
        64,
        "executed component has a complete SHA-256 identity"
    );
    let (providers, spans, metrics) = providers();
    let execution = execute_component_start(
        ComponentSource::Authenticated(&component_artifact().component),
        MachineIrSource::Runtime(&common::example_ir()),
        &WasmtimeHostConfig::default(),
        &providers,
    )
    .expect("canonical component executes through Wasmtime");
    let native = native_start();
    assert_eq!(execution.status, native.0);
    assert_eq!(execution.active_configuration, native.1);
    assert_eq!(execution.effect_count, native.2);
    assert!(execution.remaining_fuel < WasmtimeHostConfig::default().store.fuel);

    providers.force_flush().expect("exporters flush");
    let exported_spans = spans
        .get_finished_spans()
        .expect("in-memory spans are readable");
    assert!(
        exported_spans
            .iter()
            .any(|span| span.name == "component-execute"),
        "host execution span must reach the configured exporter"
    );
    assert!(exported_spans.iter().any(|span| span.name == "initialize"));
    assert!(exported_spans.iter().any(|span| span.name == "start"));
    assert!(exported_spans.iter().any(|span| span.name == "shutdown"));
    assert!(exported_spans.iter().any(|span| {
        span.events
            .iter()
            .any(|event| event.name == "transformation-completed")
    }));

    let execution_span = exported_spans
        .iter()
        .find(|span| span.name == "component-execute")
        .expect("host execution span is exported");
    for guest_name in ["initialize", "start", "shutdown"] {
        assert!(
            exported_spans.iter().any(|span| {
                span.name == guest_name
                    && span.span_context.trace_id() == execution_span.span_context.trace_id()
                    && span.parent_span_id == execution_span.span_context.span_id()
            }),
            "guest operation {guest_name} must export a direct child of component-execute with the same trace ID; exported spans: {exported_spans:#?}"
        );
    }

    let exported_metrics = metrics
        .get_finished_metrics()
        .expect("in-memory metrics are readable");
    let component_metrics = exported_metrics
        .iter()
        .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
        .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
        .filter(|metric| metric.name().starts_with("scxml.component."))
        .map(|metric| {
            let points = match metric.data() {
                opentelemetry_sdk::metrics::data::AggregatedMetrics::U64(
                    opentelemetry_sdk::metrics::data::MetricData::Histogram(histogram),
                ) => histogram
                    .data_points()
                    .map(|point| {
                        assert_eq!(
                            point.attributes().count(),
                            0,
                            "component measurements must not use dynamic labels"
                        );
                        point.count()
                    })
                    .sum(),
                _ => 0,
            };
            (metric.name().to_string(), metric.unit().to_string(), points)
        })
        .collect::<Vec<_>>();
    for (expected, unit) in [
        ("scxml.component.effects", "{effect}"),
        ("scxml.component.fuel", "{fuel}"),
    ] {
        assert!(
            component_metrics
                .iter()
                .any(|(name, actual_unit, points)| name == expected
                    && actual_unit == unit
                    && *points > 0),
            "missing {expected}"
        );
    }

    providers
        .shutdown()
        .expect("stores and guest spans were dropped");
    providers.shutdown().expect("shutdown is idempotent");
}

#[test]
fn finite_fuel_exhaustion_fails_closed_and_is_measured() {
    let _test_guard = telemetry_test_guard();
    let (providers, spans, metrics) = providers();
    let config = WasmtimeHostConfig {
        store: scxml_host_wasmtime::WasmtimeStoreConfig {
            fuel: 1,
            ..scxml_host_wasmtime::WasmtimeStoreConfig::default()
        },
        ..WasmtimeHostConfig::default()
    };
    assert!(matches!(
        execute_component_start(
            ComponentSource::Authenticated(&component_artifact().component),
            MachineIrSource::Runtime(&common::example_ir()),
            &config,
            &providers
        ),
        Err(HostError::Wasmtime(_))
    ));
    providers.force_flush().expect("fuel telemetry flushes");
    let exported_spans = spans
        .get_finished_spans()
        .expect("in-memory spans are readable");
    assert!(
        exported_spans.iter().any(|span| {
            span.name == "component-execute"
                && matches!(span.status, opentelemetry::trace::Status::Error { .. })
        }),
        "fuel exhaustion must export an error execution span; exported spans: {exported_spans:#?}"
    );
    let exported = metrics
        .get_finished_metrics()
        .expect("in-memory metrics are readable");
    let fuel_budget = exported
        .iter()
        .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
        .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
        .filter(|metric| metric.name() == "scxml.component.fuel")
        .find_map(|metric| match metric.data() {
            opentelemetry_sdk::metrics::data::AggregatedMetrics::U64(
                opentelemetry_sdk::metrics::data::MetricData::Histogram(histogram),
            ) => histogram.data_points().find_map(|point| {
                (point.attributes().count() == 0).then(|| (point.count(), point.sum()))
            }),
            _ => None,
        });
    assert_eq!(
        fuel_budget,
        Some((1, 1)),
        "the finite fuel budget must be one attribute-free fuel sample"
    );
    providers
        .shutdown()
        .expect("failed store was dropped before shutdown");
}

#[test]
fn zero_fuel_configuration_fails_before_component_compilation_and_emits_an_error() {
    let _test_guard = telemetry_test_guard();
    let (providers, spans, _metrics) = providers();
    let zero = WasmtimeHostConfig {
        store: scxml_host_wasmtime::WasmtimeStoreConfig {
            fuel: 0,
            ..scxml_host_wasmtime::WasmtimeStoreConfig::default()
        },
        ..WasmtimeHostConfig::default()
    };
    assert!(matches!(
        execute_component_start(
            ComponentSource::Authenticated(&component_artifact().component),
            MachineIrSource::Runtime(&common::example_ir()),
            &zero,
            &providers
        ),
        Err(HostError::InvalidLimit(
            scxml_host_wasmtime::InvalidLimit::Fuel
        ))
    ));

    providers
        .force_flush()
        .expect("configuration rejection spans flush");
    let rejected = spans
        .get_finished_spans()
        .expect("in-memory spans are readable")
        .into_iter()
        .filter(|span| {
            span.name == "component-execute"
                && matches!(span.status, opentelemetry::trace::Status::Error { .. })
        })
        .count();
    assert_eq!(rejected, 1, "the unbounded configuration is observable");
    providers
        .shutdown()
        .expect("configuration rejection retains no store or guest span");
}

#[test]
fn component_byte_bound_rejects_before_compilation() {
    let _test_guard = telemetry_test_guard();
    let (providers, _spans, _metrics) = providers();
    let config = WasmtimeHostConfig {
        component_bytes: 8,
        ..WasmtimeHostConfig::default()
    };
    assert!(matches!(
        execute_component_start(
            ComponentSource::Authenticated(&component_artifact().component),
            MachineIrSource::Runtime(&common::example_ir()),
            &config,
            &providers
        ),
        Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
    ));
    providers.shutdown().expect("no store was created");
}

#[test]
fn unauthenticated_raw_component_bytes_fail_before_compilation() {
    let _test_guard = telemetry_test_guard();
    let (providers, _spans, _metrics) = providers();
    assert!(matches!(
        execute_component_start(
            ComponentSource::Unauthenticated(b"not-a-component"),
            MachineIrSource::Runtime(&common::example_ir()),
            &WasmtimeHostConfig::default(),
            &providers,
        ),
        Err(HostError::Artifact(ArtifactError::Unauthenticated))
    ));
    providers
        .shutdown()
        .expect("no compiler or store was created");
}

#[test]
fn tampered_manifest_and_artifact_fail_closed_before_execution() {
    let _test_guard = telemetry_test_guard();
    let source = component_artifact();

    let manifest_bytes = std::fs::read(&source.manifest).expect("source manifest is readable");
    let manifest_directory = hostile_fixture("manifest-tamper");
    let manifest = manifest_directory.join("manifest.tsv");
    std::fs::write(&manifest, &manifest_bytes).expect("fixture manifest is writable");
    std::fs::write(
        manifest_directory.join("scxml_machine.virtualized.wasm"),
        b"bounded-tamper",
    )
    .expect("fixture artifact is writable");
    let trusted = TrustedManifestDigest::from_hex(&source.manifest_digest)
        .expect("trusted source manifest digest is valid");
    assert!(matches!(
        authenticate_component(
            &manifest_bytes,
            trusted,
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            b"bounded-tamper"
        ),
        Err(HostError::Artifact(ArtifactError::ArtifactSizeMismatch))
    ));

    let mut same_size_tamper =
        std::fs::read(&source.artifact_path).expect("source artifact is readable");
    let first = same_size_tamper
        .first_mut()
        .expect("source artifact is non-empty");
    *first ^= 1;
    assert!(matches!(
        authenticate_component(
            &manifest_bytes,
            trusted,
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            &same_size_tamper
        ),
        Err(HostError::Artifact(ArtifactError::ArtifactDigestMismatch))
    ));

    let mut tampered_manifest = manifest_bytes;
    tampered_manifest.push(b' ');
    std::fs::write(&manifest, &tampered_manifest).expect("tampered manifest is writable");
    assert!(matches!(
        authenticate_component(
            &tampered_manifest,
            trusted,
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            b"bounded-tamper"
        ),
        Err(HostError::Artifact(ArtifactError::ManifestDigestMismatch))
    ));
}

#[test]
fn authenticated_manifest_rejects_noncanonical_path_and_oversize_contracts() {
    let _test_guard = telemetry_test_guard();
    let source = component_artifact();
    let directory = hostile_fixture("path-and-size");
    let bytes = std::fs::read(&source.artifact_path).expect("source artifact is readable");
    let digest = format!("{:x}", Sha256::digest(&bytes));
    std::fs::write(directory.join("scxml_machine.virtualized.wasm"), &bytes)
        .expect("fixture artifact is writable");

    let manifest = write_fixture_manifest(&directory, bytes.len(), &digest);
    let canonical = std::fs::read_to_string(&manifest).expect("fixture manifest is readable");
    let escaped = canonical.replace(
        "scxml_machine.virtualized.wasm",
        "../scxml_machine.virtualized.wasm",
    );
    std::fs::write(&manifest, &escaped).expect("escaped manifest is writable");
    assert!(matches!(
        authenticate_component(
            escaped.as_bytes(),
            TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(escaped.as_bytes())))
                .expect("escaped manifest digest is canonical"),
            ArtifactKind::Machine,
            "../scxml_machine.virtualized.wasm",
            &bytes
        ),
        Err(HostError::Artifact(ArtifactError::NonCanonicalPath))
    ));

    let manifest = write_fixture_manifest(
        &directory,
        scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES + 1,
        &digest,
    );
    assert!(matches!(
        authenticate_component(
            &std::fs::read(&manifest).expect("oversize contract manifest is readable"),
            digest_file(&manifest),
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            &bytes
        ),
        Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
    ));

    let oversized_manifest = directory.join("oversized-manifest.tsv");
    std::fs::write(&oversized_manifest, vec![b'x'; 64 * 1024 + 1])
        .expect("oversized manifest is writable");
    let oversized_manifest = oversized_manifest
        .canonicalize()
        .expect("oversized manifest canonicalizes");
    assert!(matches!(
        authenticate_component(
            &std::fs::read(&oversized_manifest).expect("oversized manifest is readable"),
            digest_file(&oversized_manifest),
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            &bytes
        ),
        Err(HostError::Artifact(ArtifactError::ManifestTooLarge))
    ));

    // The artifact's own length passes the same size predicate as the
    // manifest field before any size or digest comparison. The ceiling is
    // GitHub's 2 GiB − 1 release-asset limit, so no test allocates a slice
    // past it: the empty slice proves this path applies the predicate, and
    // the crate's unit tests prove the predicate's exact upper boundary.
    let manifest = write_fixture_manifest(&directory, bytes.len(), &digest);
    assert!(matches!(
        authenticate_component(
            &std::fs::read(&manifest).expect("artifact-bound manifest is readable"),
            digest_file(&manifest),
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            &[]
        ),
        Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
    ));
}
