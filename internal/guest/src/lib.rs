//! The Wasm guest plumbing floppy's extension namespace components share
//! (one internal library crate, no component: no manifest, no WIT world,
//! no entry in `scripts/build-components.sh`'s component lists).
//!
//! Every namespace crate (`namespaces/*`) repeated this code verbatim, the
//! only difference being its host trait's name; each now carries its own
//! elements and re-uses what is here. Nothing in this crate does I/O, reads
//! a clock, or draws randomness: it is pure functions and type plumbing,
//! and it carries no `unsafe` code of its own (the guest glue the
//! [`extension_guest!`] macro writes into a namespace crate is
//! wit-bindgen's generated ABI, which that crate's own `allow(unsafe_code)`
//! on `wasm32` covers).
//!
//! * [`element`]: the element types every namespace's `execute` takes
//!   ([`Attribute`], [`Content`], [`Executed`], [`Failure`]), the host
//!   capabilities one `execute` call uses ([`Host`]), the attribute parsing
//!   ([`attribute_values`], [`no_content`]) and the `DataValue` <->
//!   `serde_json::Value` conversion ([`json_of`], [`number`], [`data_of`]).
//! * [`quantized`]: the `i8:` compact text of a quantized vector (the
//!   standard base64 of its signed bytes), which the conversation namespace
//!   writes voices as and the memory namespace's episode scoring reads.
//! * [`extension_guest!`]: the guest glue (the wit-bindgen world, the
//!   lifecycle, the telemetry observations and the `Guest` export) that
//!   every namespace component's crate source repeated as its `mod guest`.

#![forbid(unsafe_code)]

pub mod element;
pub mod quantized;

