//! Machine IR supply modes, the linked data-model capability, and a bounded
//! lifecycle driver for one representative machine session.
//!
//! The host stays a thin linker: it serves `ir-source` bytes it does not
//! parse, forwards data-model capability calls to a caller-supplied provider,
//! records `<log>` capability calls, and calls the machine's lifecycle
//! exports in order. Transition selection, macrostep/microstep semantics, IR
//! decoding, and IR validation all happen inside the machine component.

use crate::bindings::stateforward::scxml::types::{
    AssignPlacement as WitAssignPlacement, AssignSource as WitAssignSource,
    BaggagePolicy as WitBaggagePolicy, ComponentError, DataValue as WitDataValue, ErrorCode,
    ErrorVersion, Event as WitEvent, EventType as WitEventType,
};

/// Canonical name of the imported IR supply interface.
pub(crate) const IR_SOURCE_INTERFACE: &str = "stateforward:scxml/ir-source@0.2.0";

/// How the linked machine component obtains its lowered Machine IR.
#[derive(Clone, Copy, Debug)]
pub enum MachineIrSource<'a> {
    /// Packaged `component-wasm`: the IR is composed into the component,
    /// which must not import `ir-source`; the host serves nothing.
    Packaged,
    /// Runtime mode: the component is the generic machine, which must import
    /// `ir-source`; the host serves exactly these bytes, unbounded and
    /// unparsed (resource bounds apply to wasm builds only, so the wasm
    /// machine applies its own Machine IR byte bound). This is the only mode
    /// browser hosts use.
    Runtime(&'a [u8]),
}

impl MachineIrSource<'_> {
    /// Returns the owned bytes the host serves.
    pub(crate) fn served_bytes(self) -> Vec<u8> {
        match self {
            Self::Packaged => Vec::new(),
            Self::Runtime(bytes) => bytes.to_vec(),
        }
    }

    /// Whether a component linked in this mode must import `ir-source`.
    pub(crate) const fn requires_ir_source_import(self) -> bool {
        matches!(self, Self::Runtime(_))
    }
}

/// Closed failure a data-model provider returns to the machine.
///
/// `InvalidInput` and `Execution` are recoverable inside the machine
/// (SCXML `error.execution`); `Unavailable` and `Platform` fail the session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataModelFailure {
    /// The expression, location, or value was rejected.
    InvalidInput,
    /// Evaluation failed at run time.
    Execution,
    /// The operation is not provided.
    Unavailable,
    /// The provider itself failed.
    Platform,
}

/// Data-model capability linked into one machine session.
///
/// In a production host this is the data-model component instance; the
/// provider is owned by the session store and dropped with it. Calls are
/// synchronous, ordered, and never re-entrant.
/// `update_active_configuration` changes the configuration observed by
/// `In()` for the effects after it.
pub trait MachineDataModel {
    /// Stable data-model name (`ecmascript`, `null`, `xpath`), which selects
    /// the script language external `<script src>` text may use (SCXML 1.0
    /// section 5.8). The default, `None`, accepts no script text; Wasm
    /// script components do not depend on it.
    fn name(&self) -> Option<&str> {
        None
    }

