//! The documents a run loads by URI: `<invoke src>` (SCXML 6.4),
//! `<script src>` (5.8), and `<data src>` (5.3), and the `srcexpr` URIs an
//! `<invoke>` evaluates when it runs.
//!
//! # Which URIs load
//!
//! An `https:` URI, a `file:` URI, or a relative URI, resolved against the
//! element's in-scope `xml:base` (XML Base; [`scxml_types::resolve_reference`])
//! and then the base URI of the document that names it (RFC 3986 §5; the root
//! document's base is its `file:` URL, an `<invoke src>` child's the URL it
//! was loaded from). Any other scheme does not load. Remote documents come
//! through the run's [`Repository`] with the repository resolver's bounds
//! and fail-closed rules ([`Repository::fetch_document`]): HTTPS only,
//! bounded size, redirects, and time, no retry, no credentials, and nothing
//! at all under `--offline`.
//!
//! # When
//!
//! Every literal URI of the root document, and of every `<invoke src>`
//! document it loads (recursively, at most [`MAX_DOCUMENTS`] documents), is
//! fetched once, before the first macrostep, and listed in the consent
//! output ([`Documents::summary`]). A root `<script src>` that cannot be
//! fetched rejects the document (SCXML 5.8: "the document is considered
//! non-conformant, and the platform MUST reject it"). Any other literal URI
//! that cannot be fetched is not an error yet: its `<invoke>` or `<data>`
//! raises `error.execution` when it runs, as SCXML defines. A `srcexpr` URI
//! is fetched each time its `<invoke>` runs, through the same fetcher but
//! never from its memo, so a document written during the run loads as it is
//! then; a failure is `error.execution`. A relative one is resolved against the base URI of
//! the document whose session evaluates it: the host resolves it against
//! an `<invoke src>` document's own URL
//! ([`scxml_host_wasmtime::EmbeddedDocument::base_uri`]), so only the root
//! document's sessions (and their inline `<content>` sessions) pass one
//! here relative, resolved against the root's base.
//! Every literal URI is fetched once per root session and shared by every
//! session of the tree.
//!
//! An `<invoke src>` document carries the schema files its `event:schema`
//! attributes reference, resolved against its own base URI (read from its
//! directory for a `file:` one, exactly as the root's are, and fetched for
//! an `https:` one; [`crate::event_schemas`]), to its compile
//! ([`scxml_host_wasmtime::EmbeddedDocument::event_schemas`]): a literal
//! child's before the session, a `srcexpr` child's with the document. Its
//! `<script src>` files resolve against the same base URI: a literal
//! child's are fetched before the session with the root's, a `srcexpr`
//! child's with the document, and each session is served the scripts its
//! own document names ([`scxml_host_wasmtime::SessionEmbedder::scripts`]).
//!
//! The documents an `<invoke src>` loads are part of the run: the
//! repository namespaces and services they use are resolved with the
//! root's, before the session starts ([`Documents::children`]). A child
//! document that pins a release other than the run's carries the
//! components of that release to the host with its text, for its own
//! sessions ([`Documents::bind`]). A `srcexpr` document is not known before
//! the session, so its pins are never resolved: one that pins a release
//! (`@<tag>`) fails to load (`error.execution`) rather than run with
//! another.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use scxml_compiler_core::ScriptPackage;
use scxml_validator_core::InvokedDocument;
use sha2::{Digest as _, Sha256};

use crate::repository::{Fetched, Repository};

/// Maximum documents one run loads before its first macrostep: the root
/// and every `<invoke src>` document reached from it. A work bound on
/// every build (a chain of documents could otherwise fetch without end).
pub(crate) const MAX_DOCUMENTS: usize = 64;

/// The element that names one URI.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Site {
    Invoke,
    Script,
    Data,
}

impl Site {
    const fn element(self) -> &'static str {
        match self {
            Self::Invoke => "invoke",
            Self::Script => "script",
            Self::Data => "data",
        }
    }
}

/// One literal URI a document of the run names.
#[derive(Clone, Debug)]
struct Literal {
    site: Site,
    /// The `src` value as the engine reads it (XML white space collapsed).
    authored: String,
    /// The URL it resolves to against its document's base, if it resolves
    /// to an `https:` or `file:` URL.
    url: Option<url::Url>,
}

