//! Repository namespaces and repository services: an extension namespace,
//! or an invoked-service type, whose URI is the path of its package in a
//! GitHub repository, at a release tag or at the repository's latest
//! release.
//!
//! # The URI
//!
//! `https://github.com/<owner>/<repo>/namespaces/<name>[@<tag>]` (a
//! repository namespace) or
//! `https://github.com/<owner>/<repo>/services/<name>[@<tag>]` (a repository
//! service), exactly: the literal `namespaces` or `services` folder, one name
//! segment, no trailing slash, and, optionally, one release tag after `@`.
//! With `@<tag>` the URI pins that release; without one (a tagless URI) it
//! names the repository's latest GitHub release, resolved once per root
//! session through the same bounded, verified path. The
//! built-in tagless vocabularies `scx` embeds (`env`, `event`, `scx`, under
//! `https://github.com/stateforward/scxml.rs/namespaces/`) are matched first
//! and are never repository namespaces, so they are never fetched. Every
//! repository namespace lives in its own `namespaces/<name>/` folder and
//! every repository service in its own `services/<name>/` folder; any other
//! path is an ordinary namespace or service type URI, and a `services/` URI
//! is never a repository namespace (nor a `namespaces/` URI a repository
//! service).
//!
//! # The tag is not part of the namespace
//!
//! A package's identity is its tagless URI. The `@<tag>` is split off once,
//! when a document is parsed ([`scxml_types::strip_tag`], for namespace
//! declarations and literal SCXML `<invoke type>`s only); from then on the
//! validator, the compiler, the Machine IR, the engine, the data models,
//! and the host see only the tagless URI, exactly, and the tag is
//! resolution input for this resolver alone ([`document_pins`]). So a
//! release's manifest names the tagless URI (`namespace-uri`, and its
//! schema's `targetNamespace`, or `service-uri`), and a manifest that names
//! a pinned URI is refused; a pinned and a tagless use of a release
//! validate against the same schema. A malformed pin (an `@` not followed
//! by exactly one tag) fails closed, and a built-in vocabulary takes no
//! pin. Any other URI with an `@` (not a repository package URI) is an
//! ordinary URI, byte for byte.
//!
//! # Which release a document binds
//!
//! One document binds one release of a package: different pins of one
//! package in one document fail closed. The run binds, for each package,
//! the root document's resolution (its pin, else the latest release), or,
//! when the root does not use the package, the first `<invoke src>`
//! document's that does; every tagless use anywhere in the tree shares that
//! resolution. An `<invoke src>` document that pins another release binds
//! that release for its own sessions (and their inline `<content>`
//! sessions), alongside the run's ([`documents`](crate::documents)). A
//! repository service is resolved only for a literal `<invoke type>` no
//! `--service-package` binds.
//!
//! # Where the bytes come from
//!
//! The GitHub resolver component `scx` embeds
//! (`components/scxml-resolver-github`, `stateforward:scxml/resolver`,
//! authenticated by its SHA-256 in the embedded release) knows what a
//! repository URI and its tag mean and fetches through WASI 0.3
//! `wasi:http`; this module never builds a GitHub URL. Given the bases
//! (`--github-*-base`, or GitHub's own origins) as its configuration:
//!
//! * `resolve` reports the release a URI resolves to: the release its tag
//!   names, or the repository's latest release, asked once per repository
//!   per root session, and the commit of that release's tag;
//! * `fetch` returns the manifest and its schema (`schema-artifact`) at that
//!   commit, never at the tag, and the release's `SHA256SUMS` and component
//!   asset (`extension-artifact` or `service-artifact`), the component only
//!   when `snapshot run` or `run` binds it, before the session starts.
//!
//! This module keeps trust: the schema must match the digest the manifest
//! declares, the component the SHA-256 the release's `SHA256SUMS` lists for
//! it, and the manifest its exact URI and WIT identity.
//!
//! # Trust and the cache
//!
//! Every acquired byte is verified before it is stored, and stored exactly,
//! content-addressed, under `<cache>/sha256/<hex>`. The first resolution of
//! a URI pins it: `<cache>/index/<sha256 of the URI>.json` records the
//! commit (and, for a tagless URI, the release tag) and the manifest,
//! schema, and component digests, and every later use re-verifies the cached
//! bytes against them. A pinned URI never asks the network again (a missing
//! object is re-fetched at the pinned commit and must match the pinned
//! digest); a tagless URI asks for the latest release once per root session
//! and re-pins only when that release names another commit. Cached bytes that
//! do not match their digest fail closed. `--offline` never touches the
//! network: a URI not in the cache fails closed, and a tagless URI uses the
//! release the cache pinned.
//!
//! One CLI process runs one root session, so every resolution of the
//! process shares one [`Repository`] ([`RepositoryFlags::open`]): each URI is
//! resolved once, and the root and every invoked child share its verified
//! bytes.
//!
//! # Network bounds
//!
//! The resolver component's requests reach only the origins of the bases
//! and GitHub's release-asset hosts (the host refuses every other, so a
//! redirect cannot leave them); a base is HTTPS (plain HTTP only to a
//! loopback host given through an explicit `--github-*-base`, for mirrors
//! and tests). The component follows at most five redirects and bounds
//! every body by the limit this module gives each fetch
//! ([`MAX_MANIFEST_BYTES`], `MAX_COMPONENT_SCHEMA_BYTES`, [`MAX_SUMS_BYTES`],
//! [`MAX_COMPONENT_BYTES`]; an API answer 64 KiB); the host bounds connect
//! and total time, with no retry, cookies, proxies from the environment, or
//! credentials, except a GitHub token read from the variable
//! `--github-token-env NAME` names, which the host (never the component)
//! adds to requests for the API origin only.
//!
//! # Telemetry
//!
//! One `scxml.cli.resolve-extension-package` (or
//! `scxml.cli.resolve-service-package`) span per package resolution and one
//! `scxml.cli.resolve-extension-component` (or
//! `scxml.cli.resolve-service-component`) span per component resolution,
//! each with its closed outcome only: no URI, URL, tag, commit, digest, or
//! path is ever recorded. They cover the resolver component's calls, which
//! record nothing of their own.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(feature = "wasmtime")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(feature = "wasmtime")]
use std::time::Duration;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_validator_core::{MAX_COMPONENT_SCHEMA_BYTES, SchemaSource};
use sha2::{Digest as _, Sha256};

use crate::extensions::{Package, PackageOrigin};
use crate::services::{ServiceOrigin, ServicePackage};

/// Default GitHub REST API base.
const DEFAULT_API_BASE: &str = "https://api.github.com";
/// Default base of raw repository files.
const DEFAULT_RAW_BASE: &str = "https://raw.githubusercontent.com";
/// Default base of release downloads.
const DEFAULT_DOWNLOAD_BASE: &str = "https://github.com";
/// The hosts GitHub redirects release downloads to.
const RELEASE_ASSET_ORIGINS: [&str; 2] = [
    "https://objects.githubusercontent.com",
    "https://release-assets.githubusercontent.com",
];
/// The release asset listing every asset's SHA-256.
const SUMS_ASSET: &str = "SHA256SUMS";
/// The cache index entry format of a repository namespace.
const ENTRY_FORMAT: &str = "stateforward.scxml.extension-cache/v1";
/// Namespace cache index entry fields in their canonical order (`tag` only
/// for a tagless URI: the release it was pinned to).
const ENTRY_FIELDS: [&str; 7] = [
    "format",
    "namespace-uri",
    "tag",
    "commit",
    "manifest-sha256",
    "schema-sha256",
    "component-sha256",
];
/// The cache index entry format of a repository service. Its manifest's
/// digest pins its optional schema, whose digest the manifest declares.
const SERVICE_ENTRY_FORMAT: &str = "stateforward.scxml.service-cache/v1";
/// Service cache index entry fields in their canonical order.
const SERVICE_ENTRY_FIELDS: [&str; 6] = [
    "format",
    "service-uri",
    "tag",
    "commit",
    "manifest-sha256",
    "component-sha256",
];

/// Maximum bytes of a repository namespace URI.
pub(crate) const MAX_URI_BYTES: usize = 512;
/// The tagless vocabularies `scx` itself provides, matched before any
/// repository namespace and never fetched.
const BUILT_IN_NAMESPACES: [&str; 3] = [
    scxml_types::ENV_NAMESPACE_URI,
    scxml_types::EVENT_NAMESPACE_URI,
    crate::extensions::SCX_NAMESPACE_URI,
];
/// The one repository folder every repository namespace lives in.
const NAMESPACES_FOLDER: &str = "namespaces";
/// The one repository folder every repository service lives in.
const SERVICES_FOLDER: &str = "services";
/// Maximum bytes of a release asset name or a manifest artifact path
/// segment.
const MAX_SEGMENT_BYTES: usize = 100;
/// Maximum bytes of a package manifest (the `--extension-package` bound).
pub(crate) const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Maximum bytes of a release's `SHA256SUMS`.
pub(crate) const MAX_SUMS_BYTES: u64 = 64 * 1024;
/// Maximum bytes of an extension component: 2 GiB − 1, the host's
/// authentication bound. The component is a GitHub release asset, and GitHub
/// requires "each file included in a release must be under 2 GiB"
/// (<https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases>).
/// A download is checked against it by `Content-Length` before its body is
/// read, and its buffer grows with the bytes received.
pub(crate) const MAX_COMPONENT_BYTES: u64 = (2 << 30) - 1;
/// Maximum bytes of a cache index entry.
const MAX_ENTRY_BYTES: u64 = 16 * 1024;
/// Maximum bytes of one document fetched over the network (`<invoke src>`,
/// `<script src>`, `<data src>`, `srcexpr`): a network-ingress bound, on
/// every build.
pub(crate) const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum redirects one document download follows.
pub(crate) const MAX_REDIRECTS: usize = 5;
/// Time to establish one connection.
#[cfg(feature = "wasmtime")]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time for one whole request, body included.
#[cfg(feature = "wasmtime")]
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum bytes of a response head.
#[cfg(feature = "wasmtime")]
const MAX_RESPONSE_HEAD_BYTES: usize = 16 * 1024;

#[cfg(feature = "wasmtime")]
const _: () = assert!(
    scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES as u64 == MAX_COMPONENT_BYTES,
    "a repository component is bounded exactly like an authenticated component"
);

/// What a repository URI names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    /// An extension namespace, under `namespaces/`.
    Namespace,
    /// An invoked service, under `services/`.
    Service,
}

impl Kind {
    const fn folder(self) -> &'static str {
        match self {
            Self::Namespace => NAMESPACES_FOLDER,
            Self::Service => SERVICES_FOLDER,
        }
    }

    /// How messages name a URI of this kind.
    const fn label(self) -> &'static str {
        match self {
            Self::Namespace => "repository namespace",
            Self::Service => "repository service",
        }
    }
}