    /// Updates the active configuration observed by `In()` (WIT
    /// `machine-capabilities.update-active-configuration`): removes every
    /// `exited` state (the others keep their order), then appends every
    /// `entered` state in order; a state in both lists is removed and then
    /// re-appended. The configuration starts empty.
    ///
    /// # Errors
    ///
    /// Any failure fails the calling machine session.
    fn update_active_configuration(
        &mut self,
        exited: &[String],
        entered: &[String],
    ) -> Result<(), DataModelFailure>;
    /// Evaluates one condition, with `_event` when `event` is present.
    ///
    /// # Errors
    ///
    /// Returns the closed failure for a rejected or failed evaluation.
    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<bool, DataModelFailure>;
    /// Evaluates one value expression, with `_event` when `event` is present.
    ///
    /// # Errors
    ///
    /// Returns the closed failure for a rejected or failed evaluation.
    fn evaluate_value(
        &mut self,
        expression: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<scxml_types::DataValue, DataModelFailure>;
    /// Assigns `source` to one location with an `<assign type>` placement
    /// (SCXML 1.0 CR 2014-03-13 §C.3.6) (`ReplaceChildren` with a value
    /// source for scripts, and with a `ForeachValue` source for `<foreach>`
    /// item/index, which declares its variable), with `_event` bound to
    /// `event` (unbound when `None`)
    /// while the location and an expression source are evaluated;
    /// evaluation and assignment are one transaction, and a failed
    /// assignment changes nothing.
    ///
    /// # Errors
    ///
    /// Returns the closed failure for a rejected or failed assignment; a
    /// placement the data model does not define is `Execution`.
    fn assign(
        &mut self,
        location: &str,
        source: &scxml_types::AssignSource,
        placement: &scxml_types::AssignPlacement,
        event: Option<&scxml_types::Event>,
    ) -> Result<(), DataModelFailure>;
    /// Executes one inline script body.
    ///
    /// # Errors
    ///
    /// Returns the closed failure for a rejected or failed script.
    fn execute(
        &mut self,
        script: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<(), DataModelFailure>;
    /// Renders one `<log expr>` to its log text (SCXML 4.7; WIT
    /// `data-model-api.evaluate-log`), with `_event` when `event` is
    /// present.
    ///
    /// # Errors
    ///
    /// Returns the closed failure for a rejected or failed evaluation.
    fn evaluate_log(
        &mut self,
        expression: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<String, DataModelFailure>;

    /// `data-model-api.initialize` for a native provider: declares the
    /// document's `<data>` ids and binds the system values. The default
    /// accepts a document without declarations and answers any declaration
    /// with [`DataModelFailure::Unavailable`] (a native provider without
    /// `<data>` support).
    ///
    /// # Errors
    ///
    /// The closed failure; any failure fails the machine session.
    fn initialize(
        &mut self,
        declarations: &[String],
        _system: &crate::DataModelSystemValues,
    ) -> Result<(), DataModelFailure> {
        if declarations.is_empty() {
            Ok(())
        } else {
            Err(DataModelFailure::Unavailable)
        }
    }

    /// The linked data-model component, when this provider is one. The host
    /// then forwards every `data-model-api` function to it one-to-one, with
    /// WIT values and errors unchanged (`machine-capabilities` contract). A
    /// native provider (the default, `None`) answers the data-model
    /// functions beyond this trait with `unavailable` or `none`.
    fn as_component(&mut self) -> Option<&mut crate::ComponentDataModel> {
        None
    }
}

/// One `<log>` capability call observed by the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineLogRecord {
    /// The `<log>` label, when present.
    pub label: Option<String>,
    /// The log text the data model rendered (empty for a `<log>` without
    /// `expr`).
    pub text: String,
}

/// Observable result of `start` or one `send`, which the machine ran to
/// a stable configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineCallTrace {
    /// Session status in a stable configuration.
    pub status: crate::ComponentStatus,
    /// Active leaf states in document order in a stable configuration.
    pub active_configuration: Vec<String>,
    /// Effects committed by the command.
    pub effect_count: u32,
    /// `<log>` records emitted by the command, in order.
    pub logs: Vec<MachineLogRecord>,
}

/// Observable trace of one representative machine session.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineSessionTrace {
    /// The `start` call result followed by one result per sent event.
    pub results: Vec<MachineCallTrace>,
    /// Remaining deterministic Wasmtime fuel after shutdown.
    pub remaining_fuel: u64,
}

pub(crate) fn failure_code(failure: DataModelFailure) -> ComponentError {
    let (code, detail) = match failure {
        DataModelFailure::InvalidInput => (ErrorCode::InvalidInput, "data model rejected input"),
        DataModelFailure::Execution => (ErrorCode::Execution, "data model evaluation failed"),
        DataModelFailure::Unavailable => (ErrorCode::Unavailable, "data model is unavailable"),
        DataModelFailure::Platform => (ErrorCode::Platform, "data model provider failed"),
    };
    ComponentError {
        version: ErrorVersion::V1,
        code,
        detail: Some(detail.into()),
    }
}

