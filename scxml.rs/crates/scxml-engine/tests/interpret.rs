//! `scxml_engine::interpret` against an in-memory [`SessionCapabilities`]
//! mock: every SCXML effect semantic the machine component executes over
//! its WIT capabilities (data binding, `<send>`, `<cancel>`, `<log>`,
//! `<foreach>`, `<donedata>`, and invocations), asserted on the exact
//! capability calls and on the engine's resulting configuration.

use std::collections::BTreeMap;

use scxml_compiler_core::{MachineIr, SCXML_NAMESPACE, ScriptSourceIr, XmlNodeIr, lower};
use scxml_engine::interpret::{
    self, CapabilityErrorKind, DocumentKind, ExtensionExecution, ExtensionSnapshot, ForeachItems,
    InlineBody, InterpretFailure, Interpreter, Invocation, InvocationContext, InvocationProgress,
    InvokeRequest, InvokedSession, LoadedDocument, Observation, ProcessorSend, SendDisposition,
    SendTarget, SessionCapabilities, SessionState,
};
use scxml_engine::{BatchEnd, Command, Engine, EngineStatus, RunFailure, RunReport};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::{
    AssignPlacement, AssignSource, BaggagePolicy, DataInitializer, DataValue, Event, EventName,
    EventType, FormValueSource, InvokeId, NamedValue, SendMessage, SendRequest, StateId,
    TraceContext,
};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);
/// The source id of the documents these tests parse.
const CHILD_SOURCE: SourceId = SourceId::new(2);
/// The source id of the documents these tests parse.
const NAMED_CHILD_SOURCE: SourceId = SourceId::new(3);

/// Mock failure: the class the interpreter sees plus, for interpreter- or
/// engine-originated failures, the failure itself.
#[derive(Debug)]
struct MockError {
    kind: CapabilityErrorKind,
    failure: Option<InterpretFailure>,
}

fn fail(kind: CapabilityErrorKind) -> MockError {
    MockError {
        kind,
        failure: None,
    }
}

/// Scripted capabilities. Unscripted expressions fail with `execution`.
#[derive(Default)]
struct Mock {
    /// Ordered, human-readable capability calls.
    calls: Vec<String>,
    data_model: Option<String>,
    values: BTreeMap<String, Result<DataValue, CapabilityErrorKind>>,
    texts: BTreeMap<String, Result<Option<String>, CapabilityErrorKind>>,
    conditions: BTreeMap<String, bool>,
    documents: BTreeMap<String, String>,
    media_types: BTreeMap<String, String>,
    external_data: BTreeMap<String, DataValue>,
    initialize_failures: BTreeMap<String, CapabilityErrorKind>,
    targets: BTreeMap<String, SendTarget>,
    foreach: BTreeMap<String, ForeachItems>,
    send_result: Option<Result<SendDisposition, CapabilityErrorKind>>,
    basichttp_result: Option<Result<SendDisposition, CapabilityErrorKind>>,
    payload_none: bool,
    /// When set, the data model cannot represent inline content.
    inline_none: bool,
    compile_failure: Option<CapabilityErrorKind>,
    /// Machine IR the host answers for every child document instead of
    /// lowering it (a page-supplied `compileInvocation`).
    compiled_ir: Option<Vec<u8>>,
    start_failure: Option<CapabilityErrorKind>,
    /// Invoke ids whose start the host rejects at load (`invalid-input`).
    rejected_invokes: Vec<String>,
    /// How the mock host carries out invocation starts and cancels.
    invocation_progress: Option<InvocationProgress>,
    /// The result of the deferred request a suspended run waits on.
    deferred_result: Option<Result<(), MockError>>,
    /// Batches, logs (with the carrier they ran under), and invocation
    /// requests, in the order the interpreter made them.
    timeline: Vec<String>,
    sends: Vec<SendRequest>,
    basichttp: Vec<ProcessorSend>,
    logs: Vec<(Option<String>, String)>,
    invocations: Vec<InvokedSession>,
    observations: Vec<String>,
    payloads: Vec<Vec<NamedValue>>,
    configuration: Vec<String>,
    /// When set, batch `n` (1-based) answers [`batch_carrier`]`(n)`.
    stamp_batches: bool,
    /// The trigger carrier each opened batch saw, in order.
    batch_triggers: Vec<Option<TraceContext>>,
    /// Each autoforward: invoke id and the forwarded copy's carrier.
    forwards: Vec<(String, Option<TraceContext>)>,
    /// What `execute-extension` answers (`None`: `unavailable`, no extension
    /// component).
    extension: Option<Result<ExtensionScript, CapabilityErrorKind>>,
    /// Each executed extension: the element's local name and the bound event.
    executed: Vec<(String, String)>,
    /// `Some` when the bound extension component reads the session snapshot:
    /// each executable-content image received, with the send counter it
    /// carried.
    snapshots: Option<Vec<(scxml_engine::EngineSnapshot, u64)>>,
    /// What `load-extensions` answers (`None`: every namespace loads).
    load_failure: Option<CapabilityErrorKind>,
    /// The service types the mock host binds.
    bound_services: Vec<String>,
    /// Every service start the host accepted, in order.
    services: Vec<InvokeRequest>,
}

/// A scripted extension-component answer.
#[derive(Clone)]
struct ExtensionScript {
    outcome: Result<bool, CapabilityErrorKind>,
    internal: Vec<Event>,
    external: Vec<Event>,
}

/// Trace id every mock batch carrier shares.
const MOCK_TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

/// The carrier the mock answers for batch `index` (its span id is the
/// batch number).
fn batch_carrier(index: usize) -> TraceContext {
    TraceContext::new(
        format!("00-{MOCK_TRACE_ID}-{index:016x}-01"),
        None,
        None,
        BaggagePolicy::Drop,
    )
    .expect("mock batch carriers are canonical")
}

fn event_label(event: Option<&Event>) -> &str {
    event.map_or("-", |event| event.name.as_str())
}

impl SessionCapabilities for Mock {
    type Error = MockError;

    fn error_kind(error: &MockError) -> CapabilityErrorKind {
        error.kind
    }

    fn failure(&mut self, failure: InterpretFailure) -> MockError {
        MockError {
            kind: CapabilityErrorKind::Other,
            failure: Some(failure),
        }
    }

    fn admit_command(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    fn observe(&mut self, observation: Observation<'_>) {
        self.observations.push(match observation {
            Observation::CommandCompleted => "command".to_owned(),
            Observation::Raised(name) => format!("raised:{name}"),
            Observation::Processed(name) => format!("processed:{name}"),
        });
    }

    fn update_active_configuration(
        &mut self,
        exited: &[StateId],
        entered: &[StateId],
    ) -> Result<(), MockError> {
        self.configuration
            .retain(|state| !exited.iter().any(|gone| gone.as_str() == state));
        self.configuration
            .extend(entered.iter().map(|state| state.as_str().to_owned()));
        Ok(())
    }

    fn begin_batch(&mut self, trigger: Option<&Event>) -> Result<Option<TraceContext>, MockError> {
        self.batch_triggers
            .push(trigger.and_then(|event| event.trace_context.clone()));
        self.timeline
            .push(format!("begin:{}", self.batch_triggers.len()));
        Ok(self
            .stamp_batches
            .then(|| batch_carrier(self.batch_triggers.len())))
    }

    fn end_batch(&mut self, end: BatchEnd) {
        self.timeline.push(format!("end:{end:?}"));
    }

    fn data_model_name(&mut self) -> Option<String> {
        self.data_model.clone()
    }

    fn initialize_data_model(
        &mut self,
        declarations: &[String],
        machine_name: Option<&str>,
    ) -> Result<(), MockError> {
        self.calls.push(format!(
            "initialize:[{}]:{}",
            declarations.join(","),
            machine_name.unwrap_or("<none>")
        ));
        Ok(())
    }

    fn declare(&mut self, id: &str) -> Result<(), MockError> {
        self.calls.push(format!("declare:{id}"));
        Ok(())
    }

    fn initialize_data(
        &mut self,
        id: &str,
        initializer: &DataInitializer,
    ) -> Result<(), MockError> {
        self.calls.push(format!("data:{id}={initializer:?}"));
        match initializer {
            DataInitializer::Expression(_) => match self.initialize_failures.get(id) {
                Some(kind) => Err(fail(*kind)),
                None => Ok(()),
            },
            DataInitializer::Value(_) => Ok(()),
        }
    }

    fn set_parameter(&mut self, name: &str, value: &DataValue) -> Result<bool, MockError> {
        self.calls.push(format!("param:{name}={value:?}"));
        Ok(true)
    }

    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, MockError> {
        // `sendid==<id>` reads `_event.sendid` (SCXML 5.10.1).
        if let Some(expected) = expression.strip_prefix("sendid==") {
            return Ok(event
                .and_then(|event| event.send_id.as_ref())
                .is_some_and(|id| id.as_str() == expected));
        }
        self.conditions
            .get(expression)
            .copied()
            .ok_or(fail(CapabilityErrorKind::Execution))
    }

    fn evaluate_value(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<DataValue, MockError> {
        self.calls
            .push(format!("value:{expression}@{}", event_label(event)));
        self.values
            .get(expression)
            .cloned()
            .unwrap_or(Err(CapabilityErrorKind::Execution))
            .map_err(fail)
    }

    fn evaluate_location(
        &mut self,
        location: &str,
        event: Option<&Event>,
    ) -> Result<DataValue, MockError> {
        self.calls
            .push(format!("location:{location}@{}", event_label(event)));
        self.values
            .get(location)
            .cloned()
            .unwrap_or(Err(CapabilityErrorKind::Execution))
            .map_err(fail)
    }

    fn evaluate_text(
        &mut self,
        expression: &str,
        _event: Option<&Event>,
    ) -> Result<Option<String>, MockError> {
        self.calls.push(format!("text:{expression}"));
        self.texts
            .get(expression)
            .cloned()
            .unwrap_or(Err(CapabilityErrorKind::Execution))
            .map_err(fail)
    }

    fn evaluate_log(
        &mut self,
        expression: &str,
        _event: Option<&Event>,
    ) -> Result<String, MockError> {
        self.calls.push(format!("log-expr:{expression}"));
        match self.values.get(expression) {
            Some(Ok(DataValue::String(text))) => Ok(text.clone()),
            Some(Ok(other)) => Ok(format!("{other:?}")),
            Some(Err(kind)) => Err(fail(*kind)),
            None => Err(fail(CapabilityErrorKind::Execution)),
        }
    }

    fn evaluate_form_value(
        &mut self,
        source: &str,
        kind: FormValueSource,
        _event: Option<&Event>,
    ) -> Result<DataValue, MockError> {
        let location = kind == FormValueSource::Location;
        self.calls.push(format!("form:{source}:{location}"));
        if source == "broken" {
            return Err(fail(CapabilityErrorKind::Execution));
        }
        Ok(DataValue::String(format!("form({source})")))
    }

    fn foreach_items(
        &mut self,
        expression: &str,
        _event: Option<&Event>,
    ) -> Result<ForeachItems, MockError> {
        self.foreach
            .get(expression)
            .cloned()
            .ok_or(fail(CapabilityErrorKind::Execution))
    }

    fn namelist_key(&mut self, location: &str) -> Option<String> {
        location
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
            .then(|| location.to_owned())
    }

    fn event_payload(&mut self, entries: &[NamedValue]) -> Option<DataValue> {
        self.payloads.push(entries.to_vec());
        if self.payload_none {
            return None;
        }
        Some(DataValue::String(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>()
                .join("+"),
        ))
    }

    fn inline_content(&mut self, content: &InlineBody) -> Option<DataValue> {
        self.calls.push(format!("inline:{content:?}"));
        if self.inline_none {
            return None;
        }
        Some(match content {
            InlineBody::Text(text) => DataValue::String(text.trim().to_owned()),
            InlineBody::Markup(markup) => DataValue::Xml(markup.clone()),
        })
    }

    fn event_data_field(&mut self, data: &DataValue, name: &str) -> Option<DataValue> {
        self.calls.push(format!("field:{name}"));
        data.json_object_member(name)
    }

    fn external_data(&mut self, text: &str) -> Option<DataValue> {
        self.external_data.get(text).cloned()
    }

    fn assign(
        &mut self,
        location: &str,
        source: &AssignSource,
        placement: &AssignPlacement,
        event: Option<&Event>,
        _context: Option<&TraceContext>,
    ) -> Result<(), MockError> {
        self.calls.push(format!(
            "assign:{location}={source:?}/{placement:?}@{}",
            event_label(event)
        ));
        Ok(())
    }

    fn execute(&mut self, script: &str, _event: Option<&Event>) -> Result<(), MockError> {
        self.calls.push(format!("script:{script}"));
        Ok(())
    }

    fn run_script(
        &mut self,
        source: &ScriptSourceIr,
        _event: Option<&Event>,
    ) -> Result<(), MockError> {
        self.calls.push(format!("run-script:{}", source.uri));
        Ok(())
    }

    fn load_document(
        &mut self,
        kind: DocumentKind,
        uri: &str,
    ) -> Result<LoadedDocument, MockError> {
        self.calls.push(format!("load:{kind:?}:{uri}"));
        let text = self
            .documents
            .get(uri)
            .cloned()
            .ok_or(fail(CapabilityErrorKind::Unavailable))?;
        Ok(LoadedDocument {
            text,
            media_type: self.media_types.get(uri).cloned(),
        })
    }

    fn classify_target(&mut self, target: Option<&str>) -> SendTarget {
        self.calls.push(format!("classify:{target:?}"));
        match target {
            None | Some("#_scxml_self") => SendTarget::Local,
            Some("#_internal") => SendTarget::Internal,
            Some(target) => self
                .targets
                .get(target)
                .cloned()
                .unwrap_or(SendTarget::Invalid),
        }
    }

    fn send(&mut self, request: SendRequest) -> Result<SendDisposition, MockError> {
        self.calls
            .push(format!("send:{}", request.event.name.as_str()));
        self.sends.push(request.clone());
        if let Some(result) = self.send_result.clone() {
            return result.map_err(fail);
        }
        if request.delay_ms > 0 {
            return Ok(SendDisposition::Scheduled);
        }
        match request.target.as_deref() {
            None | Some("#_scxml_self") => {
                let mut event = request.event;
                event.origin = Some("#_scxml_session".to_owned());
                Ok(SendDisposition::EnqueueExternal(Box::new(event)))
            }
            Some(_) => Ok(SendDisposition::Routed),
        }
    }

    fn send_processor(&mut self, request: ProcessorSend) -> Result<SendDisposition, MockError> {
        // As a host does: only a bound processor answers its type (here
        // BasicHTTP's), and any other type is `invalid-input`.
        if !matches!(
            request.processor_type.as_str(),
            "basichttp" | "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor"
        ) {
            return Err(fail(CapabilityErrorKind::InvalidInput));
        }
        self.basichttp.push(request);
        self.basichttp_result
            .clone()
            .unwrap_or(Ok(SendDisposition::Routed))
            .map_err(fail)
    }

    fn cancel(&mut self, send_id: &str, _context: Option<&TraceContext>) -> Result<(), MockError> {
        self.calls.push(format!("cancel:{send_id}"));
        Ok(())
    }

    fn log(
        &mut self,
        label: Option<&str>,
        text: &str,
        context: Option<&TraceContext>,
    ) -> Result<(), MockError> {
        self.timeline.push(format!(
            "log:{}@{}",
            label.unwrap_or_default(),
            context.map_or("-", |context| context.traceparent.as_str())
        ));
        self.logs.push((label.map(str::to_owned), text.to_owned()));
        Ok(())
    }

    fn execute_extension(
        &mut self,
        element: &XmlNodeIr,
        event: Option<&Event>,
        _context: Option<&TraceContext>,
        snapshot: Option<ExtensionSnapshot<'_>>,
    ) -> Result<ExtensionExecution<MockError>, MockError> {
        assert_eq!(snapshot.is_some(), self.snapshots.is_some());
        if let (Some(snapshot), Some(snapshots)) = (snapshot, self.snapshots.as_mut()) {
            snapshots.push((snapshot.image, snapshot.session.send_counter));
        }
        let XmlNodeIr::Element { name, .. } = element else {
            return Err(fail(CapabilityErrorKind::InvalidInput));
        };
        self.executed
            .push((name.local_name.clone(), event_label(event).to_owned()));
        match self.extension.clone() {
            None => Err(fail(CapabilityErrorKind::Unavailable)),
            Some(Err(kind)) => Err(fail(kind)),
            Some(Ok(script)) => Ok(ExtensionExecution {
                outcome: script.outcome.map_err(fail),
                internal: script.internal,
                external: script.external,
            }),
        }
    }

    fn extension_snapshot_request(&mut self, _element: &XmlNodeIr) -> bool {
        self.snapshots.is_some()
    }

    fn load_extensions(&mut self, namespace_uris: &[String]) -> Result<(), MockError> {
        self.calls
            .push(format!("load-extensions {}", namespace_uris.join(" ")));
        self.load_failure.map_or(Ok(()), |kind| Err(fail(kind)))
    }

    fn compile_invocation(
        &mut self,
        source_name: &str,
        source: &str,
    ) -> Result<Vec<u8>, MockError> {
        self.calls.push(format!("compile:{source_name}"));
        if let Some(kind) = self.compile_failure {
            return Err(fail(kind));
        }
        if let Some(bytes) = &self.compiled_ir {
            return Ok(bytes.clone());
        }
        let document = parse_xml(SourceText::new(CHILD_SOURCE, source_name, source))
            .map_err(|_| fail(CapabilityErrorKind::InvalidInput))?;
        let ir = lower(&document).map_err(|_| fail(CapabilityErrorKind::InvalidInput))?;
        ir.to_json()
            .map_err(|_| fail(CapabilityErrorKind::InvalidInput))
    }

    fn start_invocation(&mut self, request: Invocation) -> Result<InvocationProgress, MockError> {
        let request = match request {
            Invocation::Session(request) => request,
            Invocation::Service(request) => return self.start_service(request),
        };
        self.calls
            .push(format!("start-invocation:{}", request.invoke_id));
        self.timeline.push(format!("start:{}", request.invoke_id));
        let result = if let Some(kind) = self.start_failure {
            Err(fail(kind))
        } else if self.rejected_invokes.contains(&request.invoke_id) {
            Err(fail(CapabilityErrorKind::InvalidInput))
        } else {
            self.invocations.push(request);
            Ok(())
        };
        self.progress(result)
    }

    fn cancel_invocation(&mut self, invoke_id: &str) -> Result<InvocationProgress, MockError> {
        self.calls.push(format!("cancel-invocation:{invoke_id}"));
        self.timeline.push(format!("cancel:{invoke_id}"));
        self.progress(Ok(()))
    }

    fn forward_to_invocation(
        &mut self,
        invoke_id: &str,
        event: &Event,
        trace_context: Option<&TraceContext>,
    ) -> Result<(), MockError> {
        self.calls
            .push(format!("forward:{invoke_id}:{}", event.name.as_str()));
        self.forwards
            .push((invoke_id.to_owned(), trace_context.cloned()));
        Ok(())
    }
}

impl Mock {
    /// Answers one invocation request now, or defers its `result` to the
    /// run's continuation.
    fn progress(&mut self, result: Result<(), MockError>) -> Result<InvocationProgress, MockError> {
        match self.invocation_progress {
            Some(InvocationProgress::Deferred) => {
                self.deferred_result = Some(result);
                Ok(InvocationProgress::Deferred)
            }
            Some(InvocationProgress::Completed) | None => {
                result.map(|()| InvocationProgress::Completed)
            }
        }
    }
}

fn machine(attributes: &str, body: &str) -> MachineIr {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript" {attributes}>{body}</scxml>"#
    );
    let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source))
        .expect("fixture SCXML is well-formed XML");
    lower(&document).expect("fixture SCXML lowers")
}

