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

/// Default time for one whole extension-component request, from resolution to
/// the end of the response body (model calls are slow; the `execute` call
/// blocks on it).
pub const DEFAULT_EXTENSION_TOTAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Configuration of one [`ExtensionHttpClient`].
#[derive(Clone, Copy, Debug)]
pub struct ExtensionHttpConfig {
    /// Time to establish one connection.
    pub connect_timeout: Duration,
    /// Time for one whole request, including the response body.
    pub total_timeout: Duration,
}

impl Default for ExtensionHttpConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHttpConfig {
    /// [`DEFAULT_CONNECT_TIMEOUT`] and [`DEFAULT_EXTENSION_TOTAL_TIMEOUT`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            total_timeout: DEFAULT_EXTENSION_TOTAL_TIMEOUT,
        }
    }
}

/// The outbound HTTP transport of extension components
/// ([`crate::ExtensionHttp`], supplied through
/// [`crate::ExtensionComponents::with_outbound_http`]).
///
/// The session host admits every request before this transport sees it
/// (the element's declared origins, or an origin a reference's value
/// supplied), so the transport sends exactly the canonical request it is
/// given (an absolute `http`/`https` URL without userinfo; anything else is
/// refused). No redirect is followed (a 3xx is the response), and no cookie,
/// credential, or environment proxy is used. TLS
/// is rustls verified against the operating system's trust store. Each
/// request runs on the calling (session) thread, bounded by the connect
/// and total timeouts; the response head by [`MAX_RESPONSE_HEAD_BYTES`]
/// and the body by [`crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`]. The
/// caller's `extension-http-fetch` span covers the request; nothing about
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

impl ExtensionHttpClient {
    /// A client with `config`.
    ///
    /// # Errors
    ///
    /// [`HttpClientError::TimeoutInvalid`] for a zero timeout.
    pub fn new(config: ExtensionHttpConfig) -> Result<Self, HttpClientError> {
        if config.connect_timeout.is_zero() || config.total_timeout.is_zero() {
            return Err(HttpClientError::TimeoutInvalid);
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
            .user_agent("scxml-extension")
            .tls_config(tls)
            .build()
            .new_agent();
        Ok(Self { agent })
    }
}

impl crate::ExtensionHttp for ExtensionHttpClient {
    fn fetch(
        &self,
        request: &crate::ExtensionHttpRequest,
    ) -> Result<crate::ExtensionHttpResponse, crate::ExtensionHttpFailure> {
        let Some(url) = parse_http_url(&request.url) else {
            return Err(crate::ExtensionHttpFailure::NotAllowed);
        };
        let response = match request.method {
            crate::ExtensionHttpMethod::Get => {
                let mut builder = self.agent.get(url.as_str());
                for (name, value) in &request.headers {
                    builder = builder.header(name.as_str(), value.as_str());
                }
                builder.call()
            }
            crate::ExtensionHttpMethod::Post => {
                let mut builder = self.agent.post(url.as_str());
                for (name, value) in &request.headers {
                    builder = builder.header(name.as_str(), value.as_str());
                }
                builder.send(request.body.as_slice())
            }
        };
        let mut response = response.map_err(|_| crate::ExtensionHttpFailure::Transport)?;
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
}