/// The documents of one run.
pub(crate) struct Documents {
    repository: Arc<Repository>,
    /// The root document's base URI.
    base: url::Url,
    /// Every literal URI, in discovery order.
    literals: Vec<Literal>,
    /// The URL and bytes of every `<invoke src>` document loaded before the
    /// session.
    children: Vec<(url::Url, Vec<u8>)>,
    /// The components each child document that pins another release binds
    /// for its own sessions, by URL.
    #[cfg(feature = "wasmtime")]
    bindings: BTreeMap<
        String,
        (
            Option<scxml_host_wasmtime::ExtensionComponents>,
            Option<scxml_host_wasmtime::ServiceComponents>,
        ),
    >,
    /// The schema files each `file:` child document's `event:schema`
    /// attributes reference, read relative to it before the session
    /// ([`crate::event_schemas::load`]), by URL.
    event_schemas: BTreeMap<String, scxml_event_schema::Resources>,
    /// The documents the root's literal `<invoke src>` elements load, for
    /// the validator (`EVENT011`).
    #[cfg(feature = "wasmtime")]
    invoked: BTreeMap<String, InvokedDocument>,
    /// The `<script src>` files of the `srcexpr` documents loaded so far, as
    /// they were when each was loaded last, by URL.
    #[cfg(feature = "wasmtime")]
    srcexpr_scripts: std::sync::Mutex<BTreeMap<String, Fetched>>,
}

impl std::fmt::Debug for Documents {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Documents")
            .field("literals", &self.literals.len())
            .finish_non_exhaustive()
    }
}

/// The base URI of the document at `path`: its absolute `file:` URL.
fn file_base(path: &Path) -> Result<url::Url, String> {
    let absolute = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve `{}`: {error}", path.display()))?;
    url::Url::from_file_path(&absolute)
        .map_err(|()| format!("`{}` has no file: URL", absolute.display()))
}

/// `reference` resolved against `base`, when the result is an `https:` or
/// `file:` URL.
fn resolve(base: &url::Url, reference: &str) -> Option<url::Url> {
    let url = base.join(reference).ok()?;
    matches!(url.scheme(), "https" | "file").then_some(url)
}

/// The literal `src` URIs of one document, in document order.
fn literals_of(base: &url::Url, text: &str) -> Vec<Literal> {
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return Vec::new();
    };
    let mut literals = Vec::new();
    document.root.visit_with_base(&mut |element, xml_base| {
        let site = [Site::Invoke, Site::Script, Site::Data]
            .into_iter()
            .find(|site| {
                element
                    .name
                    .is(Some(scxml_validator_core::SCXML_NAMESPACE), site.element())
            });
        if let Some(site) = site
            && let Some(src) = element.attribute("src")
        {
            // The URI as the engine passes it: resolved against the
            // element's in-scope XML Base, as lowering resolves a `<data>`
            // or `<script>` `src` and the machine an `<invoke>`'s.
            let src = scxml_syntax::collapse_whitespace(&src.value);
            let authored = match xml_base {
                Some(base) => scxml_types::resolve_reference(base, &src),
                None => src.into_owned(),
            };
            literals.push(Literal {
                site,
                url: resolve(base, &authored),
                authored,
            });
        }
    });
    literals
}

/// The documents the literal `<invoke src>` elements of `text` (a document
/// whose base URI is `base`) load, keyed by `src` as authored with its XML
/// white space collapsed: each `src` resolved like an `<invoke src>` the run
/// loads, and `load` reads the document at that URL.
fn invoked_documents(
    base: &url::Url,
    text: &str,
    mut load: impl FnMut(&url::Url) -> Option<InvokedDocument>,
) -> BTreeMap<String, InvokedDocument> {
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return BTreeMap::new();
    };
    let mut sites = Vec::new();
    document.root.visit_with_base(&mut |element, xml_base| {
        if element
            .name
            .is(Some(scxml_validator_core::SCXML_NAMESPACE), "invoke")
            && let Some(src) = element.attribute("src")
        {
            let key = scxml_syntax::collapse_whitespace(&src.value).into_owned();
            let authored = match xml_base {
                Some(xml_base) => scxml_types::resolve_reference(xml_base, &key),
                None => key.clone(),
            };
            sites.push((key, resolve(base, &authored)));
        }
    });
    let mut invoked = BTreeMap::new();
    for (key, url) in sites {
        if invoked.contains_key(&key) {
            continue;
        }
        if let Some(document) = url.as_ref().and_then(&mut load) {
            invoked.insert(key, document);
        }
    }
    invoked
}

