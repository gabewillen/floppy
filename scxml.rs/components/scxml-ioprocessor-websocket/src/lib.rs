//! The WebSocket Event I/O Processor (SCXML 6.1, 6.2) as a Wasm component
//! (`stateforward:scxml/event-processor`).
//!
//! A `<send type="websocket">` (or this processor's type URI) whose target
//! is a `ws://` or `wss://` URL sends one message over a connection to that
//! URL; what the peer sends back reaches the session as external events.
//!
//! * **Connections** — one per target URL and instance (so per session),
//!   opened by the first `<send>` to it through the host's
//!   `outbound-websocket.connect`, which admits only the origins this
//!   processor's package declares. It stays open until the peer closes it,
//!   it is lost, or the session ends. A `<send>` after it closed opens a new
//!   one.
//! * **Sending** — a `<send>` with `<content>` sends the content: a string as
//!   a text message, any other value as its JSON text. A `<send>` whose only
//!   parameter is `_scxmlbinary` sends its base64 value decoded as a binary
//!   message. Any other `<send>` sends a JSON object text: the event name as
//!   `_scxmleventname` (when it has one) and each parameter as a member (the
//!   last of a repeated name wins). A `<send>` resolves once its message is
//!   handed to the connection; one whose connection cannot be opened fails
//!   with `unavailable` (`error.communication`), and one that cannot be
//!   mapped with `invalid-input` (`error.execution`).
//! * **Receiving** — a text message that is a JSON object with a string
//!   `_scxmleventname` becomes that event, its other members the event's
//!   data; any other text message becomes `websocket.message` with its JSON
//!   value (or the text itself, when it is not JSON) as data; a binary
//!   message becomes `websocket.message` with `{"_scxmlbinary": <base64>}`.
//! * **Closing** — a `<send event="websocket.close">` with no parameters and
//!   no content starts closing the connection to its target (code 1000)
//!   instead of sending a message: the peer may still send messages before
//!   its own close frame arrives, and they still reach the session. A
//!   `<send>` to that URL afterwards opens a new connection; closing a URL
//!   with no connection does nothing.
//!   When the connection ends the session receives `websocket.close` with
//!   `{"code", "reason"}`: the peer's close frame, or code 1006 when the
//!   connection was lost. Every event carries the target URL as
//!   `_event.origin`, so `<send target="_event.origin" type="websocket">`
//!   answers on the same connection.
//!
//! The mapping is the portable functions below, tested natively; the guest
//! carries it out against the WIT imports.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use scxml_types::{DataValue, EventName, FiniteNumber};

/// The exact type URI this processor implements.
pub const WEBSOCKET_PROCESSOR_URI: &str =
    "https://github.com/stateforward/scxml.rs/processors/websocket";
/// The short name a `<send type>` may use in place of the URI.
pub const WEBSOCKET_PROCESSOR_ALIAS: &str = "websocket";
/// The JSON member that names the event a message carries.
pub const EVENT_NAME_MEMBER: &str = "_scxmleventname";
/// The parameter, and the JSON member, that carries binary data as base64.
pub const BINARY_MEMBER: &str = "_scxmlbinary";
/// The event of a message that names no event.
pub const MESSAGE_EVENT: &str = "websocket.message";
/// The event of a connection's end.
pub const CLOSE_EVENT: &str = "websocket.close";
/// The close code of a lost connection (RFC 6455 7.4.1, "abnormal closure").
pub const LOST_CLOSE_CODE: u16 = 1006;
/// Maximum bytes of one message either way (the WIT bound).
pub const MAX_MESSAGE_BYTES: usize = 1_048_576;

/// One WebSocket message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// A text message.
    Text(String),
    /// A binary message.
    Binary(Vec<u8>),
}

/// Why a `<send>` cannot be sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapError {
    /// No target, or one that is not a `ws://` or `wss://` URL.
    InvalidTarget,
    /// `_scxmlbinary` is not base64.
    InvalidBinary,
    /// A value has no JSON form.
    Unrepresentable,
    /// The message passes [`MAX_MESSAGE_BYTES`].
    TooLarge,
}

