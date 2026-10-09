//! `BasicHTTP` Event I/O Processor component linked to an embedder transport.
//!
//! Builds and authenticates the exact virtualized component (whose only
//! non-telemetry import is `http-transport`), drives it through the thin
//! Wasmtime linker with an in-memory transport, and compares every
//! observable result with the native reference processor.

#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use scxml_host_wasmtime::basichttp_types::{
    DataValue, Delivery, ErrorCode, HttpHeader, HttpRequest, HttpResponse, OutboundSend, Param,
    Rejection,
};
use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, BasicHttpProcessor, BasicHttpTransport,
    TelemetryProviders, TransportUnavailable, TrustedManifestDigest, WasmtimeHostConfig,
    authenticate_component,
};
use sha2::{Digest as _, Sha256};

const TARGET: &str = "http://peer.invalid/sessions/s1/basichttp";

fn component() -> &'static AuthenticatedComponent {
    static COMPONENT: OnceLock<AuthenticatedComponent> = OnceLock::new();
    COMPONENT.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let release = prebuilt::release_dir().unwrap_or_else(|| {
            let target = workspace.join("target/basichttp-proof");
            let status = Command::new("sh")
                .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                .arg("scxml_ioprocessor_basichttp")
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
                .expect("the artifact verification script runs");
            assert!(status.success(), "the BasicHTTP component must verify");
            target.join("wasm-artifacts/release")
        });
        let manifest = release.join("manifest.tsv");
        let manifest_bytes = std::fs::read(&manifest).expect("artifact manifest is readable");
        let trusted =
            TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest_bytes)))
                .expect("manifest digest is canonical");
        let path = "scxml_ioprocessor_basichttp.virtualized.wasm";
        let bytes = std::fs::read(manifest.parent().expect("manifest has a parent").join(path))
            .expect("component artifact is readable");
        authenticate_component(
            &manifest_bytes,
            trusted,
            ArtifactKind::IoProcessorBasicHttp,
            path,
            &bytes,
        )
        .expect("component artifact authenticates")
    })
}

fn providers() -> TelemetryProviders {
    let tracer = SdkTracerProvider::builder()
        .with_simple_exporter(InMemorySpanExporter::default())
        .build();
    let meter = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(InMemoryMetricExporter::default()).build())
        .build();
    TelemetryProviders::from_providers(tracer, meter)
}

/// Records every request and answers with a fixed result.
struct Recording {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    answer: Result<u16, TransportUnavailable>,
}

impl BasicHttpTransport for Recording {
    fn post(&mut self, request: HttpRequest) -> Result<HttpResponse, TransportUnavailable> {
        self.requests
            .lock()
            .expect("request log is not poisoned")
            .push(request);
        self.answer.map(|status| HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        })
    }
}

fn processor(
    answer: Result<u16, TransportUnavailable>,
) -> (
    BasicHttpProcessor<Recording>,
    Arc<Mutex<Vec<HttpRequest>>>,
    TelemetryProviders,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let providers = providers();
    let processor = BasicHttpProcessor::instantiate(
        component(),
        &WasmtimeHostConfig::default(),
        &providers,
        Recording {
            requests: Arc::clone(&requests),
            answer,
        },
    )
    .expect("the BasicHTTP component instantiates");
    (processor, requests, providers)
}

fn outbound(target: Option<&str>) -> OutboundSend {
    OutboundSend {
        event_name: Some("test".into()),
        target: target.map(str::to_owned),
        params: vec![Param {
            name: "Var1".into(),
            value: DataValue::Number(2.0),
        }],
        content: None,
        trace_context: None,
    }
}

