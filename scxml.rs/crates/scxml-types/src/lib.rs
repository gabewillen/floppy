//! Core value types shared by SCXML syntax, engines, runtimes, and components.
//!
//! These types stay independent of serialization and concrete WebAssembly
//! runtimes. WIT in `scxml-wit` is the canonical deployed contract.
//!
//! # Example
//!
//! Parse an untrusted `<send delay>` designation with the canonical parser:
//!
//! ```
//! use scxml_types::{DelayError, parse_delay_ms};
//!
//! assert_eq!(parse_delay_ms("1.5s"), Ok(1_500));
//! assert_eq!(parse_delay_ms("250ms"), Ok(250));
//! assert_eq!(parse_delay_ms("-1s"), Err(DelayError::Negative));
//! assert_eq!(parse_delay_ms("10"), Err(DelayError::Malformed));
//! ```

#![forbid(unsafe_code)]

use std::fmt;

pub mod wasi;

pub use wasi::{
    FilesystemAccess, MAX_WASI_MOUNT_BYTES, MAX_WASI_PERMISSIONS, WasiInterface, WasiPermission,
    WasiPermissionError, check_wasi_permissions, is_wasi_mount, wasi_permissions_canonical_field,
    wasi_permissions_from_json,
};

/// An implementation resource bound that applies only to wasm builds.
///
/// Returns `bound` when compiled for `wasm32` (the shipped components, whose
/// linear memory and stack are small and fixed) and `usize::MAX`, so no
/// bound, in every native build: a native validator, compiler, or host never
/// rejects a document or ends a session because of a bound it imposes itself.
/// Resource bounds are declared through this function. Format widths,
/// published schema contracts, native stack-depth guards, and guards against
/// super-linear amplification are not resource bounds and do not use it.
///
/// ```
/// let bound = scxml_types::wasm_bound(16_384);
/// if cfg!(target_arch = "wasm32") {
///     assert_eq!(bound, 16_384);
/// } else {
///     assert_eq!(bound, usize::MAX);
/// }
/// ```
#[must_use]
pub const fn wasm_bound(bound: usize) -> usize {
    if cfg!(target_arch = "wasm32") {
        bound
    } else {
        usize::MAX
    }
}

/// Whether `value` is over `bound`, a resource bound declared through
/// [`wasm_bound`].
///
/// Every comparison with such a bound is written through this function.
/// Natively the bound is the type's maximum, so no value is over it and the
/// comparison is always false: that is the bound applying to wasm builds
/// only, not a mistake, and keeping the comparison here, against a
/// parameter, keeps every caller free of a comparison with an extreme value.
///
/// ```
/// let bound = scxml_types::wasm_bound(4);
/// assert!(!scxml_types::exceeds_wasm_bound(4, bound));
/// assert_eq!(
///     scxml_types::exceeds_wasm_bound(5, bound),
///     cfg!(target_arch = "wasm32")
/// );
/// ```
#[must_use]
pub fn exceeds_wasm_bound<T: PartialOrd + Copy>(value: T, bound: T) -> bool {
    value > bound
}

/// Whether `character` is XML white space (XML 1.0 §2.3 `S`): #x20, #x9,
/// #xD, or #xA. Other Unicode white space (such as U+00A0, or U+1680, an
/// XML name character) is not.
#[must_use]
pub const fn is_xml_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r')
}

/// Version of the canonical delayed-send request byte format.
///
/// The version lives only in the little-endian `u32` that follows
/// [`DELAYED_SEND_MAGIC`]; the magic tag never changes between versions.
pub const DELAYED_SEND_WIRE_VERSION: u32 = 8;
/// The `env` attribute vocabulary: an executable-content extension element
/// may carry `env:<slot>="NAME"` or `env:<slot>expr="EXPR"` attributes in this
/// exact namespace, naming the variable whose value its host binds to the
/// slot (as an opaque reference, never the value). The host interprets it;
/// no package, schema, or component is bound to it, and it is never fetched
/// (it is not a repository namespace URI: it names no release tag).
pub const ENV_NAMESPACE_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/env";
/// The `event` attribute vocabulary: `<transition>`, `<send>`, `<raise>`, and
/// `<donedata>` may carry `event:schema` (a JSON Schema, inline or by
/// reference) or `event:schemaexpr` (an expression whose value is the
/// schema) in this exact namespace, describing the event data the element
/// handles or creates. It is metadata only: it never changes how a document
/// executes. The compiler resolves it into the Machine IR; no package,
/// schema, or component is bound to it, and it is never fetched (it is not a
/// repository namespace URI: it names no release tag).
pub const EVENT_NAMESPACE_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/event";
/// Maximum canonical bytes retained for one delayed send request.
pub const MAX_DELAYED_SEND_BYTES: usize = 65_536;
/// Maximum entries in one `<send>` message field list
/// ([`SendMessage::Params`]); matches the WIT `named-value` list bound.
pub const MAX_SEND_MESSAGE_PARAMS: usize = 1_024;
/// Exact byte length of the supported W3C `traceparent` version.
pub const TRACEPARENT_BYTES: usize = 55;
/// Maximum W3C `tracestate` bytes accepted at a component boundary.
pub const MAX_TRACESTATE_BYTES: usize = 512;
/// Maximum W3C `tracestate` list members accepted at a component boundary.
pub const MAX_TRACESTATE_MEMBERS: usize = 32;
/// Maximum W3C baggage bytes accepted when propagation is explicitly enabled.
pub const MAX_BAGGAGE_BYTES: usize = 8_192;
/// Maximum baggage list members accepted when propagation is explicitly enabled.
pub const MAX_BAGGAGE_MEMBERS: usize = 64;
/// Maximum UTF-8 bytes accepted by SCXML name and identifier value types.
pub const MAX_NAME_BYTES: usize = 1_024;
/// Maximum UTF-8 bytes accepted for one event metadata string.
pub const MAX_EVENT_METADATA_BYTES: usize = 16_384;
/// Maximum UTF-8 bytes accepted for one event text, JSON, or XML payload.
pub const MAX_EVENT_DATA_BYTES: usize = 65_536;
/// Maximum aggregate bytes accepted for one native event.
pub const MAX_EVENT_BYTES: usize = 96 * 1_024;

/// Version-independent format tag that opens every delayed-send wire record.
///
/// Decoders reject any other tag as [`SendCodecError::Invalid`] and any other
/// version under this tag as [`SendCodecError::UnsupportedVersion`].
pub const DELAYED_SEND_MAGIC: &[u8; 8] = b"SCXSEND\0";

macro_rules! name_type {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Constructs a validated value.
            ///
            /// # Errors
            ///
            /// Returns [`NameError`] when `value` is empty, contains whitespace,
            /// or exceeds [`MAX_NAME_BYTES`].
            pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
                validate_name(value.into()).map(Self)
            }

            /// Returns the value as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

name_type!(StateId, "Identifies a state in a compiled state machine.");
name_type!(
    InvokeId,
    "Identifies an invocation managed by an SCXML session."
);
name_type!(SessionId, "Identifies a running SCXML session.");
name_type!(EventName, "A validated SCXML event name.");
name_type!(SendId, "Identifies a delayed or cancellable send request.");

/// A canonical XML namespace URI used for exact registry lookup.
///
/// Canonical means that the producer supplies the stable URI identifier for the
/// component. The runtime deliberately performs no URI rewriting, prefix lookup,
/// case folding, or trailing-slash normalization.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NamespaceUri(String);

impl NamespaceUri {
    /// Constructs a non-empty absolute URI-like namespace identifier.
    ///
    /// # Errors
    ///
    /// Returns [`NamespaceUriError`] for empty, whitespace-containing, or
    /// relative values.
    pub fn new(value: impl Into<String>) -> Result<Self, NamespaceUriError> {
        let value = value.into();
        if value.is_empty() {
            return Err(NamespaceUriError::Empty);
        }
        if value.chars().any(char::is_whitespace) {
            return Err(NamespaceUriError::ContainsWhitespace);
        }
        let Some(separator) = value.find(':') else {
            return Err(NamespaceUriError::Relative);
        };
        let scheme = &value[..separator];
        if scheme.is_empty()
            || !scheme.chars().enumerate().all(|(index, character)| {
                (index == 0 && character.is_ascii_alphabetic())
                    || (index > 0
                        && (character.is_ascii_alphanumeric()
                            || matches!(character, '+' | '-' | '.')))
            })
        {
            return Err(NamespaceUriError::Relative);
        }
        Ok(Self(value))
    }

    /// Returns the exact canonical URI text used as the registry key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NamespaceUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn validate_name(value: String) -> Result<String, NameError> {
    if value.is_empty() {
        Err(NameError::Empty)
    } else if value.len() > MAX_NAME_BYTES {
        Err(NameError::TooLong)
    } else if value.contains(is_xml_space) {
        // XML white space (XML 1.0 §2.3 `S`) separates names; other
        // Unicode white space, such as U+1680, may be a name character.
        Err(NameError::ContainsWhitespace)
    } else {
        Ok(value)
    }
}

/// Whether a document loads any `<script src>`, whose source a parse-only
/// analysis of the document cannot read (the WIT `external-scripts` of
/// `check-effects`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalScripts {
    /// Every script is inline.
    Absent,
    /// At least one script is loaded from `src`.
    Present,
}

/// Failure to construct a name-like SCXML value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameError {
    /// The value was empty.
    Empty,
    /// The value exceeded [`MAX_NAME_BYTES`] UTF-8 bytes.
    TooLong,
    /// The value contained whitespace.
    ContainsWhitespace,
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("name must not be empty"),
            Self::TooLong => formatter.write_str("name exceeds its UTF-8 byte bound"),
            Self::ContainsWhitespace => formatter.write_str("name must not contain whitespace"),
        }
    }
}

impl std::error::Error for NameError {}

/// Failure to construct a canonical namespace URI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamespaceUriError {
    /// The value was empty.
    Empty,
    /// The value contained whitespace.
    ContainsWhitespace,
    /// The value did not contain a valid absolute-URI scheme.
    Relative,
}

impl fmt::Display for NamespaceUriError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("namespace URI must not be empty"),
            Self::ContainsWhitespace => {
                formatter.write_str("namespace URI must not contain whitespace")
            }
            Self::Relative => formatter.write_str("namespace URI must be absolute"),
        }
    }
}

impl std::error::Error for NamespaceUriError {}

/// W3C SCXML event classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventType {
    /// Raised by the interpreter or an invoked service.
    Internal,
    /// Received through an I/O processor.
    External,
    /// A platform-defined event.
    Platform,
}

/// W3C trace context propagated with an SCXML event across component boundaries.
///
/// Hosts validate and inject these values into their OTEL context. They are
/// transport metadata and must never affect SCXML transition selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    /// W3C `traceparent` header value.
    pub traceparent: String,
    /// Optional W3C `tracestate` header value.
    pub tracestate: Option<String>,
    /// Optional W3C baggage header value.
    pub baggage: Option<String>,
    /// Explicit policy controlling whether baggage may cross the next boundary.
    pub baggage_policy: BaggagePolicy,
}

/// Policy for potentially sensitive W3C baggage propagation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BaggagePolicy {
    /// Discard baggage at ingress and propagate only trace identity/state.
    Drop,
    /// Preserve validated, bounded baggage across component boundaries.
    Propagate,
}

/// Failure to validate distributed trace transport metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceContextError {
    /// The `traceparent` is not canonical W3C Trace Context Level 2 syntax.
    InvalidTraceparent,
    /// The `tracestate` is malformed or exceeds its byte/member bounds.
    InvalidTracestate,
    /// Propagated baggage is malformed or exceeds its byte/member bounds.
    InvalidBaggage,
    /// A drop-policy context retained baggage and is therefore noncanonical.
    BaggagePolicyViolation,
}

