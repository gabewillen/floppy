//! The canonical WHATWG allowlist of outbound HTTP targets: the `BasicHTTP`
//! network client's entries, and each extension package's declared
//! `http-origins`.

/// Maximum entries of one [`HttpAllowlist`].
pub const MAX_ALLOWLIST_ENTRIES: usize = 256;
/// Maximum bytes of one allowlist entry.
pub const MAX_ALLOWLIST_ENTRY_BYTES: usize = 2_048;

/// The canonical form of `url` (the WHATWG URL parser's serialization)
/// when it is an absolute `http` or `https` URL with a host and without
/// userinfo; `None` otherwise.
#[must_use]
pub fn canonical_http_url(url: &str) -> Option<String> {
    parse_http_url(url).map(String::from)
}

pub(crate) fn parse_http_url(url: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(url).ok()?;
    let http = matches!(parsed.scheme(), "http" | "https");
    let userinfo = !parsed.username().is_empty() || parsed.password().is_some();
    (http && !userinfo && parsed.host().is_some()).then_some(parsed)
}

/// Why one allowlist entry was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllowlistReason {
    /// More than [`MAX_ALLOWLIST_ENTRIES`] entries.
    TooMany,
    /// Longer than [`MAX_ALLOWLIST_ENTRY_BYTES`].
    TooLong,
    /// Not an absolute `http` or `https` URL with a host.
    NotHttp,
    /// Carries userinfo (credentials are never sent).
    Userinfo,
    /// Carries a query or fragment (entries are origins or path prefixes).
    QueryOrFragment,
}

/// One refused allowlist entry: its position (0-based) and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllowlistError {
    /// The entry's 0-based position.
    pub index: usize,
    /// Why it was refused.
    pub reason: AllowlistReason,
}

impl std::fmt::Display for AllowlistError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.reason {
            AllowlistReason::TooMany => {
                return write!(
                    formatter,
                    "the allowlist exceeds {MAX_ALLOWLIST_ENTRIES} entries"
                );
            }
            AllowlistReason::TooLong => "is longer than its byte bound",
            AllowlistReason::NotHttp => "is not an absolute http or https URL with a host",
            AllowlistReason::Userinfo => "carries userinfo; credentials are never sent",
            AllowlistReason::QueryOrFragment => {
                "carries a query or fragment; an entry is an origin or a path prefix"
            }
        };
        write!(formatter, "allowlist entry {} {reason}", self.index + 1)
    }
}

impl std::error::Error for AllowlistError {}

#[derive(Clone, Debug)]
struct AllowEntry {
    origin: url::Origin,
    /// The canonical path; `/` admits the whole origin.
    path: String,
}

impl AllowEntry {
    fn admits(&self, target: &url::Url) -> bool {
        if target.origin() != self.origin {
            return false;
        }
        let path = target.path();
        self.path == "/"
            || path == self.path
            || (self.path.ends_with('/') && path.starts_with(self.path.as_str()))
            || path
                .strip_prefix(self.path.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// The targets a client may send to: origins
/// (`https://api.example`, admitting every path) and canonical path
/// prefixes (`https://api.example/scxml`, admitting `/scxml`, `/scxml/`,
/// and everything below `/scxml/`, but never `/scxml-admin`). Entries and
/// targets are compared in their canonical WHATWG form, so a target's dot
/// segments cannot step outside an admitted prefix. The empty allowlist
/// admits nothing.
#[derive(Clone, Debug, Default)]
pub struct HttpAllowlist {
    entries: Vec<AllowEntry>,
}

impl HttpAllowlist {
    /// Parses every entry.
    ///
    /// # Errors
    ///
    /// The first refused entry ([`AllowlistError`]).
    pub fn new<I>(entries: I) -> Result<Self, AllowlistError>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let mut parsed = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let entry = entry.as_ref();
            let refuse = |reason| AllowlistError { index, reason };
            if index >= MAX_ALLOWLIST_ENTRIES {
                return Err(refuse(AllowlistReason::TooMany));
            }
            if entry.len() > MAX_ALLOWLIST_ENTRY_BYTES {
                return Err(refuse(AllowlistReason::TooLong));
            }
            let url = url::Url::parse(entry).map_err(|_| refuse(AllowlistReason::NotHttp))?;
            if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
                return Err(refuse(AllowlistReason::NotHttp));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(refuse(AllowlistReason::Userinfo));
            }
            if url.query().is_some() || url.fragment().is_some() {
                return Err(refuse(AllowlistReason::QueryOrFragment));
            }
            parsed.push(AllowEntry {
                origin: url.origin(),
                path: url.path().to_owned(),
            });
        }
        Ok(Self { entries: parsed })
    }

    /// Whether `url` is admitted: it must be an absolute `http`/`https`
    /// URL without userinfo, and its canonical form must fall under an
    /// entry.
    #[must_use]
    pub fn admits(&self, url: &str) -> bool {
        parse_http_url(url).is_some_and(|url| self.admits_canonical(&url))
    }

    pub(crate) fn admits_canonical(&self, url: &url::Url) -> bool {
        self.entries.iter().any(|entry| entry.admits(url))
    }

    /// Whether no entry is configured (nothing is admitted).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