/// One interpreted session: engine, interpreter state, and mock.
struct Session {
    engine: Engine,
    state: SessionState,
    caps: Mock,
}

impl Mock {
    /// `start-invocation` of an invoked service.
    fn start_service(&mut self, request: InvokeRequest) -> Result<InvocationProgress, MockError> {
        self.calls.push(format!(
            "start-service:{}:{}",
            request.invoke_id, request.type_uri
        ));
        if !self.bound_services.contains(&request.type_uri) {
            return Err(fail(CapabilityErrorKind::Unavailable));
        }
        let result = if let Some(kind) = self.start_failure {
            Err(fail(kind))
        } else {
            self.services.push(request);
            Ok(())
        };
        self.progress(result)
    }
}

impl Session {
    fn new(ir: MachineIr, caps: Mock) -> Self {
        Self {
            engine: Engine::new(ir).expect("fixture is supported"),
            state: SessionState::new("session"),
            caps,
        }
    }

    fn start(&mut self) -> Result<RunReport, RunFailure<MockError>> {
        interpret::load_extensions(&self.engine, &mut self.caps)
            .and_then(|()| interpret::bind_initial_data(&mut self.engine, &mut self.caps, &[]))
            .map_err(|error| RunFailure {
                error,
                kind: scxml_engine::RunFailureKind::Effect,
                committed_effects: 0,
            })?;
        self.run(Command::Start)
    }

    /// Runs `command`, continuing every run that suspends at a deferred
    /// invocation request with that request's result, as a host does.
    fn run(&mut self, command: Command<'_>) -> Result<RunReport, RunFailure<MockError>> {
        let mut interpreter = Interpreter::new(&mut self.caps, &mut self.state);
        let mut report = self.engine.run(command, &mut interpreter)?;
        let mut committed_effects = report.committed_effects;
        while report.suspended {
            assert!(
                self.state.is_deferred(),
                "a suspended run waits on a request"
            );
            self.caps.calls.push("suspended".to_owned());
            let result = self
                .caps
                .deferred_result
                .take()
                .expect("a suspended run has a deferred result");
            let mut interpreter = Interpreter::continuing(&mut self.caps, &mut self.state, result);
            report = self
                .engine
                .run(Command::Continue, &mut interpreter)
                .map_err(|failure| RunFailure {
                    committed_effects: committed_effects + failure.committed_effects,
                    ..failure
                })?;
            committed_effects += report.committed_effects;
        }
        report.committed_effects = committed_effects;
        Ok(report)
    }

    fn send(&mut self, event: &Event) -> Result<RunReport, RunFailure<MockError>> {
        self.run(Command::Send(event))
    }

    fn active(&self) -> Vec<String> {
        self.engine
            .leaves()
            .iter()
            .map(|state| state.as_str().to_owned())
            .collect()
    }

    fn started(ir: MachineIr, caps: Mock) -> Self {
        let mut session = Self::new(ir, caps);
        session.start().expect("the session starts");
        session
    }
}

fn external(name: &str) -> Event {
    Event::external(EventName::new(name.to_owned()).expect("valid event name"))
}

fn text(value: &str) -> DataValue {
    DataValue::String(value.to_owned())
}

/// A two-way outcome machine: `error.execution`/`error.communication`
/// route to named finals so tests read the outcome from the configuration.
fn outcome_machine(onentry: &str) -> MachineIr {
    machine(
        r#"initial="s""#,
        &format!(
            r#"<state id="s">
                 <onentry>{onentry}</onentry>
                 <transition event="error.execution" target="exec"/>
                 <transition event="error.communication" target="comm"/>
                 <transition event="ok" target="ok"/>
               </state>
               <final id="exec"/><final id="comm"/><final id="ok"/>"#
        ),
    )
}

// ---------------------------------------------------------------- data

#[test]
fn early_binding_initializes_every_declaration_in_document_order() {
    let ir = machine(
        r#"initial="s" name="doc""#,
        r#"<datamodel><data id="a" expr="1"/><data id="b"/></datamodel>
           <state id="s"><datamodel><data id="c" expr="2"/></datamodel></state>"#,
    );
    let session = Session::started(ir, Mock::default());
    assert_eq!(
        session.caps.calls,
        [
            "initialize:[a,b,c]:doc",
            "data:a=Expression(\"1\")",
            "data:b=Value(Undefined)",
            "data:c=Expression(\"2\")",
        ]
    );
}

#[test]
fn a_document_without_a_name_gives_name_no_value_and_binds_no_unneeded_data_model() {
    // SCXML 5.10: "The SCXML processor MUST bind the variable _name at load
    // time to the value of the 'name' attribute of the <scxml> element."
    let session = Session::started(
        machine(
            r#"initial="s""#,
            r#"<datamodel><data id="a"/></datamodel><state id="s"/>"#,
        ),
        Mock::default(),
    );
    assert_eq!(session.caps.calls[0], "initialize:[a]:<none>");
    let plain = machine(
        r#"initial="s""#,
        r#"<state id="s"><onentry><raise event="e"/></onentry></state>"#,
    );
    let session = Session::started(plain, Mock::default());
    assert!(
        session.caps.calls.is_empty(),
        "a document that needs no data model binds nothing"
    );
}

#[test]
fn an_illegal_initializer_leaves_undefined_and_queues_error_execution_first() {
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel><data id="a" expr="return"/></datamodel>
           <state id="s">
             <transition event="error.execution" target="pass"/>
             <transition event="*" target="fail"/>
           </state>
           <final id="pass"/><final id="fail"/>"#,
    );
    let mut caps = Mock::default();
    caps.initialize_failures
        .insert("a".into(), CapabilityErrorKind::Execution);
    let session = Session::started(ir, caps);
    assert_eq!(
        session.caps.calls,
        [
            "initialize:[a]:<none>",
            "data:a=Expression(\"return\")",
            "data:a=Value(Undefined)",
        ]
    );
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn external_src_is_the_data_models_value_else_json_else_normalized_text() {
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel>
             <data id="a" src="file:a.json"/>
             <data id="b" src="file:b.txt"/>
             <data id="c" src="file:c.xml"/>
             <data id="d" src="file:missing"/>
           </datamodel>
           <state id="s"><transition event="error.execution" target="pass"/></state>
           <final id="pass"/>"#,
    );
    let mut caps = Mock::default();
    caps.documents
        .insert("file:a.json".into(), " [1, 2] ".into());
    caps.documents
        .insert("file:b.txt".into(), "  hello \n  world ".into());
    caps.documents.insert("file:c.xml".into(), "<x/>".into());
    caps.external_data
        .insert("<x/>".into(), DataValue::Xml("<x/>".into()));
    let session = Session::started(ir, caps);
    let data: Vec<&String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("data:"))
        .collect();
    assert_eq!(
        data,
        [
            "data:a=Value(JsonArray(\"[1,2]\"))",
            "data:b=Value(String(\"hello world\"))",
            "data:c=Value(Xml(\"<x/>\"))",
            "data:d=Value(Undefined)",
        ]
    );
    assert_eq!(
        session.active(),
        ["pass"],
        "an unresolvable src raises error.execution (SCXML 5.3)"
    );
}

/// B.2.1: "if an indication of the type of the content is available
/// (e.g., via a Content-Type header), then the Processor SHOULD try to
/// interpret the content according to that indication." The mock's data
/// model claims `1` as markup, which it reads first by content; a JSON
/// indication reads it as JSON instead, an XML indication through the data
/// model, and any other or malformed label (including `text/plain`) by
/// content.
#[test]
fn an_indicated_media_type_decides_how_external_data_is_read() {
    let labels = [
        ("a", Some("application/json")),
        ("b", Some("Application/JSON ; charset=utf-8")),
        ("c", Some("application/vnd.example+json")),
        ("d", Some("application/xml")),
        ("e", Some("text/plain")),
        ("f", Some("json")),
        ("g", None),
    ];
    let declarations = labels
        .iter()
        .map(|(id, _)| format!(r#"<data id="{id}" src="file:{id}"/>"#))
        .collect::<Vec<_>>()
        .concat();
    let ir = machine(
        r#"initial="s""#,
        &format!(r#"<datamodel>{declarations}</datamodel><state id="s"/>"#),
    );
    let mut caps = Mock::default();
    for (id, label) in labels {
        caps.documents.insert(format!("file:{id}"), " 1 ".into());
        if let Some(label) = label {
            caps.media_types.insert(format!("file:{id}"), label.into());
        }
    }
    caps.external_data
        .insert(" 1 ".into(), DataValue::Xml("<one/>".into()));
    let session = Session::started(ir, caps);
    let data: Vec<String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("data:"))
        .cloned()
        .collect();
    let number = "Value(Number(FiniteNumber(1.0)))";
    let markup = "Value(Xml(\"<one/>\"))";
    assert_eq!(
        data,
        [
            format!("data:a={number}"),
            format!("data:b={number}"),
            format!("data:c={number}"),
            format!("data:d={markup}"),
            format!("data:e={markup}"),
            format!("data:f={markup}"),
            format!("data:g={markup}"),
        ]
    );
}

#[test]
fn inline_data_is_json_text_undefined_or_markup() {
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel>
             <data id="a"> {"k": 1} </data>
             <data id="b">  some   text </data>
             <data id="c"></data>
             <data id="d"><item/></data>
           </datamodel>
           <state id="s"/>"#,
    );
    let session = Session::started(ir, Mock::default());
    let data: Vec<&String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("data:") || call.starts_with("inline:"))
        .collect();
    assert_eq!(data.len(), 5);
    assert_eq!(data[0], "data:a=Value(JsonObject(\"{\\\"k\\\":1}\"))");
    assert_eq!(data[1], "data:b=Value(String(\"some text\"))");
    assert_eq!(data[2], "data:c=Value(Undefined)");
    assert!(data[3].starts_with("inline:Markup(\"<item"), "{}", data[3]);
    assert!(data[4].starts_with("data:d=Value(Xml("), "{}", data[4]);
}

/// JSON white space (RFC 8259 §2) and XML white space are #x20, #x9, #xD,
/// and #xA only, so text padded with U+1680 OGHAM SPACE MARK is neither
/// JSON nor blank: it is a string, external or inline, indicated as JSON
/// or not.
#[test]
fn data_padded_with_other_unicode_white_space_is_a_string() {
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel>
             <data id="a" src="file:a"/>
             <data id="b" src="file:b"/>
             <data id="c">&#x1680;1&#x1680;</data>
             <data id="d">&#x1680;</data>
           </datamodel>
           <state id="s"/>"#,
    );
    let mut caps = Mock::default();
    for id in ["a", "b"] {
        caps.documents
            .insert(format!("file:{id}"), "\u{1680}[1]\u{1680}".into());
    }
    caps.media_types
        .insert("file:a".into(), "application/json".into());
    let session = Session::started(ir, caps);
    let data: Vec<&String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("data:"))
        .collect();
    assert_eq!(
        data,
        [
            r#"data:a=Value(String("\u{1680}[1]\u{1680}"))"#,
            r#"data:b=Value(String("\u{1680}[1]\u{1680}"))"#,
            r#"data:c=Value(String("\u{1680}1\u{1680}"))"#,
            r#"data:d=Value(String("\u{1680}"))"#,
        ]
    );
}

