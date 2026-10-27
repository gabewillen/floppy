//! The allowlisted `BasicHTTP` network client of live sessions (feature
//! `http`): the [`OutboundHttp`] transport that carries a session's POST
//! (encoded by the `BasicHTTP` component, SCXML C.2) to another host.
//!
//! # Security posture
//!
//! It mirrors the browser host (`hosts/scxml-host-browser/src/basichttp.js`
//! and `url.js`):
//!
//! * deny by default: a target is sent only when an [`HttpAllowlist`] entry
//!   admits it;
//! * one canonical form: the target is parsed once with the WHATWG URL
//!   parser ([`canonical_http_url`]), so `..`, `%2e%2e`, and backslash
//!   segments resolve exactly as a browser's fetch resolves them, and the
//!   string the allowlist judged is the string that is requested; a target
//!   that is not an absolute `http`/`https` URL without userinfo is denied;
//! * no ambient authority: no cookies, no credentials, no proxy from the
//!   environment, no redirect is ever followed (a 3xx answer is a non-2xx
//!   failure, so a redirect can never leave the allowlist);
//! * bounded: at most [`HttpClientConfig::max_in_flight`] requests at once
//!   (one more is refused at once), a connect and a total timeout, a
//!   response head of at most [`MAX_RESPONSE_HEAD_BYTES`], and a response
//!   body that is never read (only the status matters, as in the browser's
//!   fire-and-forget fetch). The request itself is bounded by the component
//!   (`basichttp.wit` `http-request`).
//!
//! # Threads
//!
//! [`OutboundHttp::begin`] runs on the session thread and returns at once;
//! each accepted request runs on its own short-lived thread and settles its
//! [`CommunicationFailure`]: a transport failure or a non-2xx status owes
//! the sender `error.communication`.
//!
//! # Telemetry
//!
//! One `basichttp-network-post` span per request, a child of the send that
//! issued it, with only the closed outcome (`transformation-completed`, or
//! `input-rejected` plus an error category for a refusal or failure). No
//! URL, host, header, status, or body is ever recorded.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use crate::allowlist::{HttpAllowlist, parse_http_url};
use crate::basichttp_types::HttpRequest;
use crate::{CommunicationFailure, OutboundHttp, TransportUnavailable};

/// Default time to establish one connection (TCP and TLS).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default time for one whole request, from resolution to the response
/// head.
pub const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Default maximum requests in flight at once.
pub const DEFAULT_MAX_IN_FLIGHT: usize = 64;
/// Upper bound of [`HttpClientConfig::max_in_flight`].
pub const MAX_IN_FLIGHT: usize = 1_024;
/// Maximum bytes of a response head (status line and header fields).
pub const MAX_RESPONSE_HEAD_BYTES: usize = 16 * 1024;

/// Configuration of one [`NetworkClient`].
#[derive(Clone, Debug)]
pub struct HttpClientConfig {
    /// The targets it may send to.
    pub allow: HttpAllowlist,
    /// Time to establish one connection.
    pub connect_timeout: Duration,
    /// Time for one whole request.
    pub total_timeout: Duration,
    /// Requests in flight at once (1..=[`MAX_IN_FLIGHT`]).
    pub max_in_flight: usize,
}

impl HttpClientConfig {
    /// The defaults with `allow` as the allowlist.
    #[must_use]
    pub fn new(allow: HttpAllowlist) -> Self {
        Self {
            allow,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            total_timeout: DEFAULT_TOTAL_TIMEOUT,
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
        }
    }
}

/// An invalid [`HttpClientConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpClientError {
    /// A zero timeout.
    TimeoutInvalid,
    /// `max_in_flight` outside 1..=[`MAX_IN_FLIGHT`].
    InFlightInvalid,
}

impl std::fmt::Display for HttpClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimeoutInvalid => formatter.write_str("HTTP timeouts must be greater than zero"),
            Self::InFlightInvalid => write!(
                formatter,
                "requests in flight must be between 1 and {MAX_IN_FLIGHT}"
            ),
        }
    }
}

impl std::error::Error for HttpClientError {}

