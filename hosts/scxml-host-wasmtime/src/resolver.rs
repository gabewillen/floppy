//! The host of a repository resolver component
//! (`stateforward:scxml/resolver`, feature `http`): [`ResolverComponent`]
//! runs its `resolve` and `fetch` and answers its `wasi:http/client`
//! requests.
//!
//! # Security posture
//!
//! The component's only capability is WASI 0.3 `wasi:http/client.send`, and
//! this host decides what it reaches:
//!
//! * deny by default: a request is sent only when the embedder's
//!   [`HttpAllowlist`] admits its URL (a `GET` with no body); any other is
//!   answered `HTTP-request-denied` before anything is sent;
//! * no redirect is followed here: a 3xx is the response, so every hop the
//!   component follows is admitted again;
//! * no ambient authority: no cookie, proxy from the environment, or
//!   credential; the component cannot set `authorization`, `cookie`, or
//!   `proxy-authorization` (forbidden headers), and only the embedder's
//!   token, when it gives one, is added, to requests for its one origin;
//! * bounded: a connect and a total timeout ([`ExtensionHttpConfig`]) and a
//!   response head of at most [`crate::MAX_RESPONSE_HEAD_BYTES`]; the body
//!   streams into the component, which reads it within its call's limit.
//!   TLS is rustls verified against the operating system's trust store.
//!
//! Trust is the embedder's: it verifies every byte the component returns.
//!
//! # Execution
//!
//! Each call instantiates the component in a store of its own with
//! [`RESOLVER_CALL_FUEL`] fuel and runs it on the calling thread until it
//! returns; a request blocks that thread while it is sent and its body
//! read.
//!
//! # Telemetry
//!
//! No span of its own: the embedder's resolution span covers each call, and
//! a failure surfaces as the call's [`HostError`]. No URL, header, or body is
//! recorded.

use std::io::Read as _;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use bytes::Bytes;
use http_body_util::BodyExt as _;
use wasmtime::component::{HasSelf, Linker, ResourceTable};
use wasmtime_wasi_http::{
    DEFAULT_FORBIDDEN_HEADERS, Error as HttpError, RequestOptions, WasiBody, WasiHttpCtx,
    WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
};

use crate::{
    ExtensionHttpConfig, HostError, HttpAllowlist, TrustedComponentDigest, WasmtimeHostConfig,
};

#[allow(
    missing_docs,
    reason = "generated WIT bindings re-exported as `resolver_api`; the canonical documentation is scxml.wit"
)]
mod resolver_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "resolver",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "wasi:http": wasmtime_wasi_http::p3::bindings::http,
        },
        require_store_data_send: true,
    });
}

/// The `resolver-api` types a call takes and returns.
pub mod resolver_api {
    pub use super::resolver_bindings::exports::stateforward::scxml::resolver_api::{
        Artifact, Release,
    };
}

use resolver_api::{Artifact, Release};

/// Fuel for one call: far more than the component's own work (URL building
/// and JSON reading) and the copying of a body of 2 GiB, so the network's
/// time bounds end a call first; it bounds a component that loops.
pub const RESOLVER_CALL_FUEL: u64 = 1 << 40;

/// The size of each body chunk handed to the component.
const BODY_CHUNK_BYTES: usize = 64 * 1024;

/// What the host admits and how it sends.
struct Admission {
    allow: HttpAllowlist,
    agent: ureq::Agent,
    /// The one origin the token goes to, and the token.
    token: Option<(url::Origin, String)>,
}

/// One repository resolver component, compiled once and instantiated per
/// call.
pub struct ResolverComponent {
    engine: wasmtime::Engine,
    pre: resolver_bindings::ResolverPre<ResolverStoreData>,
    admission: Arc<Admission>,
}

impl std::fmt::Debug for ResolverComponent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolverComponent")
            .finish_non_exhaustive()
    }
}

