//! `stateforward:scxml-basichttp/basichttp` component adapter.
//!
//! Every `BasicHTTP` decision is the shared native logic in the crate root;
//! this module only converts canonical WIT values and forwards outbound
//! requests to the imported `http-transport` capability.

#![allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]

wit_bindgen::generate!({
    world: "basichttp",
    path: "../../crates/scxml-wit/wit/basichttp",
    generate_all,
});

use exports::stateforward::scxml_basichttp::basichttp_api::Guest;
use stateforward::scxml_basichttp::http_transport;
use stateforward::scxml_basichttp::telemetry;
use stateforward::scxml_basichttp::types::{
    BaggagePolicy, ComponentError, DataValue as WitDataValue, Delivery as WitDelivery, ErrorCode,
    ErrorVersion, HttpHeader as WitHttpHeader, HttpRequest as WitHttpRequest,
    HttpResponse as WitHttpResponse, OutboundSend, Received, Rejection as WitRejection,
    TraceContext,
};

struct Component;

impl Guest for Component {
    fn type_uri() -> String {
        telemetry::observe(
            telemetry::Operation::TypeUri,
            telemetry::Outcome::Ok,
            None,
            None,
        );
        telemetry::event(
            telemetry::Operation::TypeUri,
            telemetry::SignalEvent::TypeUriReturned,
            None,
        );
        super::BasicHttpIoProcessor.type_uri().into()
    }

    fn aliases() -> Vec<String> {
        // Pure: no telemetry (WIT contract).
        super::BASICHTTP_PROCESSOR_ALIASES
            .iter()
            .map(|alias| (*alias).to_owned())
            .collect()
    }

    fn send(request: OutboundSend) -> Result<(), ComponentError> {
        let mut context = None;
        let result = (|| {
            context = validate_context(request.trace_context.clone())?;
            let outbound = to_native_send(request)?;
            super::BasicHttpIoProcessor
                .send(&outbound, &mut ImportedTransport)
                .map_err(send_error)
        })();
        let (outcome, event) = match &result {
            Ok(()) => (telemetry::Outcome::Ok, telemetry::SignalEvent::SendAccepted),
            Err(error) => (
                if error.code == ErrorCode::Unavailable {
                    telemetry::Outcome::Failed
                } else {
                    telemetry::Outcome::Rejected
                },
                telemetry::SignalEvent::SendRejected,
            ),
        };
        telemetry::observe(telemetry::Operation::Send, outcome, None, context.as_ref());
        telemetry::event(telemetry::Operation::Send, event, context.as_ref());
        result
    }

    fn check(request: OutboundSend) -> Result<(), ComponentError> {
        // Pure: no telemetry (WIT contract).
        validate_context(request.trace_context.clone())?;
        let outbound = to_native_send(request)?;
        super::BasicHttpIoProcessor
            .check(&outbound)
            .map_err(send_error)
    }

    fn receive(request: WitHttpRequest) -> Result<Received, WitRejection> {
        // Documented saturation: a measurement only, never a decision.
        let body_bytes = u64::try_from(request.body.len()).unwrap_or(u64::MAX);
        let result = super::BasicHttpIoProcessor
            .receive(&to_native_request(request))
            .map(|event| Received {
                name: event.name.as_str().to_owned(),
                data: from_native_value(event.data),
                raw: event.raw.unwrap_or_default(),
            })
            .map_err(|rejection| match rejection {
                super::Rejection::InvalidMessage => WitRejection::InvalidMessage,
                super::Rejection::MethodNotAllowed => WitRejection::MethodNotAllowed,
                super::Rejection::PayloadTooLarge => WitRejection::PayloadTooLarge,
            });
        let (outcome, event) = if result.is_ok() {
            (
                telemetry::Outcome::Ok,
                telemetry::SignalEvent::MessageAccepted,
            )
        } else {
            (
                telemetry::Outcome::Rejected,
                telemetry::SignalEvent::MessageRejected,
            )
        };
        telemetry::observe(
            telemetry::Operation::Receive,
            outcome,
            Some(telemetry::Measurement {
                kind: telemetry::MeasurementKind::BodyBytes,
                value: body_bytes,
            }),
            None,
        );
        telemetry::event(telemetry::Operation::Receive, event, None);
        result
    }

    fn respond(outcome: WitDelivery) -> WitHttpResponse {
        let delivery = match outcome {
            WitDelivery::Enqueued => super::Delivery::Enqueued,
            WitDelivery::Rejected(rejection) => super::Delivery::Rejected(match rejection {
                WitRejection::InvalidMessage => super::Rejection::InvalidMessage,
                WitRejection::MethodNotAllowed => super::Rejection::MethodNotAllowed,
                WitRejection::PayloadTooLarge => super::Rejection::PayloadTooLarge,
            }),
            WitDelivery::UnknownSession => super::Delivery::UnknownSession,
            WitDelivery::QueueFull => super::Delivery::QueueFull,
        };
        let response = from_native_response(super::BasicHttpIoProcessor.respond(delivery));
        telemetry::observe(
            telemetry::Operation::Respond,
            telemetry::Outcome::Ok,
            None,
            None,
        );
        telemetry::event(
            telemetry::Operation::Respond,
            telemetry::SignalEvent::ResponseProduced,
            None,
        );
        response
    }
}