/// The allowlisted network transport of live sessions (see the module
/// documentation). Share it through [`crate::LiveHttp::outbound`].
pub struct NetworkClient {
    agent: ureq::Agent,
    allow: HttpAllowlist,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: usize,
}

impl std::fmt::Debug for NetworkClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkClient")
            .field("allow", &self.allow)
            .field("max_in_flight", &self.max_in_flight)
            .finish_non_exhaustive()
    }
}

impl NetworkClient {
    /// A client with `config`.
    ///
    /// # Errors
    ///
    /// [`HttpClientError`] for a zero timeout or an out-of-range in-flight
    /// bound.
    pub fn new(config: HttpClientConfig) -> Result<Self, HttpClientError> {
        if config.connect_timeout.is_zero() || config.total_timeout.is_zero() {
            return Err(HttpClientError::TimeoutInvalid);
        }
        if !(1..=MAX_IN_FLIGHT).contains(&config.max_in_flight) {
            return Err(HttpClientError::InFlightInvalid);
        }
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_connect(Some(config.connect_timeout))
            .timeout_global(Some(config.total_timeout))
            .max_response_header_size(MAX_RESPONSE_HEAD_BYTES)
            .user_agent("scxml-basichttp")
            .tls_config(tls)
            .build()
            .new_agent();
        Ok(Self {
            agent,
            allow: config.allow,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: config.max_in_flight,
        })
    }

    /// Requests in flight now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// One reserved in-flight slot, released on drop.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn refused(category: ErrorCategory) -> TransportUnavailable {
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeBasicHttpNetworkPost,
    );
    span.event(SignalEvent::InputRejected);
    span.fail(category);
    TransportUnavailable
}

/// Performs one POST; whether the recipient answered 2xx.
fn post(agent: &ureq::Agent, url: &url::Url, request: &HttpRequest) -> bool {
    let mut builder = agent.post(url.as_str());
    for header in &request.headers {
        builder = builder.header(header.name.as_str(), header.value.as_str());
    }
    // The body is never read: only the status matters.
    builder
        .send(request.body.as_slice())
        .is_ok_and(|response| response.status().is_success())
}

impl OutboundHttp for NetworkClient {
    fn begin(
        &self,
        request: HttpRequest,
        outcome: CommunicationFailure,
    ) -> Result<(), TransportUnavailable> {
        // The component only ever posts; anything else is not ours to send.
        if request.method != "POST" {
            return Err(refused(ErrorCategory::InvalidInput));
        }
        let Some(url) = parse_http_url(&request.uri) else {
            return Err(refused(ErrorCategory::InvalidInput));
        };
        if !self.allow.admits_canonical(&url) {
            return Err(refused(ErrorCategory::InvalidInput));
        }
        let reserved = self
            .in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.max_in_flight).then_some(count + 1)
            });
        if reserved.is_err() {
            return Err(refused(ErrorCategory::Budget));
        }
        let slot = Slot(Arc::clone(&self.in_flight));
        let agent = self.agent.clone();
        let parent = Context::current();
        let spawned = std::thread::Builder::new()
            .name("scxml-basichttp-post".into())
            .spawn(move || {
                let _slot = slot;
                let _parent = parent.attach();
                let span = scxml_telemetry::observe(
                    Module::HostWasmtime,
                    Operation::HostWasmtimeBasicHttpNetworkPost,
                );
                if post(&agent, &url, &request) {
                    span.event(SignalEvent::TransformationCompleted);
                    outcome.succeeded();
                } else {
                    span.event(SignalEvent::InputRejected);
                    span.fail(ErrorCategory::Platform);
                    outcome.report();
                }
            });
        // A thread that could not start sent nothing: refuse now (the slot
        // and the unsettled outcome were dropped with the closure).
        spawned
            .map(|_| ())
            .map_err(|_| refused(ErrorCategory::Platform))
    }
}

/// Configuration of the resolver's agent ([`crate::ResolverComponent`]).
#[derive(Clone, Copy, Debug)]
pub struct ExtensionHttpConfig {
    /// Time to establish one connection.
    pub connect_timeout: Duration,
    /// Time for one whole request, including the response body.
    pub total_timeout: Duration,
}

