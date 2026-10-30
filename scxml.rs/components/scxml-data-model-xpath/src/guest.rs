//! Component Model adapter for the `XPath` data model.
//!
//! Telemetry is emitted once per export through the versioned WIT facade;
//! conversions and validation helpers carry none.

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
    DataModelSnapshot, EffectFinding as WitEffectFinding, EffectFragment as WitEffectFragment,
    ExternalScripts as WitExternalScripts, Guest, SyntaxIssue as WitSyntaxIssue,
    SyntaxKind as WitSyntaxKind, SystemValues, Variable as WitVariable,
    VariableValue as WitVariableValue,
};
use stateforward::scxml::types::{
    AssignPlacement as WitAssignPlacement, AssignSource as WitAssignSource,
    BaggagePolicy as WitBaggagePolicy, ComponentError, DataInitializer as WitDataInitializer,
    DataValue as WitValue, ErrorCode, ErrorVersion, Event as WitEvent, EventType as WitEventType,
    ForeachResult, InlineBody, NamedValue, TraceContext as WitTraceContext,
};
use stateforward::scxml::{execution_host, telemetry};

use scxml_types::{
    AssignPlacement, AssignSource, BaggagePolicy, DataInitializer, DataValue, Event, EventName,
    EventType, FiniteNumber, FormValueSource, InvokeId, SendId, TraceContext,
};

use super::{
    DATA_MODEL_NAME, DataModelError, ExecutionHost, ForeachItems, SyntaxKind, SystemContext,
    VariableMarkup, XPathDataModel,
};

/// Most fragments one `check-effects` request may carry (WIT contract bound).
const MAX_EFFECT_FRAGMENTS: usize = 16_384;
/// Most source bytes one `check-effects` request may carry (WIT contract
/// bound).
const MAX_EFFECT_SOURCE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum issues returned by one `check-syntax` call (WIT contract bound).
const MAX_WIT_SYNTAX_ISSUES: usize = 4;

thread_local! {
    // The one data model of this component instance. The Component Model
    // instance is its single owner: a guest runs on one thread, and no state
    // crosses threads or instances. Each export borrows it for that call only.
    // An evaluation may call the `execution-host` import while it holds the
    // borrow; the Component Model forbids that import from re-entering this
    // instance, so no borrow ever overlaps. `initialize` replaces its state;
    // a failed call leaves it unchanged.
    static MODEL: RefCell<XPathDataModel> = RefCell::new(XPathDataModel::new());
}

struct Host;

impl ExecutionHost for Host {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        Ok(execution_host::in_state(state_id))
    }
}

struct Component;

impl Guest for Component {
    fn name() -> String {
        DATA_MODEL_NAME.into()
    }

