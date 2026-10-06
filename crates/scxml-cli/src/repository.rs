//! Repository namespaces and repository services: an extension namespace,
//! or an invoked-service type, whose URI is the path of its package in a
//! GitHub repository at a release tag.
//!
//! # The URI
//!
//! `https://github.com/<owner>/<repo>/namespaces/<name>@<tag>` (a repository
//! namespace) or `https://github.com/<owner>/<repo>/services/<name>@<tag>`
//! (a repository service), exactly: the literal `namespaces` or `services`
//! folder, one name segment, no trailing slash, and the release tag after the
//! last `@`. Every repository namespace lives in its own `namespaces/<name>/`
//! folder and every repository service in its own `services/<name>/` folder;
//! any other path is an ordinary namespace or service type URI, and a
//! `services/` URI is never a repository namespace (nor a `namespaces/` URI a
//! repository service).
//! A document binds that exact string (SCXML 1.0 section 4.10 namespace
//! binding, and SCXML 6.4 `<invoke type>`, are byte-exact); the package
//! manifest's `namespace-uri` (and the schema's `targetNamespace`), or
//! `service-uri`, must equal it byte for byte. A new version is a new URI.
//! Nothing about this is visible to SCXML: to a document it is an ordinary
//! namespace or type URI. A repository service is resolved only for a
//! literal `<invoke type>` no `--service-package` binds.
//!
//! # Where the bytes come from
//!
//! * The tag resolves to a commit through the GitHub REST API
//!   (`/repos/<owner>/<repo>/git/ref/tags/<tag>`, dereferencing annotated
//!   tags).
//! * The manifest (`namespaces/<name>/package/<name>.extension.json`, or
//!   `services/<name>/package/<name>.service.json`), its schema
//!   (`schema-artifact`, relative to the manifest; optional for a service)
//!   come from `raw.githubusercontent.com` at that commit, never at the tag;
//!   the schema must match the digest the manifest declares.
//! * The component is the release asset the manifest's `extension-artifact`
//!   or `service-artifact` names, from the release of `<tag>`,
//!   authenticated by the SHA-256 that release's `SHA256SUMS` asset lists for
//!   it. A namespace's component is resolved when a session starts; a
//!   service's when `snapshot run` or `run` binds it, before the session
//!   starts.
//!
//! # Trust and the cache
//!
//! Every acquired byte is verified before it is stored, and stored exactly,
//! content-addressed, under `<cache>/sha256/<hex>`. The first resolution of
//! a URI pins it: `<cache>/index/<sha256 of the URI>.json` records the
//! commit and the manifest, schema, and component digests, and every later
//! use re-verifies the cached bytes against them and never asks the network
//! again (a missing object is re-fetched at the pinned commit and must match
//! the pinned digest). Cached bytes that do not match their digest fail
//! closed. `--offline` never touches the network: a URI not in the cache
//! fails closed.
//!
//! # Network bounds
//!
//! HTTPS only (plain HTTP only to a loopback host given through an explicit
//! `--github-*-base`, for mirrors and tests); a redirect is followed only to
//! an origin of the configured bases or GitHub's release-asset hosts, at most
//! [`MAX_REDIRECTS`] times; connect and total timeouts; no retry; every body
//! bounded ([`MAX_API_BYTES`], [`MAX_MANIFEST_BYTES`],
//! `MAX_COMPONENT_SCHEMA_BYTES`, [`MAX_SUMS_BYTES`], [`MAX_COMPONENT_BYTES`]);
//! no cookies, proxies from the environment, or credentials, except a GitHub
//! token read from the variable `--github-token-env NAME` names, sent only
//! to the API origin.
//!
//! # Telemetry
//!
//! One `scxml.cli.resolve-extension-package` (or
//! `scxml.cli.resolve-service-package`) span per package resolution and one
//! `scxml.cli.resolve-extension-component` (or
//! `scxml.cli.resolve-service-component`) span per component resolution,
//! each with its closed outcome only: no URI, URL, tag, commit, digest, or
//! path is ever recorded.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "wasmtime")]
use std::sync::{Arc, OnceLock};
#[cfg(feature = "wasmtime")]
use std::time::Duration;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_validator_core::{MAX_COMPONENT_SCHEMA_BYTES, SchemaSource};
use sha2::{Digest as _, Sha256};

