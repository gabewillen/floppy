//! Bounded validation for untrusted validator/compiler component requests.
//!
//! The `MAX_COMPONENT_*` constants below are the enforcement of the request
//! bounds published on `validation-request` and `compile-request` in the
//! canonical WIT (`crates/scxml-wit/wit/{validator,compiler}`). The WIT text
//! is the public contract; `scxml-wit` tests fail when a documented bound and
//! its constant disagree, so both change together. Every check runs before
//! parsing, copies no caller data into errors or telemetry, and fails closed
//! on the first violation.

use std::collections::BTreeSet;
use std::fmt;

use crate::schema::{SUPPORTED_NAMESPACE_WIT_VERSION, SUPPORTED_NAMESPACE_WIT_WORLD};

/// Maximum accepted SCXML source bytes in one component request.
pub const MAX_COMPONENT_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum accepted schema resources in one component request.
pub const MAX_COMPONENT_SCHEMA_COUNT: usize = 64;
/// Maximum accepted extension packages in one component request.
pub const MAX_COMPONENT_PACKAGE_COUNT: usize = 64;
/// Maximum accepted bytes in one schema resource.
pub const MAX_COMPONENT_SCHEMA_BYTES: usize = 4 * 1024 * 1024;
/// Maximum accepted aggregate schema bytes in one component request.
pub const MAX_COMPONENT_TOTAL_SCHEMA_BYTES: usize = 16 * 1024 * 1024;
/// Maximum accepted UTF-8 bytes in one request metadata field.
pub const MAX_COMPONENT_METADATA_BYTES: usize = 4 * 1024;
/// Maximum accepted aggregate UTF-8 metadata bytes in one component request.
pub const MAX_COMPONENT_TOTAL_METADATA_BYTES: usize = 1024 * 1024;
/// Maximum accepted aggregate source, schema, and metadata bytes in a request.
pub const MAX_COMPONENT_REQUEST_BYTES: usize = 32 * 1024 * 1024;

/// Stable fail-closed errors for an untrusted component request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestValidationError {
    /// A required metadata field was empty.
    EmptyMetadata {
        /// Stable field name; never contains caller-controlled data.
        field: &'static str,
    },
    /// One metadata field exceeded its byte bound.
    MetadataTooLarge {
        /// Stable field name; never contains caller-controlled data.
        field: &'static str,
    },
    /// The source exceeded its byte bound.
    SourceTooLarge,
    /// The request supplied too many schemas.
    TooManySchemas,
    /// The request supplied too many packages.
    TooManyPackages,
    /// One schema exceeded its byte bound.
    SchemaTooLarge,
    /// Aggregate schema bytes exceeded their bound.
    TotalSchemaBytesTooLarge,
    /// Aggregate metadata bytes exceeded their bound.
    TotalMetadataBytesTooLarge,
    /// Aggregate request bytes exceeded their bound.
    RequestBytesTooLarge,
    /// Two schemas claimed the same exact namespace URI.
    DuplicateSchemaNamespace,
    /// Two packages claimed the same exact namespace identity.
    DuplicatePackageNamespace,
    /// A package named a WIT world other than the authoritative supported world.
    UnsupportedNamespaceWitWorld,
    /// A package named a WIT version other than the authoritative supported version.
    UnsupportedNamespaceWitVersion,
}