    fn check_syntax(
        kind: WitSyntaxKind,
        source: String,
    ) -> Result<Vec<WitSyntaxIssue>, ComponentError> {
        let kind = match kind {
            WitSyntaxKind::Condition => SyntaxKind::Condition,
            WitSyntaxKind::Value => SyntaxKind::Value,
            WitSyntaxKind::Location => SyntaxKind::Location,
            WitSyntaxKind::Script => SyntaxKind::Script,
        };
        let result = {
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
        record(telemetry::Operation::CheckSyntax, outcome_of(&result), None);
        result
    }

    /// `XPath` 1.0 has no assignment operator, and its function library
    /// (`XPath` 1.0 §4, plus the data model's `In()`) only computes values,
    /// so no `XPath` condition or `<log expr>` has a side effect.
    fn check_effects(
        fragments: Vec<WitEffectFragment>,
        _scripts: WitExternalScripts,
    ) -> Result<Vec<WitEffectFinding>, ComponentError> {
        let bytes = fragments.iter().try_fold(0_usize, |total, fragment| {
            total.checked_add(fragment.source.len())
        });
        let result = if fragments.len() > MAX_EFFECT_FRAGMENTS
            || bytes.is_none_or(|bytes| bytes > MAX_EFFECT_SOURCE_BYTES)
        {
            Err(ComponentError {
                version: ErrorVersion::V1,
                code: ErrorCode::PayloadTooLarge,
                detail: Some("effect-check request exceeds its bound".into()),
            })
        } else {
            Ok(Vec::new())
        };
        record(
            telemetry::Operation::CheckEffects,
            outcome_of(&result),
            None,
        );
        result
    }

    fn initialize(declarations: Vec<String>, context: SystemValues) -> Result<(), ComponentError> {
        observed(telemetry::Operation::Initialize, None, || {
            let system = SystemContext {
                session_id: context.session_id,
                machine_name: context.machine_name,
                io_processors: from_wit_value(context.io_processors)?,
            };
            // The model also binds the W3C XPath template processor name.
            MODEL.with_borrow_mut(|model| {
                model.initialize_declarations(declarations.iter().map(String::as_str), system)
            })
        })
    }

    fn declare(id: String) -> Result<(), ComponentError> {
        observed(telemetry::Operation::Assign, None, || {
            MODEL.with_borrow_mut(|model| model.declare(&id))
        })
    }

    fn initialize_data(id: String, initializer: WitDataInitializer) -> Result<(), ComponentError> {
        observed(telemetry::Operation::Assign, None, || {
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
            let value = from_wit_value(value)?;
            MODEL.with_borrow_mut(|model| model.set_parameter(&name, &value))
        })
    }

    fn evaluate_location(
        location: String,
        event: Option<WitEvent>,
    ) -> Result<WitValue, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(
            telemetry::Operation::EvaluateValue,
            context.as_ref(),
            || {
                let event = event?;
                MODEL
                    .with_borrow_mut(|model| {
                        model.evaluate_location(&location, event.as_ref(), &mut Host)
                    })
                    .map(to_wit_value)
            },
        )
    }

    fn evaluate_text(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<Option<String>, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(
            telemetry::Operation::EvaluateValue,
            context.as_ref(),
            || {
                let event = event?;
                MODEL.with_borrow_mut(|model| {
                    model.evaluate_text(&expression, event.as_ref(), &mut Host)
                })
            },
        )
    }