/// One parsed repository namespace or service URI, split by
/// [`scxml_types::strip_tag`] and [`scxml_types::RepositoryUri`] (the one
/// definition of the pattern, which the resolver component shares).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepositoryUri {
    kind: Kind,
    /// The package's identity: the URI without its tag.
    tagless: String,
    owner: String,
    repo: String,
    /// The pinned release tag; `None` for a tagless URI (the latest
    /// release).
    tag: Option<String>,
}

impl RepositoryUri {
    /// Parses `uri` when it is exactly a repository namespace URI; `None`
    /// otherwise (an ordinary namespace URI).
    pub(crate) fn parse(uri: &str) -> Option<Self> {
        Self::parse_kind(uri, Kind::Namespace)
    }

    /// Parses `uri` when it is exactly a repository service URI; `None`
    /// otherwise (an ordinary service type URI).
    pub(crate) fn parse_service(uri: &str) -> Option<Self> {
        Self::parse_kind(uri, Kind::Service)
    }

    fn parse_kind(uri: &str, kind: Kind) -> Option<Self> {
        if uri.len() > MAX_URI_BYTES {
            return None;
        }
        let (tagless, tag) = scxml_types::strip_tag(uri)?;
        if kind == Kind::Namespace && BUILT_IN_NAMESPACES.contains(&tagless) {
            return None;
        }
        let parts = scxml_types::RepositoryUri::parse(tagless)?;
        (parts.folder == kind.folder()).then(|| Self {
            kind,
            tagless: tagless.to_owned(),
            owner: parts.owner.to_owned(),
            repo: parts.repo.to_owned(),
            tag: tag.map(str::to_owned),
        })
    }
}

/// `[A-Za-z0-9._-]`, not `.` or `..`, within [`MAX_SEGMENT_BYTES`].
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_SEGMENT_BYTES
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Whether `tag` is a release tag a URI can pin: the grammar of
/// [`scxml_types::strip_tag`], its one definition.
fn valid_tag(tag: &str) -> bool {
    matches!(
        scxml_types::strip_tag(&format!(
            "https://github.com/o/r/{NAMESPACES_FOLDER}/n@{tag}"
        )),
        Some((_, Some(_)))
    )
}

/// The URI the resolver resolves for the package `tagless`: with its pin
/// (`<uri>@<tag>`), or tagless (the latest release).
pub(crate) fn pinned_uri(tagless: &str, tag: Option<&str>) -> String {
    match tag {
        Some(tag) => format!("{tagless}@{tag}"),
        None => tagless.to_owned(),
    }
}

/// A relative path of [`valid_segment`] segments (an artifact inside the
/// manifest's directory).
fn valid_relative(path: &str) -> bool {
    !path.is_empty() && path.len() <= 1_024 && path.split('/').all(valid_segment)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The repository flags every command that reads a document takes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RepositoryFlags {
    offline: bool,
    cache_dir: Option<PathBuf>,
    api_base: Option<String>,
    raw_base: Option<String>,
    download_base: Option<String>,
    token_env: Option<String>,
}

impl RepositoryFlags {
    /// The switch.
    pub(crate) const OFFLINE: &'static str = "--offline";

    /// Whether `flag` is one of the repository flags.
    pub(crate) fn is_repository_flag(flag: &str) -> bool {
        flag == Self::OFFLINE || Self::expects(flag).is_some()
    }

    /// What each value-taking flag expects, for usage messages.
    pub(crate) fn expects(flag: &str) -> Option<&'static str> {
        match flag {
            "--cache-dir" => Some("DIR"),
            "--github-api-base" | "--github-raw-base" | "--github-download-base" => Some("URL"),
            "--github-token-env" => Some("NAME"),
            _ => None,
        }
    }

    /// Records one repository flag; `Ok(false)` when `flag` is not one.
    ///
    /// # Errors
    ///
    /// A usage message for an empty or repeated value.
    pub(crate) fn accept(&mut self, flag: &str, value: &str) -> Result<bool, String> {
        let slot = match flag {
            Self::OFFLINE => {
                self.offline = true;
                return Ok(true);
            }
            "--cache-dir" => {
                if value.is_empty() || self.cache_dir.is_some() {
                    return Err("--cache-dir takes one DIR".into());
                }
                self.cache_dir = Some(PathBuf::from(value));
                return Ok(true);
            }
            "--github-api-base" => &mut self.api_base,
            "--github-raw-base" => &mut self.raw_base,
            "--github-download-base" => &mut self.download_base,
            "--github-token-env" => &mut self.token_env,
            _ => return Ok(false),
        };
        if value.is_empty() || slot.is_some() {
            return Err(format!("{flag} takes one value"));
        }
        *slot = Some(value.to_owned());
        Ok(true)
    }

    /// The repository of this process's root session: opened (the cache and
    /// the bases checked, nothing touching the network) on first use and
    /// shared by every later call with the same flags, so each URI is
    /// resolved once per root session and every resolution shares its
    /// verified bytes.
    ///
    /// # Errors
    ///
    /// A message for a base that is not an admissible URL, an unset token
    /// variable, or no cache directory (neither `--cache-dir`,
    /// `XDG_CACHE_HOME`, nor `HOME`).
    pub(crate) fn open(&self) -> Result<Arc<Repository>, String> {
        static OPENED: Mutex<Vec<(RepositoryFlags, Arc<Repository>)>> = Mutex::new(Vec::new());
        let mut opened = OPENED
            .lock()
            .map_err(|_| "the repository of this session is unavailable".to_owned())?;
        if let Some((_, repository)) = opened.iter().find(|(flags, _)| flags == self) {
            return Ok(Arc::clone(repository));
        }
        let repository = Arc::new(self.open_new()?);
        opened.push((self.clone(), Arc::clone(&repository)));
        Ok(repository)
    }

    fn open_new(&self) -> Result<Repository, String> {
        let cache_dir = match &self.cache_dir {
            Some(dir) => dir.clone(),
            None => default_cache_dir().ok_or(
                "no extension cache directory: set --cache-dir DIR (or XDG_CACHE_HOME or HOME)",
            )?,
        };
        let base = |flag: &str, value: Option<&String>, default: &str| {
            let text = value.map_or(default, String::as_str);
            parse_base(text).ok_or_else(|| {
                format!(
                    "{flag} `{text}` is not an absolute https URL without userinfo, query, or fragment (http only for a loopback host)"
                )
            })
        };
        let bases = Bases {
            api: base(
                "--github-api-base",
                self.api_base.as_ref(),
                DEFAULT_API_BASE,
            )?,
            raw: base(
                "--github-raw-base",
                self.raw_base.as_ref(),
                DEFAULT_RAW_BASE,
            )?,
            download: base(
                "--github-download-base",
                self.download_base.as_ref(),
                DEFAULT_DOWNLOAD_BASE,
            )?,
        };
        let token = match &self.token_env {
            None => None,
            Some(name) => Some(std::env::var(name).map_err(|_| {
                format!("--github-token-env names `{name}`, which is not set to a UTF-8 value")
            })?),
        };
        Ok(Repository {
            cache: Cache { root: cache_dir },
            offline: self.offline,
            bases,
            token,
            #[cfg(feature = "wasmtime")]
            client: OnceLock::new(),
            #[cfg(feature = "wasmtime")]
            resolver: OnceLock::new(),
            releases: Mutex::new(BTreeMap::new()),
            entries: Mutex::new(BTreeMap::new()),
            documents: Mutex::new(BTreeMap::new()),
        })
    }
}

/// `${XDG_CACHE_HOME:-$HOME/.cache}/scxml/extensions`, read once at the
/// CLI's composition root.
fn default_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".cache"))
        })?;
    Some(base.join("scxml").join("extensions"))
}

/// A base URL: absolute `https` (or `http` to a loopback host) without
/// userinfo, query, or fragment.
fn parse_base(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text).ok()?;
    (admissible_scheme(&url)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn admissible_scheme(url: &url::Url) -> bool {
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => {
            matches!(
                url.host(),
                Some(url::Host::Ipv4(ip)) if ip.is_loopback()
            ) || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
                || url.host_str() == Some("localhost")
        }
        _ => false,
    }
}

#[derive(Clone, Debug)]
struct Bases {
    api: url::Url,
    raw: url::Url,
    download: url::Url,
}

#[cfg(feature = "wasmtime")]
impl Bases {
    /// The resolver component's configuration: the three bases.
    fn configuration(&self) -> [(String, String); 3] {
        [
            ("github-api-base".to_owned(), self.api.to_string()),
            ("github-raw-base".to_owned(), self.raw.to_string()),
            ("github-download-base".to_owned(), self.download.to_string()),
        ]
    }

    /// The origins the resolver component may reach: the bases' and
    /// GitHub's release-asset hosts.
    fn allowlist(&self) -> Result<scxml_host_wasmtime::HttpAllowlist, String> {
        scxml_host_wasmtime::HttpAllowlist::new(
            [&self.api, &self.raw, &self.download]
                .into_iter()
                .map(|base| base.origin().ascii_serialization())
                .chain(RELEASE_ASSET_ORIGINS.map(str::to_owned)),
        )
        .map_err(|error| format!("a repository base is not admissible: {error}"))
    }
}

/// One cache index entry: what the first resolution of a URI pinned.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    /// The repository namespace or service URI.
    uri: String,
    /// The release a tagless URI was pinned to; `None` for a pinned URI,
    /// whose tag is part of it.
    tag: Option<String>,
    commit: String,
    manifest_sha256: String,
    /// A namespace's schema digest; `None` for a service, whose manifest
    /// (pinned by `manifest_sha256`) declares its schema's digest.
    schema_sha256: Option<String>,
    /// Known once the release's `SHA256SUMS` was read.
    component_sha256: Option<String>,
}

