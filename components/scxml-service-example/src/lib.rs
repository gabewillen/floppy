//! Example invoked-service component (SCXML 6.4): an echo and timer service.
//!
//! It exercises the whole `invoked-service` contract so hosts can prove
//! service invocation end to end:
//!
//! * `start` reads the `delay` parameter (milliseconds, a non-negative whole
//!   number or a string of digits) and, when it is present and positive,
//!   asks for one wakeup that far ahead (`invoked-service-host.set-wakeup`);
//! * `wake` emits `echo.tick` carrying the number of ticks so far;
//! * `send(e)` emits `echo.<name>` carrying `e`'s data, except for an event
//!   named `stop`, which completes the service with the JSON object
//!   `{"echoed":N,"ticks":M}` (the invoking session receives it on
//!   `done.invoke.<id>`), and an event named `fetch`, which GETs the URL the
//!   invocation's `endpoint` slot references (`env:endpoint="NAME"` on the
//!   `<invoke>`) through `outbound-http` and emits `echo.fetched` carrying
//!   the response status; without a bound `endpoint` the `send` fails;
//! * `send(defer)` starts the same GET without waiting for it
//!   (`outbound-http.begin`): the session keeps running, and the `wake` the
//!   host calls once the request settled takes its outcome
//!   (`outbound-http.take`) and emits `echo.fetched` with the status; a
//!   `wake` that finds no settled request is the timer's and ticks;
//! * `cancel` drops the service's state; it is idempotent.
//!
//! The decisions are the portable [`Echo`] state machine, tested natively;
//! the guest executes the [`EchoAction`]s it returns against the WIT
//! imports. It reaches nothing else: no WASI interface, and the network only
//! through the host-admitted `outbound-http` request whose URL the run's
//! value supplies (the component never sees it).

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use scxml_types::{DataValue, EventName};

/// The exact service type URI this component implements.
pub const ECHO_SERVICE_URI: &str = "https://stateforward.org/scxml/services/echo";
/// The parameter that schedules the first wakeup, in milliseconds.
pub const DELAY_PARAMETER: &str = "delay";
/// The event name that completes the service.
pub const STOP_EVENT: &str = "stop";
/// The event `wake` emits.
pub const TICK_EVENT: &str = "echo.tick";
/// The event name that GETs the `endpoint` slot's URL.
pub const FETCH_EVENT: &str = "fetch";
/// The event a fetch emits, carrying the response status.
pub const FETCHED_EVENT: &str = "echo.fetched";
/// The event name that starts the `endpoint` GET without waiting for it.
pub const DEFER_EVENT: &str = "defer";
/// The `env` slot whose value is the URL `fetch` requests.
pub const ENDPOINT_SLOT: &str = "endpoint";

/// One effect the guest carries out through `invoked-service-host`, in
/// order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EchoAction {
    /// `emit` one external event.
    Emit {
        /// The event name.
        name: EventName,
        /// Its data.
        data: DataValue,
    },
    /// `complete` with this data.
    Complete(DataValue),
    /// `set-wakeup` this many milliseconds ahead.
    SetWakeup(u64),
    /// GET the `endpoint` slot's URL and emit [`FETCHED_EVENT`] with the
    /// status.
    Fetch,
    /// Start the `endpoint` GET without waiting (`outbound-http.begin`); its
    /// outcome arrives through `wake` ([`Echo::settled`]).
    Begin,
}

/// Why the service rejected a call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EchoError {
    /// The `delay` parameter is not a non-negative whole number of
    /// milliseconds.
    InvalidDelay,
    /// `echo.<name>` is not a valid event name.
    InvalidEventName,
    /// The service already completed or was cancelled.
    NotRunning,
    /// `fetch` without an `endpoint` slot reference.
    NoEndpoint,
}

/// The echo service's state for one invocation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Echo {
    ticks: u64,
    echoed: u64,
    running: bool,
    endpoint: bool,
}