impl fmt::Display for RequestValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyMetadata { field } => {
                return write!(
                    formatter,
                    "request metadata field `{field}` must not be empty"
                );
            }
            Self::MetadataTooLarge { field } => {
                return write!(
                    formatter,
                    "request metadata field `{field}` exceeds {MAX_COMPONENT_METADATA_BYTES} bytes"
                );
            }
            Self::SourceTooLarge => "request source exceeds the component source byte limit",
            Self::TooManySchemas => "request schema count exceeds the component limit",
            Self::TooManyPackages => "request package count exceeds the component limit",
            Self::SchemaTooLarge => "request schema exceeds the per-schema byte limit",
            Self::TotalSchemaBytesTooLarge => {
                "request aggregate schema bytes exceed the component limit"
            }
            Self::TotalMetadataBytesTooLarge => {
                "request aggregate metadata bytes exceed the component limit"
            }
            Self::RequestBytesTooLarge => "request aggregate bytes exceed the component limit",
            Self::DuplicateSchemaNamespace => {
                "request contains a duplicate exact schema namespace URI"
            }
            Self::DuplicatePackageNamespace => {
                "request contains a duplicate exact package namespace identity"
            }
            Self::UnsupportedNamespaceWitWorld => {
                return write!(
                    formatter,
                    "request package WIT world must equal `{SUPPORTED_NAMESPACE_WIT_WORLD}`"
                );
            }
            Self::UnsupportedNamespaceWitVersion => {
                return write!(
                    formatter,
                    "request package WIT version must equal `{SUPPORTED_NAMESPACE_WIT_VERSION}`"
                );
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RequestValidationError {}

/// One extension package's declared identity inside a component request.
///
/// Named fields keep the seven same-typed values from being swapped at a call
/// site; [`ComponentRequestBoundary::add_package`] bounds and checks them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageIdentity<'a> {
    /// Exact namespace URI the package binds.
    pub namespace_uri: &'a str,
    /// Package version.
    pub package_version: &'a str,
    /// Extension component Wasm artifact reference.
    pub extension_artifact: &'a str,
    /// Schema artifact reference.
    pub schema_artifact: &'a str,
    /// Lowercase hex SHA-256 of the schema artifact.
    pub schema_sha256: &'a str,
    /// WIT world the extension component implements.
    pub wit_world: &'a str,
    /// WIT package version of that world.
    pub wit_version: &'a str,
}

/// Incremental request validator used before resolver or package construction.
///
/// The caller supplies every schema and package metadata field through the
/// corresponding methods, then calls [`Self::finish`]. No untrusted field value
/// is copied into an error or telemetry attribute.
#[derive(Debug)]
pub struct ComponentRequestBoundary {
    schema_namespaces: BTreeSet<String>,
    package_namespaces: BTreeSet<String>,
    total_schema_bytes: usize,
    total_metadata_bytes: usize,
    total_request_bytes: usize,
}

impl ComponentRequestBoundary {
    /// Validates fixed request fields and declared collection counts.
    ///
    /// # Errors
    ///
    /// Returns a stable bounded error when source, metadata, or counts exceed
    /// their limits.
    pub fn new(
        source_name: &str,
        source: &str,
        schema_count: usize,
        package_count: usize,
    ) -> Result<Self, RequestValidationError> {
        Self::new_inner(source_name, source, schema_count, package_count)
    }

    fn new_inner(
        source_name: &str,
        source: &str,
        schema_count: usize,
        package_count: usize,
    ) -> Result<Self, RequestValidationError> {
        if source.len() > MAX_COMPONENT_SOURCE_BYTES {
            return Err(RequestValidationError::SourceTooLarge);
        }
        if schema_count > MAX_COMPONENT_SCHEMA_COUNT {
            return Err(RequestValidationError::TooManySchemas);
        }
        if package_count > MAX_COMPONENT_PACKAGE_COUNT {
            return Err(RequestValidationError::TooManyPackages);
        }
        let mut boundary = Self {
            schema_namespaces: BTreeSet::new(),
            package_namespaces: BTreeSet::new(),
            total_schema_bytes: 0,
            total_metadata_bytes: 0,
            total_request_bytes: source.len(),
        };
        boundary.add_metadata("source-name", source_name)?;
        Ok(boundary)
    }

    /// Validates one exact-namespace schema resource.
    ///
    /// # Errors
    ///
    /// Returns a stable bounded error for invalid metadata, duplicate exact
    /// namespace identity, or individual/aggregate byte limit violations.
    pub fn add_schema(
        &mut self,
        namespace_uri: &str,
        system_id: &str,
        bytes: &[u8],
    ) -> Result<(), RequestValidationError> {
        self.add_schema_inner(namespace_uri, system_id, bytes)
    }

    /// Validates one optional request metadata field when present.
    ///
    /// # Errors
    ///
    /// Returns a stable bounded error when the supplied field is empty or
    /// exceeds individual or aggregate metadata/request byte limits.
    pub fn add_optional_metadata(
        &mut self,
        field: &'static str,
        value: Option<&str>,
    ) -> Result<(), RequestValidationError> {
        value.map_or(Ok(()), |value| self.add_metadata(field, value))
    }