impl Entry {
    /// Canonical bytes: fixed field order, two-space indentation, a final
    /// newline; a namespace's entry in `stateforward.scxml.extension-cache/v1`,
    /// a service's in `stateforward.scxml.service-cache/v1`.
    fn encode(&self) -> Vec<u8> {
        let string = |value: &str| serde_json::Value::String(value.to_owned()).to_string();
        let tag = self
            .tag
            .as_deref()
            .map(|tag| format!("\n  \"tag\": {},", string(tag)))
            .unwrap_or_default();
        let mut text = match &self.schema_sha256 {
            Some(schema) => format!(
                "{{\n  \"format\": {},\n  \"namespace-uri\": {},{tag}\n  \"commit\": {},\n  \"manifest-sha256\": {},\n  \"schema-sha256\": {}",
                string(ENTRY_FORMAT),
                string(&self.uri),
                string(&self.commit),
                string(&self.manifest_sha256),
                string(schema),
            ),
            None => format!(
                "{{\n  \"format\": {},\n  \"service-uri\": {},{tag}\n  \"commit\": {},\n  \"manifest-sha256\": {}",
                string(SERVICE_ENTRY_FORMAT),
                string(&self.uri),
                string(&self.commit),
                string(&self.manifest_sha256),
            ),
        };
        if let Some(component) = &self.component_sha256 {
            let _ = write!(text, ",\n  \"component-sha256\": {}", string(component));
        }
        text.push_str("\n}\n");
        text.into_bytes()
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let serde_json::Value::Object(object) =
            serde_json::from_slice(bytes).map_err(|_| "it is not JSON".to_owned())?
        else {
            return Err("it is not a JSON object".into());
        };
        let field = |name: &str| match object.get(name) {
            Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(format!("field `{name}` must be a string")),
            None => Ok(None),
        };
        let required = |name: &str| field(name)?.ok_or_else(|| format!("missing field `{name}`"));
        let format = required("format")?;
        let (fields, uri_field): (&[&str], &str) = match format.as_str() {
            ENTRY_FORMAT => (&ENTRY_FIELDS, "namespace-uri"),
            SERVICE_ENTRY_FORMAT => (&SERVICE_ENTRY_FIELDS, "service-uri"),
            _ => {
                return Err(format!(
                    "field `format` must be `{ENTRY_FORMAT}` or `{SERVICE_ENTRY_FORMAT}`"
                ));
            }
        };
        if let Some(unknown) = object.keys().find(|key| !fields.contains(&key.as_str())) {
            return Err(format!("unknown field `{}`", unknown.escape_debug()));
        }
        let entry = Self {
            uri: required(uri_field)?,
            tag: field("tag")?,
            commit: required("commit")?,
            manifest_sha256: required("manifest-sha256")?,
            schema_sha256: if format == ENTRY_FORMAT {
                Some(required("schema-sha256")?)
            } else {
                None
            },
            component_sha256: field("component-sha256")?,
        };
        if entry.tag.as_deref().is_some_and(|tag| !valid_tag(tag)) {
            return Err("field `tag` is not a release tag".into());
        }
        if !is_commit(&entry.commit)
            || !is_sha256_hex(&entry.manifest_sha256)
            || entry
                .schema_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256_hex(digest))
            || entry
                .component_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256_hex(digest))
        {
            return Err("a commit or digest is not canonical lowercase hex".into());
        }
        Ok(entry)
    }
}

/// The content-addressed cache.
#[derive(Clone, Debug)]
struct Cache {
    root: PathBuf,
}

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl Cache {
    fn object_path(&self, digest: &str) -> PathBuf {
        self.root.join("sha256").join(digest)
    }

    fn entry_path(&self, namespace_uri: &str) -> PathBuf {
        self.root
            .join("index")
            .join(format!("{}.json", sha256_hex(namespace_uri.as_bytes())))
    }

    /// The cached bytes of `digest`: `Ok(None)` when absent; an error when
    /// present but unreadable, oversized, or not matching `digest`.
    fn object(&self, digest: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
        let path = self.object_path(digest);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = crate::extensions::read_bounded(&path, limit)
            .map_err(|error| format!("cannot read the cached `{}`: {error}", path.display()))?;
        if sha256_hex(&bytes) != digest {
            return Err(format!(
                "the cached `{}` does not match its SHA-256 (it was modified)\n  = help: delete it; the next run without --offline fetches and verifies it again",
                path.display()
            ));
        }
        Ok(Some(bytes))
    }

    fn store(&self, bytes: &[u8]) -> Result<String, String> {
        let digest = sha256_hex(bytes);
        let path = self.object_path(&digest);
        if !path.exists() {
            write_atomically(&path, bytes)?;
        }
        Ok(digest)
    }

    /// The entry pinned for `uri`; `service` says which kind it must be.
    fn entry(&self, namespace_uri: &str, service: bool) -> Result<Option<Entry>, String> {
        let path = self.entry_path(namespace_uri);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = crate::extensions::read_bounded(&path, MAX_ENTRY_BYTES).map_err(|error| {
            format!("cannot read the cache index `{}`: {error}", path.display())
        })?;
        let entry = Entry::decode(&bytes).map_err(|error| {
            format!("the cache index `{}` is malformed: {error}", path.display())
        })?;
        if entry.uri != namespace_uri || entry.schema_sha256.is_none() != service {
            return Err(format!(
                "the cache index `{}` names another namespace or service",
                path.display()
            ));
        }
        let tagless = RepositoryUri::parse(namespace_uri)
            .or_else(|| RepositoryUri::parse_service(namespace_uri))
            .is_some_and(|uri| uri.tag.is_none());
        if entry.tag.is_some() != tagless {
            return Err(format!(
                "the cache index `{}` records a release tag exactly when its URI has none, and this one does not",
                path.display()
            ));
        }
        Ok(Some(entry))
    }

    fn write_entry(&self, entry: &Entry) -> Result<(), String> {
        write_atomically(&self.entry_path(&entry.uri), &entry.encode())
    }
}

/// Writes `bytes` to a sibling temporary file and renames it over `path`, so
/// a reader never sees a partial file.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("a cache path has no directory")?;
    fs::create_dir_all(directory).map_err(|error| {
        format!(
            "cannot create the cache directory `{}`: {error}",
            directory.display()
        )
    })?;
    let temporary = directory.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let written = fs::File::create(&temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot write the cache `{}`: {error}",
            path.display()
        ));
    }
    Ok(())
}

/// The bounded client of the documents a run loads
/// ([`Repository::fetch_document`]).
#[cfg(feature = "wasmtime")]
struct Client {
    agent: ureq::Agent,
}

#[cfg(feature = "wasmtime")]
impl Client {
    fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(TOTAL_TIMEOUT))
            .max_response_header_size(MAX_RESPONSE_HEAD_BYTES)
            .user_agent("scx")
            .tls_config(tls)
            .build()
            .new_agent();
        Self { agent }
    }
}

/// Resolves repository namespaces and services through the cache and,
/// unless offline, the embedded GitHub resolver component, and fetches the
/// documents a run loads. One per root session ([`RepositoryFlags::open`]).
pub(crate) struct Repository {
    cache: Cache,
    offline: bool,
    bases: Bases,
    token: Option<String>,
    #[cfg(feature = "wasmtime")]
    client: OnceLock<Client>,
    /// The GitHub resolver component, compiled on first use.
    #[cfg(feature = "wasmtime")]
    resolver: OnceLock<Result<scxml_host_wasmtime::ResolverComponent, String>>,
    /// The latest release (tag, commit) of each repository a tagless URI
    /// named, asked once per root session.
    releases: Mutex<BTreeMap<(String, String), (String, String)>>,
    /// The entry each URI resolved to in this root session.
    entries: Mutex<BTreeMap<String, Entry>>,
    /// Every document URL fetched in this root session, and the outcome.
    documents: Mutex<BTreeMap<String, Result<Fetched, String>>>,
}

/// One fetched document: its exact bytes and the media type its source
/// indicated (an HTTP `Content-Type` without parameters), if any.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Fetched {
    pub(crate) bytes: Vec<u8>,
    pub(crate) media_type: Option<String>,
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Repository")
            .field("offline", &self.offline)
            .finish_non_exhaustive()
    }
}

/// A resolved extension component: its exact bytes and their SHA-256.
pub(crate) struct ResolvedComponent {
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
}