impl ResolverComponent {
    /// Authenticates `component_bytes` by `trusted_digest` (kind
    /// [`crate::ArtifactKind::ResolverGithub`]), compiles them, and links
    /// them so that their requests reach only what `allow` admits, within
    /// `http`'s (nonzero) time bounds; `token` (an origin and a bearer
    /// token) is added to the admitted requests for exactly that origin.
    ///
    /// # Errors
    ///
    /// [`HostError::Artifact`] for bytes that do not match the digest or
    /// its size bound; [`HostError::Wasmtime`] when the component does not
    /// compile or does not target the `resolver` world.
    pub fn new(
        component_bytes: &[u8],
        trusted_digest: TrustedComponentDigest,
        allow: HttpAllowlist,
        http: ExtensionHttpConfig,
        token: Option<(url::Origin, String)>,
    ) -> Result<Self, HostError> {
        let component = crate::artifact::authenticate_by_digest(
            component_bytes,
            trusted_digest,
            crate::ArtifactKind::ResolverGithub,
        )?;
        let engine = crate::component_engine(&WasmtimeHostConfig::default())?;
        let compiled = wasmtime::component::Component::new(&engine, component.bytes())?;
        let mut linker = Linker::<ResolverStoreData>::new(&engine);
        crate::bindings::stateforward::scxml::types::add_to_linker::<_, HasSelf<ResolverStoreData>>(
            &mut linker,
            |state| state,
        )?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
        let pre = resolver_bindings::ResolverPre::new(linker.instantiate_pre(&compiled)?)?;
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_connect(Some(http.connect_timeout))
            .timeout_global(Some(http.total_timeout))
            .max_response_header_size(crate::MAX_RESPONSE_HEAD_BYTES)
            .user_agent("scx")
            .tls_config(tls)
            .build()
            .new_agent();
        Ok(Self {
            engine,
            pre,
            admission: Arc::new(Admission {
                allow,
                agent,
                token,
            }),
        })
    }

    /// `resolver-api.resolve`: the release the tagless package `uri`
    /// resolves to (`tag`'s, or the latest); `None` when the component does
    /// not serve `uri`.
    ///
    /// # Errors
    ///
    /// [`HostError::Component`] with the component's detail for its
    /// failure; [`HostError::Wasmtime`] for a trap (fuel exhaustion
    /// included).
    pub fn resolve(
        &self,
        configuration: &[(String, String)],
        uri: &str,
        tag: Option<&str>,
    ) -> Result<Option<Release>, HostError> {
        let (configuration, uri, tag) = (
            configuration.to_vec(),
            uri.to_owned(),
            tag.map(str::to_owned),
        );
        self.call(async move |accessor, guest| {
            guest
                .stateforward_scxml_resolver_api()
                .call_resolve(accessor, configuration, uri, tag)
                .await
        })
    }

    /// `resolver-api.fetch`: the exact bytes of `artifact` of `uri` at
    /// `release`, at most `limit` bytes; `None` when the component does not
    /// serve `uri`.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`]; [`HostError::Component`] also when the bytes
    /// exceed `limit`.
    pub fn fetch(
        &self,
        configuration: &[(String, String)],
        uri: &str,
        release: &Release,
        artifact: &Artifact,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, HostError> {
        let (configuration, uri, release, artifact) = (
            configuration.to_vec(),
            uri.to_owned(),
            release.clone(),
            artifact.clone(),
        );
        let fetched = self.call(async move |accessor, guest| {
            guest
                .stateforward_scxml_resolver_api()
                .call_fetch(accessor, configuration, uri, release, artifact, limit)
                .await
        })?;
        if fetched
            .as_ref()
            .is_some_and(|bytes| u64::try_from(bytes.len()).map_or(true, |length| length > limit))
        {
            return Err(HostError::Component(format!(
                "the resolver returned more than its {limit}-byte bound"
            )));
        }
        Ok(fetched)
    }

    /// Instantiates the component in a fresh store and runs `call` to
    /// completion on this thread.
    fn call<R: Send + 'static>(
        &self,
        call: impl AsyncFnOnce(
            &wasmtime::component::Accessor<ResolverStoreData>,
            &resolver_bindings::Resolver,
        ) -> wasmtime::Result<
            Result<R, crate::bindings::stateforward::scxml::types::ComponentError>,
        > + Send
        + 'static,
    ) -> Result<R, HostError> {
        // No memory bound beyond wasm32's 4 GiB: a call returns a body of up
        // to 2 GiB − 1 bytes. Every growth is charged to the call's copy
        // allowance like any store's.
        let limits = wasmtime::StoreLimitsBuilder::new()
            .trap_on_grow_failure(true)
            .build();
        let mut store = wasmtime::Store::new(
            &self.engine,
            ResolverStoreData {
                limits: crate::memory_growth::MeteredLimits::metered(
                    limits,
                    None,
                    RESOLVER_CALL_FUEL,
                ),
                table: ResourceTable::new(),
                http: WasiHttpCtx::new(),
                hooks: Hooks(Arc::clone(&self.admission)),
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_fuel(RESOLVER_CALL_FUEL)?;
        let guest = crate::service::block_on(self.pre.instantiate_async(&mut store))?;
        let returned = crate::service::block_on(
            store.run_concurrent(async move |accessor| call(accessor, &guest).await),
        )??;
        returned.map_err(|error| {
            HostError::Component(
                error
                    .detail
                    .unwrap_or_else(|| format!("the resolver failed ({:?})", error.code)),
            )
        })
    }
}

/// One call's store.
struct ResolverStoreData {
    limits: crate::memory_growth::MeteredLimits,
    table: ResourceTable,
    http: WasiHttpCtx,
    hooks: Hooks,
}

impl crate::bindings::stateforward::scxml::types::Host for ResolverStoreData {}

impl WasiHttpView for ResolverStoreData {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.hooks,
        }
    }
}