    fn evaluate_log(expression: String, event: Option<WitEvent>) -> Result<String, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(
            telemetry::Operation::EvaluateValue,
            context.as_ref(),
            || {
                let event = event?;
                // SCXML 4.7 with the XPath data model: `string()` of the
                // value, the text the checked-in IRP baselines record.
                MODEL.with_borrow_mut(|model| {
                    model.evaluate_string(&expression, event.as_ref(), &mut Host)
                })
            },
        )
    }

    fn evaluate_form_value(
        source: String,
        location: bool,
        event: Option<WitEvent>,
    ) -> Result<WitValue, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(
            telemetry::Operation::EvaluateValue,
            context.as_ref(),
            || {
                let event = event?;
                // The WIT `location` flag is the wire form of the source kind.
                let kind = if location {
                    FormValueSource::Location
                } else {
                    FormValueSource::Expression
                };
                MODEL
                    .with_borrow_mut(|model| {
                        model.evaluate_form_value(&source, kind, event.as_ref(), &mut Host)
                    })
                    .map(to_wit_value)
            },
        )
    }

    // Housekeeping between machine calls: no telemetry observation.
    fn release_foreach() {
        MODEL.with_borrow_mut(XPathDataModel::release_foreach);
    }

    fn foreach_items(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<ForeachResult, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        let result = (|| {
            let event = event?;
            MODEL.with_borrow_mut(|model| {
                model.foreach_items(&expression, event.as_ref(), &mut Host)
            })
        })()
        .map_err(to_component_error)
        .map(to_wit_foreach);
        record(
            telemetry::Operation::EvaluateValue,
            outcome_of(&result),
            context.as_ref(),
        );
        result
    }

    // Pure: no data-model state, no telemetry of their own. `none` fails
    // closed: the caller raises `error.execution` rather than using a
    // fabricated value.
    fn namelist_key(location: String) -> Option<String> {
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

    fn accepts_event_data(data: WitValue) -> bool {
        from_wit_value(data).is_ok_and(|data| super::accepts_event_data(&data))
    }

    fn event_data_field(data: WitValue, name: String) -> Option<WitValue> {
        let data = from_wit_value(data).ok()?;
        super::event_data_field(&data, &name).map(to_wit_value)
    }

    fn external_data(text: String) -> Option<WitValue> {
        super::external_data(&text).map(to_wit_value)
    }

    fn evaluate_condition(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<bool, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(
            telemetry::Operation::EvaluateCondition,
            context.as_ref(),
            || {
                let event = event?;
                MODEL.with_borrow_mut(|model| {
                    model.evaluate_condition(&expression, event.as_ref(), &mut Host)
                })
            },
        )
    }

    fn evaluate_value(
        expression: String,
        event: Option<WitEvent>,
    ) -> Result<WitValue, ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(
            telemetry::Operation::EvaluateValue,
            context.as_ref(),
            || {
                let event = event?;
                MODEL
                    .with_borrow_mut(|model| {
                        model.evaluate_value(&expression, event.as_ref(), &mut Host)
                    })
                    .map(to_wit_value)
            },
        )
    }

    fn assign(
        location: String,
        source: WitAssignSource,
        placement: WitAssignPlacement,
        event: Option<WitEvent>,
        context: Option<WitTraceContext>,
    ) -> Result<(), ComponentError> {
        let context = context.map(validated_context).transpose();
        let trace = context.as_ref().ok().cloned().flatten();
        observed(telemetry::Operation::Assign, trace.as_ref(), || {
            context?;
            let event = event.map(from_wit_event).transpose()?;
            let placement = match placement {
                WitAssignPlacement::ReplaceChildren => AssignPlacement::ReplaceChildren,
                WitAssignPlacement::FirstChild => AssignPlacement::FirstChild,
                WitAssignPlacement::LastChild => AssignPlacement::LastChild,
                WitAssignPlacement::PreviousSibling => AssignPlacement::PreviousSibling,
                WitAssignPlacement::NextSibling => AssignPlacement::NextSibling,
                WitAssignPlacement::Replace => AssignPlacement::Replace,
                WitAssignPlacement::Delete => AssignPlacement::Delete,
                WitAssignPlacement::AddAttribute(name) => AssignPlacement::AddAttribute(name),
            };
            let source = match source {
                WitAssignSource::Expression(expression) => AssignSource::Expression(expression),
                WitAssignSource::Value(value) => AssignSource::Value(from_wit_value(value)?),
                WitAssignSource::ForeachValue(value) => {
                    AssignSource::ForeachValue(from_wit_value(value)?)
                }
            };
            // The data model owns the assignment semantics
            // (`XPathDataModel::apply_assign`), its one definition of
            // assignment; it is all-or-nothing.
            MODEL.with_borrow_mut(|model| {
                model.apply_assign(&location, &source, &placement, event.as_ref(), &mut Host)
            })
        })
    }

    fn execute(script: String, event: Option<WitEvent>) -> Result<(), ComponentError> {
        let event = event.map(from_wit_event).transpose();
        let context = trace_of(&event);
        observed(telemetry::Operation::Execute, context.as_ref(), || {
            event?;
            MODEL.with_borrow_mut(|model| model.execute(&script))
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

    // Read-only: serializes the committed variables, evaluates nothing, and
    // records no telemetry observation.
    fn snapshot() -> Result<DataModelSnapshot, ComponentError> {
        let image = MODEL
            .with_borrow(XPathDataModel::snapshot)
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
                    value: match variable.markup {
                        VariableMarkup::Markup(markup) => {
                            WitVariableValue::Value(WitValue::Xml(markup))
                        }
                        VariableMarkup::Oversized(bytes) => WitVariableValue::Oversized(bytes),
                    },
                })
                .collect(),
        })
    }
}

