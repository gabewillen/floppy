//! The session host's `machine-capabilities` for one session: I/O
//! processors, clock, loopback, documents, invocation lifecycle, and the
//! run budget and trace. Every decision is a component's; this module
//! forwards calls and executes the decisions it gets back.

use std::collections::BTreeMap;
use std::rc::Rc;

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use super::loopback::{ChildCompile, LoopbackTransport, Routes, http_value};
use super::run::{Constructed, DeferredRequest, MachineHandle, RunShared, SessionSpec, fatal};
use super::{
    DocumentBindings, DocumentKind as EmbedderDocumentKind, HostCore, LogEntry, RunFailure,
    SessionEmbedder, SessionOutput, TraceObservation,
};
use crate::basichttp_types::{
    BaggagePolicy as HttpBaggagePolicy, ErrorCode as HttpErrorCode, OutboundSend, Param,
    TraceContext as HttpTraceContext,
};
use scxml_scheduler::{Invoked, SessionKey};

use crate::bindings::exports::stateforward::scxml::machine_api::InvocationContext;
use crate::bindings::stateforward::scxml::machine_capability_types::{
    DocumentKind, Invocation, InvocationProgress, InvokedSession, LoadedDocument, Observation,
    SendDisposition,
};
use crate::bindings::stateforward::scxml::types::InvokeRequest;
use crate::bindings::stateforward::scxml::types::{
    BaggagePolicy, BasichttpSend, ComponentError, DataValue as WitDataValue, ErrorCode,
    Event as WitEvent, InvocationAddress, SendRequest, SendTarget, Sender,
    TraceContext as WitTraceContext,
};
use crate::capability_graph::{ClockStoreData, IoRuntime, IoStoreData};
use crate::clock_bindings::exports::stateforward::scxml_clock::clock_api::ClockSnapshot;
use crate::component::component_error;
use crate::io_bindings::exports::stateforward::scxml::io_processor_api::{
    DelayedSend, DispatchDecision, DueRoute as IoDueRoute, Guest as IoGuest, PendingSend,
};
use crate::{BasicHttpProcessor, HostError};
use scxml_scheduler::DueRoute as SchedulerDueRoute;

/// Largest due batch one drain removes (the WIT `drain-due` maximum).
const MAX_DUE_BATCH_ITEMS: u32 = 1_024;

/// The invoking parent of a invoked session.
#[derive(Clone, Debug)]
pub(crate) struct ParentLink {
    /// The parent's `_sessionid`.
    pub(crate) session_id: String,
    /// The invoke id the parent assigned to this child.
    pub(crate) invoke_id: String,
}

/// Capability state of one session under the session host.
pub(crate) struct SessionLinks {
    core: Rc<HostCore>,
    shared: Rc<RunShared>,
    embedder: Rc<dyn SessionEmbedder>,
    session_id: String,
    /// This session among the run's sessions.
    key: SessionKey,
    parent: Option<ParentLink>,
    io: IoRuntime,
    basichttp: Option<BasicHttpProcessor<LoopbackTransport>>,
    execution_context: Context,
    /// The session clock's current time, shared with the data model.
    time: crate::data_model::SessionTime,
    /// The bindings of the `<invoke src>` document this session runs (or
    /// that an inline `<content>` ancestor's runs); `None` for the host's.
    bindings: Option<Rc<DocumentBindings>>,
    /// The bindings of the `<invoke src>` document this session loaded last
    /// (empty for one with none of its own), for the child it compiles
    /// next.
    loaded_bindings: Option<Rc<DocumentBindings>>,
    /// The schema files of the `<invoke src>` document this session loaded
    /// last ([`crate::EmbeddedDocument::event_schemas`]), for the child it
    /// compiles next.
    loaded_event_schemas: Option<std::collections::BTreeMap<String, Vec<u8>>>,
    /// The bindings of the child this session compiled last, for the
    /// session it starts next.
    compiled_bindings: Option<Rc<DocumentBindings>>,
}

pub(crate) fn invalid_input(detail: &'static str) -> ComponentError {
    component_error(ErrorCode::InvalidInput, detail)
}

/// Where one session sits among the root session and its invoked sessions.
pub(crate) struct SessionPlace {
    /// The session's `_sessionid`.
    pub(crate) session_id: String,
    /// The session among the run's sessions.
    pub(crate) key: SessionKey,
    /// The invoking session, for an invoked session.
    pub(crate) parent: Option<ParentLink>,
    /// The bindings of the document the session runs; `None` for the
    /// host's.
    pub(crate) bindings: Option<Rc<DocumentBindings>>,
}

