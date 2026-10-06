//! Component Model adapter for the ECMAScript data model.

#![allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]

use std::cell::RefCell;

wit_bindgen::generate!({
    world: "data-model",
    path: "../../crates/scxml-wit/wit/scxml",
});

use exports::stateforward::scxml::data_model_api::{
    DataModelSnapshot, EffectCause as WitEffectCause, EffectFinding as WitEffectFinding,
    EffectFragment as WitEffectFragment, EffectKind as WitEffectKind, EffectRole as WitEffectRole,
    EffectSpan as WitEffectSpan, ExternalScripts as WitExternalScripts, Guest,
    SyntaxIssue as WitSyntaxIssue, SyntaxKind as WitSyntaxKind, SystemValues,
    Variable as WitVariable, VariableText as WitVariableText, VariableValue as WitVariableValue,
};
use stateforward::scxml::types::{
    AssignPlacement as WitAssignPlacement, AssignSource as WitAssignSource,
    BaggagePolicy as WitBaggagePolicy, ComponentError, DataInitializer as WitDataInitializer,
    DataValue as WitValue, ErrorCode, ErrorVersion, Event as WitEvent, EventType as WitEventType,
    ForeachResult, InlineBody, NamedValue, TraceContext as WitTraceContext,
};
use stateforward::scxml::{execution_host, telemetry};

use super::{
    Answerer, DataModelError, EcmaScriptDataModel, ExecutionHost, ForeachItems, HostQuery,
    HostReply, MAX_AGGREGATE_BYTES, MAX_EXPRESSION_BYTES, MAX_FOREACH_ITEMS, MAX_SCRIPT_BYTES,
    MAX_SOURCE_BYTES, MAX_VALUE_BYTES, SystemContext, VariableValue,
};
use scxml_types::{
    AssignPlacement, AssignSource, BaggagePolicy, DataInitializer, DataValue, Event, EventName,
    EventType, FiniteNumber, FormValueSource, InvokeId, TraceContext,
};

/// Maximum issues returned by one `check-syntax` call (WIT contract bound).
const MAX_WIT_SYNTAX_ISSUES: usize = 4;
/// Maximum entries in one `named-value` list (WIT contract bound).
const MAX_NAMED_VALUES: usize = 1_024;

thread_local! {
    // The one data model of this component instance. The Component Model
    // instance is its single owner: a guest runs on one thread, and no state
    // crosses threads or instances. Each export borrows it for that call only.
    // An evaluation may call the `execution-host` import while it holds the
    // borrow; the Component Model forbids that import from re-entering this
    // instance, so no borrow ever overlaps. `initialize` replaces its state;
    // a failed call leaves it unchanged.
    static MODEL: RefCell<EcmaScriptDataModel> = RefCell::new(EcmaScriptDataModel::new());
}

struct Host;

impl ExecutionHost for Host {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        Ok(execution_host::in_state(state_id))
    }

    fn now_ms(&mut self) -> Result<u64, DataModelError> {
        Ok(execution_host::now_ms())
    }

    // The realm runs in place: it asks the imports directly.
    fn answerer(&mut self) -> Option<Answerer> {
        Some(Box::new(|query: &HostQuery| {
            Ok(match query {
                HostQuery::InState(id) => HostReply::InState(execution_host::in_state(id)),
                HostQuery::NowMs => HostReply::NowMs(execution_host::now_ms()),
            })
        }))
    }
}

struct Component;

impl Guest for Component {
    fn name() -> String {
        "ecmascript".into()
    }

    fn check_syntax(
        kind: WitSyntaxKind,
        source: String,
    ) -> Result<Vec<WitSyntaxIssue>, ComponentError> {
        let kind = match kind {
            WitSyntaxKind::Condition => super::SyntaxKind::Condition,
            WitSyntaxKind::Value => super::SyntaxKind::Value,
            WitSyntaxKind::Location => super::SyntaxKind::Location,
            WitSyntaxKind::Script => super::SyntaxKind::Script,
        };
        let limit = if kind == super::SyntaxKind::Script {
            MAX_SCRIPT_BYTES
        } else {
            MAX_EXPRESSION_BYTES
        };
        let result = if source.len() > limit {
            Err(ComponentError {
                version: ErrorVersion::V1,
                code: ErrorCode::PayloadTooLarge,
                detail: Some("syntax-check source exceeds its byte bound".into()),
            })
        } else {
            super::check_syntax(kind, &source)
                .map_err(to_component_error)
                .and_then(|issues| {
                    issues
                        .into_iter()
                        .take(MAX_WIT_SYNTAX_ISSUES)
                        .map(|issue| {
                            let offset = |value: usize| {
                                u32::try_from(value).map_err(|_| ComponentError {
                                    version: ErrorVersion::V1,
                                    code: ErrorCode::PayloadTooLarge,
                                    detail: Some("syntax issue offset exceeds u32".into()),
                                })
                            };
                            Ok(WitSyntaxIssue {
                                start: offset(issue.start)?,
                                end: offset(issue.end)?,
                                message: issue.message,
                            })
                        })
                        .collect()
                })
        };
        record_entry(
            telemetry::Operation::CheckSyntax,
            classify_component_result(&result),
        );
        result
    }