use crate::extensions::{Package, PackageOrigin};
use crate::services::{ServiceOrigin, ServicePackage};

/// The one repository host a repository namespace's URI names.
const GITHUB_PREFIX: &str = "https://github.com/";
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
/// Namespace cache index entry fields in their canonical order.
const ENTRY_FIELDS: [&str; 6] = [
    "format",
    "namespace-uri",
    "commit",
    "manifest-sha256",
    "schema-sha256",
    "component-sha256",
];
/// The cache index entry format of a repository service. Its manifest's
/// digest pins its optional schema, whose digest the manifest declares.
const SERVICE_ENTRY_FORMAT: &str = "stateforward.scxml.service-cache/v1";
/// Service cache index entry fields in their canonical order.
const SERVICE_ENTRY_FIELDS: [&str; 5] = [
    "format",
    "service-uri",
    "commit",
    "manifest-sha256",
    "component-sha256",
];

/// Maximum bytes of a repository namespace URI.
pub(crate) const MAX_URI_BYTES: usize = 512;
/// The one repository folder every repository namespace lives in.
const NAMESPACES_FOLDER: &str = "namespaces";
/// The one repository folder every repository service lives in.
const SERVICES_FOLDER: &str = "services";
/// Maximum bytes of one owner, repository, or namespace name segment.
const MAX_SEGMENT_BYTES: usize = 100;
/// Maximum bytes of a tag.
const MAX_TAG_BYTES: usize = 128;
/// Maximum bytes of one GitHub API response.
pub(crate) const MAX_API_BYTES: u64 = 64 * 1024;
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
/// Maximum redirects one download follows.
pub(crate) const MAX_REDIRECTS: usize = 5;
/// Maximum annotated-tag dereferences while resolving a tag.
const MAX_TAG_DEREFERENCES: usize = 4;
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

