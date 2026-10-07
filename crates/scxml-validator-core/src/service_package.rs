//! Invoked-service package manifests (`stateforward.scxml.service-package/v1`).
//!
//! A service package binds one exact invoked-service type URI (SCXML 6.4
//! `<invoke type>`) to the Wasm component that implements it. Like an
//! extension package manifest it is declarative data: the validator and the
//! compiler read only its URI and `env` slots (`SCXML094`, `SCXML083`); hosts
//! bind its component and grant its declared outbound HTTP origins and `env`
//! slots. The WASI interfaces the component may use are its own imports,
//! which the embedder reads from the component itself.

use sha2::{Digest, Sha256};

use crate::schema::{
    MAX_EXTENSION_PACKAGE_FIELD_BYTES, MAX_EXTENSION_PACKAGE_LIST_ITEMS, is_env_slot,
    is_http_origin,
};

/// The only service package manifest format (its `format` field) this
/// release reads.
pub const SERVICE_PACKAGE_FORMAT: &str = "stateforward.scxml.service-package/v1";
/// Exact WIT world an invoked-service component implements.
pub const SUPPORTED_SERVICE_WIT_WORLD: &str = "stateforward:scxml/invoked-service";
/// Exact WIT package version supported for invoked-service components.
pub const SUPPORTED_SERVICE_WIT_VERSION: &str = crate::schema::SUPPORTED_NAMESPACE_WIT_VERSION;
/// Manifest fields in their canonical order (`format` first); every field is
/// required except the last two ([`crate::VALIDATOR_FIELDS`]), which are
/// optional.
pub const SERVICE_PACKAGE_FIELDS: [&str; 12] = [
    "format",
    "service-uri",
    "package-version",
    "service-artifact",
    "schema-artifact",
    "schema-sha256",
    "wit-world",
    "wit-version",
    "http-origins",
    "env-slots",
    "validator-artifact",
    "validator-sha256",
];

/// Declarative package binding for one invoked-service type URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServicePackageManifest {
    /// Exact service type URI ([`scxml_types::is_service_type_uri`]).
    pub service_uri: String,
    /// Package version selected by the composition root.
    pub package_version: String,
    /// Service component artifact identifier (relative to the manifest).
    pub service_artifact: String,
    /// Optional schema artifact identifier (relative to the manifest);
    /// present exactly when `schema_sha256` is.
    pub schema_artifact: Option<String>,
    /// Lowercase SHA-256 of the schema bytes; present exactly when
    /// `schema_artifact` is.
    pub schema_sha256: Option<String>,
    /// WIT world the component implements
    /// ([`SUPPORTED_SERVICE_WIT_WORLD`]).
    pub wit_world: String,
    /// WIT package version ([`SUPPORTED_SERVICE_WIT_VERSION`]).
    pub wit_version: String,
    /// The origins or URL prefixes the component's `outbound-http` requests
    /// may reach; binding the package consents to them.
    pub http_origins: Vec<String>,
    /// The `env` slots the service accepts.
    pub env_slots: Vec<String>,
    /// Optional package validator component artifact identifier (relative
    /// to the manifest); present exactly when `validator_sha256` is.
    pub validator_artifact: Option<String>,
    /// Lowercase SHA-256 of the package validator component; present
    /// exactly when `validator_artifact` is.
    pub validator_sha256: Option<String>,
}