/// The outbound HTTP transport of invoked services
/// ([`crate::ExtensionHttp`], supplied through
/// [`crate::ServiceComponents::with_outbound_http`]).
///
/// The session host admits every request before this transport sees it
/// (the service's declared origins, or an origin a reference's value
/// supplied), so the transport sends exactly the canonical request it is
/// given (an absolute `http`/`https` URL without userinfo; anything else is
/// refused). No redirect is followed (a 3xx is the response), and no cookie,
/// credential, or environment proxy is used. TLS
/// is rustls verified against the operating system's trust store.
/// Connections are kept alive and pooled per origin (ureq's pool), so
/// consecutive requests to one origin reuse one connection and its TLS
/// session instead of opening a new one each. Each
/// request runs on its invocation's transport thread with no time limit of
/// its own: the invocation bounds it, since leaving the invoking state
/// cancels the invocation and closes the connection the request holds
/// (SCXML 6.4), and an unreachable host fails when the operating system
/// gives up connecting.
/// The response head is bounded by [`MAX_RESPONSE_HEAD_BYTES`] and the body
/// by [`crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`]. The
/// caller's `service-http-request` span covers the request; nothing about
/// it is recorded here.
pub struct ExtensionHttpClient {
    agent: ureq::Agent,
}

impl std::fmt::Debug for ExtensionHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionHttpClient")
            .finish_non_exhaustive()
    }
}

impl Default for ExtensionHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHttpClient {
    /// A client with no connect or total timeout.
    #[must_use]
    pub fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .max_response_header_size(MAX_RESPONSE_HEAD_BYTES)
            .user_agent("scxml-extension")
            .tls_config(tls)
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::Connector::chain(
                cancellable::Connector,
                ureq::unversioned::transport::RustlsConnector::default(),
            ),
            ureq::unversioned::resolver::DefaultResolver::default(),
        );
        Self { agent }
    }
}

/// Whether the request this transport thread performs was already
/// cancelled (nothing is sent for it).
fn already_cancelled() -> bool {
    crate::service::request_cancel().is_some_and(|cancel| cancel.is_cancelled())
}

impl crate::ExtensionHttp for ExtensionHttpClient {
    /// An invoked service's request runs on a transport thread that holds
    /// its invocation's cancellation (SCXML 6.4), and the connection it
    /// holds, new or pooled, is the one that cancel closes: once the
    /// request is cancelled, a connection not yet open is never used, and
    /// the one in use is shut down, so the call stops waiting for the
    /// response at once and answers
    /// [`crate::ExtensionHttpFailure::Transport`]. (Resolving the host and
    /// opening the TCP connection are not interrupted; they end when the
    /// operating system answers or gives up, and nothing is sent after a
    /// cancel.)
    fn fetch(
        &self,
        request: &crate::ExtensionHttpRequest,
    ) -> Result<crate::ExtensionHttpResponse, crate::ExtensionHttpFailure> {
        if already_cancelled() {
            return Err(crate::ExtensionHttpFailure::Transport);
        }
        perform(&self.agent, request)
    }

    /// Hands each read of the body to `part` as it arrives (at most 64 KiB
    /// a read), within [`crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`] in
    /// all, on a connection the invocation's cancel closes, as
    /// [`crate::ExtensionHttp::fetch`].
    fn send(
        &self,
        request: &crate::ExtensionHttpRequest,
        part: &mut dyn FnMut(Result<crate::ExtensionHttpPart, crate::ExtensionHttpFailure>),
    ) {
        if already_cancelled() {
            return part(Err(crate::ExtensionHttpFailure::Transport));
        }
        stream(&self.agent, request, part);
    }
}

/// Bytes of one body read of a streamed response.
const STREAM_READ_BYTES: usize = 64 * 1024;

