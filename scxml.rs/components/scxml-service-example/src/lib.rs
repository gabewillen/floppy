//! Example invoked-service component (SCXML 6.4): an echo and timer service.
//!
//! It exercises the whole `invoked-service` contract so hosts can prove
//! service invocation end to end, as one `async fn invoke`
//! (`invoked-service-api.invoke`, WASI 0.3):
//!
//! * the `delay` parameter (milliseconds, a non-negative whole number or a
//!   string of digits), when it is present and positive, starts a timer
//!   (`wasi:clocks/monotonic-clock.wait-for`, which the host runs on the
//!   session's logical clock); when it falls due the service returns
//!   `echo.tick` carrying the number of ticks so far;
//! * every event on the `events` stream (the invoking session's
//!   `<send target="#_invokeid">` and autoforwarded events) is returned as
//!   `echo.<name>` carrying the event's data, except for:
//!   * `stop`, which resolves the service's future with the JSON object
//!     `{"echoed":N,"ticks":M}` (the invoking session receives it on
//!     `done.invoke.<id>`);
//!   * `fetch`, which GETs the URL the invocation's `endpoint` slot
//!     references (`env:endpoint="NAME"` on the `<invoke>`) followed by the
//!     event's data when it is a string (a path, query, or fragment),
//!     through `outbound-http.fetch`, with `authorization: Bearer <token>`
//!     when the invocation binds the `token` slot, suspending only that task
//!     (the service keeps reading events meanwhile), and returns
//!     `echo.fetched` carrying the response status; without a bound
//!     `endpoint`, or when the request fails, the future resolves with an
//!     error (`error.communication`). The component holds only the slots'
//!     references, never their values;
//!   * `send`, which GETs the same URL through `outbound-http.send` and
//!     streams the response: it returns `echo.received` carrying the number
//!     of body bytes each read took, as they arrive, then `echo.fetched`
//!     carrying the status once the body has ended; a request or a body that
//!     fails resolves the future with that error.
//!
//! When the invoking state exits first, the host cancels the invocation
//! (SCXML 6.4) and drops its instance: there is nothing to release.
//!
//! The decisions are the portable [`Echo`] state machine, tested natively;
//! the guest carries out the [`EchoAction`]s it returns against the WIT
//! imports. It reaches nothing else: no WASI interface beyond its world's
//! logical clock, and the network only through the host-admitted
//! `outbound-http` request whose URL the run's value supplies (the component
//! never sees it).

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
/// The parameter that starts the timer, in milliseconds.
pub const DELAY_PARAMETER: &str = "delay";
/// The event name that completes the service.
pub const STOP_EVENT: &str = "stop";
/// The event the timer returns.
pub const TICK_EVENT: &str = "echo.tick";
/// The event name that GETs the `endpoint` slot's URL.
pub const FETCH_EVENT: &str = "fetch";
/// The event a fetch returns, carrying the response status.
pub const FETCHED_EVENT: &str = "echo.fetched";
/// The event name that GETs the `endpoint` slot's URL, streamed.
pub const SEND_EVENT: &str = "send";
/// The event a streamed GET returns for each body read, carrying its byte
/// count.
pub const RECEIVED_EVENT: &str = "echo.received";
/// The `env` slot whose value is the URL `fetch` requests.
pub const ENDPOINT_SLOT: &str = "endpoint";
/// The `env` slot whose value `fetch` sends as a bearer token.
pub const TOKEN_SLOT: &str = "token";

/// What the guest does for one input, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EchoAction {
    /// Return one external event to the invoking session.
    Return {
        /// The event name.
        name: EventName,
        /// Its data.
        data: DataValue,
    },
    /// Resolve the service's future with this data (`done.invoke`).
    Done(DataValue),
    /// GET the `endpoint` slot's URL followed by `path`, then
    /// [`Echo::fetched`] with the status.
    Fetch {
        /// What follows the endpoint (the `fetch` event's string data, else
        /// empty).
        path: String,
    },
    /// GET the `endpoint` slot's URL followed by `path` with its response
    /// streamed: [`Echo::received`] for each body read, then
    /// [`Echo::fetched`] with the status.
    Send {
        /// What follows the endpoint (the `send` event's string data, else
        /// empty).
        path: String,
    },
}

