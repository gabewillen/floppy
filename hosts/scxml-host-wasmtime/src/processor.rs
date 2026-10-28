//! Event I/O Processor components (SCXML 6.1, 6.2) under the session host.
//!
//! A `<send>` whose type names neither the SCXML nor the `BasicHTTP`
//! processor goes to the `stateforward:scxml/event-processor` component a
//! package binds to that exact type URI (or one of its aliases). Nothing is
//! hard-wired: the embedder binds each processor the way it binds an
//! invoked service ([`ProcessorComponents`]), and a type no package binds is
//! `error.execution` (SCXML 6.2.5).
//!
//! * **Instances** — a session's first `<send>` of a bound type opens one
//!   instance of its component for that session (`event-processor-api.open`),
//!   in its own store on a thread of its own, driven by the Component Model
//!   async event loop as an invoked service is. The instance lives as long
//!   as its session: the session's end drops it, closing every connection it
//!   holds.
//! * **Sends** — each `<send>` is one `send` subtask (`check` for a delayed
//!   one when it is evaluated, then `send` when it falls due). The host waits
//!   until the instance is idle: a call resolved by then decides the
//!   `<send>` (`ok` accepts it; `invalid-input` and `payload-too-large`
//!   raise `error.execution`; any other error raises `error.communication`
//!   with the send id); a call still pending accepts it, and an error it
//!   resolves with later raises `error.communication` with the send id.
//! * **Events** — what the instance returns on its stream reaches its
//!   session's external queue with `_event.origintype` set to the bound type
//!   URI.
//! * **Network** — `outbound-http` as an invoked service has it, and
//!   `outbound-websocket`, both admitted only to the origins the package
//!   declares (`ws` and `wss` URLs are admitted as `http` and `https`). A
//!   live session runs each connection on a thread of its own
//!   ([`ProcessorComponents::with_outbound_websocket`]) and delivers each
//!   answer as a recorded external input, which a replay delivers from its
//!   record instead; a batch run offers no WebSocket (`unavailable`).
//!
//! Telemetry: one boundary span per open, send, and connection with closed
//! labels only; type URIs, URLs, and messages never become attributes.

use std::sync::Arc;
use std::sync::mpsc;

use crate::outbound_http::ExtensionHttp;
use crate::{ArtifactError, ArtifactKind, AuthenticatedComponent, HostError, InvalidLimit};

/// Maximum processor packages one session host binds.
const MAX_BOUND_PROCESSORS: usize = 64;
/// Maximum aliases one processor package declares.
const MAX_PROCESSOR_ALIASES: usize = 8;
/// Maximum bytes of one alias.
const MAX_PROCESSOR_ALIAS_BYTES: usize = 64;

/// Maximum bytes of one WebSocket message either way (the WIT
/// `outbound-websocket.message` bound).
pub const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 1_048_576;

/// The declarative package of one bound Event I/O Processor (its
/// `stateforward.scxml.processor-package/v1` manifest, already verified by
/// the embedder): the host binds `processor_uri` and `aliases` and grants
/// `origins`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessorPackage {
    /// Exact processor type URI; the binding key.
    pub processor_uri: String,
    /// Short names a `<send type>` may use in place of the URI.
    pub aliases: Vec<String>,
    /// Package version.
    pub package_version: String,
    /// Processor component artifact identifier.
    pub processor_artifact: String,
    /// WIT world the component implements.
    pub wit_world: String,
    /// WIT version.
    pub wit_version: String,
    /// The origins or URL prefixes the component's requests and
    /// connections may reach (canonicalized like an
    /// [`crate::HttpAllowlist`]); binding the package consents to them.
    pub origins: Vec<String>,
}

impl ProcessorPackage {
    /// Whether `processor_type` (a `<send type>` value) names this package:
    /// its URI or one of its aliases, compared byte for byte.
    #[must_use]
    pub fn answers(&self, processor_type: &str) -> bool {
        self.processor_uri == processor_type
            || self.aliases.iter().any(|alias| alias == processor_type)
    }
}

/// One bound processor.
#[derive(Clone, Debug)]
struct ProcessorBinding {
    package: ProcessorPackage,
    component: AuthenticatedComponent,
}

