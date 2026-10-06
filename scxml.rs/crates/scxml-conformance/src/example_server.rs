//! The conformance harness's outbound network: a deterministic, in-process
//! stand-in for the server the example extension's `<example:fetch>`
//! (`components/scxml-extension-example`) reaches, and the fake values the
//! run holds for `env` slots.
//!
//! A namespace suite that binds the example package (`namespaces/env/irp`,
//! through `<example:fetch>`, whose package declares the `url` and `token`
//! slots) runs every request its extension component makes through this
//! transport, which the session host reaches only after it admitted the
//! request (the package declares no origin, so only an origin a held value
//! supplies entirely). Nothing touches the network: the stand-in answers
//! from the request alone, so every case is deterministic, and every key is
//! a fake test value.
//!
//! # Contract
//!
//! * Served URLs: [`BASE_URL`] (the value of [`BASE_URL_VARIABLE`]) followed
//!   by one of the paths below; any other URL answers `404`, and any method
//!   but `GET` answers `405`.
//! * Paths (every answer has an empty body; the element raises
//!   `example.fetched` on a `2xx` status and `error.execution` otherwise):
//!
//! | path | status |
//! |---|---|
//! | `/ok` | `200` |
//! | `/authorization/key` | `200` when `authorization` is `Bearer ` and the value of [`KEY`], else `401` |
//! | `/authorization/other-key` | `200` when `authorization` is `Bearer ` and the value of [`OTHER_KEY`], else `401` |
//! | `/authorization/none` | `200` when the request has no `authorization` header, else `401` |
//! | `/requests/<n>` | `200` when exactly `n` earlier requests of this case reached the transport, else `409` |
//!
//! * [`ExampleServer::reset`] clears the per-case request count; the
//!   executor calls it before every case, so `/requests/<n>` counts only the
//!   case's own earlier requests that reached the transport.
//! * Answers never carry a held value.

use std::sync::atomic::{AtomicU64, Ordering};

use scxml_host_wasmtime::{
    ExtensionHttp, ExtensionHttpFailure, ExtensionHttpMethod, ExtensionHttpRequest,
    ExtensionHttpResponse,
};

/// The variable whose fake value is the run's key.
pub const KEY: &str = "IRP_KEY";
/// A second variable holding another fake key (slot binding is observable).
pub const OTHER_KEY: &str = "IRP_OTHER_KEY";
/// The variable whose value is the served base URL.
pub const BASE_URL_VARIABLE: &str = "IRP_BASE_URL";
/// The variable whose value supplies no origin.
pub const NO_ORIGIN_BASE_URL_VARIABLE: &str = "IRP_NO_ORIGIN_BASE_URL";

/// The fake value of [`KEY`].
const KEY_VALUE: &str = "irp-test-key-0001";
/// The fake value of [`OTHER_KEY`].
const OTHER_KEY_VALUE: &str = "irp-test-key-0002";
/// The served base URL (the value of [`BASE_URL_VARIABLE`]).
pub const BASE_URL: &str = "https://irp.invalid/v1";
/// A base URL that supplies no origin (the value of
/// [`NO_ORIGIN_BASE_URL_VARIABLE`]).
const NO_ORIGIN_BASE_URL: &str = "irp/v1";