/// The WIT `foreach-result` of one node snapshot.
fn to_wit_foreach(items: ForeachItems) -> ForeachResult {
    match items {
        ForeachItems::Items(items) => {
            ForeachResult::Items(items.into_iter().map(to_wit_value).collect())
        }
        ForeachItems::Illegal => ForeachResult::Illegal,
    }
}

/// Converts the entries of one `event-payload` call; `None` when an entry
/// breaks the WIT `named-value` bounds.
fn from_wit_entries(entries: Vec<NamedValue>) -> Option<Vec<(String, DataValue)>> {
    entries
        .into_iter()
        .map(|entry| {
            if entry.name.is_empty() {
                return None;
            }
            Some((entry.name, from_wit_value(entry.value).ok()?))
        })
        .collect()
}

/// Runs one export body and records exactly one outcome and event for it.
fn observed<T>(
    operation: telemetry::Operation,
    context: Option<&WitTraceContext>,
    body: impl FnOnce() -> Result<T, DataModelError>,
) -> Result<T, ComponentError> {
    let result = body().map_err(to_component_error);
    record(operation, outcome_of(&result), context);
    result
}

fn record(
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

fn outcome_of<T>(result: &Result<T, ComponentError>) -> telemetry::Outcome {
    match result {
        Ok(_) => telemetry::Outcome::Ok,
        Err(error)
            if matches!(
                error.code,
                ErrorCode::InvalidInput
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
        detail: Some(detail.into()),
    }
}

fn from_wit_value(value: WitValue) -> Result<DataValue, DataModelError> {
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

/// Converts and fully validates one WIT event (`Event::validate` bounds
/// every field and the aggregate size).
fn from_wit_event(event: WitEvent) -> Result<Event, DataModelError> {
    let converted = Event {
        name: EventName::new(event.name).map_err(|_| DataModelError::InvalidInput)?,
        event_type: match event.event_type {
            WitEventType::Internal => EventType::Internal,
            WitEventType::External => EventType::External,
            WitEventType::Platform => EventType::Platform,
        },
        send_id: event
            .send_id
            .map(SendId::new)
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
        raw: event.raw,
        trace_context: event
            .trace_context
            .map(|context| {
                TraceContext::new(
                    context.traceparent,
                    context.tracestate,
                    context.baggage,
                    native_policy(context.baggage_policy),
                )
                .map_err(|_| DataModelError::InvalidInput)
            })
            .transpose()?,
    };
    converted
        .validate()
        .map_err(|_| DataModelError::InvalidInput)?;
    Ok(converted)
}

/// Validates one carrier and returns its canonical WIT form.
fn validated_context(context: WitTraceContext) -> Result<WitTraceContext, DataModelError> {
    TraceContext::new(
        context.traceparent,
        context.tracestate,
        context.baggage,
        native_policy(context.baggage_policy),
    )
    .map(|validated| wit_context(&validated))
    .map_err(|_| DataModelError::InvalidInput)
}

/// Trace carrier of a converted event (none for no event or a bad event).
fn trace_of(event: &Result<Option<Event>, DataModelError>) -> Option<WitTraceContext> {
    event
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .and_then(|event| event.trace_context.as_ref())
        .map(wit_context)
}

fn wit_context(context: &TraceContext) -> WitTraceContext {
    WitTraceContext {
        traceparent: context.traceparent.clone(),
        tracestate: context.tracestate.clone(),
        baggage: context.baggage.clone(),
        baggage_policy: match context.baggage_policy {
            BaggagePolicy::Drop => WitBaggagePolicy::Drop,
            BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
        },
    }
}

fn native_policy(policy: WitBaggagePolicy) -> BaggagePolicy {
    match policy {
        WitBaggagePolicy::Drop => BaggagePolicy::Drop,
        WitBaggagePolicy::Propagate => BaggagePolicy::Propagate,
    }
}

export!(Component);
