//! Event I/O Processor package manifests
//! (`stateforward.scxml.processor-package/v1`).
//!
//! A processor package binds one exact Event I/O Processor type URI (SCXML
//! 6.2 `<send type>`), and the short aliases a document may write in its
//! place, to the Wasm component that implements it. It is declarative data:
//! hosts bind its component and grant its declared origins to the
//! component's `outbound-http` requests and `outbound-websocket`
//! connections.

use crate::schema::{
    MAX_EXTENSION_PACKAGE_FIELD_BYTES, MAX_EXTENSION_PACKAGE_LIST_ITEMS, is_http_origin,
};

/// The only processor package manifest format (its `format` field) this
/// release reads.
pub const PROCESSOR_PACKAGE_FORMAT: &str = "stateforward.scxml.processor-package/v1";
/// Exact WIT world an Event I/O Processor component implements.
pub const SUPPORTED_PROCESSOR_WIT_WORLD: &str = "stateforward:scxml/event-processor";
/// Exact WIT package version supported for Event I/O Processor components.
pub const SUPPORTED_PROCESSOR_WIT_VERSION: &str = crate::schema::SUPPORTED_NAMESPACE_WIT_VERSION;
/// Maximum aliases one processor package declares.
pub const MAX_PROCESSOR_ALIASES: usize = 8;
/// Maximum bytes of one alias.
pub const MAX_PROCESSOR_ALIAS_BYTES: usize = 64;
/// Manifest fields in their canonical order (`format` first); every field
/// is required.
pub const PROCESSOR_PACKAGE_FIELDS: [&str; 8] = [
    "format",
    "processor-uri",
    "aliases",
    "package-version",
    "processor-artifact",
    "wit-world",
    "wit-version",
    "origins",
];

/// Declarative package binding for one Event I/O Processor type URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessorPackageManifest {
    /// Exact processor type URI: an absolute URI that names neither the
    /// SCXML nor the `BasicHTTP` processor.
    pub processor_uri: String,
    /// Short names a `<send type>` may write in place of the URI.
    pub aliases: Vec<String>,
    /// Package version selected by the composition root.
    pub package_version: String,
    /// Processor component artifact identifier (relative to the manifest).
    pub processor_artifact: String,
    /// WIT world the component implements
    /// ([`SUPPORTED_PROCESSOR_WIT_WORLD`]).
    pub wit_world: String,
    /// WIT package version ([`SUPPORTED_PROCESSOR_WIT_VERSION`]).
    pub wit_version: String,
    /// The origins or URL prefixes the component's requests and
    /// connections may reach; binding the package consents to them.
    pub origins: Vec<String>,
}

