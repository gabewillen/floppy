//! The `scxml` extension namespace (skeleton).
//!
//! This crate will become the `scxml` namespace's
//! `stateforward:scxml/executable-content-extension` Wasm component: the SCXML
//! authoring tools a bot uses on documents it writes. Its `validate` element
//! wraps the scxml.rs validator component and returns its canonical
//! diagnostics and machine-applicable fixes. Documents bind the namespace with
//! a prefix other than `scxml` (for example `sx`), since `scxml`
//! conventionally names the core SCXML namespace.
//!
//! It is an empty skeleton today: it exports no WIT world yet, and
//! `package/` holds no manifest or schema. A later change implements the
//! component, its `stateforward.scxml.extension-package/v1` manifest, and its
//! XSD, and adds its import allowlist to `scripts/build-components.sh`.

#![forbid(unsafe_code)]

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "scxml";

/// The namespace's repository namespace URI without its `@<tag>` suffix; a
/// document binds `{NAMESPACE_URI_BASE}@<tag>` exactly.
pub const NAMESPACE_URI_BASE: &str = "https://github.com/stateforward/nemobot/namespaces/scxml";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_namespace_uri_is_the_repository_path_of_its_name() {
        assert_eq!(
            NAMESPACE_URI_BASE.rsplit('/').next(),
            Some(NAMESPACE_NAME),
            "the URI's last segment names the namespace"
        );
    }
}