impl SessionLinks {
    pub(crate) fn new(
        core: Rc<HostCore>,
        shared: Rc<RunShared>,
        embedder: Rc<dyn SessionEmbedder>,
        place: SessionPlace,
        io: IoRuntime,
        execution_context: Context,
        time: crate::data_model::SessionTime,
    ) -> Self {
        let SessionPlace {
            session_id,
            key,
            parent,
            bindings,
        } = place;
        Self {
            core,
            shared,
            embedder,
            session_id,
            key,
            parent,
            io,
            basichttp: None,
            execution_context,
            time,
            bindings,
            loaded_bindings: None,
            loaded_event_schemas: None,
            compiled_bindings: None,
        }
    }

    /// The session clock's current time in Unix epoch ms
    /// (`execution-host.now-ms`): the run's start epoch plus the logical
    /// time.
    pub(crate) fn now_ms(&self) -> u64 {
        self.shared
            .start_epoch_ms
            .saturating_add(self.time.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The session this one invoked as `invoke_id`, while it is live.
    fn child(&self, invoke_id: &str) -> Option<SessionKey> {
        self.shared
            .invoked_sessions
            .borrow()
            .child(self.key, &invoke_id.to_owned())
    }

    /// Every live, unfinished invocation of this session (an invoked
    /// session or service), by ascending invoke id: its invoke id and the
    /// invoked session's `_sessionid` (none for a service).
    fn running_children(&self) -> Vec<(String, Option<String>)> {
        let invoked_sessions = self.shared.invoked_sessions.borrow();
        let sessions = self.shared.sessions.borrow();
        invoked_sessions
            .children(self.key)
            .filter_map(|(invoke_id, child)| {
                sessions
                    .get(child)
                    .filter(|record| !record.finished)
                    .map(|record| (invoke_id.clone(), record.session_id.clone()))
            })
            .collect()
    }

    fn parent_session_id(&self) -> Option<&str> {
        self.parent
            .as_ref()
            .map(|parent| parent.session_id.as_str())
    }

    /// Admits one engine command against the shared budget.
    pub(crate) fn admit_command(&mut self) -> Result<(), ComponentError> {
        self.shared
            .require_budget()
            .map_err(|failure| fatal(&self.shared, failure))
    }

    /// Charges and records one observation.
    pub(crate) fn observe(&mut self, observation: Observation) {
        let observation = match observation {
            Observation::CommandCompleted => {
                self.shared.charge();
                TraceObservation::Command
            }
            Observation::Raised(event) => TraceObservation::Raised(event),
            Observation::Processed(event) => TraceObservation::Processed(event),
        };
        self.shared
            .record(&self.session_id, self.parent_session_id(), observation);
    }

    /// Records one `<log>`: root records feed the run's root logs; every
    /// record extends the trace and, in a live session, streams to the
    /// embedder.
    pub(crate) fn log(&mut self, label: Option<String>, text: String) {
        let entry = LogEntry { label, text };
        if self.shared.streaming() {
            self.shared.emit(SessionOutput::Log {
                session: self.session_id.clone(),
                parent: self.parent_session_id().map(str::to_owned),
                entry: entry.clone(),
            });
        }
        if self.parent.is_none() {
            self.shared.push_root_log(entry.clone());
        }
        self.shared.record(
            &self.session_id,
            self.parent_session_id(),
            TraceObservation::Log(entry),
        );
    }

    fn io_call<T>(
        &mut self,
        body: impl FnOnce(&IoGuest, &mut wasmtime::Store<IoStoreData>) -> wasmtime::Result<T>,
    ) -> Result<T, HostError> {
        let fuel = self.core.config.store.fuel;
        self.io.store.set_fuel(fuel)?;
        self.io.store.data_mut().clock.store.set_fuel(fuel)?;
        let result = body(
            self.io.bindings.stateforward_scxml_io_processor_api(),
            &mut self.io.store,
        )?;
        self.io.store.data_mut().take_failures()?;
        Ok(result)
    }

    fn clock_call<T>(
        &mut self,
        body: impl FnOnce(
            &crate::clock_bindings::exports::stateforward::scxml_clock::clock_api::Guest,
            &mut wasmtime::Store<ClockStoreData>,
        ) -> wasmtime::Result<T>,
    ) -> Result<T, HostError> {
        let fuel = self.core.config.store.fuel;
        let clock = &mut self.io.store.data_mut().clock;
        clock.store.set_fuel(fuel)?;
        let result = body(
            clock.bindings.stateforward_scxml_clock_clock_api(),
            &mut clock.store,
        )?;
        clock.store.data_mut().common.take_telemetry_failure()?;
        Ok(result)
    }

    /// This session's addressing facts for the SCXML processor: only the
    /// invoked sessions that still run are live invocations, so a send to
    /// one that finished is undeliverable (SCXML C.1: a session that "does
    /// not exist or is inaccessible"), as its loopback location is.
    fn sender(&self) -> Sender {
        Sender {
            session_id: self.session_id.clone(),
            parent_session_id: self.parent_session_id().map(str::to_owned),
            invocations: self
                .running_children()
                .into_iter()
                .map(|(invoke_id, session_id)| InvocationAddress {
                    invoke_id,
                    session_id,
                })
                .collect(),
        }
    }

    /// The loopback addresses reachable from this session.
    fn routes(&self) -> Routes {
        let http = &self.shared.http;
        Routes {
            base: http.base.clone(),
            own: http.location(&self.session_id),
            parent: self
                .parent
                .as_ref()
                .map(|parent| (http.location(&parent.session_id), parent.invoke_id.clone())),
            // An invoked service has no `BasicHTTP` location.
            children: self
                .running_children()
                .into_iter()
                .filter_map(|(invoke_id, session_id)| {
                    session_id.map(|session_id| (http.location(&session_id), invoke_id))
                })
                .collect(),
        }
    }

    /// `_ioprocessors`: every linked processor's type URI and aliases,
    /// each with this session's location (SCXML 5.10, C.1, C.2).
    pub(crate) fn io_processors(&mut self) -> Result<WitDataValue, ComponentError> {
        let entries = self.io_processor_locations()?;
        let mut json = String::from("{");
        for (index, (key, location)) in entries.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            push_json_string(&mut json, key);
            json.push_str(":{\"location\":");
            push_json_string(&mut json, location);
            json.push('}');
        }
        json.push('}');
        match scxml_types::DataValue::canonical_json_object(&json) {
            Ok(scxml_types::DataValue::JsonObject(canonical)) => {
                Ok(WitDataValue::JsonObject(canonical))
            }
            _ => Err(fatal(
                &self.shared,
                RunFailure::Harness {
                    code: "ioprocessors-invalid",
                    message: "the linked processors' identities are not a valid JSON object".into(),
                },
            )),
        }
    }

    /// Every linked processor's type URI and aliases, each with this
    /// session's location, sorted by key; the processors answer them.
    pub(crate) fn io_processor_locations(
        &mut self,
    ) -> Result<BTreeMap<String, String>, ComponentError> {
        let session_id = self.session_id.clone();
        let scxml = self.io_call(|api, store| {
            Ok((
                api.call_type_uri(&mut *store)?,
                api.call_aliases(&mut *store)?,
                api.call_session_location(&mut *store, &session_id)?,
            ))
        });
        let (scxml_uri, scxml_aliases, scxml_location) =
            scxml.map_err(|error| fatal(&self.shared, error.into()))?;
        let core = Rc::clone(&self.core);
        let basichttp = self.shared.with_receiver(&core, |receiver| {
            Ok((receiver.type_uri().to_owned(), receiver.aliases().to_vec()))
        });
        let (basichttp_uri, basichttp_aliases) =
            basichttp.map_err(|error| fatal(&self.shared, error.into()))?;
        let basichttp_location = self.shared.http.location(&self.session_id);
        let mut entries = BTreeMap::new();
        for key in std::iter::once(scxml_uri).chain(scxml_aliases) {
            entries.insert(key, scxml_location.clone());
        }
        for key in std::iter::once(basichttp_uri).chain(basichttp_aliases) {
            entries.insert(key, basichttp_location.clone());
        }
        Ok(entries)
    }

    /// Resolves one external document through the embedder: its text and
    /// the media type its source indicated, when that is visible ASCII. The
    /// host bounds neither; the machine applies the WIT `loaded-document`
    /// bounds itself.
    ///
    /// An `<invoke src>` document's own bindings are verified here and kept,
    /// with its schema files, for the child this session compiles and
    /// starts next; one that does not verify fails the run.
    pub(crate) fn load_document(
        &mut self,
        kind: DocumentKind,
        uri: &str,
    ) -> Result<LoadedDocument, ComponentError> {
        let kind = match kind {
            DocumentKind::Data => EmbedderDocumentKind::Data,
            DocumentKind::Invocation => EmbedderDocumentKind::Invocation,
        };
        match self.embedder.document(kind, uri) {
            None => Err(component_error(
                ErrorCode::Unavailable,
                "document is not resolvable",
            )),
            Some(document) => {
                if kind == EmbedderDocumentKind::Invocation {
                    let bindings = self
                        .core
                        .document_bindings(document.extensions, document.services)
                        .map_err(|error| fatal(&self.shared, error.into()))?;
                    self.loaded_bindings = Some(bindings);
                    self.loaded_event_schemas = Some(document.event_schemas);
                }
                Ok(LoadedDocument {
                    text: document.text,
                    media_type: document.media_type.filter(|media_type| {
                        media_type
                            .bytes()
                            .all(|byte| byte == b' ' || byte.is_ascii_graphic())
                    }),
                })
            }
        }
    }

    /// Forwards target classification to the SCXML processor.
    pub(crate) fn classify_target(&mut self, target: Option<&str>) -> SendTarget {
        let sender = self.sender();
        match self.io_call(|api, store| api.call_classify_target(store, target, &sender)) {
            Ok(target) => target,
            Err(error) => {
                // The pure function has no failure channel: the fault fails
                // the machine call when it returns.
                self.shared.fault(RunFailure::Host(error));
                SendTarget::Invalid
            }
        }
    }

    /// Dispatches one send through the SCXML processor and executes its
    /// decision.
    pub(crate) fn send(
        &mut self,
        request: &SendRequest,
    ) -> Result<SendDisposition, ComponentError> {
        let sender = self.sender();
        let action = self
            .io_call(|api, store| api.call_dispatch(store, request, &sender))
            .map_err(|error| fatal(&self.shared, error.into()))?;
        match action {
            Ok(DispatchDecision::Scheduled) => Ok(SendDisposition::Scheduled),
            Ok(DispatchDecision::Local(event)) => Ok(SendDisposition::EnqueueExternal(event)),
            Ok(DispatchDecision::Parent(event)) => {
                if self.parent.is_none() {
                    return Err(fatal(
                        &self.shared,
                        RunFailure::Harness {
                            code: "parent-route-invariant",
                            message: "the root session has no parent to route to".into(),
                        },
                    ));
                }
                // Onto the invoking session's external queue; ignored once
                // this session's cancellation began (official case 252).
                self.shared
                    .invoked_sessions
                    .borrow_mut()
                    .push_to_parent(self.key, Invoked::Event(event));
                Ok(SendDisposition::Routed)
            }
            Ok(DispatchDecision::Invocation(routed)) => {
                if let Some(target) = self.child(&routed.invoke_id) {
                    self.shared
                        .invoked_sessions
                        .borrow_mut()
                        .push_external(target, routed.event);
                }
                Ok(SendDisposition::Routed)
            }
            Err(error) if error.code == ErrorCode::Unavailable => {
                Ok(SendDisposition::Undeliverable)
            }
            Err(error) => Err(error),
        }
    }

    /// Cancels one delayed send through the SCXML processor.
    pub(crate) fn cancel(
        &mut self,
        send_id: &str,
        context: Option<&WitTraceContext>,
    ) -> Result<(), ComponentError> {
        self.io_call(|api, store| api.call_cancel(store, send_id, context))
            .map_err(|error| fatal(&self.shared, error.into()))?
    }

    fn basichttp_sender(
        &mut self,
    ) -> Result<&mut BasicHttpProcessor<LoopbackTransport>, HostError> {
        if self.basichttp.is_none() {
            self.basichttp = Some(BasicHttpProcessor::instantiate_compiled(
                &self.core.engine,
                self.core.basichttp()?,
                &self.core.config,
                &self.core.providers,
                LoopbackTransport::new(
                    Rc::clone(&self.core),
                    Rc::clone(&self.shared),
                    self.session_id.clone(),
                    self.key,
                ),
                Some(&self.core.call_budget),
            )?);
        }
        self.basichttp
            .as_mut()
            .ok_or_else(|| HostError::Component("basichttp-sender-unavailable".into()))
    }

    /// Sends one `<send>` through the `BasicHTTP` processor: at once when
    /// it has no delay ([`Self::post_basichttp`]), else checked by the
    /// processor and held on the session clock by the SCXML processor until
    /// it falls due (SCXML 6.2: "It MUST dispatch the message only when the
    /// delay interval elapses."), then posted by
    /// [`Self::dispatch_due_basichttp`].
    pub(crate) fn send_basichttp(
        &mut self,
        request: BasichttpSend,
    ) -> Result<SendDisposition, ComponentError> {
        if request.delay_ms == 0 {
            self.post_basichttp(request)
        } else {
            self.schedule_basichttp(&request)
        }
    }

    /// Checks one delayed `BasicHTTP` send with the processor (every
    /// failure it would report before posting is reported now, SCXML 6.2:
    /// "The Processor MUST evaluate all arguments to `<send>` when the `<send>`
    /// element is evaluated, and not when the message is actually
    /// dispatched.") and schedules it through the SCXML processor.
    fn schedule_basichttp(
        &mut self,
        request: &BasichttpSend,
    ) -> Result<SendDisposition, ComponentError> {
        let fuel = self.core.config.store.fuel;
        let outbound = outbound_send(request);
        let checked = (|| {
            let sender = self.basichttp_sender()?;
            sender.refuel(fuel)?;
            sender.check(&outbound)
        })()
        .map_err(|error| fatal(&self.shared, error.into()))?;
        if let Err(error) = checked {
            return Err(match error.code {
                HttpErrorCode::InvalidInput => invalid_input("basichttp send is invalid"),
                HttpErrorCode::PayloadTooLarge => component_error(
                    ErrorCode::PayloadTooLarge,
                    "basichttp send exceeds its byte bound",
                ),
                // `check` never reports a delivery failure.
                HttpErrorCode::Unavailable => fatal(
                    &self.shared,
                    RunFailure::Harness {
                        code: "basichttp-check-invariant",
                        message: "the BasicHTTP processor check reported a delivery failure".into(),
                    },
                ),
            });
        }
        self.io_call(|api, store| api.call_schedule_basichttp(store, request))
            .map_err(|error| fatal(&self.shared, error.into()))?
            .map(|()| SendDisposition::Scheduled)
    }

    /// Posts one due delayed `BasicHTTP` send (SCXML 6.2) and maps its
    /// outcome onto the scheduler's due route: nothing left to route when
    /// the recipient accepted it (a loopback POST already reached the
    /// session it addresses), the sender's own external queue for its own
    /// location, and the sender's `error.communication` with the send's id
    /// when it cannot be dispatched (SCXML 6.2.4: "If it is unable to
    /// dispatch the message, the Processor MUST place the error
    /// error.communication on the internal event queue"; C.2). A failure
    /// the live outbound transport reports later arrives through the
    /// session's late-failure path with the same id.
    pub(crate) fn dispatch_due_basichttp(
        &mut self,
        request: BasichttpSend,
    ) -> Result<SchedulerDueRoute<String, WitEvent, Option<String>>, RunFailure> {
        let send_id = request.send_id.clone();
        let posted = self.post_basichttp(request);
        if let Some(failure) = self.shared.take_fault() {
            return Err(failure);
        }
        match posted {
            Ok(SendDisposition::Routed) => Ok(SchedulerDueRoute::Dispatched),
            Ok(SendDisposition::EnqueueExternal(event)) => Ok(SchedulerDueRoute::Local(event)),
            Ok(SendDisposition::Undeliverable) => {
                Ok(SchedulerDueRoute::Undeliverable(Some(send_id)))
            }
            Ok(SendDisposition::Scheduled) | Err(_) => Err(RunFailure::Harness {
                code: "basichttp-due-invariant",
                message: "a checked delayed BasicHTTP send failed validation when due".into(),
            }),
        }
    }

    /// Posts one `<send>` through the `BasicHTTP` processor over the
    /// loopback (or, outside the run's address space, the live session's
    /// outbound transport) and maps its outcome onto a disposition.
    fn post_basichttp(
        &mut self,
        request: BasichttpSend,
    ) -> Result<SendDisposition, ComponentError> {
        let routes = self.routes();
        let fuel = self.core.config.store.fuel;
        let outbound = outbound_send(&request);
        let sent = (|| {
            let sender = self.basichttp_sender()?;
            sender.transport_mut().routes = routes;
            sender.transport_mut().own = None;
            // The loopback carries the sender's carrier beside the request
            // (the processor validated it before posting).
            sender.transport_mut().trace_context = request.trace_context;
            // A late failure names this `<send>` (SCXML 5.10.1).
            sender.transport_mut().send_id = request.send_id;
            sender.refuel(fuel)?;
            let result = sender.send(&outbound)?;
            Ok::<_, HostError>((result, sender.transport_mut().own.take()))
        })();
        let (result, own) = sent.map_err(|error| fatal(&self.shared, error.into()))?;
        if self.shared.has_fault() {
            return Err(component_error(ErrorCode::Platform, "session host failure"));
        }
        match result {
            Ok(()) => Ok(own.map_or(SendDisposition::Routed, SendDisposition::EnqueueExternal)),
            Err(error) => match error.code {
                HttpErrorCode::InvalidInput => Err(invalid_input("basichttp send is invalid")),
                HttpErrorCode::PayloadTooLarge => Err(component_error(
                    ErrorCode::PayloadTooLarge,
                    "basichttp send exceeds its byte bound",
                )),
                HttpErrorCode::Unavailable => Ok(SendDisposition::Undeliverable),
            },
        }
    }

    /// `machine-capabilities.load-extensions`: gives this session an instance
    /// of the extension component of every namespace its document uses (see
    /// `HostCore::load_extensions`). A namespace that cannot be loaded answers
    /// `unavailable`; a rejected binding or a failed instance is a host
    /// failure of the run.
    pub(crate) fn load_extensions(
        &self,
        namespaces: &mut Option<crate::extension::SessionNamespaces>,
        namespace_uris: &[String],
    ) -> Result<(), ComponentError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeExtensionLoad);
        match self.core.load_extensions(
            namespaces,
            namespace_uris,
            &self.execution_context,
            self.bindings.as_deref(),
        ) {
            Ok(()) => {
                span.event(SignalEvent::ArtifactAccepted);
                Ok(())
            }
            Err(super::LoadFailure::Unavailable) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Validation);
                Err(component_error(
                    ErrorCode::Unavailable,
                    "an extension namespace the document uses cannot be loaded",
                ))
            }
            Err(super::LoadFailure::Host(error)) => {
                span.fail(ErrorCategory::Platform);
                Err(fatal(&self.shared, error.into()))
            }
        }
    }

    /// `machine-capabilities.start-invocation`: an invoked SCXML session
    /// ([`Self::start_invocation`]) or an invoked service
    /// ([`Self::start_service`]).
    pub(crate) fn start(
        &mut self,
        request: Invocation,
    ) -> Result<InvocationProgress, ComponentError> {
        match request {
            Invocation::Session(request) => self.start_invocation(request),
            Invocation::Service(request) => self.start_service(request),
        }
    }

    /// Registers one invoked service as this session's invocation and
    /// defers its start to the host (WIT `invocation-progress.deferred`),
    /// like an invoked session's start: the run starts its invocation once
    /// this session's call suspended. A type no service binds answers
    /// `unavailable` (the `<invoke>` fails with `error.execution`, SCXML
    /// 6.4); an invocation beyond the host's invoke limits answers
    /// `invalid-input`.
    fn start_service(
        &mut self,
        request: InvokeRequest,
    ) -> Result<InvocationProgress, ComponentError> {
        let result = self.request_service(request);
        if result.is_err() {
            let span =
                scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeServiceStart);
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Execution);
        }
        result
    }

    fn request_service(
        &mut self,
        request: InvokeRequest,
    ) -> Result<InvocationProgress, ComponentError> {
        if self.child(&request.invoke_id).is_some() {
            return Err(component_error(
                ErrorCode::DuplicateId,
                "invoke id is already live",
            ));
        }
        if !self
            .core
            .binds_service(&request.type_uri, self.bindings.as_deref())
        {
            return Err(component_error(
                ErrorCode::Unavailable,
                "no service component is bound to this invoke type",
            ));
        }
        if !self.shared.invoked_sessions.borrow().can_invoke(self.key) {
            return Err(invalid_input(
                "the invocation exceeds the invoked sessions bounds",
            ));
        }
        let registered = self
            .shared
            .invoked_sessions
            .borrow_mut()
            .register(self.key, request.invoke_id.clone());
        let child = registered.map_err(|violation| fatal(&self.shared, violation.into()))?;
        self.shared.sessions.borrow_mut().insert(child, None);
        let deferred = self.defer(DeferredRequest::StartService {
            child,
            request: Box::new(request),
            execution_context: self.execution_context.clone(),
            bindings: self.bindings.clone(),
        });
        if deferred.is_err() {
            self.shared.invoked_sessions.borrow_mut().remove(child);
            self.shared.sessions.borrow_mut().remove(child);
        }
        deferred
    }

    /// Compiles one child document through the compiler component, with
    /// the packages its sessions bind: an `<invoke src>` document's own
    /// bindings (the one this session loaded last), else this session's
    /// for inline `<content>`; and an `<invoke src>` document's schema
    /// files (none for inline `<content>`).
    pub(crate) fn compile_invocation(
        &mut self,
        source_name: String,
        source: String,
    ) -> Result<Vec<u8>, ComponentError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeInvokeCompile);
        let bindings = self
            .loaded_bindings
            .take()
            .or_else(|| self.bindings.clone());
        let event_schemas = self.loaded_event_schemas.take().unwrap_or_default();
        self.compiled_bindings.clone_from(&bindings);
        let core = Rc::clone(&self.core);
        let packages = core.extension_packages(bindings.as_deref());
        let compiled = self.shared.with_compiler(&core, |compiler| {
            compiler.compile(source_name, source, packages.iter(), event_schemas)
        });
        match compiled {
            Ok(ChildCompile::Lowered(bytes)) => {
                span.event(SignalEvent::ArtifactProduced);
                Ok(bytes)
            }
            Ok(ChildCompile::Rejected) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Validation);
                Err(invalid_input("the compiler rejected the child document"))
            }
            Ok(ChildCompile::Failed) => {
                span.fail(ErrorCategory::Platform);
                Err(fatal(
                    &self.shared,
                    RunFailure::Harness {
                        code: "invoke-compile-failed",
                        message: "the compiler component failed on a child document".into(),
                    },
                ))
            }
            Err(error) => {
                span.fail(ErrorCategory::Platform);
                Err(fatal(&self.shared, error.into()))
            }
        }
    }

    /// Constructs and registers one invoked session and defers its
    /// start to the host (WIT `invocation-progress.deferred`): the run
    /// carries it out once this session's call suspended, so a chain of
    /// invocations never nests calls. A request that cannot be met fails
    /// here, inside the call.
    fn start_invocation(
        &mut self,
        request: InvokedSession,
    ) -> Result<InvocationProgress, ComponentError> {
        let result = self.request_start(request);
        if result.is_err() {
            let span = scxml_telemetry::observe(
                Module::HostWasmtime,
                Operation::HostWasmtimeInvokedSessionStart,
            );
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Execution);
        }
        result
    }

    fn request_start(
        &mut self,
        request: InvokedSession,
    ) -> Result<InvocationProgress, ComponentError> {
        if self.child(&request.invoke_id).is_some() {
            return Err(component_error(
                ErrorCode::DuplicateId,
                "invoke id is already live",
            ));
        }
        // The host's invoke limits (nested invoke depth, invoked sessions): an
        // invocation beyond them cannot start, which the invoking session
        // sees as the failure of that `<invoke>` (`error.execution`).
        if !self.shared.invoked_sessions.borrow().can_invoke(self.key) {
            return Err(invalid_input(
                "the invocation exceeds the invoked sessions bounds",
            ));
        }
        let clock_now = self
            .clock_now()
            .map_err(|failure| fatal(&self.shared, failure))?;
        let registered = self
            .shared
            .invoked_sessions
            .borrow_mut()
            .register(self.key, request.invoke_id.clone());
        let child = registered.map_err(|violation| fatal(&self.shared, violation.into()))?;
        let session_id = self.shared.sessions.borrow_mut().next_invoked_session_id();
        let spec = SessionSpec {
            session_id: session_id.clone(),
            key: child,
            parent: Some(ParentLink {
                session_id: self.session_id.clone(),
                invoke_id: request.invoke_id.clone(),
            }),
            data_model: request.data_model,
            machine_ir: request.machine_ir,
            clock_now,
            bindings: self.compiled_bindings.take(),
        };
        let constructed = MachineHandle::construct(
            &self.core,
            &self.shared,
            &self.embedder,
            spec,
            &self.execution_context,
        );
        let handle = match constructed {
            Ok(Constructed::Ready(handle)) => handle,
            // A child the machine rejects at load fails the invocation
            // (`error.execution` in the parent).
            Ok(Constructed::Rejected { .. }) => {
                self.shared.invoked_sessions.borrow_mut().remove(child);
                return Err(invalid_input("the invoked session was rejected at load"));
            }
            Err(failure) => {
                self.shared.invoked_sessions.borrow_mut().remove(child);
                return Err(fatal(&self.shared, failure));
            }
        };
        self.shared
            .sessions
            .borrow_mut()
            .insert(child, Some(session_id));
        let context = InvocationContext {
            parent_session_id: self.session_id.clone(),
            invoke_id: request.invoke_id,
            parameters: request.parameters,
            // The child's start batch is the invoking batch's child.
            trace_context: request.trace_context,
        };
        let deferred = self.defer(DeferredRequest::Start {
            child,
            handle,
            context,
        });
        if deferred.is_err() {
            self.shared.invoked_sessions.borrow_mut().remove(child);
            self.shared.sessions.borrow_mut().remove(child);
        }
        deferred
    }

    /// Leaves `request` for the host to carry out once this session's call
    /// suspended. One call defers at most one request before it suspends,
    /// so a pending one breaks the host's own contract and fails the run.
    fn defer(&self, request: DeferredRequest) -> Result<InvocationProgress, ComponentError> {
        let mut slot = self.shared.deferred.borrow_mut();
        if slot.is_some() {
            drop(slot);
            return Err(fatal(
                &self.shared,
                RunFailure::Harness {
                    code: "deferred-request-pending",
                    message: "a session deferred an invocation request while one was pending"
                        .into(),
                },
            ));
        }
        *slot = Some(request);
        Ok(InvocationProgress::Deferred)
    }

    /// Cancels one invocation: its cancellation begins at once, so
    /// whatever it (or a session below it) sends to its invoking session
    /// from now on is ignored (official case 252); events already on the
    /// invoking session's queue stay. The rest (its `cancel` when
    /// unfinished, then dropping it with its external queue, its timers, and
    /// every session it invoked) is deferred to the host like a
    /// start. An absent invoke id succeeds without effect.
    pub(crate) fn cancel_invocation(
        &mut self,
        invoke_id: &str,
    ) -> Result<InvocationProgress, ComponentError> {
        let Some(child) = self.child(invoke_id) else {
            let span = scxml_telemetry::observe(
                Module::HostWasmtime,
                Operation::HostWasmtimeInvokedSessionCancel,
            );
            span.event(SignalEvent::TransformationCompleted);
            return Ok(InvocationProgress::Completed);
        };
        self.shared
            .invoked_sessions
            .borrow_mut()
            .begin_cancel(child);
        self.defer(DeferredRequest::Cancel { child })
    }

    /// Queues one autoforwarded event on the external queue of a live,
    /// unfinished invoked session.
    pub(crate) fn forward_to_invocation(&mut self, invoke_id: &str, event: WitEvent) {
        let Some(target) = self.child(invoke_id) else {
            return;
        };
        if self
            .shared
            .sessions
            .borrow()
            .get(target)
            .is_none_or(|record| record.finished)
        {
            return;
        }
        self.shared
            .invoked_sessions
            .borrow_mut()
            .push_external(target, event);
    }

    /// The session clock's earliest pending deadline.
    pub(crate) fn clock_next_deadline(&mut self) -> Result<Option<u64>, RunFailure> {
        Ok(self.clock_call(|api, store| api.call_next_deadline(store))?)
    }

    /// The session clock's logical time.
    pub(crate) fn clock_now(&mut self) -> Result<u64, RunFailure> {
        Ok(self.clock_call(|api, store| api.call_now(store))?)
    }

    /// Moves the session clock to the run's shared clock `now`, when it is
    /// behind: a session's clock is moved only before it is used, so idle
    /// sessions cost nothing when the shared clock advances.
    pub(crate) fn clock_sync(&mut self, now: u64) -> Result<(), RunFailure> {
        let own = self.time.load(std::sync::atomic::Ordering::Relaxed);
        if now > own {
            self.clock_advance(now - own)?;
        }
        Ok(())
    }

    /// Advances the session clock.
    pub(crate) fn clock_advance(&mut self, delta: u64) -> Result<(), RunFailure> {
        self.clock_call(|api, store| api.call_advance(store, delta))?
            .map(|now| self.time.store(now, std::sync::atomic::Ordering::Relaxed))
            .map_err(|_| RunFailure::Harness {
                code: "send-clock-advance",
                message: "the session clock rejected an advance".into(),
            })
    }

    /// This session's `_sessionid`.
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Release digests of the components a snapshot names.
    pub(crate) fn digests(&self) -> &super::snapshot::ComponentDigests {
        &self.core.digests
    }

    /// Runs `body` over the run's invoked sessions and their external
    /// queues, with this session's key (read-only; during a machine call
    /// the run state holds them).
    pub(crate) fn with_invoked_sessions<T>(
        &self,
        body: impl FnOnce(&super::run::RunInvokedSessions, SessionKey) -> T,
    ) -> T {
        body(&self.shared.invoked_sessions.borrow(), self.key)
    }

    /// The session clock's read-only `snapshot`.
    pub(crate) fn clock_snapshot(&mut self) -> Result<ClockSnapshot, RunFailure> {
        Ok(self.clock_call(|api, store| api.call_snapshot(store))?)
    }

    /// The SCXML processor's read-only `snapshot`: pending delayed sends in
    /// clock order.
    pub(crate) fn io_snapshot(
        &mut self,
    ) -> Result<Result<Vec<PendingSend>, ComponentError>, RunFailure> {
        Ok(self.io_call(|api, store| api.call_snapshot(store))?)
    }

    /// Drains the session's due delayed sends in clock order.
    pub(crate) fn drain_due(&mut self) -> Result<Vec<DelayedSend>, RunFailure> {
        let batch = self
            .io_call(|api, store| api.call_drain_due(store, MAX_DUE_BATCH_ITEMS))?
            .map_err(|_| RunFailure::Harness {
                code: "send-drain-due",
                message: "the SCXML processor could not drain due sends".into(),
            })?;
        if batch.rejected.is_empty() {
            Ok(batch.delivered)
        } else {
            Err(RunFailure::Harness {
                code: "send-drain-due-rejected",
                message: "a drained delayed send could not be decoded".into(),
            })
        }
    }

    /// Routes one due send at delivery time through the SCXML processor.
    pub(crate) fn route_due(&mut self, request: &SendRequest) -> Result<IoDueRoute, RunFailure> {
        let sender = self.sender();
        Ok(self.io_call(|api, store| api.call_route_due(store, request, &sender))?)
    }
}