impl TraceContext {
    /// Constructs canonical bounded trace metadata and applies baggage policy.
    ///
    /// `Drop` removes baggage before the value can be retained. `Propagate`
    /// validates baggage syntax and bounds before retaining it.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for malformed or over-bound transport fields.
    pub fn new(
        traceparent: impl Into<String>,
        tracestate: Option<String>,
        baggage: Option<String>,
        baggage_policy: BaggagePolicy,
    ) -> Result<Self, TraceContextError> {
        let traceparent = traceparent.into();
        validate_traceparent(&traceparent)?;
        if let Some(value) = tracestate.as_deref() {
            validate_tracestate(value)?;
        }
        let baggage = match baggage_policy {
            BaggagePolicy::Drop => None,
            BaggagePolicy::Propagate => {
                if let Some(value) = baggage.as_deref() {
                    validate_baggage_list(value, MAX_BAGGAGE_BYTES, MAX_BAGGAGE_MEMBERS)
                        .map_err(|()| TraceContextError::InvalidBaggage)?;
                }
                baggage
            }
        };
        Ok(Self {
            traceparent,
            tracestate,
            baggage,
            baggage_policy,
        })
    }

    /// Validates that this publicly constructible value is already canonical.
    ///
    /// # Errors
    ///
    /// Returns [`TraceContextError::BaggagePolicyViolation`] when a drop-policy
    /// value still contains baggage, or another typed syntax/bounds failure.
    pub fn validate(&self) -> Result<(), TraceContextError> {
        validate_traceparent(&self.traceparent)?;
        if let Some(value) = self.tracestate.as_deref() {
            validate_tracestate(value)?;
        }
        match (self.baggage_policy, self.baggage.as_deref()) {
            (BaggagePolicy::Drop, Some(_)) => Err(TraceContextError::BaggagePolicyViolation),
            (BaggagePolicy::Propagate, Some(value)) => {
                validate_baggage_list(value, MAX_BAGGAGE_BYTES, MAX_BAGGAGE_MEMBERS)
                    .map_err(|()| TraceContextError::InvalidBaggage)
            }
            (BaggagePolicy::Drop | BaggagePolicy::Propagate, None) => Ok(()),
        }
    }
}

impl fmt::Display for TraceContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTraceparent => "traceparent is not canonical W3C Trace Context Level 2",
            Self::InvalidTracestate => "tracestate is malformed or exceeds its bound",
            Self::InvalidBaggage => "baggage is malformed or exceeds its bound",
            Self::BaggagePolicyViolation => "drop-policy trace context retained baggage",
        })
    }
}

impl std::error::Error for TraceContextError {}

fn validate_traceparent(value: &str) -> Result<(), TraceContextError> {
    let bytes = value.as_bytes();
    let canonical = bytes.len() == TRACEPARENT_BYTES
        && bytes.get(0..2) == Some(b"00")
        && bytes.get(2) == Some(&b'-')
        && bytes.get(35) == Some(&b'-')
        && bytes.get(52) == Some(&b'-')
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 2 | 35 | 52) || byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        })
        && bytes
            .get(3..35)
            .is_some_and(|part| part.iter().any(|byte| *byte != b'0'))
        && bytes
            .get(36..52)
            .is_some_and(|part| part.iter().any(|byte| *byte != b'0'))
        && bytes
            .get(53..55)
            .and_then(|part| core::str::from_utf8(part).ok())
            .and_then(|part| u8::from_str_radix(part, 16).ok())
            .is_some_and(|flags| flags <= 0x03);
    canonical
        .then_some(())
        .ok_or(TraceContextError::InvalidTraceparent)
}

fn validate_tracestate(value: &str) -> Result<(), TraceContextError> {
    let result = (|| {
        if value.is_empty() || value.len() > MAX_TRACESTATE_BYTES || !value.is_ascii() {
            return Err(());
        }
        let mut count = 0_usize;
        let mut keys = Vec::with_capacity(MAX_TRACESTATE_MEMBERS);
        for member in value.split(',') {
            count = count.checked_add(1).ok_or(())?;
            if count > MAX_TRACESTATE_MEMBERS || member.is_empty() || member.trim() != member {
                return Err(());
            }
            let (key, item) = member.split_once('=').ok_or(())?;
            if !valid_tracestate_key(key) || !valid_tracestate_value(item) || keys.contains(&key) {
                return Err(());
            }
            keys.push(key);
        }
        Ok(())
    })();
    result.map_err(|()| TraceContextError::InvalidTracestate)
}

fn valid_tracestate_key(key: &str) -> bool {
    if let Some((tenant, system)) = key.split_once('@') {
        !system.contains('@')
            && valid_key_part(tenant, 241, true)
            && valid_key_part(system, 14, false)
    } else {
        valid_key_part(key, 256, true)
    }
}

fn valid_key_part(value: &str, maximum: usize, digit_first: bool) -> bool {
    let mut bytes = value.bytes();
    let first = bytes.next();
    !value.is_empty()
        && value.len() <= maximum
        && first
            .is_some_and(|byte| byte.is_ascii_lowercase() || (digit_first && byte.is_ascii_digit()))
        && bytes.all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'*' | b'/')
        })
}

fn valid_tracestate_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes().last().is_some_and(|byte| *byte != b' ')
        && value
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte) && !matches!(byte, b',' | b'='))
}

fn validate_baggage_list(value: &str, max_bytes: usize, max_members: usize) -> Result<(), ()> {
    if value.is_empty() || value.len() > max_bytes || !value.is_ascii() {
        return Err(());
    }
    let mut count = 0_usize;
    for member in value.split(',') {
        count = count.checked_add(1).ok_or(())?;
        if count > max_members || member.is_empty() || member.trim() != member {
            return Err(());
        }
        let (key, item) = member.split_once('=').ok_or(())?;
        if key.is_empty()
            || item.is_empty()
            || !key.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'-' | b'.' | b'*' | b'/' | b'@')
            })
            || !item
                .bytes()
                .all(|byte| (0x20..=0x7e).contains(&byte) && byte != b',')
        {
            return Err(());
        }
    }
    Ok(())
}

/// An SCXML event crossing the interpreter boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    /// Event name used for transition matching.
    pub name: EventName,
    /// Event classification.
    pub event_type: EventType,
    /// Identifier of the `<send>` operation that produced this event, when applicable.
    pub send_id: Option<SendId>,
    /// Origin URI when supplied by an I/O processor.
    pub origin: Option<String>,
    /// I/O processor URI or alias when known.
    pub origin_type: Option<String>,
    /// Invocation that produced the event, when applicable.
    pub invoke_id: Option<InvokeId>,
    /// Typed data-model value; [`DataValue::Absent`] means no payload.
    pub data: DataValue,
    /// Optional distributed trace context carried without changing event semantics.
    pub trace_context: Option<TraceContext>,
    /// Raw I/O-processor message text the event was constructed from, when
    /// the delivering processor retains one (SCXML 6.2 `_event.raw`);
    /// `None` for internal, platform, and processor events without a raw
    /// message. Bounded by event validation.
    pub raw: Option<String>,
}

/// Failure to validate an event before it crosses a component or host boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventError {
    /// One string or payload exceeded its class-specific byte bound.
    ValueTooLarge,
    /// Aggregate retained event bytes overflowed or exceeded [`MAX_EVENT_BYTES`].
    AggregateTooLarge,
    /// A publicly constructible field was malformed or noncanonical.
    InvalidValue,
    /// Distributed trace metadata was malformed or noncanonical.
    InvalidTraceContext,
}

impl fmt::Display for EventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ValueTooLarge => "event value exceeds its byte bound",
            Self::AggregateTooLarge => "event aggregate exceeds its byte bound",
            Self::InvalidValue => "event contains an invalid or noncanonical value",
            Self::InvalidTraceContext => "event trace context is invalid",
        })
    }
}

impl std::error::Error for EventError {}

impl Event {
    /// Creates a payload-free external event.
    #[must_use]
    pub fn external(name: EventName) -> Self {
        Self {
            name,
            event_type: EventType::External,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::Absent,
            trace_context: None,
            raw: None,
        }
    }

    /// Constructs one platform-generated event with the supplied name.
    #[must_use]
    pub fn platform(name: EventName) -> Self {
        Self {
            event_type: EventType::Platform,
            ..Self::external(name)
        }
    }

    /// Creates a payload-free internal event.
    #[must_use]
    pub fn internal(name: EventName) -> Self {
        Self {
            event_type: EventType::Internal,
            ..Self::external(name)
        }
    }

    /// Validates all retained fields and checked aggregate byte bounds.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for an oversized, malformed, or noncanonical
    /// event. Validation borrows the event and does not clone its fields.
    pub fn validate(&self) -> Result<(), EventError> {
        let mut total = self.name.as_str().len();
        for value in [self.origin.as_deref(), self.origin_type.as_deref()]
            .into_iter()
            .flatten()
        {
            total = add_event_bytes(total, bounded_event_bytes(value, MAX_EVENT_METADATA_BYTES)?)?;
        }
        for bytes in [
            self.send_id.as_ref().map(SendId::as_str),
            self.invoke_id.as_ref().map(InvokeId::as_str),
        ]
        .into_iter()
        .flatten()
        .map(str::len)
        {
            total = add_event_bytes(total, bytes)?;
        }
        let data_bytes = match &self.data {
            DataValue::Absent | DataValue::Undefined | DataValue::Null => 0,
            DataValue::Bool(_) => 1,
            DataValue::Number(_) => 8,
            DataValue::String(value)
            | DataValue::JsonArray(value)
            | DataValue::JsonObject(value)
            | DataValue::Xml(value) => bounded_event_bytes(value, MAX_EVENT_DATA_BYTES)?,
        };
        total = add_event_bytes(total, data_bytes)?;
        if let Some(raw) = self.raw.as_deref() {
            total = add_event_bytes(total, bounded_event_bytes(raw, MAX_EVENT_DATA_BYTES)?)?;
        }
        if let Some(context) = self.trace_context.as_ref() {
            context
                .validate()
                .map_err(|_| EventError::InvalidTraceContext)?;
            total = add_event_bytes(total, context.traceparent.len())?;
            for value in [context.tracestate.as_deref(), context.baggage.as_deref()]
                .into_iter()
                .flatten()
            {
                total = add_event_bytes(total, value.len())?;
            }
        }
        match &self.data {
            DataValue::JsonArray(value) => match DataValue::canonical_json_array(value) {
                Ok(DataValue::JsonArray(canonical)) if canonical == *value => {}
                _ => return Err(EventError::InvalidValue),
            },
            DataValue::JsonObject(value) => match DataValue::canonical_json_object(value) {
                Ok(DataValue::JsonObject(canonical)) if canonical == *value => {}
                _ => return Err(EventError::InvalidValue),
            },
            DataValue::Absent
            | DataValue::Undefined
            | DataValue::Null
            | DataValue::Bool(_)
            | DataValue::Number(_)
            | DataValue::String(_)
            | DataValue::Xml(_) => {}
        }
        Ok(())
    }
}

fn bounded_event_bytes(value: &str, maximum: usize) -> Result<usize, EventError> {
    (value.len() <= maximum)
        .then_some(value.len())
        .ok_or(EventError::ValueTooLarge)
}

fn add_event_bytes(total: usize, bytes: usize) -> Result<usize, EventError> {
    total
        .checked_add(bytes)
        .filter(|value| *value <= MAX_EVENT_BYTES)
        .ok_or(EventError::AggregateTooLarge)
}

/// Maximum UTF-8 bytes accepted for one `<send>` delay designation.
///
/// The CSS2 time grammar has no length limit; this bound keeps parsing work
/// constant for untrusted `delay`/`delayexpr` values.
pub const MAX_DELAY_BYTES: usize = 64;

/// Failure to parse a W3C SCXML `<send>` delay designation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DelayError {
    /// The value was empty, lacked a `s`/`ms` unit, or did not match the CSS2
    /// `<number>` grammar.
    Malformed,
    /// The value exceeded [`MAX_DELAY_BYTES`].
    TooLong,
    /// The value carried a `-` sign; SCXML delays cannot be negative.
    Negative,
    /// The designated delay does not fit in `u64` milliseconds.
    Overflow,
}

