//! Exact virtualized-component composition and capability lifecycle proof.

#![cfg(feature = "wasmtime")]

mod common;
mod prebuilt;

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use opentelemetry::Context;
use opentelemetry::trace::{TraceContextExt as _, TraceId, Tracer as _, TracerProvider as _};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, CapabilityGraphComponents, ComponentStatus,
    MachineIrSource, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig,
    authenticate_component, execute_capability_graph,
};
use sha2::{Digest as _, Sha256};

struct Artifacts {
    clock: AuthenticatedComponent,
    io_processor: AuthenticatedComponent,
    machine: AuthenticatedComponent,
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(build_virtualized_artifacts)
}

fn build_virtualized_artifacts() -> Artifacts {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent");
    let release = prebuilt::release_dir().unwrap_or_else(|| {
        let target = workspace.join("target/capability-graph-proof");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args(["scxml_clock", "scxml_ioprocessor_scxml", "scxml_machine"])
            .current_dir(workspace)
            // The Cargo target every host suite builds the release components
            // in, so they compile once; this suite's artifacts stay its own.
            .env(
                "CARGO_TARGET_DIR",
                workspace.join("target/session-host-proof"),
            )
            .env(
                "SCXML_WASM_ARTIFACTS_DIR",
                target.join("wasm-artifacts/release"),
            )
            .status()
            .expect("capability proof can invoke the artifact verification script");
        assert!(status.success(), "capability components must verify");
        target.join("wasm-artifacts/release")
    });

    let manifest = release.join("manifest.tsv");
    let manifest_bytes = std::fs::read(&manifest).expect("artifact manifest is readable");
    let trusted_digest =
        TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest_bytes)))
            .expect("manifest digest is canonical");
    Artifacts {
        clock: authenticate_test_component(
            &manifest_bytes,
            trusted_digest,
            ArtifactKind::Clock,
            &manifest,
        ),
        io_processor: authenticate_test_component(
            &manifest_bytes,
            trusted_digest,
            ArtifactKind::IoProcessorScxml,
            &manifest,
        ),
        machine: authenticate_test_component(
            &manifest_bytes,
            trusted_digest,
            ArtifactKind::Machine,
            &manifest,
        ),
    }
}

