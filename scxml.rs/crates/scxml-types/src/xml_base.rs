//! Reference resolution for XML Base (`xml:base`).

/// The reference `reference` resolved against `base` (RFC 3986 §5.2.2),
/// where `base` may itself be a relative reference: XML Base (W3C XML Base,
/// Second Edition, §4.2) resolves each `xml:base` against the base of its
/// parent, and the outermost against the document's own base URI, which
/// only the host knows. So this resolves exactly as RFC 3986 does when
/// `base` is absolute, and otherwise yields a reference that resolves
/// against the document's base URI to the same URI as resolving `reference`
/// against `base` would: dot segments of a relative result are kept for
/// that last resolution (removing them from a relative path could not undo
/// a leading `..`). An empty `base` leaves `reference` unchanged.
///
/// Pure, deterministic, and linear in its inputs.
#[must_use]
pub fn resolve_reference(base: &str, reference: &str) -> String {
    let reference_parts = Parts::of(reference);
    if base.is_empty() || reference_parts.scheme.is_some() {
        return reference_parts.recompose_resolved();
    }
    let base_parts = Parts::of(base);
    let mut target = Parts {
        scheme: base_parts.scheme,
        authority: base_parts.authority,
        path: String::new(),
        query: reference_parts.query,
        fragment: reference_parts.fragment,
    };
    if reference_parts.authority.is_some() {
        target.authority = reference_parts.authority;
        target.path = reference_parts.path;
    } else if reference_parts.path.is_empty() {
        target.path = base_parts.path;
        if target.query.is_none() {
            target.query = base_parts.query;
        }
    } else if reference_parts.path.starts_with('/') {
        target.path = reference_parts.path;
    } else if base_parts.authority.is_some() && base_parts.path.is_empty() {
        target.path = format!("/{}", reference_parts.path);
    } else {
        let directory = base_parts
            .path
            .rfind('/')
            .map_or("", |slash| &base_parts.path[..=slash]);
        target.path = format!("{directory}{}", reference_parts.path);
    }
    target.recompose_resolved()
}

/// The five components of a URI reference (RFC 3986 §5.2.1, Appendix B).
struct Parts<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: String,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

impl<'a> Parts<'a> {
    fn of(reference: &'a str) -> Self {
        let (rest, fragment) = match reference.split_once('#') {
            Some((rest, fragment)) => (rest, Some(fragment)),
            None => (reference, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((rest, query)) => (rest, Some(query)),
            None => (rest, None),
        };
        let (scheme, rest) = match rest.split_once(':') {
            Some((scheme, rest)) if is_scheme(scheme) => (Some(scheme), rest),
            _ => (None, rest),
        };
        let (authority, path) = match rest.strip_prefix("//") {
            Some(rest) => {
                let end = rest.find('/').unwrap_or(rest.len());
                (rest.get(..end), rest.get(end..).unwrap_or_default())
            }
            None => (None, rest),
        };
        Self {
            scheme,
            authority,
            path: path.to_owned(),
            query,
            fragment,
        }
    }

    /// The reference, its dot segments removed when its path is absolute
    /// (it has a scheme, an authority, or a leading `/`).
    fn recompose_resolved(&self) -> String {
        let path =
            if self.scheme.is_some() || self.authority.is_some() || self.path.starts_with('/') {
                remove_dot_segments(&self.path)
            } else {
                self.path.clone()
            };
        let mut text = String::new();
        if let Some(scheme) = self.scheme {
            text.push_str(scheme);
            text.push(':');
        }
        if let Some(authority) = self.authority {
            text.push_str("//");
            text.push_str(authority);
        }
        text.push_str(&path);
        if let Some(query) = self.query {
            text.push('?');
            text.push_str(query);
        }
        if let Some(fragment) = self.fragment {
            text.push('#');
            text.push_str(fragment);
        }
        text
    }
}

/// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` (RFC 3986 §3.1).
fn is_scheme(text: &str) -> bool {
    text.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

/// RFC 3986 §5.2.4.
fn remove_dot_segments(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    let last = segments.len().saturating_sub(1);
    for (index, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {
                if index == last {
                    output.push("");
                }
            }
            ".." => {
                if output.len() > 1 {
                    output.pop();
                }
                if index == last {
                    output.push("");
                }
            }
            segment => output.push(segment),
        }
    }
    let joined = output.join("/");
    if path.starts_with('/') && !joined.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_reference;

    #[test]
    fn rfc_3986_normal_examples_resolve() {
        // RFC 3986 §5.4.1, against "http://a/b/c/d;p?q".
        let base = "http://a/b/c/d;p?q";
        for (reference, expected) in [
            ("g:h", "g:h"),
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../g", "http://a/g"),
            ("../../../g", "http://a/g"),
        ] {
            assert_eq!(resolve_reference(base, reference), expected, "{reference}");
        }
    }

    #[test]
    fn relative_bases_compose_and_keep_their_dot_segments() {
        assert_eq!(resolve_reference("", "child.scxml"), "child.scxml");
        assert_eq!(resolve_reference("sub/", "child.scxml"), "sub/child.scxml");
        assert_eq!(
            resolve_reference("sub/inner/", "../child.scxml"),
            "sub/inner/../child.scxml"
        );
        assert_eq!(resolve_reference("../up/", "c.scxml"), "../up/c.scxml");
        assert_eq!(resolve_reference("sub/", "/abs.scxml"), "/abs.scxml");
        assert_eq!(
            resolve_reference("https://example.org/m/", "c.scxml"),
            "https://example.org/m/c.scxml"
        );
        assert_eq!(
            resolve_reference("sub/", "file:///x.scxml"),
            "file:///x.scxml"
        );
    }
}