impl fmt::Display for DelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "send delay is not a CSS2 time designation",
            Self::TooLong => "send delay exceeds its byte bound",
            Self::Negative => "send delay must not be negative",
            Self::Overflow => "send delay overflows the millisecond range",
        })
    }
}

impl std::error::Error for DelayError {}

/// Parses one W3C SCXML `<send>` `delay`/`delayexpr` value into milliseconds.
///
/// SCXML 1.0 section 6.2 requires the value to be a CSS2 time designation:
/// a CSS2 `<number>` (`[0-9]+` or `[0-9]*.[0-9]+`, optionally preceded by
/// `+`) immediately followed by the unit `ms` or `s`. Units are required,
/// whitespace is not permitted, and a `-` sign is rejected because a delay
/// cannot be negative. Fractional values are floored to whole milliseconds
/// (`"1.5s"` is 1500, `".0005s"` and `"0.9ms"` are 0). This is the single
/// canonical delay parser; the engine's `<send>` interpretation, which runs
/// inside the machine guest, uses it.
///
/// # Errors
///
/// Returns [`DelayError`] for over-long, malformed, negative, or overflowing
/// values. Parsing is pure, bounded by [`MAX_DELAY_BYTES`], and allocation-free.
pub fn parse_delay_ms(value: &str) -> Result<u64, DelayError> {
    if value.len() > MAX_DELAY_BYTES {
        return Err(DelayError::TooLong);
    }
    let (number, unit_ms) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000_u64)
    } else {
        return Err(DelayError::Malformed);
    };
    if number.starts_with('-') {
        return Err(DelayError::Negative);
    }
    let number = number.strip_prefix('+').unwrap_or(number);
    // CSS2 `<number>`: `[0-9]+` or `[0-9]*.[0-9]+`.
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let has_point = whole.len() != number.len();
    if (has_point && fraction.is_empty()) || (!has_point && whole.is_empty()) {
        return Err(DelayError::Malformed);
    }
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(DelayError::Malformed);
    }
    let mut whole_units = 0_u64;
    for byte in whole.bytes() {
        whole_units = whole_units
            .checked_mul(10)
            .and_then(|total| total.checked_add(u64::from(byte - b'0')))
            .ok_or(DelayError::Overflow)?;
    }
    let mut millis = whole_units
        .checked_mul(unit_ms)
        .ok_or(DelayError::Overflow)?;
    if unit_ms == 1_000 {
        // Fraction digits weigh 100, 10, and 1 ms; later digits are
        // sub-millisecond and floor deterministically.
        for (digit, weight) in fraction.bytes().zip([100_u64, 10, 1]) {
            millis = millis
                .checked_add(u64::from(digit - b'0') * weight)
                .ok_or(DelayError::Overflow)?;
        }
    }
    Ok(millis)
}

/// Failure to split an evaluated `<foreach>` array expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForeachError {
    /// The array expression did not evaluate to a JSON array value; the
    /// variant carries the closed kind that was found instead.
    NotArray(DataValueKind),
    /// The array snapshot exceeded [`MAX_EVENT_DATA_BYTES`].
    TooLarge,
    /// The array snapshot was not valid JSON array text.
    InvalidJson,
    /// An element was a number that cannot be represented as a finite value.
    NonFiniteElement,
    /// A nested array or object element could not be canonicalized.
    InvalidElement,
}

/// Closed classification of a [`DataValue`] variant with no payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataValueKind {
    /// [`DataValue::Absent`].
    Absent,
    /// [`DataValue::Undefined`].
    Undefined,
    /// [`DataValue::Null`].
    Null,
    /// [`DataValue::Bool`].
    Boolean,
    /// [`DataValue::Number`].
    Number,
    /// [`DataValue::String`].
    String,
    /// [`DataValue::JsonArray`].
    Array,
    /// [`DataValue::JsonObject`].
    Object,
    /// [`DataValue::Xml`].
    Xml,
}

impl DataValueKind {
    /// Returns the stable lower-case name of this kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Undefined => "undefined",
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::String => "string",
            Self::Array => "array",
            Self::Object => "object",
            Self::Xml => "xml",
        }
    }
}

impl fmt::Display for ForeachError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotArray(kind) => write!(
                formatter,
                "foreach array expression must evaluate to an array, got {}",
                kind.as_str()
            ),
            Self::TooLarge => formatter.write_str("foreach array snapshot exceeds its byte bound"),
            Self::InvalidJson => formatter.write_str("foreach array snapshot is not a JSON array"),
            Self::NonFiniteElement => formatter.write_str("foreach array element is not finite"),
            Self::InvalidElement => {
                formatter.write_str("foreach array element cannot be canonicalized")
            }
        }
    }
}

impl std::error::Error for ForeachError {}

/// Splits one evaluated `<foreach>` array value into its shallow element
/// snapshot (SCXML 1.0 section 4.6).
///
/// Only [`DataValue::JsonArray`] is iterable. The snapshot is taken once, so
/// body effects that mutate the array cannot change the iteration. Elements
/// map to portable values: JSON `null`, booleans, finite numbers, and strings
/// map to their scalar variants; nested arrays and objects are re-emitted as
/// canonical JSON ([`DataValue::canonical_json_array`] and
/// [`DataValue::canonical_json_object`]). This is the single canonical
/// classifier; the ECMAScript data model's `<foreach>` uses it.
///
/// # Errors
///
/// Returns [`ForeachError`] when the value is not an array, exceeds
/// [`MAX_EVENT_DATA_BYTES`], is not JSON array text, or contains an element
/// that cannot be represented canonically.
pub fn foreach_array_elements(array: &DataValue) -> Result<Vec<DataValue>, ForeachError> {
    let source = match array {
        DataValue::JsonArray(source) => source,
        DataValue::Absent => return Err(ForeachError::NotArray(DataValueKind::Absent)),
        DataValue::Undefined => return Err(ForeachError::NotArray(DataValueKind::Undefined)),
        DataValue::Null => return Err(ForeachError::NotArray(DataValueKind::Null)),
        DataValue::Bool(_) => return Err(ForeachError::NotArray(DataValueKind::Boolean)),
        DataValue::Number(_) => return Err(ForeachError::NotArray(DataValueKind::Number)),
        DataValue::String(_) => return Err(ForeachError::NotArray(DataValueKind::String)),
        DataValue::JsonObject(_) => return Err(ForeachError::NotArray(DataValueKind::Object)),
        DataValue::Xml(_) => return Err(ForeachError::NotArray(DataValueKind::Xml)),
    };
    if source.len() > MAX_EVENT_DATA_BYTES {
        return Err(ForeachError::TooLarge);
    }
    let parsed: serde_json::Value =
        serde_json::from_str(source).map_err(|_| ForeachError::InvalidJson)?;
    let serde_json::Value::Array(items) = parsed else {
        return Err(ForeachError::InvalidJson);
    };
    let mut elements = Vec::with_capacity(items.len());
    for item in items {
        let value = match &item {
            serde_json::Value::Null => DataValue::Null,
            serde_json::Value::Bool(value) => DataValue::Bool(*value),
            serde_json::Value::Number(number) => number
                .as_f64()
                .and_then(|value| FiniteNumber::new(value).ok())
                .map(DataValue::Number)
                .ok_or(ForeachError::NonFiniteElement)?,
            serde_json::Value::String(value) => DataValue::String(value.clone()),
            serde_json::Value::Array(_) => serde_json::to_string(&item)
                .ok()
                .and_then(|encoded| DataValue::canonical_json_array(&encoded).ok())
                .ok_or(ForeachError::InvalidElement)?,
            serde_json::Value::Object(_) => serde_json::to_string(&item)
                .ok()
                .and_then(|encoded| DataValue::canonical_json_object(&encoded).ok())
                .ok_or(ForeachError::InvalidElement)?,
        };
        elements.push(value);
    }
    Ok(elements)
}

/// One named payload entry (`<param>` or `namelist` item) of a `<send>`,
/// `<invoke>`, or `<donedata>`, in document order (WIT `named-value`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedValue {
    /// Entry name exactly as authored.
    pub name: String,
    /// Evaluated entry value.
    pub value: DataValue,
}

/// Body of one SCXML Event I/O Processor message (WIT `send-message`); the
/// processor derives `_event.raw` from it (SCXML 5.10.1, C.1).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SendMessage {
    /// No `<param>`, `namelist`, or `<content>`.
    #[default]
    Empty,
    /// `<param>` and `namelist` entries in document order, duplicates kept
    /// (at most [`MAX_SEND_MESSAGE_PARAMS`]).
    Params(Vec<NamedValue>),
    /// A `<content>` payload; its value is the event's `data`.
    Content,
}

/// A request emitted by `<send>` for delivery through an I/O processor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendRequest {
    /// Interpreter-owned send identifier: required for a delayed send (its
    /// clock and `<cancel>` identity), and present whenever the event
    /// carries one.
    ///
    /// [`Event::send_id`] is absent or equal to it (see
    /// [`send_ids_agree`]): the event carries the id only when the author
    /// gave `id` or `idlocation` (SCXML C.1), while a delayed send without
    /// either still needs a generated one. Canonical encoders and I/O
    /// processor boundaries reject any other pairing before delivery.
    pub send_id: Option<SendId>,
    /// Event to deliver.
    pub event: Event,
    /// Target URI. Absence asks the runtime to use the processor default.
    pub target: Option<String>,
    /// I/O processor type. Absence selects the SCXML Event I/O Processor.
    pub processor_type: Option<String>,
    /// Delay in milliseconds before delivery.
    pub delay_ms: u64,
    /// Message body the processor renders `_event.raw` from.
    pub message: SendMessage,
}

/// Canonical SCXML Event I/O Processor type URI.
pub const SCXML_EVENT_PROCESSOR_URI: &str = "http://www.w3.org/TR/scxml/#SCXMLEventProcessor";
/// Short alias SCXML documents may use for the SCXML Event I/O Processor.
pub const SCXML_PROCESSOR_ALIAS: &str = "scxml";
/// Type URI of the optional `BasicHTTP` Event I/O Processor (SCXML Appendix
/// C.2), implemented by the separate `scxml-ioprocessor-basichttp`
/// component.
pub const BASICHTTP_EVENT_PROCESSOR_URI: &str =
    "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor";
/// Short alias of the `BasicHTTP` Event I/O Processor.
pub const BASICHTTP_PROCESSOR_ALIAS: &str = "basichttp";

/// Maximum UTF-8 bytes of a [`BasicHttpSend`] event name (WIT
/// `basichttp-send.event-name`).
pub const MAX_BASICHTTP_EVENT_NAME_BYTES: usize = 1_024;
/// Maximum UTF-8 bytes of a [`BasicHttpSend`] target (WIT
/// `basichttp-send.target`).
pub const MAX_BASICHTTP_TARGET_BYTES: usize = 16_384;
/// Maximum [`BasicHttpSend`] fields (WIT `basichttp-send.params`).
pub const MAX_BASICHTTP_PARAMS: usize = 256;

/// One evaluated `<send>` addressed to the `BasicHTTP` Event I/O Processor
/// (SCXML C.2; WIT `basichttp-send`). The machine evaluates every
/// expression; the processor never sees one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasicHttpSend {
    /// The send's id: the authored `id`, else the one the machine generated
    /// (SCXML 3.14, 6.2.4). It never enters the HTTP message; a delayed send
    /// is scheduled and cancelled under it (SCXML 6.2, 6.3), and an
    /// `error.communication` for it carries it (SCXML 5.10.1).
    pub send_id: SendId,
    /// Evaluated `event`/`eventexpr` (at most
    /// [`MAX_BASICHTTP_EVENT_NAME_BYTES`]).
    pub event_name: Option<String>,
    /// Evaluated `target`/`targetexpr` (at most
    /// [`MAX_BASICHTTP_TARGET_BYTES`]).
    pub target: Option<String>,
    /// `namelist` entries then `<param>` children, in document order (at
    /// most [`MAX_BASICHTTP_PARAMS`]).
    pub params: Vec<NamedValue>,
    /// Evaluated `<content>`; mutually exclusive with `params`.
    pub content: Option<DataValue>,
    /// Delay in whole milliseconds.
    pub delay_ms: u64,
    /// Optional trace carrier; never changes semantics.
    pub trace_context: Option<TraceContext>,
}