fn authenticate_test_component(
    manifest_bytes: &[u8],
    trusted_digest: TrustedManifestDigest,
    kind: ArtifactKind,
    manifest: &Path,
) -> AuthenticatedComponent {
    let name = match kind {
        ArtifactKind::Clock => "scxml_clock",
        ArtifactKind::IoProcessorScxml => "scxml_ioprocessor_scxml",
        ArtifactKind::Machine => "scxml_machine",
        _ => panic!("capability graph fixture requested an unrelated artifact kind"),
    };
    let path = format!("{name}.virtualized.wasm");
    let artifact = manifest
        .parent()
        .expect("manifest has a parent")
        .join(&path);
    let bytes = std::fs::read(artifact).expect("component artifact is readable");
    authenticate_component(manifest_bytes, trusted_digest, kind, &path, &bytes)
        .expect("component artifact authenticates")
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

#[test]
fn virtualized_capability_graph_executes_bounded_scaffold_lifecycle() {
    let artifacts = artifacts();
    let (providers, spans, metrics) = providers();
    let execution = execute_capability_graph(
        CapabilityGraphComponents {
            clock: &artifacts.clock,
            io_processor: &artifacts.io_processor,
            machine: &artifacts.machine,
            machine_ir: MachineIrSource::Runtime(&common::example_ir()),
        },
        &WasmtimeHostConfig::default(),
        &providers,
    )
    .expect("exact virtualized capability graph executes");

    assert_eq!(execution.delivered_send_id, "scheduled-0");
    assert_eq!(execution.machine_status, ComponentStatus::Running);
    assert!(execution.remaining_fuel < WasmtimeHostConfig::default().store.fuel * 3);

    providers
        .force_flush()
        .expect("capability telemetry flushes");
    let exported_spans = spans.get_finished_spans().expect("spans are readable");
    for name in [
        "capability-graph-execute",
        "dispatch",
        "clock-schedule",
        "clock-advance",
        "drain-due",
        "start",
        "component-execute",
        "machine-graph-link",
    ] {
        assert!(
            exported_spans.iter().any(|span| span.name == name),
            "missing {name}; exported spans: {exported_spans:#?}"
        );
    }
    let propagated_trace =
        TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("test trace id is valid");
    for name in ["dispatch", "clock-schedule"] {
        assert!(
            exported_spans
                .iter()
                .any(|span| span.name == name && span.span_context.trace_id() == propagated_trace),
            "{name} must retain the supplied W3C trace identity"
        );
    }
    assert!(
        exported_spans.iter().any(|span| {
            span.name == "clock-schedule"
                && span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "scxml.outcome" && attribute.value.to_string() == "ok"
                })
        }),
        "clock observe telemetry must preserve its bounded outcome"
    );
    assert!(
        exported_spans.iter().any(|span| {
            span.name == "clock-schedule"
                && span.events.iter().any(|event| event.name == "scheduled")
        }),
        "clock event telemetry must preserve its bounded event"
    );
    assert!(
        exported_spans.iter().all(|span| span.name != "end"),
        "guest end calls must close the owned operation span rather than create detached end spans"
    );

    let exported_metrics = metrics
        .get_finished_metrics()
        .expect("metrics are readable");
    for name in [
        "scxml.component.effects",
        "scxml.component.queue.depth",
        "scxml.component.fuel",
    ] {
        assert!(
            exported_metrics.iter().any(|resource| resource
                .scope_metrics()
                .any(|scope| scope.metrics().any(|metric| metric.name() == name))),
            "missing numeric metric {name}"
        );
    }

    providers
        .shutdown()
        .expect("all capability stores were dropped");
}

#[test]
fn capability_graph_preserves_ambient_parent_across_nested_execution() {
    let artifacts = artifacts();
    let spans = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(spans.clone())
        .build();
    let metrics = InMemoryMetricExporter::default();
    let meter = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(metrics).build())
        .build();
    let providers = TelemetryProviders::from_providers(tracer_provider.clone(), meter);
    let tracer = tracer_provider.tracer("capability-graph-parent-test");
    let caller_span = tracer.start_with_context("caller", &Context::new());
    let caller_context = Context::new().with_span(caller_span);
    let caller_span_id = caller_context.span().span_context().span_id();
    let caller_trace_id = caller_context.span().span_context().trace_id();
    let guard = caller_context.clone().attach();

    execute_capability_graph(
        CapabilityGraphComponents {
            clock: &artifacts.clock,
            io_processor: &artifacts.io_processor,
            machine: &artifacts.machine,
            machine_ir: MachineIrSource::Runtime(&common::example_ir()),
        },
        &WasmtimeHostConfig::default(),
        &providers,
    )
    .expect("capability graph executes beneath the ambient caller");
    drop(guard);
    caller_context.span().end();
    providers.force_flush().expect("capability spans flush");

    let exported = spans.get_finished_spans().expect("spans are readable");
    let graph = exported
        .iter()
        .find(|span| span.name == "capability-graph-execute")
        .expect("graph execution span is exported");
    assert_eq!(graph.span_context.trace_id(), caller_trace_id);
    assert_eq!(graph.parent_span_id, caller_span_id);
    let machine_link = exported
        .iter()
        .find(|span| span.name == "machine-graph-link")
        .expect("nested machine link span is exported");
    assert_eq!(machine_link.span_context.trace_id(), caller_trace_id);
    assert_eq!(machine_link.parent_span_id, graph.span_context.span_id());

    providers.shutdown().expect("providers shut down cleanly");
}
