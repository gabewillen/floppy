//! Outbound HTTP for invoked services (`stateforward:scxml/outbound-http`)
//! and the run's `env` values they reference.
//!
//! * **Values and references** — the embedder holds the values of the
//!   variables a run grants ([`crate::ServiceComponents::with_values`]), each
//!   granted to exact package URIs ([`EnvVariable`]). A service component
//!   never receives one: when an `<invoke>` names a variable for one of its
//!   package's `env-slots` (`env:<slot>="NAME"` or `env:<slot>expr`,
//!   [`scxml_types::ENV_NAMESPACE_URI`]), the host hands the component an
//!   opaque reference, valid for that invocation, and substitutes the value
//!   only inside the invocation's `outbound-http` requests.
//! * **Admission** — deny by default: without an embedder-supplied transport
//!   ([`crate::ServiceComponents::with_outbound_http`]) every request is
//!   refused with `unavailable`. With one, [`admit_request`] substitutes the
//!   request's references, checks it against the WIT bounds, canonicalizes
//!   its URL, and admits it only to an origin or URL prefix the package
//!   declares (`http-origins`: binding a package consents to them) or to the
//!   origin a reference's value supplies entirely; every other request is
//!   refused before the transport sees it, so a value is never sent anywhere
//!   else.
//!
//! Executable-content extension components get none of this: an extension
//! element runs synchronously inside a microstep and does no I/O.

use scxml_telemetry::ErrorCategory;

use crate::bindings::stateforward::scxml::types::{ComponentError, ErrorCode};
use crate::component::component_error;
use crate::extension::MAX_NAMESPACE_URI_BYTES;
pub use crate::service::outbound_http;
use crate::{HostError, InvalidLimit};

/// Maximum bytes of one environment variable name.
pub const MAX_ENVIRONMENT_NAME_BYTES: usize = 1_024;
/// Maximum bytes of one environment variable value.
pub const MAX_ENVIRONMENT_VALUE_BYTES: usize = 65_536;
/// Maximum bytes of one `outbound-http` request URL.
pub const MAX_EXTENSION_HTTP_URL_BYTES: usize = 16_384;
/// Maximum header fields of one `outbound-http` request.
pub const MAX_EXTENSION_HTTP_HEADERS: usize = 64;
/// Maximum bytes of one `outbound-http` header name.
pub const MAX_EXTENSION_HTTP_HEADER_NAME_BYTES: usize = 256;
/// Maximum bytes of one `outbound-http` header value.
pub const MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES: usize = 8_192;
/// Maximum bytes of one `outbound-http` request body (network egress;
/// every build).
pub const MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes of one `outbound-http` response body (network
/// ingress; every build).
pub const MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Header names a component may not set: the transport owns them.
const TRANSPORT_HEADERS: [&str; 4] = ["host", "content-length", "transfer-encoding", "connection"];

/// The method of one `outbound-http` request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionHttpMethod {
    /// `GET` (no body).
    Get,
    /// `POST`.
    Post,
}

/// One checked `outbound-http` request: the method, the canonical
/// WHATWG serialization of an absolute `http`/`https` URL without userinfo, the
/// header fields within the `outbound-http` bounds, and the body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionHttpRequest {
    /// The method.
    pub method: ExtensionHttpMethod,
    /// The canonical target URL.
    pub url: String,
    /// Header fields, in order.
    pub headers: Vec<(String, String)>,
    /// The body (empty for `GET`).
    pub body: Vec<u8>,
}

/// One `outbound-http` response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionHttpResponse {
    /// The final status (100..=599).
    pub status: u16,
    /// The body, at most [`MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`].
    pub body: Vec<u8>,
}

/// Why a [`ExtensionHttp`] transport answered no response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionHttpFailure {
    /// The transport does not admit the target (nothing was sent).
    NotAllowed,
    /// Connecting, TLS, sending, or receiving failed or timed out.
    Transport,
    /// The response body exceeds [`MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`].
    ResponseTooLarge,
}

