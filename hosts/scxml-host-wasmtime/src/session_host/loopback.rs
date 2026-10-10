//! The in-process `BasicHTTP` loopback transport and the run-wide
//! `BasicHTTP` receiver and compiler instances.
//!
//! Every session's `BasicHTTP` location is a URI under the run's base: the
//! reserved `.invalid` authority [`LOOPBACK_ORIGIN`] (RFC 6761: never
//! resolvable) unless a live session configured a public URL
//! ([`super::LiveHttp`]). A POST inside that address space is delivered
//! synchronously by location in its canonical form to the sending session,
//! its parent, or one of its live children; the `BasicHTTP` component
//! decodes and answers every message. The loopback opens no socket, starts
//! no thread, and reads no clock. A POST outside the address space goes to the live session's
//! [`super::OutboundHttp`] transport, and is undeliverable without one (a
//! batch run never has one).

use std::rc::Rc;
use std::sync::Arc;

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::component::{HasSelf, Linker};

use super::network::OutboundDecision;
use super::run::RunShared;
use super::{HostCore, RunFailure};
use crate::basichttp_types::{
    DataValue as HttpValue, Delivery, HttpHeader, HttpRequest, HttpResponse, Rejection,
};
use crate::bindings::stateforward::scxml::types::{
    DataValue as WitDataValue, Event as WitEvent, EventType, TraceContext as WitTraceContext,
};
use crate::compiler_bindings::stateforward::scxml_compiler::telemetry as compiler_telemetry;
use crate::compiler_bindings::stateforward::scxml_compiler::types::{
    ArtifactKind, CompileOptions, CompileRequest, ComponentError as CompilerError,
    ErrorCode as CompilerErrorCode, EventSchemaResource, ExtensionPackage, SchemaPolicy,
    SchemaResource, ScriptResolution,
};
use crate::guest_spans::GuestSpanTable;
use crate::memory_growth::MeteredLimits;
use crate::{
    BasicHttpProcessor, BasicHttpTransport, HostError, LifecycleLease, TelemetryProviders,
    TransportUnavailable, record_store_limits,
};

/// Scheme and authority of every in-process `BasicHTTP` location.
pub const LOOPBACK_ORIGIN: &str = "http://basichttp.loopback.invalid";

/// The `BasicHTTP` location of session `session_id` (SCXML C.2).
#[must_use]
pub fn loopback_location(session_id: &str) -> String {
    format!("{LOOPBACK_ORIGIN}/sessions/{session_id}/basichttp")
}

/// Which session a loopback URI addresses, seen from one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Route {
    /// The sending session itself.
    Own,
    /// The sender's parent; carries the sender's own invoke id.
    Parent(String),
    /// One live child of the sender, by invoke id.
    Child(String),
}

/// The loopback addresses reachable from one session.
#[derive(Clone, Debug, Default)]
pub(crate) struct Routes {
    /// The base of the run's `BasicHTTP` address space (no trailing `/`).
    pub(crate) base: String,
    /// The session's own location.
    pub(crate) own: String,
    /// The parent's location and the session's own invoke id.
    pub(crate) parent: Option<(String, String)>,
    /// Each live, unfinished child's location and invoke id.
    pub(crate) children: Vec<(String, String)>,
}

impl Routes {
    /// Resolves `uri`: `Err` outside the loopback address space (no
    /// route), `Ok(None)` when no live session answers it. The target, the
    /// base, and every location compare in their WHATWG canonical form
    /// (the `url` crate's serialization, the browser host's
    /// `new URL(...).href`), so a default port, letter case in the scheme
    /// or host, and `.` or `..` segments address a session exactly as they
    /// do in the browser host and in an HTTP request.
    pub(crate) fn resolve(&self, uri: &str) -> Result<Option<Route>, ()> {
        let target = canonical_location(uri);
        let mut space = canonical_location(&self.base);
        if !space.ends_with('/') {
            space.push('/');
        }
        if !target.starts_with(&space) {
            return Err(());
        }
        let at = |location: &str| canonical_location(location) == target;
        if at(&self.own) {
            return Ok(Some(Route::Own));
        }
        if let Some((location, invoke_id)) = &self.parent
            && at(location)
        {
            return Ok(Some(Route::Parent(invoke_id.clone())));
        }
        Ok(self
            .children
            .iter()
            .find(|(location, _)| at(location))
            .map(|(_, invoke_id)| Route::Child(invoke_id.clone())))
    }
}