/// One delayed send the SCXML Event I/O Processor holds on the project clock
/// until its delay elapses (SCXML 6.2: "It MUST dispatch the message only
/// when the delay interval elapses."), whichever Event I/O Processor then
/// dispatches it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DelayedSend {
    /// A send of the SCXML Event I/O Processor (SCXML C.1).
    Scxml(SendRequest),
    /// A send of the `BasicHTTP` Event I/O Processor (SCXML C.2).
    BasicHttp(BasicHttpSend),
}

impl DelayedSend {
    /// The send id the clock timer carries: the request's for an SCXML
    /// send (absent only for an undelayed one), the send's own for a
    /// `BasicHTTP` send.
    #[must_use]
    pub fn send_id(&self) -> Option<&SendId> {
        match self {
            Self::Scxml(request) => request.send_id.as_ref(),
            Self::BasicHttp(send) => Some(&send.send_id),
        }
    }

    /// The delay in whole milliseconds.
    #[must_use]
    pub const fn delay_ms(&self) -> u64 {
        match self {
            Self::Scxml(request) => request.delay_ms,
            Self::BasicHttp(send) => send.delay_ms,
        }
    }
}

/// Closed classification of one `<send>` processor type value. This is the
/// single source of truth for processor-type spelling: validators,
/// capability gates, runners, and the component itself classify through it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessorKind {
    /// The SCXML Event I/O Processor (URI or `scxml` alias).
    Scxml,
    /// The optional `BasicHTTP` Event I/O Processor (URI or `basichttp` alias),
    /// provided by the `scxml-ioprocessor-basichttp` component.
    BasicHttp,
    /// Any other value: an invalid processor identifier (SCXML 6.2 raises
    /// `error.execution`).
    Unknown,
}

/// Classifies one exact processor type value (no case folding or trimming).
#[must_use]
pub fn classify_processor_type(value: &str) -> ProcessorKind {
    match value {
        SCXML_PROCESSOR_ALIAS | SCXML_EVENT_PROCESSOR_URI => ProcessorKind::Scxml,
        BASICHTTP_PROCESSOR_ALIAS | BASICHTTP_EVENT_PROCESSOR_URI => ProcessorKind::BasicHttp,
        _ => ProcessorKind::Unknown,
    }
}

/// `<invoke>` type URI of an SCXML invoked session (SCXML 6.4).
pub const SCXML_INVOKE_TYPE_URI: &str = "http://www.w3.org/TR/scxml/";

/// Whether one exact `<invoke>` type value selects an SCXML invoked session:
/// [`SCXML_INVOKE_TYPE_URI`], the same URI without its trailing slash (the
/// spelling the prepared W3C IRP corpus uses, official case 216), or the
/// [`SCXML_PROCESSOR_ALIAS`]. No case folding or trimming. This is the single
/// source of truth for SCXML invoke-type spelling.
#[must_use]
pub fn is_scxml_invoke_type(value: &str) -> bool {
    value == SCXML_PROCESSOR_ALIAS
        || value == SCXML_INVOKE_TYPE_URI
        || SCXML_INVOKE_TYPE_URI.strip_suffix('/') == Some(value)
}

/// Largest invoked-service type URI in bytes: a format bound of the WIT
/// `service-invocation.type-uri` and the Machine IR's `invoke_types`, so it
/// applies on every build.
pub const MAX_SERVICE_TYPE_URI_BYTES: usize = 4_096;

/// Whether one exact `<invoke>` type value names an invoked service: not an
/// SCXML invoke type ([`is_scxml_invoke_type`]), an absolute URI (a scheme,
/// `:`, and no white space; see [`NamespaceUri::new`]), and at most
/// [`MAX_SERVICE_TYPE_URI_BYTES`] bytes. No case folding or trimming: hosts
/// bind services by this exact text.
#[must_use]
pub fn is_service_type_uri(value: &str) -> bool {
    !is_scxml_invoke_type(value)
        && value.len() <= MAX_SERVICE_TYPE_URI_BYTES
        && NamespaceUri::new(value).is_ok()
}

/// Failure to encode or decode a canonical delayed-send request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendCodecError {
    /// The payload exceeded [`MAX_DELAYED_SEND_BYTES`].
    TooLarge,
    /// The payload was malformed, non-canonical, or contained an invalid value.
    Invalid,
    /// The payload named an unsupported schema version.
    UnsupportedVersion,
}

impl fmt::Display for SendCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooLarge => "delayed-send payload exceeds its byte bound",
            Self::Invalid => "delayed-send payload is invalid",
            Self::UnsupportedVersion => "delayed-send payload version is unsupported",
        })
    }
}

impl std::error::Error for SendCodecError {}

/// Whether a request's send id and its event's agree: the event carries no
/// send id, or the request's own (SCXML C.1: the delivered `_event.sendid`
/// is set only when the author gave `id` or `idlocation`, while the request
/// may still carry a generated one).
#[must_use]
pub fn send_ids_agree(request: Option<&SendId>, event: Option<&SendId>) -> bool {
    event.is_none_or(|event| request.is_some_and(|request| request.as_str() == event.as_str()))
}

/// Wire kind byte of a [`DelayedSend::Scxml`] record.
const DELAYED_SEND_KIND_SCXML: u8 = 0;
/// Wire kind byte of a [`DelayedSend::BasicHttp`] record.
const DELAYED_SEND_KIND_BASICHTTP: u8 = 1;

/// Encodes one delayed send into the canonical versioned wire format:
/// [`DELAYED_SEND_MAGIC`], the little-endian [`DELAYED_SEND_WIRE_VERSION`],
/// one kind byte (`0` SCXML, `1` `BasicHTTP`), then the send's fields.
///
/// The returned bytes are owned by the caller and contain no ambient data.
/// Field order is fixed, all strings are UTF-8 length-prefixed, and absence is
/// distinct from an empty string.
///
/// # Errors
///
/// Returns [`SendCodecError::TooLarge`] when the canonical payload exceeds the
/// byte bound or a field length or count cannot be represented. Returns
/// [`SendCodecError::Invalid`] when an SCXML request's and its event's send
/// IDs do not agree ([`send_ids_agree`]) or a value is not canonical.
pub fn encode_delayed_send(send: &DelayedSend) -> Result<Vec<u8>, SendCodecError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(DELAYED_SEND_MAGIC);
    bytes.extend_from_slice(&DELAYED_SEND_WIRE_VERSION.to_le_bytes());
    match send {
        DelayedSend::Scxml(request) => {
            bytes.push(DELAYED_SEND_KIND_SCXML);
            write_send_request(&mut bytes, request)?;
        }
        DelayedSend::BasicHttp(send) => {
            bytes.push(DELAYED_SEND_KIND_BASICHTTP);
            write_basichttp_send(&mut bytes, send)?;
        }
    }
    if bytes.len() > MAX_DELAYED_SEND_BYTES {
        return Err(SendCodecError::TooLarge);
    }
    Ok(bytes)
}

/// Writes the fields of one SCXML send request.
fn write_send_request(bytes: &mut Vec<u8>, request: &SendRequest) -> Result<(), SendCodecError> {
    if !send_ids_agree(request.send_id.as_ref(), request.event.send_id.as_ref()) {
        return Err(SendCodecError::Invalid);
    }
    request.event.validate().map_err(map_event_codec_error)?;
    write_optional_string(bytes, request.send_id.as_ref().map(SendId::as_str))?;
    write_string(bytes, request.event.name.as_str())?;
    bytes.push(match request.event.event_type {
        EventType::Internal => 0,
        EventType::External => 1,
        EventType::Platform => 2,
    });
    write_optional_string(bytes, request.event.send_id.as_ref().map(SendId::as_str))?;
    write_optional_string(bytes, request.event.origin.as_deref())?;
    write_optional_string(bytes, request.event.origin_type.as_deref())?;
    write_optional_string(
        bytes,
        request.event.invoke_id.as_ref().map(InvokeId::as_str),
    )?;
    write_data_value(bytes, &request.event.data)?;
    write_optional_string(bytes, request.event.raw.as_deref())?;
    write_trace_context(bytes, request.event.trace_context.as_ref())?;
    write_optional_string(bytes, request.target.as_deref())?;
    write_optional_string(bytes, request.processor_type.as_deref())?;
    bytes.extend_from_slice(&request.delay_ms.to_le_bytes());
    write_send_message(bytes, &request.message)
}

/// Writes the fields of one `BasicHTTP` send: send id, event name, target,
/// fields (a `u32` count then each name and value), content, delay, and
/// trace carrier.
fn write_basichttp_send(bytes: &mut Vec<u8>, send: &BasicHttpSend) -> Result<(), SendCodecError> {
    check_basichttp_send(send)?;
    write_string(bytes, send.send_id.as_str())?;
    write_optional_string(bytes, send.event_name.as_deref())?;
    write_optional_string(bytes, send.target.as_deref())?;
    let count = u32::try_from(send.params.len()).map_err(|_| SendCodecError::TooLarge)?;
    bytes.extend_from_slice(&count.to_le_bytes());
    for field in &send.params {
        write_string(bytes, &field.name)?;
        write_data_value(bytes, &field.value)?;
    }
    match &send.content {
        None => bytes.push(0),
        Some(content) => {
            bytes.push(1);
            write_data_value(bytes, content)?;
        }
    }
    bytes.extend_from_slice(&send.delay_ms.to_le_bytes());
    write_trace_context(bytes, send.trace_context.as_ref())
}

/// The bounds of the WIT `basichttp-send` record: event name, target, field
/// count, and field names (non-empty, at most [`MAX_NAME_BYTES`]).
fn check_basichttp_send(send: &BasicHttpSend) -> Result<(), SendCodecError> {
    if send
        .event_name
        .as_ref()
        .is_some_and(|name| name.len() > MAX_BASICHTTP_EVENT_NAME_BYTES)
        || send
            .target
            .as_ref()
            .is_some_and(|target| target.len() > MAX_BASICHTTP_TARGET_BYTES)
        || send.params.len() > MAX_BASICHTTP_PARAMS
    {
        return Err(SendCodecError::TooLarge);
    }
    if send
        .params
        .iter()
        .any(|field| field.name.is_empty() || field.name.len() > MAX_NAME_BYTES)
    {
        return Err(SendCodecError::Invalid);
    }
    Ok(())
}

/// Writes an optional trace carrier: tag `0` none, `1` then traceparent,
/// tracestate, baggage, and baggage policy.
fn write_trace_context(
    bytes: &mut Vec<u8>,
    context: Option<&TraceContext>,
) -> Result<(), SendCodecError> {
    match context {
        None => bytes.push(0),
        Some(context) => {
            context.validate().map_err(|_| SendCodecError::Invalid)?;
            bytes.push(1);
            write_string(bytes, &context.traceparent)?;
            write_optional_string(bytes, context.tracestate.as_deref())?;
            write_optional_string(bytes, context.baggage.as_deref())?;
            bytes.push(match context.baggage_policy {
                BaggagePolicy::Drop => 0,
                BaggagePolicy::Propagate => 1,
            });
        }
    }
    Ok(())
}