impl Echo {
    /// Starts the service with the invocation's parameters (in document
    /// order; the last `delay` wins).
    ///
    /// # Errors
    ///
    /// [`EchoError::InvalidDelay`] for a `delay` that is not a non-negative
    /// whole number of milliseconds.
    pub fn start<'a>(
        parameters: impl IntoIterator<Item = (&'a str, &'a DataValue)>,
    ) -> Result<(Self, Vec<EchoAction>), EchoError> {
        let mut delay = None;
        for (name, value) in parameters {
            if name == DELAY_PARAMETER {
                delay = Some(delay_ms(value)?);
            }
        }
        let actions = delay
            .filter(|delay| *delay > 0)
            .map(EchoAction::SetWakeup)
            .into_iter()
            .collect();
        Ok((
            Self {
                ticks: 0,
                echoed: 0,
                running: true,
                endpoint: false,
            },
            actions,
        ))
    }

    /// The invocation bound the `endpoint` slot to a value (the guest holds
    /// its reference).
    pub fn bind_endpoint(&mut self) {
        self.endpoint = true;
    }

    /// One event the invoking session sent: `stop` completes the service,
    /// `fetch` requests the `endpoint` URL, and any other event is echoed as
    /// `echo.<name>` with its data.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion or cancellation,
    /// [`EchoError::NoEndpoint`] for `fetch` without a bound endpoint, and
    /// [`EchoError::InvalidEventName`] when `echo.<name>` is not a valid
    /// event name.
    pub fn send(&mut self, name: &str, data: DataValue) -> Result<Vec<EchoAction>, EchoError> {
        if !self.running {
            return Err(EchoError::NotRunning);
        }
        if name == FETCH_EVENT || name == DEFER_EVENT {
            return if self.endpoint {
                Ok(vec![if name == FETCH_EVENT {
                    EchoAction::Fetch
                } else {
                    EchoAction::Begin
                }])
            } else {
                Err(EchoError::NoEndpoint)
            };
        }
        if name == STOP_EVENT {
            self.running = false;
            let summary = format!("{{\"echoed\":{},\"ticks\":{}}}", self.echoed, self.ticks);
            let summary = DataValue::canonical_json_object(&summary)
                .map_err(|_| EchoError::InvalidEventName)?;
            return Ok(vec![EchoAction::Complete(summary)]);
        }
        let name =
            EventName::new(format!("echo.{name}")).map_err(|_| EchoError::InvalidEventName)?;
        self.echoed = self.echoed.saturating_add(1);
        Ok(vec![EchoAction::Emit { name, data }])
    }

    /// The wakeup fell due: emits `echo.tick` with the tick count.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion or cancellation.
    pub fn wake(&mut self) -> Result<Vec<EchoAction>, EchoError> {
        if !self.running {
            return Err(EchoError::NotRunning);
        }
        self.ticks = self.ticks.saturating_add(1);
        let name =
            EventName::new(TICK_EVENT.to_owned()).map_err(|_| EchoError::InvalidEventName)?;
        // The count saturates at `u32::MAX` (documented saturation).
        let count = u32::try_from(self.ticks).unwrap_or(u32::MAX);
        let data = scxml_types::FiniteNumber::new(f64::from(count))
            .map_or(DataValue::Absent, DataValue::Number);
        Ok(vec![EchoAction::Emit { name, data }])
    }

    /// A deferred GET settled with `status`: emits [`FETCHED_EVENT`] with it.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion or cancellation.
    pub fn settled(&mut self, status: u16) -> Result<Vec<EchoAction>, EchoError> {
        if !self.running {
            return Err(EchoError::NotRunning);
        }
        let name =
            EventName::new(FETCHED_EVENT.to_owned()).map_err(|_| EchoError::InvalidEventName)?;
        let data = scxml_types::FiniteNumber::new(f64::from(status))
            .map_or(DataValue::Absent, DataValue::Number);
        Ok(vec![EchoAction::Emit { name, data }])
    }

    /// The invocation was cancelled; idempotent.
    pub fn cancel(&mut self) {
        self.running = false;
    }

    /// Whether the service still accepts `send` and `wake`.
    #[must_use]
    pub const fn running(&self) -> bool {
        self.running
    }
}