/// One file of a release, as the resolver component fetches it
/// (`resolver-api.artifact`).
#[derive(Clone, Copy, Debug)]
enum Artifact<'a> {
    Manifest,
    Schema(&'a str),
    Component(&'a str),
    Sha256Sums,
}

/// The message of a resolver component failure: its own detail, or the
/// host's failure.
#[cfg(feature = "wasmtime")]
fn resolver_failure(error: scxml_host_wasmtime::HostError) -> String {
    match error {
        scxml_host_wasmtime::HostError::Component(detail) => detail,
        error => format!("the repository resolver failed: {error}"),
    }
}

impl Repository {
    #[cfg(feature = "wasmtime")]
    fn client(&self) -> &Client {
        self.client.get_or_init(Client::new)
    }

    /// The embedded GitHub resolver component, authenticated by its
    /// SHA-256 and granted only the bases' origins and GitHub's
    /// release-asset hosts; the token, if any, goes to the API origin only.
    #[cfg(feature = "wasmtime")]
    fn resolver(&self) -> Result<&scxml_host_wasmtime::ResolverComponent, String> {
        self.resolver
            .get_or_init(|| {
                let digest = scxml_host_wasmtime::TrustedComponentDigest::from_hex(
                    crate::components::embedded::RESOLVER_COMPONENT_SHA256,
                )
                .map_err(|error| format!("the embedded resolver's digest is invalid: {error}"))?;
                scxml_host_wasmtime::ResolverComponent::new(
                    crate::components::embedded::RESOLVER_COMPONENT,
                    digest,
                    self.bases.allowlist()?,
                    scxml_host_wasmtime::ExtensionHttpConfig {
                        connect_timeout: CONNECT_TIMEOUT,
                        total_timeout: TOTAL_TIMEOUT,
                    },
                    self.token
                        .clone()
                        .map(|token| (self.bases.api.origin(), token)),
                )
                .map_err(|error| format!("the repository resolver does not load: {error}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn require_online(&self, uri: &str, what: &str) -> Result<(), String> {
        if self.offline {
            return Err(format!(
                "{} `{uri}`: its {what} is not in the extension cache, and --offline forbids fetching it\n  = help: run once without --offline to fetch and pin it",
                label(uri)
            ));
        }
        Ok(())
    }

    /// The release (tag, commit) the package `uri` resolves to: `tag`'s,
    /// or, without one, its repository's latest, as the resolver component
    /// reports it.
    #[cfg(feature = "wasmtime")]
    fn release(&self, uri: &RepositoryUri, tag: Option<&str>) -> Result<(String, String), String> {
        let release = self
            .resolver()?
            .resolve(&self.bases.configuration(), &uri.tagless, tag)
            .map_err(resolver_failure)?
            .ok_or_else(|| format!("the repository resolver does not serve `{}`", uri.tagless))?;
        Ok((release.tag, release.commit))
    }

    /// The bytes of `artifact` of the package `uri` at the release `(tag,
    /// commit)`, within `limit`, as the resolver component fetches them;
    /// the caller verifies them.
    #[cfg(feature = "wasmtime")]
    fn fetch(
        &self,
        uri: &RepositoryUri,
        (tag, commit): (&str, &str),
        artifact: Artifact<'_>,
        limit: u64,
    ) -> Result<Vec<u8>, String> {
        use scxml_host_wasmtime::resolver_api;
        let artifact = match artifact {
            Artifact::Manifest => resolver_api::Artifact::Manifest,
            Artifact::Schema(path) => resolver_api::Artifact::Schema(path.to_owned()),
            Artifact::Component(asset) => resolver_api::Artifact::Component(asset.to_owned()),
            Artifact::Sha256Sums => resolver_api::Artifact::Sha256sums,
        };
        let release = resolver_api::Release {
            tag: tag.to_owned(),
            commit: commit.to_owned(),
        };
        self.resolver()?
            .fetch(
                &self.bases.configuration(),
                &uri.tagless,
                &release,
                &artifact,
                limit,
            )
            .map_err(resolver_failure)?
            .ok_or_else(|| format!("the repository resolver does not serve `{}`", uri.tagless))
    }

    /// Without Wasmtime this build runs no resolver component: only what
    /// the cache already holds resolves.
    #[cfg(not(feature = "wasmtime"))]
    fn release(&self, uri: &RepositoryUri, tag: Option<&str>) -> Result<(String, String), String> {
        let _ = (&self.token, &self.bases, tag);
        Err(no_resolver(uri))
    }

    /// Without Wasmtime this build runs no resolver component.
    #[cfg(not(feature = "wasmtime"))]
    #[expect(
        clippy::unused_self,
        reason = "the same signature as the networked build's"
    )]
    fn fetch(
        &self,
        uri: &RepositoryUri,
        _release: (&str, &str),
        _artifact: Artifact<'_>,
        _limit: u64,
    ) -> Result<Vec<u8>, String> {
        Err(no_resolver(uri))
    }

    /// The document at `url`, fetched once per root session (its outcome,
    /// failure included, is kept): a `file:` URL is read as a regular file,
    /// an `https:` URL fetched within [`MAX_DOCUMENT_BYTES`], following at
    /// most [`MAX_REDIRECTS`] redirects, each to another `https:` URL, with
    /// the client's timeouts, no retry, and no credentials, cookies, or
    /// proxies. `--offline` forbids every network fetch; any other scheme is
    /// refused.
    ///
    /// # Errors
    ///
    /// A message naming why the document cannot be loaded.
    pub(crate) fn fetch_document(&self, url: &url::Url) -> Result<Fetched, String> {
        let key = url.as_str().to_owned();
        if let Some(outcome) = self
            .documents
            .lock()
            .map_err(|_| "the document memo is unavailable".to_owned())?
            .get(&key)
        {
            return outcome.clone();
        }
        let span = observe(Module::Cli, Operation::CliReadDocument);
        let outcome = match url.scheme() {
            "file" => url
                .to_file_path()
                .map_err(|()| format!("`{url}` is not a local file path"))
                .and_then(|path| {
                    crate::read_source(&path)
                        .map(|bytes| Fetched {
                            bytes,
                            media_type: None,
                        })
                        .map_err(|error| error.to_string())
                }),
            "https"
                if url.host().is_some()
                    && url.username().is_empty()
                    && url.password().is_none() =>
            {
                if self.offline {
                    Err(format!(
                        "`{url}` is not a local file, and --offline forbids fetching it"
                    ))
                } else {
                    self.get_document(url.clone())
                }
            }
            _ => Err(format!(
                "`{url}` is neither an https: URL (without userinfo) nor a file: URL"
            )),
        };
        record_outcome(&span, outcome.is_ok());
        self.documents
            .lock()
            .map_err(|_| "the document memo is unavailable".to_owned())?
            .insert(key, outcome.clone());
        outcome
    }

    /// Without Wasmtime this build links no network client.
    #[cfg(not(feature = "wasmtime"))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the same signature as the networked build's"
    )]
    #[expect(
        clippy::unused_self,
        reason = "the same signature as the networked build's"
    )]
    fn get_document(&self, url: url::Url) -> Result<Fetched, String> {
        Err(format!(
            "cannot fetch `{url}`: this scx was built without `--features wasmtime`, which links its network client"
        ))
    }

    /// GETs one `https:` document within [`MAX_DOCUMENT_BYTES`], following
    /// at most [`MAX_REDIRECTS`] redirects to `https:` URLs.
    #[cfg(feature = "wasmtime")]
    fn get_document(&self, url: url::Url) -> Result<Fetched, String> {
        let shown = url.to_string();
        let mut current = url;
        for _ in 0..=MAX_REDIRECTS {
            if current.scheme() != "https"
                || current.host().is_none()
                || !current.username().is_empty()
                || current.password().is_some()
            {
                return Err(format!(
                    "`{shown}` leads to `{current}`, which is not an https: URL without userinfo"
                ));
            }
            let mut response = self
                .client()
                .agent
                .get(current.as_str())
                .call()
                .map_err(|error| format!("cannot fetch `{shown}`: {error}"))?;
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| format!("`{shown}` redirects without a location"))?;
                current = current
                    .join(location)
                    .map_err(|_| format!("`{shown}` redirects to an invalid location"))?;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(format!("`{shown}` answered HTTP {status}"));
            }
            let media_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            let too_large =
                || format!("`{shown}` is larger than its {MAX_DOCUMENT_BYTES}-byte bound");
            if response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .is_some_and(|length| length > MAX_DOCUMENT_BYTES)
            {
                return Err(too_large());
            }
            let bytes = response
                .body_mut()
                .with_config()
                .limit(MAX_DOCUMENT_BYTES.saturating_add(1))
                .read_to_vec()
                .map_err(|error| match error {
                    ureq::Error::BodyExceedsLimit(_) => too_large(),
                    error => format!("cannot read `{shown}`: {error}"),
                })?;
            if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_DOCUMENT_BYTES) {
                return Err(too_large());
            }
            return Ok(Fetched { bytes, media_type });
        }
        Err(format!(
            "`{shown}` redirects more than {MAX_REDIRECTS} times"
        ))
    }

    /// The latest release of the repository a tagless `uri` names: its tag
    /// and the commit the tag names, asked once per repository per root
    /// session.
    fn latest_release(&self, uri: &RepositoryUri) -> Result<(String, String), String> {
        let key = (uri.owner.clone(), uri.repo.clone());
        if let Some(release) = self
            .releases
            .lock()
            .map_err(|_| "the release memo is unavailable".to_owned())?
            .get(&key)
        {
            return Ok(release.clone());
        }
        let release = self.release(uri, None)?;
        self.releases
            .lock()
            .map_err(|_| "the release memo is unavailable".to_owned())?
            .insert(key, release.clone());
        Ok(release)
    }

    /// The entry `text` (the URI `uri` parses from) resolves to in this root
    /// session: the cache's pin of a pinned URI (or, on first use, `pin` at
    /// the commit its tag names); for a tagless URI, the latest release,
    /// re-pinned with `pin` when the cache pinned another one (offline: the
    /// cache's pin).
    fn session_entry(
        &self,
        text: &str,
        uri: &RepositoryUri,
        service: bool,
        pin: impl FnOnce(Option<String>, String) -> Result<Entry, String>,
    ) -> Result<Entry, String> {
        let memo = |entries: &Mutex<BTreeMap<String, Entry>>| {
            entries
                .lock()
                .map(|entries| entries.get(text).cloned())
                .map_err(|_| "the resolution memo is unavailable".to_owned())
        };
        if let Some(entry) = memo(&self.entries)? {
            return Ok(entry);
        }
        let cached = self.cache.entry(text, service)?;
        let entry = match (&uri.tag, cached) {
            (Some(_), Some(entry)) => entry,
            (Some(tag), None) => {
                self.require_online(text, "package")?;
                pin(None, self.release(uri, Some(tag))?.1)?
            }
            (None, Some(entry)) if self.offline => entry,
            (None, cached) => {
                self.require_online(text, "package")?;
                let (tag, commit) = self.latest_release(uri)?;
                match cached {
                    Some(entry) if entry.commit == commit && entry.tag.as_ref() == Some(&tag) => {
                        entry
                    }
                    _ => pin(Some(tag), commit)?,
                }
            }
        };
        self.remember(&entry)?;
        Ok(entry)
    }

    fn remember(&self, entry: &Entry) -> Result<(), String> {
        self.entries
            .lock()
            .map_err(|_| "the resolution memo is unavailable".to_owned())?
            .insert(entry.uri.clone(), entry.clone());
        Ok(())
    }

    /// Cached bytes of `digest`, or (online) the bytes `fetch` returns,
    /// which must match `digest`.
    fn pinned(
        &self,
        namespace_uri: &str,
        what: &str,
        digest: &str,
        limit: u64,
        fetch: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String> {
        if let Some(bytes) = self.cache.object(digest, limit)? {
            return Ok(bytes);
        }
        self.require_online(namespace_uri, what)?;
        let bytes = fetch()?;
        if sha256_hex(&bytes) != digest {
            return Err(format!(
                "{} `{namespace_uri}`: its {what} does not match the SHA-256 pinned for it (the release's {SUMS_ASSET}, its pinned manifest, or the cache's first resolution)",
                label(namespace_uri)
            ));
        }
        self.cache.store(&bytes)?;
        Ok(bytes)
    }

    /// The package of the repository namespace `namespace_uri` (manifest and
    /// schema, verified), from the cache or its repository; pins the URI on
    /// first use.
    ///
    /// # Errors
    ///
    /// A message for every acquisition, bound, or verification failure.
    pub(crate) fn resolve_package(&self, namespace_uri: &str) -> Result<Package, String> {
        let span = observe(Module::Cli, Operation::CliResolveExtensionPackage);
        let result = self.resolve_package_inner(namespace_uri);
        record_outcome(&span, result.is_ok());
        result.map(|(package, _)| package)
    }

    fn resolve_package_inner(&self, namespace_uri: &str) -> Result<(Package, Entry), String> {
        let uri = RepositoryUri::parse(namespace_uri)
            .ok_or_else(|| format!("`{namespace_uri}` is not a repository namespace URI"))?;
        let schema_limit = u64::try_from(MAX_COMPONENT_SCHEMA_BYTES)
            .map_err(|_| "the schema bound does not fit")?;
        let entry = self.session_entry(namespace_uri, &uri, false, |tag, commit| {
            self.pin_package(namespace_uri, &uri, tag, commit, schema_limit)
        })?;
        let schema_sha256 = entry
            .schema_sha256
            .clone()
            .ok_or("a namespace's cache index pins no schema")?;
        let tag = release_tag(&uri, &entry)?;
        let release = (tag.as_str(), entry.commit.as_str());
        let manifest = self.pinned(
            namespace_uri,
            "manifest",
            &entry.manifest_sha256,
            MAX_MANIFEST_BYTES,
            || self.fetch(&uri, release, Artifact::Manifest, MAX_MANIFEST_BYTES),
        )?;
        let parsed = scxml_validator_core::ExtensionPackageManifest::from_json(&manifest).map_err(
            |error| {
                format!(
                    "repository namespace `{namespace_uri}`: its manifest is malformed: {error}"
                )
            },
        )?;
        check_manifest(namespace_uri, &parsed)?;
        let schema = self.pinned(
            namespace_uri,
            "schema",
            &schema_sha256,
            schema_limit,
            || {
                self.fetch(
                    &uri,
                    release,
                    Artifact::Schema(&parsed.schema_artifact),
                    schema_limit,
                )
            },
        )?;
        let mut package = verify_package(namespace_uri, &manifest, &schema, &entry)?;
        package.validator = self.validator(
            namespace_uri,
            &uri,
            release,
            package.manifest.validator_artifact.as_deref(),
            package.manifest.validator_sha256.as_deref(),
        )?;
        Ok((package, entry))
    }

    /// The package validator component a verified repository manifest
    /// names (`validator-artifact`, a release asset), authenticated like a
    /// package's own component: the release's `SHA256SUMS` must list
    /// exactly the manifest's `validator-sha256` for it, and the bytes must
    /// hash to it; cached by that digest, which the pinned manifest pins.
    /// `None` when the manifest names none.
    fn validator(
        &self,
        package_uri: &str,
        uri: &RepositoryUri,
        release: (&str, &str),
        artifact: Option<&str>,
        sha256: Option<&str>,
    ) -> Result<Option<Vec<u8>>, String> {
        let (Some(asset), Some(sha256)) = (artifact, sha256) else {
            return Ok(None);
        };
        if !valid_segment(asset) {
            return Err(format!(
                "{} `{package_uri}`: its `validator-artifact` is not a plain release asset name",
                uri.kind.label()
            ));
        }
        self.pinned(
            package_uri,
            "package validator",
            sha256,
            MAX_COMPONENT_BYTES,
            || {
                let sums = self.fetch(uri, release, Artifact::Sha256Sums, MAX_SUMS_BYTES)?;
                if sums_entry(&sums, asset).as_deref() != Some(sha256) {
                    return Err(format!(
                        "{} `{package_uri}`: release `{}` does not list the manifest's `validator-sha256` for `{asset}` in its {SUMS_ASSET}",
                        uri.kind.label(),
                        release.0
                    ));
                }
                self.fetch(uri, release, Artifact::Component(asset), MAX_COMPONENT_BYTES)
            },
        )
        .map(Some)
    }

    /// First use of `namespace_uri` (or of its tagless URI's new release):
    /// fetches and verifies its package at `commit`, stores it, and pins the
    /// URI (with `tag` for a tagless URI).
    fn pin_package(
        &self,
        namespace_uri: &str,
        uri: &RepositoryUri,
        tag: Option<String>,
        commit: String,
        schema_limit: u64,
    ) -> Result<Entry, String> {
        let release_tag = tag
            .clone()
            .or_else(|| uri.tag.clone())
            .ok_or("a tagless URI resolved to no release tag")?;
        let release = (release_tag.as_str(), commit.as_str());
        let manifest = self.fetch(uri, release, Artifact::Manifest, MAX_MANIFEST_BYTES)?;
        let parsed = scxml_validator_core::ExtensionPackageManifest::from_json(&manifest).map_err(
            |error| {
                format!(
                    "repository namespace `{namespace_uri}`: its manifest is malformed: {error}"
                )
            },
        )?;
        check_manifest(namespace_uri, &parsed)?;
        let schema = self.fetch(
            uri,
            release,
            Artifact::Schema(&parsed.schema_artifact),
            schema_limit,
        )?;
        let entry = Entry {
            uri: namespace_uri.to_owned(),
            tag,
            commit,
            manifest_sha256: sha256_hex(&manifest),
            schema_sha256: Some(sha256_hex(&schema)),
            component_sha256: None,
        };
        // Only a verified package is stored and pinned.
        verify_package(namespace_uri, &manifest, &schema, &entry)?;
        self.cache.store(&manifest)?;
        self.cache.store(&schema)?;
        self.cache.write_entry(&entry)?;
        Ok(entry)
    }

    /// The SHA-256 of the repository namespace's extension component, as the
    /// cache pinned it or the release's `SHA256SUMS` lists it (then pinned).
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_package`], and a release without the asset in
    /// `SHA256SUMS`.
    pub(crate) fn component_sha256(&self, namespace_uri: &str) -> Result<String, String> {
        let (package, entry) = self.resolve_package_inner(namespace_uri)?;
        let uri = RepositoryUri::parse(namespace_uri)
            .ok_or_else(|| format!("`{namespace_uri}` is not a repository namespace URI"))?;
        self.pin_component(
            &uri,
            namespace_uri,
            &package.manifest.extension_artifact,
            entry,
        )
        .map(|entry| entry.component_sha256.unwrap_or_default())
    }

    /// Pins the SHA-256 the release's `SHA256SUMS` lists for `asset`, once.
    fn pin_component(
        &self,
        uri: &RepositoryUri,
        namespace_uri: &str,
        asset: &str,
        mut entry: Entry,
    ) -> Result<Entry, String> {
        if entry.component_sha256.is_some() {
            return Ok(entry);
        }
        let tag = release_tag(uri, &entry)?;
        self.require_online(
            namespace_uri,
            match uri.kind {
                Kind::Namespace => "extension component digest",
                Kind::Service => "service component digest",
            },
        )?;
        let sums = self.fetch(
            uri,
            (&tag, &entry.commit),
            Artifact::Sha256Sums,
            MAX_SUMS_BYTES,
        )?;
        let digest = sums_entry(&sums, asset).ok_or_else(|| {
            format!(
                "{} `{namespace_uri}`: release `{tag}` lists no single SHA-256 for `{asset}` in its {SUMS_ASSET}",
                uri.kind.label(),
            )
        })?;
        entry.component_sha256 = Some(digest);
        self.cache.write_entry(&entry)?;
        self.remember(&entry)?;
        Ok(entry)
    }

    /// The repository namespace's package and extension component bytes,
    /// authenticated by the pinned SHA-256: from the cache, or downloaded
    /// from the release and verified.
    ///
    /// # Errors
    ///
    /// As [`Self::component_sha256`], and a download that does not match.
    pub(crate) fn resolve_component(
        &self,
        namespace_uri: &str,
    ) -> Result<ResolvedComponent, String> {
        let span = observe(Module::Cli, Operation::CliResolveExtensionComponent);
        let result = (|| {
            let (package, entry) = self.resolve_package_inner(namespace_uri)?;
            let uri = RepositoryUri::parse(namespace_uri)
                .ok_or_else(|| format!("`{namespace_uri}` is not a repository namespace URI"))?;
            let asset = package.manifest.extension_artifact.clone();
            let entry = self.pin_component(&uri, namespace_uri, &asset, entry)?;
            let tag = release_tag(&uri, &entry)?;
            let sha256 = entry
                .component_sha256
                .clone()
                .ok_or("the cache pinned no component digest")?;
            let bytes = self.pinned(
                namespace_uri,
                "extension component",
                &sha256,
                MAX_COMPONENT_BYTES,
                || {
                    self.fetch(
                        &uri,
                        (&tag, &entry.commit),
                        Artifact::Component(&asset),
                        MAX_COMPONENT_BYTES,
                    )
                },
            )?;
            Ok(ResolvedComponent { bytes, sha256 })
        })();
        record_outcome(&span, result.is_ok());
        result
    }

    /// The package of the repository service `service_uri` (manifest,
    /// and optional schema, verified), from the cache or its
    /// repository; pins the URI on first use.
    ///
    /// # Errors
    ///
    /// A message for every acquisition, bound, or verification failure.
    pub(crate) fn resolve_service_package(
        &self,
        service_uri: &str,
    ) -> Result<ServicePackage, String> {
        let span = observe(Module::Cli, Operation::CliResolveServicePackage);
        let result = self.resolve_service_inner(service_uri);
        record_outcome(&span, result.is_ok());
        result.map(|(package, _)| package)
    }

    fn resolve_service_inner(&self, service_uri: &str) -> Result<(ServicePackage, Entry), String> {
        let uri = RepositoryUri::parse_service(service_uri)
            .ok_or_else(|| format!("`{service_uri}` is not a repository service URI"))?;
        let entry = self.session_entry(service_uri, &uri, true, |tag, commit| {
            self.pin_service(service_uri, &uri, tag, commit)
        })?;
        let tag = release_tag(&uri, &entry)?;
        let release = (tag.as_str(), entry.commit.as_str());
        let manifest = self.pinned(
            service_uri,
            "manifest",
            &entry.manifest_sha256,
            MAX_MANIFEST_BYTES,
            || self.fetch(&uri, release, Artifact::Manifest, MAX_MANIFEST_BYTES),
        )?;
        let parsed = parse_service_manifest(service_uri, &manifest)?;
        let schema = self.service_schema(service_uri, &uri, release, &parsed)?;
        let mut package = verify_service(service_uri, &manifest, schema.as_deref(), &entry)?;
        package.validator = self.validator(
            service_uri,
            &uri,
            release,
            package.manifest.validator_artifact.as_deref(),
            package.manifest.validator_sha256.as_deref(),
        )?;
        Ok((package, entry))
    }

    /// The optional schema of a repository service at `release`, checked
    /// against the digest its manifest declares.
    fn service_schema(
        &self,
        service_uri: &str,
        uri: &RepositoryUri,
        release: (&str, &str),
        manifest: &scxml_validator_core::ServicePackageManifest,
    ) -> Result<Option<Vec<u8>>, String> {
        check_service_manifest(service_uri, manifest)?;
        let schema = match (&manifest.schema_artifact, &manifest.schema_sha256) {
            (Some(artifact), Some(digest)) => Some(self.pinned(
                service_uri,
                "schema",
                digest,
                crate::services::MAX_SCHEMA_BYTES,
                || {
                    self.fetch(
                        uri,
                        release,
                        Artifact::Schema(artifact),
                        crate::services::MAX_SCHEMA_BYTES,
                    )
                },
            )?),
            _ => None,
        };
        Ok(schema)
    }

    /// First use of `service_uri` (or of its tagless URI's new release):
    /// fetches and verifies its package at `commit`, stores it, and pins the
    /// URI (with `tag` for a tagless URI).
    fn pin_service(
        &self,
        service_uri: &str,
        uri: &RepositoryUri,
        tag: Option<String>,
        commit: String,
    ) -> Result<Entry, String> {
        let release_tag = tag
            .clone()
            .or_else(|| uri.tag.clone())
            .ok_or("a tagless URI resolved to no release tag")?;
        let release = (release_tag.as_str(), commit.as_str());
        let manifest = self.fetch(uri, release, Artifact::Manifest, MAX_MANIFEST_BYTES)?;
        let parsed = parse_service_manifest(service_uri, &manifest)?;
        let schema = self.service_schema(service_uri, uri, release, &parsed)?;
        let entry = Entry {
            uri: service_uri.to_owned(),
            tag,
            commit,
            manifest_sha256: sha256_hex(&manifest),
            schema_sha256: None,
            component_sha256: None,
        };
        // Only a verified package is stored and pinned.
        verify_service(service_uri, &manifest, schema.as_deref(), &entry)?;
        self.cache.store(&manifest)?;
        self.cache.write_entry(&entry)?;
        Ok(entry)
    }

    /// The SHA-256 of the repository service's component, as the cache
    /// pinned it or the release's `SHA256SUMS` lists it (then pinned).
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_service_package`], and a release without the
    /// asset in `SHA256SUMS`.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn service_component_sha256(&self, service_uri: &str) -> Result<String, String> {
        let (package, entry) = self.resolve_service_inner(service_uri)?;
        let uri = RepositoryUri::parse_service(service_uri)
            .ok_or_else(|| format!("`{service_uri}` is not a repository service URI"))?;
        self.pin_component(&uri, service_uri, &package.manifest.service_artifact, entry)?
            .component_sha256
            .ok_or_else(|| "the cache pinned no component digest".to_owned())
    }

    /// The repository service's package and component bytes, authenticated
    /// by the SHA-256 its release's `SHA256SUMS` lists (pinned on first
    /// use): from the cache, or downloaded from the release and verified.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_service_package`], a release without the asset in
    /// `SHA256SUMS`, and a download that does not match.
    pub(crate) fn resolve_service_component(
        &self,
        service_uri: &str,
    ) -> Result<ResolvedService, String> {
        let span = observe(Module::Cli, Operation::CliResolveServiceComponent);
        let result = (|| {
            let (package, entry) = self.resolve_service_inner(service_uri)?;
            let uri = RepositoryUri::parse_service(service_uri)
                .ok_or_else(|| format!("`{service_uri}` is not a repository service URI"))?;
            let asset = package.manifest.service_artifact.clone();
            let entry = self.pin_component(&uri, service_uri, &asset, entry)?;
            let tag = release_tag(&uri, &entry)?;
            let sha256 = entry
                .component_sha256
                .clone()
                .ok_or("the cache pinned no component digest")?;
            let bytes = self.pinned(
                service_uri,
                "service component",
                &sha256,
                MAX_COMPONENT_BYTES,
                || {
                    self.fetch(
                        &uri,
                        (&tag, &entry.commit),
                        Artifact::Component(&asset),
                        MAX_COMPONENT_BYTES,
                    )
                },
            )?;
            Ok(ResolvedService { bytes, sha256 })
        })();
        record_outcome(&span, result.is_ok());
        result
    }
}