/// The outbound HTTP transport an embedder grants to invoked services
/// (`stateforward:scxml/outbound-http`). The host admits every request
/// ([`admit_request`]) before calling [`ExtensionHttp::fetch`]: the
/// implementation must bound its own time and the response body, follow no
/// redirect, and add no ambient credentials. With the `http` feature, `ExtensionHttpClient`
/// is the allowlisted implementation.
pub trait ExtensionHttp: std::fmt::Debug + Send + Sync {
    /// Performs one request.
    ///
    /// # Errors
    ///
    /// [`ExtensionHttpFailure`] when no response is answered.
    fn fetch(
        &self,
        request: &ExtensionHttpRequest,
    ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure>;
}

/// One variable a run grants: its name, its value, and the exact package URIs
/// (invoked-service type URIs) whose `<invoke>`s may reference it. A slot reference to the variable from any
/// other package is absent, exactly like a variable with no value. Its `Debug`
/// output never prints the value.
#[derive(Clone, Eq, PartialEq)]
pub struct EnvVariable {
    name: String,
    value: String,
    packages: Vec<String>,
}

impl std::fmt::Debug for EnvVariable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnvVariable")
            .field("name", &self.name)
            .field("packages", &self.packages)
            .finish_non_exhaustive()
    }
}

impl EnvVariable {
    /// `name` holding `value`, granted to exactly `packages` (sorted and
    /// checked by [`ExtensionEnvironment::new`]).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        packages: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut packages: Vec<String> = packages.into_iter().collect();
        packages.sort();
        Self {
            name: name.into(),
            value: value.into(),
            packages,
        }
    }

    /// The variable's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The exact package URIs granted the variable, sorted.
    #[must_use]
    pub fn packages(&self) -> &[String] {
        &self.packages
    }

    fn is_valid(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= MAX_ENVIRONMENT_NAME_BYTES
            && !self.name.contains(['=', '\0'])
            && self.value.len() <= MAX_ENVIRONMENT_VALUE_BYTES
            && !self.value.contains('\0')
            && !self.packages.is_empty()
            && self.packages.windows(2).all(|pair| pair[0] != pair[1])
            && self.packages.iter().all(|uri| {
                uri.len() <= MAX_NAMESPACE_URI_BYTES && scxml_types::NamespaceUri::new(uri).is_ok()
            })
    }
}

/// The values of the variables a run grants invoked services' `env` slots,
/// each scoped to the package URIs it is granted to: exactly what the
/// embedder passes, never the host process environment. Components never
/// receive them, only references ([`crate::ServiceComponents::with_values`]),
/// and only for a variable granted to the referencing `<invoke>`'s package.
/// [`ExtensionEnvironment::none`] (the default) holds nothing, so every slot
/// is absent. Its `Debug` output names the variables and their packages
/// only, never a value, so formatting it (or a type that holds it, such as
/// [`crate::ServiceComponents`]) never prints a secret.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ExtensionEnvironment {
    variables: Vec<EnvVariable>,
}

impl std::fmt::Debug for ExtensionEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionEnvironment")
            .field("variables", &self.variables)
            .finish_non_exhaustive()
    }
}

impl ExtensionEnvironment {
    /// No variables.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Exactly `variables`, sorted by name.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] ([`InvalidLimit::Environment`]) for a
    /// duplicate name, an empty name or one containing `=` or NUL, a name over
    /// [`MAX_ENVIRONMENT_NAME_BYTES`], a value over
    /// [`MAX_ENVIRONMENT_VALUE_BYTES`] or containing NUL, or a variable
    /// granted to no package, to one package twice, or to a package URI that
    /// is not an absolute URI within [`MAX_NAMESPACE_URI_BYTES`].
    pub fn new(variables: impl IntoIterator<Item = EnvVariable>) -> Result<Self, HostError> {
        let mut variables: Vec<EnvVariable> = variables.into_iter().collect();
        variables.sort_by(|left, right| left.name.cmp(&right.name));
        let invalid = variables
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
            || !variables.iter().all(EnvVariable::is_valid);
        if invalid {
            return Err(HostError::InvalidLimit(InvalidLimit::Environment));
        }
        Ok(Self { variables })
    }