    fn add_schema_inner(
        &mut self,
        namespace_uri: &str,
        system_id: &str,
        bytes: &[u8],
    ) -> Result<(), RequestValidationError> {
        self.add_metadata("schema.namespace-uri", namespace_uri)?;
        self.add_metadata("schema.system-id", system_id)?;
        if bytes.len() > MAX_COMPONENT_SCHEMA_BYTES {
            return Err(RequestValidationError::SchemaTooLarge);
        }
        self.total_schema_bytes = self
            .total_schema_bytes
            .checked_add(bytes.len())
            .ok_or(RequestValidationError::TotalSchemaBytesTooLarge)?;
        if self.total_schema_bytes > MAX_COMPONENT_TOTAL_SCHEMA_BYTES {
            return Err(RequestValidationError::TotalSchemaBytesTooLarge);
        }
        self.add_request_bytes(bytes.len())?;
        if !self.schema_namespaces.insert(namespace_uri.to_owned()) {
            return Err(RequestValidationError::DuplicateSchemaNamespace);
        }
        Ok(())
    }

    /// Validates one extension package's exact identity and metadata fields.
    ///
    /// # Errors
    ///
    /// Returns a stable bounded error for empty/oversized metadata, duplicate
    /// exact namespace identity, or aggregate request limit violations.
    pub fn add_package(
        &mut self,
        package: &PackageIdentity<'_>,
    ) -> Result<(), RequestValidationError> {
        for (field, value) in [
            ("package.namespace-uri", package.namespace_uri),
            ("package.package-version", package.package_version),
            ("package.extension-artifact", package.extension_artifact),
            ("package.schema-artifact", package.schema_artifact),
            ("package.schema-sha256", package.schema_sha256),
            ("package.wit-world", package.wit_world),
            ("package.wit-version", package.wit_version),
        ] {
            self.add_metadata(field, value)?;
        }
        if package.wit_world != SUPPORTED_NAMESPACE_WIT_WORLD {
            return Err(RequestValidationError::UnsupportedNamespaceWitWorld);
        }
        if package.wit_version != SUPPORTED_NAMESPACE_WIT_VERSION {
            return Err(RequestValidationError::UnsupportedNamespaceWitVersion);
        }
        if !self
            .package_namespaces
            .insert(package.namespace_uri.to_owned())
        {
            return Err(RequestValidationError::DuplicatePackageNamespace);
        }
        Ok(())
    }

    /// Marks successful completion after every declared entry was checked.
    ///
    /// # Errors
    ///
    /// Returns a stable bounded error if the final aggregate exceeds its cap.
    pub fn finish(self) -> Result<(), RequestValidationError> {
        if self.total_request_bytes > MAX_COMPONENT_REQUEST_BYTES {
            Err(RequestValidationError::RequestBytesTooLarge)
        } else {
            Ok(())
        }
    }

    fn add_metadata(
        &mut self,
        field: &'static str,
        value: &str,
    ) -> Result<(), RequestValidationError> {
        if value.is_empty() {
            return Err(RequestValidationError::EmptyMetadata { field });
        }
        if value.len() > MAX_COMPONENT_METADATA_BYTES {
            return Err(RequestValidationError::MetadataTooLarge { field });
        }
        self.total_metadata_bytes = self
            .total_metadata_bytes
            .checked_add(value.len())
            .ok_or(RequestValidationError::TotalMetadataBytesTooLarge)?;
        if self.total_metadata_bytes > MAX_COMPONENT_TOTAL_METADATA_BYTES {
            return Err(RequestValidationError::TotalMetadataBytesTooLarge);
        }
        self.add_request_bytes(value.len())
    }