    fn check_effects(
        fragments: Vec<WitEffectFragment>,
        scripts: WitExternalScripts,
    ) -> Result<Vec<WitEffectFinding>, ComponentError> {
        let result = effects_entry(&fragments, scripts);
        record_entry(
            telemetry::Operation::CheckEffects,
            classify_component_result(&result),
        );
        result
    }

    fn initialize(declarations: Vec<String>, context: SystemValues) -> Result<(), ComponentError> {
        observed(telemetry::Operation::Initialize, None, || {
            validate_total_len(declarations.iter().map(String::len), MAX_SOURCE_BYTES)?;
            validate_wit_system(&context)?;
            MODEL.with_borrow_mut(|model| {
                model.initialize_declarations(
                    declarations.iter().map(String::as_str),
                    SystemContext {
                        session_id: context.session_id,
                        machine_name: context.machine_name,
                        io_processors: from_wit_value(context.io_processors)?,
                    },
                )
            })
        })
    }

    fn declare(id: String) -> Result<(), ComponentError> {
        observed(telemetry::Operation::Assign, None, || {
            validate_wit_text(&id, MAX_EXPRESSION_BYTES)?;
            MODEL.with_borrow_mut(|model| model.declare(&id))
        })
    }

    fn initialize_data(id: String, initializer: WitDataInitializer) -> Result<(), ComponentError> {
        let validated = (|| {
            validate_wit_text(&id, MAX_EXPRESSION_BYTES)?;
            match &initializer {
                WitDataInitializer::Expression(expression) => {
                    validate_wit_text(expression, MAX_EXPRESSION_BYTES)?;
                    validate_total([id.len(), expression.len()])
                }
                WitDataInitializer::Value(value) => {
                    validate_wit_value(value)?;
                    validate_total([id.len(), wit_value_bytes(value)])
                }
            }
        })();
        observed(telemetry::Operation::Assign, None, || {
            validated?;
            let initializer = match initializer {
                WitDataInitializer::Expression(expression) => {
                    DataInitializer::Expression(expression)
                }
                WitDataInitializer::Value(value) => DataInitializer::Value(from_wit_value(value)?),
            };
            MODEL.with_borrow_mut(|model| model.initialize_data(&id, &initializer, &mut Host))
        })
    }

    fn set_parameter(name: String, value: WitValue) -> Result<bool, ComponentError> {
        observed(telemetry::Operation::Assign, None, || {
            validate_wit_text(&name, MAX_EXPRESSION_BYTES)?;
            validate_total([name.len(), wit_value_bytes(&value)])?;
            let value = from_wit_value(value)?;
            MODEL.with_borrow_mut(|model| model.set_parameter(&name, &value))
        })
    }

    fn evaluate_location(
        location: String,
        event: Option<WitEvent>,
    ) -> Result<WitValue, ComponentError> {
        let context = validate_input_and_event(&location, MAX_EXPRESSION_BYTES, event.as_ref());
        observed_context(telemetry::Operation::EvaluateValue, context, || {
            let event = event.map(from_wit_event).transpose()?;
            MODEL
                .with_borrow(|model| model.evaluate_location(&location, event.as_ref(), &mut Host))
                .map(to_wit_value)
        })
    }