/// The WHATWG canonical form of `location`, or `location` itself when it
/// does not parse as an absolute URL.
fn canonical_location(location: &str) -> String {
    url::Url::parse(location).map_or_else(|_| location.to_owned(), String::from)
}

/// A transport that routes nothing: the receiver instance never sends.
pub(crate) struct NoTransport;

impl BasicHttpTransport for NoTransport {
    fn post(&mut self, _request: HttpRequest) -> Result<HttpResponse, TransportUnavailable> {
        Err(TransportUnavailable)
    }
}

/// The run's `BasicHTTP` receiver: decodes inbound messages and answers
/// delivery outcomes.
pub(crate) struct ReceiverRuntime {
    processor: BasicHttpProcessor<NoTransport>,
    type_uri: String,
    aliases: Vec<String>,
    fuel: u64,
}

impl ReceiverRuntime {
    pub(crate) fn instantiate(core: &HostCore) -> Result<Self, HostError> {
        let mut processor = BasicHttpProcessor::instantiate_compiled(
            &core.engine,
            core.basichttp()?,
            &core.config,
            &core.providers,
            NoTransport,
            Some(&core.call_budget),
        )?;
        let type_uri = processor.type_uri()?;
        let aliases = processor.aliases()?;
        Ok(Self {
            processor,
            type_uri,
            aliases,
            fuel: core.config.store.fuel,
        })
    }

    /// The `BasicHTTP` type URI.
    pub(crate) fn type_uri(&self) -> &str {
        &self.type_uri
    }

    /// The `BasicHTTP` type aliases.
    pub(crate) fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// Decodes one inbound message into the external event it carries,
    /// with `trace_context` as the event's carrier.
    pub(crate) fn receive(
        &mut self,
        request: &HttpRequest,
        trace_context: Option<WitTraceContext>,
    ) -> Result<Result<WitEvent, Rejection>, HostError> {
        self.processor.refuel(self.fuel)?;
        Ok(self.processor.receive(request)?.map(|received| WitEvent {
            name: received.name,
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: Some(self.type_uri.clone()),
            invoke_id: None,
            data: scxml_value(received.data),
            raw: Some(received.raw),
            trace_context,
        }))
    }

    /// The response the component chooses for `delivery`.
    pub(crate) fn respond(&mut self, delivery: Delivery) -> Result<HttpResponse, HostError> {
        self.processor.refuel(self.fuel)?;
        self.processor.respond(delivery)
    }
}

/// The loopback transport of one session's `BasicHTTP` sender. The session
/// sets [`Self::routes`] before each send and takes [`Self::own`] after it.
///
/// A POST outside the run's address space goes to the live session's
/// [`super::OutboundHttp`] transport: an accepted request answers the
/// component `202 Accepted` at once, and a later failure is owed to this
/// session as `error.communication` (see `network`). Without a transport
/// (every batch run) such a POST is unavailable (`error.communication` in
/// the sender at once).
pub(crate) struct LoopbackTransport {
    core: Rc<HostCore>,
    shared: Rc<RunShared>,
    /// The sending session's `_sessionid`.
    session_id: String,
    /// The sending session among the run's sessions.
    key: scxml_scheduler::SessionKey,
    /// Addresses reachable from the sending session for the current send.
    pub(crate) routes: Routes,
    /// An event the current send delivered to the sending session itself.
    pub(crate) own: Option<WitEvent>,
    /// The current send's carrier, delivered on the received event.
    pub(crate) trace_context: Option<WitTraceContext>,
    /// The current `<send>`'s id, owed back with its late failure.
    pub(crate) send_id: String,
}

/// The status the component is told for a request the outbound transport
/// accepted (its outcome arrives later).
pub(crate) const OUTBOUND_ACCEPTED_STATUS: u16 = 202;