/// A `delay` parameter's milliseconds.
fn delay_ms(value: &DataValue) -> Result<u64, EchoError> {
    match value {
        DataValue::Number(number) => {
            let value = number.get();
            if value >= 0.0 && value.fract() == 0.0 && value <= 9_007_199_254_740_992.0 {
                format!("{value:.0}")
                    .parse::<u64>()
                    .map_err(|_| EchoError::InvalidDelay)
            } else {
                Err(EchoError::InvalidDelay)
            }
        }
        DataValue::String(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
            text.parse::<u64>().map_err(|_| EchoError::InvalidDelay)
        }
        _ => Err(EchoError::InvalidDelay),
    }
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "invoked-service",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use std::cell::RefCell;

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::invoked_service_host;
    use stateforward::scxml::outbound_http::{self, HttpRequest, TextPart};
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, ServiceInvocation,
        SlotReference,
    };

    use super::{Echo, EchoAction, EchoError};

    thread_local! {
        // This instance's one invocation. The Component Model instance is
        // its single owner (the host instantiates one per invocation, on one
        // thread); each export borrows it for that call only, and the
        // imports it calls cannot re-enter the component.
        static SERVICE: RefCell<Option<Echo>> = const { RefCell::new(None) };
        // The `endpoint` slot's reference, valid for the whole invocation.
        static ENDPOINT: RefCell<Option<u32>> = const { RefCell::new(None) };
        // The ticket of the deferred GET in flight, if any.
        static DEFERRED: RefCell<Option<u64>> = const { RefCell::new(None) };
    }

    struct Component;

    impl Guest for Component {
        fn type_uri() -> String {
            telemetry::event(
                Operation::TypeUri,
                SignalEvent::TransformationCompleted,
                None,
            );
            super::ECHO_SERVICE_URI.to_owned()
        }

        fn start(
            request: ServiceInvocation,
            references: Vec<SlotReference>,
        ) -> Result<(), ComponentError> {
            observed(Operation::Start, || {
                if request.type_uri != super::ECHO_SERVICE_URI {
                    return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
                }
                let parameters = request
                    .parameters
                    .into_iter()
                    .map(|entry| Ok((entry.name, native_value(entry.value)?)))
                    .collect::<Result<Vec<_>, ComponentError>>()?;
                let (mut echo, actions) = Echo::start(
                    parameters
                        .iter()
                        .map(|(name, value)| (name.as_str(), value)),
                )
                .map_err(echo_error)?;
                let endpoint = references
                    .iter()
                    .find(|binding| binding.slot == super::ENDPOINT_SLOT)
                    .map(|binding| binding.reference);
                if endpoint.is_some() {
                    echo.bind_endpoint();
                }
                ENDPOINT.set(endpoint);
                SERVICE.set(Some(echo));
                run(actions)
            })
        }

        fn send(event: Event) -> Result<(), ComponentError> {
            observed(Operation::Send, || {
                let data = native_value(event.data)?;
                let actions = with_service(|echo| echo.send(&event.name, data))?;
                run(actions)
            })
        }

        fn wake() -> Result<(), ComponentError> {
            observed(Operation::Wake, || {
                // A settled deferred GET, else the timer.
                let settled = DEFERRED
                    .with_borrow(|ticket| *ticket)
                    .and_then(outbound_http::take);
                let actions = match settled {
                    Some(outcome) => {
                        DEFERRED.set(None);
                        let response = outcome?;
                        with_service(|echo| echo.settled(response.status))?
                    }
                    None => with_service(Echo::wake)?,
                };
                run(actions)
            })
        }

        fn cancel() -> Result<(), ComponentError> {
            observed(Operation::Cancel, || {
                SERVICE.with_borrow_mut(|service| {
                    if let Some(echo) = service.as_mut() {
                        echo.cancel();
                    }
                });
                Ok(())
            })
        }
    }

    fn with_service(
        body: impl FnOnce(&mut Echo) -> Result<Vec<EchoAction>, EchoError>,
    ) -> Result<Vec<EchoAction>, ComponentError> {
        SERVICE.with_borrow_mut(|service| match service.as_mut() {
            Some(echo) => body(echo).map_err(echo_error),
            None => Err(error(ErrorCode::Protocol, "the service was not started")),
        })
    }

    fn run(actions: Vec<EchoAction>) -> Result<(), ComponentError> {
        for action in actions {
            match action {
                EchoAction::Emit { name, data } => invoked_service_host::emit(&Event {
                    name: name.as_str().to_owned(),
                    event_type: EventType::External,
                    send_id: None,
                    origin: None,
                    origin_type: None,
                    invoke_id: None,
                    data: wit_value(data),
                    raw: None,
                    trace_context: None,
                })?,
                EchoAction::Complete(data) => {
                    invoked_service_host::complete(Some(&wit_value(data)))?;
                }
                EchoAction::SetWakeup(delay) => invoked_service_host::set_wakeup(delay)?,
                EchoAction::Begin => {
                    let reference = ENDPOINT
                        .with_borrow(|endpoint| *endpoint)
                        .ok_or_else(|| error(ErrorCode::Protocol, "no endpoint is bound"))?;
                    let ticket = outbound_http::begin(&HttpRequest {
                        method: "GET".to_owned(),
                        url: vec![TextPart::Reference(reference)],
                        headers: Vec::new(),
                        body: Vec::new(),
                    })?;
                    DEFERRED.set(Some(ticket));
                }
                EchoAction::Fetch => {
                    let reference = ENDPOINT
                        .with_borrow(|endpoint| *endpoint)
                        .ok_or_else(|| error(ErrorCode::Protocol, "no endpoint is bound"))?;
                    let response = outbound_http::fetch(&HttpRequest {
                        method: "GET".to_owned(),
                        url: vec![TextPart::Reference(reference)],
                        headers: Vec::new(),
                        body: Vec::new(),
                    })?;
                    invoked_service_host::emit(&Event {
                        name: super::FETCHED_EVENT.to_owned(),
                        event_type: EventType::External,
                        send_id: None,
                        origin: None,
                        origin_type: None,
                        invoke_id: None,
                        data: DataValue::Number(f64::from(response.status)),
                        raw: None,
                        trace_context: None,
                    })?;
                }
            }
        }
        Ok(())
    }

    fn native_value(value: DataValue) -> Result<scxml_types::DataValue, ComponentError> {
        let invalid = || error(ErrorCode::InvalidInput, "a data value is invalid");
        Ok(match value {
            DataValue::Absent => scxml_types::DataValue::Absent,
            DataValue::Undefined => scxml_types::DataValue::Undefined,
            DataValue::Null => scxml_types::DataValue::Null,
            DataValue::Boolean(value) => scxml_types::DataValue::Bool(value),
            DataValue::Number(value) => scxml_types::DataValue::Number(
                scxml_types::FiniteNumber::new(value).map_err(|_| invalid())?,
            ),
            DataValue::Text(value) => scxml_types::DataValue::String(value),
            DataValue::JsonArray(value) => {
                scxml_types::DataValue::canonical_json_array(&value).map_err(|_| invalid())?
            }
            DataValue::JsonObject(value) => {
                scxml_types::DataValue::canonical_json_object(&value).map_err(|_| invalid())?
            }
            DataValue::Xml(value) => scxml_types::DataValue::Xml(value),
        })
    }

    fn wit_value(value: scxml_types::DataValue) -> DataValue {
        match value {
            scxml_types::DataValue::Absent => DataValue::Absent,
            scxml_types::DataValue::Undefined => DataValue::Undefined,
            scxml_types::DataValue::Null => DataValue::Null,
            scxml_types::DataValue::Bool(value) => DataValue::Boolean(value),
            scxml_types::DataValue::Number(value) => DataValue::Number(value.get()),
            scxml_types::DataValue::String(value) => DataValue::Text(value),
            scxml_types::DataValue::JsonArray(value) => DataValue::JsonArray(value),
            scxml_types::DataValue::JsonObject(value) => DataValue::JsonObject(value),
            scxml_types::DataValue::Xml(value) => DataValue::Xml(value),
        }
    }

    fn echo_error(failure: EchoError) -> ComponentError {
        match failure {
            EchoError::InvalidDelay => {
                error(ErrorCode::InvalidInput, "the delay parameter is invalid")
            }
            EchoError::InvalidEventName => {
                error(ErrorCode::InvalidInput, "the echoed event name is invalid")
            }
            EchoError::NotRunning => error(ErrorCode::Protocol, "the service is not running"),
            EchoError::NoEndpoint => error(ErrorCode::InvalidInput, "no endpoint slot is bound"),
        }
    }

    fn error(code: ErrorCode, detail: &str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: Some(detail.to_owned()),
        }
    }

    /// One export call's closed telemetry: its outcome and one event.
    fn observed<T>(
        operation: Operation,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(failure) if failure.code == ErrorCode::InvalidInput => Outcome::Rejected,
            Err(_) => Outcome::Failed,
        };
        telemetry::observe(operation, outcome, None, None);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            None,
        );
        result
    }

    export!(Component);
}