#[test]
fn xpath_inline_children_are_always_the_data_models_value() {
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel><data id="a">  text  </data></datamodel><state id="s"/>"#,
    );
    let caps = Mock {
        data_model: Some("xpath".into()),
        ..Mock::default()
    };
    let session = Session::started(ir, caps);
    assert!(
        session
            .caps
            .calls
            .contains(&"inline:Text(\"  text  \")".to_owned()),
        "{:?}",
        session.caps.calls
    );
}

#[test]
fn late_binding_declares_and_values_state_data_before_its_onentry() {
    let ir = machine(
        r#"initial="s" binding="late""#,
        r#"<datamodel><data id="top" expr="0"/></datamodel>
           <state id="s">
             <datamodel><data id="late" expr="1"/></datamodel>
             <onentry><log label="entered" expr="late"/></onentry>
           </state>"#,
    );
    let mut caps = Mock::default();
    caps.values.insert("late".into(), Ok(text("one")));
    let session = Session::started(ir, caps);
    assert_eq!(
        session.caps.calls,
        [
            "initialize:[top,late]:<none>",
            "data:late=Value(Undefined)",
            "data:top=Expression(\"0\")",
            "declare:late",
            "data:late=Expression(\"1\")",
            "log-expr:late",
        ]
    );
    assert_eq!(session.caps.logs, [(Some("entered".into()), "one".into())]);
}

#[test]
fn a_failed_late_initializer_raises_error_execution_without_aborting_entry() {
    let ir = machine(
        r#"initial="s" binding="late""#,
        r#"<state id="s">
             <datamodel><data id="late" expr="bad"/></datamodel>
             <onentry><log label="entered"/></onentry>
             <transition event="error.execution" target="pass"/>
           </state>
           <final id="pass"/>"#,
    );
    let mut caps = Mock::default();
    caps.initialize_failures
        .insert("late".into(), CapabilityErrorKind::InvalidInput);
    let session = Session::started(ir, caps);
    assert_eq!(session.caps.logs, [(Some("entered".into()), String::new())]);
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn invocation_parameters_value_only_the_top_level_data_they_name() {
    // SCXML 6.4.3: a parameter naming a top-level <data> is its initial
    // value, and its own `expr` is never evaluated; any other parameter is
    // not added.
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel><data id="p" expr="own"/></datamodel><state id="s"><datamodel><data id="q"/></datamodel></state>"#,
    );
    let mut session = Session::new(ir, Mock::default());
    let parameters = [
        NamedValue {
            name: "p".into(),
            value: text("v"),
        },
        NamedValue {
            name: "q".into(),
            value: text("w"),
        },
        NamedValue {
            name: "r".into(),
            value: text("x"),
        },
    ];
    interpret::bind_initial_data(&mut session.engine, &mut session.caps, &parameters)
        .expect("binds");
    let calls = &session.caps.calls;
    assert!(
        calls.iter().any(|call| call == "param:p=String(\"v\")"),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| call.contains("own")
            || call.starts_with("param:q")
            || call.starts_with("param:r")),
        "{calls:?}"
    );
    let plain = machine(r#"initial="s""#, r#"<state id="s"/>"#);
    let mut session = Session::new(plain, Mock::default());
    interpret::bind_initial_data(&mut session.engine, &mut session.caps, &parameters)
        .expect("a document with no data takes parameters and adds none");
    assert!(session.caps.calls.is_empty(), "{:?}", session.caps.calls);
}