impl LoopbackTransport {
    pub(crate) const fn new(
        core: Rc<HostCore>,
        shared: Rc<RunShared>,
        session_id: String,
        key: scxml_scheduler::SessionKey,
    ) -> Self {
        Self {
            core,
            shared,
            session_id,
            key,
            routes: Routes {
                base: String::new(),
                own: String::new(),
                parent: None,
                children: Vec::new(),
            },
            own: None,
            trace_context: None,
            send_id: String::new(),
        }
    }

    fn deliver(&mut self, route: Route, event: WitEvent) -> Delivery {
        match route {
            Route::Own => self.own = Some(event),
            // The sender's invoke id stamps the event on its parent's queue.
            Route::Parent(_) => {
                self.shared
                    .invoked_sessions
                    .borrow_mut()
                    .push_to_parent(self.key, scxml_scheduler::Invoked::Event(event));
            }
            Route::Child(invoke_id) => {
                let mut invoked_sessions = self.shared.invoked_sessions.borrow_mut();
                if let Some(target) = invoked_sessions.child(self.key, &invoke_id) {
                    invoked_sessions.push_external(target, event);
                }
            }
        }
        Delivery::Enqueued
    }

    /// Hands one POST outside the address space to the outbound transport,
    /// or answers it from the recorded decisions of a replaying session
    /// (see `network`, "Record and replay"); a recording session journals
    /// the decision.
    fn outbound(&self, request: HttpRequest) -> Result<HttpResponse, TransportUnavailable> {
        let decision = match self.shared.http.replayed_decision() {
            Some(Ok(decision)) => decision,
            Some(Err(super::network::ReplayExhausted)) => {
                self.shared.fault(RunFailure::Harness {
                    code: "replay-diverged",
                    message: "the replay asked for an outbound BasicHTTP decision the recording does not have".into(),
                });
                OutboundDecision::Refused
            }
            None => {
                let result = self.begin_outbound(request);
                self.shared.http.record_decision(if result.is_ok() {
                    OutboundDecision::Accepted
                } else {
                    OutboundDecision::Refused
                });
                return result;
            }
        };
        if decision == OutboundDecision::Accepted {
            Ok(accepted_response())
        } else {
            Err(TransportUnavailable)
        }
    }

    /// Hands one POST to the live outbound transport.
    fn begin_outbound(&self, request: HttpRequest) -> Result<HttpResponse, TransportUnavailable> {
        let Some(outbound) = self.shared.http.outbound.as_ref() else {
            return Err(TransportUnavailable);
        };
        let outcome = self
            .shared
            .http
            .failures
            .ticket(super::network::OwedFailure {
                key: self.key,
                session_id: self.session_id.clone(),
                send_id: self.send_id.clone(),
            });
        outbound.begin(request, outcome)?;
        Ok(accepted_response())
    }
}

/// The answer the component gets for a request the outbound transport
/// accepted.
const fn accepted_response() -> HttpResponse {
    HttpResponse {
        status: OUTBOUND_ACCEPTED_STATUS,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

impl BasicHttpTransport for LoopbackTransport {
    fn post(&mut self, request: HttpRequest) -> Result<HttpResponse, TransportUnavailable> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeLoopbackPost);
        let trace_context = self.trace_context.take();
        let Ok(route) = self.routes.resolve(&request.uri) else {
            // Outside the address space: the outbound transport, if any.
            let result = self.outbound(request);
            if result.is_ok() {
                span.event(SignalEvent::TransformationCompleted);
            } else {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::InvalidInput);
            }
            return result;
        };
        let core = Rc::clone(&self.core);
        let shared = Rc::clone(&self.shared);
        let response = shared.with_receiver(&core, |receiver| {
            let delivery = match route {
                None => Delivery::UnknownSession,
                Some(route) => match receiver.receive(&request, trace_context)? {
                    Err(rejection) => Delivery::Rejected(rejection),
                    Ok(event) => self.deliver(route, event),
                },
            };
            receiver.respond(delivery)
        });
        match response {
            Ok(response) => {
                span.event(SignalEvent::TransformationCompleted);
                Ok(response)
            }
            Err(error) => {
                span.fail(ErrorCategory::Platform);
                self.shared.fault(RunFailure::Host(error));
                Err(TransportUnavailable)
            }
        }
    }
}

