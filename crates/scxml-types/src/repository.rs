//! The repository package URI pattern ([`RepositoryUri`]).

use crate::is_package_segment;

/// The parts of a tagless repository package URI (see [`crate::strip_tag`]):
/// `https://github.com/<owner>/<repo>/<folder>/<name>`, where `folder` is
/// `namespaces` or `services`. This is the one definition of the pattern:
/// the parser splits pins with it, the CLI routes repository packages with
/// it, and the repository resolver component builds its requests from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepositoryUri<'a> {
    /// The repository's owner.
    pub owner: &'a str,
    /// The repository's name.
    pub repo: &'a str,
    /// `namespaces` (an extension namespace) or `services` (an invoked
    /// service).
    pub folder: &'a str,
    /// The package's folder name.
    pub name: &'a str,
}

impl<'a> RepositoryUri<'a> {
    /// The parts of `uri` when it is exactly a tagless repository package
    /// URI; `None` for every other URI, a pinned one included.
    #[must_use]
    pub fn parse(uri: &'a str) -> Option<Self> {
        let path = uri.strip_prefix("https://github.com/")?;
        let mut segments = path.split('/');
        let mut next = || {
            segments
                .next()
                .filter(|segment| is_package_segment(segment))
        };
        let parsed = Self {
            owner: next()?,
            repo: next()?,
            folder: next().filter(|folder| matches!(*folder, "namespaces" | "services"))?,
            name: next()?,
        };
        segments.next().is_none().then_some(parsed)
    }
}
