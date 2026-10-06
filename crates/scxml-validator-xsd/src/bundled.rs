//! Bundled official W3C SCXML 1.0 XML Schema set.
//!
//! The eight documents under `schemas/w3c-scxml-1.0/` are the unmodified
//! bytes published by the W3C at <https://www.w3.org/2011/04/SCXML/>, the
//! schema location named by the SCXML 1.0 Recommendation (2015-09-01). Their
//! license is the W3C schema notice reproduced in `scxml-copyright.xsd` and
//! `schemas/w3c-scxml-1.0/NOTICE.md`: use, copy, modification, and
//! distribution are permitted provided the copyright notice and permission
//! paragraph appear in all copies.
//!
//! [`BundledSchemaResolver`] makes this set the default schema for the exact
//! SCXML namespace URI. A caller-registered schema for that namespace always
//! takes precedence. The bytes are compiled into the binary, so validation
//! performs no filesystem or network access to obtain them, and every build
//! re-verifies their SHA-256 identity in tests.

use scxml_validator_core::{SCXML_NAMESPACE, SchemaResolveError, SchemaResolver, SchemaSource};

/// Upstream directory of the official SCXML 1.0 schema set.
pub const W3C_SCXML_SCHEMA_BASE_URL: &str = "https://www.w3.org/2011/04/SCXML/";

/// One vendored official schema document with its exact provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BundledSchemaDocument {
    /// Upstream file name relative to [`W3C_SCXML_SCHEMA_BASE_URL`].
    pub file_name: &'static str,
    /// Lowercase SHA-256 of the exact vendored bytes (equal to upstream).
    pub sha256: &'static str,
    /// Whether the document contributes schema components. The copyright
    /// document is annotation-only and is carried for its license text.
    pub component_document: bool,
    /// Exact vendored bytes.
    pub bytes: &'static [u8],
}

macro_rules! bundled {
    ($file:literal, $sha:literal, $component:literal) => {
        BundledSchemaDocument {
            file_name: $file,
            sha256: $sha,
            component_document: $component,
            bytes: include_bytes!(concat!("../schemas/w3c-scxml-1.0/", $file)),
        }
    };
}

/// The complete vendored W3C SCXML 1.0 schema set, driver first.
pub const W3C_SCXML_SCHEMA_DOCUMENTS: [BundledSchemaDocument; 8] = [
    bundled!(
        "scxml.xsd",
        "98e14d0e03d5f9bb49b03fe4a2c24d72c165789d68fe93eee24fc403e4b23b48",
        true
    ),
    bundled!(
        "scxml-module-core.xsd",
        "cbe0cde7a2cc165b84263e0dad45abd74296ed99a9196611e4b0c98b494222e4",
        true
    ),
    bundled!(
        "scxml-module-data.xsd",
        "c34438431aa5d9815a63a64415e3127b832abd451ca509ff79a89d529dddba47",
        true
    ),
    bundled!(
        "scxml-module-external.xsd",
        "899b13488332a6b37700ea53766e16d39b1b4f38fb829d0d3b12f0955a08057d",
        true
    ),
    bundled!(
        "scxml-datatypes.xsd",
        "51036cac697db9be6f92ad853215dc3166990ca292786fed1612d0666932163b",
        true
    ),
    bundled!(
        "scxml-attribs.xsd",
        "b74de05e9e8d86123a4e8cca6655761fae78009e8d63b95cc27ae38222a2de01",
        true
    ),
    bundled!(
        "scxml-contentmodels.xsd",
        "704baaf6e19c480fa76039390dc0a0e045f5544b5f68ec294c47fe51d40d0053",
        true
    ),
    bundled!(
        "scxml-copyright.xsd",
        "a980a440bf960a57168230c5bcd60f2cfe82e1dc287e339850b2ef42cc22e35b",
        false
    ),
];