/// Why a build without Wasmtime cannot resolve `uri` beyond the cache.
#[cfg(not(feature = "wasmtime"))]
fn no_resolver(uri: &RepositoryUri) -> String {
    format!(
        "cannot resolve `{}`: this scx was built without `--features wasmtime`, which runs its repository resolver component\n  = help: rebuild with `--features wasmtime`; repository namespaces already in the extension cache still resolve",
        uri.tagless
    )
}
/// A resolved repository service component: its exact bytes and SHA-256.
pub(crate) struct ResolvedService {
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "only a run binds the component, which needs the wasmtime feature"
        )
    )]
    pub(crate) bytes: Vec<u8>,
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "only a run binds the component, which needs the wasmtime feature"
        )
    )]
    pub(crate) sha256: String,
}

fn parse_service_manifest(
    service_uri: &str,
    bytes: &[u8],
) -> Result<scxml_validator_core::ServicePackageManifest, String> {
    scxml_validator_core::ServicePackageManifest::from_json(bytes).map_err(|error| {
        format!("repository service `{service_uri}`: its manifest is malformed: {error}")
    })
}

/// What a repository service's manifest must name beyond the format: its own
/// URI exactly, and artifacts that are plain relative paths (the component a
/// single release asset name).
fn check_service_manifest(
    service_uri: &str,
    manifest: &scxml_validator_core::ServicePackageManifest,
) -> Result<(), String> {
    check_identity(service_uri, &manifest.service_uri, "service-uri")?;
    if !valid_segment(&manifest.service_artifact) {
        return Err(format!(
            "repository service `{service_uri}`: its `service-artifact` is not a plain release asset name"
        ));
    }
    if manifest
        .schema_artifact
        .as_deref()
        .is_some_and(|artifact| !valid_relative(artifact))
    {
        return Err(format!(
            "repository service `{service_uri}`: its `schema-artifact` is not a plain relative path"
        ));
    }
    Ok(())
}

