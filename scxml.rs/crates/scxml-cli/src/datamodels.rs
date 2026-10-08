//! Data-model packages (SCXML 1.0 §5.1 `datamodel`, named by
//! `datamodel:src`, [`scxml_types::DATAMODEL_NAMESPACE_URI`]).
//!
//! * `--data-model-package MANIFEST.json` (repeatable; `validate`,
//!   `compile`, `snapshot run`, and `run`) binds one data model: a
//!   `stateforward.scxml.data-model-package/v1` manifest naming the exact
//!   data-model package URI a `datamodel:src` names, the data model's name
//!   (the `datamodel` token), its version, its component, and its WIT
//!   world and version.
//! * A `datamodel:src` of exactly
//!   `https://github.com/<owner>/<repo>/datamodels/<name>[@<tag>]` that no
//!   `--data-model-package` binds is a repository data model, resolved like
//!   a repository service ([`crate::repository`]): its manifest at the
//!   release's commit when the document is read, and its component from the
//!   release (verified against `SHA256SUMS`) when `snapshot run` or `run`
//!   binds it. Each is resolved once per run for the whole document tree
//!   (the root's pin, else the first document's that names it). A data
//!   model that does not resolve is not bound, so the validator reports it
//!   (`PKG005`) instead of the run failing on the resolution.
//! * `snapshot run` and `run` execute each bound data model's component,
//!   resolved relative to the manifest (`data-model-artifact`) or by
//!   `--data-model-component URI=PATH`, and authenticated by the SHA-256 in
//!   `--data-model-component-sha256 URI=HEX` (required for every package;
//!   the manifest is not a trust anchor). The host binds it by the data
//!   model's name, for every session of the run.
//! * The built-in `ecmascript`, `null`, and `xpath` ship with `scx` and are
//!   never bound here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use scxml_telemetry::{ErrorCategory, Module, Operation, observe};
use scxml_validator_core::DataModelPackageManifest;

use crate::extensions::read_bounded;

/// Where a data-model package came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DataModelOrigin {
    /// A `--data-model-package` manifest file.
    Manifest(PathBuf),
    /// A repository data model, as the resolver resolved it (the tagless
    /// URI, or it with the document's `@<tag>`).
    Repository(String),
}

/// One verified data-model package.
#[derive(Clone, Debug)]
pub(crate) struct DataModelPackage {
    /// Where it came from.
    pub(crate) origin: DataModelOrigin,
    /// The verified manifest.
    pub(crate) manifest: DataModelPackageManifest,
}

/// The data models of one command: the `--data-model-package` ones, then
/// the repository data models the documents name that none of them binds.
#[derive(Debug, Default)]
pub(crate) struct DataModels {
    /// Every package, `--data-model-package` ones first.
    pub(crate) packages: Vec<DataModelPackage>,
    /// The repository store, opened only when a document names a repository
    /// data model.
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "only a run binds repository components, which needs the wasmtime feature"
        )
    )]
    store: Option<std::sync::Arc<crate::repository::Repository>>,
}

impl DataModels {
    /// The name each bound package declares, by exact data-model package
    /// URI (for `PKG005` and `DATAMODEL002`).
    pub(crate) fn bound(&self) -> BTreeMap<String, String> {
        self.packages
            .iter()
            .map(|package| {
                (
                    package.manifest.data_model_uri.clone(),
                    package.manifest.datamodel.clone(),
                )
            })
            .collect()
    }

    /// What binding each data model consents to: its name, URI, and
    /// version.
    pub(crate) fn summary(&self) -> Vec<String> {
        self.packages
            .iter()
            .map(|package| {
                format!(
                    "data model `{}`\n  = package: {} {}",
                    package.manifest.datamodel,
                    package.manifest.data_model_uri,
                    package.manifest.package_version
                )
            })
            .collect()
    }
}