/// The guest glue of one extension namespace component.
///
/// Expands to the `mod guest` every namespace crate carried (wit-bindgen's
/// `executable-content-extension` world, the created/live/shutdown
/// lifecycle, one closed-label telemetry observation per entry point, and
/// the `Guest` export calling the crate's own `execute`). The path of the
/// WIT world resolves against the expanding crate's manifest directory, so
/// every namespace crate writes the same `../../scxml.rs/...` as before.
///
/// The glue calls `super::NAMESPACE_URI` and `super::execute`, which every
/// namespace crate defines.
///
/// Two forms:
///
/// * `extension_guest!()` for the namespaces whose host is the shared
///   [`element::Host`] (read and assign, the full value mapping), whose
///   standard guest-side implementation it writes;
/// * `extension_guest!(host = { items... })` for a namespace whose host
///   trait is richer (`decision`'s guards and snapshots, `device`'s
///   snapshots): the items replace the standard implementation and are
///   expanded inside the guest module, where the generated WIT modules, the
///   `Data` alias of `scxml_types::DataValue`, and `super::` (the crate
///   root) are in scope.
#[macro_export]
macro_rules! extension_guest {
    () => {
        $crate::extension_guest!(host = {
            impl $crate::element::Host for Host {
                type Error = ComponentError;

                fn read(
                    &mut self,
                    expression: &str,
                ) -> Result<Result<::scxml_types::DataValue, ()>, ComponentError> {
                    match data_model_host::evaluate(expression) {
                        Ok(value) => Ok(match value {
                            DataValue::Absent => Ok(Data::Absent),
                            DataValue::Undefined => Ok(Data::Undefined),
                            DataValue::Null => Ok(Data::Null),
                            DataValue::Boolean(value) => Ok(Data::Bool(value)),
                            DataValue::Number(value) => scxml_types::FiniteNumber::new(value)
                                .map(Data::Number)
                                .map_err(|_| ()),
                            DataValue::Text(text) => Ok(Data::String(text)),
                            DataValue::JsonArray(text) => Ok(Data::JsonArray(text)),
                            DataValue::JsonObject(text) => Ok(Data::JsonObject(text)),
                            DataValue::Xml(text) => Ok(Data::Xml(text)),
                        }),
                        Err(error) if recoverable(error.code) => Ok(Err(())),
                        Err(error) => Err(error),
                    }
                }

                fn assign(
                    &mut self,
                    location: &str,
                    value: &::scxml_types::DataValue,
                ) -> Result<Result<(), ()>, ComponentError> {
                    let value = match value {
                        Data::Null => DataValue::Null,
                        Data::Bool(value) => DataValue::Boolean(*value),
                        Data::Number(value) => DataValue::Number(value.get()),
                        Data::String(text) => DataValue::Text(text.clone()),
                        Data::JsonArray(text) => DataValue::JsonArray(text.clone()),
                        Data::JsonObject(text) => DataValue::JsonObject(text.clone()),
                        Data::Absent | Data::Undefined | Data::Xml(_) => return Ok(Err(())),
                    };
                    match data_model_host::assign(location, &value) {
                        Ok(()) => Ok(Ok(())),
                        Err(error) if recoverable(error.code) => Ok(Err(())),
                        Err(error) => Err(error),
                    }
                }
            }
        });
    };
    (host = { $($items:item)* }) => {
        #[cfg(target_arch = "wasm32")]
        #[allow(
            clippy::same_length_and_capacity,
            reason = "generated canonical-ABI ownership transfer"
        )]
        mod guest {
            ::wit_bindgen::generate!({
                world: "executable-content-extension",
                path: "../../scxml.rs/crates/scxml-wit/wit/scxml",
            });

            use std::cell::Cell;

            use exports::stateforward::scxml::executable_content_extension_api::Guest;
            use stateforward::scxml::data_model_host;
            use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
            use stateforward::scxml::types::{
                ComponentError, DataValue, ErrorCode, ErrorVersion, ExecutableContentExtension,
                ExtensionContent, ExtensionOutcome, TraceContext,
            };

            use ::scxml_types::DataValue as Data;
            use super::{Attribute, Content, Executed, Failure};

            /// The lifecycle of this instance.
            #[derive(Clone, Copy, Eq, PartialEq)]
            enum Lifecycle {
                Created,
                Live,
                ShutDown,
            }

            thread_local! {
                static STATE: Cell<Lifecycle> = const { Cell::new(Lifecycle::Created) };
            }

            struct Component;

            /// The host capabilities of one `execute` call.
            struct Host;

            /// Evaluation and assignment failures (and a session with no data
            /// model) are document errors.
            fn recoverable(code: ErrorCode) -> bool {
                matches!(
                    code,
                    ErrorCode::InvalidInput
                        | ErrorCode::Execution
                        | ErrorCode::Unavailable
                        | ErrorCode::PayloadTooLarge
                )
            }

            $($items)*

            fn content(items: Vec<ExtensionContent>) -> Vec<Content> {
                items
                    .into_iter()
                    .map(|item| match item {
                        ExtensionContent::Text(text) => Content::Text(text),
                        ExtensionContent::StartElement(_) => Content::Start,
                        ExtensionContent::EndElement(_) => Content::End,
                    })
                    .collect()
            }

            impl Guest for Component {
                fn namespace_uri() -> String {
                    super::NAMESPACE_URI.into()
                }

                fn initialize() -> Result<(), ComponentError> {
                    observed(Operation::Initialize, None, || {
                        if STATE.get() != Lifecycle::Created {
                            return Err(error(
                                ErrorCode::Protocol,
                                "initialize was already called",
                            ));
                        }
                        STATE.set(Lifecycle::Live);
                        Ok(())
                    })
                }

                fn execute(
                    element: ExecutableContentExtension,
                    context: Option<TraceContext>,
                ) -> Result<ExtensionOutcome, ComponentError> {
                    observed(Operation::Execute, context.as_ref(), || {
                        if element.name.namespace_uri.as_deref() != Some(super::NAMESPACE_URI) {
                            return Err(error(
                                ErrorCode::Protocol,
                                "element namespace does not match extension component identity",
                            ));
                        }
                        if STATE.get() != Lifecycle::Live {
                            return Err(error(
                                ErrorCode::Protocol,
                                "execute outside the initialized lifecycle",
                            ));
                        }
                        let attributes: Vec<Attribute> = element
                            .attributes
                            .into_iter()
                            .map(|attribute| Attribute {
                                namespace_uri: attribute.name.namespace_uri,
                                local_name: attribute.name.local_name,
                                value: attribute.value,
                            })
                            .collect();
                        let flattened = content(element.content);
                        match super::execute(
                            &element.name.local_name,
                            &attributes,
                            &flattened,
                            &mut Host,
                        ) {
                            Ok(Executed::Handled) => Ok(ExtensionOutcome::Handled),
                            Ok(Executed::NotHandled) => Ok(ExtensionOutcome::NotHandled),
                            Err(Failure::Execution(detail)) => {
                                Err(error(ErrorCode::Execution, detail))
                            }
                            Err(Failure::Host(host)) => Err(host),
                        }
                    })
                }

                fn shutdown() -> Result<(), ComponentError> {
                    observed(Operation::Shutdown, None, || {
                        STATE.set(Lifecycle::ShutDown);
                        Ok(())
                    })
                }
            }

            /// Records one closed-label observation per entry point: the
            /// operation, its outcome, and one signal event (no attribute
            /// carries input).
            fn observed<T>(
                operation: Operation,
                context: Option<&TraceContext>,
                body: impl FnOnce() -> Result<T, ComponentError>,
            ) -> Result<T, ComponentError> {
                let result = body();
                let outcome = match &result {
                    Ok(_) => Outcome::Ok,
                    Err(error) if error.code == ErrorCode::Execution => Outcome::Failed,
                    Err(_) => Outcome::Rejected,
                };
                telemetry::observe(operation, outcome, None, context);
                telemetry::event(
                    operation,
                    if result.is_ok() {
                        SignalEvent::TransformationCompleted
                    } else {
                        SignalEvent::InputRejected
                    },
                    context,
                );
                result
            }

            fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
                ComponentError {
                    version: ErrorVersion::V1,
                    code,
                    detail: (detail.len() <= 160).then(|| detail.into()),
                }
            }

            export!(Component);
        }
    };
}