#[test]
fn log_renders_its_text_through_the_data_model() {
    let mut caps = Mock::default();
    caps.values.insert("x".into(), Ok(text("rendered")));
    let session = Session::started(
        outcome_machine(r#"<log label="L" expr="x"/><log label="E"/>"#),
        caps,
    );
    assert_eq!(
        session.caps.logs,
        [
            (Some("L".into()), "rendered".into()),
            (Some("E".into()), String::new())
        ]
    );
}

#[test]
fn foreach_iterates_the_data_models_snapshot_and_rejects_illegal_collections() {
    let mut caps = Mock::default();
    caps.foreach.insert(
        "items".into(),
        ForeachItems::Items(vec![text("a"), text("b")]),
    );
    let session = Session::started(
        outcome_machine(r#"<foreach array="items" item="it" index="i"><log label="x"/></foreach>"#),
        caps,
    );
    let assigns: Vec<&String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("assign:"))
        .collect();
    assert_eq!(assigns.len(), 4, "{assigns:?}");
    assert!(assigns[0].starts_with("assign:it=ForeachValue(String(\"a\"))"));
    assert_eq!(session.caps.logs.len(), 2);

    let mut caps = Mock::default();
    caps.foreach.insert("items".into(), ForeachItems::Illegal);
    let session = Session::started(
        outcome_machine(r#"<foreach array="items" item="it"><log label="x"/></foreach>"#),
        caps,
    );
    assert_eq!(session.active(), ["exec"], "SCXML 4.6: illegal array");
    assert!(session.caps.logs.is_empty());
}

#[test]
fn foreach_index_starts_at_the_data_models_base() {
    // SCXML 4.6 leaves the index base to the data model: ECMAScript arrays
    // count from 0, while SCXML 1.0 CR 2014-03-13 §C.3.10: the XPath
    // iteration "goes from an index of 1 by increments of one to an index
    // of count(node-set)".
    for (model, expected) in [(None, [0.0, 1.0]), (Some("xpath"), [1.0, 2.0])] {
        let mut caps = Mock {
            data_model: model.map(str::to_owned),
            ..Mock::default()
        };
        caps.foreach.insert(
            "items".into(),
            ForeachItems::Items(vec![text("a"), text("b")]),
        );
        let session = Session::started(
            outcome_machine(r#"<foreach array="items" item="it" index="i"/>"#),
            caps,
        );
        let indexes: Vec<&String> = session
            .caps
            .calls
            .iter()
            .filter(|call| call.starts_with("assign:i="))
            .collect();
        assert_eq!(indexes.len(), 2, "{model:?}: {indexes:?}");
        for (call, index) in indexes.iter().zip(expected) {
            assert!(
                call.starts_with(&format!(
                    "assign:i=ForeachValue(Number(FiniteNumber({index:?})))"
                )),
                "{model:?}: {call}"
            );
        }
    }
}

#[test]
fn raise_is_observed_and_every_command_is_reported() {
    let mut session = Session::started(outcome_machine(r#"<raise event="ok"/>"#), Mock::default());
    assert_eq!(session.active(), ["ok"]);
    assert_eq!(session.caps.observations[0], "command");
    assert!(session.caps.observations.contains(&"raised:ok".to_owned()));
    let _ = session.run(Command::Resume);
}

#[test]
fn every_event_transition_selection_processes_is_observed_after_its_command() {
    // The raised internal event is processed by the next engine command,
    // whose selection took its transition.
    let session = Session::started(outcome_machine(r#"<raise event="ok"/>"#), Mock::default());
    assert_eq!(session.active(), ["ok"]);
    assert_eq!(
        session.caps.observations,
        ["command", "raised:ok", "command", "processed:ok"]
    );

    // An external event that enables no transition is still processed
    // (dequeued and examined by selection) and observed.
    let mut session = Session::started(
        machine(
            r#"initial="s""#,
            r#"<state id="s"><transition event="go" target="f"/></state><final id="f"/>"#,
        ),
        Mock::default(),
    );
    let ignored = Event::external(EventName::new("other").expect("event name fixture is valid"));
    session
        .send(&ignored)
        .expect("the ignored event is processed");
    assert_eq!(session.active(), ["s"]);
    assert_eq!(
        session.caps.observations,
        ["command", "command", "processed:other"]
    );
}

#[test]
fn entering_a_top_level_final_state_processes_no_event_raised_on_entry() {
    // Official case 415's shape: the machine halts on entering the
    // top-level final state; the event its onentry raised is never
    // processed (SCXML 3.13).
    let session = Session::started(
        machine(
            r#"initial="final""#,
            r#"<final id="final"><onentry><raise event="event1"/></onentry></final>"#,
        ),
        Mock::default(),
    );
    assert_eq!(*session.engine.status(), EngineStatus::Finished);
    assert_eq!(session.caps.observations, ["command", "raised:event1"]);
}

// ---------------------------------------------------------------- send

#[test]
fn send_evaluates_dynamic_fields_and_self_delivery_reaches_the_external_queue() {
    let mut caps = Mock::default();
    caps.texts.insert("ev".into(), Ok(Some("ok".into())));
    caps.texts
        .insert("tg".into(), Ok(Some("#_scxml_self".into())));
    let session = Session::started(
        outcome_machine(r#"<send eventexpr="ev" targetexpr="tg"/>"#),
        caps,
    );
    assert_eq!(session.active(), ["ok"]);
    let request = &session.caps.sends[0];
    assert_eq!(request.event.name.as_str(), "ok");
    assert_eq!(request.event.event_type, EventType::External);
    assert_eq!(request.target.as_deref(), Some("#_scxml_self"));
    assert_eq!(
        request.send_id, None,
        "SCXML 6.2.4: no id without idlocation"
    );
    assert_eq!(request.message, SendMessage::Empty);
    assert_eq!(request.event.data, DataValue::Undefined);
}

#[test]
fn every_argument_is_evaluated_before_the_target_is_classified() {
    // SCXML 6.2: "The Processor MUST evaluate all arguments to <send> when
    // the <send> element is evaluated, and not when the message is actually
    // dispatched. If the evaluation of <send>'s arguments produces an error,
    // the Processor MUST discard the message without attempting to deliver
    // it." An argument error is error.execution, even to an unreachable
    // target, and the target is never looked up.
    for send in [
        r##"<send event="x" target="#_scxml_gone"><param name="p" expr="bad"/></send>"##,
        r##"<send event="x" target="#_scxml_gone" delayexpr="bad"/>"##,
    ] {
        let mut caps = Mock::default();
        caps.targets
            .insert("#_scxml_gone".into(), SendTarget::Unreachable);
        caps.values
            .insert("bad".into(), Err(CapabilityErrorKind::Execution));
        caps.texts
            .insert("bad".into(), Err(CapabilityErrorKind::Execution));
        let session = Session::started(outcome_machine(send), caps);
        assert_eq!(session.active(), ["exec"], "{send}");
        assert!(
            !session
                .caps
                .calls
                .iter()
                .any(|call| call.starts_with("classify:")),
            "{send}: the target is not classified after an argument error: {:?}",
            session.caps.calls
        );
    }
}

#[test]
fn an_unreachable_target_is_classified_after_the_arguments_and_errors_carry_the_send_id() {
    let mut caps = Mock::default();
    caps.targets
        .insert("#_scxml_gone".into(), SendTarget::Unreachable);
    caps.texts.insert("d".into(), Ok(Some("1s".into())));
    caps.values.insert("v".into(), Ok(text("1")));
    let session = Session::started(
        outcome_machine(
            r##"<send event="x" target="#_scxml_gone" idlocation="loc" delayexpr="d">
                  <param name="p" expr="v"/>
                </send>"##,
        ),
        caps,
    );
    assert_eq!(session.active(), ["comm"]);
    let calls = &session.caps.calls;
    let position = |prefix: &str| calls.iter().position(|call| call.starts_with(prefix));
    let classify = position("classify:").expect("the target is classified");
    for argument in ["text:d", "value:v"] {
        assert!(
            position(argument).is_some_and(|at| at < classify),
            "{argument} is evaluated before the target is classified: {calls:?}"
        );
    }
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("assign:loc=Value(String(\"sendid_0\"))")),
        "{calls:?}"
    );
    assert_eq!(session.state.send_counter, 1);
}

#[test]
fn a_cancel_id_no_send_can_have_raises_error_execution() {
    // SCXML 6.3.1: `sendidexpr` "evaluates to the ID of a delayed event";
    // a value no send id can equal (empty, containing white space) is not a
    // legal value, which is error.execution (5.9.3), not a session failure.
    for value in ["", " ", "a b"] {
        let mut caps = Mock::default();
        caps.texts.insert("e".into(), Ok(Some(value.into())));
        let session = Session::started(outcome_machine(r#"<cancel sendidexpr="e"/>"#), caps);
        assert_eq!(session.active(), ["exec"], "{value:?}");
    }
}

#[test]
fn error_events_of_a_failed_send_carry_its_send_id() {
    let ir = machine(
        r#"initial="s""#,
        r#"<state id="s">
              <onentry><send id="mine" event="x" target="nowhere"/></onentry>
              <transition event="error.execution" cond="sendid==mine" target="pass"/>
            </state>
            <final id="pass"/>"#,
    );
    let mut session = Session::new(ir, Mock::default());
    session.start().expect("starts");
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn an_invalid_delay_raises_error_execution_with_the_send_id() {
    let mut caps = Mock::default();
    caps.texts.insert("d".into(), Ok(Some("soon".into())));
    let session = Session::started(outcome_machine(r#"<send event="x" delayexpr="d"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
    assert!(session.caps.sends.is_empty());
}

#[test]
fn delayed_sends_without_an_id_are_scheduled_under_a_generated_id() {
    let session = Session::started(
        outcome_machine(r#"<send event="later" delay="1s"/><send event="now"/>"#),
        Mock::default(),
    );
    let delayed = &session.caps.sends[0];
    assert_eq!(delayed.delay_ms, 1_000);
    assert_eq!(
        delayed.send_id.as_ref().map(scxml_types::SendId::as_str),
        Some("sendid_0")
    );
    // SCXML C.1: the author gave neither `id` nor `idlocation`, so the
    // delivered event's sendid stays empty; the generated id is the clock's.
    assert_eq!(delayed.event.send_id, None);
    assert_eq!(session.caps.sends[1].send_id, None);
}

#[test]
fn delivered_events_carry_exactly_the_authored_send_id() {
    // SCXML C.1: the receiving event's sendid matches the sending session's
    // when the author gave `id` or `idlocation`, delayed or not.
    let session = Session::started(
        outcome_machine(
            r#"<send event="a" id="lit"/><send event="b" idlocation="loc"/><send event="c" idlocation="loc" delay="1s"/><send event="d" id="late" delay="1s"/>"#,
        ),
        Mock {
            send_result: Some(Ok(SendDisposition::Routed)),
            ..Mock::default()
        },
    );
    let ids: Vec<(Option<&str>, Option<&str>)> = session
        .caps
        .sends
        .iter()
        .map(|request| {
            (
                request.send_id.as_ref().map(scxml_types::SendId::as_str),
                request
                    .event
                    .send_id
                    .as_ref()
                    .map(scxml_types::SendId::as_str),
            )
        })
        .collect();
    assert_eq!(
        ids,
        [
            (Some("lit"), Some("lit")),
            (Some("sendid_0"), Some("sendid_0")),
            (Some("sendid_1"), Some("sendid_1")),
            (Some("late"), Some("late")),
        ]
    );
}

#[test]
fn params_then_namelist_become_fields_with_duplicates_kept() {
    let mut caps = Mock::default();
    caps.values.insert("1".into(), Ok(DataValue::Null));
    caps.values.insert("v".into(), Ok(text("vv")));
    let session = Session::started(
        outcome_machine(
            r#"<send event="ok" namelist="v"><param name="p" expr="1"/><param name="p" location="v"/></send>"#,
        ),
        caps,
    );
    assert_eq!(session.active(), ["ok"]);
    let SendMessage::Params(fields) = &session.caps.sends[0].message else {
        panic!("a namelist send carries fields");
    };
    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(names, ["p", "p", "v"]);
    assert_eq!(session.caps.sends[0].event.data, text("p+p+v"));
}

#[test]
fn illegal_payloads_raise_error_execution() {
    // Content with params and a param without a source are rejected when
    // the document lowers; the interpreter still guards them.
    for onentry in [
        // An illegal namelist entry (official case 553).
        r#"<send event="ok" namelist="a.b"/>"#,
        // An unknown processor type (official case 199).
        r#"<send event="ok" type="nope"/>"#,
        // A non-string `eventexpr` (SCXML 6.2.3).
        r#"<send eventexpr="nonstring"/>"#,
        // An event name that is not a valid SCXML name.
        r#"<send eventexpr="spaced"/>"#,
    ] {
        let mut caps = Mock::default();
        caps.values.insert("1".into(), Ok(DataValue::Null));
        caps.texts.insert("nonstring".into(), Ok(None));
        caps.texts.insert("spaced".into(), Ok(Some("a b".into())));
        let session = Session::started(outcome_machine(onentry), caps);
        assert_eq!(session.active(), ["exec"], "{onentry}");
        assert!(session.caps.sends.is_empty(), "{onentry}");
    }
    let caps = Mock {
        payload_none: true,
        values: BTreeMap::from([("1".to_owned(), Ok(DataValue::Null))]),
        ..Mock::default()
    };
    let session = Session::started(
        outcome_machine(r#"<send event="ok"><param name="p" expr="1"/></send>"#),
        caps,
    );
    assert_eq!(session.active(), ["exec"], "an uncarriable value");
}

#[test]
fn content_is_the_event_data() {
    let session = Session::started(
        outcome_machine(r#"<send event="ok"><content>  hello  </content></send>"#),
        Mock::default(),
    );
    assert_eq!(session.active(), ["ok"]);
    assert_eq!(session.caps.sends[0].message, SendMessage::Content);
    assert_eq!(session.caps.sends[0].event.data, text("hello"));
}

#[test]
fn internal_target_queues_an_unstamped_internal_event_without_the_processor() {
    let session = Session::started(
        outcome_machine(r##"<send event="ok" target="#_internal"/>"##),
        Mock::default(),
    );
    assert_eq!(session.active(), ["ok"]);
    assert!(session.caps.sends.is_empty());
}

#[test]
fn processor_dispositions_map_to_scxml_errors() {
    for (result, expected) in [
        (Ok(SendDisposition::Undeliverable), "comm"),
        (Err(CapabilityErrorKind::PayloadTooLarge), "exec"),
        (Err(CapabilityErrorKind::InvalidInput), "exec"),
        // SCXML 3.12.2: "it MUST signal any errors that occur by raising
        // SCXML events whose names begin with 'error.'": a clock with no
        // room for another delayed send is an internal error.
        (Err(CapabilityErrorKind::CapacityExhausted), "exec"),
        (Ok(SendDisposition::Routed), "s"),
    ] {
        let caps = Mock {
            send_result: Some(result),
            ..Mock::default()
        };
        let session = Session::started(outcome_machine(r#"<send event="ok"/>"#), caps);
        assert_eq!(session.active(), [expected]);
    }
    let caps = Mock {
        send_result: Some(Err(CapabilityErrorKind::Other)),
        ..Mock::default()
    };
    let mut session = Session::new(outcome_machine(r#"<send event="ok"/>"#), caps);
    let failure = session.start().expect_err("platform failures are fatal");
    assert_eq!(failure.error.kind, CapabilityErrorKind::Other);
}

#[test]
fn basichttp_sends_evaluate_form_fields_namelist_first() {
    let session = Session::started(
        outcome_machine(
            r#"<send event="e" type="basichttp" target="http://x" namelist="n"><param name="p" expr="1"/></send>"#,
        ),
        Mock::default(),
    );
    let request = &session.caps.basichttp[0];
    assert_eq!(request.event_name.as_deref(), Some("e"));
    let names: Vec<&str> = request
        .params
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["n", "p"]);
    assert_eq!(request.params[0].value, text("form(n)"));
    let caps = Mock {
        basichttp_result: Some(Ok(SendDisposition::Undeliverable)),
        ..Mock::default()
    };
    let session = Session::started(
        outcome_machine(r#"<send event="e" type="basichttp" target="http://x"/>"#),
        caps,
    );
    assert_eq!(session.active(), ["comm"], "official case 577");
}

/// A machine whose `<send>` error is accepted only when `_event.sendid` is
/// `expected` (SCXML 5.10.1): `pass` then, `wrong` for any other error.
fn send_error_machine(onentry: &str, expected: &str) -> MachineIr {
    machine(
        r#"initial="s""#,
        &format!(
            r#"<state id="s">
                 <onentry>{onentry}</onentry>
                 <transition event="error" cond="sendid=={expected}" target="pass"/>
                 <transition event="error" target="wrong"/>
               </state>
               <final id="pass"/><final id="wrong"/>"#
        ),
    )
}

#[test]
fn every_error_of_a_failed_send_carries_its_send_id() {
    // SCXML 5.10.1: an error event triggered by a failed attempt to send
    // carries the send id of the triggering <send>, whichever stage failed
    // and whichever processor it selected.
    for onentry in [
        // An unknown processor type (official case 199).
        r#"<send id="mine" event="x" type="nope"/>"#,
        // A failed `eventexpr`, evaluated before the target.
        r#"<send id="mine" eventexpr="broken"/>"#,
        // An illegal `namelist` entry in the payload (official case 553).
        r#"<send id="mine" event="x" namelist="a.b"/>"#,
        // A failed `<param>` expression.
        r#"<send id="mine" event="x"><param name="p" expr="broken"/></send>"#,
        // A BasicHTTP send whose target evaluation fails.
        r#"<send id="mine" type="basichttp" targetexpr="broken"/>"#,
        // A BasicHTTP send whose form field fails.
        r#"<send id="mine" type="basichttp" target="http://x"><param name="p" expr="broken"/></send>"#,
    ] {
        let session = Session::started(send_error_machine(onentry, "mine"), Mock::default());
        assert_eq!(session.active(), ["pass"], "{onentry}");
    }
}

#[test]
fn a_send_without_an_authored_id_raises_errors_under_a_generated_one() {
    // SCXML 3.14: the processor generates a send's id each time it executes
    // when the author gave none; the error names it (5.10.1).
    let session = Session::started(
        send_error_machine(r#"<send event="x" type="nope"/>"#, "sendid_0"),
        Mock::default(),
    );
    assert_eq!(session.active(), ["pass"]);
    let caps = Mock {
        send_result: Some(Ok(SendDisposition::Undeliverable)),
        ..Mock::default()
    };
    let session = Session::started(send_error_machine(r#"<send event="x"/>"#, "sendid_0"), caps);
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn idlocation_is_stored_when_the_send_is_evaluated_and_names_its_errors() {
    // SCXML 6.2.4: the id is generated and stored when the element is
    // evaluated, so even an `eventexpr` failure names the stored id.
    let session = Session::started(
        send_error_machine(r#"<send idlocation="loc" eventexpr="broken"/>"#, "sendid_0"),
        Mock::default(),
    );
    assert_eq!(session.active(), ["pass"]);
    assert!(
        session
            .caps
            .calls
            .iter()
            .any(|call| call.starts_with("assign:loc=Value(String(\"sendid_0\"))")),
        "{:?}",
        session.caps.calls
    );
}

#[test]
fn basichttp_requests_carry_the_send_id() {
    for (onentry, expected) in [
        (
            r#"<send id="mine" type="basichttp" target="http://x" event="e"/>"#,
            "mine",
        ),
        (
            r#"<send idlocation="loc" type="basichttp" target="http://x" event="e"/>"#,
            "sendid_0",
        ),
        (
            r#"<send type="basichttp" target="http://x" event="e"/>"#,
            "sendid_0",
        ),
    ] {
        let session = Session::started(outcome_machine(onentry), Mock::default());
        assert_eq!(
            session.caps.basichttp[0].send_id.as_str(),
            expected,
            "{onentry}"
        );
    }
}

#[test]
fn an_immediate_basichttp_refusal_carries_the_send_id() {
    for (onentry, expected) in [
        (
            r#"<send id="mine" type="basichttp" target="http://x" event="e"/>"#,
            "mine",
        ),
        (
            r#"<send idlocation="loc" type="basichttp" target="http://x" event="e"/>"#,
            "sendid_0",
        ),
        (
            r#"<send type="basichttp" target="http://x" event="e"/>"#,
            "sendid_0",
        ),
    ] {
        for (result, name) in [
            (Ok(SendDisposition::Undeliverable), "error.communication"),
            (Err(CapabilityErrorKind::InvalidInput), "error.execution"),
        ] {
            let caps = Mock {
                basichttp_result: Some(result),
                ..Mock::default()
            };
            let ir = machine(
                r#"initial="s""#,
                &format!(
                    r#"<state id="s">
                         <onentry>{onentry}</onentry>
                         <transition event="{name}" cond="sendid=={expected}" target="pass"/>
                         <transition event="*" target="wrong"/>
                       </state>
                       <final id="pass"/><final id="wrong"/>"#
                ),
            );
            let session = Session::started(ir, caps);
            assert_eq!(session.active(), ["pass"], "{onentry} {name}");
        }
    }
}

// ---------------------------------------------------------------- defer

/// `xmlns:event` and an initial state, for the `<event:defer>` machines.
fn defer_machine(initial: &str, body: &str) -> MachineIr {
    machine(
        &format!(
            r#"initial="{initial}" xmlns:event="{}""#,
            scxml_types::EVENT_NAMESPACE_URI
        ),
        body,
    )
}

/// The events selection processed, in order.
fn processed(session: &Session) -> Vec<&str> {
    session
        .caps
        .observations
        .iter()
        .filter_map(|observation| observation.strip_prefix("processed:"))
        .collect()
}

#[test]
fn deferred_events_wait_for_their_deferring_state_to_be_exited_in_order() {
    let mut session = Session::started(
        defer_machine(
            "busy",
            r#"<state id="busy">
                 <transition event="a"><event:defer/></transition>
                 <transition event="b"><event:defer/></transition>
                 <transition event="done" target="idle"/>
               </state>
               <state id="idle"><transition event="a" target="took_a"/></state>
               <state id="took_a"><transition event="b" target="pass"/></state>
               <final id="pass"/>"#,
        ),
        Mock::default(),
    );
    session.send(&external("a")).expect("a is deferred");
    session.send(&external("b")).expect("b is deferred");
    assert_eq!(session.active(), ["busy"]);
    assert_eq!(processed(&session), ["a", "b"]);
    session.send(&external("done")).expect("done releases them");
    assert_eq!(session.active(), ["pass"]);
    assert_eq!(processed(&session), ["a", "b", "done", "a", "b"]);
}

#[test]
fn a_deferred_event_keeps_waiting_through_changes_inside_the_deferring_state() {
    // `p` holds `x` while its children change; it comes back once `p` is
    // exited.
    let mut session = Session::started(
        defer_machine(
            "p",
            r#"<state id="p" initial="c1">
                 <transition event="x"><event:defer/></transition>
                 <state id="c1"><transition event="next" target="c2"/></state>
                 <state id="c2"><transition event="leave" target="q"/></state>
               </state>
               <state id="q"><transition event="x" target="pass"/></state>
               <final id="pass"/>"#,
        ),
        Mock::default(),
    );
    session.send(&external("x")).expect("x is deferred");
    session.send(&external("next")).expect("c1 to c2");
    assert_eq!(session.active(), ["c2"]);
    session.send(&external("leave")).expect("p is exited");
    assert_eq!(session.active(), ["pass"]);
    assert_eq!(processed(&session), ["x", "next", "leave", "x"]);
}

#[test]
fn entering_a_state_inside_the_deferring_state_that_handles_the_event_releases_it() {
    let mut session = Session::started(
        defer_machine(
            "working",
            r#"<state id="working" initial="busy">
                 <transition event="x"><event:defer/></transition>
                 <state id="busy"><transition event="next" target="hearing"/></state>
                 <state id="hearing"><transition event="x" target="pass"/></state>
               </state>
               <final id="pass"/>"#,
        ),
        Mock::default(),
    );
    session.send(&external("x")).expect("working holds x");
    session.send(&external("next")).expect("hearing takes x");
    assert_eq!(session.active(), ["pass"]);
    assert_eq!(processed(&session), ["x", "next", "x"]);
}

#[test]
fn a_released_event_another_state_defers_is_held_again() {
    // `b` defers what `a` released, inside `<if>`, until it is exited too.
    let mut session = Session::started(
        defer_machine(
            "a",
            r#"<state id="a">
                 <transition event="x"><event:defer/></transition>
                 <transition event="next" target="b"/>
               </state>
               <state id="b">
                 <transition event="x"><if cond="true"><event:defer/></if></transition>
                 <transition event="next" target="c"/>
               </state>
               <state id="c"><transition event="x" target="pass"/></state>
               <final id="pass"/>"#,
        ),
        Mock {
            conditions: [("true".to_owned(), true)].into(),
            ..Mock::default()
        },
    );
    session.send(&external("x")).expect("a holds x");
    session.send(&external("next")).expect("b holds x again");
    assert_eq!(session.active(), ["b"]);
    session.send(&external("next")).expect("c takes x");
    assert_eq!(session.active(), ["pass"]);
    assert_eq!(processed(&session), ["x", "next", "x", "next", "x"]);
}

#[test]
fn the_microstep_that_defers_an_event_does_not_release_it() {
    let mut session = Session::started(
        defer_machine(
            "s",
            r#"<state id="s"><transition event="x" target="t"><event:defer/></transition></state>
               <state id="t">
                 <transition event="x" target="fail"/>
                 <transition event="go" target="u"/>
               </state>
               <state id="u"><transition event="x" target="pass"/></state>
               <final id="pass"/><final id="fail"/>"#,
        ),
        Mock::default(),
    );
    session.send(&external("x")).expect("x is taken and held");
    assert_eq!(session.active(), ["t"]);
    session.send(&external("go")).expect("t to u releases x");
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn regions_that_both_defer_an_event_hold_it_once() {
    let mut session = Session::started(
        defer_machine(
            "both",
            r#"<parallel id="both">
                 <state id="r1"><transition event="x"><event:defer/></transition></state>
                 <state id="r2"><transition event="x"><event:defer/></transition></state>
                 <transition event="go" target="idle"/>
               </parallel>
               <state id="idle"><transition event="x" target="once"/></state>
               <state id="once">
                 <transition event="x" target="fail"/>
                 <transition event="check" target="pass"/>
               </state>
               <final id="pass"/><final id="fail"/>"#,
        ),
        Mock::default(),
    );
    session.send(&external("x")).expect("x is deferred");
    session.send(&external("go")).expect("go releases x");
    assert_eq!(session.active(), ["once"]);
    session.send(&external("check")).expect("x came back once");
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn defer_outside_a_transition_with_an_event_is_not_machine_ir() {
    for body in [
        r#"<state id="s"><onentry><event:defer/></onentry></state>"#,
        r#"<state id="s"><transition target="s"><event:defer/></transition></state>"#,
    ] {
        let source = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" xmlns:event="{}" version="1.0" initial="s">{body}</scxml>"#,
            scxml_types::EVENT_NAMESPACE_URI
        );
        let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source))
            .expect("fixture SCXML is well-formed XML");
        assert!(lower(&document).is_err(), "{body}");
    }
}

// ---------------------------------------------------------------- cancel

#[test]
fn cancel_resolves_sendidexpr_and_rejects_non_strings() {
    let mut caps = Mock::default();
    caps.texts.insert("id".into(), Ok(Some("s1".into())));
    let session = Session::started(
        outcome_machine(r#"<cancel sendidexpr="id"/><cancel sendid="lit"/>"#),
        caps,
    );
    let cancels: Vec<&String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("cancel:"))
        .collect();
    assert_eq!(cancels, ["cancel:s1", "cancel:lit"]);
    let mut caps = Mock::default();
    caps.texts.insert("id".into(), Ok(None));
    let session = Session::started(outcome_machine(r#"<cancel sendidexpr="id"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
}

// ---------------------------------------------------------------- donedata

#[test]
fn donedata_params_ride_on_the_done_state_event() {
    let ir = machine(
        r#"initial="c""#,
        r#"<state id="c" initial="f">
             <final id="f"><donedata><param name="p" expr="1"/></donedata></final>
             <transition event="done.state.c" cond="data" target="pass"/>
           </state>
           <final id="pass"/>"#,
    );
    let mut caps = Mock::default();
    caps.values.insert("1".into(), Ok(DataValue::Null));
    caps.conditions.insert("data".into(), true);
    let session = Session::started(ir, caps);
    assert_eq!(session.active(), ["pass"]);
    assert_eq!(session.caps.payloads.len(), 1);
}

/// SCXML 5.7.1 (`<param>`): "If the 'location' attribute does not refer to a
/// valid location in the data model, or if the evaluation of the 'expr'
/// produces an error, the SCXML Processor MUST place the error
/// 'error.execution' on the internal event queue and MUST ignore the name
/// and value." Only the failing param is ignored: the others still make
/// the done event's data.
#[test]
fn a_failing_donedata_param_is_ignored_and_the_others_are_kept() {
    let ir = machine(
        r#"initial="c""#,
        r#"<state id="c" initial="f">
             <final id="f"><donedata><param name="good" expr="1"/><param name="bad" expr="boom"/><param name="also" expr="2"/></donedata></final>
             <transition event="error.execution" target="err"/>
           </state>
           <state id="err"><transition event="done.state.c" cond="data" target="pass"/></state>
           <final id="pass"/>"#,
    );
    let mut caps = Mock::default();
    caps.values.insert("1".into(), Ok(DataValue::Null));
    caps.values.insert("2".into(), Ok(DataValue::Bool(true)));
    // "boom" has no value: the mock fails it with `execution`.
    caps.conditions.insert("data".into(), true);
    let session = Session::started(ir, caps);
    assert_eq!(session.active(), ["pass"]);
    let names: Vec<Vec<&str>> = session
        .caps
        .payloads
        .iter()
        .map(|entries| entries.iter().map(|entry| entry.name.as_str()).collect())
        .collect();
    assert_eq!(names, [vec!["good", "also"]]);

    // When every param fails, the done event has no data (official cases
    // 343 and 488: "empty event.data").
    let ir = machine(
        r#"initial="c""#,
        r#"<state id="c" initial="f">
             <final id="f"><donedata><param name="bad" expr="boom"/></donedata></final>
             <transition event="error.execution" target="err"/>
           </state>
           <state id="err"><transition event="done.state.c" target="pass"/></state>
           <final id="pass"/>"#,
    );
    let session = Session::started(ir, Mock::default());
    assert_eq!(session.active(), ["pass"]);
    assert!(
        session.caps.payloads.is_empty(),
        "{:?}",
        session.caps.payloads
    );
}

#[test]
fn an_illegal_data_value_raises_error_execution_and_leaves_the_data_empty() {
    // SCXML 5.3: "If the value specified for a <data> element (by 'src',
    // children, or the environment) is not a legal data value, the SCXML
    // Processor MUST raise place error.execution in the internal event
    // queue and MUST create an empty data element in the data model with
    // the specified id."
    let ir = machine(
        r#"initial="s""#,
        r#"<datamodel><data id="d"><x/></data></datamodel>
           <state id="s"><transition event="error.execution" target="pass"/></state>
           <final id="pass"/>"#,
    );
    let caps = Mock {
        inline_none: true,
        ..Mock::default()
    };
    let session = Session::started(ir, caps);
    assert_eq!(session.active(), ["pass"]);
    assert!(
        session
            .caps
            .calls
            .iter()
            .any(|call| call == "data:d=Value(Undefined)"),
        "{:?}",
        session.caps.calls
    );
}

#[test]
fn donedata_content_the_data_model_cannot_represent_raises_error_execution() {
    // SCXML 3.12.2: once executing, the processor "MUST signal any errors
    // that occur by raising SCXML events whose names begin with 'error.'";
    // 5.9.3: a value that is not a legal data value is error.execution.
    let ir = machine(
        r#"initial="c""#,
        r#"<state id="c" initial="f">
             <final id="f"><donedata><content><x/></content></donedata></final>
             <transition event="error.execution" target="err"/>
           </state>
           <state id="err"><transition event="done.state.c" target="pass"/></state>
           <final id="pass"/>"#,
    );
    let caps = Mock {
        inline_none: true,
        ..Mock::default()
    };
    let session = Session::started(ir, caps);
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn illegal_donedata_raises_error_execution_and_still_completes() {
    let ir = machine(
        r#"initial="c""#,
        r#"<state id="c" initial="f">
             <final id="f"><donedata><param name="p" location="1+1"/></donedata></final>
             <transition event="error.execution" target="err"/>
             <transition event="done.state.c" target="done"/>
           </state>
           <state id="err"><transition event="done.state.c" target="pass"/></state>
           <state id="done"/>
           <final id="pass"/>"#,
    );
    let session = Session::started(ir, Mock::default());
    assert_eq!(session.active(), ["pass"], "official 488/528 order");
}

#[test]
fn top_level_donedata_is_return_data_for_every_session() {
    let ir = || {
        machine(
            r#"initial="f""#,
            r#"<final id="f"><donedata><content expr="v"/></donedata></final>"#,
        )
    };
    let mut caps = Mock::default();
    caps.values.insert("v".into(), Ok(text("result")));
    let mut session = Session::new(ir(), caps);
    session.state.set_invoked(InvocationContext {
        parent_session_id: "parent".into(),
        invoke_id: "child".into(),
    });
    session.start().expect("starts");
    assert_eq!(*session.engine.status(), EngineStatus::Finished);
    assert_eq!(session.state.take_return_data(), Some(text("result")));
    assert_eq!(session.state.take_return_data(), None);

    // SCXML 5.5: for a session nobody invoked the processor "SHOULD
    // return the data to the environment in an implementation-dependent
    // manner"; the machine returns it the same way.
    let mut caps = Mock::default();
    caps.values.insert("v".into(), Ok(text("result")));
    let mut session = Session::new(ir(), caps);
    session.start().expect("starts");
    assert_eq!(session.state.take_return_data(), Some(text("result")));
}

// ---------------------------------------------------------------- invoke

const CHILD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><final id="c"/></scxml>"#;

fn invoking_machine(invoke: &str) -> MachineIr {
    machine(
        r#"initial="s""#,
        &format!(
            r#"<state id="s">
                 {invoke}
                 <transition event="error.execution" target="exec"/>
                 <transition event="leave" target="left"/>
               </state>
               <final id="exec"/><state id="left"/>"#
        ),
    )
}

#[test]
fn invoke_src_compiles_and_starts_the_child_with_evaluated_parameters() {
    let mut caps = Mock::default();
    caps.documents
        .insert("file:child.scxml".into(), CHILD.into());
    caps.values.insert("1".into(), Ok(DataValue::Null));
    caps.values.insert("v".into(), Ok(text("vv")));
    let mut session = Session::started(
        invoking_machine(
            r#"<invoke src="file:child.scxml" idlocation="where" namelist="v" autoforward="true">
                 <param name="p" expr="1"/>
                 <finalize><log label="finalized"/></finalize>
               </invoke>"#,
        ),
        caps,
    );
    assert_eq!(session.active(), ["s"]);
    let started = &session.caps.invocations[0];
    assert_eq!(started.invoke_id, "s.0");
    assert_eq!(started.data_model.as_deref(), Some("ecmascript"));
    let names: Vec<&str> = started.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["p", "v"]);
    assert!(
        session
            .caps
            .calls
            .contains(&"compile:child.scxml".to_owned())
    );
    assert!(
        session
            .caps
            .calls
            .iter()
            .any(|call| call.starts_with("assign:where=Value(String(\"s.0\"))")),
        "{:?}",
        session.caps.calls
    );
    assert!(
        session.caps.calls.iter().any(|call| call == "location:v@-"),
        "invoke arguments evaluate with no current event"
    );
    // SCXML 6.4: a child event runs `<finalize>` first; an external event
    // is autoforwarded (6.4.1).
    let mut done = external("done.invoke.s.0");
    done.invoke_id = Some(InvokeId::new("s.0").expect("valid id"));
    session.send(&done).expect("delivers");
    assert_eq!(
        session.caps.logs,
        [(Some("finalized".into()), String::new())]
    );
    session.send(&external("leave")).expect("delivers");
    assert!(session.caps.calls.contains(&"forward:s.0:leave".to_owned()));
    // Exiting the invoking state cancels the invocation.
    assert!(
        session
            .caps
            .calls
            .contains(&"cancel-invocation:s.0".to_owned())
    );
    assert_eq!(session.active(), ["left"]);
    assert_eq!(session.state.invoke_counter, 1);
}

/// SCXML 6.4.1 / 6.2.4: both id counters start at 0 and only a generated
/// id takes an ordinal; a literal `id` leaves the counter alone.
#[test]
fn generated_ids_start_at_zero_and_literal_ids_take_no_ordinal() {
    let fresh = SessionState::new("session");
    assert_eq!((fresh.send_counter, fresh.invoke_counter), (0, 0));

    let mut caps = Mock::default();
    caps.documents
        .insert("file:child.scxml".into(), CHILD.into());
    let session = Session::started(
        invoking_machine(
            r#"<invoke id="kid" src="file:child.scxml"/>
               <invoke src="file:child.scxml"/>"#,
        ),
        caps,
    );
    let invoked: Vec<&str> = session
        .caps
        .invocations
        .iter()
        .map(|invocation| invocation.invoke_id.as_str())
        .collect();
    assert_eq!(invoked, ["kid", "s.0"]);
    assert_eq!(session.state.invoke_counter, 1);
    assert_eq!(session.state.send_counter, 0);
}

#[test]
fn inline_content_children_become_a_standalone_child_document() {
    let session = Session::started(
        invoking_machine(
            r#"<invoke id="kid"><content><scxml version="1.0" initial="c"><final id="c"/></scxml></content></invoke>"#,
        ),
        Mock::default(),
    );
    assert_eq!(session.caps.invocations[0].invoke_id, "kid");
    // A child document without `datamodel` uses the default data model
    // (SCXML 3.2.1: "platform-specific"), as a root document does.
    assert_eq!(
        session.caps.invocations[0].data_model.as_deref(),
        Some("ecmascript")
    );
    assert!(
        session
            .caps
            .calls
            .contains(&"compile:invoke-content.scxml".to_owned())
    );
}

/// A generated invoke id is `<state>.<n>` (SCXML 6.4.1 `stateid.platformid`)
/// and has no length bound, so a long state id still invokes.
#[test]
fn a_generated_invoke_id_of_any_length_starts() {
    let state = format!("s{}", "x".repeat(4_096));
    let mut caps = Mock::default();
    caps.documents
        .insert("file:child.scxml".into(), CHILD.into());
    let session = Session::started(
        machine(
            &format!(r#"initial="{state}""#),
            &format!(
                r#"<state id="{state}">
                     <invoke src="file:child.scxml"/>
                     <transition event="error.execution" target="exec"/>
                   </state>
                   <final id="exec"/>"#
            ),
        ),
        caps,
    );
    assert_eq!(session.active(), [state.as_str()]);
    assert_eq!(session.caps.invocations[0].invoke_id, format!("{state}.0"));
}

#[test]
fn invocation_failures_raise_error_execution_and_start_nothing() {
    // An unresolvable source.
    let session = Session::started(
        invoking_machine(r#"<invoke src="file:missing"/>"#),
        Mock::default(),
    );
    assert_eq!(session.active(), ["exec"]);
    assert!(session.caps.invocations.is_empty());
    // A type no host binds (official case 216): a literal one must be a
    // service URI (`PKG003`), so the unsupported type is evaluated.
    let mut caps = Mock::default();
    caps.texts.insert("kind".into(), Ok(Some("nope".into())));
    let session = Session::started(
        invoking_machine(r#"<invoke typeexpr="kind" src="file:x"/>"#),
        caps,
    );
    assert_eq!(session.active(), ["exec"]);
    // A document the compiler rejects.
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), CHILD.into());
    caps.compile_failure = Some(CapabilityErrorKind::InvalidInput);
    let session = Session::started(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
    // Compiled child IR that does not decode, or that decodes but fails
    // Machine IR validation (a `<raise>` event that is not a name): the
    // invocation fails (SCXML 3.12.2), the session goes on.
    let named = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c"><state id="c"><onentry><raise event="NAME"/></onentry></state></scxml>"#;
    let named = lower(
        &parse_xml(SourceText::new(NAMED_CHILD_SOURCE, "named", named)).expect("child parses"),
    )
    .expect("child lowers")
    .to_json()
    .expect("child IR encodes");
    let not_a_name = String::from_utf8(named)
        .expect("IR JSON is UTF-8")
        .replace("\"NAME\"", "\"e e\"")
        .into_bytes();
    for compiled in [b"not machine IR".to_vec(), not_a_name] {
        let mut caps = Mock::default();
        caps.documents.insert("file:x".into(), CHILD.into());
        caps.compiled_ir = Some(compiled);
        let session = Session::started(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
        assert_eq!(session.active(), ["exec"]);
        assert!(session.caps.invocations.is_empty());
    }
    // A child rejected at load.
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), CHILD.into());
    caps.start_failure = Some(CapabilityErrorKind::InvalidInput);
    let session = Session::started(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
    // A failing parameter (official case 554).
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), CHILD.into());
    let session = Session::started(
        invoking_machine(r#"<invoke src="file:x"><param name="p" expr="boom"/></invoke>"#),
        caps,
    );
    assert_eq!(session.active(), ["exec"]);
    assert!(session.caps.invocations.is_empty());
    // A child document that itself invokes starts like any other.
    let nested = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c"><state id="c"><invoke src="file:y"/></state></scxml>"#;
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), nested.into());
    let session = Session::started(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
    assert_eq!(session.active(), ["s"]);
    assert_eq!(session.caps.invocations.len(), 1);
}

/// WIT `start-invocation`: only `invalid-input` (a child rejected at load)
/// fails the invocation; `unavailable` (the host provides no invocation),
/// `capacity-exhausted`, and every other class end the
/// invoking session, like any other effect capability error.
#[test]
fn start_invocation_failures_other_than_invalid_input_end_the_session() {
    for kind in [
        CapabilityErrorKind::Unavailable,
        CapabilityErrorKind::PayloadTooLarge,
        CapabilityErrorKind::Other,
    ] {
        let mut caps = Mock::default();
        caps.documents.insert("file:x".into(), CHILD.into());
        caps.start_failure = Some(kind);
        let mut session = Session::new(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
        let failure = session
            .start()
            .expect_err("a non-recoverable start-invocation failure ends the session");
        assert_eq!(failure.error.kind, kind, "{kind:?} is returned unchanged");
        assert!(failure.error.failure.is_none(), "{kind:?}: host failure");
        assert!(session.caps.invocations.is_empty(), "{kind:?}");
    }
}

const ECHO: &str = "urn:svc:echo";

/// A machine that invokes `invoke` in `s`, routing `error.execution`,
/// `error.communication`, and `echo.*` to named states.
fn service_machine(invoke: &str) -> MachineIr {
    machine(
        r#"initial="s""#,
        &format!(
            r#"<state id="s">
                 {invoke}
                 <transition event="error.execution" target="exec"/>
                 <transition event="error.communication" target="comm"/>
                 <transition event="leave" target="left"/>
               </state>
               <final id="exec"/><final id="comm"/><state id="left"/>"#
        ),
    )
}

#[test]
fn a_service_invocation_evaluates_its_request_and_loads_nothing() {
    let mut caps = Mock::default();
    caps.bound_services.push(ECHO.into());
    caps.values.insert("v".into(), Ok(text("value")));
    caps.values.insert("c".into(), Ok(text("payload")));
    caps.texts
        .insert("where".into(), Ok(Some("svc://evaluated".into())));
    let session = Session::started(
        service_machine(&format!(
            r#"<invoke type="{ECHO}" id="echo" srcexpr="where"><param name="p" expr="v"/><content expr="c"/></invoke>"#
        )),
        caps,
    );
    assert_eq!(session.active(), ["s"]);
    assert!(
        !session
            .caps
            .calls
            .iter()
            .any(|call| call.starts_with("load-document") || call.starts_with("compile:")),
        "a service source is data, never loaded or compiled: {:?}",
        session.caps.calls
    );
    let [request] = session.caps.services.as_slice() else {
        panic!("one service start: {:?}", session.caps.services);
    };
    assert_eq!(request.invoke_id, "echo");
    assert_eq!(request.type_uri, ECHO);
    assert_eq!(request.source.as_deref(), Some("svc://evaluated"));
    assert_eq!(request.content, Some(text("payload")));
    assert_eq!(
        request.parameters,
        [NamedValue {
            name: "p".into(),
            value: text("value")
        }]
    );
    let image = session
        .engine
        .snapshot(scxml_engine::SnapshotOptions::default())
        .expect("a stable session snapshots");
    assert_eq!(image.invocations.len(), 1);
    assert_eq!(image.invocations[0].type_uri, ECHO);
}

#[test]
fn an_unbound_or_platform_specific_service_type_raises_error_execution() {
    // `typeexpr` naming a service no host binds (`unavailable`).
    let mut caps = Mock::default();
    caps.texts
        .insert("t".into(), Ok(Some("urn:svc:missing".into())));
    let session = Session::started(service_machine(r#"<invoke typeexpr="t"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
    assert!(session.caps.services.is_empty());
    // `typeexpr` evaluating to a platform-specific type that is no URI
    // (SCXML 6.4.1): the host judges it, and none binds it.
    let mut caps = Mock::default();
    caps.texts.insert("t".into(), Ok(Some("echo".into())));
    let session = Session::started(service_machine(r#"<invoke typeexpr="t"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
    assert!(
        session
            .caps
            .calls
            .iter()
            .any(|call| call == "start-service:s.0:echo"),
        "{:?}",
        session.caps.calls
    );
    // A service that rejects its start.
    let mut caps = Mock::default();
    caps.bound_services.push(ECHO.into());
    caps.start_failure = Some(CapabilityErrorKind::InvalidInput);
    let session = Session::started(
        service_machine(&format!(r#"<invoke type="{ECHO}"/>"#)),
        caps,
    );
    assert_eq!(session.active(), ["exec"]);
}

#[test]
fn an_unbound_literal_service_type_raises_error_execution_at_invocation_start() {
    // SCXML 6.4: invocation is a runtime action. A literal type no host
    // binds loads nothing at session start; its `<invoke>` fails when it
    // starts, exactly like an unbound `typeexpr`.
    let session = Session::started(
        service_machine(&format!(r#"<invoke type="{ECHO}"/>"#)),
        Mock::default(),
    );
    assert_eq!(session.active(), ["exec"]);
    assert!(session.caps.services.is_empty());
    assert!(
        session
            .caps
            .calls
            .iter()
            .any(|call| call.starts_with("start-service")),
        "{:?}",
        session.caps.calls
    );
}

#[test]
fn a_deferred_service_start_settles_when_the_run_continues() {
    let mut caps = Mock::default();
    caps.bound_services.push(ECHO.into());
    caps.invocation_progress = Some(InvocationProgress::Deferred);
    let session = Session::started(
        service_machine(&format!(r#"<invoke type="{ECHO}" id="echo"/>"#)),
        caps,
    );
    assert_eq!(session.active(), ["s"]);
    assert!(session.caps.calls.contains(&"suspended".to_owned()));
    let image = session
        .engine
        .snapshot(scxml_engine::SnapshotOptions::default())
        .expect("snapshot");
    assert_eq!(image.invocations[0].type_uri, ECHO);
}

#[test]
fn a_failed_service_raises_error_communication_named_after_its_invoke_id() {
    let mut caps = Mock::default();
    caps.bound_services.push(ECHO.into());
    let ir = machine(
        r#"initial="s""#,
        &format!(
            r#"<state id="s">
                 <invoke type="{ECHO}" id="echo"/>
                 <transition event="error.communication.other" target="other"/>
                 <transition event="error.communication" cond="failedEcho" target="comm"/>
               </state>
               <final id="other"/>
               <final id="comm"/>"#
        ),
    );
    caps.conditions.insert("failedEcho".into(), true);
    let mut session = Session::started(ir, caps);
    session
        .engine
        .queue_communication_error(None, Some(InvokeId::new("echo").expect("id")), None)
        .expect("a running session queues the failure");
    let queued = session
        .engine
        .next_internal()
        .expect("the failure waits on the internal queue")
        .clone();
    assert_eq!(queued.name.as_str(), "error.communication.echo");
    assert_eq!(queued.event_type, EventType::Platform);
    assert_eq!(
        queued.invoke_id.as_ref().map(InvokeId::as_str),
        Some("echo")
    );
    session
        .run(Command::Resume)
        .expect("the resumed run processes it");
    assert_eq!(session.active(), ["comm"]);
    assert!(
        session
            .caps
            .calls
            .contains(&"cancel-invocation:echo".to_owned()),
        "leaving the invoking state cancels the failed service: {:?}",
        session.caps.calls
    );
}

#[test]
fn an_invoked_session_invokes_like_any_other_session() {
    let invoking = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><state id="c"><invoke src="file:y"/></state></scxml>"#;
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), invoking.into());
    caps.documents.insert("file:y".into(), CHILD.into());
    let mut session = Session::new(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
    session.state.set_invoked(InvocationContext {
        parent_session_id: "parent".into(),
        invoke_id: "me".into(),
    });
    session
        .start()
        .expect("an invoked session starts its own invocation (SCXML 6.4)");
    assert_eq!(session.caps.invocations.len(), 1);
}

/// Everything one run of a session observably did through its capabilities.
#[derive(Debug, PartialEq)]
struct Observed {
    timeline: Vec<String>,
    calls: Vec<String>,
    observations: Vec<String>,
    active: Vec<String>,
    invoke_counter: u64,
    outcome: Result<(u32, bool), CapabilityErrorKind>,
}

/// Runs `drive` on a session of `ir` whose host carries invocation
/// requests out as `progress` says.
fn observed(
    ir: &MachineIr,
    progress: InvocationProgress,
    caps: &Mock,
    drive: impl Fn(&mut Session) -> Result<RunReport, RunFailure<MockError>>,
) -> Observed {
    let mut session = Session::new(
        ir.clone(),
        Mock {
            documents: caps.documents.clone(),
            rejected_invokes: caps.rejected_invokes.clone(),
            start_failure: caps.start_failure,
            stamp_batches: true,
            invocation_progress: Some(progress),
            ..Mock::default()
        },
    );
    let outcome = drive(&mut session)
        .map(|report| (report.committed_effects, report.stable))
        .map_err(|failure| failure.error.kind);
    if progress == InvocationProgress::Deferred {
        assert!(
            session.caps.calls.iter().any(|call| call == "suspended"),
            "the deferring host suspended a run"
        );
    }
    Observed {
        timeline: session.caps.timeline,
        calls: session
            .caps
            .calls
            .into_iter()
            .filter(|call| call != "suspended")
            .collect(),
        observations: session.caps.observations,
        active: session
            .engine
            .leaves()
            .iter()
            .map(|state| state.as_str().to_owned())
            .collect(),
        invoke_counter: session.state.invoke_counter,
        outcome,
    }
}

/// SCXML 6.4: a host that starts or cancels an invocation after the
/// invoking run suspended (Engine `Command::Continue`) is indistinguishable
/// from one that does it inside the call: the same requests, logs,
/// batches and their carriers, observations, configuration, generated ids,
/// committed effects, and failures, in the same order.
#[test]
fn a_deferred_invocation_start_or_cancel_is_indistinguishable_from_an_immediate_one() {
    let ir = machine(
        r#"initial="s""#,
        r#"<state id="s" initial="s1">
             <invoke id="a" src="file:x"/>
             <invoke id="b" src="file:x"/>
             <invoke src="file:x"/>
             <onexit><log label="exit-s"/></onexit>
             <transition event="leave" target="out"><log label="leaving"/></transition>
             <state id="s1">
               <invoke id="d" src="file:x"/>
               <onentry><log label="enter-s1"/></onentry>
               <onexit><log label="exit-s1"/></onexit>
               <transition event="error.execution" target="s2"><log label="recovered"/></transition>
             </state>
             <state id="s2">
               <invoke id="e" src="file:x"/>
               <onentry><log label="enter-s2"/></onentry>
               <onexit><log label="exit-s2"/></onexit>
             </state>
           </state>
           <final id="out"><onentry><log label="out"/></onentry></final>"#,
    );
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), CHILD.into());
    caps.rejected_invokes.push("b".into());
    let start_send_leave = |session: &mut Session| {
        session.start()?;
        session.send(&external("leave"))
    };
    let immediate = observed(&ir, InvocationProgress::Completed, &caps, start_send_leave);
    assert_eq!(immediate.active, ["out"]);
    assert!(
        immediate.timeline.contains(&"cancel:e".to_owned()),
        "{:?}",
        immediate.timeline
    );
    let deferred = observed(&ir, InvocationProgress::Deferred, &caps, start_send_leave);
    assert_eq!(deferred, immediate);

    // exitInterpreter (`Command::Cancel`): every active invocation is
    // cancelled as the final `<onexit>` of its state.
    let cancel = |session: &mut Session| {
        session.start()?;
        session.run(Command::Cancel)
    };
    let immediate = observed(&ir, InvocationProgress::Completed, &caps, cancel);
    let deferred = observed(&ir, InvocationProgress::Deferred, &caps, cancel);
    assert_eq!(deferred, immediate);

    // A start failure that ends the session ends it the same way.
    caps.start_failure = Some(CapabilityErrorKind::Other);
    let start = |session: &mut Session| session.start();
    let immediate = observed(&ir, InvocationProgress::Completed, &caps, start);
    assert_eq!(immediate.outcome, Err(CapabilityErrorKind::Other));
    let deferred = observed(&ir, InvocationProgress::Deferred, &caps, start);
    assert_eq!(deferred, immediate);
}

/// A suspended run accepts only `Command::Continue`, and `Continue` needs a
/// suspended run; each rejection leaves the engine unchanged.
#[test]
fn a_suspended_run_continues_only_through_continue() {
    let mut caps = Mock::default();
    caps.documents.insert("file:x".into(), CHILD.into());
    caps.invocation_progress = Some(InvocationProgress::Deferred);
    let mut session = Session::new(invoking_machine(r#"<invoke src="file:x"/>"#), caps);
    let continued = {
        let mut interpreter =
            Interpreter::continuing(&mut session.caps, &mut session.state, Ok(()));
        session.engine.run(Command::Continue, &mut interpreter)
    };
    assert_eq!(
        continued.expect_err("nothing is suspended").kind,
        scxml_engine::RunFailureKind::Rejected
    );
    interpret::bind_initial_data(&mut session.engine, &mut session.caps, &[]).expect("data binds");
    let report = {
        let mut interpreter = Interpreter::new(&mut session.caps, &mut session.state);
        session
            .engine
            .run(Command::Start, &mut interpreter)
            .expect("the start runs to the invocation")
    };
    assert!(report.suspended && !report.stable);
    assert!(session.engine.is_suspended());
    let rejected = session
        .send(&external("leave"))
        .expect_err("a suspended run takes no new command");
    assert_eq!(rejected.kind, scxml_engine::RunFailureKind::Rejected);
    assert!(session.engine.is_suspended());
    let result = session.caps.deferred_result.take().expect("deferred");
    let report = {
        let mut interpreter =
            Interpreter::continuing(&mut session.caps, &mut session.state, result);
        session
            .engine
            .run(Command::Continue, &mut interpreter)
            .expect("the run continues")
    };
    assert!(!report.suspended && report.stable);
    assert!(!session.engine.is_suspended());
    assert_eq!(session.active(), ["s"]);
}

// ---------------------------------------------------------------- trace

/// Every event a batch raises, sends, or fails with carries that batch's
/// own carrier, so the batch that processes it is its child.
#[test]
fn raised_sent_and_send_error_events_carry_the_sending_batch_carrier() {
    let mut caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    caps.targets
        .insert("#_scxml_gone".into(), SendTarget::Unreachable);
    let session = Session::started(
        machine(
            r#"initial="s""#,
            r##"<state id="s">
                  <onentry><raise event="r"/><send event="i" target="#_internal"/><send event="x"/></onentry>
                  <onentry><send event="y" target="#_scxml_gone"/></onentry>
                  <onentry><send event="z" target="nowhere"/></onentry>
                  <transition event="r i error.communication error.execution"/>
                </state>"##,
        ),
        caps,
    );
    // After start, the batches processing r, i (#_internal), error.communication, and
    // error.execution (each `<onentry>` is its own block) all name the
    // start batch (1); the eventless batches have no trigger.
    let triggers = &session.caps.batch_triggers;
    assert_eq!(triggers[0], None, "start has no trigger");
    let carried: Vec<_> = triggers.iter().flatten().collect();
    assert_eq!(carried, [&batch_carrier(1); 4], "{triggers:?}");
    assert_eq!(
        session.caps.sends[0].event.trace_context.as_ref(),
        Some(&batch_carrier(1)),
        "the processor receives the sending batch's carrier"
    );
}

/// An error queued outside a block failure (a failed invocation start)
/// names the batch that attempted it.
#[test]
fn a_failed_invocation_error_carries_the_invoking_batch_carrier() {
    let caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    let session = Session::started(invoking_machine(r#"<invoke src="file:missing"/>"#), caps);
    assert_eq!(session.active(), ["exec"]);
    let triggers = &session.caps.batch_triggers;
    let last = triggers.len();
    let parent = triggers[last - 1]
        .as_ref()
        .expect("the error.execution batch has a carrier");
    let parent_batch = (1..last)
        .find(|index| &batch_carrier(*index) == parent)
        .expect("the carrier names an earlier batch");
    assert!(parent_batch < last);
}

/// An autoforwarded copy carries the forwarding batch's carrier, not the
/// carrier the event arrived with.
#[test]
fn an_autoforwarded_event_carries_the_forwarding_batch_carrier() {
    let mut caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    caps.documents
        .insert("file:child.scxml".into(), CHILD.into());
    let mut session = Session::started(
        invoking_machine(r#"<invoke id="kid" src="file:child.scxml" autoforward="true"/>"#),
        caps,
    );
    let inbound = TraceContext::new(
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        None,
        Some("tenant=a".into()),
        BaggagePolicy::Propagate,
    )
    .expect("canonical inbound carrier");
    let mut leave = external("leave");
    leave.trace_context = Some(inbound.clone());
    let before = session.caps.batch_triggers.len();
    session.send(&leave).expect("delivers");
    assert_eq!(
        session.caps.batch_triggers.get(before),
        Some(&Some(inbound)),
        "the processing batch is the inbound carrier's child"
    );
    assert_eq!(
        session.caps.forwards,
        [("kid".to_owned(), Some(batch_carrier(before + 1)))]
    );
}

/// The invoked session's start request carries the invoking batch's
/// carrier, so the child's start batch is that batch's child.
#[test]
fn an_invoked_session_carries_the_invoking_batch_carrier() {
    let mut caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    caps.documents
        .insert("file:child.scxml".into(), CHILD.into());
    let session = Session::started(
        invoking_machine(r#"<invoke id="kid" src="file:child.scxml"/>"#),
        caps,
    );
    let invoking = session.caps.batch_triggers.len();
    assert_eq!(
        session
            .caps
            .invocations
            .iter()
            .map(|request| request.trace_context.clone())
            .collect::<Vec<_>>(),
        [Some(batch_carrier(invoking))],
        "the request names the batch that ran <invoke>"
    );
}

/// A failed eventless transition condition names the batch whose effects
/// it observed (here the start batch), not nothing.
#[test]
fn a_failed_eventless_condition_error_carries_the_preceding_batch_carrier() {
    let caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    let session = Session::started(
        machine(
            r#"initial="s""#,
            r#"<state id="s">
                  <transition cond="broken" target="never"/>
                  <transition event="error.execution" target="done"/>
                </state>
                <state id="never"/><final id="done"/>"#,
        ),
        caps,
    );
    assert_eq!(session.active(), ["done"]);
    assert_eq!(
        session.caps.batch_triggers,
        [None, Some(batch_carrier(1))],
        "the error.execution batch is the start batch's child"
    );
}

/// Before the run opens any batch, a failed condition on an external
/// event names the event under evaluation: the error's batch is a sibling
/// of the event's own batch, never an earlier call's batch.
#[test]
fn a_failed_condition_before_the_first_batch_carries_the_evaluated_event_carrier() {
    let caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    let mut session = Session::started(
        machine(
            r#"initial="s""#,
            r#"<state id="s">
                  <transition event="go" cond="broken" target="never"/>
                  <transition event="error.execution" target="done"/>
                </state>
                <state id="never"/><final id="done"/>"#,
        ),
        caps,
    );
    let inbound = TraceContext::new(
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        None,
        None,
        BaggagePolicy::Drop,
    )
    .expect("canonical inbound carrier");
    let mut go = external("go");
    go.trace_context = Some(inbound.clone());
    session.send(&go).expect("delivers");
    assert_eq!(session.active(), ["done"]);
    assert_eq!(
        session.caps.batch_triggers.last(),
        Some(&Some(inbound)),
        "{:?}",
        session.caps.batch_triggers
    );
}

/// A failed `<if>` condition names the batch that ran the content before
/// it in the same transaction.
#[test]
fn a_failed_if_condition_error_carries_the_evaluating_batch_carrier() {
    let caps = Mock {
        stamp_batches: true,
        ..Mock::default()
    };
    let session = Session::started(
        machine(
            r#"initial="s""#,
            r#"<state id="s">
                  <onentry><if cond="broken"><raise event="never"/></if></onentry>
                  <transition event="error.execution" target="done"/>
                </state>
                <final id="done"/>"#,
        ),
        caps,
    );
    assert_eq!(session.active(), ["done"]);
    // Start (1) defers the `<if>`; its continuation batch (2) follows the
    // decision; the error.execution batch names batch 1.
    assert_eq!(
        session.caps.batch_triggers,
        [None, None, Some(batch_carrier(1))]
    );
}

#[test]
fn donedata_alone_requires_the_data_model() {
    let ir = machine(
        r#"initial="f""#,
        r#"<final id="f"><donedata><param name="a" expr="42"/></donedata></final>"#,
    );
    assert!(scxml_engine::interpret::requires_data_model(&ir));
    let plain = machine(r#"initial="f""#, r#"<final id="f"/>"#);
    assert!(!scxml_engine::interpret::requires_data_model(&plain));
}

/// Every executable-content site, at any nesting depth, counts toward the
/// data-model requirement; each document's only data-model use sits at one
/// site (SCXML 3.6, 5.9: `<initial>` transition content runs in the data
/// model like any other executable content).
#[test]
fn every_executable_content_site_requires_the_data_model() {
    let log = r#"<log label="x" expr="1 + 1"/>"#;
    let sites = [
        (
            "nested initial transition",
            r#"initial="p""#.to_owned(),
            format!(
                r#"<state id="p"><initial><transition target="a">{log}</transition></initial><state id="a"/></state>"#
            ),
        ),
        (
            "deeply nested initial transition",
            r#"initial="p""#.to_owned(),
            format!(
                r#"<state id="p"><state id="q"><initial><transition target="a">{log}</transition></initial><state id="a"/></state></state>"#
            ),
        ),
        (
            "nested onentry",
            r#"initial="p""#.to_owned(),
            format!(r#"<state id="p"><state id="a"><onentry>{log}</onentry></state></state>"#),
        ),
        (
            "nested onexit",
            r#"initial="p""#.to_owned(),
            format!(r#"<state id="p"><state id="a"><onexit>{log}</onexit></state></state>"#),
        ),
        (
            "nested transition",
            r#"initial="p""#.to_owned(),
            format!(
                r#"<state id="p"><state id="a"><transition event="e" target="a">{log}</transition></state></state>"#
            ),
        ),
        (
            "history default transition",
            r#"initial="p""#.to_owned(),
            format!(
                r#"<state id="p"><history id="h"><transition target="a">{log}</transition></history><state id="a"/></state>"#
            ),
        ),
        (
            "nested donedata",
            r#"initial="p""#.to_owned(),
            r#"<state id="p"><final id="f"><donedata><content expr="1"/></donedata></final></state>"#
                .to_owned(),
        ),
        (
            "invoke finalize",
            r#"initial="p""#.to_owned(),
            format!(
                r#"<state id="p"><invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="c"/></scxml></content><finalize>{log}</finalize></invoke></state>"#
            ),
        ),
    ];
    for (site, attributes, body) in &sites {
        let ir = machine(attributes, body);
        assert!(
            scxml_engine::interpret::requires_data_model(&ir),
            "{site}: executable content must require the data model"
        );
    }
    let plain = machine(
        r#"initial="p""#,
        r#"<state id="p"><initial><transition target="a"><raise event="e"/></transition></initial><state id="a"/></state>"#,
    );
    assert!(!scxml_engine::interpret::requires_data_model(&plain));
}

// ---------------------------------------------------------------- extension
// namespaces

/// A machine whose transition on `go` runs one executable content extension `x:act`.
fn extension_machine() -> MachineIr {
    machine(
        r#"initial="s" xmlns:x="https://example.org/x""#,
        r#"<state id="s">
             <transition event="go"><x:act/></transition>
             <transition event="error.execution" target="exec"/>
             <transition event="internal.first" target="internal"/>
             <transition event="external.second" target="external"/>
           </state>
           <final id="exec"/><final id="internal"/><final id="external"/>"#,
    )
}

fn internal_event(name: &str) -> Event {
    Event::internal(EventName::new(name.to_owned()).expect("valid event name"))
}

fn executed(script: Result<ExtensionScript, CapabilityErrorKind>) -> Session {
    let caps = Mock {
        extension: Some(script),
        ..Mock::default()
    };
    let mut session = Session::started(extension_machine(), caps);
    let _ = session.send(&external("go"));
    session
}

#[test]
fn a_handled_executable_content_extension_sees_its_event_and_queues_internal_before_external() {
    let session = executed(Ok(ExtensionScript {
        outcome: Ok(true),
        internal: vec![internal_event("internal.first")],
        external: vec![external("external.second")],
    }));
    assert_eq!(session.caps.executed, [("act".to_owned(), "go".to_owned())]);
    assert_eq!(
        session.active(),
        ["internal"],
        "the internal queue drains before the external one"
    );
}

#[test]
fn external_events_a_extension_sends_reach_the_external_queue() {
    let session = executed(Ok(ExtensionScript {
        outcome: Ok(true),
        internal: Vec::new(),
        external: vec![external("external.second")],
    }));
    assert_eq!(session.active(), ["external"]);
}

#[test]
fn a_declined_or_failed_executable_content_extension_raises_error_execution() {
    for outcome in [
        Ok(false),
        Err(CapabilityErrorKind::Execution),
        Err(CapabilityErrorKind::InvalidInput),
    ] {
        let session = executed(Ok(ExtensionScript {
            outcome,
            internal: Vec::new(),
            external: Vec::new(),
        }));
        assert_eq!(session.active(), ["exec"], "{outcome:?}");
    }
}

#[test]
fn events_a_failing_extension_queued_are_still_applied() {
    let session = executed(Ok(ExtensionScript {
        outcome: Ok(false),
        internal: vec![internal_event("internal.first")],
        external: Vec::new(),
    }));
    assert_eq!(
        session.active(),
        ["internal"],
        "the queued event precedes the error.execution the decline raises"
    );
}

#[test]
fn an_unbound_namespace_or_a_fatal_extension_failure_fails_the_session() {
    for (script, kind) in [
        (None, CapabilityErrorKind::Unavailable),
        (
            Some(Err(CapabilityErrorKind::Other)),
            CapabilityErrorKind::Other,
        ),
        (
            Some(Ok(ExtensionScript {
                outcome: Err(CapabilityErrorKind::Other),
                internal: Vec::new(),
                external: Vec::new(),
            })),
            CapabilityErrorKind::Other,
        ),
    ] {
        let caps = Mock {
            extension: script,
            ..Mock::default()
        };
        let mut session = Session::started(extension_machine(), caps);
        let failure = session
            .send(&external("go"))
            .expect_err("the session fails closed");
        assert_eq!(failure.error.kind, kind);
    }
}

#[test]
fn a_session_loads_its_extension_namespaces_before_data_binding() {
    let ir = machine(
        r#"initial="s" xmlns:x="https://example.org/x" xmlns:y="https://example.org/y""#,
        r#"<datamodel><data id="d" expr="1"/></datamodel>
           <state id="s"><transition event="go"><y:b/><x:a/></transition></state>"#,
    );
    let mut caps = Mock::default();
    caps.values.insert("1".into(), Ok(text("1")));
    let session = Session::started(ir.clone(), caps);
    let load = session
        .caps
        .calls
        .iter()
        .position(|call| call.starts_with("load-extensions"))
        .expect("the session loads its namespaces");
    assert_eq!(
        session.caps.calls[load], "load-extensions https://example.org/x https://example.org/y",
        "once, with the sorted exact URIs"
    );
    assert_eq!(load, 0, "before data binding and the first macrostep");

    let mut session = Session::new(
        ir,
        Mock {
            load_failure: Some(CapabilityErrorKind::Unavailable),
            ..Mock::default()
        },
    );
    let failure = session
        .start()
        .expect_err("a namespace that cannot load fails the start closed");
    assert_eq!(failure.error.kind, CapabilityErrorKind::Unavailable);
    assert_eq!(
        session.caps.calls,
        ["load-extensions https://example.org/x https://example.org/y"],
        "no data is bound and nothing executes"
    );

    let plain = Session::started(
        machine(r#"initial="s""#, r#"<state id="s"/>"#),
        Mock::default(),
    );
    assert!(
        !plain
            .caps
            .calls
            .iter()
            .any(|call| call.starts_with("load-extensions")),
        "a document without extension namespaces loads nothing"
    );
}

/// The executable-content image of an executable-content extension in the entry
/// content of a compound state: the configuration is the one `In()` observes
/// there (the compound state entered, its child not yet), and the actions walk
/// starts at that innermost state, so its own transitions are listed.
#[test]
fn the_executable_content_image_reflects_the_configuration_the_effect_observes() {
    let ir = machine(
        r#"initial="idle" xmlns:x="https://example.org/x""#,
        r#"<state id="idle"><transition event="go" target="outer"/></state>
           <state id="outer">
             <onentry><x:act/></onentry>
             <transition event="answer.yes" target="done"/>
             <transition event="answer.no answer.maybe" target="done" cond="true"/>
             <state id="inner"><transition event="inner.only" target="done"/></state>
           </state>
           <final id="done"/>"#,
    );
    let caps = Mock {
        extension: Some(Ok(ExtensionScript {
            outcome: Ok(true),
            internal: Vec::new(),
            external: Vec::new(),
        })),
        snapshots: Some(Vec::new()),
        ..Mock::default()
    };
    let mut session = Session::started(ir, caps);
    let _ = session.send(&external("go"));
    assert_eq!(session.active(), ["inner"]);
    let images = session.caps.snapshots.as_deref().unwrap_or_default();
    let [(image, send_counter)] = images else {
        panic!("one executable-content image: {images:?}");
    };
    let ids = |states: &[scxml_types::StateId]| -> Vec<String> {
        states.iter().map(|id| id.as_str().to_owned()).collect()
    };
    assert_eq!(ids(&image.configuration), ["outer"]);
    assert!(image.atomic.is_empty(), "a compound state is not atomic");
    let listed: Vec<(String, Vec<String>, bool)> = image
        .transitions
        .iter()
        .map(|transition| {
            assert!(transition.exit.is_empty() && transition.entry.is_empty());
            (
                transition.source.as_str().to_owned(),
                transition.events.clone(),
                transition.cond.is_some(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("outer".to_owned(), vec!["answer.yes".to_owned()], false),
            (
                "outer".to_owned(),
                vec!["answer.no".to_owned(), "answer.maybe".to_owned()],
                true
            ),
        ],
        "the walk starts at the innermost observed state; `idle` was exited"
    );
    assert_eq!(*send_counter, 0);
}

/// The image of an executable-content extension in an atomic state's entry
/// content lists that state as atomic and walks its ancestors; an extension
/// component that does not read the snapshot gets no image (and the engine
/// computes none).
#[test]
fn the_executable_content_image_is_computed_only_for_a_extension_that_reads_it() {
    let ir = machine(
        r#"initial="p" xmlns:x="https://example.org/x""#,
        r#"<state id="p">
             <transition event="leave" target="done"/>
             <state id="ask">
               <onentry><x:act/></onentry>
               <transition event="reply.*" target="done"/>
             </state>
           </state>
           <final id="done"/>"#,
    );
    for wants_snapshot in [true, false] {
        let caps = Mock {
            extension: Some(Ok(ExtensionScript {
                outcome: Ok(true),
                internal: Vec::new(),
                external: Vec::new(),
            })),
            snapshots: wants_snapshot.then(Vec::new),
            ..Mock::default()
        };
        let session = Session::started(ir.clone(), caps);
        assert_eq!(session.caps.executed.len(), 1);
        let Some(images) = session.caps.snapshots.as_deref() else {
            assert!(!wants_snapshot);
            continue;
        };
        let image = &images[0].0;
        let ids = |states: &[scxml_types::StateId]| -> Vec<String> {
            states.iter().map(|id| id.as_str().to_owned()).collect()
        };
        assert_eq!(ids(&image.configuration), ["p", "ask"]);
        assert_eq!(ids(&image.atomic), ["ask"]);
        let sources: Vec<&str> = image
            .transitions
            .iter()
            .map(|transition| transition.source.as_str())
            .collect();
        assert_eq!(sources, ["ask", "p"]);
    }
}

/// B.2.1: "Otherwise the Processor MUST treat the content as a
/// space-normalized string literal". Space normalization is `XPath` 1.0
/// `normalize-space()` over XML white space ("the same as those allowed by
/// the S production in XML": space, tab, carriage return, line feed), the
/// definition the `XPath` data model uses: other characters, such as U+00A0
/// NO-BREAK SPACE, are content.
#[test]
fn text_data_is_space_normalized_over_xml_white_space() {
    let ir = machine(
        r#"initial="s""#,
        "<datamodel><data id=\"a\">\t one\u{a0} two \r\n three </data></datamodel><state id=\"s\"/>",
    );
    let session = Session::started(ir, Mock::default());
    let data: Vec<&String> = session
        .caps
        .calls
        .iter()
        .filter(|call| call.starts_with("data:"))
        .collect();
    // `Debug` escapes U+00A0 as `\u{a0}`.
    assert_eq!(data, [r#"data:a=Value(String("one\u{a0} two three"))"#]);
}

/// SCXML 3.14, of `<send>` and `<invoke>` ids: "in the case where the author
/// does not provide them, the processor MUST generate a new unique ID not at
/// load time but each time the element is executed." One `<send
/// idlocation>` executed three times by a `<foreach>`, and one `<invoke>`
/// executed on each entry of its state, get a new id every time.
#[test]
fn generated_ids_are_new_each_time_the_element_is_executed() {
    let mut caps = Mock {
        send_result: Some(Ok(SendDisposition::Routed)),
        ..Mock::default()
    };
    caps.foreach.insert(
        "items".into(),
        ForeachItems::Items(vec![text("a"), text("b"), text("c")]),
    );
    let session = Session::started(
        outcome_machine(
            r#"<foreach array="items" item="it"><send event="x" idlocation="loc"/></foreach>"#,
        ),
        caps,
    );
    let ids: Vec<Option<&str>> = session
        .caps
        .sends
        .iter()
        .map(|request| {
            request
                .event
                .send_id
                .as_ref()
                .map(scxml_types::SendId::as_str)
        })
        .collect();
    assert_eq!(ids, [Some("sendid_0"), Some("sendid_1"), Some("sendid_2")]);

    let mut caps = Mock::default();
    caps.documents
        .insert("file:child.scxml".into(), CHILD.into());
    let mut session = Session::started(
        machine(
            r#"initial="s""#,
            r#"<state id="s">
                 <invoke src="file:child.scxml" idlocation="where"/>
                 <transition event="again" target="s"/>
               </state>"#,
        ),
        caps,
    );
    session.send(&external("again")).expect("delivers");
    session.send(&external("again")).expect("delivers");
    let invoked: Vec<&str> = session
        .caps
        .invocations
        .iter()
        .map(|invocation| invocation.invoke_id.as_str())
        .collect();
    assert_eq!(invoked, ["s.0", "s.1", "s.2"]);
}

#[test]
fn a_service_invocation_names_its_env_slot_variables_never_values() {
    let env = scxml_types::ENV_NAMESPACE_URI;
    let mut caps = Mock::default();
    caps.bound_services.push(ECHO.into());
    caps.texts
        .insert("base".into(), Ok(Some("BASE_URL".into())));
    let session = Session::started(
        service_machine(&format!(
            r#"<invoke xmlns:env="{env}" type="{ECHO}" id="echo" env:token="TOKEN" env:baseurlexpr="base"/>"#
        )),
        caps,
    );
    assert_eq!(session.active(), ["s"]);
    let [request] = session.caps.services.as_slice() else {
        panic!("one service start: {:?}", session.caps.services);
    };
    let env_attribute = |slot: &str, name: &str| scxml_types::ExtensionAttribute {
        name: scxml_types::QualifiedName {
            namespace_uri: Some(scxml_types::NamespaceUri::new(env).expect("the env namespace")),
            local_name: slot.into(),
        },
        value: name.into(),
    };
    assert_eq!(
        request.attributes,
        [
            env_attribute("baseurl", "BASE_URL"),
            env_attribute("token", "TOKEN")
        ]
    );
    // An expression that fails fails the invocation.
    let mut caps = Mock::default();
    caps.bound_services.push(ECHO.into());
    let session = Session::started(
        service_machine(&format!(
            r#"<invoke xmlns:env="{env}" type="{ECHO}" env:tokenexpr="missing"/>"#
        )),
        caps,
    );
    assert_eq!(session.active(), ["exec"]);
    assert!(session.caps.services.is_empty());
    // A `typeexpr` evaluating to the SCXML type with slots bound fails closed.
    let mut caps = Mock::default();
    caps.texts.insert("t".into(), Ok(Some("scxml".into())));
    let session = Session::started(
        service_machine(&format!(
            r#"<invoke xmlns:env="{env}" typeexpr="t" src="c.scxml" env:token="TOKEN"/>"#
        )),
        caps,
    );
    assert_eq!(session.active(), ["exec"]);
}

// ------------------------------------------------------- event contracts

/// The `event` vocabulary's namespace declaration.
const EVENT_NS: &str = r#"xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event""#;

/// An outcome machine (`exec`, `comm`, `ok`) declaring the `event`
/// namespace, whose `ok` transition carries `handler` (an `event:schema`
/// attribute, or empty) and logs `after` once its `onentry` finishes.
fn contract_machine(onentry: &str, handler: &str, extra: &str) -> MachineIr {
    machine(
        &format!(r#"initial="s" {EVENT_NS}"#),
        &format!(
            r#"<state id="s">
                 <onentry>{onentry}<log label="after"/></onentry>
                 <transition event="error.execution" target="exec"/>
                 <transition event="error.communication" target="comm"/>
                 <transition event="ok" target="ok" {handler}/>
                 {extra}
               </state>
               <final id="exec"/><final id="comm"/><final id="ok"/>"#
        ),
    )
}

fn object(json: &str) -> DataValue {
    DataValue::canonical_json_object(json).expect("a JSON object")
}

fn number(value: f64) -> DataValue {
    DataValue::Number(scxml_types::FiniteNumber::new(value).expect("finite"))
}

/// Scripted values `good` (`{"n":1}`) and `bad` (`2`).
fn contract_caps() -> Mock {
    let mut caps = Mock::default();
    caps.values.insert("good".into(), Ok(object(r#"{"n":1}"#)));
    caps.values.insert("bad".into(), Ok(number(2.0)));
    caps.targets.insert("#_parent".into(), SendTarget::Parent);
    caps
}

const OBJECT_SCHEMA: &str = r#"event:schema='{"type":"object","required":["n"]}'"#;

#[test]
fn a_send_whose_data_its_own_schema_rejects_is_not_dispatched_and_raises_error_execution() {
    // Whatever the target: the session's own queue, `#_internal`, or a
    // routed one.
    for target in ["", r##"target="#_internal""##, r##"target="#_parent""##] {
        let session = Session::started(
            contract_machine(
                &format!(
                    r#"<send event="ok" {target} {OBJECT_SCHEMA}><content expr="bad"/></send>"#
                ),
                "",
                "",
            ),
            contract_caps(),
        );
        assert_eq!(session.active(), ["exec"], "{target}");
        assert!(
            session.caps.sends.is_empty(),
            "{target}: nothing is dispatched"
        );
        assert!(
            session.caps.logs.is_empty(),
            "{target}: error.execution ends the block: {:?}",
            session.caps.logs
        );
        let session = Session::started(
            contract_machine(
                &format!(
                    r#"<send event="ok" {target} {OBJECT_SCHEMA}><content expr="good"/></send>"#
                ),
                "",
                "",
            ),
            contract_caps(),
        );
        assert_eq!(session.caps.logs.len(), 1, "{target}: the block goes on");
        if target.contains("_parent") {
            assert_eq!(session.active(), ["s"], "routed away");
            assert_eq!(session.caps.sends.len(), 1);
        } else {
            assert_eq!(session.active(), ["ok"], "{target}");
        }
    }
}

#[test]
fn the_error_of_a_rejected_send_carries_its_send_id() {
    let ir = machine(
        &format!(r#"initial="s" {EVENT_NS}"#),
        &format!(
            r#"<state id="s">
                 <onentry><send id="mine" event="x" {OBJECT_SCHEMA}><content expr="bad"/></send></onentry>
                 <transition event="error.execution" cond="sendid==mine" target="pass"/>
                 <transition event="error" target="wrong"/>
               </state>
               <final id="pass"/><final id="wrong"/>"#
        ),
    );
    let session = Session::started(ir, contract_caps());
    assert_eq!(session.active(), ["pass"]);
}

#[test]
fn an_event_the_session_sends_itself_must_satisfy_every_transition_handling_it() {
    for target in ["", r##"target="#_internal""##, r##"target="#_scxml_self""##] {
        for (value, expected) in [("bad", "exec"), ("good", "ok")] {
            let session = Session::started(
                contract_machine(
                    &format!(r#"<send event="ok" {target}><content expr="{value}"/></send>"#),
                    OBJECT_SCHEMA,
                    "",
                ),
                contract_caps(),
            );
            assert_eq!(session.active(), [expected], "{target} {value}");
        }
    }
    // No data at all is `null`, which an object schema rejects.
    let session = Session::started(
        contract_machine(r#"<send event="ok"/>"#, OBJECT_SCHEMA, ""),
        contract_caps(),
    );
    assert_eq!(session.active(), ["exec"]);
}

#[test]
fn every_matching_descriptor_counts_in_any_state_and_other_events_do_not() {
    // A transition of a state that is not active, matching by prefix,
    // rejects the data; a transition handling another event does not.
    let elsewhere = format!(
        r#"<state id="other"><transition event="ok.*" target="ok" {OBJECT_SCHEMA}/></state>"#
    );
    let ir = machine(
        &format!(r#"initial="s" {EVENT_NS}"#),
        &format!(
            r#"<state id="s">
                 <onentry><send event="ok.sub"><content expr="bad"/></send></onentry>
                 <transition event="error.execution" target="exec"/>
                 <transition event="ok.sub" target="ok"/>
               </state>
               {elsewhere}
               <final id="exec"/><final id="ok"/>"#
        ),
    );
    assert_eq!(Session::started(ir, contract_caps()).active(), ["exec"]);
    let session = Session::started(
        contract_machine(
            r#"<send event="ok"><content expr="bad"/></send>"#,
            "",
            &format!(r#"<transition event="other" target="exec" {OBJECT_SCHEMA}/>"#),
        ),
        contract_caps(),
    );
    assert_eq!(session.active(), ["ok"]);
}

#[test]
fn a_routed_send_is_not_judged_by_this_documents_transitions() {
    let session = Session::started(
        contract_machine(
            r##"<send event="ok" target="#_parent"><content expr="bad"/></send>"##,
            OBJECT_SCHEMA,
            "",
        ),
        contract_caps(),
    );
    assert_eq!(session.active(), ["s"]);
    assert_eq!(session.caps.sends.len(), 1);
}

#[test]
fn a_raise_its_contracts_reject_is_not_raised_and_raises_error_execution() {
    // A `<raise>` has no data (`null`).
    for (raise, handler, expected) in [
        (r#"<raise event="ok"/>"#, OBJECT_SCHEMA, "exec"),
        (
            r#"<raise event="ok" event:schema='{"type":"object"}'/>"#,
            "",
            "exec",
        ),
        (
            r#"<raise event="ok" event:schema='{"type":"null"}'/>"#,
            r#"event:schema='{"type":["null","object"]}'"#,
            "ok",
        ),
        (r#"<raise event="ok"/>"#, "", "ok"),
    ] {
        let session = Session::started(contract_machine(raise, handler, ""), contract_caps());
        assert_eq!(session.active(), [expected], "{raise} {handler}");
        assert_eq!(
            session.caps.logs.len(),
            usize::from(expected == "ok"),
            "{raise}: a rejected raise ends its block"
        );
    }
}

#[test]
fn schemaexpr_constrains_nothing_and_is_never_evaluated() {
    let session = Session::started(
        contract_machine(
            r#"<send event="ok" event:schemaexpr="s1"><content expr="bad"/></send><raise event="x" event:schemaexpr="s2"/>"#,
            r#"event:schemaexpr="s3""#,
            "",
        ),
        contract_caps(),
    );
    assert_eq!(session.active(), ["ok"]);
    assert!(
        !session
            .caps
            .calls
            .iter()
            .any(|call| call.contains(":s1") || call.contains(":s2") || call.contains(":s3")),
        "{:?}",
        session.caps.calls
    );
}

#[test]
fn a_bare_done_event_is_never_checked() {
    let ir = machine(
        &format!(r#"initial="c" {EVENT_NS}"#),
        &format!(
            r#"<state id="c" initial="f">
                 <final id="f"/>
                 <transition event="done.state.c" target="ok" {OBJECT_SCHEMA}/>
                 <transition event="error.execution" target="exec"/>
               </state>
               <final id="exec"/><final id="ok"/>"#
        ),
    );
    assert_eq!(Session::started(ir, contract_caps()).active(), ["ok"]);
}

#[test]
fn transitions_still_match_external_events_by_name_alone() {
    // The receiving transition's schema is its declared contract; an event
    // that arrives with other data is still delivered and selects it.
    let ir = machine(
        &format!(r#"initial="s" {EVENT_NS}"#),
        &format!(
            r#"<state id="s"><transition event="go" target="done" {OBJECT_SCHEMA}/></state>
               <final id="done"/>"#
        ),
    );
    let mut session = Session::started(ir, contract_caps());
    let mut event = external("go");
    event.data = number(2.0);
    session.send(&event).expect("the event is processed");
    assert_eq!(session.active(), ["done"]);
}

#[test]
fn accepts_event_data_judges_an_arriving_event_by_every_transition_handling_it() {
    // What a host asks the receiving session before it routes another
    // session's event or delivers its own input (WIT
    // `session.accepts-event-data`): only the document's transitions
    // count, matched by descriptor in any state.
    let ir = machine(
        &format!(r#"initial="s" {EVENT_NS}"#),
        &format!(
            r#"<state id="s"><transition event="go" target="t" {OBJECT_SCHEMA}/></state>
               <state id="t"><transition event="go.*" event:schema='{{"type":"object","maxProperties":1}}'/>
                 <transition event="other" event:schema="false"/></state>"#
        ),
    );
    let mut engine = Engine::new(ir).expect("supported");
    assert!(engine.accepts_event_data("go", &object(r#"{"n":1}"#)));
    assert!(!engine.accepts_event_data("go", &number(2.0)));
    assert!(!engine.accepts_event_data("go.sub", &object(r#"{"n":1,"m":2}"#)));
    assert!(engine.accepts_event_data("go.sub", &object(r#"{"n":1}"#)));
    assert!(engine.accepts_event_data("elsewhere", &number(2.0)));
    assert!(!engine.accepts_event_data("other", &DataValue::Absent));
}