/// Decodes and fully validates one canonical delayed send
/// ([`encode_delayed_send`]).
///
/// Input bytes are borrowed and never retained. Unknown versions and kinds,
/// malformed option tags, invalid names, invalid UTF-8, trailing bytes, and
/// over-bound payloads fail closed.
///
/// # Errors
///
/// Returns a typed codec failure without producing a partially decoded value.
pub fn decode_delayed_send(bytes: &[u8]) -> Result<DelayedSend, SendCodecError> {
    if bytes.len() > MAX_DELAYED_SEND_BYTES {
        return Err(SendCodecError::TooLarge);
    }
    let mut reader = WireReader { bytes, cursor: 0 };
    if reader.read::<8>()? != *DELAYED_SEND_MAGIC {
        return Err(SendCodecError::Invalid);
    }
    let version = u32::from_le_bytes(reader.read()?);
    if version != DELAYED_SEND_WIRE_VERSION {
        return Err(SendCodecError::UnsupportedVersion);
    }
    let send = match reader.read::<1>()?[0] {
        DELAYED_SEND_KIND_SCXML => DelayedSend::Scxml(reader.read_send_request()?),
        DELAYED_SEND_KIND_BASICHTTP => DelayedSend::BasicHttp(reader.read_basichttp_send()?),
        _ => return Err(SendCodecError::Invalid),
    };
    if reader.cursor != bytes.len() {
        return Err(SendCodecError::Invalid);
    }
    Ok(send)
}

impl WireReader<'_> {
    /// Reads the fields [`write_send_request`] wrote.
    fn read_send_request(&mut self) -> Result<SendRequest, SendCodecError> {
        let reader = self;
        let send_id = reader
            .read_optional_string()?
            .map(SendId::new)
            .transpose()
            .map_err(|_| SendCodecError::Invalid)?;
        let name = EventName::new(reader.read_string()?).map_err(|_| SendCodecError::Invalid)?;
        let event_type = match reader.read::<1>()?[0] {
            0 => EventType::Internal,
            1 => EventType::External,
            2 => EventType::Platform,
            _ => return Err(SendCodecError::Invalid),
        };
        let event_send_id = reader
            .read_optional_string()?
            .map(SendId::new)
            .transpose()
            .map_err(|_| SendCodecError::Invalid)?;
        let origin = reader.read_optional_string()?;
        let origin_type = reader.read_optional_string()?;
        let invoke_id = reader
            .read_optional_string()?
            .map(InvokeId::new)
            .transpose()
            .map_err(|_| SendCodecError::Invalid)?;
        let data = reader.read_data_value()?;
        let raw = reader.read_optional_string()?;
        let trace_context = reader.read_trace_context()?;
        let target = reader.read_optional_string()?;
        let processor_type = reader.read_optional_string()?;
        let delay_ms = u64::from_le_bytes(reader.read()?);
        let message = reader.read_send_message()?;
        if !send_ids_agree(send_id.as_ref(), event_send_id.as_ref()) {
            return Err(SendCodecError::Invalid);
        }
        let request = SendRequest {
            send_id,
            event: Event {
                name,
                event_type,
                send_id: event_send_id,
                origin,
                origin_type,
                invoke_id,
                data,
                trace_context,
                raw,
            },
            target,
            processor_type,
            delay_ms,
            message,
        };
        request.event.validate().map_err(map_event_codec_error)?;
        Ok(request)
    }

    /// Reads the fields [`write_basichttp_send`] wrote.
    fn read_basichttp_send(&mut self) -> Result<BasicHttpSend, SendCodecError> {
        let send_id = SendId::new(self.read_string()?).map_err(|_| SendCodecError::Invalid)?;
        let event_name = self.read_optional_string()?;
        let target = self.read_optional_string()?;
        let count = u32::from_le_bytes(self.read()?);
        let count = usize::try_from(count).map_err(|_| SendCodecError::Invalid)?;
        if count > MAX_BASICHTTP_PARAMS {
            return Err(SendCodecError::Invalid);
        }
        let mut params = Vec::with_capacity(count);
        for _ in 0..count {
            let name = self.read_string()?;
            let value = self.read_data_value()?;
            params.push(NamedValue { name, value });
        }
        let content = match self.read::<1>()?[0] {
            0 => None,
            1 => Some(self.read_data_value()?),
            _ => return Err(SendCodecError::Invalid),
        };
        let delay_ms = u64::from_le_bytes(self.read()?);
        let trace_context = self.read_trace_context()?;
        let send = BasicHttpSend {
            send_id,
            event_name,
            target,
            params,
            content,
            delay_ms,
            trace_context,
        };
        check_basichttp_send(&send).map_err(|_| SendCodecError::Invalid)?;
        Ok(send)
    }

    /// Reads an optional trace carrier [`write_trace_context`] wrote.
    fn read_trace_context(&mut self) -> Result<Option<TraceContext>, SendCodecError> {
        match self.read::<1>()?[0] {
            0 => Ok(None),
            1 => {
                let traceparent = self.read_string()?;
                let tracestate = self.read_optional_string()?;
                let baggage = self.read_optional_string()?;
                let baggage_policy = match self.read::<1>()?[0] {
                    0 => BaggagePolicy::Drop,
                    1 => BaggagePolicy::Propagate,
                    _ => return Err(SendCodecError::Invalid),
                };
                TraceContext::new(traceparent, tracestate, baggage, baggage_policy)
                    .map(Some)
                    .map_err(|_| SendCodecError::Invalid)
            }
            _ => Err(SendCodecError::Invalid),
        }
    }
}

fn map_event_codec_error(error: EventError) -> SendCodecError {
    match error {
        EventError::ValueTooLarge | EventError::AggregateTooLarge => SendCodecError::TooLarge,
        EventError::InvalidValue | EventError::InvalidTraceContext => SendCodecError::Invalid,
    }
}

fn write_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), SendCodecError> {
    let length = u32::try_from(value.len()).map_err(|_| SendCodecError::TooLarge)?;
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    if bytes.len() > MAX_DELAYED_SEND_BYTES {
        return Err(SendCodecError::TooLarge);
    }
    Ok(())
}

fn write_optional_string(bytes: &mut Vec<u8>, value: Option<&str>) -> Result<(), SendCodecError> {
    match value {
        None => {
            bytes.push(0);
            Ok(())
        }
        Some(value) => {
            bytes.push(1);
            write_string(bytes, value)
        }
    }
}

/// Writes one `<send>` message: tag `0` empty, `1` fields (a `u32` count
/// then each name and value), `2` content.
fn write_send_message(bytes: &mut Vec<u8>, message: &SendMessage) -> Result<(), SendCodecError> {
    match message {
        SendMessage::Empty => bytes.push(0),
        SendMessage::Params(fields) => {
            if fields.len() > MAX_SEND_MESSAGE_PARAMS {
                return Err(SendCodecError::TooLarge);
            }
            bytes.push(1);
            let count = u32::try_from(fields.len()).map_err(|_| SendCodecError::TooLarge)?;
            bytes.extend_from_slice(&count.to_le_bytes());
            for field in fields {
                if field.name.is_empty() || field.name.len() > MAX_NAME_BYTES {
                    return Err(SendCodecError::Invalid);
                }
                write_string(bytes, &field.name)?;
                write_data_value(bytes, &field.value)?;
            }
        }
        SendMessage::Content => bytes.push(2),
    }
    Ok(())
}

struct WireReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl WireReader<'_> {
    /// Reads one `<send>` message written by [`write_send_message`].
    fn read_send_message(&mut self) -> Result<SendMessage, SendCodecError> {
        match self.read::<1>()?[0] {
            0 => Ok(SendMessage::Empty),
            1 => {
                let count = u32::from_le_bytes(self.read()?);
                let count = usize::try_from(count).map_err(|_| SendCodecError::Invalid)?;
                if count > MAX_SEND_MESSAGE_PARAMS {
                    return Err(SendCodecError::Invalid);
                }
                let mut fields = Vec::with_capacity(count);
                for _ in 0..count {
                    let name = self.read_string()?;
                    if name.is_empty() || name.len() > MAX_NAME_BYTES {
                        return Err(SendCodecError::Invalid);
                    }
                    let value = self.read_data_value()?;
                    fields.push(NamedValue { name, value });
                }
                Ok(SendMessage::Params(fields))
            }
            2 => Ok(SendMessage::Content),
            _ => Err(SendCodecError::Invalid),
        }
    }

    fn read<const N: usize>(&mut self) -> Result<[u8; N], SendCodecError> {
        let end = self.cursor.checked_add(N).ok_or(SendCodecError::Invalid)?;
        let source = self
            .bytes
            .get(self.cursor..end)
            .ok_or(SendCodecError::Invalid)?;
        let mut result = [0_u8; N];
        result.copy_from_slice(source);
        self.cursor = end;
        Ok(result)
    }

    fn read_string(&mut self) -> Result<String, SendCodecError> {
        let length = u32::from_le_bytes(self.read()?);
        let length = usize::try_from(length).map_err(|_| SendCodecError::Invalid)?;
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(SendCodecError::Invalid)?;
        let source = self
            .bytes
            .get(self.cursor..end)
            .ok_or(SendCodecError::Invalid)?;
        let value = core::str::from_utf8(source)
            .map_err(|_| SendCodecError::Invalid)?
            .to_owned();
        self.cursor = end;
        Ok(value)
    }

    fn read_optional_string(&mut self) -> Result<Option<String>, SendCodecError> {
        self.read::<1>().and_then(|tag| match tag[0] {
            0 => Ok(None),
            1 => self.read_string().map(Some),
            _ => Err(SendCodecError::Invalid),
        })
    }

    fn read_data_value(&mut self) -> Result<DataValue, SendCodecError> {
        match self.read::<1>()?[0] {
            0 => Ok(DataValue::Absent),
            1 => Ok(DataValue::Undefined),
            2 => Ok(DataValue::Null),
            3 => match self.read::<1>()?[0] {
                0 => Ok(DataValue::Bool(false)),
                1 => Ok(DataValue::Bool(true)),
                _ => Err(SendCodecError::Invalid),
            },
            4 => FiniteNumber::new(f64::from_le_bytes(self.read()?))
                .map(DataValue::Number)
                .map_err(|_| SendCodecError::Invalid),
            5 => self.read_string().map(DataValue::String),
            6 => self.read_string().and_then(|value| {
                DataValue::canonical_json_array(&value).map_err(|_| SendCodecError::Invalid)
            }),
            7 => self.read_string().and_then(|value| {
                DataValue::canonical_json_object(&value).map_err(|_| SendCodecError::Invalid)
            }),
            8 => self.read_string().map(DataValue::Xml),
            _ => Err(SendCodecError::Invalid),
        }
    }
}

/// Data-model value used by the portable component contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataValue {
    /// No value was supplied at the boundary.
    Absent,
    /// The ECMAScript `undefined` value.
    Undefined,
    /// The data-model null value.
    Null,
    /// A boolean value.
    Bool(bool),
    /// A finite number.
    Number(FiniteNumber),
    /// A string value.
    String(String),
    /// A canonical JSON array string.
    JsonArray(String),
    /// A canonical JSON object string with recursively sorted object keys.
    JsonObject(String),
    /// An XML payload preserved as UTF-8 source text.
    Xml(String),
}

/// What the source text of one `BasicHTTP` form value (SCXML C.2) is: a
/// `<param expr>` value expression, or a `namelist` entry or `<param
/// location>`. The native mirror of the WIT `evaluate-form-value`
/// `location` flag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormValueSource {
    /// A value expression.
    Expression,
    /// A location expression; data models may require it to select data.
    Location,
}