/// The documents the literal `<invoke src>` elements of the document at
/// `path` (holding `source`) load from `file:` URLs, each with its schema
/// files read relative to it, for `validate` and `compile`, which fetch no
/// document (a remote `src` is not judged).
pub(crate) fn local_invoked_documents(
    path: &Path,
    source: &[u8],
) -> BTreeMap<String, InvokedDocument> {
    let Ok(base) = file_base(path) else {
        return BTreeMap::new();
    };
    invoked_documents(&base, &String::from_utf8_lossy(source), |url| {
        let child = url.to_file_path().ok()?;
        // The run path's reader: only a regular file is opened, and a child
        // that cannot be read is skipped as `prefetch` skips it.
        let bytes = crate::read_source(&child).ok()?;
        let event_schemas = crate::event_schemas::load(&child, &bytes);
        Some(InvokedDocument {
            text: String::from_utf8(bytes).ok()?,
            event_schemas,
        })
    })
}

/// The text of each file the `<script src>` elements of `text` (a document
/// whose base URI is `base`) name, keyed by `src` as authored with its XML
/// white space collapsed, for the validator's side-effect analysis
/// ([`scxml_validator_core::Validator::with_script_resources`]): each `src`
/// resolved like a `<script src>` the run loads, and `load` reads the file
/// at that URL. A file that cannot be read, or is not UTF-8 (a Wasm
/// script), is left out and stays unread.
fn script_files(
    base: &url::Url,
    text: &str,
    mut load: impl FnMut(&url::Url) -> Option<Vec<u8>>,
) -> scxml_validator_core::ScriptResources {
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return BTreeMap::new();
    };
    let mut sites = Vec::new();
    document.root.visit_with_base(&mut |element, xml_base| {
        if element
            .name
            .is(Some(scxml_validator_core::SCXML_NAMESPACE), "script")
            && let Some(src) = element.attribute("src")
        {
            let key = scxml_syntax::collapse_whitespace(&src.value).into_owned();
            let authored = match xml_base {
                Some(xml_base) => scxml_types::resolve_reference(xml_base, &key),
                None => key.clone(),
            };
            sites.push((key, resolve(base, &authored)));
        }
    });
    let mut scripts = BTreeMap::new();
    for (key, url) in sites {
        if scripts.contains_key(&key) {
            continue;
        }
        if let Some(text) = url
            .as_ref()
            .and_then(&mut load)
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            scripts.insert(key, text);
        }
    }
    scripts
}

/// The text of each file the `<script src>` elements of the document at
/// `path` (holding `source`) name by a `file:` URL, for `validate` and
/// `compile`, which fetch nothing (a remote script stays unread).
pub(crate) fn local_script_files(
    path: &Path,
    source: &[u8],
) -> scxml_validator_core::ScriptResources {
    let Ok(base) = file_base(path) else {
        return BTreeMap::new();
    };
    script_files(&base, &String::from_utf8_lossy(source), |url| {
        // The run path's reader: only a regular file is opened.
        crate::read_source(&url.to_file_path().ok()?).ok()
    })
}