    fn evaluate_text(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<Option<String>, ComponentError> {
        let context = validate_input_and_event(&expression, MAX_EXPRESSION_BYTES, event.as_ref());
        observed_context(telemetry::Operation::EvaluateValue, context, || {
            let event = event.map(from_wit_event).transpose()?;
            MODEL.with_borrow(|model| model.evaluate_text(&expression, event.as_ref(), &mut Host))
        })
    }

    fn evaluate_log(expression: String, event: Option<WitEvent>) -> Result<String, ComponentError> {
        let context = validate_input_and_event(&expression, MAX_EXPRESSION_BYTES, event.as_ref());
        observed_context(telemetry::Operation::EvaluateValue, context, || {
            // SCXML 4.7: the deterministic ECMAScript log rendering of
            // `<log expr>`, the text the checked-in IRP baselines record.
            match event.map(from_wit_event).transpose()? {
                Some(event) => {
                    MODEL.with_borrow(|model| model.evaluate_log(&expression, &event, &mut Host))
                }
                None => MODEL
                    .with_borrow(|model| model.evaluate_log_without_event(&expression, &mut Host)),
            }
        })
    }

    fn evaluate_form_value(
        source: String,
        location: bool,
        event: Option<WitEvent>,
    ) -> Result<WitValue, ComponentError> {
        let context = validate_input_and_event(&source, MAX_EXPRESSION_BYTES, event.as_ref());
        observed_context(telemetry::Operation::EvaluateValue, context, || {
            let event = event.map(from_wit_event).transpose()?;
            // The WIT `location` flag is the wire form of the source kind.
            let kind = if location {
                FormValueSource::Location
            } else {
                FormValueSource::Expression
            };
            MODEL
                .with_borrow(|model| {
                    model.evaluate_form_value(&source, kind, event.as_ref(), &mut Host)
                })
                .map(to_wit_value)
        })
    }

    fn foreach_items(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<ForeachResult, ComponentError> {
        let context = validate_input_and_event(&expression, MAX_EXPRESSION_BYTES, event.as_ref());
        let trace = context.as_ref().ok().cloned().flatten();
        let result = context
            .and_then(|_| {
                let event = event.map(from_wit_event).transpose()?;
                MODEL.with_borrow(|model| {
                    model.foreach_items(&expression, event.as_ref(), &mut Host)
                })
            })
            .map_err(to_component_error)
            .and_then(to_wit_foreach);
        record_context(
            telemetry::Operation::EvaluateValue,
            classify_component_result(&result),
            trace.as_ref(),
        );
        result
    }

    // Pure: no data-model state, no telemetry of their own. `none` fails
    // closed: the caller raises `error.execution` rather than using a
    // fabricated value.
    fn namelist_key(location: String) -> Option<String> {
        if location.len() > MAX_EXPRESSION_BYTES {
            return None;
        }
        super::namelist_key(&location).map(str::to_owned)
    }

    fn event_payload(entries: Vec<NamedValue>) -> Option<WitValue> {
        from_wit_entries(entries)
            .and_then(|entries| super::event_payload(&entries))
            .map(to_wit_value)
    }

    fn inline_content(content: InlineBody) -> Option<WitValue> {
        super::inline_content(match &content {
            InlineBody::Text(text) => super::InlineContent::Text(text),
            InlineBody::Markup(markup) => super::InlineContent::Markup(markup),
        })
        .map(to_wit_value)
    }

    fn external_data(text: String) -> Option<WitValue> {
        super::external_data(&text).map(to_wit_value)
    }

    // Every portable value has an ECMAScript form (B.2).
    fn accepts_event_data(data: WitValue) -> bool {
        from_wit_value(data).is_ok()
    }

    fn event_data_field(data: WitValue, name: String) -> Option<WitValue> {
        let data = from_wit_value(data).ok()?;
        super::event_data_field(&data, &name).map(to_wit_value)
    }

    fn evaluate_condition(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<bool, ComponentError> {
        let context = validate_input_and_event(&expression, MAX_EXPRESSION_BYTES, event.as_ref());
        observed_context(telemetry::Operation::EvaluateCondition, context, || {
            // `none`: SCXML defines no current event, so `_event` stays
            // unbound; never fabricate one.
            match event.map(from_wit_event).transpose()? {
                Some(event) => MODEL
                    .with_borrow(|model| model.evaluate_condition(&expression, &event, &mut Host)),
                None => MODEL.with_borrow(|model| {
                    model.evaluate_condition_without_event(&expression, &mut Host)
                }),
            }
        })
    }

    fn evaluate_value(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<WitValue, ComponentError> {
        let context = validate_input_and_event(&expression, MAX_EXPRESSION_BYTES, event.as_ref());
        observed_context(telemetry::Operation::EvaluateValue, context, || {
            let value = match event.map(from_wit_event).transpose()? {
                Some(event) => {
                    MODEL.with_borrow(|model| model.evaluate_value(&expression, &event, &mut Host))
                }
                None => MODEL.with_borrow(|model| {
                    model.evaluate_value_without_event(&expression, &mut Host)
                }),
            };
            value.map(to_wit_value)
        })
    }

    fn assign(
        location: String,
        source: WitAssignSource,
        placement: WitAssignPlacement,
        event: Option<WitEvent>,
        context: Option<WitTraceContext>,
    ) -> Result<(), ComponentError> {
        let attribute_bytes = match &placement {
            WitAssignPlacement::AddAttribute(name) => name.len(),
            _ => 0,
        };
        let validated_context = (|| {
            validate_wit_text(&location, MAX_EXPRESSION_BYTES)?;
            validate_wit_text_len(attribute_bytes, MAX_EXPRESSION_BYTES)?;
            let source_bytes = match &source {
                WitAssignSource::Expression(expression) => {
                    validate_wit_text(expression, MAX_EXPRESSION_BYTES)?;
                    expression.len()
                }
                WitAssignSource::Value(value) | WitAssignSource::ForeachValue(value) => {
                    validate_wit_value(value)?;
                    wit_value_bytes(value)
                }
            };
            let event_bytes = match &event {
                Some(event) => {
                    validate_wit_event_fields(event)?;
                    wit_event_bytes(event)?
                }
                None => 0,
            };
            validate_total([location.len(), attribute_bytes, source_bytes, event_bytes])?;
            validate_wit_context(context.as_ref())
        })();
        observed_context(telemetry::Operation::Assign, validated_context, || {
            // The data model owns the assignment semantics
            // (`EcmaScriptDataModel::apply_assign`), its one definition of
            // assignment.
            let source = match source {
                WitAssignSource::Expression(expression) => AssignSource::Expression(expression),
                WitAssignSource::Value(value) => AssignSource::Value(from_wit_value(value)?),
                WitAssignSource::ForeachValue(value) => {
                    AssignSource::ForeachValue(from_wit_value(value)?)
                }
            };
            let placement = from_wit_placement(placement);
            let event = event.map(from_wit_event).transpose()?;
            MODEL.with_borrow_mut(|model| {
                model.apply_assign(&location, &source, &placement, event.as_ref(), &mut Host)
            })
        })
    }

    fn execute(script: String, event: Option<WitEvent>) -> Result<(), ComponentError> {
        let context = validate_input_and_event(&script, MAX_SCRIPT_BYTES, event.as_ref());
        observed_context(telemetry::Operation::Execute, context, || {
            match event.map(from_wit_event).transpose()? {
                Some(event) => {
                    MODEL.with_borrow_mut(|model| model.execute(&script, &event, &mut Host))
                }
                None => {
                    MODEL.with_borrow_mut(|model| model.execute_without_event(&script, &mut Host))
                }
            }
        })
    }

    fn system_context() -> Result<SystemValues, ComponentError> {
        observed(telemetry::Operation::SystemContext, None, || {
            MODEL.with_borrow(|model| {
                let context = model.system_context()?;
                Ok(SystemValues {
                    session_id: context.session_id,
                    machine_name: context.machine_name,
                    io_processors: to_wit_value(context.io_processors),
                })
            })
        })
    }

    // Housekeeping between machine calls: no telemetry observation. Before
    // initialization there is no loop to forget.
    fn release_foreach() {
        MODEL.with_borrow(|model| model.release_foreach().unwrap_or_default());
    }

    // Read-only: reads each declared id's value from the realm without
    // running user code, evaluates nothing, and records no telemetry
    // observation.
    fn snapshot() -> Result<DataModelSnapshot, ComponentError> {
        let image = MODEL
            .with_borrow(EcmaScriptDataModel::snapshot)
            .map_err(to_component_error)?;
        Ok(DataModelSnapshot {
            system: SystemValues {
                session_id: image.system.session_id,
                machine_name: image.system.machine_name,
                io_processors: to_wit_value(image.system.io_processors),
            },
            variables: image
                .variables
                .into_iter()
                .map(|variable| WitVariable {
                    name: variable.name,
                    value: match variable.value {
                        VariableValue::Value(value) => WitVariableValue::Value(to_wit_value(value)),
                        VariableValue::Text { kind, text } => {
                            WitVariableValue::Text(WitVariableText { kind, text })
                        }
                        VariableValue::Oversized(bytes) => WitVariableValue::Oversized(bytes),
                    },
                })
                .collect(),
        })
    }
}

fn observed<T>(
    operation: telemetry::Operation,
    context: Option<&WitTraceContext>,
    body: impl FnOnce() -> Result<T, DataModelError>,
) -> Result<T, ComponentError> {
    let result = body();
    let outcome = match &result {
        Ok(_) => telemetry::Outcome::Ok,
        Err(DataModelError::InvalidInput) => telemetry::Outcome::Rejected,
        Err(_) => telemetry::Outcome::Failed,
    };
    telemetry::observe(operation, outcome, None, context);
    telemetry::event(
        operation,
        if result.is_ok() {
            telemetry::SignalEvent::TransformationCompleted
        } else {
            telemetry::SignalEvent::InputRejected
        },
        context,
    );
    result.map_err(to_component_error)
}

/// The WIT `foreach-result` of one item snapshot. More than
/// [`MAX_FOREACH_ITEMS`] items (WIT contract bound) is `payload-too-large`.
fn to_wit_foreach(items: ForeachItems) -> Result<ForeachResult, ComponentError> {
    match items {
        ForeachItems::Items(items) if items.len() > MAX_FOREACH_ITEMS => Err(too_many_items()),
        ForeachItems::TooLarge => Err(too_many_items()),
        ForeachItems::Items(items) => Ok(ForeachResult::Items(
            items.into_iter().map(to_wit_value).collect(),
        )),
        ForeachItems::Illegal => Ok(ForeachResult::Illegal),
    }
}

fn too_many_items() -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code: ErrorCode::PayloadTooLarge,
        detail: Some("foreach collection exceeds 10000 items".into()),
    }
}

/// Converts the entries of one `event-payload` call; `None` when the list
/// or an entry breaks the WIT `named-value` bounds.
fn from_wit_entries(entries: Vec<NamedValue>) -> Option<Vec<(String, DataValue)>> {
    if entries.len() > MAX_NAMED_VALUES {
        return None;
    }
    entries
        .into_iter()
        .map(|entry| {
            if entry.name.is_empty() || entry.name.len() > scxml_types::MAX_NAME_BYTES {
                return None;
            }
            Some((entry.name, from_wit_value(entry.value).ok()?))
        })
        .collect()
}

fn observed_context<T>(
    operation: telemetry::Operation,
    context: Result<Option<WitTraceContext>, DataModelError>,
    body: impl FnOnce() -> Result<T, DataModelError>,
) -> Result<T, ComponentError> {
    match context {
        Ok(context) => observed(operation, context.as_ref(), body),
        Err(error) => observed(operation, None, || Err(error)),
    }
}

fn to_component_error(error: DataModelError) -> ComponentError {
    let (code, detail) = match error {
        DataModelError::InvalidInput => (ErrorCode::InvalidInput, "data-model input is invalid"),
        DataModelError::Execution | DataModelError::UnsupportedValue => {
            (ErrorCode::Execution, "data-model execution failed")
        }
        DataModelError::NotInitialized => (ErrorCode::Unavailable, "data model is not initialized"),
        DataModelError::Host => (ErrorCode::Platform, "host capability failed"),
    };
    ComponentError {
        version: ErrorVersion::V1,
        code,
        detail: (detail.len() <= 160).then(|| detail.into()),
    }
}

/// The native mirror of one WIT `assign-placement`.
fn from_wit_placement(placement: WitAssignPlacement) -> AssignPlacement {
    match placement {
        WitAssignPlacement::ReplaceChildren => AssignPlacement::ReplaceChildren,
        WitAssignPlacement::FirstChild => AssignPlacement::FirstChild,
        WitAssignPlacement::LastChild => AssignPlacement::LastChild,
        WitAssignPlacement::PreviousSibling => AssignPlacement::PreviousSibling,
        WitAssignPlacement::NextSibling => AssignPlacement::NextSibling,
        WitAssignPlacement::Replace => AssignPlacement::Replace,
        WitAssignPlacement::Delete => AssignPlacement::Delete,
        WitAssignPlacement::AddAttribute(name) => AssignPlacement::AddAttribute(name),
    }
}

fn from_wit_value(value: WitValue) -> Result<DataValue, DataModelError> {
    validate_wit_value(&value)?;
    match value {
        WitValue::Absent => Ok(DataValue::Absent),
        WitValue::Undefined => Ok(DataValue::Undefined),
        WitValue::Null => Ok(DataValue::Null),
        WitValue::Boolean(value) => Ok(DataValue::Bool(value)),
        WitValue::Number(value) => FiniteNumber::new(value)
            .map(DataValue::Number)
            .map_err(|_| DataModelError::InvalidInput),
        WitValue::Text(value) => Ok(DataValue::String(value)),
        WitValue::JsonArray(value) => Ok(DataValue::JsonArray(value)),
        WitValue::JsonObject(value) => Ok(DataValue::JsonObject(value)),
        WitValue::Xml(value) => Ok(DataValue::Xml(value)),
    }
}

fn to_wit_value(value: DataValue) -> WitValue {
    match value {
        DataValue::Absent => WitValue::Absent,
        DataValue::Undefined => WitValue::Undefined,
        DataValue::Null => WitValue::Null,
        DataValue::Bool(value) => WitValue::Boolean(value),
        DataValue::Number(value) => WitValue::Number(value.get()),
        DataValue::String(value) => WitValue::Text(value),
        DataValue::JsonArray(value) => WitValue::JsonArray(value),
        DataValue::JsonObject(value) => WitValue::JsonObject(value),
        DataValue::Xml(value) => WitValue::Xml(value),
    }
}

fn from_wit_event(event: WitEvent) -> Result<Event, DataModelError> {
    validate_wit_event_fields(&event)?;
    Ok(Event {
        name: EventName::new(event.name).map_err(|_| DataModelError::InvalidInput)?,
        event_type: match event.event_type {
            WitEventType::Internal => EventType::Internal,
            WitEventType::External => EventType::External,
            WitEventType::Platform => EventType::Platform,
        },
        send_id: event
            .send_id
            .map(scxml_types::SendId::new)
            .transpose()
            .map_err(|_| DataModelError::InvalidInput)?,
        origin: event.origin,
        origin_type: event.origin_type,
        invoke_id: event
            .invoke_id
            .map(InvokeId::new)
            .transpose()
            .map_err(|_| DataModelError::InvalidInput)?,
        data: from_wit_value(event.data)?,
        raw: event.raw.clone(),
        trace_context: event
            .trace_context
            .map(|context| {
                TraceContext::new(
                    context.traceparent,
                    context.tracestate,
                    context.baggage,
                    match context.baggage_policy {
                        WitBaggagePolicy::Drop => BaggagePolicy::Drop,
                        WitBaggagePolicy::Propagate => BaggagePolicy::Propagate,
                    },
                )
                .map_err(|_| DataModelError::InvalidInput)
            })
            .transpose()?,
    })
}

fn validate_wit_context(
    context: Option<&WitTraceContext>,
) -> Result<Option<WitTraceContext>, DataModelError> {
    context
        .map(|context| {
            validate_wit_trace_context_fields(context)?;
            TraceContext::new(
                context.traceparent.clone(),
                context.tracestate.clone(),
                context.baggage.clone(),
                match context.baggage_policy {
                    WitBaggagePolicy::Drop => BaggagePolicy::Drop,
                    WitBaggagePolicy::Propagate => BaggagePolicy::Propagate,
                },
            )
            .map_err(|_| DataModelError::InvalidInput)
            .map(|validated| WitTraceContext {
                traceparent: validated.traceparent,
                tracestate: validated.tracestate,
                baggage: validated.baggage,
                baggage_policy: match validated.baggage_policy {
                    BaggagePolicy::Drop => WitBaggagePolicy::Drop,
                    BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
                },
            })
        })
        .transpose()
}

fn validate_wit_text(value: &str, max: usize) -> Result<(), DataModelError> {
    validate_wit_text_len(value.len(), max)
}

fn validate_wit_text_len(len: usize, max: usize) -> Result<(), DataModelError> {
    if len <= max {
        Ok(())
    } else {
        Err(DataModelError::InvalidInput)
    }
}

fn validate_wit_value(value: &WitValue) -> Result<(), DataModelError> {
    match value {
        WitValue::Text(value)
        | WitValue::JsonArray(value)
        | WitValue::JsonObject(value)
        | WitValue::Xml(value) => validate_wit_text(value, MAX_VALUE_BYTES),
        WitValue::Number(value) if !value.is_finite() => Err(DataModelError::InvalidInput),
        WitValue::Absent
        | WitValue::Undefined
        | WitValue::Null
        | WitValue::Boolean(_)
        | WitValue::Number(_) => Ok(()),
    }
}

fn wit_value_bytes(value: &WitValue) -> usize {
    match value {
        WitValue::Text(value)
        | WitValue::JsonArray(value)
        | WitValue::JsonObject(value)
        | WitValue::Xml(value) => value.len(),
        WitValue::Absent
        | WitValue::Undefined
        | WitValue::Null
        | WitValue::Boolean(_)
        | WitValue::Number(_) => 8,
    }
}

fn validate_wit_system(value: &SystemValues) -> Result<(), DataModelError> {
    if value.session_id.is_empty() || value.machine_name.as_ref().is_some_and(String::is_empty) {
        return Err(DataModelError::InvalidInput);
    }
    validate_wit_text(&value.session_id, MAX_VALUE_BYTES)?;
    if let Some(name) = &value.machine_name {
        validate_wit_text(name, MAX_VALUE_BYTES)?;
    }
    validate_wit_value(&value.io_processors)?;
    if !matches!(value.io_processors, WitValue::JsonObject(_)) {
        return Err(DataModelError::InvalidInput);
    }
    validate_total([
        value.session_id.len(),
        value.machine_name.as_ref().map_or(0, String::len),
        wit_value_bytes(&value.io_processors),
    ])
}

fn validate_wit_trace_context_fields(value: &WitTraceContext) -> Result<(), DataModelError> {
    validate_wit_text(&value.traceparent, scxml_types::TRACEPARENT_BYTES)?;
    if let Some(value) = value.tracestate.as_deref() {
        validate_wit_text(value, scxml_types::MAX_TRACESTATE_BYTES)?;
    }
    if let Some(value) = value.baggage.as_deref() {
        validate_wit_text(value, scxml_types::MAX_BAGGAGE_BYTES)?;
    }
    Ok(())
}

fn validate_wit_event_fields(event: &WitEvent) -> Result<(), DataModelError> {
    validate_wit_text(&event.name, scxml_types::MAX_NAME_BYTES)?;
    if let Some(value) = event.send_id.as_deref() {
        validate_wit_text(value, scxml_types::MAX_NAME_BYTES)?;
    }
    if let Some(value) = event.origin.as_deref() {
        validate_wit_text(value, MAX_VALUE_BYTES)?;
    }
    if let Some(value) = event.origin_type.as_deref() {
        validate_wit_text(value, MAX_VALUE_BYTES)?;
    }
    if let Some(value) = event.invoke_id.as_deref() {
        validate_wit_text(value, scxml_types::MAX_NAME_BYTES)?;
    }
    validate_wit_value(&event.data)?;
    if let Some(value) = event.trace_context.as_ref() {
        validate_wit_trace_context_fields(value)?;
    }
    validate_total([wit_event_bytes(event)?])
}

fn wit_event_bytes(event: &WitEvent) -> Result<usize, DataModelError> {
    [
        event.name.len(),
        event.send_id.as_ref().map_or(0, String::len),
        event.origin.as_ref().map_or(0, String::len),
        event.origin_type.as_ref().map_or(0, String::len),
        event.invoke_id.as_ref().map_or(0, String::len),
        wit_value_bytes(&event.data),
        event.trace_context.as_ref().map_or(0, |context| {
            context.traceparent.len()
                + context.tracestate.as_ref().map_or(0, String::len)
                + context.baggage.as_ref().map_or(0, String::len)
        }),
    ]
    .into_iter()
    .try_fold(0_usize, |total, value| {
        total.checked_add(value).ok_or(DataModelError::InvalidInput)
    })
}

/// Bounds one expression or script and, when present, its `_event`;
/// returns the event's validated trace context.
fn validate_input_and_event(
    input: &str,
    max: usize,
    event: Option<&WitEvent>,
) -> Result<Option<WitTraceContext>, DataModelError> {
    validate_wit_text(input, max)?;
    match event {
        Some(event) => {
            validate_total([input.len(), wit_event_bytes(event)?])?;
            validate_wit_event(event)
        }
        None => validate_total([input.len()]).map(|()| None),
    }
}

fn validate_wit_event(event: &WitEvent) -> Result<Option<WitTraceContext>, DataModelError> {
    validate_wit_event_fields(event)
        .and_then(|()| validate_wit_context(event.trace_context.as_ref()))
}

/// Bounds the summed byte length of `values` by `max`.
fn validate_total_len(
    values: impl IntoIterator<Item = usize>,
    max: usize,
) -> Result<(), DataModelError> {
    let total = values.into_iter().try_fold(0_usize, |total, value| {
        total.checked_add(value).ok_or(DataModelError::InvalidInput)
    })?;
    validate_wit_text_len(total, max)
}

fn validate_total(values: impl IntoIterator<Item = usize>) -> Result<(), DataModelError> {
    values
        .into_iter()
        .try_fold(0_usize, |total, value| {
            total.checked_add(value).ok_or(DataModelError::InvalidInput)
        })
        .and_then(|total| {
            if scxml_types::exceeds_wasm_bound(total, MAX_AGGREGATE_BYTES) {
                Err(DataModelError::InvalidInput)
            } else {
                Ok(())
            }
        })
}

fn classify_component_result<T>(result: &Result<T, ComponentError>) -> telemetry::Outcome {
    match result {
        Ok(_) => telemetry::Outcome::Ok,
        Err(error)
            if matches!(
                error.code,
                ErrorCode::InvalidInput
                    | ErrorCode::DuplicateId
                    | ErrorCode::UnknownId
                    | ErrorCode::CapacityExhausted
                    | ErrorCode::PayloadTooLarge
                    | ErrorCode::UnsupportedVersion
                    | ErrorCode::Protocol
            ) =>
        {
            telemetry::Outcome::Rejected
        }
        Err(_) => telemetry::Outcome::Failed,
    }
}

/// `check-effects`: converts the request, analyzes, and converts the
/// findings (offsets checked into `u32`).
fn effects_entry(
    fragments: &[WitEffectFragment],
    scripts: WitExternalScripts,
) -> Result<Vec<WitEffectFinding>, ComponentError> {
    let too_large = || ComponentError {
        version: ErrorVersion::V1,
        code: ErrorCode::PayloadTooLarge,
        detail: Some("effect-check request exceeds its bound".into()),
    };
    let invalid = || ComponentError {
        version: ErrorVersion::V1,
        code: ErrorCode::InvalidInput,
        detail: Some("malformed effect-check fragment".into()),
    };
    let bytes = fragments.iter().try_fold(0_usize, |total, fragment| {
        total.checked_add(fragment.source.len())
    });
    if fragments.len() > super::effects::MAX_EFFECT_FRAGMENTS
        || bytes.is_none_or(|bytes| bytes > super::effects::MAX_EFFECT_SOURCE_BYTES)
    {
        return Err(too_large());
    }
    let mut request = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let role = match (fragment.role, fragment.name.as_deref()) {
            (WitEffectRole::Data, Some(name)) => super::EffectRole::Data(name),
            (WitEffectRole::Data, None) | (_, Some(_)) => return Err(invalid()),
            (WitEffectRole::Condition, None) => super::EffectRole::Condition,
            (WitEffectRole::Log, None) => super::EffectRole::Log,
            (WitEffectRole::Script, None) => super::EffectRole::Script,
            (WitEffectRole::Value, None) => super::EffectRole::Value,
            (WitEffectRole::Location, None) => super::EffectRole::Location,
        };
        request.push(super::EffectFragment {
            role,
            source: &fragment.source,
        });
    }
    let scripts = match scripts {
        WitExternalScripts::Absent => scxml_types::ExternalScripts::Absent,
        WitExternalScripts::Present => scxml_types::ExternalScripts::Present,
    };
    let findings = super::check_effects(&request, scripts).map_err(to_component_error)?;
    let offset = |value: usize| u32::try_from(value).map_err(|_| too_large());
    let span = |span: super::EffectSpan| {
        Ok(WitEffectSpan {
            fragment: offset(span.fragment)?,
            start: offset(span.start)?,
            end: offset(span.end)?,
        })
    };
    findings
        .into_iter()
        .map(|finding| {
            Ok(WitEffectFinding {
                certain: finding.certainty == super::Certainty::Certain,
                kind: wit_effect_kind(finding.kind),
                span: span(finding.span)?,
                subject: finding.subject,
                cause: finding
                    .cause
                    .map(|cause| {
                        Ok::<_, ComponentError>(WitEffectCause {
                            kind: wit_effect_kind(cause.kind),
                            span: span(cause.span)?,
                            subject: cause.subject,
                            function: cause.function,
                        })
                    })
                    .transpose()?,
                operator: finding.operator.map(span).transpose()?,
            })
        })
        .collect()
}

const fn wit_effect_kind(kind: super::EffectKind) -> WitEffectKind {
    match kind {
        super::EffectKind::Assignment => WitEffectKind::Assignment,
        super::EffectKind::Update => WitEffectKind::Update,
        super::EffectKind::Deletion => WitEffectKind::Deletion,
        super::EffectKind::MutatingCall => WitEffectKind::MutatingCall,
        super::EffectKind::Random => WitEffectKind::Random,
        super::EffectKind::FunctionCall => WitEffectKind::FunctionCall,
        super::EffectKind::UnknownCall => WitEffectKind::UnknownCall,
        super::EffectKind::DynamicCode => WitEffectKind::DynamicCode,
        super::EffectKind::ImplicitCall => WitEffectKind::ImplicitCall,
        super::EffectKind::UnknownWrite => WitEffectKind::UnknownWrite,
        super::EffectKind::DynamicScope => WitEffectKind::DynamicScope,
        super::EffectKind::UnseenScript => WitEffectKind::UnseenScript,
        super::EffectKind::TooComplex => WitEffectKind::TooComplex,
        super::EffectKind::SystemVariableWrite => WitEffectKind::SystemVariableWrite,
    }
}

/// Records the outcome of the `check-syntax` or `check-effects` WIT entry
/// point.
fn record_entry(operation: telemetry::Operation, outcome: telemetry::Outcome) {
    record_context(operation, outcome, None);
}

/// Records the outcome of one WIT entry point that converts its result
/// after the data-model call (`foreach-items`).
fn record_context(
    operation: telemetry::Operation,
    outcome: telemetry::Outcome,
    context: Option<&WitTraceContext>,
) {
    telemetry::observe(operation, outcome, None, context);
    telemetry::event(
        operation,
        if outcome == telemetry::Outcome::Ok {
            telemetry::SignalEvent::TransformationCompleted
        } else {
            telemetry::SignalEvent::InputRejected
        },
        context,
    );
}

export!(Component);