/// The W3C Trace Context an external HTTP client sent: its single
/// `traceparent` header and first `tracestate` header, validated by the
/// canonical carrier rules. A missing, repeated, or invalid `traceparent`
/// yields none and an invalid `tracestate` is dropped, as W3C Trace Context
/// requires of a receiver; inbound baggage never crosses (`drop`). Never
/// changes semantics.
pub(crate) fn inbound_http_trace_context(headers: &[HttpHeader]) -> Option<WitTraceContext> {
    let mut traceparents = header_values(headers, "traceparent");
    let traceparent = traceparents.next()?;
    if traceparents.next().is_some() {
        return None;
    }
    let tracestate = header_values(headers, "tracestate")
        .next()
        .map(str::to_owned);
    let context = scxml_types::TraceContext::new(
        traceparent,
        tracestate,
        None,
        scxml_types::BaggagePolicy::Drop,
    )
    .or_else(|_| {
        scxml_types::TraceContext::new(traceparent, None, None, scxml_types::BaggagePolicy::Drop)
    })
    .ok()?;
    Some(crate::session::wit_trace_context(&context))
}

/// HTTP optional white space (RFC 9110 §5.5 `OWS`: SP and HTAB), the only
/// padding a field value carries.
const HTTP_OPTIONAL_WHITE_SPACE: [char; 2] = [' ', '\t'];

/// The values of every header named `name` (ASCII case-insensitive), in
/// order, without leading and trailing HTTP optional white space.
fn header_values<'a>(
    headers: &'a [HttpHeader],
    name: &'a str,
) -> impl Iterator<Item = &'a str> + 'a {
    headers
        .iter()
        .filter(move |header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.trim_matches(HTTP_OPTIONAL_WHITE_SPACE))
}

/// Converts a `BasicHTTP` value to the SCXML package's identical variant.
pub(crate) fn scxml_value(value: HttpValue) -> WitDataValue {
    match value {
        HttpValue::Absent => WitDataValue::Absent,
        HttpValue::Undefined => WitDataValue::Undefined,
        HttpValue::Null => WitDataValue::Null,
        HttpValue::Boolean(value) => WitDataValue::Boolean(value),
        HttpValue::Number(value) => WitDataValue::Number(value),
        HttpValue::Text(value) => WitDataValue::Text(value),
        HttpValue::JsonArray(value) => WitDataValue::JsonArray(value),
        HttpValue::JsonObject(value) => WitDataValue::JsonObject(value),
        HttpValue::Xml(value) => WitDataValue::Xml(value),
    }
}

/// Converts an SCXML value to the `BasicHTTP` package's identical variant.
pub(crate) fn http_value(value: WitDataValue) -> HttpValue {
    match value {
        WitDataValue::Absent => HttpValue::Absent,
        WitDataValue::Undefined => HttpValue::Undefined,
        WitDataValue::Null => HttpValue::Null,
        WitDataValue::Boolean(value) => HttpValue::Boolean(value),
        WitDataValue::Number(value) => HttpValue::Number(value),
        WitDataValue::Text(value) => HttpValue::Text(value),
        WitDataValue::JsonArray(value) => HttpValue::JsonArray(value),
        WitDataValue::JsonObject(value) => HttpValue::JsonObject(value),
        WitDataValue::Xml(value) => HttpValue::Xml(value),
    }
}

/// Store state of the run's compiler instance.
pub(crate) struct CompilerStoreData {
    limits: MeteredLimits,
    providers: TelemetryProviders,
    execution_context: Context,
    guest_spans: GuestSpanTable,
    _store_lease: LifecycleLease,
}

/// The run's compiler component instance (`compile-invocation`).
pub(crate) struct CompilerRuntime {
    store: wasmtime::Store<CompilerStoreData>,
    bindings: crate::compiler_bindings::Compiler,
    fuel: u64,
}

/// Outcome of compiling one child document.
#[derive(Clone)]
pub(crate) enum ChildCompile {
    /// Lowered Machine IR bytes.
    Lowered(Vec<u8>),
    /// The compiler rejected the document (diagnostics, no artifact).
    Rejected,
    /// The compiler failed with a closed code other than a rejection.
    Failed,
}

