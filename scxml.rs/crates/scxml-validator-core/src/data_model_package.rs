//! Data-model package manifests (`stateforward.scxml.data-model-package/v1`).
//!
//! A data-model package binds one exact data-model package URI (the value
//! of an `<scxml>`'s `datamodel:src`, [`scxml_types::DATAMODEL_NAMESPACE_URI`])
//! and the data model's name (the core `datamodel` token the document
//! writes, SCXML 1.0 §3.2.1) to the Wasm component that implements it
//! (`stateforward:scxml/data-model`, the world the built-in data models
//! implement; its `data-model-api.check-syntax` is also the validator's
//! `expression-syntax` checker). Like an invoked-service package manifest
//! it is declarative data: the validator reads only its URI and name
//! (`PKG005`, `DATAMODEL002`); hosts bind its component by the name.

use crate::schema::MAX_EXTENSION_PACKAGE_FIELD_BYTES;

/// The only data-model package manifest format (its `format` field) this
/// release reads.
pub const DATA_MODEL_PACKAGE_FORMAT: &str = "stateforward.scxml.data-model-package/v1";
/// Exact WIT world a data-model component implements.
pub const SUPPORTED_DATA_MODEL_WIT_WORLD: &str = "stateforward:scxml/data-model";
/// Exact WIT package version supported for data-model components.
pub const SUPPORTED_DATA_MODEL_WIT_VERSION: &str = crate::schema::SUPPORTED_NAMESPACE_WIT_VERSION;
/// Manifest fields in their canonical order (`format` first); every field is
/// required.
pub const DATA_MODEL_PACKAGE_FIELDS: [&str; 7] = [
    "format",
    "data-model-uri",
    "datamodel",
    "package-version",
    "data-model-artifact",
    "wit-world",
    "wit-version",
];

/// Declarative package binding for one data model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataModelPackageManifest {
    /// Exact data-model package URI: an absolute URI of at most
    /// [`scxml_types::MAX_SERVICE_TYPE_URI_BYTES`] bytes, tagless.
    pub data_model_uri: String,
    /// The data model's name, which the document's `datamodel` token must
    /// equal: an XML `NMTOKEN` that is none of the built-in data models
    /// (`ecmascript`, `null`, `xpath`).
    pub datamodel: String,
    /// Package version selected by the composition root.
    pub package_version: String,
    /// Data-model component artifact identifier (relative to the manifest).
    pub data_model_artifact: String,
    /// WIT world the component implements
    /// ([`SUPPORTED_DATA_MODEL_WIT_WORLD`]).
    pub wit_world: String,
    /// WIT package version ([`SUPPORTED_DATA_MODEL_WIT_VERSION`]).
    pub wit_version: String,
}