/// Event I/O Processor components one session host links, keyed by exact
/// type URI and alias.
#[derive(Clone, Debug)]
pub struct ProcessorComponents {
    bindings: Vec<ProcessorBinding>,
    http: Option<Arc<dyn ExtensionHttp>>,
    websocket: Option<Arc<dyn WebSocketTransport>>,
    call_fuel: u64,
}

impl Default for ProcessorComponents {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`ProcessorComponents`] hands the session host.
pub(crate) struct ProcessorParts {
    pub(crate) bindings: Vec<(ProcessorPackage, AuthenticatedComponent)>,
    pub(crate) http: Option<Arc<dyn ExtensionHttp>>,
    pub(crate) websocket: Option<Arc<dyn WebSocketTransport>>,
    pub(crate) call_fuel: u64,
}

/// Whether `alias` is a short processor name: 1 to 64 bytes of ASCII
/// letters, digits, `.`, `_`, and `-`.
fn is_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= MAX_PROCESSOR_ALIAS_BYTES
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl ProcessorComponents {
    /// No bindings, no outbound HTTP or WebSocket, and
    /// [`crate::DEFAULT_EXTENSION_CALL_FUEL`] between two inputs.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            http: None,
            websocket: None,
            call_fuel: crate::DEFAULT_EXTENSION_CALL_FUEL,
        }
    }

    /// The transport admitted `outbound-http` requests go through, as
    /// [`crate::ServiceComponents::with_outbound_http`]. Without one (the
    /// default) every request is refused with `unavailable`.
    #[must_use]
    pub fn with_outbound_http(mut self, transport: Arc<dyn ExtensionHttp>) -> Self {
        self.http = Some(transport);
        self
    }

    /// The transport admitted `outbound-websocket` connections go through.
    /// Without one (the default) every connection is refused with
    /// `unavailable`; with one, a live session runs each admitted connection
    /// on a thread of its own and delivers what the peer sends as external
    /// inputs of the instance. A batch run refuses every connection.
    #[must_use]
    pub fn with_outbound_websocket(mut self, transport: Arc<dyn WebSocketTransport>) -> Self {
        self.websocket = Some(transport);
        self
    }

    /// Binds `component` to `package.processor_uri` and its aliases,
    /// compared byte for byte with each `<send type>`.
    ///
    /// # Errors
    ///
    /// [`HostError::Artifact`]: a component that is not
    /// [`ArtifactKind::ProcessorComponent`]
    /// ([`ArtifactError::ArtifactKindMismatch`]); a URI that is no absolute
    /// URI, an alias that is not a short name, a type the SCXML or
    /// `BasicHTTP` processor answers, a type bound twice, past 64 bindings or
    /// 8 aliases, or invalid declared origins
    /// ([`ArtifactError::NamespaceBinding`]).
    pub fn bind(
        mut self,
        package: ProcessorPackage,
        component: &AuthenticatedComponent,
    ) -> Result<Self, HostError> {
        if component.kind() != ArtifactKind::ProcessorComponent {
            return Err(ArtifactError::ArtifactKindMismatch.into());
        }
        let builtin = |name: &str| {
            scxml_types::classify_processor_type(name) != scxml_types::ProcessorKind::Unknown
        };
        let names = || std::iter::once(&package.processor_uri).chain(&package.aliases);
        if !scxml_types::is_service_type_uri(&package.processor_uri)
            || package.aliases.len() > MAX_PROCESSOR_ALIASES
            || !package.aliases.iter().all(|alias| is_alias(alias))
            || names().any(|name| builtin(name))
            || names().enumerate().any(|(index, name)| {
                names()
                    .skip(index.saturating_add(1))
                    .any(|other| other == name)
            })
            || self.bindings.len() >= MAX_BOUND_PROCESSORS
            || self
                .bindings
                .iter()
                .any(|bound| names().any(|name| bound.package.answers(name)))
            || package.origins.len() > crate::MAX_DECLARED_ITEMS
            || crate::HttpAllowlist::new(&package.origins).is_err()
        {
            return Err(ArtifactError::NamespaceBinding.into());
        }
        self.bindings.push(ProcessorBinding {
            package,
            component: component.clone(),
        });
        Ok(self)
    }

    /// Sets the instruction fuel an instance may run between two inputs.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] for zero fuel.
    pub fn with_call_fuel(mut self, fuel: u64) -> Result<Self, HostError> {
        if fuel == 0 {
            return Err(HostError::InvalidLimit(InvalidLimit::Fuel));
        }
        self.call_fuel = fuel;
        Ok(self)
    }

    /// Whether no processor is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub(crate) fn into_parts(self) -> ProcessorParts {
        ProcessorParts {
            bindings: self
                .bindings
                .into_iter()
                .map(|binding| (binding.package, binding.component))
                .collect(),
            http: self.http,
            websocket: self.websocket,
            call_fuel: self.call_fuel,
        }
    }
}