impl CompilerRuntime {
    pub(crate) fn instantiate(core: &HostCore) -> Result<Self, HostError> {
        let mut linker = Linker::<CompilerStoreData>::new(&core.engine);
        crate::compiler_bindings::Compiler::add_to_linker::<_, HasSelf<CompilerStoreData>>(
            &mut linker,
            |state| state,
        )?;
        let config = &core.config;
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(config.store.memory_bytes)
            .table_elements(config.store.table_elements)
            .instances(config.store.instances)
            .tables(config.store.tables)
            .memories(config.store.memories)
            .trap_on_grow_failure(true)
            .build();
        let mut store = wasmtime::Store::new(
            &core.engine,
            CompilerStoreData {
                limits: MeteredLimits::new(limits, Arc::clone(&core.call_budget)),
                providers: core.providers.clone(),
                execution_context: Context::current(),
                guest_spans: GuestSpanTable::new(),
                _store_lease: core.providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        core.call_budget.share(&mut store);
        store.set_fuel(config.store.fuel)?;
        record_store_limits(&config.store);
        let bindings =
            crate::compiler_bindings::Compiler::instantiate(&mut store, core.compiler()?, &linker)?;
        Ok(Self {
            store,
            bindings,
            fuel: config.store.fuel,
        })
    }

    /// Compiles one child document to lowered Machine IR with schema
    /// validation off, runtime script resolution, every namespace package
    /// the host binds (so a child may use its parent's namespaces), and the
    /// schema files its `event:schema` attributes reference (an `<invoke
    /// src>` document's, [`crate::EmbeddedDocument::event_schemas`]).
    pub(crate) fn compile<'a>(
        &mut self,
        source_name: String,
        source: String,
        packages: impl Iterator<Item = &'a crate::ExtensionPackage>,
        event_schemas: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> Result<ChildCompile, HostError> {
        self.store.set_fuel(self.fuel)?;
        let (schemas, packages): (Vec<_>, Vec<_>) = packages
            .map(|package| {
                (
                    SchemaResource {
                        namespace_uri: package.namespace_uri.clone(),
                        system_id: package.schema_artifact.clone(),
                        bytes: package.schema.clone(),
                    },
                    ExtensionPackage {
                        namespace_uri: package.namespace_uri.clone(),
                        package_version: package.package_version.clone(),
                        extension_artifact: package.extension_artifact.clone(),
                        schema_artifact: package.schema_artifact.clone(),
                        schema_sha256: package.schema_sha256.clone(),
                        wit_world: package.wit_world.clone(),
                        wit_version: package.wit_version.clone(),
                    },
                )
            })
            .unzip();
        let request = CompileRequest {
            source_name,
            source,
            schemas,
            packages,
            scripts: Vec::new(),
            // An `<invoke src>` document's schema files, as its embedder
            // read them; an inline `<content>` document gets none, so a
            // schema reference there rejects the child, as any validation
            // error does.
            event_schemas: event_schemas
                .into_iter()
                .map(|(uri, bytes)| EventSchemaResource { uri, bytes })
                .collect(),
            options: CompileOptions {
                schema_policy: SchemaPolicy::Off,
                requested_artifact: ArtifactKind::LoweredIr,
                script_resolution: ScriptResolution::Runtime,
            },
            machine_template: None,
            trace_context: None,
        };
        let result = self
            .bindings
            .stateforward_scxml_compiler_compiler_api()
            .call_compile(&mut self.store, &request)?;
        Ok(match result {
            Ok(compiled) => compiled
                .artifact_bytes
                .map_or(ChildCompile::Rejected, ChildCompile::Lowered),
            Err(CompilerError {
                code: CompilerErrorCode::InvalidRequest,
                ..
            }) => ChildCompile::Rejected,
            Err(_) => ChildCompile::Failed,
        })
    }
}

impl crate::compiler_bindings::stateforward::scxml_compiler::types::Host for CompilerStoreData {}

impl compiler_telemetry::Host for CompilerStoreData {
    fn start(
        &mut self,
        _module: compiler_telemetry::Module,
        _operation: compiler_telemetry::Operation,
        parent: Option<compiler_telemetry::TraceContext>,
    ) -> (u64, Option<compiler_telemetry::TraceContext>) {
        let parent = if let Some(carrier) = parent {
            let extracted =
                scxml_telemetry::extract_trace_context(&scxml_telemetry::TraceContext {
                    traceparent: carrier.traceparent,
                    tracestate: carrier.tracestate,
                });
            if let Ok(parent) = extracted {
                parent
            } else {
                crate::component::guest_telemetry_refused(&self.execution_context);
                return (0, None);
            }
        } else {
            self.execution_context.clone()
        };
        let started = self.guest_spans.start(
            &self.providers,
            "scxml-component-guest",
            "compiler-component",
            "compile",
            &parent,
        );
        if let Ok((handle, carrier)) = started {
            (
                handle,
                Some(compiler_telemetry::TraceContext {
                    traceparent: carrier.traceparent,
                    tracestate: carrier.tracestate,
                }),
            )
        } else {
            crate::component::guest_telemetry_refused(&self.execution_context);
            (0, None)
        }
    }

    fn event(&mut self, span: u64, event: compiler_telemetry::SignalEvent) {
        let event = match event {
            compiler_telemetry::SignalEvent::InputRejected => "input-rejected",
            compiler_telemetry::SignalEvent::ArtifactProduced => "artifact-produced",
        };
        if self.guest_spans.event(span, event).is_err() {
            crate::component::guest_telemetry_refused(&self.execution_context);
        }
    }

    fn end(&mut self, span: u64, error: Option<compiler_telemetry::ErrorCategory>) {
        let error = error.map(|category| match category {
            compiler_telemetry::ErrorCategory::InvalidInput => "invalid-input",
            compiler_telemetry::ErrorCategory::Validation => "validation",
            compiler_telemetry::ErrorCategory::Lowering => "lowering",
            compiler_telemetry::ErrorCategory::Report => "report",
            compiler_telemetry::ErrorCategory::Packaging => "packaging",
        });
        if self.guest_spans.end_with_error(span, error).is_err() {
            crate::component::guest_telemetry_refused(&self.execution_context);
        }
    }
}

#[cfg(test)]
mod compile_growth_tests;

#[cfg(test)]
mod trace_context_tests {
    use super::inbound_http_trace_context;
    use crate::basichttp_types::HttpHeader;

    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    const TRACESTATE: &str = "vendor=value";

    fn headers(traceparent: &str, tracestate: &str) -> Vec<HttpHeader> {
        vec![
            HttpHeader {
                name: "traceparent".into(),
                value: traceparent.into(),
            },
            HttpHeader {
                name: "tracestate".into(),
                value: tracestate.into(),
            },
        ]
    }

    /// Only HTTP optional white space (SP and HTAB, RFC 9110 §5.5) pads a
    /// trace header, as the browser host reads it
    /// (`hosts/scxml-host-browser/test/basichttp.test.mjs`): U+FEFF,
    /// U+0085, and U+00A0 are part of the value, which is then invalid.
    #[test]
    fn a_trace_header_is_padded_by_http_white_space_only() {
        let read = |traceparent: &str, tracestate: &str| {
            inbound_http_trace_context(&headers(traceparent, tracestate))
                .map(|context| (context.traceparent, context.tracestate))
        };
        assert_eq!(
            read(&format!(" \t{TRACEPARENT}\t "), &format!("\t{TRACESTATE} ")),
            Some((TRACEPARENT.to_owned(), Some(TRACESTATE.to_owned())))
        );
        for padding in ['\u{FEFF}', '\u{85}', '\u{A0}'] {
            assert_eq!(
                read(&format!("{padding}{TRACEPARENT}"), TRACESTATE),
                None,
                "{padding:?}"
            );
            assert_eq!(
                read(&format!("{TRACEPARENT}{padding}"), TRACESTATE),
                None,
                "{padding:?}"
            );
            assert_eq!(
                read(TRACEPARENT, &format!("{padding}{TRACESTATE}")),
                Some((TRACEPARENT.to_owned(), None)),
                "{padding:?}"
            );
        }
    }
}