/// The `BasicHTTP` processor's view of one machine `basichttp-send`.
fn outbound_send(request: &BasichttpSend) -> OutboundSend {
    OutboundSend {
        event_name: request.event_name.clone(),
        target: request.target.clone(),
        params: request
            .params
            .iter()
            .map(|field| Param {
                name: field.name.clone(),
                value: http_value(field.value.clone()),
            })
            .collect(),
        content: request.content.clone().map(http_value),
        trace_context: request.trace_context.clone().map(http_trace_context),
    }
}

/// Appends `value` as a JSON string literal.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                out.push_str("\\u00");
                out.push(hex_digit(u32::from(control) >> 4));
                out.push(hex_digit(u32::from(control) & 0xf));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Lowercase hexadecimal digit of a nibble (`0..16`).
fn hex_digit(nibble: u32) -> char {
    char::from_digit(nibble & 0xf, 16).unwrap_or('0')
}

fn http_trace_context(context: WitTraceContext) -> HttpTraceContext {
    HttpTraceContext {
        traceparent: context.traceparent,
        tracestate: context.tracestate,
        baggage: context.baggage,
        baggage_policy: match context.baggage_policy {
            BaggagePolicy::Drop => HttpBaggagePolicy::Drop,
            BaggagePolicy::Propagate => HttpBaggagePolicy::Propagate,
        },
    }
}