/// Verifies a repository service's manifest and optional schema against the
/// pinned entry and each other, and returns its package.
fn verify_service(
    service_uri: &str,
    manifest_bytes: &[u8],
    schema: Option<&[u8]>,
    entry: &Entry,
) -> Result<ServicePackage, String> {
    if sha256_hex(manifest_bytes) != entry.manifest_sha256 {
        return Err(format!(
            "repository service `{service_uri}`: its manifest does not match the SHA-256 pinned for it"
        ));
    }
    let manifest = parse_service_manifest(service_uri, manifest_bytes)?;
    check_service_manifest(service_uri, &manifest)?;
    manifest.verify(schema).map_err(|error| {
        format!("repository service `{service_uri}`: its package does not verify: {error}")
    })?;
    let tag = RepositoryUri::parse_service(service_uri)
        .and_then(|uri| uri.tag)
        .or_else(|| entry.tag.clone())
        .unwrap_or_default();
    Ok(ServicePackage {
        origin: ServiceOrigin::Repository {
            uri: service_uri.to_owned(),
            tag,
            commit: entry.commit.clone(),
        },
        manifest,
        validator: None,
    })
}

/// The release tag of `uri`: its own, or, for a tagless URI, the one its
/// entry was pinned to.
fn release_tag(uri: &RepositoryUri, entry: &Entry) -> Result<String, String> {
    uri.tag
        .clone()
        .or_else(|| entry.tag.clone())
        .ok_or_else(|| "a tagless URI's cache index pins no release tag".to_owned())
}

/// How messages name `uri`: a repository service or namespace.
fn label(uri: &str) -> &'static str {
    if RepositoryUri::parse_service(uri).is_some() {
        Kind::Service.label()
    } else {
        Kind::Namespace.label()
    }
}

/// The closed outcome of one resolution span.
fn record_outcome(span: &scxml_telemetry::FunctionSpan, accepted: bool) {
    if accepted {
        span.event(SignalEvent::ArtifactAccepted);
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::InputRead);
    }
}

/// The package identity `uri` (resolved as written, its tag included)
/// names: its tagless URI ([`scxml_types::strip_tag`]).
fn identity(uri: &str) -> &str {
    scxml_types::strip_tag(uri).map_or(uri, |(tagless, _)| tagless)
}

/// Checks that a release's manifest `field` names the package `uri`
/// resolves: its tagless URI, byte for byte. A manifest naming a pinned URI
/// is refused: the tag is never part of the namespace.
fn check_identity(uri: &str, named: &str, field: &str) -> Result<(), String> {
    let expected = identity(uri);
    if named == expected {
        return Ok(());
    }
    if let Some((tagless, Some(tag))) = scxml_types::strip_tag(named) {
        return Err(format!(
            "{} `{uri}`: its manifest's `{field}` names the pinned URI `{named}`\n  = help: a release's manifest names its package's tagless URI `{tagless}`; `@{tag}` is how a document pins a release, never part of the namespace",
            label(uri)
        ));
    }
    Err(format!(
        "{} `{uri}`: its manifest's `{field}` is `{named}`, not `{expected}`\n  = help: a release's manifest names its package's tagless URI byte for byte",
        label(uri)
    ))
}

/// What a repository namespace's manifest must name beyond the format: its own
/// URI exactly, and artifacts that are plain relative paths (the component a
/// single release asset name).
fn check_manifest(
    namespace_uri: &str,
    manifest: &scxml_validator_core::ExtensionPackageManifest,
) -> Result<(), String> {
    check_identity(namespace_uri, &manifest.namespace_uri, "namespace-uri")?;
    if !valid_segment(&manifest.extension_artifact) {
        return Err(format!(
            "repository namespace `{namespace_uri}`: its `extension-artifact` is not a plain release asset name"
        ));
    }
    if !valid_relative(&manifest.schema_artifact) {
        return Err(format!(
            "repository namespace `{namespace_uri}`: its `schema-artifact` is not a plain relative path"
        ));
    }
    Ok(())
}

/// Verifies a repository namespace's manifest and schema bytes against the
/// pinned entry and each other, and returns its package.
fn verify_package(
    namespace_uri: &str,
    manifest_bytes: &[u8],
    schema_bytes: &[u8],
    entry: &Entry,
) -> Result<Package, String> {
    let manifest = scxml_validator_core::ExtensionPackageManifest::from_json(manifest_bytes)
        .map_err(|error| {
            format!("repository namespace `{namespace_uri}`: its manifest is malformed: {error}")
        })?;
    check_manifest(namespace_uri, &manifest)?;
    if Some(&manifest.schema_sha256) != entry.schema_sha256.as_ref() {
        return Err(format!(
            "repository namespace `{namespace_uri}`: its manifest's schema-sha256 does not match its schema"
        ));
    }
    let schema = SchemaSource::new(manifest.schema_artifact.clone(), schema_bytes.to_vec());
    manifest.verify_schema(&schema).map_err(|error| {
        format!("repository namespace `{namespace_uri}`: its package does not verify: {error}")
    })?;
    if schema_target_namespace(schema_bytes).as_deref() != Some(identity(namespace_uri)) {
        return Err(format!(
            "repository namespace `{namespace_uri}`: its schema's targetNamespace is not `{}`, its package's tagless URI",
            identity(namespace_uri)
        ));
    }
    let tag = RepositoryUri::parse(namespace_uri)
        .and_then(|uri| uri.tag)
        .or_else(|| entry.tag.clone())
        .unwrap_or_default();
    Ok(Package {
        origin: PackageOrigin::Repository {
            uri: namespace_uri.to_owned(),
            tag,
            commit: entry.commit.clone(),
        },
        manifest,
        schema,
        validator: None,
    })
}

/// The `targetNamespace` of an XML Schema document's root element.
fn schema_target_namespace(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "schema",
        text,
    ))
    .ok()?;
    document
        .root
        .name
        .is(Some("http://www.w3.org/2001/XMLSchema"), "schema")
        .then(|| {
            document
                .root
                .attribute("targetNamespace")
                .map(|a| a.value.clone())
        })
        .flatten()
}