/// Whether `alias` is a short processor name: 1 to 64 bytes of ASCII
/// letters, digits, `.`, `_`, and `-`.
#[must_use]
pub fn is_processor_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= MAX_PROCESSOR_ALIAS_BYTES
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl ProcessorPackageManifest {
    /// Parses one [`PROCESSOR_PACKAGE_FORMAT`] manifest: a JSON object with
    /// exactly the manifest fields ([`PROCESSOR_PACKAGE_FIELDS`]; any other
    /// field is rejected), every value a string except `aliases` and
    /// `origins` (arrays of at most [`MAX_EXTENSION_PACKAGE_LIST_ITEMS`]
    /// strings), and `format` equal to [`PROCESSOR_PACKAGE_FORMAT`]. The
    /// other entries are judged by [`Self::verify`]. Deterministic and
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
            .find(|key| !PROCESSOR_PACKAGE_FIELDS.contains(&key.as_str()))
        {
            return Err(format!(
                "unknown field `{}` (the fields are {})",
                unknown.escape_debug(),
                PROCESSOR_PACKAGE_FIELDS.join(", ")
            ));
        }
        let field = |name: &str| -> Result<String, String> {
            match object.get(name) {
                Some(serde_json::Value::String(value)) => Ok(value.clone()),
                Some(_) => Err(format!("field `{name}` must be a string")),
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
        if field("format")? != PROCESSOR_PACKAGE_FORMAT {
            return Err(format!(
                "field `format` must be `{PROCESSOR_PACKAGE_FORMAT}`"
            ));
        }
        Ok(Self {
            processor_uri: field("processor-uri")?,
            aliases: list("aliases")?,
            package_version: field("package-version")?,
            processor_artifact: field("processor-artifact")?,
            wit_world: field("wit-world")?,
            wit_version: field("wit-version")?,
            origins: list("origins")?,
        })
    }

    /// The manifest as canonical JSON: the fields in
    /// [`PROCESSOR_PACKAGE_FIELDS`] order, compact. Parsing it with
    /// [`Self::from_json`] yields `self`.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let text = |value: &str| serde_json::Value::String(value.to_owned()).to_string();
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
            "{{\"format\":{},\"processor-uri\":{},\"aliases\":{},\"package-version\":{},\"processor-artifact\":{},\"wit-world\":{},\"wit-version\":{},\"origins\":{}}}",
            text(PROCESSOR_PACKAGE_FORMAT),
            text(&self.processor_uri),
            list(&self.aliases),
            text(&self.package_version),
            text(&self.processor_artifact),
            text(&self.wit_world),
            text(&self.wit_version),
            list(&self.origins),
        )
    }

    /// Verifies bounded identity, the supported WIT identity, and the
    /// declared aliases and origins. Deterministic and side-effect-free; it
    /// never reads or instantiates the processor artifact.
    ///
    /// # Errors
    ///
    /// A stable message naming the offending field, never its value: an
    /// empty or oversized field, a `processor-uri` that is no absolute URI
    /// or names the SCXML or `BasicHTTP` processor, an invalid, built-in, or
    /// repeated alias, more than [`MAX_PROCESSOR_ALIASES`] aliases, an
    /// invalid or repeated origin, or an unsupported WIT identity.
    pub fn verify(&self) -> Result<(), String> {
        for (name, value) in [
            ("processor-uri", self.processor_uri.as_str()),
            ("package-version", self.package_version.as_str()),
            ("processor-artifact", self.processor_artifact.as_str()),
            ("wit-world", self.wit_world.as_str()),
            ("wit-version", self.wit_version.as_str()),
        ] {
            if value.is_empty() {
                return Err(format!(
                    "processor package field `{name}` must not be empty"
                ));
            }
            if value.len() > MAX_EXTENSION_PACKAGE_FIELD_BYTES {
                return Err(format!(
                    "processor package field `{name}` exceeds {MAX_EXTENSION_PACKAGE_FIELD_BYTES} bytes"
                ));
            }
        }
        let builtin = |name: &str| {
            scxml_types::classify_processor_type(name) != scxml_types::ProcessorKind::Unknown
        };
        if !scxml_types::is_service_type_uri(&self.processor_uri) || builtin(&self.processor_uri) {
            return Err(
                "processor package field `processor-uri` must be an absolute URI that names neither the SCXML nor the BasicHTTP processor"
                    .to_owned(),
            );
        }
        if self.aliases.len() > MAX_PROCESSOR_ALIASES {
            return Err(format!(
                "processor package field `aliases` lists more than {MAX_PROCESSOR_ALIASES} entries"
            ));
        }
        for (index, alias) in self.aliases.iter().enumerate() {
            if !is_processor_alias(alias) || builtin(alias) {
                return Err("processor package field `aliases` has an invalid entry".to_owned());
            }
            if self
                .aliases
                .get(..index)
                .is_some_and(|before| before.contains(alias))
            {
                return Err("processor package field `aliases` lists an entry twice".to_owned());
            }
        }
        if self.origins.len() > MAX_EXTENSION_PACKAGE_LIST_ITEMS {
            return Err(format!(
                "processor package field `origins` lists more than {MAX_EXTENSION_PACKAGE_LIST_ITEMS} entries"
            ));
        }
        for (index, origin) in self.origins.iter().enumerate() {
            if !is_http_origin(origin) {
                return Err("processor package field `origins` has an invalid entry".to_owned());
            }
            if self
                .origins
                .get(..index)
                .is_some_and(|before| before.contains(origin))
            {
                return Err("processor package field `origins` lists an entry twice".to_owned());
            }
        }
        if self.wit_world != SUPPORTED_PROCESSOR_WIT_WORLD {
            return Err(format!(
                "processor package WIT world must equal `{SUPPORTED_PROCESSOR_WIT_WORLD}`"
            ));
        }
        if self.wit_version != SUPPORTED_PROCESSOR_WIT_VERSION {
            return Err(format!(
                "processor package WIT version must equal `{SUPPORTED_PROCESSOR_WIT_VERSION}`"
            ));
        }
        Ok(())
    }

    /// Whether `processor_type` (a `<send type>` value) names this package:
    /// its URI or one of its aliases, compared byte for byte.
    #[must_use]
    pub fn answers(&self, processor_type: &str) -> bool {
        self.processor_uri == processor_type
            || self.aliases.iter().any(|alias| alias == processor_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn websocket() -> ProcessorPackageManifest {
        ProcessorPackageManifest {
            processor_uri: "https://github.com/stateforward/scxml.rs/processors/websocket".into(),
            aliases: vec!["websocket".into()],
            package_version: "0.2.0".into(),
            processor_artifact: "scxml_ioprocessor_websocket.virtualized.wasm".into(),
            wit_world: SUPPORTED_PROCESSOR_WIT_WORLD.into(),
            wit_version: SUPPORTED_PROCESSOR_WIT_VERSION.into(),
            origins: Vec::new(),
        }
    }

    #[test]
    fn the_checked_in_websocket_manifest_parses_verifies_and_is_canonical() {
        let bytes = include_bytes!(
            "../../../components/scxml-ioprocessor-websocket/package/websocket.processor.json"
        );
        let manifest = ProcessorPackageManifest::from_json(bytes).expect("parses");
        assert_eq!(manifest, websocket());
        assert_eq!(manifest.verify(), Ok(()));
        let canonical = manifest.to_canonical_json();
        assert_eq!(
            ProcessorPackageManifest::from_json(canonical.as_bytes()).expect("round trips"),
            manifest
        );
        assert!(manifest.answers("websocket"));
        assert!(manifest.answers("https://github.com/stateforward/scxml.rs/processors/websocket"));
        assert!(!manifest.answers("WebSocket"));
    }

    #[test]
    fn unknown_missing_and_mistyped_fields_fail_closed() {
        let canonical = websocket().to_canonical_json();
        let extra = canonical.replacen('{', "{\"extra\":1,", 1);
        assert!(
            ProcessorPackageManifest::from_json(extra.as_bytes())
                .expect_err("unknown field")
                .contains("unknown field `extra`")
        );
        let missing = canonical.replace(",\"origins\":[]", "");
        assert!(ProcessorPackageManifest::from_json(missing.as_bytes()).is_err());
        let wrong = canonical.replace(
            PROCESSOR_PACKAGE_FORMAT,
            "stateforward.scxml.processor-package/v2",
        );
        assert!(ProcessorPackageManifest::from_json(wrong.as_bytes()).is_err());
        let typed = canonical.replace("\"aliases\":[\"websocket\"]", "\"aliases\":\"websocket\"");
        assert!(ProcessorPackageManifest::from_json(typed.as_bytes()).is_err());
    }

    #[test]
    fn verification_binds_identity_aliases_and_origins() {
        for uri in [
            "basichttp",
            "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor",
            "http://www.w3.org/TR/scxml/#SCXMLEventProcessor",
            "not a uri",
        ] {
            let mut manifest = websocket();
            manifest.processor_uri = uri.into();
            assert!(manifest.verify().is_err(), "{uri}");
        }
        for aliases in [
            vec!["scxml".to_owned()],
            vec!["basichttp".to_owned()],
            vec!["a b".to_owned()],
            vec![String::new()],
            vec!["ws".to_owned(), "ws".to_owned()],
            vec!["a".to_owned(); MAX_PROCESSOR_ALIASES + 1],
        ] {
            let mut manifest = websocket();
            manifest.aliases = aliases.clone();
            assert!(manifest.verify().is_err(), "{aliases:?}");
        }
        let mut origins = websocket();
        origins.origins = vec!["https://api.example.com".into()];
        assert_eq!(origins.verify(), Ok(()));
        origins.origins.push("https://api.example.com".into());
        assert!(origins.verify().is_err());
        let mut world = websocket();
        world.wit_world = "stateforward:scxml/invoked-service".into();
        assert!(world.verify().is_err());
    }
}