/// Reads and verifies every `--data-model-package` manifest, then resolves
/// every repository data model the document `source` or one of its
/// `children` (the `<invoke src>` documents a run loads) names in an
/// `<scxml datamodel:src>` that none of them binds. A repository data model
/// that cannot be resolved stays unbound (the validator's `PKG005`).
///
/// # Errors
///
/// An unreadable, oversized, malformed, or unverifiable manifest; a data
/// model URI or name bound twice; or a malformed or conflicting pin.
pub(crate) fn resolve(
    source: &[u8],
    children: &[(url::Url, Vec<u8>)],
    manifests: &[PathBuf],
    repository: &crate::repository::RepositoryFlags,
) -> Result<DataModels, Box<dyn std::error::Error>> {
    let mut packages = load(manifests)?;
    let bound: Vec<String> = packages
        .iter()
        .map(|package| package.manifest.data_model_uri.clone())
        .collect();
    let mut uses = Vec::with_capacity(children.len().saturating_add(1));
    for (name, bytes) in std::iter::once(("the document".to_owned(), source)).chain(
        children
            .iter()
            .map(|(url, bytes)| (format!("`{url}`"), bytes.as_slice())),
    ) {
        uses.push(crate::repository::document_pins(bytes, &name)?.data_models);
    }
    let plan = crate::repository::plan(&uses, &bound);
    if let Some((_, tagless, pinned)) = plan.pinned.first() {
        return Err(format!(
            "`{pinned}` pins another release of the data model `{tagless}` than the run binds\n  = help: a run binds one release of each data model for every document it loads; pin the same tag (or none) everywhere"
        )
        .into());
    }
    if plan.run.is_empty() {
        return Ok(DataModels {
            packages,
            store: None,
        });
    }
    let store = repository.open()?;
    for uri in plan.run.values() {
        if let Ok(manifest) = store.resolve_data_model_package(uri) {
            push(
                &mut packages,
                DataModelPackage {
                    origin: DataModelOrigin::Repository(uri.clone()),
                    manifest,
                },
            )?;
        }
    }
    Ok(DataModels {
        packages,
        store: Some(store),
    })
}

/// Reads and verifies every `--data-model-package` manifest.
fn load(manifests: &[PathBuf]) -> Result<Vec<DataModelPackage>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadDataModelPackage);
    let result = (|| {
        let mut packages = Vec::with_capacity(manifests.len());
        for path in manifests {
            let shown = path.display();
            let bytes = read_bounded(path, crate::repository::MAX_MANIFEST_BYTES)
                .map_err(|error| format!("cannot read data-model package `{shown}`: {error}"))?;
            let manifest = DataModelPackageManifest::from_json(&bytes)
                .map_err(|error| format!("data-model package `{shown}` is malformed: {error}"))?;
            manifest.verify().map_err(|error| {
                format!("data-model package `{shown}` does not verify: {error}")
            })?;
            push(
                &mut packages,
                DataModelPackage {
                    origin: DataModelOrigin::Manifest(path.clone()),
                    manifest,
                },
            )?;
        }
        Ok(packages)
    })();
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    result
}

/// Appends `package` unless another binds its URI or its name.
fn push(packages: &mut Vec<DataModelPackage>, package: DataModelPackage) -> Result<(), String> {
    if let Some(bound) = packages.iter().find(|bound| {
        bound.manifest.data_model_uri == package.manifest.data_model_uri
            || bound.manifest.datamodel == package.manifest.datamodel
    }) {
        return Err(format!(
            "data-model packages `{}` and `{}` bind the same URI or the same data model name\n  = help: a run binds one package per data model",
            bound.manifest.data_model_uri, package.manifest.data_model_uri
        ));
    }
    packages.push(package);
    Ok(())
}

/// `snapshot run` and `run` flags for executing data models.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DataModelFlags {
    /// `--data-model-package` manifests.
    pub(crate) packages: Vec<PathBuf>,
    components: BTreeMap<String, PathBuf>,
    digests: BTreeMap<String, String>,
}

impl DataModelFlags {
    /// The flags this module owns, each taking one value.
    pub(crate) const FLAGS: [&'static str; 3] = [
        "--data-model-package",
        "--data-model-component",
        "--data-model-component-sha256",
    ];