/// Where an `<assign>` places its value relative to each node its location
/// selects (SCXML 1.0 section 5.4, and the `XPath` data model's `type` and
/// `attr` in SCXML 1.0 CR 2014-03-13 §C.3.6); the
/// native mirror of the WIT `assign-placement`.
///
/// [`AssignPlacement::ReplaceChildren`] is the default and the only
/// placement the ECMAScript and null data models define; engine-materialized
/// assignments (`<foreach>` item and index) always use it. A data model
/// fails a placement it does not define with an execution error and changes
/// no data.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AssignPlacement {
    /// Replace every child of the location node (the default).
    #[default]
    ReplaceChildren,
    /// Insert the value before the first child of the location node.
    FirstChild,
    /// Insert the value after the last child of the location node.
    LastChild,
    /// Insert the value as the preceding sibling of the location node.
    PreviousSibling,
    /// Insert the value as the following sibling of the location node.
    NextSibling,
    /// Replace the location node itself with the value.
    Replace,
    /// Delete the location node; the value is ignored.
    Delete,
    /// Add, or replace, the attribute with this exact `attr` name on the
    /// location node, valued by the value's string-value.
    AddAttribute(String),
}

/// How one `<data>` declaration is valued (SCXML 5.2-5.3); the native
/// mirror of the WIT `data-initializer`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataInitializer {
    /// The `expr` value expression, evaluated with no current event.
    Expression(String),
    /// A value already materialized (inline content or an external `src`
    /// document).
    Value(DataValue),
}

/// What an `<assign>` assigns (SCXML 1.0 section 5.4); the native mirror of
/// the WIT `assign-source`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignSource {
    /// The `expr` value expression, which the data model evaluates and
    /// assigns in one transaction (so any value it can represent, portable
    /// or not, is assignable).
    Expression(String),
    /// A value the engine or host already materialized for an existing
    /// location: inline `<assign>` children, a generated `idlocation` id,
    /// or [`DataValue::Absent`] for the `delete` placement (SCXML 1.0 CR 2014-03-13 §C.3.6).
    Value(DataValue),
    /// A `<foreach>` `item` or `index` value (SCXML 1.0 section 4.6). The
    /// location is then the attribute's variable name, not a location
    /// expression: the data model declares the variable when it is not
    /// declared yet and binds the value, and fails a name that is not a
    /// legal variable name. Always paired with
    /// [`AssignPlacement::ReplaceChildren`].
    ForeachValue(DataValue),
}

/// A finite ECMAScript number with stable equality semantics.
#[derive(Clone, Copy, Debug)]
pub struct FiniteNumber(f64);

impl FiniteNumber {
    /// Constructs a finite number, normalizing negative zero to positive zero.
    ///
    /// # Errors
    ///
    /// Returns [`DataValueError::NonFiniteNumber`] for NaN or infinity.
    pub fn new(value: f64) -> Result<Self, DataValueError> {
        if !value.is_finite() {
            return Err(DataValueError::NonFiniteNumber);
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }

    /// Returns the finite IEEE-754 value.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for FiniteNumber {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for FiniteNumber {}

impl DataValue {
    /// Parses and canonicalizes a JSON array.
    ///
    /// # Errors
    ///
    /// Returns [`DataValueError`] when the text is not a JSON array or contains
    /// a non-finite/unrepresentable number.
    pub fn canonical_json_array(value: &str) -> Result<Self, DataValueError> {
        canonical_json(value, JsonShape::Array).map(Self::JsonArray)
    }

    /// Parses and canonicalizes a JSON object.
    ///
    /// # Errors
    ///
    /// Returns [`DataValueError`] when the text is not a JSON object or contains
    /// an unrepresentable value.
    pub fn canonical_json_object(value: &str) -> Result<Self, DataValueError> {
        canonical_json(value, JsonShape::Object).map(Self::JsonObject)
    }

    /// The portable value of member `name` of a [`Self::JsonObject`] (a
    /// primitive, or a canonical JSON array or object); `None` for another
    /// kind, a missing member, or a member with no portable value.
    #[must_use]
    pub fn json_object_member(&self, name: &str) -> Option<Self> {
        let Self::JsonObject(text) = self else {
            return None;
        };
        let serde_json::Value::Object(mut members) = serde_json::from_str(text).ok()? else {
            return None;
        };
        match members.remove(name)? {
            serde_json::Value::Null => Some(Self::Null),
            serde_json::Value::Bool(value) => Some(Self::Bool(value)),
            serde_json::Value::Number(number) => {
                FiniteNumber::new(number.as_f64()?).ok().map(Self::Number)
            }
            serde_json::Value::String(value) => Some(Self::String(value)),
            member @ serde_json::Value::Array(_) => {
                Self::canonical_json_array(&serde_json::to_string(&member).ok()?).ok()
            }
            member @ serde_json::Value::Object(_) => {
                Self::canonical_json_object(&serde_json::to_string(&member).ok()?).ok()
            }
        }
    }
}

/// Failure to construct a canonical portable data value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataValueError {
    /// Input was not valid JSON or had the wrong top-level shape.
    InvalidJson,
    /// JSON input exceeded [`MAX_EVENT_DATA_BYTES`] before parsing.
    ValueTooLarge,
    /// ECMAScript number was NaN or infinite.
    NonFiniteNumber,
}

impl fmt::Display for DataValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJson => "data value is not canonicalizable JSON of the required shape",
            Self::ValueTooLarge => "data value exceeds its byte bound",
            Self::NonFiniteNumber => "data-model numbers must be finite",
        })
    }
}

impl std::error::Error for DataValueError {}

#[derive(Clone, Copy)]
enum JsonShape {
    Array,
    Object,
}

fn canonical_json(value: &str, shape: JsonShape) -> Result<String, DataValueError> {
    if value.len() > MAX_EVENT_DATA_BYTES {
        return Err(DataValueError::ValueTooLarge);
    }
    let parsed: serde_json::Value =
        serde_json::from_str(value).map_err(|_| DataValueError::InvalidJson)?;
    let shape_matches = matches!(
        (&parsed, shape),
        (serde_json::Value::Array(_), JsonShape::Array)
            | (serde_json::Value::Object(_), JsonShape::Object)
    );
    if !shape_matches {
        return Err(DataValueError::InvalidJson);
    }
    let canonical = serde_json::to_string(&parsed).map_err(|_| DataValueError::InvalidJson)?;
    if canonical.len() > MAX_EVENT_DATA_BYTES {
        return Err(DataValueError::ValueTooLarge);
    }
    Ok(canonical)
}

fn write_data_value(bytes: &mut Vec<u8>, value: &DataValue) -> Result<(), SendCodecError> {
    match value {
        DataValue::Absent => bytes.push(0),
        DataValue::Undefined => bytes.push(1),
        DataValue::Null => bytes.push(2),
        DataValue::Bool(value) => {
            bytes.push(3);
            bytes.push(u8::from(*value));
        }
        DataValue::Number(value) => {
            bytes.push(4);
            bytes.extend_from_slice(&value.get().to_le_bytes());
        }
        DataValue::String(value) => {
            bytes.push(5);
            write_string(bytes, value)?;
        }
        DataValue::JsonArray(value) => {
            if DataValue::canonical_json_array(value) != Ok(DataValue::JsonArray(value.clone())) {
                return Err(SendCodecError::Invalid);
            }
            bytes.push(6);
            write_string(bytes, value)?;
        }
        DataValue::JsonObject(value) => {
            if DataValue::canonical_json_object(value) != Ok(DataValue::JsonObject(value.clone())) {
                return Err(SendCodecError::Invalid);
            }
            bytes.push(7);
            write_string(bytes, value)?;
        }
        DataValue::Xml(value) => {
            bytes.push(8);
            write_string(bytes, value)?;
        }
    }
    Ok(())
}

/// Expanded XML name independent of any source prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedName {
    /// Namespace URI, or `None` for an unqualified XML name.
    pub namespace_uri: Option<NamespaceUri>,
    /// Namespace-local name.
    pub local_name: String,
}

/// Attribute preserved on an executable content extension element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionAttribute {
    /// Expanded attribute name.
    pub name: QualifiedName,
    /// Unescaped attribute value.
    pub value: String,
}

/// Stable flat content token for executable-content extensions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionContent {
    /// Character content in document order.
    Text(String),
    /// Start of a nested element.
    StartElement {
        /// Expanded element name.
        name: QualifiedName,
        /// Attributes in source order.
        attributes: Vec<ExtensionAttribute>,
    },
    /// End of a nested element.
    EndElement(QualifiedName),
}

/// WIT-safe representation of an executable content extension element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableContentExtension {
    /// Expanded element name; execution binds by this namespace URI, never a
    /// prefix.
    pub name: QualifiedName,
    /// Attributes in source order.
    pub attributes: Vec<ExtensionAttribute>,
    /// Flat, balanced content tokens in document order.
    pub content: Vec<ExtensionContent>,
}

impl ExecutableContentExtension {
    /// Returns the canonical namespace an executable-content extension
    /// requires to execute.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutableContentExtensionError::Unqualified`] for an
    /// unqualified element.
    pub fn namespace_uri(&self) -> Result<&NamespaceUri, ExecutableContentExtensionError> {
        self.name
            .namespace_uri
            .as_ref()
            .ok_or(ExecutableContentExtensionError::Unqualified)
    }

    /// Validates the flat content stream.
    ///
    /// # Errors
    ///
    /// Returns a structural error for mismatched or unclosed nested elements.
    pub fn validate(&self) -> Result<(), ExecutableContentExtensionError> {
        let mut stack = Vec::new();
        for token in &self.content {
            match token {
                ExtensionContent::Text(_) => {}
                ExtensionContent::StartElement { name, .. } => stack.push(name),
                ExtensionContent::EndElement(name) => match stack.pop() {
                    Some(open) if open == name => {}
                    Some(_) => return Err(ExecutableContentExtensionError::MismatchedContent),
                    None => return Err(ExecutableContentExtensionError::UnexpectedEnd),
                },
            }
        }
        if stack.is_empty() {
            Ok(())
        } else {
            Err(ExecutableContentExtensionError::UnclosedContent)
        }
    }
}

/// Validation failure for an executable-content extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableContentExtensionError {
    /// An executable-content extension requires an expanded namespace URI.
    Unqualified,
    /// Content contained an end token without a matching start token.
    UnexpectedEnd,
    /// Content start and end names did not match.
    MismatchedContent,
    /// Content ended with open nested elements.
    UnclosedContent,
}

impl fmt::Display for ExecutableContentExtensionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unqualified => {
                formatter.write_str("executable-content extension must have a namespace URI")
            }
            Self::UnexpectedEnd => {
                formatter.write_str("extension content has an unexpected end token")
            }
            Self::MismatchedContent => {
                formatter.write_str("extension content names do not balance")
            }
            Self::UnclosedContent => {
                formatter.write_str("extension content has an unclosed element")
            }
        }
    }
}