/// The connection URL a `<send>` targets: an absolute `ws://` or `wss://`
/// URL with a host.
///
/// # Errors
///
/// [`MapError::InvalidTarget`] otherwise.
pub fn target_url(target: Option<&str>) -> Result<&str, MapError> {
    let target = target.ok_or(MapError::InvalidTarget)?;
    let rest = ["wss://", "ws://"]
        .into_iter()
        .find_map(|scheme| {
            target
                .get(..scheme.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
                .and_then(|_| target.get(scheme.len()..))
        })
        .ok_or(MapError::InvalidTarget)?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || target.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(MapError::InvalidTarget);
    }
    Ok(target)
}

/// The JSON number `JSON.stringify` writes for `value`: an integer within
/// ±(2^53 − 1) as an integer (`1`, not `1.0`; `-0` as `0`).
fn json_number(value: f64) -> Option<serde_json::Value> {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    if value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "an integral value within ±(2^53 − 1) converts exactly"
        )]
        return Some(serde_json::Value::from(value as i64));
    }
    serde_json::Number::from_f64(value).map(serde_json::Value::Number)
}

/// The JSON value of one data value; `None` for no value (absent or
/// undefined).
fn json_value(value: &DataValue) -> Result<Option<serde_json::Value>, MapError> {
    Ok(Some(match value {
        DataValue::Absent | DataValue::Undefined => return Ok(None),
        DataValue::Null => serde_json::Value::Null,
        DataValue::Bool(value) => serde_json::Value::Bool(*value),
        DataValue::Number(value) => json_number(value.get()).ok_or(MapError::Unrepresentable)?,
        DataValue::String(value) | DataValue::Xml(value) => {
            serde_json::Value::String(value.clone())
        }
        DataValue::JsonArray(text) | DataValue::JsonObject(text) => {
            serde_json::from_str(text).map_err(|_| MapError::Unrepresentable)?
        }
    }))
}

/// The message one `<send>` sends (see the crate documentation).
///
/// # Errors
///
/// [`MapError`] for a value with no JSON form, an invalid `_scxmlbinary`,
/// or a message past its bound.
pub fn outbound(
    event_name: Option<&str>,
    params: &[(String, DataValue)],
    content: Option<&DataValue>,
) -> Result<Frame, MapError> {
    let frame = match (content, params) {
        (Some(DataValue::String(text) | DataValue::Xml(text)), _) => Frame::Text(text.clone()),
        (Some(content), _) if !matches!(content, DataValue::Absent | DataValue::Undefined) => {
            let value = json_value(content)?.ok_or(MapError::Unrepresentable)?;
            Frame::Text(value.to_string())
        }
        (_, [(name, DataValue::String(encoded))]) if name == BINARY_MEMBER => {
            Frame::Binary(base64_decode(encoded).ok_or(MapError::InvalidBinary)?)
        }
        (_, [(name, _)]) if name == BINARY_MEMBER => return Err(MapError::InvalidBinary),
        _ => {
            let mut object = serde_json::Map::new();
            for (name, value) in params {
                if let Some(value) = json_value(value)? {
                    object.insert(name.clone(), value);
                }
            }
            if let Some(event_name) = event_name {
                object.insert(
                    EVENT_NAME_MEMBER.to_owned(),
                    serde_json::Value::String(event_name.to_owned()),
                );
            }
            Frame::Text(serde_json::Value::Object(object).to_string())
        }
    };
    let bytes = match &frame {
        Frame::Text(text) => text.len(),
        Frame::Binary(bytes) => bytes.len(),
    };
    if bytes > MAX_MESSAGE_BYTES {
        return Err(MapError::TooLarge);
    }
    Ok(frame)
}

/// Whether one `<send>` closes its target's connection rather than sending
/// a message: the event `websocket.close` with no parameters and no
/// content.
#[must_use]
pub fn is_close(
    event_name: Option<&str>,
    params: &[(String, DataValue)],
    content: Option<&DataValue>,
) -> bool {
    event_name == Some(CLOSE_EVENT)
        && params.is_empty()
        && content.is_none_or(|content| matches!(content, DataValue::Absent | DataValue::Undefined))
}

