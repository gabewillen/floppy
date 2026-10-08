//! The GitHub resolver component (`stateforward:scxml/resolver`): resolves
//! repository packages, the extension namespaces, invoked-service types, and
//! data models whose URI is a package's folder in a GitHub repository
//! ([`scxml_types::RepositoryUri`]), to a release of that repository, and
//! fetches the release's files. `scx` embeds it.
//!
//! # What a URI means here
//!
//! A package `https://github.com/<owner>/<repo>/<folder>/<name>` (`folder`
//! is `namespaces`, `services`, or `datamodels`) lives in
//! `<folder>/<name>/package/` of `<owner>/<repo>`. A tag (the `@<tag>` a document pinned) names the
//! release of the same tag; without one the package names the repository's
//! latest GitHub release. So:
//!
//! * `resolve` asks the GitHub REST API for the latest release
//!   (`/repos/<owner>/<repo>/releases/latest`, its `tag_name`) when no tag is
//!   given, and resolves the tag to its commit
//!   (`/repos/<owner>/<repo>/git/ref/tags/<tag>`, dereferencing at most
//!   four annotated tags);
//! * `fetch` reads the manifest (`<name>.extension.json`,
//!   `<name>.service.json`, or `<name>.data-model.json`) and the schema from raw repository files at the
//!   release's commit, never at its tag, and the component and `SHA256SUMS`
//!   from the release's assets.
//!
//! The base URLs are the host's configuration (`github-api-base`,
//! `github-raw-base`, `github-download-base`): GitHub's own, or a mirror's.
//!
//! # Bounds
//!
//! Every request goes through `wasi:http/client`, which the host admits only
//! to the origins it grants (a refused request is reported as leaving the
//! GitHub or configured origins). A redirect is followed at most
//! five times; a body is refused when its declared length, or
//! the bytes read, exceed the call's limit (an API answer's is
//! 64 KiB). No credential is sent: the component holds none, and
//! the host adds what it holds. Time bounds are the host's.
//!
//! # Trust
//!
//! None is decided here. The host verifies every byte this component returns
//! before it pins or caches it.
//!
//! # Telemetry
//!
//! None of its own: the host's `scxml.cli.resolve-*` span covers each call
//! with its closed outcome (and no URI, URL, tag, commit, or digest).

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "the portable logic runs in the wasm32 guest; natively only its tests use it"
    )
)]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use scxml_types::RepositoryUri;

/// Maximum redirects one request follows.
const MAX_REDIRECTS: usize = 5;
/// Maximum bytes of one GitHub API answer.
const MAX_API_BYTES: u64 = 64 * 1024;
/// Maximum annotated-tag dereferences while resolving a tag.
const MAX_TAG_DEREFERENCES: usize = 4;
/// The largest `limit` a `fetch` takes: 2 GiB − 1, GitHub's release-asset
/// bound and the host's authentication bound.
const MAX_LIMIT: u64 = (2 << 30) - 1;
/// The release asset listing every asset's SHA-256.
const SUMS_ASSET: &str = "SHA256SUMS";
/// The `Accept` of a GitHub API request.
const API_ACCEPT: &str = "application/vnd.github+json";

/// The host's configuration: the three base URLs.
#[derive(Debug)]
struct Configuration {
    api: url::Url,
    raw: url::Url,
    download: url::Url,
}

impl Configuration {
    /// Parses the host's name and value pairs: each of `github-api-base`,
    /// `github-raw-base`, and `github-download-base` exactly once, and
    /// nothing else.
    fn parse(pairs: &[(String, String)]) -> Result<Self, String> {
        let (mut api, mut raw, mut download) = (None, None, None);
        for (name, value) in pairs {
            let slot = match name.as_str() {
                "github-api-base" => &mut api,
                "github-raw-base" => &mut raw,
                "github-download-base" => &mut download,
                _ => {
                    return Err(format!(
                        "the resolver configuration names the unknown `{}`",
                        name.escape_debug()
                    ));
                }
            };
            if slot.is_some() {
                return Err(format!("the resolver configuration repeats `{name}`"));
            }
            *slot = Some(parse_base(value).ok_or_else(|| {
                format!("the resolver configuration's `{name}` is not an absolute http(s) URL without userinfo, query, or fragment")
            })?);
        }
        let missing = |name: &str| format!("the resolver configuration has no `{name}`");
        Ok(Self {
            api: api.ok_or_else(|| missing("github-api-base"))?,
            raw: raw.ok_or_else(|| missing("github-raw-base"))?,
            download: download.ok_or_else(|| missing("github-download-base"))?,
        })
    }