impl ServicePackageManifest {
    /// Parses one [`SERVICE_PACKAGE_FORMAT`] manifest: a JSON object with
    /// exactly the manifest fields ([`SERVICE_PACKAGE_FIELDS`]; any other
    /// field is rejected), every value a string except `schema-artifact` and
    /// `schema-sha256` (each a string or `null`) and `http-origins` and
    /// `env-slots` (arrays of at most [`MAX_EXTENSION_PACKAGE_LIST_ITEMS`]
    /// strings), and `format` equal to [`SERVICE_PACKAGE_FORMAT`]. The other
    /// entries are judged by [`Self::verify`]. Deterministic and
    /// side-effect-free.
    ///
    /// # Errors
    ///
    /// A message naming the offending field, never its value.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "it is not JSON".to_owned())?;
        let serde_json::Value::Object(object) = value else {
            return Err("it is not a JSON object".into());
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| !SERVICE_PACKAGE_FIELDS.contains(&key.as_str()))
        {
            return Err(format!(
                "unknown field `{}` (the fields are {})",
                unknown.escape_debug(),
                SERVICE_PACKAGE_FIELDS.join(", ")
            ));
        }
        let field = |name: &str| -> Result<String, String> {
            match object.get(name) {
                Some(serde_json::Value::String(value)) => Ok(value.clone()),
                Some(_) => Err(format!("field `{name}` must be a string")),
                None => Err(format!("missing field `{name}`")),
            }
        };
        let nullable = |name: &str| -> Result<Option<String>, String> {
            match object.get(name) {
                Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
                Some(serde_json::Value::Null) => Ok(None),
                Some(_) => Err(format!("field `{name}` must be a string or null")),
                None => Err(format!("missing field `{name}`")),
            }
        };
        let list = |name: &str| -> Result<Vec<String>, String> {
            match object.get(name) {
                Some(serde_json::Value::Array(items))
                    if items.len() <= MAX_EXTENSION_PACKAGE_LIST_ITEMS =>
                {
                    items
                        .iter()
                        .map(|item| match item {
                            serde_json::Value::String(text) => Ok(text.clone()),
                            _ => Err(format!("field `{name}` must be an array of strings")),
                        })
                        .collect()
                }
                Some(serde_json::Value::Array(_)) => Err(format!(
                    "field `{name}` lists more than {MAX_EXTENSION_PACKAGE_LIST_ITEMS} entries"
                )),
                Some(_) => Err(format!("field `{name}` must be an array of strings")),
                None => Err(format!("missing field `{name}`")),
            }
        };
        if field("format")? != SERVICE_PACKAGE_FORMAT {
            return Err(format!("field `format` must be `{SERVICE_PACKAGE_FORMAT}`"));
        }
        Ok(Self {
            service_uri: field("service-uri")?,
            package_version: field("package-version")?,
            service_artifact: field("service-artifact")?,
            schema_artifact: nullable("schema-artifact")?,
            schema_sha256: nullable("schema-sha256")?,
            wit_world: field("wit-world")?,
            wit_version: field("wit-version")?,
            http_origins: list("http-origins")?,
            env_slots: list("env-slots")?,
            validator_artifact: crate::schema::optional_field(&object, "validator-artifact")?,
            validator_sha256: crate::schema::optional_field(&object, "validator-sha256")?,
        })
    }

    /// The manifest as canonical JSON: the fields in
    /// [`SERVICE_PACKAGE_FIELDS`] order, compact, `null` for an absent
    /// schema. Parsing it with [`Self::from_json`] yields `self`.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let text = |value: &str| serde_json::Value::String(value.to_owned()).to_string();
        let nullable = |value: Option<&str>| value.map_or_else(|| "null".to_owned(), text);
        let list = |items: &[String]| {
            serde_json::Value::Array(
                items
                    .iter()
                    .map(|item| serde_json::Value::String(item.clone()))
                    .collect(),
            )
            .to_string()
        };
        format!(
            "{{\"format\":{},\"service-uri\":{},\"package-version\":{},\"service-artifact\":{},\"schema-artifact\":{},\"schema-sha256\":{},\"wit-world\":{},\"wit-version\":{},\"http-origins\":{},\"env-slots\":{}",
            text(SERVICE_PACKAGE_FORMAT),
            text(&self.service_uri),
            text(&self.package_version),
            text(&self.service_artifact),
            nullable(self.schema_artifact.as_deref()),
            nullable(self.schema_sha256.as_deref()),
            text(&self.wit_world),
            text(&self.wit_version),
            list(&self.http_origins),
            list(&self.env_slots),
        ) + &match (&self.validator_artifact, &self.validator_sha256) {
            (Some(artifact), Some(sha256)) => format!(
                ",\"validator-artifact\":{},\"validator-sha256\":{}}}",
                text(artifact),
                text(sha256)
            ),
            _ => "}".to_owned(),
        }
    }

    /// Verifies bounded identity, the supported WIT identity, the declared
    /// lists, and, when the manifest names a schema, its integrity against
    /// `schema` (its bytes, which the caller read relative to the manifest).
    /// Deterministic and side-effect-free; it never reads or instantiates
    /// the service artifact.
    ///
    /// # Errors
    ///
    /// A stable message naming the offending field, never its value: an
    /// empty or oversized field, a `service-uri` that is no service type
    /// URI, `schema-artifact` and `schema-sha256` not both present or both
    /// absent, a schema whose bytes are missing or do not hash to
    /// `schema-sha256`, schema bytes for a manifest without a schema, an
    /// unsupported WIT identity, or an invalid or repeated list entry.
    pub fn verify(&self, schema: Option<&[u8]>) -> Result<(), String> {
        let schema_fields: [(&str, Option<&str>); 2] = [
            ("schema-artifact", self.schema_artifact.as_deref()),
            ("schema-sha256", self.schema_sha256.as_deref()),
        ];
        for (name, value) in [
            ("service-uri", Some(self.service_uri.as_str())),
            ("package-version", Some(self.package_version.as_str())),
            ("service-artifact", Some(self.service_artifact.as_str())),
            ("wit-world", Some(self.wit_world.as_str())),
            ("wit-version", Some(self.wit_version.as_str())),
        ]
        .into_iter()
        .chain(schema_fields)
        {
            let Some(value) = value else { continue };
            if value.is_empty() {
                return Err(format!("service package field `{name}` must not be empty"));
            }
            if value.len() > MAX_EXTENSION_PACKAGE_FIELD_BYTES {
                return Err(format!(
                    "service package field `{name}` exceeds {MAX_EXTENSION_PACKAGE_FIELD_BYTES} bytes"
                ));
            }
        }
        crate::schema::verify_validator_fields(
            "service",
            self.validator_artifact.as_deref(),
            self.validator_sha256.as_deref(),
        )?;
        if !scxml_types::is_service_type_uri(&self.service_uri) {
            return Err(
                "service package field `service-uri` must be an absolute service type URI that is not the SCXML type"
                    .to_owned(),
            );
        }
        for (name, items, valid) in [
            (
                "env-slots",
                &self.env_slots,
                is_env_slot as fn(&str) -> bool,
            ),
            ("http-origins", &self.http_origins, is_http_origin),
        ] {
            if items.len() > MAX_EXTENSION_PACKAGE_LIST_ITEMS {
                return Err(format!(
                    "service package field `{name}` lists more than {MAX_EXTENSION_PACKAGE_LIST_ITEMS} entries"
                ));
            }
            for (index, item) in items.iter().enumerate() {
                if !valid(item) {
                    return Err(format!(
                        "service package field `{name}` has an invalid entry"
                    ));
                }
                if items
                    .get(..index)
                    .is_some_and(|before| before.contains(item))
                {
                    return Err(format!(
                        "service package field `{name}` lists an entry twice"
                    ));
                }
            }
        }
        if self.wit_world != SUPPORTED_SERVICE_WIT_WORLD {
            return Err(format!(
                "service package WIT world must equal `{SUPPORTED_SERVICE_WIT_WORLD}`"
            ));
        }
        if self.wit_version != SUPPORTED_SERVICE_WIT_VERSION {
            return Err(format!(
                "service package WIT version must equal `{SUPPORTED_SERVICE_WIT_VERSION}`"
            ));
        }
        match (&self.schema_sha256, schema) {
            (Some(_), _) if self.schema_artifact.is_none() => Err(
                "service package fields `schema-artifact` and `schema-sha256` must be both present or both null"
                    .to_owned(),
            ),
            (None, _) if self.schema_artifact.is_some() => Err(
                "service package fields `schema-artifact` and `schema-sha256` must be both present or both null"
                    .to_owned(),
            ),
            (None, Some(_)) => Err("schema bytes were supplied for a service package without a schema".to_owned()),
            (None, None) => Ok(()),
            (Some(_), None) => Err("the service package's schema bytes are missing".to_owned()),
            (Some(expected), Some(bytes)) => {
                if &hex::encode(Sha256::digest(bytes)) == expected {
                    Ok(())
                } else {
                    Err("schema digest mismatch for service package".to_owned())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo() -> ServicePackageManifest {
        ServicePackageManifest {
            service_uri: "https://stateforward.org/scxml/services/echo".into(),
            package_version: "0.2.0".into(),
            service_artifact: "scxml_service_example.virtualized.wasm".into(),
            schema_artifact: None,
            schema_sha256: None,
            wit_world: SUPPORTED_SERVICE_WIT_WORLD.into(),
            wit_version: SUPPORTED_SERVICE_WIT_VERSION.into(),
            http_origins: Vec::new(),
            env_slots: vec!["endpoint".into(), "token".into()],
            validator_artifact: None,
            validator_sha256: None,
        }
    }

    #[test]
    fn the_checked_in_example_manifest_parses_verifies_and_is_canonical() {
        let bytes =
            include_bytes!("../../../components/scxml-service-example/package/echo.service.json");
        let manifest = ServicePackageManifest::from_json(bytes).expect("parses");
        assert_eq!(manifest, echo());
        assert_eq!(manifest.verify(None), Ok(()));
        let canonical = manifest.to_canonical_json();
        assert_eq!(
            ServicePackageManifest::from_json(canonical.as_bytes()).expect("round trips"),
            manifest
        );
        let mut validated = manifest.clone();
        validated.validator_artifact = Some("echo_validator.wasm".into());
        validated.validator_sha256 = Some("ab".repeat(32));
        assert_eq!(validated.verify(None), Ok(()));
        let canonical = validated.to_canonical_json();
        assert_eq!(
            ServicePackageManifest::from_json(canonical.as_bytes()).expect("round trips"),
            validated
        );
        let fields: Vec<usize> = SERVICE_PACKAGE_FIELDS
            .iter()
            .filter_map(|field| canonical.find(&format!("\"{field}\"")))
            .collect();
        assert_eq!(fields.len(), SERVICE_PACKAGE_FIELDS.len());
        assert!(
            fields.windows(2).all(|pair| pair[0] < pair[1]),
            "{canonical}"
        );
    }

    #[test]
    fn unknown_missing_and_mistyped_fields_fail_closed() {
        let canonical = echo().to_canonical_json();
        let extra = canonical.replacen('{', "{\"extra\":1,", 1);
        assert!(
            ServicePackageManifest::from_json(extra.as_bytes())
                .expect_err("unknown field")
                .contains("unknown field `extra`")
        );
        let missing = canonical.replace(",\"env-slots\":[\"endpoint\",\"token\"]", "");
        assert!(ServicePackageManifest::from_json(missing.as_bytes()).is_err());
        let wrong = canonical.replace(
            SERVICE_PACKAGE_FORMAT,
            "stateforward.scxml.service-package/v2",
        );
        assert!(ServicePackageManifest::from_json(wrong.as_bytes()).is_err());
        let typed = canonical.replace("\"schema-sha256\":null", "\"schema-sha256\":1");
        assert!(ServicePackageManifest::from_json(typed.as_bytes()).is_err());
    }

    #[test]
    fn verification_binds_identity_and_the_optional_schema_digest() {
        let mut scxml = echo();
        scxml.service_uri = "http://www.w3.org/TR/scxml/".into();
        assert!(scxml.verify(None).is_err());
        let mut half = echo();
        half.schema_artifact = Some("echo.schema.json".into());
        assert!(half.verify(None).is_err());
        let mut schema = echo();
        schema.schema_artifact = Some("echo.schema.json".into());
        schema.schema_sha256 = Some(hex::encode(Sha256::digest(b"{}")));
        assert_eq!(schema.verify(Some(b"{}")), Ok(()));
        assert!(schema.verify(Some(b"[]")).is_err());
        assert!(schema.verify(None).is_err());
        assert!(echo().verify(Some(b"{}")).is_err());
        let mut world = echo();
        world.wit_world = "stateforward:scxml/executable-content-extension".into();
        assert!(world.verify(None).is_err());
        let mut slots = echo();
        slots.env_slots = vec!["apikey".into(), "apikey".into()];
        assert!(slots.verify(None).is_err());
        let mut validator = echo();
        validator.validator_artifact = Some("v.wasm".into());
        assert!(validator.verify(None).is_err(), "the validator fields pair");
        validator.validator_sha256 = Some("AB".repeat(32));
        assert!(
            validator.verify(None).is_err(),
            "the digest is lowercase hex"
        );
        validator.validator_sha256 = Some("ab".repeat(32));
        assert_eq!(validator.verify(None), Ok(()));
    }
}