/// One WebSocket message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketMessage {
    /// A text frame.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
}

impl WebSocketMessage {
    /// The message's payload bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Binary(bytes) => bytes.len(),
        }
    }

    /// Whether the payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A close frame: its code and reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketClose {
    /// The close code.
    pub code: u16,
    /// The reason, at most 123 bytes.
    pub reason: String,
}

/// One admitted connection request, exactly as the transport opens it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketRequest {
    /// The canonical `ws` or `wss` URL.
    pub url: String,
    /// Handshake header fields, in order (never `host`, `connection`,
    /// `upgrade`, or a `sec-websocket-*` field).
    pub headers: Vec<(String, String)>,
    /// Subprotocols offered, in preference order.
    pub protocols: Vec<String>,
}

/// Why a connection failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSocketFailure {
    /// The transport refused the URL.
    NotAllowed,
    /// The opening handshake failed: connect, TLS, status, or timeout.
    Handshake,
    /// The connection was lost after it opened.
    Transport,
    /// The peer sent a message past [`MAX_WEBSOCKET_MESSAGE_BYTES`].
    MessageTooLarge,
}

/// What a transport answered one connection, in order: `Opened` (or a
/// failure), then any number of `Message`s, then `Closed` or `Failed`;
/// nothing follows either.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketAnswer {
    /// The opening handshake completed with this selected subprotocol.
    Opened(Option<String>),
    /// One message from the peer.
    Message(WebSocketMessage),
    /// The connection closed: the peer's close frame, or none after the
    /// host's own close.
    Closed(Option<WebSocketClose>),
    /// The connection failed.
    Failed(WebSocketFailure),
}

impl WebSocketAnswer {
    /// Whether nothing follows this answer for its connection.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        matches!(self, Self::Closed(_) | Self::Failed(_))
    }
}

/// What the host hands a connection to send.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketOutgoing {
    /// Send one message.
    Message(WebSocketMessage),
    /// Close the connection with code 1000.
    Close,
}

/// The embedder's WebSocket client.
pub trait WebSocketTransport: std::fmt::Debug + Send + Sync {
    /// Opens `request` and runs the connection until it ends, on the
    /// calling thread: hands each answer to `answer` as it arrives, in order
    /// ([`WebSocketAnswer`]), and sends what `outgoing` yields, in order. A
    /// [`WebSocketOutgoing::Close`] or a disconnected `outgoing` (the
    /// instance ended) closes the connection with code 1000.
    fn connect(
        &self,
        request: &WebSocketRequest,
        outgoing: mpsc::Receiver<WebSocketOutgoing>,
        answer: &mut dyn FnMut(WebSocketAnswer),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_short_names() {
        assert!(is_alias("websocket"));
        assert!(is_alias("speech.stream-v1_2"));
        assert!(!is_alias(""));
        assert!(!is_alias("a b"));
        assert!(!is_alias("https://example.com/"));
        assert!(!is_alias(&"a".repeat(MAX_PROCESSOR_ALIAS_BYTES + 1)));
    }

    #[test]
    fn a_package_answers_its_uri_and_aliases_only() {
        let package = ProcessorPackage {
            processor_uri: "https://example.com/processors/ws".into(),
            aliases: vec!["ws".into()],
            package_version: "1.0.0".into(),
            processor_artifact: "ws.wasm".into(),
            wit_world: "stateforward:scxml/event-processor".into(),
            wit_version: "0.2.0".into(),
            origins: Vec::new(),
        };
        assert!(package.answers("https://example.com/processors/ws"));
        assert!(package.answers("ws"));
        assert!(!package.answers("WS"));
        assert!(!package.answers("https://example.com/processors/ws/"));
    }
}