    fn add_request_bytes(&mut self, bytes: usize) -> Result<(), RequestValidationError> {
        self.total_request_bytes = self
            .total_request_bytes
            .checked_add(bytes)
            .ok_or(RequestValidationError::RequestBytesTooLarge)?;
        if self.total_request_bytes > MAX_COMPONENT_REQUEST_BYTES {
            return Err(RequestValidationError::RequestBytesTooLarge);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_syntax::telemetry::{Recorder, install_recorder};

    fn boundary() -> ComponentRequestBoundary {
        ComponentRequestBoundary::new("machine.scxml", "<scxml/>", 2, 2)
            .expect("small request prelude must be accepted")
    }

    #[test]
    fn accepts_bounded_unique_request() {
        let mut request = boundary();
        request
            .add_schema("urn:a", "a.xsd", b"schema")
            .expect("bounded schema must be accepted");
        request
            .add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "1",
                extension_artifact: "a.wasm",
                schema_artifact: "a.xsd",
                schema_sha256: "00",
                wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
                wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            })
            .expect("bounded package must be accepted");
        assert_eq!(request.finish(), Ok(()));
    }

    #[test]
    fn rejects_duplicate_exact_schema_namespace() {
        let mut request = boundary();
        request
            .add_schema("urn:a", "a.xsd", b"schema")
            .expect("first exact schema identity must be accepted");
        assert_eq!(
            request.add_schema("urn:a", "other.xsd", b"other"),
            Err(RequestValidationError::DuplicateSchemaNamespace)
        );
    }

    #[test]
    fn preserves_exact_uri_distinctions() {
        let mut request = boundary();
        request
            .add_schema("urn:a", "a.xsd", b"schema")
            .expect("first exact schema identity must be accepted");
        request
            .add_schema("urn:a/", "a-slash.xsd", b"schema")
            .expect("different exact URI must remain distinct");
    }