/// The data value of one JSON value.
fn data_value(value: serde_json::Value) -> DataValue {
    match value {
        serde_json::Value::Null => DataValue::Null,
        serde_json::Value::Bool(value) => DataValue::Bool(value),
        serde_json::Value::Number(number) => number
            .as_f64()
            .and_then(|value| FiniteNumber::new(value).ok())
            .map_or(DataValue::String(number.to_string()), DataValue::Number),
        serde_json::Value::String(value) => DataValue::String(value),
        value @ serde_json::Value::Array(_) => {
            DataValue::canonical_json_array(&value.to_string()).unwrap_or(DataValue::Absent)
        }
        value @ serde_json::Value::Object(_) => {
            DataValue::canonical_json_object(&value.to_string()).unwrap_or(DataValue::Absent)
        }
    }
}

/// The event one received message becomes: its name and data (see the
/// crate documentation).
#[must_use]
pub fn inbound(frame: Frame) -> (EventName, DataValue) {
    let message = || EventName::new(MESSAGE_EVENT.to_owned());
    let (name, data) = match frame {
        Frame::Text(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(mut members)) => {
                let named = match members.get(EVENT_NAME_MEMBER) {
                    Some(serde_json::Value::String(name)) => EventName::new(name.clone()).ok(),
                    _ => None,
                };
                match named {
                    Some(name) => {
                        members.remove(EVENT_NAME_MEMBER);
                        (Ok(name), data_value(serde_json::Value::Object(members)))
                    }
                    None => (message(), data_value(serde_json::Value::Object(members))),
                }
            }
            Ok(value) => (message(), data_value(value)),
            Err(_) => (message(), DataValue::String(text)),
        },
        Frame::Binary(bytes) => {
            let mut object = serde_json::Map::new();
            object.insert(
                BINARY_MEMBER.to_owned(),
                serde_json::Value::String(base64_encode(&bytes)),
            );
            (message(), data_value(serde_json::Value::Object(object)))
        }
    };
    match name {
        Ok(name) => (name, data),
        // `websocket.message` is a valid name; an invalid one never results.
        Err(_) => (
            EventName::new(MESSAGE_EVENT.to_owned()).unwrap_or_else(|_| unreachable_name()),
            data,
        ),
    }
}

/// The event of a connection's end: `websocket.close` with its code and
/// reason.
#[must_use]
pub fn closed(code: u16, reason: &str) -> (EventName, DataValue) {
    let mut object = serde_json::Map::new();
    object.insert("code".to_owned(), serde_json::Value::from(code));
    object.insert(
        "reason".to_owned(),
        serde_json::Value::String(reason.to_owned()),
    );
    (
        EventName::new(CLOSE_EVENT.to_owned()).unwrap_or_else(|_| unreachable_name()),
        data_value(serde_json::Value::Object(object)),
    )
}