impl Documents {
    /// Fetches every literal URI of the root document at `path` (holding
    /// `source`) and of every `<invoke src>` document reached from it.
    ///
    /// # Errors
    ///
    /// A root `<script src>` that does not resolve or cannot be fetched
    /// (SCXML 5.8), more than [`MAX_DOCUMENTS`] documents, or a root path
    /// with no `file:` URL.
    pub(crate) fn prefetch(
        repository: Arc<Repository>,
        path: &Path,
        source: &[u8],
    ) -> Result<Self, String> {
        let base = file_base(path)?;
        let mut documents = Self {
            repository,
            base: base.clone(),
            literals: Vec::new(),
            children: Vec::new(),
            #[cfg(feature = "wasmtime")]
            bindings: BTreeMap::new(),
            event_schemas: BTreeMap::new(),
            #[cfg(feature = "wasmtime")]
            invoked: BTreeMap::new(),
            #[cfg(feature = "wasmtime")]
            srcexpr_scripts: std::sync::Mutex::new(BTreeMap::new()),
        };
        let root = String::from_utf8_lossy(source).into_owned();
        let mut pending = vec![(base.clone(), root, true)];
        let mut visited = BTreeSet::from([base.as_str().to_owned()]);
        while let Some((base, text, is_root)) = pending.pop() {
            for literal in literals_of(&base, &text) {
                let fetched = literal
                    .url
                    .as_ref()
                    .map(|url| documents.repository.fetch_document(url));
                if is_root && literal.site == Site::Script {
                    match &fetched {
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            return Err(format!(
                                "`<script src=\"{}\">` cannot be fetched, so the document is rejected (SCXML 1.0 §5.8): {error}",
                                literal.authored
                            ));
                        }
                        None => {
                            return Err(format!(
                                "`<script src=\"{}\">` is not an https:, file:, or relative URI, so the document is rejected (SCXML 1.0 §5.8)",
                                literal.authored
                            ));
                        }
                    }
                }
                if literal.site == Site::Invoke
                    && let (Some(url), Some(Ok(child))) = (&literal.url, &fetched)
                    && visited.insert(url.as_str().to_owned())
                {
                    if visited.len() > MAX_DOCUMENTS {
                        return Err(format!(
                            "the run loads more than {MAX_DOCUMENTS} documents through <invoke src>"
                        ));
                    }
                    let schemas = documents.child_event_schemas(url, &child.bytes, false);
                    if !schemas.is_empty() {
                        documents
                            .event_schemas
                            .insert(url.as_str().to_owned(), schemas);
                    }
                    documents.children.push((url.clone(), child.bytes.clone()));
                    pending.push((
                        url.clone(),
                        String::from_utf8_lossy(&child.bytes).into_owned(),
                        false,
                    ));
                }
                documents.literals.push(literal);
            }
        }
        #[cfg(feature = "wasmtime")]
        {
            documents.invoked = invoked_documents(&base, &String::from_utf8_lossy(source), |url| {
                let (_, bytes) = documents.children.iter().find(|(child, _)| child == url)?;
                Some(InvokedDocument {
                    text: String::from_utf8(bytes.clone()).ok()?,
                    event_schemas: documents
                        .event_schemas
                        .get(url.as_str())
                        .cloned()
                        .unwrap_or_default(),
                })
            });
        }
        Ok(documents)
    }

    /// The documents the root's literal `<invoke src>` elements load, with
    /// their schema files, as the validator takes them
    /// ([`scxml_validator_core::Validator::with_invoked_documents`]).
    #[cfg(feature = "wasmtime")]
    pub(crate) fn invoked_documents(&self) -> BTreeMap<String, InvokedDocument> {
        self.invoked.clone()
    }

    /// The URL and bytes of every `<invoke src>` document loaded before the
    /// session, whose repository namespaces and services the run binds with
    /// the root's.
    pub(crate) fn children(&self) -> &[(url::Url, Vec<u8>)] {
        &self.children
    }

    /// Has the child document at `url` carry `extensions` and `services` to
    /// the host for its own sessions
    /// ([`scxml_host_wasmtime::EmbeddedDocument`]).
    #[cfg(feature = "wasmtime")]
    pub(crate) fn bind(
        &mut self,
        url: &url::Url,
        extensions: Option<scxml_host_wasmtime::ExtensionComponents>,
        services: Option<scxml_host_wasmtime::ServiceComponents>,
    ) {
        let entry = self.bindings.entry(url.as_str().to_owned()).or_default();
        if extensions.is_some() {
            entry.0 = extensions;
        }
        if services.is_some() {
            entry.1 = services;
        }
    }

    /// The consent listing: every literal URI, the URL it loads, and its
    /// size and SHA-256 (or why it is unavailable).
    pub(crate) fn summary(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut lines = Vec::new();
        for literal in &self.literals {
            if !seen.insert((literal.site, literal.authored.clone(), literal.url.clone())) {
                continue;
            }
            let head = format!(
                "document `{}`\n  = for: <{} src=\"{}\">",
                literal
                    .url
                    .as_ref()
                    .map_or(literal.authored.as_str(), url::Url::as_str),
                literal.site.element(),
                literal.authored
            );
            let state = match literal
                .url
                .as_ref()
                .map(|url| self.repository.fetch_document(url))
            {
                Some(Ok(fetched)) => format!(
                    "  = loaded: {} bytes, sha256 {:x}",
                    fetched.bytes.len(),
                    Sha256::digest(&fetched.bytes)
                ),
                Some(Err(error)) => format!("  = unavailable: {error}"),
                None => "  = unavailable: not an https:, file:, or relative URI".to_owned(),
            };
            lines.push(format!("{head}\n{state}"));
        }
        lines
    }

    /// The URL `uri` (as the host passes it) names: `uri` resolved against
    /// the root's base. The host passes a URI relative only from a session
    /// of the root document (or an inline `<content>` session below one);
    /// one a child document names arrives resolved against that child's
    /// base URI. `None` when it does not resolve to an `https:` or `file:`
    /// URL.
    fn url_of(&self, uri: &str) -> Option<url::Url> {
        resolve(&self.base, &scxml_syntax::collapse_whitespace(uri))
    }

    /// The fetched document `uri` names: a literal one as it was fetched
    /// before the session, a `srcexpr` one as it is now.
    fn load(&self, uri: &str) -> Option<(url::Url, Fetched)> {
        let url = self.url_of(uri)?;
        let fetched = if self.is_literal(&url) {
            self.repository.fetch_document(&url)
        } else {
            self.repository.fetch_document_now(&url)
        }
        .ok()?;
        Some((url, fetched))
    }

    /// Whether a literal URI of the run's documents names `url`.
    fn is_literal(&self, url: &url::Url) -> bool {
        self.literals
            .iter()
            .any(|literal| literal.url.as_ref() == Some(url))
    }

    /// The schema files the `<invoke src>` document at `url` (holding
    /// `bytes`) references, resolved against `url`: read from its directory
    /// for a `file:` URL, fetched for an `https:` one (`now`: not from the
    /// fetcher's memo).
    fn child_event_schemas(
        &self,
        url: &url::Url,
        bytes: &[u8],
        now: bool,
    ) -> scxml_event_schema::Resources {
        match url.to_file_path() {
            Ok(path) => crate::event_schemas::load(&path, bytes),
            Err(()) => crate::event_schemas::fetch(url, bytes, |schema| {
                if now {
                    self.repository.fetch_document_now(schema)
                } else {
                    self.repository.fetch_document(schema)
                }
                .ok()
                .map(|fetched| fetched.bytes)
            }),
        }
    }

    /// The script package of every root `<script src>` (its exact authored
    /// URI, media type, SHA-256, and bytes), for the root's compile-time
    /// pins.
    ///
    /// # Errors
    ///
    /// A script whose media type is neither indicated nor implied by its
    /// URL (`.js`, `.mjs`, `.wasm`).
    pub(crate) fn script_packages(&self, source: &[u8]) -> Result<Vec<ScriptPackage>, String> {
        let root = String::from_utf8_lossy(source);
        let mut packages = BTreeMap::new();
        for literal in literals_of(&self.base, &root) {
            if literal.site != Site::Script || packages.contains_key(&literal.authored) {
                continue;
            }
            let Some((url, fetched)) = self.load(&literal.authored) else {
                continue;
            };
            let media_type = script_media_type(&url, &fetched).ok_or_else(|| {
                format!(
                    "`<script src=\"{}\">` indicates no script media type (text/javascript or application/wasm, or a .js, .mjs, or .wasm URL), so the document is rejected (SCXML 1.0 §5.8)",
                    literal.authored
                )
            })?;
            packages.insert(
                literal.authored.clone(),
                ScriptPackage {
                    uri: literal.authored,
                    media_type: media_type.to_owned(),
                    sha256: format!("{:x}", Sha256::digest(&fetched.bytes)),
                    bytes: fetched.bytes,
                },
            );
        }
        Ok(packages.into_values().collect())
    }
}