    /// Records one flag.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn accept(&mut self, flag: &str, value: &str) -> Result<(), String> {
        let split = |usage: &str| match value.rsplit_once('=') {
            Some((uri, rest)) if !uri.is_empty() && !rest.is_empty() => {
                Ok((uri.to_owned(), rest.to_owned()))
            }
            _ => Err(usage.to_owned()),
        };
        match flag {
            "--data-model-package" => {
                if value.is_empty() {
                    return Err("--data-model-package requires MANIFEST.json".into());
                }
                self.packages.push(PathBuf::from(value));
            }
            "--data-model-component" => {
                let (uri, path) = split("--data-model-component requires URI=PATH")?;
                if self.components.insert(uri, PathBuf::from(path)).is_some() {
                    return Err("--data-model-component was given twice for one URI".into());
                }
            }
            "--data-model-component-sha256" => {
                let (uri, digest) = split("--data-model-component-sha256 requires URI=HEX")?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(
                        "--data-model-component-sha256 requires 64 lowercase hex characters".into(),
                    );
                }
                if self.digests.insert(uri, digest).is_some() {
                    return Err("--data-model-component-sha256 was given twice for one URI".into());
                }
            }
            other => return Err(format!("unknown option `{other}`")),
        }
        Ok(())
    }

    /// Checks that every component flag names a `--data-model-package` URI
    /// and that every such package has exactly one digest.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn check(&self, data_models: &DataModels) -> Result<(), String> {
        let explicit = |uri: &str| {
            data_models.packages.iter().any(|package| {
                matches!(package.origin, DataModelOrigin::Manifest(_))
                    && package.manifest.data_model_uri == uri
            })
        };
        if let Some(uri) = self
            .components
            .keys()
            .chain(self.digests.keys())
            .find(|uri| !explicit(uri))
        {
            return Err(format!(
                "`{uri}` is not the data-model URI of any --data-model-package\n  = help: --data-model-component and --data-model-component-sha256 name a bound data-model URI exactly"
            ));
        }
        if let Some(package) = data_models.packages.iter().find(|package| {
            matches!(package.origin, DataModelOrigin::Manifest(_))
                && !self.digests.contains_key(&package.manifest.data_model_uri)
        }) {
            return Err(format!(
                "data-model package `{}` needs --data-model-component-sha256 {}=HEX\n  = help: the SHA-256 of the data-model component is the trust anchor the run executes it under",
                package.manifest.data_model_uri, package.manifest.data_model_uri
            ));
        }
        Ok(())
    }

    /// Reads, authenticates, and binds every package's data-model
    /// component, by the data model's name.
    ///
    /// # Errors
    ///
    /// An unreadable or oversized component, a digest mismatch, or a binding
    /// the host refuses.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn components(
        &self,
        data_models: &DataModels,
    ) -> Result<scxml_host_wasmtime::DataModelComponents, Box<dyn std::error::Error>> {
        let mut bound = scxml_host_wasmtime::DataModelComponents::new();
        for package in &data_models.packages {
            let manifest = &package.manifest;
            let (bytes, digest, shown) = match &package.origin {
                DataModelOrigin::Repository(uri) => {
                    let store = data_models
                        .store
                        .as_ref()
                        .ok_or("a repository data model was resolved without a repository store")?;
                    let fetched = store.resolve_data_model_component(uri)?;
                    (fetched.bytes, fetched.sha256, uri.clone())
                }
                DataModelOrigin::Manifest(manifest_path) => {
                    let uri = &manifest.data_model_uri;
                    let path = match self.components.get(uri) {
                        Some(path) => path.clone(),
                        None => relative_to_manifest(manifest_path, &manifest.data_model_artifact)?,
                    };
                    let limit =
                        u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)?;
                    let bytes = read_bounded(&path, limit).map_err(|error| {
                        format!(
                            "cannot read the data-model component `{}`: {error}",
                            path.display()
                        )
                    })?;
                    let digest = self
                        .digests
                        .get(uri)
                        .ok_or("every data-model package needs --data-model-component-sha256")?;
                    (bytes, digest.clone(), path.display().to_string())
                }
            };
            let component = scxml_host_wasmtime::authenticate_data_model_component(
                &bytes,
                scxml_host_wasmtime::TrustedComponentDigest::from_hex(&digest)?,
            )
            .map_err(|error| {
                format!("the data-model component `{shown}` does not authenticate: {error}")
            })?;
            bound = bound.bind(&manifest.datamodel, &component)?;
        }
        Ok(bound)
    }
}

/// `artifact` inside the manifest's directory: a relative path of plain
/// components only.
#[cfg(feature = "wasmtime")]
fn relative_to_manifest(manifest: &std::path::Path, artifact: &str) -> Result<PathBuf, String> {
    use std::path::{Component, Path};
    let relative = Path::new(artifact);
    if artifact.is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(
            "field `data-model-artifact` must be a relative path inside the manifest's directory"
                .to_owned(),
        );
    }
    Ok(manifest
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../datamodels/example/package/example.data-model.json")
    }

    #[test]
    fn the_example_package_loads_and_binds_its_name() {
        let packages = load(&[example()]).expect("the example data-model package loads");
        let data_models = DataModels {
            packages,
            store: None,
        };
        assert_eq!(
            data_models.bound(),
            BTreeMap::from([(
                "https://github.com/stateforward/scxml.rs/datamodels/example".to_owned(),
                "example".to_owned()
            )])
        );
        assert!(load(&[example(), example()]).is_err(), "bound twice");
        let mut flags = DataModelFlags::default();
        assert!(flags.check(&data_models).is_err(), "the digest is required");
        flags
            .accept(
                "--data-model-component-sha256",
                &format!(
                    "https://github.com/stateforward/scxml.rs/datamodels/example={}",
                    "a".repeat(64)
                ),
            )
            .expect("a digest flag");
        assert_eq!(flags.check(&data_models), Ok(()));
        assert!(
            DataModelFlags::default()
                .accept("--data-model-component-sha256", "urn:x=AB")
                .is_err()
        );
    }
}