/// A name for a branch no input reaches (the constant names are valid).
fn unreachable_name() -> EventName {
    EventName::new("websocket".to_owned()).unwrap_or_else(|_| unreachable_name())
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 4).
#[must_use]
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let sextets = [triple >> 18, triple >> 12, triple >> 6, triple];
        for (index, sextet) in sextets.into_iter().enumerate() {
            if index <= chunk.len() {
                let digit = BASE64
                    .get((sextet & 0x3f) as usize)
                    .copied()
                    .unwrap_or(b'A');
                out.push(char::from(digit));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 with padding; `None` for anything else.
#[must_use]
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let value = |byte: u8| BASE64.iter().position(|digit| *digit == byte);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.chunks(4).count();
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == chunks;
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let mut triple = 0_u32;
        for (position, byte) in chunk.iter().enumerate() {
            let sextet = if position >= 4 - padding {
                0
            } else {
                u32::try_from(value(*byte)?).ok()?
            };
            triple = (triple << 6) | sextet;
        }
        let [_, high, middle, low] = triple.to_be_bytes();
        let decoded = [high, middle, low];
        out.extend_from_slice(decoded.get(..3 - padding)?);
    }
    Some(out)
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "event-processor",
        path: "../../crates/scxml-wit/wit/scxml",
        generate_all,
    });

    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use exports::stateforward::scxml::event_processor_api::{Guest, ProcessorStart};
    use stateforward::scxml::outbound_http::TextPart;
    use stateforward::scxml::outbound_websocket::{self, Message, WebsocketRequest};
    use stateforward::scxml::types::{
        ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType, ProcessorSend,
    };

    use super::{Frame, MapError};

    type Closed = Option<ComponentError>;

    // Tasks. The instance's work runs as tasks of `open`'s task: the
    // writer of the session's events, the bell, and one task (plus its
    // reader) per connection. A `<send>` runs in a task of its own, and two
    // tasks never wake each other through a Rust waker: a `send` rings the
    // bell (an intra-component byte stream) to wake `open`'s task, and its
    // connection writes its outcome to the `<send>`'s own byte stream.

    /// Where one `<send>` waits for its outcome.
    struct Slot {
        outcome: Option<Result<(), ComponentError>>,
        /// Written once the outcome is set; the `<send>` reads it.
        done: Option<wit_bindgen::StreamWriter<u8>>,
    }

    type SharedSlot = Rc<RefCell<Slot>>;

    /// One message waiting for its connection, and the slot its `<send>`
    /// waits on.
    struct Queued {
        /// The message, or `None` to close the connection.
        frame: Option<Frame>,
        slot: SharedSlot,
    }

    /// One connection, by target URL.
    #[derive(Default)]
    struct Connection {
        queue: VecDeque<Queued>,
        /// The connection ended: its writer stops.
        ended: bool,
        waker: Option<Waker>,
    }

    type Connections = BTreeMap<String, Rc<RefCell<Connection>>>;

    /// The instance: its events for the session, and its connections.
    #[derive(Default)]
    struct State {
        events: VecDeque<Event>,
        events_waker: Option<Waker>,
        /// Connections the manager has not started yet.
        starting: VecDeque<String>,
        manager_waker: Option<Waker>,
        connections: Connections,
        /// Rung by a `send` after it queues work; `None` while one is
        /// ringing, whose ring covers what was queued meanwhile.
        bell: Option<wit_bindgen::StreamWriter<u8>>,
        /// The future the session's processor end waits on; never written,
        /// so the instance stays open as long as its session.
        _closed: Option<wit_bindgen::FutureWriter<Closed>>,
    }

    thread_local! {
        static STATE: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
    }

    fn state() -> Option<Rc<RefCell<State>>> {
        STATE.with_borrow(Clone::clone)
    }

    /// Wakes a task of `open`'s task, from `open`'s task.
    fn wake(waker: &mut Option<Waker>) {
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }

    /// Spawns `task` as a task of the current one and wakes the current
    /// one, so `task` is polled before it sleeps: a task spawned from a task
    /// that then waits is otherwise not polled until something else wakes it.
    async fn spawn(task: impl std::future::Future<Output = ()> + 'static) {
        wit_bindgen::spawn(task);
        std::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            Poll::Ready(())
        })
        .await;
    }

    fn push_event(
        state: &Rc<RefCell<State>>,
        origin: &str,
        (name, data): (scxml_types::EventName, scxml_types::DataValue),
    ) {
        let mut state = state.borrow_mut();
        state.events.push_back(Event {
            name: name.as_str().to_owned(),
            event_type: EventType::External,
            send_id: None,
            origin: Some(origin.to_owned()),
            origin_type: None,
            invoke_id: None,
            data: wit_value(data),
            raw: None,
            trace_context: None,
        });
        wake(&mut state.events_waker);
    }

    /// Wakes every task of `open`'s task that waits on what a `send`
    /// queues.
    fn wake_all(state: &Rc<RefCell<State>>) {
        let mut state = state.borrow_mut();
        wake(&mut state.manager_waker);
        for connection in state.connections.values() {
            wake(&mut connection.borrow_mut().waker);
        }
    }

    struct Component;

    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the WIT functions are `async func`s; `open` returns once its tasks are spawned"
    )]
    impl Guest for Component {
        async fn open(
            start: ProcessorStart,
        ) -> Result<
            (
                wit_bindgen::StreamReader<Event>,
                wit_bindgen::FutureReader<Closed>,
            ),
            ComponentError,
        > {
            if start.type_uri != super::WEBSOCKET_PROCESSOR_URI {
                return Err(error(ErrorCode::InvalidInput, "processor type URI differs"));
            }
            let (mut events, events_reader) = wit_stream::new::<Event>();
            let (closed, closed_reader) = wit_future::new::<Closed>(|| None);
            let (bell, mut rung) = wit_stream::new::<u8>();
            let shared = Rc::new(RefCell::new(State {
                bell: Some(bell),
                _closed: Some(closed),
                ..State::default()
            }));
            STATE.with_borrow_mut(|slot| *slot = Some(Rc::clone(&shared)));
            // The writer: every event for the session, in order.
            let writer = Rc::clone(&shared);
            wit_bindgen::spawn(async move {
                loop {
                    let event = std::future::poll_fn(|cx| {
                        let mut state = writer.borrow_mut();
                        if let Some(event) = state.events.pop_front() {
                            Poll::Ready(event)
                        } else {
                            state.events_waker = Some(cx.waker().clone());
                            Poll::Pending
                        }
                    })
                    .await;
                    if events.write_one(event).await.is_some() {
                        return;
                    }
                }
            });
            // The bell: each ring wakes what waits on a `send`'s work.
            let ringer = Rc::clone(&shared);
            wit_bindgen::spawn(async move {
                while rung.next().await.is_some() {
                    wake_all(&ringer);
                }
            });
            // The manager: starts each connection as a task of this
            // instance, so it outlives the `<send>` that opened it.
            let manager = Rc::clone(&shared);
            wit_bindgen::spawn(async move {
                loop {
                    let url = std::future::poll_fn(|cx| {
                        let mut state = manager.borrow_mut();
                        if let Some(url) = state.starting.pop_front() {
                            Poll::Ready(url)
                        } else {
                            state.manager_waker = Some(cx.waker().clone());
                            Poll::Pending
                        }
                    })
                    .await;
                    let shared = Rc::clone(&manager);
                    spawn(async move { run(shared, url).await }).await;
                }
            });
            Ok((events_reader, closed_reader))
        }

        async fn check(request: ProcessorSend) -> Result<(), ComponentError> {
            frame(&request).map(|_| ())
        }

        async fn send(request: ProcessorSend) -> Result<(), ComponentError> {
            let (url, frame) = frame(&request)?;
            let shared =
                state().ok_or_else(|| error(ErrorCode::Protocol, "the instance is not open"))?;
            let (done, mut settled) = wit_stream::new::<u8>();
            let slot = Rc::new(RefCell::new(Slot {
                outcome: None,
                done: Some(done),
            }));
            let bell = {
                let mut state = shared.borrow_mut();
                let connection = match state.connections.get(&url) {
                    Some(connection) => Rc::clone(connection),
                    // Nothing to close.
                    None if frame.is_none() => return Ok(()),
                    None => {
                        let connection = Rc::new(RefCell::new(Connection::default()));
                        state
                            .connections
                            .insert(url.clone(), Rc::clone(&connection));
                        state.starting.push_back(url.clone());
                        connection
                    }
                };
                connection.borrow_mut().queue.push_back(Queued {
                    frame,
                    slot: Rc::clone(&slot),
                });
                state.bell.take()
            };
            if let Some(mut bell) = bell {
                let rang = bell.write_one(0).await.is_none();
                if rang {
                    shared.borrow_mut().bell = Some(bell);
                }
            }
            // The outcome is set before its byte is written; a dropped
            // stream (the instance ended) leaves it unset.
            let _ = settled.next().await;
            let outcome = slot.borrow_mut().outcome.take();
            outcome.unwrap_or_else(|| Err(error(ErrorCode::Unavailable, "the instance ended")))
        }
    }

    /// Settles one queued `<send>`: sets its outcome and writes its byte.
    async fn settle(slot: &SharedSlot, outcome: Result<(), ComponentError>) {
        let done = {
            let mut slot = slot.borrow_mut();
            slot.outcome = Some(outcome);
            slot.done.take()
        };
        if let Some(mut done) = done {
            spawn(async move {
                let _ = done.write_one(0).await;
            })
            .await;
        }
    }

    /// Fails every `<send>` still queued on the connection to `url` and
    /// forgets it, so the next `<send>` to it opens a new one.
    async fn fail_queued(shared: &Rc<RefCell<State>>, url: &str, failure: &ComponentError) {
        let connection = shared.borrow_mut().connections.remove(url);
        if let Some(connection) = connection {
            let queued: Vec<Queued> = connection.borrow_mut().queue.drain(..).collect();
            for queued in queued {
                settle(&queued.slot, Err(failure.clone())).await;
            }
        }
    }

    /// Runs the connection to `url`: opens it, sends what its `<send>`s
    /// queue, delivers what the peer sends, and its end.
    async fn run(shared: Rc<RefCell<State>>, url: String) {
        let Some(connection) = shared.borrow().connections.get(&url).cloned() else {
            return;
        };
        let (mut outgoing, outgoing_reader) = wit_stream::new::<Message>();
        let request = WebsocketRequest {
            url: vec![TextPart::Literal(url.clone())],
            headers: Vec::new(),
            protocols: Vec::new(),
        };
        let (_, mut incoming, end) =
            match outbound_websocket::connect(request, outgoing_reader).await {
                Ok(opened) => opened,
                Err(failure) => {
                    fail_queued(&shared, &url, &failure).await;
                    return;
                }
            };
        // The reader: every message, then the end.
        let reader_shared = Rc::clone(&shared);
        let reader_url = url.clone();
        let reader_connection = Rc::clone(&connection);
        spawn(async move {
            while let Some(message) = incoming.next().await {
                let frame = match message {
                    Message::Text(text) => Frame::Text(text),
                    Message::Binary(bytes) => Frame::Binary(bytes),
                };
                push_event(&reader_shared, &reader_url, super::inbound(frame));
            }
            drop(incoming);
            let (code, reason) = match end.await {
                Ok(Some(frame)) => (frame.code, frame.reason),
                Ok(None) => (1000, String::new()),
                Err(failure) => (super::LOST_CLOSE_CODE, failure.detail.unwrap_or_default()),
            };
            let lost = error(ErrorCode::Unavailable, "the WebSocket connection closed");
            let current = reader_shared
                .borrow()
                .connections
                .get(&reader_url)
                .is_some_and(|known| Rc::ptr_eq(known, &reader_connection));
            if current {
                fail_queued(&reader_shared, &reader_url, &lost).await;
            }
            push_event(&reader_shared, &reader_url, super::closed(code, &reason));
            let mut ended = reader_connection.borrow_mut();
            ended.ended = true;
            wake(&mut ended.waker);
        })
        .await;
        // The writer: each queued message in order.
        loop {
            let queued = std::future::poll_fn(|cx| {
                let mut connection = connection.borrow_mut();
                if let Some(queued) = connection.queue.pop_front() {
                    Poll::Ready(Some(queued))
                } else if connection.ended {
                    Poll::Ready(None)
                } else {
                    connection.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
            .await;
            let Some(Queued { frame, slot }) = queued else {
                return;
            };
            let Some(frame) = frame else {
                // Ending the stream closes the connection (code 1000); the
                // reader still delivers what the peer sends until its close.
                drop(outgoing);
                settle(&slot, Ok(())).await;
                let closing = error(
                    ErrorCode::Unavailable,
                    "the WebSocket connection is closing",
                );
                let current = shared
                    .borrow()
                    .connections
                    .get(&url)
                    .is_some_and(|known| Rc::ptr_eq(known, &connection));
                if current {
                    fail_queued(&shared, &url, &closing).await;
                }
                return;
            };
            let message = match frame {
                Frame::Text(text) => Message::Text(text),
                Frame::Binary(bytes) => Message::Binary(bytes),
            };
            if outgoing.write_one(message).await.is_some() {
                let lost = error(ErrorCode::Unavailable, "the WebSocket connection closed");
                settle(&slot, Err(lost.clone())).await;
                fail_queued(&shared, &url, &lost).await;
                return;
            }
            settle(&slot, Ok(())).await;
        }
    }

    /// The target URL and message of one `<send>`.
    fn frame(request: &ProcessorSend) -> Result<(String, Option<Frame>), ComponentError> {
        let url = super::target_url(request.target.as_deref()).map_err(map_error)?;
        let params = request
            .params
            .iter()
            .map(|param| Ok((param.name.clone(), native_value(param.value.clone())?)))
            .collect::<Result<Vec<_>, ComponentError>>()?;
        let content = request.content.clone().map(native_value).transpose()?;
        if super::is_close(request.event_name.as_deref(), &params, content.as_ref()) {
            return Ok((url.to_owned(), None));
        }
        let frame = super::outbound(request.event_name.as_deref(), &params, content.as_ref())
            .map_err(map_error)?;
        Ok((url.to_owned(), Some(frame)))
    }

    fn map_error(failure: MapError) -> ComponentError {
        match failure {
            MapError::InvalidTarget => error(
                ErrorCode::InvalidInput,
                "a WebSocket send needs a ws:// or wss:// target",
            ),
            MapError::InvalidBinary => error(ErrorCode::InvalidInput, "_scxmlbinary is not base64"),
            MapError::Unrepresentable => {
                error(ErrorCode::InvalidInput, "a sent value has no JSON form")
            }
            MapError::TooLarge => error(
                ErrorCode::PayloadTooLarge,
                "the message exceeds its byte bound",
            ),
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
    use super::*;

    fn text(value: &str) -> DataValue {
        DataValue::String(value.to_owned())
    }

    #[test]
    fn targets_are_ws_or_wss_urls_with_a_host() {
        assert_eq!(
            target_url(Some("wss://example.com/v1")),
            Ok("wss://example.com/v1")
        );
        assert_eq!(target_url(Some("WS://example.com")), Ok("WS://example.com"));
        for bad in [
            None,
            Some("https://example.com"),
            Some("wss://"),
            Some("wss:///x"),
            Some("wss://a b"),
        ] {
            assert_eq!(target_url(bad), Err(MapError::InvalidTarget), "{bad:?}");
        }
    }

    #[test]
    fn content_is_sent_as_text_or_json() {
        assert_eq!(
            outbound(Some("ignored"), &[], Some(&text("hello"))),
            Ok(Frame::Text("hello".into()))
        );
        let object = DataValue::canonical_json_object(r#"{"b":1,"a":true}"#).expect("object");
        assert_eq!(
            outbound(None, &[], Some(&object)),
            Ok(Frame::Text(r#"{"a":true,"b":1}"#.into()))
        );
        let number = DataValue::Number(FiniteNumber::new(2.5).expect("finite"));
        assert_eq!(
            outbound(None, &[], Some(&number)),
            Ok(Frame::Text("2.5".into()))
        );
        // Numbers are written as JSON.stringify writes them.
        for (value, written) in [
            (1.0, "1"),
            (-0.0, "0"),
            (9_007_199_254_740_991.0, "9007199254740991"),
            (1e300, "1e+300"),
        ] {
            let number = DataValue::Number(FiniteNumber::new(value).expect("finite"));
            assert_eq!(
                outbound(None, &[], Some(&number)),
                Ok(Frame::Text(written.into())),
                "{value}"
            );
        }
    }

    #[test]
    fn params_become_a_json_object_naming_the_event() {
        let params = vec![
            ("speaking".to_owned(), DataValue::Bool(true)),
            ("type".to_owned(), text("agent")),
            ("gone".to_owned(), DataValue::Absent),
        ];
        assert_eq!(
            outbound(Some("agent.state"), &params, None),
            Ok(Frame::Text(
                r#"{"_scxmleventname":"agent.state","speaking":true,"type":"agent"}"#.into()
            ))
        );
        assert_eq!(outbound(None, &[], None), Ok(Frame::Text("{}".into())));
    }

    #[test]
    fn a_lone_binary_param_is_a_binary_message() {
        let params = vec![(BINARY_MEMBER.to_owned(), text("AAEC/w=="))];
        assert_eq!(
            outbound(Some("audio"), &params, None),
            Ok(Frame::Binary(vec![0, 1, 2, 255]))
        );
        let bad = vec![(BINARY_MEMBER.to_owned(), text("not base64"))];
        assert_eq!(outbound(None, &bad, None), Err(MapError::InvalidBinary));
        let number = vec![(BINARY_MEMBER.to_owned(), DataValue::Null)];
        assert_eq!(outbound(None, &number, None), Err(MapError::InvalidBinary));
    }

    #[test]
    fn messages_past_the_bound_are_refused() {
        let big = text(&"x".repeat(MAX_MESSAGE_BYTES + 1));
        assert_eq!(outbound(None, &[], Some(&big)), Err(MapError::TooLarge));
    }

    #[test]
    fn received_messages_become_events() {
        let (name, data) = inbound(Frame::Text(
            r#"{"_scxmleventname":"speech.partial","text":"hi"}"#.into(),
        ));
        assert_eq!(name.as_str(), "speech.partial");
        assert_eq!(data, DataValue::JsonObject(r#"{"text":"hi"}"#.into()));

        let (name, data) = inbound(Frame::Text(r#"{"type":"final","text":"hi"}"#.into()));
        assert_eq!(name.as_str(), MESSAGE_EVENT);
        assert_eq!(
            data,
            DataValue::JsonObject(r#"{"text":"hi","type":"final"}"#.into())
        );

        let (name, data) = inbound(Frame::Text(r#"{"_scxmleventname":"bad name!"}"#.into()));
        assert_eq!(name.as_str(), MESSAGE_EVENT);
        assert_eq!(
            data,
            DataValue::JsonObject(r#"{"_scxmleventname":"bad name!"}"#.into())
        );

        assert_eq!(inbound(Frame::Text("plain".into())).1, text("plain"));
        assert_eq!(
            inbound(Frame::Text("[1,2]".into())).1,
            DataValue::JsonArray("[1,2]".into())
        );
        let (name, data) = inbound(Frame::Binary(vec![0, 1, 2, 255]));
        assert_eq!(name.as_str(), MESSAGE_EVENT);
        assert_eq!(
            data,
            DataValue::JsonObject(r#"{"_scxmlbinary":"AAEC/w=="}"#.into())
        );
    }

    #[test]
    fn only_a_bare_close_event_closes() {
        assert!(is_close(Some(CLOSE_EVENT), &[], None));
        assert!(is_close(
            Some(CLOSE_EVENT),
            &[],
            Some(&DataValue::Undefined)
        ));
        assert!(!is_close(Some(CLOSE_EVENT), &[], Some(&text("bye"))));
        assert!(!is_close(
            Some(CLOSE_EVENT),
            &[("code".into(), text("1000"))],
            None
        ));
        assert!(!is_close(Some("close"), &[], None));
        assert!(!is_close(None, &[], None));
    }

    #[test]
    fn a_close_carries_its_code_and_reason() {
        let (name, data) = closed(1006, "lost");
        assert_eq!(name.as_str(), CLOSE_EVENT);
        assert_eq!(
            data,
            DataValue::JsonObject(r#"{"code":1006,"reason":"lost"}"#.into())
        );
    }

    #[test]
    fn base64_round_trips_and_rejects_malformed_text() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(base64_decode(&base64_encode(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        for bad in ["Zm8", "Zm=8", "Z===", "Zm8=Zm8=", "Zm 8"] {
            assert_eq!(base64_decode(bad), None, "{bad}");
        }
    }
}