/// The host's answer to the component's requests.
struct Hooks(Arc<Admission>);

type Sent = Box<
    dyn Future<
            Output = Result<
                (
                    http::Response<WasiBody>,
                    Box<dyn Future<Output = Result<(), HttpError>> + Send>,
                ),
                HttpError,
            >,
        > + Send,
>;

impl WasiHttpHooks for Hooks {
    fn is_forbidden_header(&mut self, name: &http::HeaderName) -> bool {
        DEFAULT_FORBIDDEN_HEADERS.contains(name)
            || *name == http::header::AUTHORIZATION
            || *name == http::header::COOKIE
    }

    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: Box<dyn Future<Output = Result<(), HttpError>> + Send>,
    ) -> Sent {
        let admission = Arc::clone(&self.0);
        Box::new(async move {
            let response = send(&admission, &request)?;
            let done: Box<dyn Future<Output = Result<(), HttpError>> + Send> =
                Box::new(async { Ok(()) });
            Ok((response, done))
        })
    }
}

/// Sends one admitted `GET` and returns its head with a body that streams
/// from the connection.
fn send(
    admission: &Admission,
    request: &http::Request<WasiBody>,
) -> Result<http::Response<WasiBody>, HttpError> {
    if request.method() != http::Method::GET {
        return Err(HttpError::HttpRequestMethodInvalid);
    }
    let url = crate::allowlist::parse_http_url(&request.uri().to_string())
        .ok_or(HttpError::HttpRequestUriInvalid)?;
    if !admission.allow.admits_canonical(&url) {
        return Err(HttpError::HttpRequestDenied);
    }
    let mut builder = admission.agent.get(url.as_str());
    for (name, value) in request.headers() {
        if *name != http::header::HOST {
            builder = builder.header(name, value);
        }
    }
    if let Some((origin, token)) = &admission.token
        && url.origin() == *origin
    {
        builder = builder.header("authorization", &format!("Bearer {token}"));
    }
    let response = builder.call().map_err(|error| match error {
        ureq::Error::Timeout(_) => HttpError::ConnectionTimeout,
        ureq::Error::HostNotFound => HttpError::DestinationNotFound,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => HttpError::TlsProtocolError,
        _ => HttpError::ConnectionRefused,
    })?;
    let (head, body) = response.into_parts();
    let mut answer = http::Response::builder().status(head.status);
    for (name, value) in &head.headers {
        answer = answer.header(name, value);
    }
    answer
        .body(
            Streamed {
                reader: Some(body.into_reader()),
            }
            .boxed_unsync(),
        )
        .map_err(|_| HttpError::HttpProtocolError)
}

/// A response body read from the connection as the component asks for it:
/// each poll reads one chunk, blocking the calling thread (the call's
/// thread) until it arrives or the total timeout ends it.
struct Streamed {
    reader: Option<ureq::BodyReader<'static>>,
}

impl http_body::Body for Streamed {
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, HttpError>>> {
        let Some(reader) = self.reader.as_mut() else {
            return Poll::Ready(None);
        };
        let mut chunk = vec![0; BODY_CHUNK_BYTES];
        match reader.read(&mut chunk) {
            Ok(0) => {
                self.reader = None;
                Poll::Ready(None)
            }
            Ok(read) => {
                chunk.truncate(read);
                Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from(chunk)))))
            }
            Err(_) => {
                self.reader = None;
                Poll::Ready(Some(Err(HttpError::ConnectionReadTimeout)))
            }
        }
    }
}