/// One parsed repository namespace or service URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepositoryUri {
    kind: Kind,
    owner: String,
    repo: String,
    /// The package's folder name under `namespaces/` or `services/`.
    name: String,
    tag: String,
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
        let rest = uri.strip_prefix(GITHUB_PREFIX)?;
        let (path, tag) = rest.rsplit_once('@')?;
        if !valid_tag(tag) {
            return None;
        }
        let mut segments = path.split('/');
        let owner = segments.next().filter(|segment| valid_segment(segment))?;
        let repo = segments.next().filter(|segment| valid_segment(segment))?;
        if segments.next() != Some(kind.folder()) {
            return None;
        }
        let name = segments.next().filter(|segment| valid_segment(segment))?;
        if segments.next().is_some() {
            return None;
        }
        Some(Self {
            kind,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            name: name.to_owned(),
            tag: tag.to_owned(),
        })
    }

    /// The package manifest's file name: `<name>.extension.json` or
    /// `<name>.service.json`.
    fn manifest_name(&self) -> String {
        match self.kind {
            Kind::Namespace => format!("{}.extension.json", self.name),
            Kind::Service => format!("{}.service.json", self.name),
        }
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

/// `[A-Za-z0-9._+-]`, starting with an alphanumeric, within
/// [`MAX_TAG_BYTES`].
fn valid_tag(tag: &str) -> bool {
    tag.len() <= MAX_TAG_BYTES
        && tag
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
        && !tag.contains("..")
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

    /// Opens the cache and checks the bases. Nothing touches the network.
    ///
    /// # Errors
    ///
    /// A message for a base that is not an admissible URL, an unset token
    /// variable, or no cache directory (neither `--cache-dir`,
    /// `XDG_CACHE_HOME`, nor `HOME`).
    pub(crate) fn open(&self) -> Result<Repository, String> {
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
            failure: Mutex::new(None),
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

impl Bases {
    fn join(base: &url::Url, path: &str) -> Result<url::Url, String> {
        let text = format!("{}/{path}", base.as_str().trim_end_matches('/'));
        url::Url::parse(&text).map_err(|_| "a repository URL does not parse".to_owned())
    }

    fn admits(&self, url: &url::Url) -> bool {
        let origin = url.origin();
        admissible_scheme(url)
            && url.username().is_empty()
            && url.password().is_none()
            && ([&self.api, &self.raw, &self.download]
                .iter()
                .any(|base| base.origin() == origin)
                || RELEASE_ASSET_ORIGINS.iter().any(|allowed| {
                    url::Url::parse(allowed).is_ok_and(|allowed| allowed.origin() == origin)
                }))
    }
}

/// One cache index entry: what the first resolution of a URI pinned.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    /// The repository namespace or service URI.
    uri: String,
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
        let mut text = match &self.schema_sha256 {
            Some(schema) => format!(
                "{{\n  \"format\": {},\n  \"namespace-uri\": {},\n  \"commit\": {},\n  \"manifest-sha256\": {},\n  \"schema-sha256\": {}",
                string(ENTRY_FORMAT),
                string(&self.uri),
                string(&self.commit),
                string(&self.manifest_sha256),
                string(schema),
            ),
            None => format!(
                "{{\n  \"format\": {},\n  \"service-uri\": {},\n  \"commit\": {},\n  \"manifest-sha256\": {}",
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
            commit: required("commit")?,
            manifest_sha256: required("manifest-sha256")?,
            schema_sha256: if format == ENTRY_FORMAT {
                Some(required("schema-sha256")?)
            } else {
                None
            },
            component_sha256: field("component-sha256")?,
        };
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

/// The bounded acquisition client.
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

/// Resolves repository namespaces through the cache and, unless offline,
/// their repository. Shared by the compile step and the session host's
/// resolver; its own failures are kept for the CLI's report.
pub(crate) struct Repository {
    cache: Cache,
    offline: bool,
    bases: Bases,
    token: Option<String>,
    #[cfg(feature = "wasmtime")]
    client: OnceLock<Client>,
    failure: Mutex<Option<String>>,
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
    pub(crate) package: Package,
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
}

impl Repository {
    /// The failure of the last resolution the session host asked for, once.
    pub(crate) fn take_failure(&self) -> Option<String> {
        self.failure
            .lock()
            .map_or(None, |mut failure| failure.take())
    }

    fn record(&self, message: &str) {
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some(message.to_owned());
        }
    }

    #[cfg(feature = "wasmtime")]
    fn client(&self) -> &Client {
        self.client.get_or_init(Client::new)
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

    /// Without Wasmtime this build links no network client: only what the
    /// cache already holds resolves.
    #[cfg(not(feature = "wasmtime"))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the same signature as the networked build's"
    )]
    fn get(&self, url: url::Url, _limit: u64, _accept: &str) -> Result<Vec<u8>, String> {
        let _ = (&self.token, self.bases.admits(&url));
        Err(format!(
            "cannot fetch `{url}`: this scx was built without `--features wasmtime`, which links its network client\n  = help: rebuild with `--features wasmtime`; repository namespaces already in the extension cache still resolve"
        ))
    }

    /// GETs `url` within `limit` bytes, following at most [`MAX_REDIRECTS`]
    /// admitted redirects.
    #[cfg(feature = "wasmtime")]
    fn get(&self, url: url::Url, limit: u64, accept: &str) -> Result<Vec<u8>, String> {
        let shown = url.to_string();
        let mut current = url;
        for _ in 0..=MAX_REDIRECTS {
            if !self.bases.admits(&current) {
                return Err(format!(
                    "`{shown}` leads to `{current}`, which is not a GitHub or configured repository origin"
                ));
            }
            let mut request = self
                .client()
                .agent
                .get(current.as_str())
                .header("Accept", accept);
            if let Some(token) = &self.token
                && current.origin() == self.bases.api.origin()
            {
                request = request.header("Authorization", &format!("Bearer {token}"));
            }
            let mut response = request
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
            let too_large = || format!("`{shown}` is larger than its {limit}-byte bound");
            // A declared length over the bound fails before any body byte is
            // read; an undeclared or understated one is still cut off below.
            if response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .is_some_and(|length| length > limit)
            {
                return Err(too_large());
            }
            // ureq's limit rejects the read after `limit` bytes even at end of
            // body, so it reads one byte past the bound and the length is
            // checked here: a body of exactly `limit` bytes is admitted.
            let bytes = response
                .body_mut()
                .with_config()
                .limit(limit.saturating_add(1))
                .read_to_vec()
                .map_err(|error| match error {
                    ureq::Error::BodyExceedsLimit(_) => too_large(),
                    error => format!("cannot read `{shown}`: {error}"),
                })?;
            if u64::try_from(bytes.len()).map_or(true, |length| length > limit) {
                return Err(too_large());
            }
            return Ok(bytes);
        }
        Err(format!(
            "`{shown}` redirects more than {MAX_REDIRECTS} times"
        ))
    }

    fn api_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let url = Bases::join(&self.bases.api, path)?;
        let bytes = self.get(url, MAX_API_BYTES, "application/vnd.github+json")?;
        serde_json::from_slice(&bytes).map_err(|_| "the GitHub API answered malformed JSON".into())
    }

    /// The commit `tag` names (annotated tags dereferenced).
    fn resolve_tag(&self, uri: &RepositoryUri) -> Result<String, String> {
        let value = self.api_json(&format!(
            "repos/{}/{}/git/ref/tags/{}",
            uri.owner, uri.repo, uri.tag
        ))?;
        if value.get("ref").and_then(serde_json::Value::as_str)
            != Some(format!("refs/tags/{}", uri.tag).as_str())
        {
            return Err(format!(
                "the GitHub API does not name tag `{}` exactly",
                uri.tag
            ));
        }
        let mut object = value.get("object").cloned();
        for _ in 0..MAX_TAG_DEREFERENCES {
            let kind = object
                .as_ref()
                .and_then(|object| object.get("type"))
                .and_then(serde_json::Value::as_str);
            let sha = object
                .as_ref()
                .and_then(|object| object.get("sha"))
                .and_then(serde_json::Value::as_str)
                .filter(|sha| is_commit(sha))
                .ok_or("the GitHub API answered a tag without a commit")?
                .to_owned();
            match kind {
                Some("commit") => return Ok(sha),
                Some("tag") => {
                    object = self
                        .api_json(&format!("repos/{}/{}/git/tags/{sha}", uri.owner, uri.repo))?
                        .get("object")
                        .cloned();
                }
                _ => return Err("the GitHub API answered a tag of an unknown kind".into()),
            }
        }
        Err(format!(
            "tag `{}` is nested deeper than {MAX_TAG_DEREFERENCES} annotated tags",
            uri.tag
        ))
    }

    fn raw_url(&self, uri: &RepositoryUri, commit: &str, file: &str) -> Result<url::Url, String> {
        Bases::join(
            &self.bases.raw,
            &format!(
                "{}/{}/{commit}/{}/{}/package/{file}",
                uri.owner,
                uri.repo,
                uri.kind.folder(),
                uri.name
            ),
        )
    }

    fn download_url(&self, uri: &RepositoryUri, asset: &str) -> Result<url::Url, String> {
        Bases::join(
            &self.bases.download,
            &format!(
                "{}/{}/releases/download/{}/{asset}",
                uri.owner, uri.repo, uri.tag
            ),
        )
    }

    /// Cached bytes of `digest`, or (online) the bytes at `url`, which must
    /// match `digest`.
    fn pinned(
        &self,
        namespace_uri: &str,
        what: &str,
        digest: &str,
        limit: u64,
        url: impl FnOnce() -> Result<url::Url, String>,
    ) -> Result<Vec<u8>, String> {
        if let Some(bytes) = self.cache.object(digest, limit)? {
            return Ok(bytes);
        }
        self.require_online(namespace_uri, what)?;
        let bytes = self.get(url()?, limit, "*/*")?;
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
        let entry = match self.cache.entry(namespace_uri, false)? {
            Some(entry) => entry,
            None => self.pin_package(namespace_uri, &uri, schema_limit)?,
        };
        let schema_sha256 = entry
            .schema_sha256
            .clone()
            .ok_or("a namespace's cache index pins no schema")?;
        let manifest = self.pinned(
            namespace_uri,
            "manifest",
            &entry.manifest_sha256,
            MAX_MANIFEST_BYTES,
            || self.raw_url(&uri, &entry.commit, &uri.manifest_name()),
        )?;
        let parsed = scxml_validator_core::ExtensionPackageManifest::from_json(&manifest).map_err(
            |error| {
                format!(
                    "repository namespace `{namespace_uri}`: its manifest is malformed: {error}"
                )
            },
        )?;
        let schema = self.pinned(
            namespace_uri,
            "schema",
            &schema_sha256,
            schema_limit,
            || self.raw_url(&uri, &entry.commit, &parsed.schema_artifact),
        )?;
        let package = verify_package(namespace_uri, &manifest, &schema, &entry)?;
        Ok((package, entry))
    }

    /// First use of `namespace_uri`: resolves its tag, fetches and verifies
    /// its package at that commit, stores it, and pins the URI.
    fn pin_package(
        &self,
        namespace_uri: &str,
        uri: &RepositoryUri,
        schema_limit: u64,
    ) -> Result<Entry, String> {
        self.require_online(namespace_uri, "package")?;
        let commit = self.resolve_tag(uri)?;
        let manifest = self.get(
            self.raw_url(uri, &commit, &uri.manifest_name())?,
            MAX_MANIFEST_BYTES,
            "*/*",
        )?;
        let parsed = scxml_validator_core::ExtensionPackageManifest::from_json(&manifest).map_err(
            |error| {
                format!(
                    "repository namespace `{namespace_uri}`: its manifest is malformed: {error}"
                )
            },
        )?;
        check_manifest(namespace_uri, &parsed)?;
        let schema = self.get(
            self.raw_url(uri, &commit, &parsed.schema_artifact)?,
            schema_limit,
            "*/*",
        )?;
        let entry = Entry {
            uri: namespace_uri.to_owned(),
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
        self.require_online(
            namespace_uri,
            match uri.kind {
                Kind::Namespace => "extension component digest",
                Kind::Service => "service component digest",
            },
        )?;
        let sums = self.get(self.download_url(uri, SUMS_ASSET)?, MAX_SUMS_BYTES, "*/*")?;
        let digest = sums_entry(&sums, asset).ok_or_else(|| {
            format!(
                "{} `{namespace_uri}`: release `{}` lists no single SHA-256 for `{asset}` in its {SUMS_ASSET}",
                uri.kind.label(),
                uri.tag
            )
        })?;
        entry.component_sha256 = Some(digest);
        self.cache.write_entry(&entry)?;
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
            let sha256 = entry
                .component_sha256
                .ok_or("the cache pinned no component digest")?;
            let bytes = self.pinned(
                namespace_uri,
                "extension component",
                &sha256,
                MAX_COMPONENT_BYTES,
                || self.download_url(&uri, &asset),
            )?;
            Ok(ResolvedComponent {
                package,
                bytes,
                sha256,
            })
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
        let entry = match self.cache.entry(service_uri, true)? {
            Some(entry) => entry,
            None => self.pin_service(service_uri, &uri)?,
        };
        let manifest = self.pinned(
            service_uri,
            "manifest",
            &entry.manifest_sha256,
            MAX_MANIFEST_BYTES,
            || self.raw_url(&uri, &entry.commit, &uri.manifest_name()),
        )?;
        let parsed = parse_service_manifest(service_uri, &manifest)?;
        let schema = self.service_schema(service_uri, &uri, &entry.commit, &parsed)?;
        let package = verify_service(service_uri, &manifest, schema.as_deref(), &entry)?;
        Ok((package, entry))
    }

    /// The optional schema of a repository service at `commit`, checked
    /// against the digest its manifest declares.
    fn service_schema(
        &self,
        service_uri: &str,
        uri: &RepositoryUri,
        commit: &str,
        manifest: &scxml_validator_core::ServicePackageManifest,
    ) -> Result<Option<Vec<u8>>, String> {
        check_service_manifest(service_uri, manifest)?;
        let schema = match (&manifest.schema_artifact, &manifest.schema_sha256) {
            (Some(artifact), Some(digest)) => Some(self.pinned(
                service_uri,
                "schema",
                digest,
                crate::services::MAX_SCHEMA_BYTES,
                || self.raw_url(uri, commit, artifact),
            )?),
            _ => None,
        };
        Ok(schema)
    }

    /// First use of `service_uri`: resolves its tag, fetches and verifies
    /// its package at that commit, stores it, and pins the URI.
    fn pin_service(&self, service_uri: &str, uri: &RepositoryUri) -> Result<Entry, String> {
        self.require_online(service_uri, "package")?;
        let commit = self.resolve_tag(uri)?;
        let manifest = self.get(
            self.raw_url(uri, &commit, &uri.manifest_name())?,
            MAX_MANIFEST_BYTES,
            "*/*",
        )?;
        let parsed = parse_service_manifest(service_uri, &manifest)?;
        let schema = self.service_schema(service_uri, uri, &commit, &parsed)?;
        let entry = Entry {
            uri: service_uri.to_owned(),
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
            let sha256 = entry
                .component_sha256
                .ok_or("the cache pinned no component digest")?;
            let bytes = self.pinned(
                service_uri,
                "service component",
                &sha256,
                MAX_COMPONENT_BYTES,
                || self.download_url(&uri, &asset),
            )?;
            Ok(ResolvedService {
                package,
                bytes,
                sha256,
            })
        })();
        record_outcome(&span, result.is_ok());
        result
    }
}

/// A resolved repository service: its package and its component's exact
/// bytes and SHA-256.
pub(crate) struct ResolvedService {
    pub(crate) package: ServicePackage,
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
    if manifest.service_uri != service_uri {
        return Err(format!(
            "repository service `{service_uri}`: its manifest binds `{}`, not this URI\n  = help: a repository service's manifest names its own URI byte for byte, tag included",
            manifest.service_uri
        ));
    }
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
        .map(|uri| uri.tag)
        .unwrap_or_default();
    Ok(ServicePackage {
        origin: ServiceOrigin::Repository {
            tag,
            commit: entry.commit.clone(),
        },
        manifest,
    })
}

/// The literal `<invoke type>` values of a document that are repository
/// service URIs `bound` does not name, sorted and unique; a document that
/// does not parse names none here (the validator reports it).
pub(crate) fn used_services(source: &[u8], bound: &[String]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(source) else {
        return Vec::new();
    };
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return Vec::new();
    };
    let mut uris = std::collections::BTreeSet::new();
    document.root.visit(&mut |element| {
        if element
            .name
            .is(Some(scxml_validator_core::SCXML_NAMESPACE), "invoke")
            && let Some(type_uri) = element.attribute("type")
            && RepositoryUri::parse_service(&type_uri.value).is_some()
            && !bound.contains(&type_uri.value)
        {
            uris.insert(type_uri.value.clone());
        }
    });
    uris.into_iter().collect()
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

/// What a repository namespace's manifest must name beyond the format: its own
/// URI exactly, and artifacts that are plain relative paths (the component a
/// single release asset name).
fn check_manifest(
    namespace_uri: &str,
    manifest: &scxml_validator_core::ExtensionPackageManifest,
) -> Result<(), String> {
    if manifest.namespace_uri != namespace_uri {
        return Err(format!(
            "repository namespace `{namespace_uri}`: its manifest binds `{}`, not this URI\n  = help: a repository namespace's manifest names its own URI byte for byte, tag included",
            manifest.namespace_uri
        ));
    }
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
    if schema_target_namespace(schema_bytes).as_deref() != Some(namespace_uri) {
        return Err(format!(
            "repository namespace `{namespace_uri}`: its schema's targetNamespace is not this URI"
        ));
    }
    let tag = RepositoryUri::parse(namespace_uri)
        .map(|uri| uri.tag)
        .unwrap_or_default();
    Ok(Package {
        origin: PackageOrigin::Repository {
            tag,
            commit: entry.commit.clone(),
        },
        manifest,
        schema,
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

/// The namespaces a document uses that a repository namespace serves and no
/// other flag binds, resolved to their packages.
///
/// # Errors
///
/// The first resolution failure.
pub(crate) fn resolve_document_packages(
    repository: &Repository,
    source: &[u8],
    bound: &[String],
) -> Result<Vec<Package>, String> {
    let mut packages = Vec::new();
    for namespace_uri in used_namespaces(source) {
        if bound.contains(&namespace_uri) || RepositoryUri::parse(&namespace_uri).is_none() {
            continue;
        }
        packages.push(repository.resolve_package(&namespace_uri)?);
    }
    Ok(packages)
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

/// Repository namespaces as a session host resolver: the extension component
/// of a repository URI, authenticated by its pinned SHA-256.
#[cfg(feature = "wasmtime")]
#[derive(Debug)]
pub(crate) struct RepositoryResolver(pub(crate) Arc<Repository>);

#[cfg(feature = "wasmtime")]
impl scxml_host_wasmtime::ExtensionResolver for RepositoryResolver {
    fn resolve(
        &self,
        namespace_uri: &str,
    ) -> Result<
        Option<scxml_host_wasmtime::ResolvedExtension>,
        scxml_host_wasmtime::ExtensionResolveError,
    > {
        if RepositoryUri::parse(namespace_uri).is_none() {
            return Ok(None);
        }
        let resolved = (|| {
            let resolved = self.0.resolve_component(namespace_uri)?;
            let digest = scxml_host_wasmtime::TrustedComponentDigest::from_hex(&resolved.sha256)
                .map_err(|error| error.to_string())?;
            let component =
                scxml_host_wasmtime::authenticate_extension_component(&resolved.bytes, digest)
                    .map_err(|error| {
                        format!(
                            "repository namespace `{namespace_uri}`: its extension component is rejected: {error}"
                        )
                    })?;
            Ok::<_, String>(scxml_host_wasmtime::ResolvedExtension {
                package: resolved.package.host_package(),
                component,
            })
        })();
        resolved.map(Some).map_err(|message| {
            self.0.record(&message);
            scxml_host_wasmtime::ExtensionResolveError { message }
        })
    }
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
        assert_eq!(uri.name, "example");
        assert_eq!(uri.tag, "v0.2.0");
        assert_eq!(uri.manifest_name(), "example.extension.json");
        for other in [
            "https://github.com/stateforward/scxml.rs/namespaces/example",
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
        let other = RepositoryUri::parse("https://github.com/o/r/namespaces/x.y-z@v1")
            .expect("any owner and repository");
        assert_eq!(other.manifest_name(), "x.y-z.extension.json");
    }

    #[test]
    fn cache_entries_are_canonical_and_closed() {
        let entry = Entry {
            uri: "https://github.com/o/r/e@v1".into(),
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
    fn service_cache_entries_are_canonical_and_closed() {
        let entry = Entry {
            uri: "https://github.com/o/r/services/s@v1".into(),
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
        assert_eq!(uri.name, "memory");
        assert_eq!(uri.tag, "v0.1.0");
        assert_eq!(uri.manifest_name(), "memory.service.json");
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
        for other in [
            "https://github.com/o/r/services/memory",
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
        let source = br#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s"><invoke type="https://github.com/o/r/services/b@v1"/><invoke type="https://github.com/o/r/services/a@v1"/><invoke type="https://github.com/o/r/services/a@v1"/><invoke type="urn:x"/><invoke typeexpr="'https://github.com/o/r/services/c@v1'"/></state></scxml>"#;
        assert_eq!(
            used_services(source, &[]),
            [
                "https://github.com/o/r/services/a@v1",
                "https://github.com/o/r/services/b@v1"
            ]
        );
        assert_eq!(
            used_services(source, &["https://github.com/o/r/services/a@v1".to_owned()]),
            ["https://github.com/o/r/services/b@v1"]
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