/// The values the run holds, by variable name: exactly these, never the
/// process environment.
#[must_use]
pub fn values() -> Vec<(String, String)> {
    [
        (KEY, KEY_VALUE),
        (OTHER_KEY, OTHER_KEY_VALUE),
        (BASE_URL_VARIABLE, BASE_URL),
        (NO_ORIGIN_BASE_URL_VARIABLE, NO_ORIGIN_BASE_URL),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect()
}

/// The stand-in transport (see the module documentation).
#[derive(Debug, Default)]
pub struct ExampleServer {
    /// Requests of the current case that reached the transport.
    requests: AtomicU64,
}

impl ExampleServer {
    /// A stand-in that has seen no request.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a new case: forgets the previous case's requests.
    pub fn reset(&self) {
        self.requests.store(0, Ordering::SeqCst);
    }
}

impl ExtensionHttp for ExampleServer {
    fn fetch(
        &self,
        request: &ExtensionHttpRequest,
    ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure> {
        let earlier = self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(answer(request, earlier))
    }
}

/// The answer to `request`, the case's `earlier`-th request that reached
/// the transport.
#[must_use]
pub fn answer(request: &ExtensionHttpRequest, earlier: u64) -> ExtensionHttpResponse {
    let status = |status: u16| ExtensionHttpResponse {
        status,
        body: Vec::new(),
    };
    let Some(path) = request
        .url
        .strip_prefix(BASE_URL)
        .filter(|path| path.starts_with('/'))
    else {
        return status(404);
    };
    if request.method != ExtensionHttpMethod::Get {
        return status(405);
    }
    let authorization = request
        .headers
        .iter()
        .find(|(field, _)| field.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str());
    let bearer =
        |key: &str| authorization.and_then(|value| value.strip_prefix("Bearer ")) == Some(key);
    let admitted = |admitted: bool| status(if admitted { 200 } else { 401 });
    match path {
        "/ok" => status(200),
        "/authorization/key" => admitted(bearer(KEY_VALUE)),
        "/authorization/other-key" => admitted(bearer(OTHER_KEY_VALUE)),
        "/authorization/none" => admitted(authorization.is_none()),
        _ => match path
            .strip_prefix("/requests/")
            .and_then(|count| count.parse::<u64>().ok())
        {
            Some(count) if count == earlier => status(200),
            Some(_) => status(409),
            None => status(404),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str, authorization: Option<&str>) -> ExtensionHttpRequest {
        ExtensionHttpRequest {
            method: ExtensionHttpMethod::Get,
            url: format!("{BASE_URL}{path}"),
            headers: authorization
                .map(|value| ("authorization".to_owned(), value.to_owned()))
                .into_iter()
                .collect(),
            body: Vec::new(),
        }
    }

    #[test]
    fn served_paths_answer_by_the_request() {
        let key = format!("Bearer {KEY_VALUE}");
        let other = format!("Bearer {OTHER_KEY_VALUE}");
        for (path, authorization, expected) in [
            ("/ok", None, 200),
            ("/authorization/key", Some(key.as_str()), 200),
            ("/authorization/key", Some(other.as_str()), 401),
            ("/authorization/key", None, 401),
            ("/authorization/other-key", Some(other.as_str()), 200),
            ("/authorization/other-key", Some(key.as_str()), 401),
            ("/authorization/none", None, 200),
            ("/authorization/none", Some(key.as_str()), 401),
            ("/requests/0", None, 200),
            ("/requests/1", None, 409),
            ("/elsewhere", None, 404),
        ] {
            assert_eq!(
                answer(&request(path, authorization), 0).status,
                expected,
                "{path} {authorization:?}"
            );
        }
        let mut post = request("/ok", None);
        post.method = ExtensionHttpMethod::Post;
        assert_eq!(answer(&post, 0).status, 405);
        let mut elsewhere = request("/ok", None);
        elsewhere.url = "https://irp.invalid/v1x/ok".into();
        assert_eq!(answer(&elsewhere, 0).status, 404);
    }

    #[test]
    fn the_request_count_is_per_case() {
        let server = ExampleServer::new();
        assert_eq!(
            server
                .fetch(&request("/requests/0", None))
                .map(|r| r.status),
            Ok(200)
        );
        assert_eq!(
            server
                .fetch(&request("/requests/1", None))
                .map(|r| r.status),
            Ok(200)
        );
        server.reset();
        assert_eq!(
            server
                .fetch(&request("/requests/0", None))
                .map(|r| r.status),
            Ok(200)
        );
    }

    #[test]
    fn the_held_values_are_fake_and_distinct() {
        let values = values();
        assert_eq!(values.len(), 4);
        assert!(values.iter().all(|(name, _)| name.starts_with("IRP_")));
        assert!(
            values
                .iter()
                .all(|(_, value)| !value.contains("key") || value.starts_with("irp-test-key-"))
        );
    }
}