fn invalid(detail: &'static str) -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code: ErrorCode::InvalidInput,
        detail: Some(detail.into()),
    }
}

/// Converts one guest-supplied WIT value to the portable value contract.
pub(crate) fn native_value(value: WitDataValue) -> Result<scxml_types::DataValue, ComponentError> {
    match value {
        WitDataValue::Absent => Ok(scxml_types::DataValue::Absent),
        WitDataValue::Undefined => Ok(scxml_types::DataValue::Undefined),
        WitDataValue::Null => Ok(scxml_types::DataValue::Null),
        WitDataValue::Boolean(value) => Ok(scxml_types::DataValue::Bool(value)),
        WitDataValue::Number(value) => scxml_types::FiniteNumber::new(value)
            .map(scxml_types::DataValue::Number)
            .map_err(|_| invalid("number must be finite")),
        WitDataValue::Text(value) => Ok(scxml_types::DataValue::String(value)),
        WitDataValue::JsonArray(value) => scxml_types::DataValue::canonical_json_array(&value)
            .map_err(|_| invalid("JSON array is invalid")),
        WitDataValue::JsonObject(value) => scxml_types::DataValue::canonical_json_object(&value)
            .map_err(|_| invalid("JSON object is invalid")),
        WitDataValue::Xml(value) => Ok(scxml_types::DataValue::Xml(value)),
    }
}

/// Converts one portable value to its WIT representation.
pub(crate) fn wit_value(value: scxml_types::DataValue) -> WitDataValue {
    match value {
        scxml_types::DataValue::Absent => WitDataValue::Absent,
        scxml_types::DataValue::Undefined => WitDataValue::Undefined,
        scxml_types::DataValue::Null => WitDataValue::Null,
        scxml_types::DataValue::Bool(value) => WitDataValue::Boolean(value),
        scxml_types::DataValue::Number(value) => WitDataValue::Number(value.get()),
        scxml_types::DataValue::String(value) => WitDataValue::Text(value),
        scxml_types::DataValue::JsonArray(value) => WitDataValue::JsonArray(value),
        scxml_types::DataValue::JsonObject(value) => WitDataValue::JsonObject(value),
        scxml_types::DataValue::Xml(value) => WitDataValue::Xml(value),
    }
}

/// Converts one guest-supplied WIT assignment source to the portable source.
pub(crate) fn native_source(
    source: WitAssignSource,
) -> Result<scxml_types::AssignSource, ComponentError> {
    match source {
        WitAssignSource::Expression(expression) => {
            Ok(scxml_types::AssignSource::Expression(expression))
        }
        WitAssignSource::Value(value) => native_value(value).map(scxml_types::AssignSource::Value),
        WitAssignSource::ForeachValue(value) => {
            native_value(value).map(scxml_types::AssignSource::ForeachValue)
        }
    }
}

/// Converts one portable assignment source to its WIT representation.
pub(crate) fn wit_source(source: scxml_types::AssignSource) -> WitAssignSource {
    match source {
        scxml_types::AssignSource::Expression(expression) => {
            WitAssignSource::Expression(expression)
        }
        scxml_types::AssignSource::Value(value) => WitAssignSource::Value(wit_value(value)),
        scxml_types::AssignSource::ForeachValue(value) => {
            WitAssignSource::ForeachValue(wit_value(value))
        }
    }
}

