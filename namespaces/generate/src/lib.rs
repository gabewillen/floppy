//! The `generate` extension namespace (skeleton).
//!
//! This crate will become the `generate` namespace's
//! `stateforward:scxml/executable-content-extension` Wasm component: model
//! generation (text and schema-constrained structured output) through a
//! pluggable `OpenAI`-compatible backend: `OpenAI` first, `NVIDIA` Nemotron
//! and CLM later, selected by the `env:baseurl` slot.
//!
//! It is an empty skeleton today: it exports no WIT world yet, and
//! `package/` holds no manifest or schema. A later change implements the
//! component, its `stateforward.scxml.extension-package/v1` manifest, and its
//! XSD, and adds its import allowlist to `scripts/build-components.sh`.

#![forbid(unsafe_code)]

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "generate";

/// The namespace's repository namespace URI without its `@<tag>` suffix; a
/// document binds `{NAMESPACE_URI_BASE}@<tag>` exactly.
pub const NAMESPACE_URI_BASE: &str = "https://github.com/stateforward/nemobot/namespaces/generate";

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