/// Returns the component documents of the official schema set as supplied
/// schema sources whose system identifiers are the exact upstream URLs.
///
/// The XML namespace import in the upstream driver names a remote
/// `schemaLocation`; it is never fetched because the SCXML 1.0 schema set
/// declares no `xml:*` attribute uses, and the backend installs no resolver.
#[must_use]
pub fn w3c_scxml_schema_set() -> Vec<SchemaSource> {
    W3C_SCXML_SCHEMA_DOCUMENTS
        .iter()
        .filter(|document| document.component_document)
        .map(|document| {
            SchemaSource::new(
                format!("{W3C_SCXML_SCHEMA_BASE_URL}{}", document.file_name),
                document.bytes,
            )
        })
        .collect()
}

/// Resolver decorator that supplies the bundled W3C SCXML 1.0 schema set for
/// the exact SCXML namespace when the wrapped resolver has no mapping for it.
///
/// Every other namespace, and any explicit SCXML-namespace registration,
/// resolves exactly as the wrapped resolver does. Load failures of an
/// explicit registration are never masked by the bundled fallback.
#[derive(Clone, Debug, Default)]
pub struct BundledSchemaResolver<R> {
    inner: R,
}

impl<R> BundledSchemaResolver<R> {
    /// Wraps `inner` with the bundled core-schema fallback.
    #[must_use]
    pub const fn new(inner: R) -> Self {
        Self { inner }
    }
}

impl<R: SchemaResolver> SchemaResolver for BundledSchemaResolver<R> {
    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
        let result = self.inner.resolve(namespace_uri);
        if matches!(result, Err(SchemaResolveError::NotFound)) && namespace_uri == SCXML_NAMESPACE {
            // A single-document view cannot represent the multi-document
            // official set; callers use `resolve_documents` for it.
            return Err(SchemaResolveError::Load(
                "the bundled W3C SCXML 1.0 schema is a multi-document set; resolve it with `resolve_documents`".to_owned(),
            ));
        }
        result
    }

    fn resolve_documents(
        &self,
        namespace_uri: &str,
    ) -> Result<Vec<SchemaSource>, SchemaResolveError> {
        match self.inner.resolve_documents(namespace_uri) {
            Err(SchemaResolveError::NotFound) if namespace_uri == SCXML_NAMESPACE => {
                Ok(w3c_scxml_schema_set())
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use scxml_validator_core::MemorySchemaResolver;
    use sha2::{Digest as _, Sha256};

    use super::*;

    #[test]
    fn vendored_bytes_match_recorded_upstream_sha256_provenance() {
        for document in W3C_SCXML_SCHEMA_DOCUMENTS {
            assert_eq!(
                hex::encode(Sha256::digest(document.bytes)),
                document.sha256,
                "{} drifted from the recorded W3C bytes",
                document.file_name
            );
        }
        let notice = include_str!("../schemas/w3c-scxml-1.0/NOTICE.md");
        for document in W3C_SCXML_SCHEMA_DOCUMENTS {
            assert!(
                notice.contains(document.sha256) && notice.contains(document.file_name),
                "NOTICE.md must record {} with its digest",
                document.file_name
            );
        }
    }

    #[test]
    fn bundled_set_is_default_only_for_the_exact_scxml_namespace() {
        let resolver = BundledSchemaResolver::new(MemorySchemaResolver::new());
        let set = resolver
            .resolve_documents(SCXML_NAMESPACE)
            .expect("bundled core schema set must resolve");
        assert_eq!(set.len(), 7);
        assert_eq!(
            set[0].system_id,
            "https://www.w3.org/2011/04/SCXML/scxml.xsd"
        );
        for near_miss in [
            "http://www.w3.org/2005/07/scxml/",
            "https://www.w3.org/2005/07/scxml",
            "HTTP://www.w3.org/2005/07/scxml",
        ] {
            assert_eq!(
                resolver.resolve_documents(near_miss),
                Err(SchemaResolveError::NotFound)
            );
        }
    }

    #[test]
    fn explicit_core_registration_takes_precedence_over_bundled_set() {
        let mut inner = MemorySchemaResolver::new();
        inner.insert(
            SCXML_NAMESPACE,
            SchemaSource::new("memory:subset.xsd", b"<schema/>".to_vec()),
        );
        let resolver = BundledSchemaResolver::new(inner);
        let set = resolver
            .resolve_documents(SCXML_NAMESPACE)
            .expect("explicit registration must resolve");
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].system_id, "memory:subset.xsd");
    }
}
