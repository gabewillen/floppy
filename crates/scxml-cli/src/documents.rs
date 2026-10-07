//! The documents a run loads by URI: `<invoke src>` (SCXML 6.4),
//! `<script src>` (5.8), and `<data src>` (5.3), and the `srcexpr` URIs an
//! `<invoke>` evaluates when it runs.
//!
//! # Which URIs load
//!
//! An `https:` URI, a `file:` URI, or a relative URI, resolved against the
//! base URI of the document that names it (RFC 3986 §5; the root
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
//! is fetched when its `<invoke>` runs, through the same fetcher (relative
//! ones against the root document's base, the only base the host's
//! `load-document` call identifies); a failure is `error.execution`.
//! Everything is fetched once per root session and shared by every session
//! of the tree.
//!
//! The documents an `<invoke src>` loads are part of the run: the
//! repository namespaces and services they use are resolved with the
//! root's, before the session starts ([`Documents::children`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use scxml_compiler_core::ScriptPackage;
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
    /// The bytes of every `<invoke src>` document loaded before the session.
    children: Vec<Vec<u8>>,
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
    document.root.visit(&mut |element| {
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
            let authored = scxml_syntax::collapse_whitespace(&src.value).into_owned();
            literals.push(Literal {
                site,
                url: resolve(base, &authored),
                authored,
            });
        }
    });
    literals
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
                    documents.children.push(child.bytes.clone());
                    pending.push((
                        url.clone(),
                        String::from_utf8_lossy(&child.bytes).into_owned(),
                        false,
                    ));
                }
                documents.literals.push(literal);
            }
        }
        Ok(documents)
    }

    /// The bytes of every `<invoke src>` document loaded before the session,
    /// whose repository namespaces and services the run binds with the
    /// root's.
    pub(crate) fn children(&self) -> &[Vec<u8>] {
        &self.children
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

    /// The URL `uri` (as the engine passes it) names: the one its literal
    /// sites resolved to, or, for a URI no literal names (a `srcexpr`
    /// value), `uri` resolved against the root's base. `None` when it does
    /// not resolve, or when literals in documents of different bases
    /// resolve the same text to different URLs.
    fn url_of(&self, uri: &str) -> Option<url::Url> {
        let authored = scxml_syntax::collapse_whitespace(uri);
        let urls: BTreeSet<&url::Url> = self
            .literals
            .iter()
            .filter(|literal| literal.authored == authored)
            .filter_map(|literal| literal.url.as_ref())
            .collect();
        let named = self
            .literals
            .iter()
            .any(|literal| literal.authored == authored);
        match (urls.len(), named) {
            (1, _) => urls.into_iter().next().cloned(),
            (0, false) => resolve(&self.base, &authored),
            _ => None,
        }
    }

    /// The fetched document `uri` names (fetching a `srcexpr` URI now).
    fn load(&self, uri: &str) -> Option<(url::Url, Fetched)> {
        let url = self.url_of(uri)?;
        let fetched = self.repository.fetch_document(&url).ok()?;
        Some((url, fetched))
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

    use super::{Documents, Site, extension_media_type, script_media_type};

    /// Every fetched `<script src>` of the run's documents, served to the
    /// sessions whose scripts load at run time (invoked children).
    struct Scripts(BTreeMap<String, ResolvedScript>);

    impl ScriptResolver for Scripts {
        fn resolve(&mut self, uri: &str) -> Result<ResolvedScript, ScriptResolveError> {
            self.0
                .get(scxml_syntax::collapse_whitespace(uri).as_ref())
                .cloned()
                .ok_or(ScriptResolveError::NotFound)
        }
    }

    impl SessionEmbedder for Documents {
        fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
            let (url, fetched) = self.load(uri)?;
            let media_type = fetched
                .media_type
                .clone()
                .or_else(|| extension_media_type(&url).map(str::to_owned));
            let text = String::from_utf8(fetched.bytes).ok()?;
            Some(EmbeddedDocument { text, media_type })
        }

        fn scripts(&self) -> MachineScripts {
            let mut scripts = BTreeMap::new();
            let mut trusted = Vec::new();
            for literal in self
                .literals
                .iter()
                .filter(|literal| literal.site == Site::Script)
            {
                if scripts.contains_key(&literal.authored) {
                    continue;
                }
                let Some(url) = literal.url.as_ref() else {
                    continue;
                };
                let Ok(fetched) = self.repository.fetch_document(url) else {
                    continue;
                };
                let Some(media_type) = script_media_type(url, &fetched) else {
                    continue;
                };
                if let Ok(digest) = TrustedComponentDigest::from_hex(&format!(
                    "{:x}",
                    Sha256::digest(&fetched.bytes)
                )) {
                    trusted.push(digest);
                }
                scripts.insert(
                    literal.authored.clone(),
                    ResolvedScript {
                        media_type: media_type.to_owned(),
                        bytes: fetched.bytes,
                    },
                );
            }
            MachineScripts::runtime(Box::new(Scripts(scripts)), ScriptPolicy::new(trusted))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_https_and_file_uris_resolve_and_others_do_not() {
        let base = url::Url::parse("file:///work/bots/root.scx").expect("a URL");
        assert_eq!(
            resolve(&base, "child.scx").map(String::from),
            Some("file:///work/bots/child.scx".to_owned())
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