#[test]
fn component_send_matches_the_native_reference_encoding() {
    let (mut processor, requests, _providers) = processor(Ok(200));
    assert_eq!(
        processor.type_uri().expect("type-uri runs"),
        scxml_ioprocessor_basichttp::BASICHTTP_EVENT_PROCESSOR_URI
    );
    processor
        .send(&outbound(Some(TARGET)))
        .expect("send runs")
        .expect("a 200 answer delivers the send");
    let sent = requests
        .lock()
        .expect("request log is not poisoned")
        .clone();
    let native = scxml_ioprocessor_basichttp::BasicHttpIoProcessor
        .encode(&scxml_ioprocessor_basichttp::OutboundSend {
            event_name: Some("test".into()),
            target: Some(TARGET.into()),
            params: vec![scxml_ioprocessor_basichttp::Param {
                name: "Var1".into(),
                value: scxml_types::DataValue::Number(
                    scxml_types::FiniteNumber::new(2.0).expect("finite"),
                ),
            }],
            content: None,
        })
        .expect("native reference encodes");
    assert_eq!(sent.len(), 1, "exactly one request must be sent");
    let request = sent.first().expect("one request was sent");
    assert_eq!(request.method, native.method);
    assert_eq!(request.uri, native.uri);
    assert_eq!(request.body, native.body);
    assert_eq!(request.body, b"_scxmleventname=test&Var1=2");
    assert_eq!(
        request
            .headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str()))
            .collect::<Vec<_>>(),
        native
            .headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str()))
            .collect::<Vec<_>>()
    );

    // The recipient side of the same component decodes the request.
    let received = processor
        .receive(request)
        .expect("receive runs")
        .expect("the encoded request decodes");
    assert_eq!(received.name, "test");
    assert!(
        matches!(&received.data, DataValue::JsonObject(json) if json == "{\"Var1\":\"2\"}"),
        "form parameters decode to a canonical JSON object"
    );
    assert!(
        received
            .raw
            .starts_with(&format!("POST {TARGET} HTTP/1.1\r\n"))
    );
    assert_eq!(
        processor
            .respond(Delivery::Enqueued)
            .expect("respond runs")
            .status,
        200
    );
    assert!(processor.remaining_fuel().expect("fuel is metered") > 0);
}

#[test]
fn undeliverable_sends_fail_with_unavailable_and_bad_messages_are_rejected() {
    let (mut refusing, requests, _providers) = processor(Ok(503));
    let missing = refusing
        .send(&outbound(None))
        .expect("send runs")
        .expect_err("a send without a target is undeliverable");
    assert_eq!(missing.code, ErrorCode::Unavailable);
    assert!(
        requests
            .lock()
            .expect("request log is not poisoned")
            .is_empty(),
        "a send without a target must not reach the transport"
    );
    let refused = refusing
        .send(&outbound(Some(TARGET)))
        .expect("send runs")
        .expect_err("a 503 answer is undeliverable");
    assert_eq!(refused.code, ErrorCode::Unavailable);
    // `check` validates without posting: a missing target is a delivery
    // failure left to the send; an invalid target is reported now.
    let posted = requests.lock().expect("request log is not poisoned").len();
    assert!(refusing.check(&outbound(None)).expect("check runs").is_ok());
    let invalid = refusing
        .check(&outbound(Some("ftp://example.test/")))
        .expect("check runs")
        .expect_err("an invalid target is reported by check");
    assert_eq!(invalid.code, ErrorCode::InvalidInput);
    assert_eq!(
        requests.lock().expect("request log is not poisoned").len(),
        posted,
        "check never reaches the transport"
    );

    let (mut unreachable, _, _unreachable_providers) = processor(Err(TransportUnavailable));
    assert_eq!(
        unreachable
            .send(&outbound(Some(TARGET)))
            .expect("send runs")
            .map_err(|error| error.code),
        Err(ErrorCode::Unavailable)
    );

    let get = HttpRequest {
        method: "GET".into(),
        uri: TARGET.into(),
        headers: vec![HttpHeader {
            name: "Content-Type".into(),
            value: "text/plain".into(),
        }],
        body: Vec::new(),
    };
    let rejection = refusing
        .receive(&get)
        .expect("receive runs")
        .expect_err("GET is not accepted");
    assert_eq!(rejection, Rejection::MethodNotAllowed);
    let response = refusing
        .respond(Delivery::Rejected(rejection))
        .expect("respond runs");
    assert_eq!(response.status, 405);
}