#[cfg(test)]
mod tests {
    use super::{Echo, EchoAction, EchoError, TICK_EVENT};
    use scxml_types::{DataValue, FiniteNumber};

    fn number(value: f64) -> DataValue {
        DataValue::Number(FiniteNumber::new(value).expect("finite"))
    }

    #[test]
    fn start_schedules_a_wakeup_from_the_delay_parameter() {
        let delay = number(250.0);
        let (_, actions) = Echo::start([("delay", &delay)]).expect("valid delay");
        assert_eq!(actions, [EchoAction::SetWakeup(250)]);
        let text = DataValue::String("40".into());
        let (_, actions) = Echo::start([("other", &delay), ("delay", &text)]).expect("digits");
        assert_eq!(actions, [EchoAction::SetWakeup(40)]);
        let (_, actions) = Echo::start([]).expect("no delay");
        assert!(actions.is_empty());
        let zero = number(0.0);
        assert!(Echo::start([("delay", &zero)]).expect("zero").1.is_empty());
        for bad in [
            number(-1.0),
            number(1.5),
            DataValue::String("1s".into()),
            DataValue::Null,
        ] {
            assert_eq!(
                Echo::start([("delay", &bad)]).map(|_| ()),
                Err(EchoError::InvalidDelay)
            );
        }
    }