    #[test]
    fn rejects_duplicate_package_namespace_identity() {
        let mut request = boundary();
        request
            .add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "1",
                extension_artifact: "a.wasm",
                schema_artifact: "a.xsd",
                schema_sha256: "00",
                wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
                wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            })
            .expect("first package identity must be accepted");
        assert_eq!(
            request.add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "2",
                extension_artifact: "b.wasm",
                schema_artifact: "b.xsd",
                schema_sha256: "11",
                wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
                wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            }),
            Err(RequestValidationError::DuplicatePackageNamespace)
        );
    }

    #[test]
    fn rejects_unsupported_package_wit_identity_with_exact_diagnostics() {
        let mut world = boundary();
        assert_eq!(
            world.add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "1",
                extension_artifact: "a.wasm",
                schema_artifact: "a.xsd",
                schema_sha256: "00",
                wit_world: "stateforward:scxml/other",
                wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            }),
            Err(RequestValidationError::UnsupportedNamespaceWitWorld)
        );
        assert_eq!(
            RequestValidationError::UnsupportedNamespaceWitWorld.to_string(),
            format!("request package WIT world must equal `{SUPPORTED_NAMESPACE_WIT_WORLD}`")
        );

        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let mut version = boundary();
        assert_eq!(
            version.add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "1",
                extension_artifact: "a.wasm",
                schema_artifact: "a.xsd",
                schema_sha256: "00",
                wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
                wit_version: "0.3.0",
            }),
            Err(RequestValidationError::UnsupportedNamespaceWitVersion)
        );
        assert_eq!(
            RequestValidationError::UnsupportedNamespaceWitVersion.to_string(),
            format!("request package WIT version must equal `{SUPPORTED_NAMESPACE_WIT_VERSION}`")
        );
        assert!(
            recorder.observations().is_empty(),
            "request bound checks run inside the component export span and record nothing of their own"
        );
    }

    #[test]
    fn bounds_package_wit_identity_before_compatibility_checks() {
        let oversized = "x".repeat(MAX_COMPONENT_METADATA_BYTES + 1);
        let mut world = boundary();
        assert_eq!(
            world.add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "1",
                extension_artifact: "a.wasm",
                schema_artifact: "a.xsd",
                schema_sha256: "00",
                wit_world: &oversized,
                wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            }),
            Err(RequestValidationError::MetadataTooLarge {
                field: "package.wit-world"
            })
        );

        let mut version = boundary();
        assert_eq!(
            version.add_package(&PackageIdentity {
                namespace_uri: "urn:a",
                package_version: "1",
                extension_artifact: "a.wasm",
                schema_artifact: "a.xsd",
                schema_sha256: "00",
                wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
                wit_version: &oversized,
            }),
            Err(RequestValidationError::MetadataTooLarge {
                field: "package.wit-version"
            })
        );
    }

    #[test]
    fn rejects_each_untrusted_collection_and_byte_bound() {
        assert_eq!(
            ComponentRequestBoundary::new(
                "machine.scxml",
                &"x".repeat(MAX_COMPONENT_SOURCE_BYTES + 1),
                0,
                0,
            )
            .expect_err("oversized source must be rejected"),
            RequestValidationError::SourceTooLarge
        );
        assert_eq!(
            ComponentRequestBoundary::new(
                "machine.scxml",
                "<scxml/>",
                MAX_COMPONENT_SCHEMA_COUNT + 1,
                0,
            )
            .expect_err("oversized schema count must be rejected"),
            RequestValidationError::TooManySchemas
        );
        assert_eq!(
            ComponentRequestBoundary::new(
                "machine.scxml",
                "<scxml/>",
                0,
                MAX_COMPONENT_PACKAGE_COUNT + 1,
            )
            .expect_err("oversized package count must be rejected"),
            RequestValidationError::TooManyPackages
        );
        let mut request = boundary();
        assert_eq!(
            request.add_schema("urn:a", "a.xsd", &vec![0; MAX_COMPONENT_SCHEMA_BYTES + 1],),
            Err(RequestValidationError::SchemaTooLarge)
        );
        let oversized = "x".repeat(MAX_COMPONENT_METADATA_BYTES + 1);
        assert_eq!(
            ComponentRequestBoundary::new(&oversized, "<scxml/>", 0, 0)
                .expect_err("oversized source name must be rejected"),
            RequestValidationError::MetadataTooLarge {
                field: "source-name"
            }
        );
    }

    #[test]
    fn rejects_aggregate_schema_metadata_and_request_bytes() {
        let mut schemas = ComponentRequestBoundary::new("m", "x", 5, 0)
            .expect("small request prelude must be accepted");
        for index in 0..4 {
            schemas
                .add_schema(
                    &format!("urn:{index}"),
                    &format!("{index}.xsd"),
                    &vec![0; MAX_COMPONENT_SCHEMA_BYTES],
                )
                .expect("schemas through aggregate limit must be accepted");
        }
        assert_eq!(
            schemas.add_schema("urn:overflow", "overflow.xsd", b"x"),
            Err(RequestValidationError::TotalSchemaBytesTooLarge)
        );

        let mut metadata = ComponentRequestBoundary::new("m", "x", 0, 64)
            .expect("small request prelude must be accepted");
        let large = "x".repeat(MAX_COMPONENT_METADATA_BYTES);
        let mut rejected = None;
        for index in 0..64 {
            let prefix = format!("urn:{index}:");
            let namespace = format!(
                "{prefix}{}",
                "x".repeat(MAX_COMPONENT_METADATA_BYTES - prefix.len())
            );
            match metadata.add_package(&PackageIdentity {
                namespace_uri: &namespace,
                package_version: &large,
                extension_artifact: &large,
                schema_artifact: &large,
                schema_sha256: &large,
                wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
                wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            }) {
                Ok(()) => {}
                Err(error) => {
                    rejected = Some(error);
                    break;
                }
            }
        }
        assert_eq!(
            rejected,
            Some(RequestValidationError::TotalMetadataBytesTooLarge)
        );

        let mut aggregate =
            ComponentRequestBoundary::new("m", &"x".repeat(MAX_COMPONENT_SOURCE_BYTES), 4, 0)
                .expect("maximum source alone must be accepted");
        for index in 0..3 {
            aggregate
                .add_schema(
                    &format!("urn:{index}"),
                    &format!("{index}.xsd"),
                    &vec![0; MAX_COMPONENT_SCHEMA_BYTES],
                )
                .expect("request bytes below aggregate limit must be accepted");
        }
        assert_eq!(
            aggregate.add_schema(
                "urn:request-overflow",
                "overflow.xsd",
                &vec![0; MAX_COMPONENT_SCHEMA_BYTES],
            ),
            Err(RequestValidationError::RequestBytesTooLarge)
        );
    }
}