    fn latest_release(&self, package: &RepositoryUri<'_>) -> Result<url::Url, String> {
        join(
            &self.api,
            &format!("repos/{}/{}/releases/latest", package.owner, package.repo),
        )
    }

    fn tag_ref(&self, package: &RepositoryUri<'_>, tag: &str) -> Result<url::Url, String> {
        join(
            &self.api,
            &format!(
                "repos/{}/{}/git/ref/tags/{tag}",
                package.owner, package.repo
            ),
        )
    }

    fn tag_object(&self, package: &RepositoryUri<'_>, sha: &str) -> Result<url::Url, String> {
        join(
            &self.api,
            &format!("repos/{}/{}/git/tags/{sha}", package.owner, package.repo),
        )
    }

    /// The URL of one artifact of `package` at the release `(tag, commit)`.
    fn artifact(
        &self,
        package: &RepositoryUri<'_>,
        tag: &str,
        commit: &str,
        artifact: &Artifact<'_>,
    ) -> Result<url::Url, String> {
        let raw = |file: &str| {
            join(
                &self.raw,
                &format!(
                    "{}/{}/{commit}/{}/{}/package/{file}",
                    package.owner, package.repo, package.folder, package.name
                ),
            )
        };
        let asset = |asset: &str| {
            join(
                &self.download,
                &format!(
                    "{}/{}/releases/download/{tag}/{asset}",
                    package.owner, package.repo
                ),
            )
        };
        match artifact {
            Artifact::Manifest => raw(&manifest_name(package)),
            Artifact::Schema(path) if valid_relative(path) => raw(path),
            Artifact::Component(name) if valid_segment(name) => asset(name),
            Artifact::Sha256Sums => asset(SUMS_ASSET),
            Artifact::Schema(_) | Artifact::Component(_) => {
                Err("the artifact is not a plain relative path or release asset name".into())
            }
        }
    }
}

/// One file of a release (the WIT `artifact`).
#[derive(Debug)]
enum Artifact<'a> {
    Manifest,
    Schema(&'a str),
    Component(&'a str),
    Sha256Sums,
}

/// A base URL: absolute `http` or `https` with a host, and no userinfo,
/// query, or fragment. Which origins it may reach is the host's grant.
fn parse_base(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn join(base: &url::Url, path: &str) -> Result<url::Url, String> {
    let text = format!("{}/{path}", base.as_str().trim_end_matches('/'));
    url::Url::parse(&text).map_err(|_| "a repository URL does not parse".to_owned())
}

/// The package manifest's file name.
fn manifest_name(package: &RepositoryUri<'_>) -> String {
    match package.folder {
        "services" => format!("{}.service.json", package.name),
        "datamodels" => format!("{}.data-model.json", package.name),
        _ => format!("{}.extension.json", package.name),
    }
}

/// `[A-Za-z0-9._-]`, not `.` or `..`, at most 100 bytes.
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 100
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A relative path of [`valid_segment`] segments.
fn valid_relative(path: &str) -> bool {
    !path.is_empty() && path.len() <= 1_024 && path.split('/').all(valid_segment)
}

/// Whether `tag` pins a release of the package `uri`: the grammar of
/// [`scxml_types::strip_tag`], its one definition.
fn valid_tag(uri: &str, tag: &str) -> bool {
    matches!(
        scxml_types::strip_tag(&format!("{uri}@{tag}")),
        Some((_, Some(split))) if split == tag
    )
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn json(bytes: &[u8]) -> Result<serde_json::Value, String> {
    serde_json::from_slice(bytes).map_err(|_| "the GitHub API answered malformed JSON".into())
}

/// The `tag_name` of a latest-release answer.
fn latest_tag(bytes: &[u8], uri: &str, package: &RepositoryUri<'_>) -> Result<String, String> {
    json(bytes)?
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .filter(|tag| valid_tag(uri, tag))
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "the GitHub API answered no valid `tag_name` for the latest release of `{}/{}`",
                package.owner, package.repo
            )
        })
}

/// What a tag reference or annotated tag points at.
#[derive(Debug, Eq, PartialEq)]
enum Object {
    Commit(String),
    Tag(String),
}

/// The object a `git/ref/tags/<tag>` answer names; it must name `tag`
/// exactly.
fn tag_ref(bytes: &[u8], tag: &str) -> Result<Object, String> {
    let value = json(bytes)?;
    if value.get("ref").and_then(serde_json::Value::as_str)
        != Some(format!("refs/tags/{tag}").as_str())
    {
        return Err(format!("the GitHub API does not name tag `{tag}` exactly"));
    }
    object(value.get("object"))
}