/// Sends one checked request through `agent` and streams its response to
/// `part`: its status, each body read, and its end, or the failure that
/// ends it.
fn stream(
    agent: &ureq::Agent,
    request: &crate::ExtensionHttpRequest,
    part: &mut dyn FnMut(Result<crate::ExtensionHttpPart, crate::ExtensionHttpFailure>),
) {
    use std::io::Read as _;
    let mut response = match call(agent, request) {
        Ok(response) => response,
        Err(failure) => return part(Err(failure)),
    };
    part(Ok(crate::ExtensionHttpPart::Status(
        response.status().as_u16(),
    )));
    let mut reader = response.body_mut().as_reader();
    let mut received = 0_usize;
    let mut buffer = vec![0_u8; STREAM_READ_BYTES];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return part(Ok(crate::ExtensionHttpPart::End)),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return part(Err(crate::ExtensionHttpFailure::Transport)),
        };
        received = received.saturating_add(read);
        if received > crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES {
            return part(Err(crate::ExtensionHttpFailure::ResponseTooLarge));
        }
        let Some(bytes) = buffer.get(..read) else {
            return part(Err(crate::ExtensionHttpFailure::Transport));
        };
        part(Ok(crate::ExtensionHttpPart::Body(bytes.to_vec())));
    }
}

/// Sends one checked request through `agent`: its response head, or the
/// failure.
fn call(
    agent: &ureq::Agent,
    request: &crate::ExtensionHttpRequest,
) -> Result<ureq::http::Response<ureq::Body>, crate::ExtensionHttpFailure> {
    let Some(url) = parse_http_url(&request.url) else {
        return Err(crate::ExtensionHttpFailure::NotAllowed);
    };
    let response = match request.method {
        crate::ExtensionHttpMethod::Get => {
            let mut builder = agent.get(url.as_str());
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            builder.call()
        }
        crate::ExtensionHttpMethod::Post => {
            let mut builder = agent.post(url.as_str());
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            builder.send(request.body.as_slice())
        }
    };
    response.map_err(|_| crate::ExtensionHttpFailure::Transport)
}

/// Performs one checked request through `agent`, reading the body within
/// its bound.
fn perform(
    agent: &ureq::Agent,
    request: &crate::ExtensionHttpRequest,
) -> Result<crate::ExtensionHttpResponse, crate::ExtensionHttpFailure> {
    {
        let mut response = call(agent, request)?;
        let status = response.status().as_u16();
        let limit = u64::try_from(crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES)
            .map_err(|_| crate::ExtensionHttpFailure::Transport)?;
        let body = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|error| match error {
                ureq::Error::BodyExceedsLimit(_) => crate::ExtensionHttpFailure::ResponseTooLarge,
                _ => crate::ExtensionHttpFailure::Transport,
            })?;
        Ok(crate::ExtensionHttpResponse { status, body })
    }
}

/// The TCP connector of [`ExtensionHttpClient`]: it opens a connection
/// like ureq's own TCP connector, and each connection registers its
/// shutdown with the cancellation of the request using it, which a pooled
/// connection changes with each request that takes it.
mod cancellable {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::{Arc, Mutex, PoisonError};

    use ureq::unversioned::transport::{
        Buffers, ConnectionDetails, LazyBuffers, NextTimeout, Transport,
    };

    use crate::service::CancelSignal;