/// The script media type `fetched` indicates, else the one its URL's
/// extension implies.
fn script_media_type(url: &url::Url, fetched: &Fetched) -> Option<&'static str> {
    match fetched.media_type.as_deref() {
        Some("text/javascript" | "application/javascript") => return Some("text/javascript"),
        Some(media) if media == scxml_script::WASM_MEDIA_TYPE => {
            return Some(scxml_script::WASM_MEDIA_TYPE);
        }
        _ => {}
    }
    let (_, extension) = url.path().rsplit_once('.')?;
    if extension.eq_ignore_ascii_case("js") || extension.eq_ignore_ascii_case("mjs") {
        Some("text/javascript")
    } else if extension.eq_ignore_ascii_case("wasm") {
        Some(scxml_script::WASM_MEDIA_TYPE)
    } else {
        None
    }
}

/// The `<data src>` media type a URL's extension registers when its source
/// indicated none (SCXML B.2.1 reads a JSON or XML indication): `.json` is
/// `application/json` (RFC 8259 §11) and `.xml` is `application/xml`
/// (RFC 7303 §9.1). Other extensions indicate nothing; `.txt` in particular
/// stays unlabeled, because the official W3C cases ship JSON and XML in
/// `.txt` files and expect them read by content.
#[cfg_attr(
    not(any(feature = "wasmtime", test)),
    expect(dead_code, reason = "only a running session loads documents")
)]
fn extension_media_type(url: &url::Url) -> Option<&'static str> {
    let (_, extension) = url.path().rsplit_once('.')?;
    if extension.eq_ignore_ascii_case("json") {
        Some("application/json")
    } else if extension.eq_ignore_ascii_case("xml") {
        Some("application/xml")
    } else {
        None
    }
}