/// The object a `git/tags/<sha>` answer names.
fn tag_object(bytes: &[u8]) -> Result<Object, String> {
    object(json(bytes)?.get("object"))
}

fn object(object: Option<&serde_json::Value>) -> Result<Object, String> {
    let sha = object
        .and_then(|object| object.get("sha"))
        .and_then(serde_json::Value::as_str)
        .filter(|sha| is_commit(sha))
        .ok_or("the GitHub API answered a tag without a commit")?
        .to_owned();
    match object
        .and_then(|object| object.get("type"))
        .and_then(serde_json::Value::as_str)
    {
        Some("commit") => Ok(Object::Commit(sha)),
        Some("tag") => Ok(Object::Tag(sha)),
        _ => Err("the GitHub API answered a tag of an unknown kind".into()),
    }
}

/// Whether a declared `Content-Length` is over `limit`.
fn declared_over(content_length: Option<&[u8]>, limit: u64) -> bool {
    content_length
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|length| length > limit)
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "resolver",
        path: "../../crates/scxml-wit/wit/scxml",
        // The world's `wasi:http` interfaces too.
        generate_all,
    });

    use exports::stateforward::scxml::resolver_api::{Artifact, Guest, Release};
    use stateforward::scxml::types::{ComponentError, ErrorCode, ErrorVersion};
    use wasi::http::client;
    use wasi::http::types::{
        ErrorCode as HttpErrorCode, Fields, Method, Request, Response, Scheme,
    };
    use wit_bindgen::StreamResult;

    use super::{
        API_ACCEPT, Configuration, MAX_API_BYTES, MAX_LIMIT, MAX_REDIRECTS, MAX_TAG_DEREFERENCES,
        Object, RepositoryUri,
    };

    fn error(code: ErrorCode, detail: String) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: Some(detail),
        }
    }

    fn invalid(detail: String) -> ComponentError {
        error(ErrorCode::InvalidInput, detail)
    }

    fn unavailable(detail: String) -> ComponentError {
        error(ErrorCode::Unavailable, detail)
    }

    struct Resolver;

    impl Guest for Resolver {
        async fn resolve(
            configuration: Vec<(String, String)>,
            uri: String,
            tag: Option<String>,
        ) -> Result<Option<Release>, ComponentError> {
            let configuration = Configuration::parse(&configuration).map_err(invalid)?;
            let Some(package) = RepositoryUri::parse(&uri) else {
                return Ok(None);
            };
            let tag = match tag {
                Some(tag) if super::valid_tag(&uri, &tag) => tag,
                Some(tag) => {
                    return Err(invalid(format!(
                        "`{}` is not a release tag",
                        tag.escape_debug()
                    )));
                }
                None => {
                    let answer = get(
                        configuration.latest_release(&package).map_err(invalid)?,
                        MAX_API_BYTES,
                        API_ACCEPT,
                    )
                    .await?;
                    super::latest_tag(&answer, &uri, &package).map_err(unavailable)?
                }
            };
            let answer = get(
                configuration.tag_ref(&package, &tag).map_err(invalid)?,
                MAX_API_BYTES,
                API_ACCEPT,
            )
            .await?;
            let mut object = super::tag_ref(&answer, &tag).map_err(unavailable)?;
            for _ in 0..MAX_TAG_DEREFERENCES {
                match object {
                    Object::Commit(commit) => return Ok(Some(Release { tag, commit })),
                    Object::Tag(sha) => {
                        let answer = get(
                            configuration.tag_object(&package, &sha).map_err(invalid)?,
                            MAX_API_BYTES,
                            API_ACCEPT,
                        )
                        .await?;
                        object = super::tag_object(&answer).map_err(unavailable)?;
                    }
                }
            }
            Err(unavailable(format!(
                "tag `{tag}` is nested deeper than {MAX_TAG_DEREFERENCES} annotated tags"
            )))
        }

        async fn fetch(
            configuration: Vec<(String, String)>,
            uri: String,
            release: Release,
            artifact: Artifact,
            limit: u64,
        ) -> Result<Option<Vec<u8>>, ComponentError> {
            let configuration = Configuration::parse(&configuration).map_err(invalid)?;
            let Some(package) = RepositoryUri::parse(&uri) else {
                return Ok(None);
            };
            if !super::valid_tag(&uri, &release.tag) || !super::is_commit(&release.commit) {
                return Err(invalid("the release is not a tag and a commit".into()));
            }
            if limit > MAX_LIMIT {
                return Err(invalid(format!("a limit is at most {MAX_LIMIT} bytes")));
            }
            let artifact = match &artifact {
                Artifact::Manifest => super::Artifact::Manifest,
                Artifact::Schema(path) => super::Artifact::Schema(path),
                Artifact::Component(name) => super::Artifact::Component(name),
                Artifact::Sha256sums => super::Artifact::Sha256Sums,
            };
            let url = configuration
                .artifact(&package, &release.tag, &release.commit, &artifact)
                .map_err(invalid)?;
            get(url, limit, "*/*").await.map(Some)
        }
    }

    /// GETs `url` within `limit` bytes, following at most [`MAX_REDIRECTS`]
    /// redirects, each admitted (or refused) by the host.
    async fn get(url: url::Url, limit: u64, accept: &str) -> Result<Vec<u8>, ComponentError> {
        let shown = url.to_string();
        let mut current = url;
        for _ in 0..=MAX_REDIRECTS {
            let request = request(&current, accept).map_err(|()| {
                invalid(format!(
                    "`{shown}` leads to `{current}`, which no request can name"
                ))
            })?;
            let response = match client::send(request).await {
                Ok(response) => response,
                Err(HttpErrorCode::HttpRequestDenied) => {
                    return Err(unavailable(format!(
                        "`{shown}` leads to `{current}`, which is not a GitHub or configured repository origin"
                    )));
                }
                Err(failure) => {
                    return Err(unavailable(format!("cannot fetch `{shown}`: {failure:?}")));
                }
            };
            let status = response.get_status_code();
            let headers = response.get_headers();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = headers
                    .get("location")
                    .into_iter()
                    .next()
                    .and_then(|value| String::from_utf8(value).ok())
                    .ok_or_else(|| {
                        unavailable(format!("`{shown}` redirects without a location"))
                    })?;
                current = current.join(&location).map_err(|_| {
                    unavailable(format!("`{shown}` redirects to an invalid location"))
                })?;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(unavailable(format!("`{shown}` answered HTTP {status}")));
            }
            let too_large = || {
                error(
                    ErrorCode::PayloadTooLarge,
                    format!("`{shown}` is larger than its {limit}-byte bound"),
                )
            };
            // A declared length over the bound fails before any body byte is
            // read; an undeclared or understated one is still cut off below.
            if super::declared_over(
                headers.get("content-length").first().map(Vec::as_slice),
                limit,
            ) {
                return Err(too_large());
            }
            drop(headers);
            let (_done, done) = wit_future::new(|| Ok(()));
            let (mut body, _trailers) = Response::consume_body(response, done);
            let mut bytes = Vec::new();
            loop {
                let (result, chunk) = body.read(Vec::with_capacity(64 * 1024)).await;
                match result {
                    StreamResult::Complete(_) => {
                        bytes.extend_from_slice(&chunk);
                        if u64::try_from(bytes.len()).map_or(true, |length| length > limit) {
                            return Err(too_large());
                        }
                    }
                    StreamResult::Dropped => return Ok(bytes),
                    StreamResult::Cancelled => {
                        return Err(unavailable(format!("cannot read `{shown}`")));
                    }
                }
            }
        }
        Err(unavailable(format!(
            "`{shown}` redirects more than {MAX_REDIRECTS} times"
        )))
    }

    /// A GET of `url` with `Accept: accept`.
    fn request(url: &url::Url, accept: &str) -> Result<Request, ()> {
        let headers = Fields::from_list(&[("accept".to_owned(), accept.as_bytes().to_vec())])
            .map_err(|_| ())?;
        let (_trailers, trailers) = wit_future::new(|| Ok(None));
        let (request, _sent) = Request::new(headers, None, trailers, None);
        let scheme = match url.scheme() {
            "https" => Scheme::Https,
            "http" => Scheme::Http,
            _ => return Err(()),
        };
        let host = url.host_str().ok_or(())?;
        let authority = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        request.set_method(&Method::Get)?;
        request.set_scheme(Some(&scheme))?;
        request.set_authority(Some(&authority))?;
        request.set_path_with_query(Some(&path))?;
        Ok(request)
    }

    export!(Resolver);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration() -> Configuration {
        Configuration::parse(&[
            ("github-api-base".into(), "http://127.0.0.1:9/api".into()),
            (
                "github-raw-base".into(),
                "https://raw.githubusercontent.com".into(),
            ),
            ("github-download-base".into(), "https://github.com/".into()),
        ])
        .expect("a complete configuration")
    }

    #[test]
    fn the_configuration_names_each_base_once_and_nothing_else() {
        let _ = configuration();
        for refused in [
            vec![(
                "github-api-base".to_owned(),
                "https://api.github.com".to_owned(),
            )],
            vec![("other".to_owned(), "https://x".to_owned())],
            vec![
                ("github-api-base".to_owned(), "https://a".to_owned()),
                ("github-api-base".to_owned(), "https://b".to_owned()),
            ],
            vec![
                ("github-api-base".to_owned(), "https://user@a".to_owned()),
                ("github-raw-base".to_owned(), "https://b".to_owned()),
                ("github-download-base".to_owned(), "https://c".to_owned()),
            ],
        ] {
            assert!(Configuration::parse(&refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn urls_follow_the_github_layout() {
        let configuration = configuration();
        let uri = "https://github.com/o/r/namespaces/x";
        let package = RepositoryUri::parse(uri).expect("a repository package URI");
        assert_eq!(
            configuration.latest_release(&package).map(String::from),
            Ok("http://127.0.0.1:9/api/repos/o/r/releases/latest".to_owned())
        );
        assert_eq!(
            configuration.tag_ref(&package, "v1").map(String::from),
            Ok("http://127.0.0.1:9/api/repos/o/r/git/ref/tags/v1".to_owned())
        );
        let commit = "a".repeat(40);
        assert_eq!(
            configuration
                .artifact(&package, "v1", &commit, &Artifact::Manifest)
                .map(String::from),
            Ok(format!(
                "https://raw.githubusercontent.com/o/r/{commit}/namespaces/x/package/x.extension.json"
            ))
        );
        assert_eq!(
            configuration
                .artifact(&package, "v1", &commit, &Artifact::Component("x.wasm"))
                .map(String::from),
            Ok("https://github.com/o/r/releases/download/v1/x.wasm".to_owned())
        );
        assert_eq!(
            configuration
                .artifact(&package, "v1", &commit, &Artifact::Sha256Sums)
                .map(String::from),
            Ok("https://github.com/o/r/releases/download/v1/SHA256SUMS".to_owned())
        );
        for refused in [
            Artifact::Schema("../x.xsd"),
            Artifact::Schema("/x.xsd"),
            Artifact::Component("a/b"),
            Artifact::Component(".."),
        ] {
            assert!(
                configuration
                    .artifact(&package, "v1", &commit, &refused)
                    .is_err(),
                "{refused:?}"
            );
        }
        let service = RepositoryUri::parse("https://github.com/o/r/services/s")
            .expect("a repository service URI");
        assert_eq!(manifest_name(&service), "s.service.json");
        let data_model = RepositoryUri::parse("https://github.com/o/r/datamodels/d")
            .expect("a repository data-model URI");
        assert_eq!(manifest_name(&data_model), "d.data-model.json");
    }

    #[test]
    fn tags_resolve_to_commits() {
        let commit = "c".repeat(40);
        let reference =
            format!(r#"{{"ref":"refs/tags/v1","object":{{"type":"tag","sha":"{commit}"}}}}"#);
        assert_eq!(
            tag_ref(reference.as_bytes(), "v1"),
            Ok(Object::Tag(commit.clone()))
        );
        assert!(
            tag_ref(reference.as_bytes(), "v2")
                .is_err_and(|error| error.contains("does not name tag `v2` exactly"))
        );
        let annotated = format!(r#"{{"object":{{"type":"commit","sha":"{commit}"}}}}"#);
        assert_eq!(tag_object(annotated.as_bytes()), Ok(Object::Commit(commit)));
        assert!(tag_object(br#"{"object":{"type":"blob","sha":"x"}}"#).is_err());
        let uri = "https://github.com/o/r/namespaces/x";
        let package = RepositoryUri::parse(uri).expect("a repository package URI");
        assert_eq!(
            latest_tag(br#"{"tag_name":"v2.0.0"}"#, uri, &package),
            Ok("v2.0.0".to_owned())
        );
        assert!(latest_tag(br#"{"tag_name":"-v2"}"#, uri, &package).is_err());
        assert!(latest_tag(b"not json", uri, &package).is_err());
    }

    #[test]
    fn a_declared_length_over_the_limit_is_refused() {
        assert!(declared_over(Some(b"11"), 10));
        assert!(!declared_over(Some(b"10"), 10));
        assert!(!declared_over(None, 10));
    }
}