impl DataModelPackageManifest {
    /// Parses one [`DATA_MODEL_PACKAGE_FORMAT`] manifest: a JSON object with
    /// exactly the manifest fields ([`DATA_MODEL_PACKAGE_FIELDS`]; any other
    /// field is rejected), every value a string, and `format` equal to
    /// [`DATA_MODEL_PACKAGE_FORMAT`]. The other entries are judged by
    /// [`Self::verify`]. Deterministic and side-effect-free.
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
            .find(|key| !DATA_MODEL_PACKAGE_FIELDS.contains(&key.as_str()))
        {
            return Err(format!(
                "unknown field `{}` (the fields are {})",
                unknown.escape_debug(),
                DATA_MODEL_PACKAGE_FIELDS.join(", ")
            ));
        }
        let field = |name: &str| -> Result<String, String> {
            match object.get(name) {
                Some(serde_json::Value::String(value)) => Ok(value.clone()),
                Some(_) => Err(format!("field `{name}` must be a string")),
                None => Err(format!("missing field `{name}`")),
            }
        };
        if field("format")? != DATA_MODEL_PACKAGE_FORMAT {
            return Err(format!(
                "field `format` must be `{DATA_MODEL_PACKAGE_FORMAT}`"
            ));
        }
        Ok(Self {
            data_model_uri: field("data-model-uri")?,
            datamodel: field("datamodel")?,
            package_version: field("package-version")?,
            data_model_artifact: field("data-model-artifact")?,
            wit_world: field("wit-world")?,
            wit_version: field("wit-version")?,
        })
    }

    /// Verifies bounded identity, the data-model URI and name, and the
    /// supported WIT identity. Deterministic and side-effect-free; it never
    /// reads or instantiates the component.
    ///
    /// # Errors
    ///
    /// A stable message naming the offending field, never its value: an
    /// empty or oversized field, a `data-model-uri` that is no absolute URI
    /// or carries a release tag, a `datamodel` that is no `NMTOKEN` or names
    /// a built-in data model, or an unsupported WIT identity.
    pub fn verify(&self) -> Result<(), String> {
        for (name, value) in [
            ("data-model-uri", self.data_model_uri.as_str()),
            ("datamodel", self.datamodel.as_str()),
            ("package-version", self.package_version.as_str()),
            ("data-model-artifact", self.data_model_artifact.as_str()),
            ("wit-world", self.wit_world.as_str()),
            ("wit-version", self.wit_version.as_str()),
        ] {
            if value.is_empty() {
                return Err(format!(
                    "data-model package field `{name}` must not be empty"
                ));
            }
            if value.len() > MAX_EXTENSION_PACKAGE_FIELD_BYTES {
                return Err(format!(
                    "data-model package field `{name}` exceeds {MAX_EXTENSION_PACKAGE_FIELD_BYTES} bytes"
                ));
            }
        }
        if !is_data_model_uri(&self.data_model_uri) {
            return Err(
                "data-model package field `data-model-uri` must be a tagless absolute URI"
                    .to_owned(),
            );
        }
        if !is_nmtoken(&self.datamodel)
            || crate::datamodel_check::KNOWN_DATA_MODELS.contains(&self.datamodel.as_str())
        {
            return Err(
                "data-model package field `datamodel` must be an NMTOKEN other than `ecmascript`, `null`, and `xpath`"
                    .to_owned(),
            );
        }
        if self.wit_world != SUPPORTED_DATA_MODEL_WIT_WORLD {
            return Err(format!(
                "data-model package WIT world must equal `{SUPPORTED_DATA_MODEL_WIT_WORLD}`"
            ));
        }
        if self.wit_version != SUPPORTED_DATA_MODEL_WIT_VERSION {
            return Err(format!(
                "data-model package WIT version must equal `{SUPPORTED_DATA_MODEL_WIT_VERSION}`"
            ));
        }
        Ok(())
    }
}

/// Whether `value` can be a data-model package URI: an absolute URI of at
/// most [`scxml_types::MAX_SERVICE_TYPE_URI_BYTES`] bytes with no release
/// tag ([`scxml_types::strip_tag`] leaves it unchanged).
#[must_use]
pub fn is_data_model_uri(value: &str) -> bool {
    value.len() <= scxml_types::MAX_SERVICE_TYPE_URI_BYTES
        && scxml_types::NamespaceUri::new(value).is_ok()
        && scxml_types::strip_tag(value) == Some((value, None))
}

/// XML 1.0 `Nmtoken` over ASCII name characters and any non-ASCII
/// character (the XML Schema `NMTOKEN` the `datamodel` attribute has).
fn is_nmtoken(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_' | ':')
                || !character.is_ascii()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_example_manifest_parses_and_verifies() {
        let bytes = include_bytes!("../../../datamodels/example/package/example.data-model.json");
        let manifest = DataModelPackageManifest::from_json(bytes).expect("parses");
        assert_eq!(manifest.verify(), Ok(()));
        assert_eq!(manifest.datamodel, "example");
        assert_eq!(
            manifest.data_model_uri,
            "https://github.com/stateforward/scxml.rs/datamodels/example"
        );
    }

    #[test]
    fn unknown_fields_built_in_names_and_pinned_uris_fail_closed() {
        let bytes = include_bytes!("../../../datamodels/example/package/example.data-model.json");
        let text = std::str::from_utf8(bytes).expect("UTF-8");
        let extra = text.replacen('{', "{\"extra\":\"x\",", 1);
        assert!(DataModelPackageManifest::from_json(extra.as_bytes()).is_err());
        let manifest = DataModelPackageManifest::from_json(bytes).expect("parses");
        let mut built_in = manifest.clone();
        built_in.datamodel = "ecmascript".into();
        assert!(built_in.verify().is_err());
        let mut token = manifest.clone();
        token.datamodel = "a b".into();
        assert!(token.verify().is_err());
        let mut pinned = manifest.clone();
        pinned.data_model_uri.push_str("@v1.0.0");
        assert!(pinned.verify().is_err());
        let mut world = manifest;
        world.wit_world = "stateforward:scxml/invoked-service".into();
        assert!(world.verify().is_err());
    }
}