/// The one SHA-256 `SHA256SUMS` lists for `asset` (`<hex>  <name>` or
/// `<hex> *<name>` lines); `None` for none, several, or a malformed file.
fn sums_entry(bytes: &[u8], asset: &str) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut found = None;
    for line in text.lines() {
        let (digest, name) = line.split_once(' ')?;
        let name = name.strip_prefix(' ').or_else(|| name.strip_prefix('*'))?;
        if !is_sha256_hex(digest) {
            return None;
        }
        if name == asset && found.replace(digest.to_owned()).is_some() {
            return None;
        }
    }
    found
}

/// The repository packages one document names, each with the release its
/// URIs pin (`@<tag>`, which [`scxml_types::strip_tag`] split off when the
/// document was parsed): the tagless URI of every repository namespace it
/// declares and every repository service its literal SCXML `<invoke type>`s
/// name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DocumentPins {
    /// Repository namespace URIs and their pins.
    pub(crate) namespaces: BTreeMap<String, Option<String>>,
    /// Repository service URIs and their pins.
    pub(crate) services: BTreeMap<String, Option<String>>,
}

/// The repository packages `source` (shown as `name` in messages) names,
/// with their pins; a document that does not parse names none here (the
/// validator reports it).
///
/// # Errors
///
/// A malformed pin (an `@` after a repository package URI not followed by
/// exactly one tag), a pin of a built-in vocabulary, or one package pinned
/// to two releases in the document.
pub(crate) fn document_pins(source: &[u8], name: &str) -> Result<DocumentPins, String> {
    let Ok(text) = std::str::from_utf8(source) else {
        return Ok(DocumentPins::default());
    };
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return Ok(DocumentPins::default());
    };
    let mut pins = DocumentPins::default();
    let mut failure = None;
    document.root.visit(&mut |element| {
        let declarations = element.namespaces.iter().map(|declaration| {
            (
                declaration.uri.as_str(),
                declaration.tag.as_deref(),
                Kind::Namespace,
            )
        });
        let invoke = element
            .name
            .is(Some(scxml_validator_core::SCXML_NAMESPACE), "invoke")
            .then(|| element.attribute("type"))
            .flatten()
            .map(|attribute| {
                (
                    attribute.value.as_str(),
                    attribute.tag.as_deref(),
                    Kind::Service,
                )
            });
        for (uri, tag, kind) in declarations.chain(invoke) {
            if failure.is_none()
                && let Err(error) = pins.add(uri, tag, kind, name)
            {
                failure = Some(error);
            }
        }
    });
    failure.map_or(Ok(pins), Err)
}

impl DocumentPins {
    fn add(&mut self, uri: &str, tag: Option<&str>, kind: Kind, name: &str) -> Result<(), String> {
        if scxml_types::strip_tag(uri).is_none() {
            return Err(format!(
                "{name}: `{uri}` pins a release with a malformed tag\n  = help: a pin is one `@<tag>` at the very end of a repository package URI: `[A-Za-z0-9._+-]`, starting with a letter or digit, without `..`"
            ));
        }
        if let Some(tag) = tag
            && BUILT_IN_NAMESPACES.contains(&uri)
        {
            return Err(format!(
                "{name}: `{uri}@{tag}` pins a built-in vocabulary\n  = help: `{uri}` ships with scx and is versioned with it; it takes no `@<tag>`"
            ));
        }
        let (map, repository) = match kind {
            Kind::Namespace => (&mut self.namespaces, RepositoryUri::parse(uri).is_some()),
            Kind::Service => (
                &mut self.services,
                RepositoryUri::parse_service(uri).is_some(),
            ),
        };
        if !repository {
            return Ok(());
        }
        match map.get_mut(uri) {
            None => {
                map.insert(uri.to_owned(), tag.map(str::to_owned));
            }
            Some(pinned) => match (pinned.as_deref(), tag) {
                (Some(first), Some(second)) if first != second => {
                    return Err(format!(
                        "{name}: {} `{uri}` is pinned to both `@{first}` and `@{second}`\n  = help: one document binds one release of a package; pin one tag (or none, for the root's release)",
                        kind.label()
                    ));
                }
                (None, Some(_)) => *pinned = tag.map(str::to_owned),
                _ => {}
            },
        }
        Ok(())
    }
}

/// How one run binds the repository packages of one kind, from each
/// document's uses (`documents[0]` the root's, then every `<invoke src>`
/// document's in load order; each a tagless URI and its pin) less the
/// `bound` ones: the URI the run resolves for each package (the first
/// document's that uses it, so the root's when the root does), and, per
/// document, each package it pins to a resolution other than the run's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Plan {
    /// Each package's tagless URI and the URI the run resolves for it.
    pub(crate) run: BTreeMap<String, String>,
    /// Each document index, tagless URI, and the pinned URI it resolves.
    pub(crate) pinned: Vec<(usize, String, String)>,
}

/// Plans one kind's resolutions (see [`Plan`]).
pub(crate) fn plan(documents: &[BTreeMap<String, Option<String>>], bound: &[String]) -> Plan {
    let mut plan = Plan::default();
    for uses in documents {
        for (uri, tag) in uses {
            if !bound.contains(uri) && !plan.run.contains_key(uri) {
                plan.run
                    .insert(uri.clone(), pinned_uri(uri, tag.as_deref()));
            }
        }
    }
    for (index, uses) in documents.iter().enumerate() {
        for (uri, tag) in uses {
            if let (Some(tag), Some(run)) = (tag, plan.run.get(uri)) {
                let pinned = pinned_uri(uri, Some(tag));
                if *run != pinned {
                    plan.pinned.push((index, uri.clone(), pinned));
                }
            }
        }
    }
    plan
}