/// The imported `http-transport` capability as the native transport trait.
struct ImportedTransport;

impl super::HttpTransport for ImportedTransport {
    fn post(
        &mut self,
        request: &super::HttpRequest,
    ) -> Result<super::HttpResponse, super::TransportFailure> {
        http_transport::post(&from_native_request(request))
            .map(to_native_response)
            .map_err(|()| super::TransportFailure)
    }
}

fn validate_context(context: Option<TraceContext>) -> Result<Option<TraceContext>, ComponentError> {
    context
        .map(|context| {
            scxml_types::TraceContext::new(
                context.traceparent,
                context.tracestate,
                context.baggage,
                match context.baggage_policy {
                    BaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                    BaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                },
            )
            .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
            .map(|validated| TraceContext {
                traceparent: validated.traceparent,
                tracestate: validated.tracestate,
                baggage: validated.baggage,
                baggage_policy: match validated.baggage_policy {
                    scxml_types::BaggagePolicy::Drop => BaggagePolicy::Drop,
                    scxml_types::BaggagePolicy::Propagate => BaggagePolicy::Propagate,
                },
            })
        })
        .transpose()
}

fn to_native_send(request: OutboundSend) -> Result<super::OutboundSend, ComponentError> {
    if request.params.len() > super::MAX_PARAMS {
        return Err(send_error(super::SendError::PayloadTooLarge));
    }
    let mut params = Vec::with_capacity(request.params.len());
    for param in request.params {
        params.push(super::Param {
            name: param.name,
            value: to_native_value(param.value)?,
        });
    }
    Ok(super::OutboundSend {
        event_name: request.event_name,
        target: request.target,
        params,
        content: request.content.map(to_native_value).transpose()?,
    })
}

fn to_native_value(value: WitDataValue) -> Result<scxml_types::DataValue, ComponentError> {
    match value {
        WitDataValue::Absent => Ok(scxml_types::DataValue::Absent),
        WitDataValue::Undefined => Ok(scxml_types::DataValue::Undefined),
        WitDataValue::Null => Ok(scxml_types::DataValue::Null),
        WitDataValue::Boolean(value) => Ok(scxml_types::DataValue::Bool(value)),
        WitDataValue::Number(value) => scxml_types::FiniteNumber::new(value)
            .map(scxml_types::DataValue::Number)
            .map_err(|_| error(ErrorCode::InvalidInput, "parameter number must be finite")),
        WitDataValue::Text(value) => Ok(scxml_types::DataValue::String(value)),
        WitDataValue::JsonArray(value) => Ok(scxml_types::DataValue::JsonArray(value)),
        WitDataValue::JsonObject(value) => Ok(scxml_types::DataValue::JsonObject(value)),
        WitDataValue::Xml(value) => Ok(scxml_types::DataValue::Xml(value)),
    }
}

fn from_native_value(value: scxml_types::DataValue) -> WitDataValue {
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

fn to_native_headers(headers: Vec<WitHttpHeader>) -> Vec<super::HttpHeader> {
    headers
        .into_iter()
        .map(|header| super::HttpHeader {
            name: header.name,
            value: header.value,
        })
        .collect()
}

fn from_native_headers(headers: &[super::HttpHeader]) -> Vec<WitHttpHeader> {
    headers
        .iter()
        .map(|header| WitHttpHeader {
            name: header.name.clone(),
            value: header.value.clone(),
        })
        .collect()
}

fn to_native_request(request: WitHttpRequest) -> super::HttpRequest {
    super::HttpRequest {
        method: request.method,
        uri: request.uri,
        headers: to_native_headers(request.headers),
        body: request.body,
    }
}

fn from_native_request(request: &super::HttpRequest) -> WitHttpRequest {
    WitHttpRequest {
        method: request.method.clone(),
        uri: request.uri.clone(),
        headers: from_native_headers(&request.headers),
        body: request.body.clone(),
    }
}

fn to_native_response(response: WitHttpResponse) -> super::HttpResponse {
    super::HttpResponse {
        status: response.status,
        headers: to_native_headers(response.headers),
        body: response.body,
    }
}

fn from_native_response(response: super::HttpResponse) -> WitHttpResponse {
    WitHttpResponse {
        status: response.status,
        headers: from_native_headers(&response.headers),
        body: response.body,
    }
}

fn send_error(cause: super::SendError) -> ComponentError {
    error(
        match cause.kind() {
            super::SendErrorKind::InvalidInput => ErrorCode::InvalidInput,
            super::SendErrorKind::PayloadTooLarge => ErrorCode::PayloadTooLarge,
            super::SendErrorKind::Unavailable => ErrorCode::Unavailable,
        },
        cause.detail(),
    )
}

fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code,
        detail: (detail.len() <= 160).then(|| detail.into()),
    }
}

export!(Component);