    fn cancelled() -> ureq::Error {
        ureq::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "the request was cancelled",
        ))
    }

    /// Opens one TCP connection whose shutdown the cancellation of the
    /// request using it owns.
    #[derive(Debug)]
    pub(super) struct Connector;

    impl ureq::unversioned::transport::Connector<()> for Connector {
        type Out = Connection;

        fn connect(
            &self,
            details: &ConnectionDetails,
            _chained: Option<()>,
        ) -> Result<Option<Self::Out>, ureq::Error> {
            if super::already_cancelled() {
                return Err(cancelled());
            }
            let timeout = details.timeout.not_zero().map(|after| *after);
            let mut failure = None;
            let mut opened = None;
            for address in &details.addrs {
                let attempt = match timeout {
                    Some(timeout) => TcpStream::connect_timeout(address, timeout),
                    None => TcpStream::connect(address),
                };
                match attempt {
                    Ok(stream) => {
                        opened = Some(stream);
                        break;
                    }
                    Err(error) => failure = Some(error),
                }
            }
            let Some(stream) = opened else {
                return Err(match failure {
                    Some(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                        ureq::Error::Timeout(ureq::Timeout::Connect)
                    }
                    Some(error) => ureq::Error::Io(error),
                    None => cancelled(),
                });
            };
            if details.config.no_delay() {
                stream.set_nodelay(true)?;
            }
            let mut connection = Connection {
                stream,
                buffers: LazyBuffers::new(
                    details.config.input_buffer_size(),
                    details.config.output_buffer_size(),
                ),
                read_timeout: None,
                write_timeout: None,
                bound: None,
            };
            // Fails when the request was cancelled while connecting.
            connection.bind()?;
            Ok(Some(connection))
        }
    }

    /// The cancellation a connection is bound to, and the clone of its
    /// socket that the cancellation shuts down while the binding holds it.
    struct Binding {
        cancel: Arc<CancelSignal>,
        armed: Arc<Mutex<Option<TcpStream>>>,
    }

    /// One cancellable TCP connection (ureq's `TcpTransport`, whose socket
    /// the cancellation of the request using it can shut down from another
    /// thread).
    pub(super) struct Connection {
        stream: TcpStream,
        buffers: LazyBuffers,
        read_timeout: Option<std::time::Duration>,
        write_timeout: Option<std::time::Duration>,
        bound: Option<Binding>,
    }

    impl Connection {
        /// Binds the connection to the cancellation of the request this
        /// thread performs, which a cancel then shuts down (at once when it
        /// was already cancelled, which fails).
        fn bind(&mut self) -> Result<(), ureq::Error> {
            let current = crate::service::request_cancel();
            let same = match (&self.bound, &current) {
                (Some(bound), Some(cancel)) => Arc::ptr_eq(&bound.cancel, cancel),
                (None, None) => true,
                _ => false,
            };
            if !same {
                self.release();
                if let Some(cancel) = current {
                    let armed = Arc::new(Mutex::new(Some(self.stream.try_clone()?)));
                    let shutdown = Arc::clone(&armed);
                    cancel.on_cancel(move || {
                        let held = shutdown
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .take();
                        if let Some(stream) = held {
                            // Already closed is as good as closed.
                            let _ = stream.shutdown(Shutdown::Both);
                        }
                    });
                    self.bound = Some(Binding { cancel, armed });
                }
            }
            match &self.bound {
                Some(bound) if bound.cancel.is_cancelled() => Err(cancelled()),
                _ => Ok(()),
            }
        }

        /// Unbinds the connection: a later cancel of the request that used
        /// it no longer shuts it down.
        fn release(&mut self) {
            if let Some(bound) = self.bound.take() {
                bound
                    .armed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
            }
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            self.release();
        }
    }

    impl std::fmt::Debug for Connection {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.debug_struct("Connection").finish_non_exhaustive()
        }
    }

    /// `error` as ureq reports it: a would-block or timed-out read or write
    /// is the timeout `timeout` names.
    fn io_error(error: std::io::Error, timeout: &NextTimeout) -> ureq::Error {
        match error.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                ureq::Error::Timeout(timeout.reason)
            }
            _ => ureq::Error::Io(error),
        }
    }

    /// Whether nothing waits on an idle `stream`: no unexpected byte, no
    /// close, and no error.
    fn probe(stream: &TcpStream) -> std::io::Result<bool> {
        stream.set_nonblocking(true)?;
        let mut byte = [0_u8; 1];
        let open = match (&*stream).read(&mut byte) {
            Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
            Ok(_) => false,
        };
        stream.set_nonblocking(false)?;
        Ok(open)
    }

    impl Transport for Connection {
        fn buffers(&mut self) -> &mut dyn Buffers {
            &mut self.buffers
        }

        fn transmit_output(
            &mut self,
            amount: usize,
            timeout: NextTimeout,
        ) -> Result<(), ureq::Error> {
            self.bind()?;
            let next = timeout.not_zero().map(|after| *after);
            if next != self.write_timeout {
                self.stream.set_write_timeout(next)?;
                self.write_timeout = next;
            }
            let output = self
                .buffers
                .output()
                .get(..amount)
                .ok_or_else(|| ureq::Error::Io(std::io::ErrorKind::InvalidInput.into()))?;
            self.stream
                .write_all(output)
                .map_err(|error| io_error(error, &timeout))
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
            self.bind()?;
            let next = timeout.not_zero().map(|after| *after);
            if next != self.read_timeout {
                self.stream.set_read_timeout(next)?;
                self.read_timeout = next;
            }
            let input = self.buffers.input_append_buf();
            let amount = self
                .stream
                .read(input)
                .map_err(|error| io_error(error, &timeout))?;
            self.buffers.input_appended(amount);
            Ok(amount > 0)
        }

        /// Asked when the connection returns to the pool and when it is
        /// taken from it: the request it served is done with it, so it is
        /// unbound, and it is open while no byte and no close waits on its
        /// socket (ureq's own probe).
        fn is_open(&mut self) -> bool {
            self.release();
            probe(&self.stream).unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowlist::{AllowlistReason, MAX_ALLOWLIST_ENTRIES, canonical_http_url};

    fn allow(entries: &[&str]) -> HttpAllowlist {
        HttpAllowlist::new(entries).expect("valid allowlist")
    }

    #[test]
    fn targets_are_canonicalized_before_the_allowlist() {
        let list = allow(&["https://api.example/scxml", "http://127.0.0.1:8080"]);
        for (target, admitted) in [
            ("https://api.example/scxml", true),
            ("https://api.example/scxml/", true),
            ("https://api.example/scxml/events?x=1", true),
            ("HTTPS://API.EXAMPLE:443/scxml/a", true),
            ("https://api.example/scxml-admin", false),
            ("https://api.example/scxml/../admin", false),
            ("https://api.example/scxml/%2e%2e/admin", false),
            ("https://api.example/scxml/%2E%2E/admin", false),
            ("https://api.example/scxml\\..\\admin", false),
            ("https://api.example/other/../scxml/ok", true),
            ("https://user:pw@api.example/scxml", false),
            ("https://user@api.example/scxml", false),
            ("http://api.example/scxml", false),
            ("https://api.example:8443/scxml", false),
            ("https://api.example.evil/scxml", false),
            ("http://127.0.0.1:8080/anything", true),
            ("http://127.0.0.1:8081/anything", false),
            ("http://localhost:8080/anything", false),
            ("ftp://api.example/scxml", false),
            ("/scxml", false),
            ("not a url", false),
        ] {
            assert_eq!(list.admits(target), admitted, "{target}");
        }
        assert!(!HttpAllowlist::default().admits("https://api.example/"));
    }

    #[test]
    fn a_trailing_slash_prefix_admits_only_below_it() {
        let list = allow(&["https://api.example/hooks/"]);
        assert!(list.admits("https://api.example/hooks/a"));
        assert!(list.admits("https://api.example/hooks/"));
        assert!(!list.admits("https://api.example/hooks"));
        assert!(!list.admits("https://api.example/hooksx"));
    }

    #[test]
    fn invalid_entries_are_refused_with_their_position() {
        for (entries, reason) in [
            (&["ftp://api.example"][..], AllowlistReason::NotHttp),
            (&["api.example"][..], AllowlistReason::NotHttp),
            (
                &["https://a.example", "https://u:p@api.example"][..],
                AllowlistReason::Userinfo,
            ),
            (
                &["https://api.example/?q"][..],
                AllowlistReason::QueryOrFragment,
            ),
            (
                &["https://api.example/#f"][..],
                AllowlistReason::QueryOrFragment,
            ),
        ] {
            let error = HttpAllowlist::new(entries).expect_err("refused");
            assert_eq!(error.reason, reason, "{entries:?}");
            assert_eq!(error.index, entries.len() - 1);
        }
        let many = vec!["https://api.example"; MAX_ALLOWLIST_ENTRIES + 1];
        assert_eq!(
            HttpAllowlist::new(&many).expect_err("too many").reason,
            AllowlistReason::TooMany
        );
    }

    #[test]
    fn canonical_urls_drop_nothing_the_parser_keeps() {
        assert_eq!(
            canonical_http_url("HTTP://Api.Example:80/a/./b/../c"),
            Some("http://api.example/a/c".to_owned())
        );
        assert_eq!(canonical_http_url("https://u@api.example/"), None);
        assert_eq!(canonical_http_url("mailto:x@y"), None);
    }

    #[test]
    fn a_service_request_has_no_time_limit() {
        let timeouts = ExtensionHttpClient::new().agent.config().timeouts();
        assert_eq!(
            (
                timeouts.global,
                timeouts.per_call,
                timeouts.resolve,
                timeouts.connect,
                timeouts.send_request,
                timeouts.send_body,
                timeouts.recv_response,
                timeouts.recv_body,
            ),
            (None, None, None, None, None, None, None, None),
            "the invocation's lifetime is the only bound"
        );
    }

    #[test]
    fn configurations_are_bounded() {
        let mut config = HttpClientConfig::new(HttpAllowlist::default());
        config.max_in_flight = 0;
        assert_eq!(
            NetworkClient::new(config.clone()).err(),
            Some(HttpClientError::InFlightInvalid)
        );
        config.max_in_flight = 1;
        config.total_timeout = Duration::ZERO;
        assert_eq!(
            NetworkClient::new(config).err(),
            Some(HttpClientError::TimeoutInvalid)
        );
    }

    /// A loopback server that accepts one connection only and answers each
    /// request on it with `200` and `ok`, keeping it alive; the request
    /// whose path is `/hang` gets no answer. It reports each request's path
    /// and ends when the client closes the connection.
    fn one_connection_server() -> (
        u16,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
        let port = listener.local_addr().expect("an address").port();
        let (paths_tx, paths) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the client connects");
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("a clone"));
            let mut writer = stream;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 {
                        return;
                    }
                    if header == "\r\n" {
                        break;
                    }
                }
                let hang = path == "/hang";
                if paths_tx.send(path).is_err() {
                    return;
                }
                if !hang
                    && writer
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .is_err()
                {
                    return;
                }
            }
        });
        (port, paths, server)
    }

    /// Fetches `path` from `port` on a transport thread of its own whose
    /// request has `cancel`.
    fn fetch_on_thread(
        client: &Arc<ExtensionHttpClient>,
        port: u16,
        path: &str,
        cancel: &Arc<crate::service::CancelSignal>,
    ) -> std::thread::JoinHandle<Result<crate::ExtensionHttpResponse, crate::ExtensionHttpFailure>>
    {
        let client = Arc::clone(client);
        let cancel = Arc::clone(cancel);
        let request = crate::ExtensionHttpRequest {
            method: crate::ExtensionHttpMethod::Get,
            url: format!("http://127.0.0.1:{port}{path}"),
            headers: Vec::new(),
            body: Vec::new(),
        };
        std::thread::spawn(move || {
            crate::service::set_request_cancel(Some(cancel));
            crate::ExtensionHttp::fetch(&*client, &request)
        })
    }

    #[test]
    fn consecutive_service_requests_reuse_one_connection() {
        let (port, paths, server) = one_connection_server();
        let client = Arc::new(ExtensionHttpClient::new());
        for path in ["/a", "/b", "/c"] {
            let cancel = Arc::new(crate::service::CancelSignal::default());
            let response = fetch_on_thread(&client, port, path, &cancel)
                .join()
                .expect("the request thread ends")
                .expect("a response on the one connection");
            assert_eq!(
                (response.status, response.body.as_slice()),
                (200, &b"ok"[..])
            );
            assert_eq!(paths.recv().expect("the request arrived"), path);
        }
        drop(client);
        server.join().expect("the server ends");
    }

    #[test]
    fn cancelling_a_finished_request_leaves_its_pooled_connection_open() {
        let (port, paths, server) = one_connection_server();
        let client = Arc::new(ExtensionHttpClient::new());
        let first = Arc::new(crate::service::CancelSignal::default());
        fetch_on_thread(&client, port, "/a", &first)
            .join()
            .expect("the request thread ends")
            .expect("a response");
        // The first request's invocation ends after its answer.
        first.cancel();
        let second = Arc::new(crate::service::CancelSignal::default());
        let response = fetch_on_thread(&client, port, "/b", &second)
            .join()
            .expect("the request thread ends")
            .expect("the pooled connection still answers");
        assert_eq!(response.status, 200);
        assert_eq!(paths.iter().take(2).collect::<Vec<_>>(), ["/a", "/b"]);
        drop(client);
        server.join().expect("the server ends");
    }

    #[test]
    fn cancelling_a_request_closes_the_pooled_connection_it_holds() {
        let (port, paths, server) = one_connection_server();
        let client = Arc::new(ExtensionHttpClient::new());
        let first = Arc::new(crate::service::CancelSignal::default());
        fetch_on_thread(&client, port, "/a", &first)
            .join()
            .expect("the request thread ends")
            .expect("a response");
        let second = Arc::new(crate::service::CancelSignal::default());
        let waiting = fetch_on_thread(&client, port, "/hang", &second);
        assert_eq!(paths.recv().expect("the first request arrived"), "/a");
        assert_eq!(
            paths
                .recv_timeout(Duration::from_secs(10))
                .expect("the second request arrived on the pooled connection"),
            "/hang"
        );
        let cancelled = std::time::Instant::now();
        second.cancel();
        assert_eq!(
            waiting.join().expect("the request thread ends"),
            Err(crate::ExtensionHttpFailure::Transport)
        );
        assert!(cancelled.elapsed() < Duration::from_secs(2));
        // The closed connection is never pooled: the server sees it end.
        server.join().expect("the server ends");
        drop(client);
    }

    #[test]
    fn an_already_cancelled_request_sends_nothing() {
        let (port, paths, server) = one_connection_server();
        let client = Arc::new(ExtensionHttpClient::new());
        let cancel = Arc::new(crate::service::CancelSignal::default());
        cancel.cancel();
        assert_eq!(
            fetch_on_thread(&client, port, "/a", &cancel)
                .join()
                .expect("the request thread ends"),
            Err(crate::ExtensionHttpFailure::Transport)
        );
        let live = Arc::new(crate::service::CancelSignal::default());
        fetch_on_thread(&client, port, "/b", &live)
            .join()
            .expect("the request thread ends")
            .expect("a response");
        assert_eq!(paths.recv().expect("one request arrived"), "/b");
        drop(client);
        server.join().expect("the server ends");
    }

    /// Times `SCXML_HTTP_PROBE_COUNT` (default 10) sequential GETs of
    /// `SCXML_HTTP_PROBE_URL`, each on a transport thread of its own that
    /// holds a cancellation, as an invoked service's requests run.
    #[test]
    #[ignore = "a live probe: set SCXML_HTTP_PROBE_URL"]
    fn probe_request_latency() {
        let url = std::env::var("SCXML_HTTP_PROBE_URL").expect("SCXML_HTTP_PROBE_URL");
        let count = std::env::var("SCXML_HTTP_PROBE_COUNT")
            .ok()
            .and_then(|count| count.parse().ok())
            .unwrap_or(10_usize);
        let client = Arc::new(ExtensionHttpClient::new());
        let mut times = Vec::new();
        for _ in 0..count {
            let client = Arc::clone(&client);
            let request = crate::ExtensionHttpRequest {
                method: crate::ExtensionHttpMethod::Get,
                url: url.clone(),
                headers: Vec::new(),
                body: Vec::new(),
            };
            let started = std::time::Instant::now();
            let status = std::thread::spawn(move || {
                crate::service::set_request_cancel(Some(Arc::new(
                    crate::service::CancelSignal::default(),
                )));
                crate::ExtensionHttp::fetch(&*client, &request)
                    .expect("a response")
                    .status
            })
            .join()
            .expect("the request thread ends");
            times.push(started.elapsed());
            eprintln!(
                "request {}: {status} in {:?}",
                times.len(),
                started.elapsed()
            );
        }
        let rest = times.get(1..).unwrap_or_default();
        let mean = rest.iter().sum::<Duration>() / u32::try_from(rest.len().max(1)).unwrap_or(1);
        eprintln!("first {:?}, mean of the rest {mean:?}", times.first());
    }
}