/// Why the service rejected an input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EchoError {
    /// The `delay` parameter is not a non-negative whole number of
    /// milliseconds.
    InvalidDelay,
    /// `echo.<name>` is not a valid event name.
    InvalidEventName,
    /// The service already completed.
    NotRunning,
    /// `fetch` or `send` without an `endpoint` slot reference.
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
    /// order; the last `delay` wins), and the timer's milliseconds when one
    /// is due.
    ///
    /// # Errors
    ///
    /// [`EchoError::InvalidDelay`] for a `delay` that is not a non-negative
    /// whole number of milliseconds.
    pub fn start<'a>(
        parameters: impl IntoIterator<Item = (&'a str, &'a DataValue)>,
    ) -> Result<(Self, Option<u64>), EchoError> {
        let mut delay = None;
        for (name, value) in parameters {
            if name == DELAY_PARAMETER {
                delay = Some(delay_ms(value)?);
            }
        }
        Ok((
            Self {
                ticks: 0,
                echoed: 0,
                running: true,
                endpoint: false,
            },
            delay.filter(|delay| *delay > 0),
        ))
    }

    /// The invocation bound the `endpoint` slot to a value (the guest holds
    /// its reference).
    pub fn bind_endpoint(&mut self) {
        self.endpoint = true;
    }

    /// One event the invoking session sent: `stop` completes the service,
    /// `fetch` requests the `endpoint` URL, and any other event is returned
    /// as `echo.<name>` with its data.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion,
    /// [`EchoError::NoEndpoint`] for `fetch` without a bound endpoint, and
    /// [`EchoError::InvalidEventName`] when `echo.<name>` is not a valid
    /// event name.
    pub fn send(&mut self, name: &str, data: DataValue) -> Result<EchoAction, EchoError> {
        if !self.running {
            return Err(EchoError::NotRunning);
        }
        if name == FETCH_EVENT || name == SEND_EVENT {
            return if self.endpoint {
                let path = match data {
                    DataValue::String(path) => path,
                    _ => String::new(),
                };
                Ok(if name == FETCH_EVENT {
                    EchoAction::Fetch { path }
                } else {
                    EchoAction::Send { path }
                })
            } else {
                Err(EchoError::NoEndpoint)
            };
        }
        if name == STOP_EVENT {
            self.running = false;
            let summary = format!("{{\"echoed\":{},\"ticks\":{}}}", self.echoed, self.ticks);
            let summary = DataValue::canonical_json_object(&summary)
                .map_err(|_| EchoError::InvalidEventName)?;
            return Ok(EchoAction::Done(summary));
        }
        let name =
            EventName::new(format!("echo.{name}")).map_err(|_| EchoError::InvalidEventName)?;
        self.echoed = self.echoed.saturating_add(1);
        Ok(EchoAction::Return { name, data })
    }

    /// The timer fell due: returns `echo.tick` with the tick count.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion.
    pub fn tick(&mut self) -> Result<EchoAction, EchoError> {
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
        Ok(EchoAction::Return { name, data })
    }

    /// A GET answered `status`: returns [`FETCHED_EVENT`] with it.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion.
    pub fn fetched(&mut self, status: u16) -> Result<EchoAction, EchoError> {
        if !self.running {
            return Err(EchoError::NotRunning);
        }
        let name =
            EventName::new(FETCHED_EVENT.to_owned()).map_err(|_| EchoError::InvalidEventName)?;
        let data = scxml_types::FiniteNumber::new(f64::from(status))
            .map_or(DataValue::Absent, DataValue::Number);
        Ok(EchoAction::Return { name, data })
    }

    /// A streamed GET read `bytes` body bytes: returns [`RECEIVED_EVENT`]
    /// with the count.
    ///
    /// # Errors
    ///
    /// [`EchoError::NotRunning`] after completion.
    pub fn received(&mut self, bytes: usize) -> Result<EchoAction, EchoError> {
        if !self.running {
            return Err(EchoError::NotRunning);
        }
        let name =
            EventName::new(RECEIVED_EVENT.to_owned()).map_err(|_| EchoError::InvalidEventName)?;
        // The count saturates at `u32::MAX` (documented saturation).
        let count = u32::try_from(bytes).unwrap_or(u32::MAX);
        let data = scxml_types::FiniteNumber::new(f64::from(count))
            .map_or(DataValue::Absent, DataValue::Number);
        Ok(EchoAction::Return { name, data })
    }

    /// Whether the service still takes inputs.
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
        // The world's `wasi:clocks` interfaces too.
        generate_all,
    });

    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use exports::stateforward::scxml::invoked_service_api::Guest;
    use stateforward::scxml::outbound_http::{self, HttpHeader, HttpRequest, TextPart};
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, InvokeRequest,
    };
    use wasi::clocks::monotonic_clock;

    use super::{Echo, EchoAction, EchoError};

    /// The service's result: `ok` with its done data, or its error.
    type Done = Result<Option<DataValue>, ComponentError>;

    /// What the service's tasks hand the task that owns the returned
    /// stream, in order.
    enum Output {
        Event(Event),
        Done(Done),
    }

    /// A single-threaded queue from the service's tasks to the writer task.
    #[derive(Default)]
    struct Queue {
        items: VecDeque<Output>,
        waker: Option<Waker>,
    }

    type Outputs = Rc<RefCell<Queue>>;

    fn push(outputs: &Outputs, output: Output) {
        let mut queue = outputs.borrow_mut();
        queue.items.push_back(output);
        if let Some(waker) = queue.waker.take() {
            waker.wake();
        }
    }

    async fn next(outputs: &Outputs) -> Output {
        std::future::poll_fn(|cx| {
            let mut queue = outputs.borrow_mut();
            if let Some(output) = queue.items.pop_front() {
                Poll::Ready(output)
            } else {
                queue.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    struct Component;

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT `invoke` is an `async func`; it returns once its tasks are spawned"
    )]
    impl Guest for Component {
        async fn invoke(
            request: InvokeRequest,
            mut events: wit_bindgen::StreamReader<Event>,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Done>,
            ),
            ComponentError,
        > {
            let started = start(&request);
            telemetry::observe(
                Operation::Start,
                if started.is_ok() {
                    Outcome::Ok
                } else {
                    Outcome::Rejected
                },
                None,
                None,
            );
            let (echo, delay, slots) = started?;
            let echo = Rc::new(RefCell::new(echo));
            let outputs: Outputs = Rc::default();
            let (mut returned, returned_reader) = wit_stream::new::<Event>();
            let (done, done_reader) = wit_future::new::<Done>(|| {
                Err(error(
                    ErrorCode::Platform,
                    "the service ended without a result",
                ))
            });
            // The writer: every returned event in order, then the result.
            let writer_outputs = Rc::clone(&outputs);
            wit_bindgen::spawn(async move {
                loop {
                    match next(&writer_outputs).await {
                        Output::Event(event) => {
                            if returned.write_one(event).await.is_some() {
                                return;
                            }
                        }
                        Output::Done(result) => {
                            drop(returned);
                            let _ = done.write(result).await;
                            return;
                        }
                    }
                }
            });
            // The timer, on the session's logical clock.
            if let Some(delay) = delay {
                let timer_echo = Rc::clone(&echo);
                let timer_outputs = Rc::clone(&outputs);
                wit_bindgen::spawn(async move {
                    monotonic_clock::wait_for(delay.saturating_mul(1_000_000)).await;
                    let action = timer_echo.borrow_mut().tick();
                    carry_out(action, &timer_echo, &timer_outputs, slots);
                });
            }
            // The events the invoking session sends.
            wit_bindgen::spawn(async move {
                while let Some(event) = events.next().await {
                    telemetry::event(Operation::Send, SignalEvent::InputAccepted, None);
                    let action = match native_value(event.data) {
                        Ok(data) => echo.borrow_mut().send(&event.name, data),
                        Err(failure) => {
                            push(&outputs, Output::Done(Err(failure)));
                            return;
                        }
                    };
                    if carry_out(action, &echo, &outputs, slots) {
                        return;
                    }
                    // A task spawned from a running task (a fetch) is polled
                    // only once the executor runs again: yield so it does.
                    wit_bindgen::yield_async().await;
                }
            });
            Ok((returned_reader, done_reader))
        }
    }

    /// The references the invocation bound to the `endpoint` and `token`
    /// slots.
    #[derive(Clone, Copy)]
    struct Slots {
        endpoint: Option<u32>,
        token: Option<u32>,
    }

    /// Parses the request: the service state, its timer, and its slot
    /// references.
    fn start(request: &InvokeRequest) -> Result<(Echo, Option<u64>, Slots), ComponentError> {
        if request.type_uri != super::ECHO_SERVICE_URI {
            return Err(error(ErrorCode::InvalidInput, "service type URI differs"));
        }
        let parameters = request
            .parameters
            .iter()
            .map(|entry| Ok((entry.name.as_str(), native_value(entry.value.clone())?)))
            .collect::<Result<Vec<_>, ComponentError>>()?;
        let (mut echo, delay) = Echo::start(parameters.iter().map(|(name, value)| (*name, value)))
            .map_err(echo_error)?;
        let slot = |name: &str| {
            request
                .references
                .iter()
                .find(|binding| binding.slot == name)
                .map(|binding| binding.reference)
        };
        let slots = Slots {
            endpoint: slot(super::ENDPOINT_SLOT),
            token: slot(super::TOKEN_SLOT),
        };
        if slots.endpoint.is_some() {
            echo.bind_endpoint();
        }
        Ok((echo, delay, slots))
    }

    /// Carries out one action; `true` when the service is done.
    fn carry_out(
        action: Result<EchoAction, EchoError>,
        echo: &Rc<RefCell<Echo>>,
        outputs: &Outputs,
        slots: Slots,
    ) -> bool {
        match action {
            Err(EchoError::NotRunning) => true,
            Err(failure) => {
                push(outputs, Output::Done(Err(echo_error(failure))));
                true
            }
            Ok(EchoAction::Return { name, data }) => {
                push(
                    outputs,
                    Output::Event(event(name.as_str(), wit_value(data))),
                );
                false
            }
            Ok(EchoAction::Done(data)) => {
                push(outputs, Output::Done(Ok(Some(wit_value(data)))));
                true
            }
            Ok(EchoAction::Fetch { path }) => get(path, false, echo, outputs, slots),
            Ok(EchoAction::Send { path }) => get(path, true, echo, outputs, slots),
        }
    }

    /// GETs the `endpoint` slot's URL followed by `path` (`streamed`:
    /// through `send`, reporting each body read), suspending only its own
    /// task; `true` when the service is done.
    fn get(
        path: String,
        streamed: bool,
        echo: &Rc<RefCell<Echo>>,
        outputs: &Outputs,
        slots: Slots,
    ) -> bool {
        let Some(reference) = slots.endpoint else {
            push(
                outputs,
                Output::Done(Err(echo_error(EchoError::NoEndpoint))),
            );
            return true;
        };
        // Suspends only this task: the service keeps reading events.
        let echo = Rc::clone(echo);
        let outputs = Rc::clone(outputs);
        wit_bindgen::spawn(async move {
            let mut url = vec![TextPart::Reference(reference)];
            if !path.is_empty() {
                url.push(TextPart::Literal(path));
            }
            let headers = slots
                .token
                .map(|token| HttpHeader {
                    name: "authorization".to_owned(),
                    value: vec![
                        TextPart::Literal("Bearer ".to_owned()),
                        TextPart::Reference(token),
                    ],
                })
                .into_iter()
                .collect();
            let request = HttpRequest {
                method: "GET".to_owned(),
                url,
                headers,
                body: Vec::new(),
            };
            let status = if streamed {
                send(request, &echo, &outputs, slots).await
            } else {
                outbound_http::fetch(request)
                    .await
                    .map(|response| response.status)
            };
            match status {
                Ok(status) => {
                    let action = echo.borrow_mut().fetched(status);
                    carry_out(action, &echo, &outputs, slots);
                }
                Err(failure) => push(&outputs, Output::Done(Err(failure))),
            }
        });
        false
    }

    /// Sends `request` and reads its body to the end, returning
    /// `echo.received` for each read; its status once the body ended.
    async fn send(
        request: HttpRequest,
        echo: &Rc<RefCell<Echo>>,
        outputs: &Outputs,
        slots: Slots,
    ) -> Result<u16, ComponentError> {
        let (status, mut body, end) = outbound_http::send(request).await?;
        loop {
            let (result, read) = body.read(Vec::with_capacity(65_536)).await;
            if !read.is_empty() {
                let action = echo.borrow_mut().received(read.len());
                carry_out(action, echo, outputs, slots);
            }
            if !matches!(result, wit_bindgen::StreamResult::Complete(_)) {
                break;
            }
        }
        drop(body);
        end.await.map(|()| status)
    }

    fn event(name: &str, data: DataValue) -> Event {
        Event {
            name: name.to_owned(),
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data,
            raw: None,
            trace_context: None,
        }
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
    fn start_reads_the_timer_from_the_delay_parameter() {
        let delay = number(250.0);
        let (_, timer) = Echo::start([("delay", &delay)]).expect("valid delay");
        assert_eq!(timer, Some(250));
        let text = DataValue::String("40".into());
        let (_, timer) = Echo::start([("other", &delay), ("delay", &text)]).expect("digits");
        assert_eq!(timer, Some(40));
        let (_, timer) = Echo::start([]).expect("no delay");
        assert_eq!(timer, None);
        let zero = number(0.0);
        assert_eq!(Echo::start([("delay", &zero)]).expect("zero").1, None);
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
        let action = echo
            .send("ping", DataValue::String("x".into()))
            .expect("echo");
        assert!(matches!(
            action,
            EchoAction::Return { name, data } if name.as_str() == "echo.ping" && data == DataValue::String("x".into())
        ));
        let tick = echo.tick().expect("tick");
        assert!(matches!(
            tick,
            EchoAction::Return { name, .. } if name.as_str() == TICK_EVENT
        ));
        let done = echo.send("stop", DataValue::Absent).expect("stop");
        assert_eq!(
            done,
            EchoAction::Done(DataValue::JsonObject("{\"echoed\":1,\"ticks\":1}".into()))
        );
        assert!(!echo.running());
        assert_eq!(
            echo.send("late", DataValue::Absent),
            Err(EchoError::NotRunning)
        );
        assert_eq!(echo.tick(), Err(EchoError::NotRunning));
    }

    #[test]
    fn invalid_echo_names_fail_closed() {
        let (mut echo, _) = Echo::start([]).expect("start");
        assert_eq!(
            echo.send(&"a".repeat(1_100), DataValue::Absent),
            Err(EchoError::InvalidEventName)
        );
    }

    #[test]
    fn fetch_requests_the_bound_endpoint_only_and_returns_its_status() {
        let (mut echo, _) = Echo::start([]).expect("start");
        assert_eq!(
            echo.send(super::FETCH_EVENT, DataValue::Absent),
            Err(EchoError::NoEndpoint)
        );
        echo.bind_endpoint();
        assert_eq!(
            echo.send(super::FETCH_EVENT, DataValue::Absent),
            Ok(EchoAction::Fetch {
                path: String::new()
            })
        );
        assert_eq!(
            echo.send(super::FETCH_EVENT, DataValue::String("/ok?q".into())),
            Ok(EchoAction::Fetch {
                path: "/ok?q".into()
            })
        );
        let action = echo.fetched(204).expect("running");
        assert!(matches!(
            action,
            EchoAction::Return { name, data } if name.as_str() == super::FETCHED_EVENT && data == number(204.0)
        ));
    }
}