#[cfg(feature = "wasmtime")]
mod embedder {
    use std::collections::BTreeMap;

    use scxml_host_wasmtime::{
        DocumentKind, EmbeddedDocument, MachineScripts, ResolvedScript, ScriptPolicy,
        ScriptResolveError, ScriptResolver, SessionEmbedder, TrustedComponentDigest,
    };
    use sha2::{Digest as _, Sha256};

    use super::{
        Documents, Fetched, Site, extension_media_type, literals_of, resolve, script_media_type,
    };

    /// Every fetched `<script src>` of the run's documents, by URL, served
    /// to one session whose scripts load at run time (an invoked child): a
    /// `src` resolves against the base URI of the document it runs.
    struct Scripts {
        base: url::Url,
        scripts: BTreeMap<String, ResolvedScript>,
    }

    impl ScriptResolver for Scripts {
        fn resolve(&mut self, uri: &str) -> Result<ResolvedScript, ScriptResolveError> {
            resolve(&self.base, &scxml_syntax::collapse_whitespace(uri))
                .and_then(|url| self.scripts.get(url.as_str()))
                .cloned()
                .ok_or(ScriptResolveError::NotFound)
        }
    }

    impl SessionEmbedder for Documents {
        fn document(&self, kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
            let (url, fetched) = self.load(uri)?;
            let media_type = fetched
                .media_type
                .clone()
                .or_else(|| extension_media_type(&url).map(str::to_owned));
            let (extensions, services) = if kind == DocumentKind::Invocation {
                // A child loaded only now (a `srcexpr`) had no pin
                // resolved before the session: a pinned one does not load.
                if !self.children.iter().any(|(child, _)| *child == url) {
                    let pins =
                        crate::repository::document_pins(&fetched.bytes, url.as_str()).ok()?;
                    if pins
                        .namespaces
                        .values()
                        .chain(pins.services.values())
                        .any(Option::is_some)
                    {
                        return None;
                    }
                }
                self.bindings.get(url.as_str()).cloned().unwrap_or_default()
            } else {
                (None, None)
            };
            // A literal child's schema files and scripts were loaded before
            // the session; a `srcexpr` child's load with it, as they are now.
            let event_schemas = if kind != DocumentKind::Invocation {
                BTreeMap::new()
            } else if self.is_literal(&url) {
                self.event_schemas
                    .get(url.as_str())
                    .cloned()
                    .unwrap_or_default()
            } else {
                self.load_srcexpr_scripts(&url, &fetched.bytes);
                self.child_event_schemas(&url, &fetched.bytes, true)
            };
            let text = String::from_utf8(fetched.bytes).ok()?;
            Some(EmbeddedDocument {
                text,
                media_type,
                extensions,
                services,
                event_schemas,
                base_uri: (kind == DocumentKind::Invocation).then(|| url.to_string()),
            })
        }

        fn scripts(&self, base_uri: Option<&str>) -> MachineScripts {
            let mut fetched = BTreeMap::new();
            for literal in self
                .literals
                .iter()
                .filter(|literal| literal.site == Site::Script)
            {
                let Some(url) = literal.url.as_ref() else {
                    continue;
                };
                if fetched.contains_key(url.as_str()) {
                    continue;
                }
                if let Ok(script) = self.repository.fetch_document(url) {
                    fetched.insert(url.as_str().to_owned(), (url.clone(), script));
                }
            }
            if let Ok(loaded) = self.srcexpr_scripts.lock() {
                for (key, script) in loaded.iter() {
                    if let Ok(url) = url::Url::parse(key) {
                        fetched.insert(key.clone(), (url, script.clone()));
                    }
                }
            }
            let mut scripts = BTreeMap::new();
            let mut trusted = Vec::new();
            for (key, (url, script)) in fetched {
                let Some(media_type) = script_media_type(&url, &script) else {
                    continue;
                };
                if let Ok(digest) = TrustedComponentDigest::from_hex(&format!(
                    "{:x}",
                    Sha256::digest(&script.bytes)
                )) {
                    trusted.push(digest);
                }
                scripts.insert(
                    key,
                    ResolvedScript {
                        media_type: media_type.to_owned(),
                        bytes: script.bytes,
                    },
                );
            }
            let base = base_uri
                .and_then(|base| url::Url::parse(base).ok())
                .unwrap_or_else(|| self.base.clone());
            MachineScripts::runtime(
                Box::new(Scripts { base, scripts }),
                ScriptPolicy::new(trusted),
            )
        }
    }