impl std::error::Error for ExecutableContentExtensionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_uris_are_exact_absolute_uris_that_are_not_scxml() {
        for accepted in [
            "https://stateforward.org/scxml/services/echo",
            "urn:example:service",
            "x-service:a",
        ] {
            assert!(is_service_type_uri(accepted), "{accepted}");
        }
        let over = format!("urn:{}", "a".repeat(MAX_SERVICE_TYPE_URI_BYTES));
        for rejected in [
            "scxml",
            "http://www.w3.org/TR/scxml/",
            "http://www.w3.org/TR/scxml",
            "",
            "foo",
            "1urn:x",
            "urn:a b",
            over.as_str(),
        ] {
            assert!(!is_service_type_uri(rejected), "{rejected}");
        }
        let at_bound = format!("urn:{}", "a".repeat(MAX_SERVICE_TYPE_URI_BYTES - 4));
        assert!(is_service_type_uri(&at_bound));
    }

    /// Encodes an SCXML delayed send.
    fn encode_send_request(request: &SendRequest) -> Result<Vec<u8>, SendCodecError> {
        encode_delayed_send(&DelayedSend::Scxml(request.clone()))
    }

    /// Decodes a delayed send that must be an SCXML one.
    fn decode_send_request(bytes: &[u8]) -> Result<SendRequest, SendCodecError> {
        match decode_delayed_send(bytes)? {
            DelayedSend::Scxml(request) => Ok(request),
            DelayedSend::BasicHttp(_) => Err(SendCodecError::Invalid),
        }
    }

    fn basichttp_send() -> BasicHttpSend {
        BasicHttpSend {
            send_id: SendId::new("post-1").expect("test send id is valid"),
            event_name: Some("order.ready".into()),
            target: Some("http://example.test/sessions/a/basichttp".into()),
            params: vec![
                NamedValue {
                    name: "a".into(),
                    value: DataValue::String("1 2".into()),
                },
                NamedValue {
                    name: "a".into(),
                    value: DataValue::Number(FiniteNumber::new(2.5).expect("finite")),
                },
            ],
            content: None,
            delay_ms: 250,
            trace_context: Some(TraceContext {
                traceparent: "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01".into(),
                tracestate: None,
                baggage: None,
                baggage_policy: BaggagePolicy::Drop,
            }),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn resource_bounds_do_not_apply_natively() {
        assert_eq!(wasm_bound(7), usize::MAX);
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_resource_bounds_keep_their_value() {
        assert_eq!(wasm_bound(7), 7);
    }

    #[test]
    fn basichttp_delayed_sends_round_trip_and_fail_closed() {
        let fields = basichttp_send();
        let content = BasicHttpSend {
            event_name: None,
            target: None,
            params: Vec::new(),
            content: Some(DataValue::Xml("<a/>".into())),
            trace_context: None,
            ..basichttp_send()
        };
        for send in [fields.clone(), content] {
            let delayed = DelayedSend::BasicHttp(send);
            let bytes = encode_delayed_send(&delayed).expect("bounded send encodes");
            assert_eq!(bytes[12], 1, "the BasicHTTP kind byte follows the header");
            assert_eq!(decode_delayed_send(&bytes), Ok(delayed.clone()));
            assert_eq!(delayed.send_id().map(SendId::as_str), Some("post-1"));
            assert_eq!(delayed.delay_ms(), 250);
        }
        // Unknown kinds, trailing and truncated bytes, and over-bound or
        // unnamed fields fail closed.
        let bytes = encode_delayed_send(&DelayedSend::BasicHttp(fields.clone()))
            .expect("bounded send encodes");
        let mut unknown = bytes.clone();
        unknown[12] = 2;
        assert_eq!(decode_delayed_send(&unknown), Err(SendCodecError::Invalid));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode_delayed_send(&trailing), Err(SendCodecError::Invalid));
        assert_eq!(
            decode_delayed_send(&bytes[..bytes.len() - 1]),
            Err(SendCodecError::Invalid)
        );
        let unnamed = BasicHttpSend {
            params: vec![NamedValue {
                name: String::new(),
                value: DataValue::Null,
            }],
            ..fields.clone()
        };
        assert_eq!(
            encode_delayed_send(&DelayedSend::BasicHttp(unnamed)),
            Err(SendCodecError::Invalid)
        );
        let too_many = BasicHttpSend {
            params: vec![
                NamedValue {
                    name: "a".into(),
                    value: DataValue::Null,
                };
                MAX_BASICHTTP_PARAMS + 1
            ],
            ..fields.clone()
        };
        assert_eq!(
            encode_delayed_send(&DelayedSend::BasicHttp(too_many)),
            Err(SendCodecError::TooLarge)
        );
        let long_target = BasicHttpSend {
            target: Some("x".repeat(MAX_BASICHTTP_TARGET_BYTES + 1)),
            ..fields
        };
        assert_eq!(
            encode_delayed_send(&DelayedSend::BasicHttp(long_target)),
            Err(SendCodecError::TooLarge)
        );
    }

    #[test]
    fn json_object_members_are_portable_values() {
        let object = DataValue::canonical_json_object(r#"{"a":1,"b":"t","c":[1],"d":{"e":null}}"#)
            .expect("fixture object is canonical");
        assert_eq!(
            object.json_object_member("a"),
            Some(DataValue::Number(FiniteNumber::new(1.0).expect("finite")))
        );
        assert_eq!(
            object.json_object_member("b"),
            Some(DataValue::String("t".into()))
        );
        assert_eq!(
            object.json_object_member("c"),
            Some(DataValue::JsonArray("[1]".into()))
        );
        assert_eq!(
            object.json_object_member("d"),
            Some(DataValue::JsonObject(r#"{"e":null}"#.into()))
        );
        assert_eq!(object.json_object_member("missing"), None);
        assert_eq!(DataValue::String("a".into()).json_object_member("a"), None);
    }
    use scxml_telemetry::{Recorder, install_recorder};

    fn unqualified_executable_content_extension() -> ExecutableContentExtension {
        ExecutableContentExtension {
            name: QualifiedName {
                namespace_uri: None,
                local_name: "custom".into(),
            },
            attributes: Vec::new(),
            content: vec![ExtensionContent::EndElement(QualifiedName {
                namespace_uri: None,
                local_name: "nested".into(),
            })],
        }
    }

    #[test]
    fn trace_context_is_bounded_canonical_and_applies_baggage_policy() {
        let traceparent = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
        let dropped = TraceContext::new(
            traceparent,
            Some("vendor=value".into()),
            Some("secret=value".into()),
            BaggagePolicy::Drop,
        )
        .expect("canonical trace context is accepted");
        assert_eq!(dropped.baggage, None);
        assert_eq!(dropped.validate(), Ok(()));

        assert_eq!(
            TraceContext::new(
                "00-00000000000000000000000000000000-0123456789abcdef-01",
                None,
                None,
                BaggagePolicy::Drop,
            ),
            Err(TraceContextError::InvalidTraceparent)
        );
        assert_eq!(
            TraceContext::new(
                traceparent,
                None,
                Some("bad baggage".into()),
                BaggagePolicy::Propagate,
            ),
            Err(TraceContextError::InvalidBaggage)
        );
        let noncanonical = TraceContext {
            traceparent: traceparent.into(),
            tracestate: None,
            baggage: Some("secret=value".into()),
            baggage_policy: BaggagePolicy::Drop,
        };
        assert_eq!(
            noncanonical.validate(),
            Err(TraceContextError::BaggagePolicyViolation)
        );
    }

    #[test]
    fn trace_context_level_two_rejects_hostile_tracestate_and_accepts_defined_flags() {
        for flags in ["00", "01", "02", "03"] {
            let traceparent =
                format!("00-0123456789abcdef0123456789abcdef-0123456789abcdef-{flags}");
            TraceContext::new(
                traceparent,
                Some("1vendor=value,tenant@vendor=other".into()),
                None,
                BaggagePolicy::Drop,
            )
            .expect("Level 2 defined trace flags must be accepted");
        }

        for tracestate in [
            "vendor=a=b",
            "foo@bar@baz=value",
            "Vendor=value",
            "vendor=value ",
            "vendor=value,vendor=duplicate",
        ] {
            assert_eq!(
                TraceContext::new(
                    "00-0123456789abcdef0123456789abcdef-0123456789abcdef-03",
                    Some(tracestate.into()),
                    None,
                    BaggagePolicy::Drop,
                ),
                Err(TraceContextError::InvalidTracestate),
                "hostile tracestate must fail: {tracestate}"
            );
        }
        assert_eq!(
            TraceContext::new(
                "00-0123456789abcdef0123456789abcdef-0123456789abcdef-04",
                None,
                None,
                BaggagePolicy::Drop,
            ),
            Err(TraceContextError::InvalidTraceparent)
        );
    }

    #[test]
    fn event_and_json_bounds_are_checked_before_canonicalization() {
        let json_at_limit = format!("[\"{}\"]", "x".repeat(MAX_EVENT_DATA_BYTES - 4));
        assert_eq!(json_at_limit.len(), MAX_EVENT_DATA_BYTES);
        assert!(DataValue::canonical_json_array(&json_at_limit).is_ok());
        let json_over_limit = format!("[\"{}\"]", "x".repeat(MAX_EVENT_DATA_BYTES - 3));
        assert_eq!(
            DataValue::canonical_json_array(&json_over_limit),
            Err(DataValueError::ValueTooLarge)
        );

        let mut bounded =
            Event::external(EventName::new("bounded").expect("event name fixture must be valid"));
        bounded.data = DataValue::String("x".repeat(MAX_EVENT_DATA_BYTES));
        assert_eq!(bounded.validate(), Ok(()));

        bounded.data = DataValue::String("x".repeat(MAX_EVENT_DATA_BYTES + 1));
        assert_eq!(bounded.validate(), Err(EventError::ValueTooLarge));

        let mut aggregate =
            Event::external(EventName::new("aggregate").expect("event name fixture must be valid"));
        aggregate.origin = Some("o".repeat(MAX_EVENT_METADATA_BYTES));
        aggregate.origin_type = Some("t".repeat(MAX_EVENT_METADATA_BYTES));
        aggregate.data = DataValue::String("d".repeat(MAX_EVENT_DATA_BYTES));
        assert_eq!(aggregate.validate(), Err(EventError::AggregateTooLarge));
    }

    #[test]
    fn names_reject_whitespace() {
        assert_eq!(
            EventName::new("two words"),
            Err(NameError::ContainsWhitespace)
        );
        assert_eq!(
            SessionId::new("x".repeat(MAX_NAME_BYTES + 1)),
            Err(NameError::TooLong)
        );
    }

    #[test]
    fn namespace_uri_requires_absolute_exact_identifier() {
        assert_eq!(
            NamespaceUri::new("example/path"),
            Err(NamespaceUriError::Relative)
        );
        assert_ne!(
            NamespaceUri::new("https://example.test/ext")
                .expect("namespace URI without trailing slash must be valid"),
            NamespaceUri::new("https://example.test/ext/")
                .expect("namespace URI with trailing slash must be valid")
        );
    }

    #[test]
    fn extension_content_must_balance() {
        let name = QualifiedName {
            namespace_uri: Some(
                NamespaceUri::new("urn:example:test")
                    .expect("extension-content namespace URI fixture must be valid"),
            ),
            local_name: "child".into(),
        };
        let element = ExecutableContentExtension {
            name: name.clone(),
            attributes: Vec::new(),
            content: vec![
                ExtensionContent::StartElement {
                    name: name.clone(),
                    attributes: Vec::new(),
                },
                ExtensionContent::EndElement(name),
            ],
        };
        assert_eq!(element.validate(), Ok(()));
    }

    #[test]
    fn delayed_send_wire_roundtrips_all_fields_deterministically() {
        let request = SendRequest {
            send_id: Some(SendId::new("timer-1").expect("test send id is valid")),
            event: Event {
                name: EventName::new("order.ready").expect("test event name is valid"),
                event_type: EventType::External,
                send_id: Some(SendId::new("timer-1").expect("test send id is valid")),
                origin: Some("https://example.test/source".into()),
                origin_type: Some("scxml".into()),
                invoke_id: Some(InvokeId::new("child-1").expect("test invoke id is valid")),
                data: DataValue::JsonObject("{\"ok\":true}".into()),
                trace_context: Some(TraceContext {
                    traceparent: "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01".into(),
                    tracestate: Some("vendor=value".into()),
                    baggage: Some("tenant=example".into()),
                    baggage_policy: BaggagePolicy::Propagate,
                }),
                raw: None,
            },
            target: Some("#_internal".into()),
            processor_type: Some("scxml".into()),
            delay_ms: 25,
            message: SendMessage::Empty,
        };
        let first = encode_send_request(&request).expect("bounded request encodes");
        let second = encode_send_request(&request).expect("same request encodes again");
        assert_eq!(first, second);
        assert_eq!(decode_send_request(&first), Ok(request));
    }

    #[test]
    fn delay_parser_accepts_exact_css2_time_designations() {
        for (value, expected) in [
            ("0s", 0),
            ("0ms", 0),
            ("1s", 1_000),
            ("30ms", 30),
            ("1.5s", 1_500),
            (".5s", 500),
            ("0.25s", 250),
            ("1.2345s", 1_234),
            (".0005s", 0),
            ("0.9ms", 0),
            ("2.9ms", 2),
            ("+2s", 2_000),
            ("007s", 7_000),
            ("18446744073709551615ms", u64::MAX),
        ] {
            assert_eq!(parse_delay_ms(value), Ok(expected), "delay {value:?}");
        }
    }

    #[test]
    fn delay_parser_rejects_values_outside_the_css2_time_grammar() {
        for value in [
            "", "s", "ms", "5", "1.5", "1.s", ".s", "1.2.3s", " 1s", "1s ", "1 s", "1S", "1sec",
            "1m", "1e3ms", "0x10ms", "+-1s", "++1s", "1_000ms",
        ] {
            assert_eq!(
                parse_delay_ms(value),
                Err(DelayError::Malformed),
                "delay {value:?}"
            );
        }
        assert_eq!(parse_delay_ms("-1s"), Err(DelayError::Negative));
        assert_eq!(parse_delay_ms("-0ms"), Err(DelayError::Negative));
        assert_eq!(
            parse_delay_ms("18446744073709551616ms"),
            Err(DelayError::Overflow)
        );
        assert_eq!(
            parse_delay_ms("18446744073709552s"),
            Err(DelayError::Overflow)
        );
        let long = format!("{}s", "1".repeat(MAX_DELAY_BYTES));
        assert_eq!(parse_delay_ms(&long), Err(DelayError::TooLong));
        let boundary = format!("{}ms", "0".repeat(MAX_DELAY_BYTES - 2));
        assert_eq!(parse_delay_ms(&boundary), Ok(0));
        assert!(!DelayError::Malformed.to_string().is_empty());
    }

    #[test]
    fn foreach_classifier_snapshots_canonical_shallow_elements() {
        let array = DataValue::JsonArray("[null,true,1.5,\"a\",[2,1],{\"b\":1,\"a\":2}]".into());
        assert_eq!(
            foreach_array_elements(&array),
            Ok(vec![
                DataValue::Null,
                DataValue::Bool(true),
                DataValue::Number(FiniteNumber::new(1.5).expect("finite fixture")),
                DataValue::String("a".into()),
                DataValue::JsonArray("[2,1]".into()),
                DataValue::JsonObject("{\"a\":2,\"b\":1}".into()),
            ])
        );
        assert_eq!(
            foreach_array_elements(&DataValue::JsonArray("[]".into())),
            Ok(Vec::new())
        );
    }

    #[test]
    fn foreach_classifier_rejects_non_arrays_and_invalid_snapshots() {
        assert_eq!(
            foreach_array_elements(&DataValue::String("[1]".into())),
            Err(ForeachError::NotArray(DataValueKind::String))
        );
        assert_eq!(
            foreach_array_elements(&DataValue::JsonObject("{}".into())),
            Err(ForeachError::NotArray(DataValueKind::Object))
        );
        assert_eq!(
            foreach_array_elements(&DataValue::Undefined),
            Err(ForeachError::NotArray(DataValueKind::Undefined))
        );
        assert_eq!(
            foreach_array_elements(&DataValue::JsonArray("{\"a\":1}".into())),
            Err(ForeachError::InvalidJson)
        );
        assert_eq!(
            foreach_array_elements(&DataValue::JsonArray("[1,".into())),
            Err(ForeachError::InvalidJson)
        );
        let oversized = format!("[{}]", "1,".repeat(MAX_EVENT_DATA_BYTES / 2) + "1");
        assert_eq!(
            foreach_array_elements(&DataValue::JsonArray(oversized)),
            Err(ForeachError::TooLarge)
        );
        assert_eq!(
            ForeachError::NotArray(DataValueKind::Xml).to_string(),
            "foreach array expression must evaluate to an array, got xml"
        );
    }

    #[test]
    fn platform_events_are_platform_typed() {
        let event = Event::platform(EventName::new("error.platform").expect("valid name"));
        assert_eq!(event.event_type, EventType::Platform);
    }

    #[test]
    fn delayed_send_wire_roundtrips_raw_message_text() {
        let request = SendRequest {
            send_id: Some(SendId::new("timer-raw").expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new("timer-raw").expect("test send id is valid")),
                raw: Some("<message>raw</message>".into()),
                ..Event::external(EventName::new("raw.ready").expect("test event name is valid"))
            },
            target: None,
            processor_type: None,
            delay_ms: 3,
            message: SendMessage::Empty,
        };
        let bytes = encode_send_request(&request).expect("bounded raw request encodes");
        let decoded = decode_send_request(&bytes).expect("encoded raw request decodes");
        assert_eq!(
            decoded.event.raw.as_deref(),
            Some("<message>raw</message>"),
            "_event.raw must survive the delayed-send wire round trip"
        );
        assert_eq!(decoded, request);

        let absent = SendRequest {
            event: Event {
                raw: None,
                ..request.event.clone()
            },
            ..request
        };
        let absent_bytes = encode_send_request(&absent).expect("request without raw encodes");
        assert_ne!(absent_bytes, bytes, "raw absence must stay distinct");
        assert_eq!(decode_send_request(&absent_bytes), Ok(absent));
    }

    #[test]
    fn delayed_send_wire_rejects_unknown_truncated_and_trailing_bytes() {
        let request = SendRequest {
            send_id: Some(SendId::new("timer-1").expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new("timer-1").expect("test send id is valid")),
                ..Event::external(EventName::new("ready").expect("test event name is valid"))
            },
            target: None,
            processor_type: None,
            delay_ms: 1,
            message: SendMessage::Empty,
        };
        let bytes = encode_send_request(&request).expect("bounded request encodes");

        let mut unsupported = bytes.clone();
        unsupported[8..12].copy_from_slice(&(DELAYED_SEND_WIRE_VERSION + 1).to_le_bytes());
        assert_eq!(
            decode_send_request(&unsupported),
            Err(SendCodecError::UnsupportedVersion)
        );
        assert_eq!(
            decode_send_request(&bytes[..bytes.len() - 1]),
            Err(SendCodecError::Invalid)
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(decode_send_request(&trailing), Err(SendCodecError::Invalid));
    }

    #[test]
    fn delayed_send_message_round_trips_and_rejects_malformed_fields() {
        let base = SendRequest {
            send_id: Some(SendId::new("s1").expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new("s1").expect("test send id is valid")),
                ..Event::external(EventName::new("ready").expect("test event name is valid"))
            },
            target: None,
            processor_type: None,
            delay_ms: 5,
            message: SendMessage::Empty,
        };
        let fields = SendRequest {
            message: SendMessage::Params(vec![
                NamedValue {
                    name: "a".into(),
                    value: DataValue::String("1".into()),
                },
                NamedValue {
                    name: "a".into(),
                    value: DataValue::Null,
                },
            ]),
            ..base.clone()
        };
        let content = SendRequest {
            message: SendMessage::Content,
            ..base.clone()
        };
        let mut encodings = Vec::new();
        for request in [&base, &fields, &content] {
            let bytes = encode_send_request(request).expect("bounded message encodes");
            assert_eq!(decode_send_request(&bytes).as_ref(), Ok(request));
            encodings.push(bytes);
        }
        assert_ne!(
            encodings[0], encodings[1],
            "field lists stay distinct from empty"
        );
        assert_ne!(
            encodings[0], encodings[2],
            "content stays distinct from empty"
        );

        let unnamed = SendRequest {
            message: SendMessage::Params(vec![NamedValue {
                name: String::new(),
                value: DataValue::Null,
            }]),
            ..base.clone()
        };
        assert_eq!(encode_send_request(&unnamed), Err(SendCodecError::Invalid));
        let too_many = SendRequest {
            message: SendMessage::Params(vec![
                NamedValue {
                    name: "a".into(),
                    value: DataValue::Null,
                };
                MAX_SEND_MESSAGE_PARAMS + 1
            ]),
            ..base
        };
        assert_eq!(
            encode_send_request(&too_many),
            Err(SendCodecError::TooLarge)
        );

        // An unknown message tag fails closed.
        let mut unknown = encodings[0].clone();
        let last = unknown.len() - 1;
        unknown[last] = 3;
        assert_eq!(decode_send_request(&unknown), Err(SendCodecError::Invalid));
    }

    #[test]
    fn delayed_send_wire_header_is_format_tag_then_version() {
        let request = SendRequest {
            send_id: None,
            event: Event::external(EventName::new("ready").expect("test event name is valid")),
            target: None,
            processor_type: None,
            delay_ms: 0,
            message: SendMessage::Empty,
        };
        let bytes = encode_send_request(&request).expect("bounded request encodes");
        assert_eq!(&bytes[..8], b"SCXSEND\0");
        assert_eq!(&bytes[8..12], &8_u32.to_le_bytes());
        assert_eq!(bytes[12], 0, "the SCXML kind byte follows the header");
        assert_eq!(DELAYED_SEND_WIRE_VERSION, 8);

        // Hard cutover: a pre-v6 record (legacy version-bearing magic) is not
        // a delayed-send record at all, and a v5 header under the current tag
        // is an unsupported version.
        let mut legacy_magic = bytes.clone();
        legacy_magic[..8].copy_from_slice(b"SCXSND04");
        legacy_magic[8..12].copy_from_slice(&5_u32.to_le_bytes());
        assert_eq!(
            decode_send_request(&legacy_magic),
            Err(SendCodecError::Invalid)
        );
        let mut legacy_version = bytes;
        legacy_version[8..12].copy_from_slice(&5_u32.to_le_bytes());
        assert_eq!(
            decode_send_request(&legacy_version),
            Err(SendCodecError::UnsupportedVersion)
        );
    }

    #[test]
    fn delayed_send_wire_rejects_mismatched_event_send_id() {
        let request = SendRequest {
            send_id: Some(SendId::new("request-id").expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new("event-id").expect("test send id is valid")),
                ..Event::external(EventName::new("ready").expect("test event name is valid"))
            },
            target: None,
            processor_type: None,
            delay_ms: 1,
            message: SendMessage::Empty,
        };
        assert_eq!(encode_send_request(&request), Err(SendCodecError::Invalid));
        // An event may carry no send id while the request keeps a generated
        // one (SCXML C.1), but never an id the request lacks.
        let unnamed = SendRequest {
            event: Event::external(EventName::new("ready").expect("test event name is valid")),
            ..request.clone()
        };
        let bytes = encode_send_request(&unnamed).expect("an unnamed event encodes");
        assert_eq!(decode_send_request(&bytes), Ok(unnamed));
        let orphan = SendRequest {
            send_id: None,
            ..request
        };
        assert_eq!(encode_send_request(&orphan), Err(SendCodecError::Invalid));
    }

    /// Value types are called from inside boundary spans (and from hot
    /// engine paths), so constructors, getters, formatting, and validation
    /// emit no telemetry of their own.
    #[test]
    fn value_types_emit_no_telemetry() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());

        let event_name = EventName::new("ready").expect("test event name is valid");
        assert_eq!(
            EventName::new("two words"),
            Err(NameError::ContainsWhitespace)
        );
        assert_eq!(event_name.as_str(), "ready");
        assert_eq!(event_name.to_string(), "ready");
        assert_eq!(
            NamespaceUri::new("relative"),
            Err(NamespaceUriError::Relative)
        );
        let trace_context = TraceContext::new(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
            Some("vendor=value".into()),
            None,
            BaggagePolicy::Drop,
        )
        .expect("test trace context is canonical");
        assert_eq!(trace_context.validate(), Ok(()));
        let external = Event::external(event_name.clone());
        assert_eq!(external.validate(), Ok(()));
        let _internal = Event::internal(event_name);
        assert_eq!(parse_delay_ms("1s"), Ok(1_000));
        assert_eq!(
            unqualified_executable_content_extension().validate(),
            Err(ExecutableContentExtensionError::UnexpectedEnd)
        );

        assert_eq!(recorder.observations(), Vec::new());
    }
}