    #[test]
    fn send_echoes_and_stop_completes_with_a_summary() {
        let (mut echo, _) = Echo::start([]).expect("start");
        let actions = echo
            .send("ping", DataValue::String("x".into()))
            .expect("echo");
        assert!(matches!(
            actions.as_slice(),
            [EchoAction::Emit { name, data }] if name.as_str() == "echo.ping" && *data == DataValue::String("x".into())
        ));
        let tick = echo.wake().expect("wake");
        assert!(matches!(
            tick.as_slice(),
            [EchoAction::Emit { name, .. }] if name.as_str() == TICK_EVENT
        ));
        let done = echo.send("stop", DataValue::Absent).expect("stop");
        assert_eq!(
            done,
            [EchoAction::Complete(DataValue::JsonObject(
                "{\"echoed\":1,\"ticks\":1}".into()
            ))]
        );
        assert!(!echo.running());
        assert_eq!(
            echo.send("late", DataValue::Absent),
            Err(EchoError::NotRunning)
        );
        assert_eq!(echo.wake(), Err(EchoError::NotRunning));
    }

    #[test]
    fn invalid_echo_names_and_cancellation_fail_closed() {
        let (mut echo, _) = Echo::start([]).expect("start");
        assert_eq!(
            echo.send(&"a".repeat(1_100), DataValue::Absent),
            Err(EchoError::InvalidEventName)
        );
        echo.cancel();
        echo.cancel();
        assert!(!echo.running());
        assert_eq!(echo.wake(), Err(EchoError::NotRunning));
    }

    #[test]
    fn fetch_requests_the_bound_endpoint_only() {
        let (mut echo, _) = Echo::start([]).expect("start");
        assert_eq!(
            echo.send(super::FETCH_EVENT, DataValue::Absent),
            Err(EchoError::NoEndpoint)
        );
        echo.bind_endpoint();
        assert_eq!(
            echo.send(super::FETCH_EVENT, DataValue::Absent),
            Ok(vec![EchoAction::Fetch])
        );
        assert_eq!(
            echo.send(super::DEFER_EVENT, DataValue::Absent),
            Ok(vec![EchoAction::Begin])
        );
    }

    #[test]
    fn a_settled_deferred_fetch_emits_its_status() {
        let (mut echo, _) = Echo::start([]).expect("start");
        let actions = echo.settled(204).expect("running");
        assert!(matches!(
            actions.as_slice(),
            [EchoAction::Emit { name, data }] if name.as_str() == super::FETCHED_EVENT && *data == number(204.0)
        ));
        echo.cancel();
        assert_eq!(echo.settled(204), Err(EchoError::NotRunning));
    }
}