    /// The variables, sorted by name.
    #[must_use]
    pub fn variables(&self) -> &[EnvVariable] {
        &self.variables
    }

    /// The value held for `name` when it is granted to `package` (exact
    /// URI); `None` when it is unheld or granted only to other packages.
    pub(crate) fn held(&self, package: &str, name: &str) -> Option<&str> {
        self.variables
            .binary_search_by(|held| held.name.as_str().cmp(name))
            .ok()
            .and_then(|index| self.variables.get(index))
            .filter(|held| {
                held.packages
                    .binary_search_by(|granted| granted.as_str().cmp(package))
                    .is_ok()
            })
            .map(|held| held.value.as_str())
    }
}

/// Maximum entries of a [`crate::ServicePackage`]'s `env_slots` and of its
/// `http_origins` (the manifest bound).
pub const MAX_DECLARED_ITEMS: usize = 32;
/// Maximum bytes of one `env` slot name (the manifest bound).
const MAX_ENV_SLOT_BYTES: usize = 64;
/// Maximum parts of one `outbound-http` URL, header value, or body.
pub const MAX_EXTENSION_HTTP_PARTS: usize = 64;
/// The suffix of an `env:<slot>expr` attribute.
const EXPR_SUFFIX: &str = "expr";

/// An `env-slots` entry: `[a-z][a-z0-9-]*`, at most 64 bytes, never ending in
/// `expr`.
pub(crate) fn is_env_slot(slot: &str) -> bool {
    slot.len() <= MAX_ENV_SLOT_BYTES
        && !slot.ends_with(EXPR_SUFFIX)
        && slot
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && slot
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A variable name an `env` slot may name: `[A-Za-z_][A-Za-z0-9_]*`.
pub(crate) fn is_env_name(name: &str) -> bool {
    name.len() <= MAX_ENVIRONMENT_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Whether `name` is an RFC 9110 token within its byte bound.
fn is_header_name(name: &str) -> bool {
    (1..=MAX_EXTENSION_HTTP_HEADER_NAME_BYTES).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// One `outbound-http` request with every reference substituted, and the
/// origin a reference's value supplied entirely, if any.
#[derive(Debug)]
pub(crate) struct Substituted {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    supplied_origin: Option<url::Origin>,
}

/// Substitutes every reference of `request` with the value this call holds
/// for it. The URL's origin is *supplied* when its first part is a reference
/// whose value is an absolute `http`/`https` URL without userinfo and the
/// rest of the URL, if any, begins its path, query, or fragment (`/`, `?`,
/// `#`): the run, not the document or the component, chose that origin.
///
/// # Errors
///
/// `invalid-input` for more than [`MAX_EXTENSION_HTTP_PARTS`] parts in a
/// field, or a reference the call did not receive.
pub(crate) fn substitute(
    request: outbound_http::HttpRequest,
    references: &[(u32, String)],
) -> Result<Substituted, ComponentError> {
    let invalid = |detail| component_error(ErrorCode::InvalidInput, detail);
    let value = |reference: u32| {
        references
            .iter()
            .find(|(held, _)| *held == reference)
            .map(|(_, value)| value.as_str())
            .ok_or_else(|| invalid("the request names a reference this execution did not receive"))
    };
    let text = |parts: &[outbound_http::TextPart]| -> Result<String, ComponentError> {
        if parts.len() > MAX_EXTENSION_HTTP_PARTS {
            return Err(invalid("a request field has too many parts"));
        }
        let mut text = String::new();
        for part in parts {
            match part {
                outbound_http::TextPart::Literal(literal) => text.push_str(literal),
                outbound_http::TextPart::Reference(reference) => text.push_str(value(*reference)?),
            }
        }
        Ok(text)
    };
    let url = text(&request.url)?;
    let supplied_origin = match request.url.first() {
        Some(outbound_http::TextPart::Reference(reference)) => {
            let supplied = value(*reference)?;
            let rest = url.get(supplied.len()..).unwrap_or_default();
            crate::allowlist::parse_http_url(supplied)
                .filter(|_| rest.is_empty() || rest.starts_with(['/', '?', '#']))
                .map(|supplied| supplied.origin())
        }
        _ => None,
    };
    let mut headers = Vec::with_capacity(request.headers.len().min(MAX_EXTENSION_HTTP_HEADERS + 1));
    for header in &request.headers {
        headers.push((header.name.clone(), text(&header.value)?));
    }
    if request.body.len() > MAX_EXTENSION_HTTP_PARTS {
        return Err(invalid("a request field has too many parts"));
    }
    let mut body = Vec::new();
    for part in request.body {
        match part {
            outbound_http::BodyPart::Bytes(bytes) => body.extend_from_slice(&bytes),
            outbound_http::BodyPart::Reference(reference) => {
                body.extend_from_slice(value(reference)?.as_bytes());
            }
        }
    }
    Ok(Substituted {
        method: request.method,
        url,
        headers,
        body,
        supplied_origin,
    })
}

/// Whether the canonical `url` may be requested: its package declares it
/// (`allow`), or a reference's value supplied its whole origin.
pub(crate) fn admitted(
    url: &str,
    allow: &crate::HttpAllowlist,
    supplied_origin: Option<&url::Origin>,
) -> bool {
    crate::allowlist::parse_http_url(url).is_some_and(|url| {
        allow.admits_canonical(&url)
            || supplied_origin.is_some_and(|origin| url.origin() == *origin)
    })
}

/// Checks one substituted `outbound-http` request against its WIT bounds and
/// canonicalizes its URL (the WHATWG serialization that is admitted and
/// requested).
///
/// # Errors
///
/// `invalid-input` for a request outside the bounds.
pub(crate) fn checked_http_request(
    request: Substituted,
) -> Result<ExtensionHttpRequest, ComponentError> {
    let invalid = |detail| Err(component_error(ErrorCode::InvalidInput, detail));
    let method = match request.method.as_str() {
        "GET" => ExtensionHttpMethod::Get,
        "POST" => ExtensionHttpMethod::Post,
        _ => return invalid("the request method is not GET or POST"),
    };
    if request.url.len() > MAX_EXTENSION_HTTP_URL_BYTES {
        return invalid("the request URL exceeds its byte bound");
    }
    let url = match url::Url::parse(&request.url) {
        Ok(url)
            if matches!(url.scheme(), "http" | "https")
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            String::from(url)
        }
        _ => return invalid("the request URL is not an absolute http or https URL"),
    };
    if request.headers.len() > MAX_EXTENSION_HTTP_HEADERS {
        return invalid("the request has too many header fields");
    }
    let mut headers = Vec::with_capacity(request.headers.len());
    for header in request.headers {
        if !is_header_name(&header.0)
            || TRANSPORT_HEADERS
                .iter()
                .any(|owned| header.0.eq_ignore_ascii_case(owned))
        {
            return invalid("a request header name is invalid or owned by the transport");
        }
        if header.1.len() > MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES
            || header.1.contains(['\r', '\n', '\0'])
        {
            return invalid("a request header value is invalid");
        }
        headers.push(header);
    }
    if request.body.len() > MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES
        || (method == ExtensionHttpMethod::Get && !request.body.is_empty())
    {
        return invalid("the request body exceeds its bound or accompanies GET");
    }
    Ok(ExtensionHttpRequest {
        method,
        url,
        headers,
        body: request.body,
    })
}

/// The refusal of a caller the host grants no transport.
pub(crate) fn no_transport() -> (ComponentError, ErrorCategory) {
    (
        component_error(
            ErrorCode::Unavailable,
            "the host offers this component no outbound HTTP",
        ),
        ErrorCategory::Unsupported,
    )
}

/// Admits one `outbound-http` request before anything is sent: substitutes
/// `references`, checks the WIT bounds, refuses it when the caller has no
/// transport (`has_transport`), canonicalizes the URL, and admits it only to
/// `allow` or an origin a reference's value supplied entirely (else calls
/// `refused` with the origin and refuses it). Returns the canonical request
/// a transport sends exactly.
pub(crate) fn admit_request(
    request: outbound_http::HttpRequest,
    references: &[(u32, String)],
    allow: &crate::HttpAllowlist,
    has_transport: bool,
    refused: impl FnOnce(&str),
) -> Result<ExtensionHttpRequest, (ComponentError, ErrorCategory)> {
    let substituted =
        substitute(request, references).map_err(|error| (error, ErrorCategory::InvalidInput))?;
    let supplied_origin = substituted.supplied_origin.clone();
    let request =
        checked_http_request(substituted).map_err(|error| (error, ErrorCategory::InvalidInput))?;
    if !has_transport {
        return Err(no_transport());
    }
    if !admitted(&request.url, allow, supplied_origin.as_ref()) {
        if let Some(url) = crate::allowlist::parse_http_url(&request.url) {
            refused(&url.origin().ascii_serialization());
        }
        return Err((
            component_error(
                ErrorCode::Unavailable,
                "the request target is neither declared by the package nor supplied by the run",
            ),
            ErrorCategory::InvalidInput,
        ));
    }
    Ok(request)
}

/// What one admitted request's transport outcome becomes for the caller: a
/// response within the status range and the response-body bound, or the
/// `outbound-http` failure (`unavailable` for a refusal or transport
/// failure, `payload-too-large` for an oversized body).
pub(crate) fn settle_response(
    outcome: Result<ExtensionHttpResponse, ExtensionHttpFailure>,
) -> Result<outbound_http::HttpResponse, (ComponentError, ErrorCategory)> {
    let response = outcome.map_err(|failure| match failure {
        ExtensionHttpFailure::NotAllowed => (
            component_error(
                ErrorCode::Unavailable,
                "the transport refused the request target",
            ),
            ErrorCategory::InvalidInput,
        ),
        ExtensionHttpFailure::Transport => (
            component_error(ErrorCode::Unavailable, "the HTTP transport failed"),
            ErrorCategory::Platform,
        ),
        ExtensionHttpFailure::ResponseTooLarge => (
            component_error(
                ErrorCode::PayloadTooLarge,
                "the response body exceeds its byte bound",
            ),
            ErrorCategory::Platform,
        ),
    })?;
    if !(100..=599).contains(&response.status) {
        return Err((
            component_error(ErrorCode::Unavailable, "the HTTP transport failed"),
            ErrorCategory::Platform,
        ));
    }
    if response.body.len() > MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES {
        return Err((
            component_error(
                ErrorCode::PayloadTooLarge,
                "the response body exceeds its byte bound",
            ),
            ErrorCategory::Platform,
        ));
    }
    Ok(outbound_http::HttpResponse {
        status: response.status,
        body: response.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_names_variables_and_never_prints_a_value() {
        const VALUE: &str = "sk-debug-must-not-print";
        let values =
            ExtensionEnvironment::new([EnvVariable::new("API_TOKEN", VALUE, ["urn:x".to_owned()])])
                .expect("a valid variable");
        let environment = format!("{values:?}");
        assert!(environment.contains("API_TOKEN"), "{environment}");
        for text in [
            environment,
            format!(
                "{:?}",
                crate::ServiceComponents::new().with_values(values.clone())
            ),
        ] {
            assert!(!text.contains(VALUE), "a value was formatted: {text}");
        }
    }

    fn http_request(
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> outbound_http::HttpRequest {
        outbound_http::HttpRequest {
            method: method.into(),
            url: vec![outbound_http::TextPart::Literal(url.into())],
            headers: headers
                .iter()
                .map(|(name, value)| outbound_http::HttpHeader {
                    name: (*name).into(),
                    value: vec![outbound_http::TextPart::Literal((*value).into())],
                })
                .collect(),
            body: vec![outbound_http::BodyPart::Bytes(body.to_vec())],
        }
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "mirrors the host's owned request"
    )]
    fn checked(
        request: outbound_http::HttpRequest,
    ) -> Result<ExtensionHttpRequest, ComponentError> {
        substitute(request, &[]).and_then(checked_http_request)
    }

    fn rejected(request: outbound_http::HttpRequest) -> Option<String> {
        let error = checked(request).err()?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        error.detail
    }

    /// A request whose URL is `parts` (`Ok` a literal, `Err` a reference)
    /// and whose one header carries reference 1.
    fn parts(url: &[Result<&str, u32>]) -> outbound_http::HttpRequest {
        let part = |part: &Result<&str, u32>| match part {
            Ok(literal) => outbound_http::TextPart::Literal((*literal).into()),
            Err(reference) => outbound_http::TextPart::Reference(*reference),
        };
        outbound_http::HttpRequest {
            method: "POST".into(),
            url: url.iter().map(part).collect(),
            headers: vec![outbound_http::HttpHeader {
                name: "authorization".into(),
                value: vec![
                    outbound_http::TextPart::Literal("Bearer ".into()),
                    outbound_http::TextPart::Reference(1),
                ],
            }],
            body: vec![
                outbound_http::BodyPart::Bytes(b"{\"k\":\"".to_vec()),
                outbound_http::BodyPart::Reference(1),
                outbound_http::BodyPart::Bytes(b"\"}".to_vec()),
            ],
        }
    }

    /// Whether the request `url` (with references 1 = a secret and 2 =
    /// `base`) is admitted under `declared`, and where it would go.
    fn admission(declared: &[&str], base: &str, url: &[Result<&str, u32>]) -> Option<String> {
        let references = [(1, "sk-secret".to_owned()), (2, base.to_owned())];
        let substituted = substitute(parts(url), &references).expect("references resolve");
        let supplied = substituted.supplied_origin.clone();
        let request = checked_http_request(substituted).ok()?;
        let allow = crate::HttpAllowlist::new(declared).expect("valid declarations");
        admitted(&request.url, &allow, supplied.as_ref()).then_some(request.url)
    }

    #[test]
    fn references_are_substituted_inside_the_request_only() {
        let references = [
            (1, "sk-secret".to_owned()),
            (2, "https://api.example/v1".to_owned()),
        ];
        let substituted =
            substitute(parts(&[Err(2), Ok("/responses")]), &references).expect("resolves");
        assert_eq!(substituted.url, "https://api.example/v1/responses");
        assert_eq!(
            substituted.headers,
            [("authorization".to_owned(), "Bearer sk-secret".to_owned())]
        );
        assert_eq!(substituted.body, br#"{"k":"sk-secret"}"#);
        let unknown = substitute(parts(&[Err(3)]), &references).expect_err("an unknown reference");
        assert_eq!(unknown.code, ErrorCode::InvalidInput);
        let too_many: Vec<Result<&str, u32>> = vec![Ok("x"); MAX_EXTENSION_HTTP_PARTS + 1];
        assert!(substitute(parts(&too_many), &references).is_err());
    }

    #[test]
    fn requests_reach_only_declared_or_run_supplied_origins() {
        let declared = ["https://api.example/v1"];
        // Declared: a literal URL under a declared prefix.
        assert_eq!(
            admission(&declared, "", &[Ok("https://api.example/v1/responses")]).as_deref(),
            Some("https://api.example/v1/responses")
        );
        // Not declared, not supplied: refused, so the secret never leaves.
        assert_eq!(
            admission(&declared, "", &[Ok("https://evil.example/x")]),
            None
        );
        assert_eq!(
            admission(&declared, "", &[Ok("https://api.example/v2/x")]),
            None
        );
        // Run-supplied: the whole origin comes from a reference's value.
        for (base, rest, expected) in [
            (
                "http://127.0.0.1:11434/v1",
                "/responses",
                "http://127.0.0.1:11434/v1/responses",
            ),
            (
                "http://127.0.0.1:11434",
                "/v1/responses",
                "http://127.0.0.1:11434/v1/responses",
            ),
            ("https://local.example", "", "https://local.example/"),
            (
                "https://local.example",
                "?q=1",
                "https://local.example/?q=1",
            ),
            (
                "https://bücher.example/v1",
                "/x",
                "https://xn--bcher-kva.example/v1/x",
            ),
        ] {
            assert_eq!(
                admission(&[], base, &[Err(2), Ok(rest)]).as_deref(),
                Some(expected),
                "{base} + {rest}"
            );
        }
        // Not run-supplied: a literal extends or precedes the authority, the
        // value carries userinfo, or the value is no URL at all.
        for (base, url) in [
            ("http://127.0.0.1", vec![Err(2), Ok(":8080/x")]),
            ("https://local.example", vec![Err(2), Ok(".evil.example/x")]),
            ("https://local.example", vec![Err(2), Ok("@evil.example/x")]),
            ("local.example", vec![Ok("https://"), Err(2), Ok("/x")]),
            ("sk", vec![Ok("https://"), Err(2), Ok(".evil.example/x")]),
            ("https://user:pw@local.example", vec![Err(2), Ok("/x")]),
            ("not a url", vec![Err(2), Ok("/x")]),
            ("https://local.example", vec![Ok(""), Err(2), Ok("/x")]),
        ] {
            assert_eq!(admission(&[], base, &url), None, "{base} {url:?}");
        }
        // A secret reference as the authority is no URL: refused.
        assert_eq!(admission(&[], "", &[Err(1), Ok("/x")]), None);
    }

    #[test]
    fn extension_http_requests_are_canonicalized_within_their_bounds() {
        let accepted = checked(http_request(
            "POST",
            "HTTP://Api.Example:80/v1/./x/../responses",
            &[
                ("Authorization", "Bearer k"),
                ("content-type", "application/json"),
            ],
            b"{}",
        ))
        .expect("a request within the bounds is accepted");
        assert_eq!(
            accepted,
            ExtensionHttpRequest {
                method: ExtensionHttpMethod::Post,
                url: "http://api.example/v1/responses".into(),
                headers: vec![
                    ("Authorization".into(), "Bearer k".into()),
                    ("content-type".into(), "application/json".into()),
                ],
                body: b"{}".to_vec(),
            }
        );
        let get = checked(http_request("GET", "https://a.example/", &[], b""))
            .expect("an empty GET is accepted");
        assert_eq!(get.method, ExtensionHttpMethod::Get);
        let long_url = format!(
            "https://a.example/{}",
            "x".repeat(MAX_EXTENSION_HTTP_URL_BYTES)
        );
        let long_value = "v".repeat(MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES + 1);
        let many: Vec<(&str, &str)> = vec![("x", "v"); MAX_EXTENSION_HTTP_HEADERS + 1];
        let body = vec![0; MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES + 1];
        for request in [
            http_request("PUT", "https://a.example/", &[], b""),
            http_request("post", "https://a.example/", &[], b""),
            http_request("POST", &long_url, &[], b""),
            http_request("POST", "/relative", &[], b""),
            http_request("POST", "ftp://a.example/", &[], b""),
            http_request("POST", "https://user:pw@a.example/", &[], b""),
            http_request("POST", "https://user@a.example/", &[], b""),
            http_request("POST", "https://:pw@a.example/", &[], b""),
            http_request("POST", "https://a.example/", &[("Host", "b.example")], b""),
            http_request(
                "POST",
                "https://a.example/",
                &[("Content-Length", "1")],
                b"",
            ),
            http_request("POST", "https://a.example/", &[("bad name", "v")], b""),
            http_request("POST", "https://a.example/", &[("", "v")], b""),
            http_request(
                "POST",
                "https://a.example/",
                &[("x", "a\r\nInjected: 1")],
                b"",
            ),
            http_request("POST", "https://a.example/", &[("x", &long_value)], b""),
            http_request("POST", "https://a.example/", &many, b""),
            http_request("POST", "https://a.example/", &[], &body),
            http_request("GET", "https://a.example/", &[], b"x"),
        ] {
            let url = format!("{:?}", request.url);
            assert!(rejected(request).is_some(), "{url} is refused");
        }
    }

    fn variable(name: &str, value: &str, packages: &[&str]) -> EnvVariable {
        EnvVariable::new(name, value, packages.iter().map(|uri| (*uri).to_owned()))
    }

    #[test]
    fn environments_are_sorted_and_bounded() {
        let environment = ExtensionEnvironment::new([
            variable("B", "2", &["urn:y", "urn:x"]),
            variable("A", "1", &["urn:x"]),
        ])
        .expect("valid variables");
        let names: Vec<&str> = environment
            .variables()
            .iter()
            .map(EnvVariable::name)
            .collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(
            environment.variables()[1].packages(),
            ["urn:x".to_owned(), "urn:y".to_owned()],
            "packages are sorted"
        );
        for variables in [
            vec![variable("", "v", &["urn:x"])],
            vec![variable("A=B", "v", &["urn:x"])],
            vec![variable("A", "v\0", &["urn:x"])],
            vec![
                variable("A", "1", &["urn:x"]),
                variable("A", "2", &["urn:y"]),
            ],
            vec![variable(
                "A",
                &"x".repeat(MAX_ENVIRONMENT_VALUE_BYTES + 1),
                &["urn:x"],
            )],
            vec![variable("A", "v", &[])],
            vec![variable("A", "v", &["urn:x", "urn:x"])],
            vec![variable("A", "v", &["not a uri"])],
        ] {
            assert!(matches!(
                ExtensionEnvironment::new(variables),
                Err(HostError::InvalidLimit(InvalidLimit::Environment))
            ));
        }
        assert!(ExtensionEnvironment::none().variables().is_empty());
    }

    #[test]
    fn a_value_is_held_only_for_the_packages_it_is_granted_to() {
        let environment = ExtensionEnvironment::new([
            variable("KEY", "k", &["urn:a", "urn:c"]),
            variable("OTHER", "o", &["urn:b"]),
        ])
        .expect("valid variables");
        assert_eq!(environment.held("urn:a", "KEY"), Some("k"));
        assert_eq!(environment.held("urn:c", "KEY"), Some("k"));
        assert_eq!(environment.held("urn:b", "KEY"), None, "out of scope");
        assert_eq!(environment.held("urn:a", "OTHER"), None, "out of scope");
        assert_eq!(environment.held("urn:a", "MISSING"), None, "unheld");
        assert_eq!(environment.held("urn:A", "KEY"), None, "exact URI only");
    }

    /// The host sets no bound on how many variables an environment passes
    /// (a native resource bound would refuse an inherited environment).
    #[test]
    fn an_environment_passes_any_number_of_variables() {
        let environment = ExtensionEnvironment::new(
            (0..1_000).map(|index| variable(&format!("V{index:04}"), "", &["urn:x"])),
        )
        .expect("an environment of any size is valid");
        assert_eq!(environment.variables().len(), 1_000);
    }
}