/// Converts one guest-supplied WIT placement to the portable placement.
pub(crate) fn native_placement(placement: WitAssignPlacement) -> scxml_types::AssignPlacement {
    match placement {
        WitAssignPlacement::ReplaceChildren => scxml_types::AssignPlacement::ReplaceChildren,
        WitAssignPlacement::FirstChild => scxml_types::AssignPlacement::FirstChild,
        WitAssignPlacement::LastChild => scxml_types::AssignPlacement::LastChild,
        WitAssignPlacement::PreviousSibling => scxml_types::AssignPlacement::PreviousSibling,
        WitAssignPlacement::NextSibling => scxml_types::AssignPlacement::NextSibling,
        WitAssignPlacement::Replace => scxml_types::AssignPlacement::Replace,
        WitAssignPlacement::Delete => scxml_types::AssignPlacement::Delete,
        WitAssignPlacement::AddAttribute(name) => scxml_types::AssignPlacement::AddAttribute(name),
    }
}

/// Converts one portable placement to its WIT representation.
pub(crate) fn wit_placement(placement: scxml_types::AssignPlacement) -> WitAssignPlacement {
    match placement {
        scxml_types::AssignPlacement::ReplaceChildren => WitAssignPlacement::ReplaceChildren,
        scxml_types::AssignPlacement::FirstChild => WitAssignPlacement::FirstChild,
        scxml_types::AssignPlacement::LastChild => WitAssignPlacement::LastChild,
        scxml_types::AssignPlacement::PreviousSibling => WitAssignPlacement::PreviousSibling,
        scxml_types::AssignPlacement::NextSibling => WitAssignPlacement::NextSibling,
        scxml_types::AssignPlacement::Replace => WitAssignPlacement::Replace,
        scxml_types::AssignPlacement::Delete => WitAssignPlacement::Delete,
        scxml_types::AssignPlacement::AddAttribute(name) => WitAssignPlacement::AddAttribute(name),
    }
}

/// Converts one guest-supplied `_event` to the portable event contract,
/// validating every field through its owning constructor.
pub(crate) fn native_event(event: WitEvent) -> Result<scxml_types::Event, ComponentError> {
    let event = scxml_types::Event {
        name: scxml_types::EventName::new(event.name)
            .map_err(|_| invalid("event name is invalid"))?,
        event_type: match event.event_type {
            WitEventType::Internal => scxml_types::EventType::Internal,
            WitEventType::External => scxml_types::EventType::External,
            WitEventType::Platform => scxml_types::EventType::Platform,
        },
        send_id: event
            .send_id
            .map(scxml_types::SendId::new)
            .transpose()
            .map_err(|_| invalid("event send id is invalid"))?,
        origin: event.origin,
        origin_type: event.origin_type,
        invoke_id: event
            .invoke_id
            .map(scxml_types::InvokeId::new)
            .transpose()
            .map_err(|_| invalid("event invoke id is invalid"))?,
        data: native_value(event.data)?,
        raw: event.raw,
        trace_context: event
            .trace_context
            .map(|context| {
                scxml_types::TraceContext::new(
                    context.traceparent,
                    context.tracestate,
                    context.baggage,
                    match context.baggage_policy {
                        WitBaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                        WitBaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                    },
                )
                .map_err(|_| invalid("event trace context is invalid"))
            })
            .transpose()?,
    };
    event.validate().map_err(|_| invalid("event is invalid"))?;
    Ok(event)
}

/// A validated carrier as the WIT `trace-context`.
pub(crate) fn wit_trace_context(
    context: &scxml_types::TraceContext,
) -> crate::bindings::stateforward::scxml::types::TraceContext {
    crate::bindings::stateforward::scxml::types::TraceContext {
        traceparent: context.traceparent.clone(),
        tracestate: context.tracestate.clone(),
        baggage: context.baggage.clone(),
        baggage_policy: match context.baggage_policy {
            scxml_types::BaggagePolicy::Drop => WitBaggagePolicy::Drop,
            scxml_types::BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
        },
    }
}

/// Builds the external WIT event the driver sends for `name`.
pub(crate) fn external_event(name: &str) -> WitEvent {
    WitEvent {
        name: name.to_owned(),
        event_type: WitEventType::External,
        send_id: None,
        origin: None,
        origin_type: None,
        invoke_id: None,
        data: WitDataValue::Absent,
        raw: None,
        trace_context: None,
    }
}