    impl Documents {
        /// Fetches, as they are now, the `<script src>` files the `srcexpr`
        /// document at `url` (holding `bytes`) names, resolved against
        /// `url`, for the sessions that run it.
        fn load_srcexpr_scripts(&self, url: &url::Url, bytes: &[u8]) {
            let text = String::from_utf8_lossy(bytes);
            let mut loaded = Vec::new();
            for literal in literals_of(url, &text) {
                if literal.site != Site::Script {
                    continue;
                }
                if let Some(script) = literal.url.as_ref().and_then(|script| {
                    self.repository
                        .fetch_document_now(script)
                        .ok()
                        .map(|fetched: Fetched| (script.as_str().to_owned(), fetched))
                }) {
                    loaded.push(script);
                }
            }
            if let Ok(mut scripts) = self.srcexpr_scripts.lock() {
                scripts.extend(loaded);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_https_and_file_uris_resolve_and_others_do_not() {
        let base = url::Url::parse("file:///work/documents/root.scx").expect("a URL");
        assert_eq!(
            resolve(&base, "child.scx").map(String::from),
            Some("file:///work/documents/child.scx".to_owned())
        );
        assert_eq!(
            resolve(&base, "../lib/a.js").map(String::from),
            Some("file:///work/lib/a.js".to_owned())
        );
        assert_eq!(
            resolve(&base, "https://example.com/x.scx").map(String::from),
            Some("https://example.com/x.scx".to_owned())
        );
        let remote = url::Url::parse("https://example.com/a/b.scx").expect("a URL");
        assert_eq!(
            resolve(&remote, "c.scx").map(String::from),
            Some("https://example.com/a/c.scx".to_owned())
        );
        for refused in [
            "http://example.com/x",
            "ftp://example.com/x",
            "data:text/plain,x",
        ] {
            assert!(resolve(&base, refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn literals_are_read_from_scxml_elements_only() {
        let base = url::Url::parse("file:///w/root.scx").expect("a URL");
        let text = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:x" version="1.0"><datamodel><data id="d" src=" d.json "/></datamodel><script src="lib.js"/><state id="s"><invoke src="c.scx"/><invoke srcexpr="'e.scx'"/><x:invoke src="no.scx"/></state></scxml>"#;
        let literals = literals_of(&base, text);
        let found: Vec<(Site, &str)> = literals
            .iter()
            .map(|literal| (literal.site, literal.authored.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (Site::Data, "d.json"),
                (Site::Script, "lib.js"),
                (Site::Invoke, "c.scx")
            ]
        );
    }

    #[test]
    fn script_media_types_come_from_the_source_or_the_extension() {
        let url = |text: &str| url::Url::parse(text).expect("a URL");
        let fetched = |media: Option<&str>| Fetched {
            bytes: Vec::new(),
            media_type: media.map(str::to_owned),
        };
        assert_eq!(
            script_media_type(
                &url("https://e.com/a"),
                &fetched(Some("application/javascript"))
            ),
            Some("text/javascript")
        );
        assert_eq!(
            script_media_type(&url("file:///a.mjs"), &fetched(None)),
            Some("text/javascript")
        );
        assert_eq!(
            script_media_type(&url("file:///a.wasm"), &fetched(None)),
            Some(scxml_script::WASM_MEDIA_TYPE)
        );
        assert_eq!(
            script_media_type(&url("file:///a.txt"), &fetched(None)),
            None
        );
        assert_eq!(
            extension_media_type(&url("file:///d.JSON")),
            Some("application/json")
        );
        assert_eq!(extension_media_type(&url("file:///d.txt")), None);
    }
}