/// Every namespace a document's element and attribute names use, sorted; a
/// document that does not parse uses none here (the validator reports it).
pub(crate) fn used_namespaces(source: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(source) else {
        return Vec::new();
    };
    scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    ))
    .map(|document| document.used_namespaces())
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_uris_are_exact() {
        let uri = RepositoryUri::parse(
            "https://github.com/stateforward/scxml.rs/namespaces/example@v0.2.0",
        )
        .expect("a repository namespace URI");
        assert_eq!(uri.owner, "stateforward");
        assert_eq!(uri.repo, "scxml.rs");
        assert_eq!(
            uri.tagless,
            "https://github.com/stateforward/scxml.rs/namespaces/example"
        );
        assert_eq!(uri.tag.as_deref(), Some("v0.2.0"));
        let tagless =
            RepositoryUri::parse("https://github.com/stateforward/nemobot/namespaces/decision")
                .expect("a tagless repository namespace URI");
        assert_eq!(tagless.tag, None);
        // The vocabularies scx provides are matched first: never fetched.
        for built_in in [
            "https://github.com/stateforward/scxml.rs/namespaces/env",
            "https://github.com/stateforward/scxml.rs/namespaces/event",
            "https://github.com/stateforward/scxml.rs/namespaces/scx",
        ] {
            assert!(RepositoryUri::parse(built_in).is_none(), "{built_in}");
        }
        for other in [
            "https://github.com/stateforward/scxml.rs@v0.2.0",
            "https://github.com/stateforward/scxml.rs/namespaces/example/@v0.2.0",
            "https://github.com/stateforward/scxml.rs/tree/main/namespaces@v1",
            // Only the `namespaces/<name>` folder: the old `elements/` folder,
            // a bare package path, and nested paths are ordinary URIs. (The
            // old folder is spelled in pieces so a repository-wide search
            // for its URIs stays empty.)
            concat!(
                "https://github.com/stateforward/scxml.rs/",
                "elements",
                "/example@v0.2.0"
            ),
            "https://github.com/stateforward/scxml.rs/example@v0.2.0",
            "https://github.com/stateforward/scxml.rs/namespaces@v0.2.0",
            "https://github.com/stateforward/scxml.rs/namespaces/example/package@v0.2.0",
            "https://github.com/stateforward/scxml.rs/Namespaces/example@v0.2.0",
            "https://github.com/stateforward/scxml.rs/namespaces//example@v0.2.0",
            "https://github.com/stateforward/scxml.rs/namespaces/../x@v1",
            "https://github.com/stateforward/scxml.rs/namespaces/example@",
            "https://github.com/stateforward/scxml.rs/namespaces/example@-v1",
            "https://github.com/stateforward/scxml.rs/namespaces/example@v1..2",
            "https://github.com/stateforward/scxml.rs/namespaces/open ai@v1",
            "https://github.com/stateforward/scxml.rs/namespaces/example@v1?x",
            "http://github.com/stateforward/scxml.rs/namespaces/example@v1",
            "HTTPS://github.com/stateforward/scxml.rs/namespaces/example@v1",
            "https://github.com.evil/stateforward/scxml.rs/namespaces/example@v1",
            "https://stateforward.org/scxml/example",
        ] {
            assert!(RepositoryUri::parse(other).is_none(), "{other}");
        }
        let long = format!(
            "https://github.com/o/r/namespaces/{}@v1",
            "a".repeat(MAX_SEGMENT_BYTES + 1)
        );
        assert!(RepositoryUri::parse(&long).is_none());
        assert!(
            RepositoryUri::parse("https://github.com/o/r/namespaces/x.y-z@v1").is_some(),
            "any owner and repository"
        );
    }

    #[test]
    fn cache_entries_are_canonical_and_closed() {
        let entry = Entry {
            uri: "https://github.com/o/r/e@v1".into(),
            tag: None,
            commit: "a".repeat(40),
            manifest_sha256: "b".repeat(64),
            schema_sha256: Some("c".repeat(64)),
            component_sha256: Some("d".repeat(64)),
        };
        let bytes = entry.encode();
        assert_eq!(Entry::decode(&bytes), Ok(entry.clone()));
        assert!(bytes.ends_with(b"\n}\n"));
        let unknown = String::from_utf8(bytes)
            .expect("UTF-8")
            .replace("\"commit\"", "\"extra\": \"x\",\n  \"commit\"");
        assert!(Entry::decode(unknown.as_bytes()).is_err());
        let short = Entry {
            commit: "a".repeat(39),
            ..entry
        };
        assert!(Entry::decode(&short.encode()).is_err());
    }

    #[test]
    fn tagless_entries_record_their_release_tag() {
        let entry = Entry {
            uri: "https://github.com/o/r/services/s".into(),
            tag: Some("v2".into()),
            commit: "a".repeat(40),
            manifest_sha256: "b".repeat(64),
            schema_sha256: None,
            component_sha256: None,
        };
        let bytes = entry.encode();
        let text = String::from_utf8(bytes.clone()).expect("UTF-8");
        assert!(text.contains("\"service-uri\": \"https://github.com/o/r/services/s\",\n  \"tag\": \"v2\",\n  \"commit\""));
        assert_eq!(Entry::decode(&bytes), Ok(entry));
        let bad = text.replace("\"v2\"", "\"-v2\"");
        assert!(Entry::decode(bad.as_bytes()).is_err());
    }

    #[test]
    fn service_cache_entries_are_canonical_and_closed() {
        let entry = Entry {
            uri: "https://github.com/o/r/services/s@v1".into(),
            tag: None,
            commit: "a".repeat(40),
            manifest_sha256: "b".repeat(64),
            schema_sha256: None,
            component_sha256: Some("d".repeat(64)),
        };
        let bytes = entry.encode();
        let text = String::from_utf8(bytes.clone()).expect("UTF-8");
        assert!(text.contains("\"format\": \"stateforward.scxml.service-cache/v1\""));
        assert!(text.contains("\"service-uri\""));
        assert!(!text.contains("schema-sha256"));
        assert_eq!(Entry::decode(&bytes), Ok(entry));
        // A service entry carries no schema digest, and a namespace entry no
        // service URI.
        let with_schema = text.replace(
            "\"commit\"",
            &format!("\"schema-sha256\": \"{}\",\n  \"commit\"", "c".repeat(64)),
        );
        assert!(Entry::decode(with_schema.as_bytes()).is_err());
        let mixed = text.replace("service-cache/v1", "extension-cache/v1");
        assert!(Entry::decode(mixed.as_bytes()).is_err());
    }

    #[test]
    fn repository_service_uris_are_exact_and_never_namespaces() {
        let uri = RepositoryUri::parse_service(
            "https://github.com/stateforward/nemobot/services/memory@v0.1.0",
        )
        .expect("a repository service URI");
        assert_eq!(uri.kind, Kind::Service);
        assert_eq!(
            uri.tagless,
            "https://github.com/stateforward/nemobot/services/memory"
        );
        assert_eq!(uri.tag.as_deref(), Some("v0.1.0"));
        assert!(
            RepositoryUri::parse("https://github.com/stateforward/nemobot/services/memory@v0.1.0")
                .is_none(),
            "a services/ URI is never a repository namespace"
        );
        assert!(
            RepositoryUri::parse_service(
                "https://github.com/stateforward/scxml.rs/namespaces/example@v0.2.0"
            )
            .is_none(),
            "a namespaces/ URI is never a repository service"
        );
        assert_eq!(
            RepositoryUri::parse_service("https://github.com/o/r/services/memory")
                .map(|uri| uri.tag),
            Some(None),
            "a tagless repository service names the latest release"
        );
        for other in [
            "https://github.com/o/r/services/memory/@v1",
            "https://github.com/o/r/services@v1",
            "https://github.com/o/r/Services/memory@v1",
            "https://github.com/o/r/services/../x@v1",
            "https://github.com/o/r/services/memory/package@v1",
            "http://github.com/o/r/services/memory@v1",
            "https://github.com/o/r/services/memory@v1?x",
        ] {
            assert!(RepositoryUri::parse_service(other).is_none(), "{other}");
        }
    }

    fn pins_of(source: &str) -> Result<DocumentPins, String> {
        document_pins(source.as_bytes(), "the document")
    }

    #[test]
    fn documents_pin_repository_packages_once_each() {
        let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="https://github.com/o/r/namespaces/x@v1" xmlns:y="https://github.com/o/r/namespaces/y" xmlns:u="urn:a@b" xmlns:w="https://example.org/a@b/ns" version="1.0"><state id="s"><invoke type="https://github.com/o/r/services/b@v1"/><invoke type="https://github.com/o/r/services/a"/><invoke type="https://github.com/o/r/services/a@v2"/><invoke type="urn:x"/><invoke typeexpr="'https://github.com/o/r/services/c@v1'"/></state></scxml>"#;
        let pins = pins_of(source).expect("one pin per package");
        let entries = |map: &BTreeMap<String, Option<String>>| -> Vec<(String, Option<String>)> {
            map.iter()
                .map(|(uri, tag)| (uri.clone(), tag.clone()))
                .collect()
        };
        assert_eq!(
            entries(&pins.namespaces),
            [
                (
                    "https://github.com/o/r/namespaces/x".to_owned(),
                    Some("v1".to_owned())
                ),
                ("https://github.com/o/r/namespaces/y".to_owned(), None),
            ]
        );
        // A tagless use and a pinned one of one package in one document bind
        // the pinned release; a `typeexpr` is never resolved.
        assert_eq!(
            entries(&pins.services),
            [
                (
                    "https://github.com/o/r/services/a".to_owned(),
                    Some("v2".to_owned())
                ),
                (
                    "https://github.com/o/r/services/b".to_owned(),
                    Some("v1".to_owned())
                ),
            ]
        );
    }

    #[test]
    fn different_pins_of_one_package_in_one_document_fail_closed() {
        let namespaces = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:a="https://github.com/o/r/namespaces/x@v1" xmlns:b="https://github.com/o/r/namespaces/x@v2" version="1.0"/>"#;
        let error = pins_of(namespaces).expect_err("two releases of x");
        assert!(
            error.contains("is pinned to both `@v1` and `@v2`"),
            "{error}"
        );
        let services = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s"><invoke type="https://github.com/o/r/services/s@v1"/><invoke type="https://github.com/o/r/services/s@v2"/></state></scxml>"#;
        assert!(pins_of(services).is_err());
    }

    #[test]
    fn malformed_pins_and_pinned_built_ins_fail_closed() {
        for malformed in [
            "https://github.com/o/r/namespaces/x@",
            "https://github.com/o/r/namespaces/x@v1/extra",
            "https://github.com/o/r/namespaces/x@v1@v2",
        ] {
            let source = format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="{malformed}" version="1.0"/>"#
            );
            let error = pins_of(&source).expect_err(malformed);
            assert!(error.contains("malformed tag"), "{error}");
            let source = format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s"><invoke type="{}"/></state></scxml>"#,
                malformed.replace("namespaces", "services")
            );
            assert!(pins_of(&source).is_err(), "{malformed}");
        }
        for built_in in ["env", "event", "scx"] {
            let source = format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:b="https://github.com/stateforward/scxml.rs/namespaces/{built_in}@v1" version="1.0"/>"#
            );
            let error = pins_of(&source).expect_err(built_in);
            assert!(error.contains("pins a built-in vocabulary"), "{error}");
            // Tagless, a built-in is never a repository package.
            let source = format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:b="https://github.com/stateforward/scxml.rs/namespaces/{built_in}" version="1.0"/>"#
            );
            assert_eq!(pins_of(&source), Ok(DocumentPins::default()));
        }
    }

    #[test]
    fn the_run_binds_the_roots_resolution_and_a_child_pin_alongside_it() {
        let uri = "https://github.com/o/r/namespaces/x".to_owned();
        let uses = |tag: Option<&str>| BTreeMap::from([(uri.clone(), tag.map(str::to_owned))]);
        let none = BTreeMap::new();
        // The root pins vA; a tagless child shares the root's resolution.
        let plan_a = plan(&[uses(Some("vA")), uses(None)], &[]);
        assert_eq!(
            plan_a.run,
            BTreeMap::from([(uri.clone(), format!("{uri}@vA"))])
        );
        assert!(plan_a.pinned.is_empty());
        // The root is tagless; a child pinning vB binds vB for itself.
        let plan_b = plan(&[uses(None), uses(Some("vB"))], &[]);
        assert_eq!(plan_b.run, BTreeMap::from([(uri.clone(), uri.clone())]));
        assert_eq!(plan_b.pinned, [(1, uri.clone(), format!("{uri}@vB"))]);
        // The root pins vA and a child vB: both releases, each its own.
        let plan_c = plan(&[uses(Some("vA")), uses(Some("vB"))], &[]);
        assert_eq!(plan_c.run[&uri], format!("{uri}@vA"));
        assert_eq!(plan_c.pinned, [(1, uri.clone(), format!("{uri}@vB"))]);
        // The root does not use it: the first child's resolution is the
        // run's, and a later tagless child shares it.
        let plan_d = plan(&[none.clone(), uses(Some("vB")), uses(None)], &[]);
        assert_eq!(plan_d.run[&uri], format!("{uri}@vB"));
        assert!(plan_d.pinned.is_empty());
        // A flag-bound package is never resolved.
        assert_eq!(
            plan(&[uses(Some("vA"))], std::slice::from_ref(&uri)),
            Plan::default()
        );
    }

    #[test]
    fn a_release_manifest_names_the_tagless_uri() {
        let uri = "https://github.com/o/r/namespaces/x";
        assert_eq!(check_identity(uri, uri, "namespace-uri"), Ok(()));
        assert_eq!(
            check_identity(&format!("{uri}@v1"), uri, "namespace-uri"),
            Ok(()),
            "a pinned use resolves the same tagless identity"
        );
        let error = check_identity(uri, &format!("{uri}@v1"), "namespace-uri")
            .expect_err("a manifest naming a pinned URI");
        assert!(error.contains("names the pinned URI"), "{error}");
        assert!(error.contains(&format!("tagless URI `{uri}`")), "{error}");
        let error = check_identity(uri, "https://github.com/o/r/namespaces/y", "namespace-uri")
            .expect_err("another package");
        assert!(
            error.contains("not `https://github.com/o/r/namespaces/x`"),
            "{error}"
        );
    }

    #[test]
    fn sha256sums_name_one_digest_per_asset() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let sums = format!("{a}  x.wasm\n{b} *y.wasm\n");
        assert_eq!(sums_entry(sums.as_bytes(), "x.wasm"), Some(a.clone()));
        assert_eq!(sums_entry(sums.as_bytes(), "y.wasm"), Some(b.clone()));
        assert_eq!(sums_entry(sums.as_bytes(), "z.wasm"), None);
        let twice = format!("{a}  x.wasm\n{b}  x.wasm\n");
        assert_eq!(sums_entry(twice.as_bytes(), "x.wasm"), None);
        assert_eq!(sums_entry(b"zz  x.wasm\n", "x.wasm"), None);
    }

    #[test]
    fn bases_admit_only_https_or_loopback_http() {
        assert!(parse_base("https://api.github.com").is_some());
        assert!(parse_base("http://127.0.0.1:8080/api").is_some());
        assert!(parse_base("http://[::1]:8080").is_some());
        assert!(parse_base("http://localhost:1").is_some());
        for refused in [
            "http://example.com",
            "ftp://127.0.0.1",
            "https://user@api.github.com",
            "https://api.github.com/?x",
            "https://api.github.com/#x",
            "relative",
        ] {
            assert!(parse_base(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn schemas_name_their_target_namespace() {
        let schema =
            br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:x"/>"#;
        assert_eq!(schema_target_namespace(schema).as_deref(), Some("urn:x"));
        assert_eq!(
            schema_target_namespace(br#"<schema targetNamespace="urn:x"/>"#),
            None
        );
    }
}
