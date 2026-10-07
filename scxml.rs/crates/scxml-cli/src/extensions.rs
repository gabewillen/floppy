//! extension package flags (SCXML 1.0 section 4.10).
//!
//! * `--extension-package MANIFEST.json` (repeatable; `validate`, `compile`,
//!   `snapshot run`, and `run`) binds one extension package: a
//!   `stateforward.scxml.extension-package/v2` manifest naming the exact
//!   namespace URI, package version, extension artifact, schema artifact and
//!   its SHA-256, and WIT world and version. An extension element is
//!   executable content and does no I/O, so the manifest declares no
//!   outbound origins and no `env` slots. The schema artifact is resolved
//!   relative to the
//!   manifest's directory, read within its byte bound, and checked against
//!   the manifest before use; it then governs the namespace's XML structure
//!   (so `--schema` may not name the same URI). `compile` also hands the
//!   manifest to the compiler, which requires one for every used extension
//!   namespace.
//! * A namespace the document uses whose URI is a repository namespace
//!   (`https://github.com/<owner>/<repo>/namespaces/<name>`, pinned to a
//!   release with `@<tag>` where the document declares it, see
//!   [`crate::repository`]) and that no flag binds is resolved from the
//!   extension cache or its repository: its package for validation and
//!   compilation, its extension component when a session starts.
//!   `--offline`, `--cache-dir DIR`, and the `--github-*` flags govern that.
//! * `snapshot run` and `run` execute an `--extension-package` namespace's
//!   extension component, resolved relative to the manifest
//!   (`extension-artifact`) or by `--extension-component URI=PATH`, and
//!   authenticated by the SHA-256 in `--extension-component-sha256 URI=HEX`
//!   (required for every such package; the manifest is not a trust anchor).
//! * The document scopes every variable: an invoked service's `<invoke>`
//!   names a variable for one of its slots with `env:<slot>="NAME"`, and the
//!   variable goes only to the packages (the `<invoke>`'s literal `type`)
//!   whose `<invoke>`s name it. An `env:<slot>expr` slot names its variable
//!   only when its `<invoke>` runs, so its package may receive any variable
//!   the run holds.
//!   `--env-file PATH` (repeatable; Docker's format, [`crate::env_file`])
//!   and `--env NAME` / `--env NAME=VALUE` (repeatable) supply values: a
//!   variable's value is its flag's `VALUE`, else the last `--env-file`
//!   entry for it, else, only when `--env NAME` names it, the calling
//!   shell's. A variable no slot can name is dropped. Its component receives
//!   an opaque reference, never the value, and the host substitutes the
//!   value only into its admitted outbound requests; a slot whose variable
//!   holds no value is absent. The values are masked in the text the CLI
//!   writes ([`crate::mask`]).
//! * Every command that reads a document lists, per bound element, its
//!   namespace and version: what importing it consents to.
//!
//! Files are read with a regular-file check and a byte bound.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use scxml_compiler_core::NamespaceSchemaBinding;
use scxml_telemetry::{ErrorCategory, Module, Operation, observe};
use scxml_validator_core::{
    ExtensionPackageManifest, MAX_COMPONENT_SCHEMA_BYTES, MemorySchemaResolver, SchemaResolveError,
    SchemaResolver, SchemaSource,
};

use crate::repository::{Repository, RepositoryFlags, RepositoryUri};

/// Byte bound of one manifest file.
const MAX_MANIFEST_BYTES: u64 = crate::repository::MAX_MANIFEST_BYTES;

/// Where a package came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PackageOrigin {
    /// An `--extension-package` manifest file.
    Manifest(PathBuf),
    /// The `scx` namespace (`namespaces/scx`), which `scx` embeds: its
    /// manifest and schema compiled in, its component (under the `wasmtime`
    /// feature) authenticated by the SHA-256 `build.rs` checked against the
    /// release manifest.
    Embedded,
    /// A repository namespace (the extension cache or its repository), at the
    /// commit its release tag pinned.
    Repository {
        /// The URI the resolver resolved: the tagless URI, or it with the
        /// document's `@<tag>`.
        uri: String,
        /// The release tag (the pin, or the release a tagless URI resolved).
        tag: String,
        /// The commit the tag named when the cache pinned it.
        commit: String,
    },
}

/// One verified extension package.
#[derive(Clone, Debug)]
pub(crate) struct Package {
    /// Where it came from.
    pub(crate) origin: PackageOrigin,
    /// The verified manifest.
    pub(crate) manifest: ExtensionPackageManifest,
    /// The schema bytes the manifest's digest names.
    pub(crate) schema: SchemaSource,
    /// The package validator component bytes the manifest's
    /// `validator-sha256` names, when it names one.
    pub(crate) validator: Option<Vec<u8>>,
}

/// The exact URI of the `scx` namespace (`namespaces/scx`), which `scx`
/// embeds: tagless, like the `env` and `event` vocabularies, because it is
/// versioned with the toolchain.
pub(crate) const SCX_NAMESPACE_URI: &str =
    "https://github.com/stateforward/scxml.rs/namespaces/scx";

impl Package {
    /// The embedded `scx` package: its compiled-in manifest, verified against
    /// its compiled-in schema like any `--extension-package`.
    ///
    /// # Errors
    ///
    /// A manifest or schema that does not verify (a build defect).
    pub(crate) fn embedded() -> Result<Self, String> {
        let manifest = ExtensionPackageManifest::from_json(include_bytes!(
            "../../../namespaces/scx/package/scx.extension.json"
        ))
        .map_err(|error| format!("the embedded scx package is malformed: {error}"))?;
        let schema = SchemaSource::new(
            manifest.schema_artifact.clone(),
            include_bytes!("../../../namespaces/scx/package/scx.xsd").to_vec(),
        );
        manifest
            .verify_schema(&schema)
            .map_err(|error| format!("the embedded scx package does not verify: {error}"))?;
        if manifest.namespace_uri != SCX_NAMESPACE_URI {
            return Err("the embedded scx package binds another namespace".into());
        }
        Ok(Self {
            origin: PackageOrigin::Embedded,
            manifest,
            schema,
            validator: None,
        })
    }

    /// The URI the resolver resolved for a repository namespace (its tagless
    /// URI, or it with the document's `@<tag>`); `None` otherwise.
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "only a run binds repository components, which needs the wasmtime feature"
        )
    )]
    pub(crate) fn resolved_uri(&self) -> Option<&str> {
        match &self.origin {
            PackageOrigin::Repository { uri, .. } => Some(uri),
            PackageOrigin::Manifest(_) | PackageOrigin::Embedded => None,
        }
    }

    /// The package as messages name it: the manifest path, or the
    /// repository namespace's URI.
    pub(crate) fn label(&self) -> String {
        match &self.origin {
            PackageOrigin::Manifest(path) => path.display().to_string(),
            PackageOrigin::Repository { .. } | PackageOrigin::Embedded => {
                self.manifest.namespace_uri.clone()
            }
        }
    }

    /// The extension artifact's default path: `extension-artifact` relative to
    /// the manifest's directory (an `--extension-package` only).
    #[cfg(any(feature = "wasmtime", test))]
    pub(crate) fn extension_component_path(&self) -> Result<PathBuf, String> {
        match &self.origin {
            PackageOrigin::Manifest(path) => relative_to_manifest(
                path,
                &self.manifest.extension_artifact,
                "extension-artifact",
            ),
            PackageOrigin::Repository { .. } => {
                Err("a repository namespace's extension component comes from its release".into())
            }
            PackageOrigin::Embedded => {
                Err("the scx namespace's extension component is embedded".into())
            }
        }
    }

    /// The package as the session host binds it.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn host_package(&self) -> scxml_host_wasmtime::ExtensionPackage {
        let manifest = &self.manifest;
        scxml_host_wasmtime::ExtensionPackage {
            namespace_uri: manifest.namespace_uri.clone(),
            package_version: manifest.package_version.clone(),
            extension_artifact: manifest.extension_artifact.clone(),
            schema_artifact: manifest.schema_artifact.clone(),
            schema_sha256: manifest.schema_sha256.clone(),
            wit_world: manifest.wit_world.clone(),
            wit_version: manifest.wit_version.clone(),
            schema: self.schema.bytes.clone(),
        }
    }
}

/// Reads and verifies every `--extension-package` manifest.
///
/// # Errors
///
/// A message naming the manifest for an unreadable, oversized, malformed,
/// or unverifiable manifest or schema, a namespace bound twice, or one also
/// named by `--schema`.
pub(crate) fn load(
    manifests: &[PathBuf],
    schema_flags: &[(String, PathBuf)],
) -> Result<Vec<Package>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadExtensionPackage);
    let result = (|| {
        let mut packages: Vec<Package> = Vec::with_capacity(manifests.len());
        for path in manifests {
            let package = load_one(path)?;
            if packages
                .iter()
                .any(|bound| bound.manifest.namespace_uri == package.manifest.namespace_uri)
            {
                return Err(format!(
                    "extension package `{}` binds a namespace another --extension-package already binds",
                    path.display()
                )
                .into());
            }
            if schema_flags
                .iter()
                .any(|(uri, _)| *uri == package.manifest.namespace_uri)
            {
                return Err(format!(
                    "extension package `{}` binds a namespace that --schema also names\n  = help: a package's own schema governs its namespace; drop the --schema mapping",
                    path.display()
                )
                .into());
            }
            packages.push(package);
        }
        Ok(packages)
    })();
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    result
}

/// The packages of one command: the `--extension-package` ones and the
/// repository namespaces the document uses that nothing else binds.
#[derive(Debug, Default)]
pub(crate) struct Packages {
    /// `--extension-package` packages.
    pub(crate) explicit: Vec<Package>,
    /// Repository namespaces the document uses: the run's resolution of
    /// each ([`crate::repository::plan`]).
    pub(crate) repository: Vec<Package>,
    /// The `<invoke src>` documents that pin a release other than the
    /// run's, each with the packages its sessions bind in place of the
    /// run's.
    pub(crate) documents: Vec<(url::Url, Vec<Package>)>,
    /// The repository store, opened only when the document uses one.
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "the session host's resolver reads it, which needs the wasmtime feature"
        )
    )]
    pub(crate) store: Option<Arc<Repository>>,
    /// Every `env` slot binding the document's `<invoke>`s make.
    pub(crate) slot_uses: Vec<SlotUse>,
}

impl Packages {
    /// Loads the `--extension-package` manifests and resolves every
    /// repository namespace `source` or one of its `children` (the
    /// `<invoke src>` documents a run loads, with their URLs) uses that
    /// neither they nor `--schema` bind: each package once for the run (the
    /// root's pin, else the first document's that uses it), and once more
    /// for each child document that pins another release.
    ///
    /// # Errors
    ///
    /// As [`load`], a malformed or conflicting pin
    /// ([`crate::repository::document_pins`]), an unopenable extension
    /// cache, or a repository namespace that cannot be resolved.
    pub(crate) fn resolve(
        source: &[u8],
        children: &[(url::Url, Vec<u8>)],
        manifests: &[PathBuf],
        schema_flags: &[(String, PathBuf)],
        repository: &RepositoryFlags,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut explicit = load(manifests, schema_flags)?;
        let mut documents_uses = Vec::with_capacity(children.len().saturating_add(1));
        for (name, bytes) in std::iter::once(("the document".to_owned(), source)).chain(
            children
                .iter()
                .map(|(url, bytes)| (format!("`{url}`"), bytes.as_slice())),
        ) {
            let pins = crate::repository::document_pins(bytes, &name)?;
            let all = crate::repository::used_namespaces(bytes);
            let used: BTreeMap<String, Option<String>> = all
                .iter()
                .filter(|uri| RepositoryUri::parse(uri).is_some())
                .map(|uri| (uri.clone(), pins.namespaces.get(uri).cloned().flatten()))
                .collect();
            documents_uses.push((used, all));
        }
        // The scx namespace ships with scx: a document that uses it binds the
        // embedded package unless a flag binds the URI.
        if documents_uses
            .iter()
            .any(|(_, all)| all.iter().any(|uri| uri == SCX_NAMESPACE_URI))
            && !explicit
                .iter()
                .any(|package| package.manifest.namespace_uri == SCX_NAMESPACE_URI)
            && !schema_flags.iter().any(|(uri, _)| uri == SCX_NAMESPACE_URI)
        {
            explicit.push(Package::embedded()?);
        }
        let bound: Vec<String> = explicit
            .iter()
            .map(|package| package.manifest.namespace_uri.clone())
            .chain(schema_flags.iter().map(|(uri, _)| uri.clone()))
            .collect();
        let uses: Vec<BTreeMap<String, Option<String>>> =
            documents_uses.into_iter().map(|(used, _)| used).collect();
        let plan = crate::repository::plan(&uses, &bound);
        let (store, repository_packages, documents) = if plan.run.is_empty() {
            (None, Vec::new(), Vec::new())
        } else {
            let store = repository.open()?;
            let mut packages = Vec::with_capacity(plan.run.len());
            for uri in plan.run.values() {
                packages.push(store.resolve_package(uri)?);
            }
            let mut documents: Vec<(url::Url, Vec<Package>)> = Vec::new();
            for (index, tagless, pinned) in &plan.pinned {
                let run_tag = packages.iter().find_map(|package| match &package.origin {
                    PackageOrigin::Repository { tag, .. }
                        if package.manifest.namespace_uri == *tagless =>
                    {
                        Some(tag.as_str())
                    }
                    _ => None,
                });
                // A pin naming the release the run already resolved shares it.
                if run_tag == scxml_types::strip_tag(pinned).and_then(|(_, tag)| tag) {
                    continue;
                }
                let Some((url, _)) = index.checked_sub(1).and_then(|child| children.get(child))
                else {
                    continue;
                };
                let package = store.resolve_package(pinned)?;
                match documents.iter_mut().find(|(bound, _)| bound == url) {
                    Some((_, packages)) => packages.push(package),
                    None => documents.push((url.clone(), vec![package])),
                }
            }
            (Some(store), packages, documents)
        };
        Ok(Self {
            explicit,
            repository: repository_packages,
            documents,
            store,
            slot_uses: std::iter::once(source)
                .chain(children.iter().map(|(_, bytes)| bytes.as_slice()))
                .flat_map(slot_uses)
                .collect(),
        })
    }

    /// Every package, `--extension-package` ones first.
    pub(crate) fn all(&self) -> Vec<Package> {
        self.explicit
            .iter()
            .chain(&self.repository)
            .cloned()
            .collect()
    }

    /// What importing each element consents to: its namespace and version.
    pub(crate) fn summary(&self) -> Vec<String> {
        let documents = self
            .documents
            .iter()
            .flat_map(|(url, packages)| packages.iter().map(move |package| (package, Some(url))));
        let mut lines = Vec::new();
        for (package, document) in self
            .explicit
            .iter()
            .chain(&self.repository)
            .map(|package| (package, None))
            .chain(documents)
        {
            let manifest = &package.manifest;
            let uri = &manifest.namespace_uri;
            let version = match &package.origin {
                PackageOrigin::Manifest(path) => format!(
                    "package {} from --extension-package {}",
                    manifest.package_version,
                    path.display()
                ),
                PackageOrigin::Repository { tag, commit, .. } => format!(
                    "release {tag} (commit {commit}), package {}",
                    manifest.package_version
                ),
                PackageOrigin::Embedded => {
                    format!("package {} built into scx", manifest.package_version)
                }
            };
            let scope = document.map_or_else(String::new, |url| {
                format!("\n  = for: the sessions of <invoke src> document `{url}`")
            });
            lines.push(format!("element `{uri}`\n  = version: {version}{scope}"));
        }
        lines
    }
}

/// How one `env` attribute names its variable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SlotName {
    /// `env:<slot>="NAME"`.
    Variable(String),
    /// `env:<slot>expr="EXPR"` (evaluated per execution).
    Expression,
}

/// One `env` attribute of an `<invoke>` with a literal `type`, in the
/// document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SlotUse {
    /// The package the slot belongs to: the `<invoke>`'s literal `type`.
    pub(crate) namespace_uri: String,
    /// The slot (the attribute's local name, `expr` stripped).
    pub(crate) slot: String,
    /// The variable it names.
    pub(crate) name: SlotName,
}

/// Every `env` attribute on an `<invoke>` with a literal `type`, in document
/// order; a document that does not parse binds none here (the validator's
/// `SCXML084` rejects one anywhere else).
pub(crate) fn slot_uses(source: &[u8]) -> Vec<SlotUse> {
    let Ok(text) = std::str::from_utf8(source) else {
        return Vec::new();
    };
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return Vec::new();
    };
    let mut uses = Vec::new();
    document.root.visit(&mut |element| {
        if element.name.namespace_uri.as_deref() != Some(scxml_validator_core::SCXML_NAMESPACE)
            || element.name.local_name != "invoke"
        {
            return;
        }
        // An invoked service's package is its `<invoke>`'s literal `type`.
        let Some(namespace_uri) = element
            .attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace_uri.is_none() && attribute.name.local_name == "type"
            })
            .map(|attribute| attribute.value.as_str())
        else {
            return;
        };
        for attribute in &element.attributes {
            if attribute.name.namespace_uri.as_deref() != Some(scxml_types::ENV_NAMESPACE_URI) {
                continue;
            }
            let local = attribute.name.local_name.as_str();
            let (slot, name) = match local.strip_suffix("expr") {
                Some(slot) if !slot.is_empty() => (slot.to_owned(), SlotName::Expression),
                _ => (
                    local.to_owned(),
                    SlotName::Variable(attribute.value.clone()),
                ),
            };
            uses.push(SlotUse {
                namespace_uri: namespace_uri.to_owned(),
                slot,
                name,
            });
        }
    });
    uses
}

fn load_one(path: &Path) -> Result<Package, Box<dyn std::error::Error>> {
    let shown = path.display();
    let bytes = read_bounded(path, MAX_MANIFEST_BYTES)
        .map_err(|error| format!("cannot read extension package `{shown}`: {error}"))?;
    let manifest = ExtensionPackageManifest::from_json(&bytes)
        .map_err(|error| format!("extension package `{shown}` is malformed: {error}"))?;
    let schema_path = relative_to_manifest(path, &manifest.schema_artifact, "schema-artifact")
        .map_err(|error| format!("extension package `{shown}`: {error}"))?;
    let limit = u64::try_from(MAX_COMPONENT_SCHEMA_BYTES)?;
    let schema_bytes = read_bounded(&schema_path, limit).map_err(|error| {
        format!(
            "extension package `{shown}`: cannot read its schema `{}`: {error}",
            schema_path.display()
        )
    })?;
    let schema = SchemaSource::new(manifest.schema_artifact.clone(), schema_bytes);
    manifest
        .verify_schema(&schema)
        .map_err(|error| format!("extension package `{shown}` does not verify: {error}"))?;
    let validator = read_validator(
        path,
        manifest.validator_artifact.as_deref(),
        manifest.validator_sha256.as_deref(),
    )
    .map_err(|error| format!("extension package `{shown}`: {error}"))?;
    Ok(Package {
        origin: PackageOrigin::Manifest(path.to_path_buf()),
        manifest,
        schema,
        validator,
    })
}

/// The package validator component a verified manifest names
/// (`validator-artifact` relative to the manifest's directory), read within
/// the component bound and checked against `validator-sha256`; `None` when
/// the manifest names none.
pub(crate) fn read_validator(
    manifest: &Path,
    artifact: Option<&str>,
    sha256: Option<&str>,
) -> Result<Option<Vec<u8>>, String> {
    let (Some(artifact), Some(sha256)) = (artifact, sha256) else {
        return Ok(None);
    };
    let path = relative_to_manifest(manifest, artifact, "validator-artifact")?;
    let bytes = read_bounded(&path, crate::repository::MAX_COMPONENT_BYTES).map_err(|error| {
        format!(
            "cannot read its package validator `{}`: {error}",
            path.display()
        )
    })?;
    if hex_sha256(&bytes) != sha256 {
        return Err(format!(
            "its package validator `{}` does not match `validator-sha256`",
            path.display()
        ));
    }
    Ok(Some(bytes))
}

/// The lowercase hex SHA-256 of `bytes`.
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// `artifact` inside the manifest's directory: a relative path of plain
/// components only (no root, `..`, or prefix), so a manifest cannot reach
/// outside its own directory tree.
fn relative_to_manifest(manifest: &Path, artifact: &str, field: &str) -> Result<PathBuf, String> {
    let relative = Path::new(artifact);
    if artifact.is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "field `{field}` must be a relative path inside the manifest's directory"
        ));
    }
    Ok(manifest
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(relative))
}

/// Reads `path` only when it is a regular file of at most `limit` bytes,
/// reading at most `limit + 1` bytes.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("not a regular file".into());
    }
    if metadata.len() > limit {
        return Err(format!("larger than its {limit}-byte bound"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > limit) {
        return Err(format!("larger than its {limit}-byte bound"));
    }
    Ok(bytes)
}

/// The manifests and schema bindings the compiler verifies.
pub(crate) fn compile_inputs(
    packages: &[Package],
) -> (Vec<ExtensionPackageManifest>, Vec<NamespaceSchemaBinding>) {
    packages
        .iter()
        .map(|package| {
            (
                package.manifest.clone(),
                NamespaceSchemaBinding {
                    namespace_uri: package.manifest.namespace_uri.clone(),
                    schema: package.schema.clone(),
                },
            )
        })
        .unzip()
}

/// A schema resolver that answers bound namespaces with their packages'
/// verified schema bytes and every other namespace with `inner`.
pub(crate) struct PackageSchemaResolver<R> {
    inner: R,
    packages: MemorySchemaResolver,
    bound: BTreeSet<String>,
}

impl<R> PackageSchemaResolver<R> {
    pub(crate) fn new(inner: R, packages: &[Package]) -> Self {
        let mut memory = MemorySchemaResolver::new();
        let mut bound = BTreeSet::new();
        for package in packages {
            memory.insert(
                package.manifest.namespace_uri.clone(),
                package.schema.clone(),
            );
            bound.insert(package.manifest.namespace_uri.clone());
        }
        Self {
            inner,
            packages: memory,
            bound,
        }
    }
}

impl<R: SchemaResolver> SchemaResolver for PackageSchemaResolver<R> {
    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
        if self.bound.contains(namespace_uri) {
            self.packages.resolve(namespace_uri)
        } else {
            self.inner.resolve(namespace_uri)
        }
    }

    fn resolve_documents(
        &self,
        namespace_uri: &str,
    ) -> Result<Vec<SchemaSource>, SchemaResolveError> {
        if self.bound.contains(namespace_uri) {
            self.packages.resolve_documents(namespace_uri)
        } else {
            self.inner.resolve_documents(namespace_uri)
        }
    }
}

/// One `--env` flag.
#[derive(Clone, Debug, Eq, PartialEq)]
struct EnvFlag {
    /// The variable.
    name: String,
    /// `=VALUE`, when given.
    value: Option<String>,
}

/// One variable the run holds a value for, and the exact package URIs
/// (service type URIs) whose `<invoke>`s the document lets name it. `Debug` names the variable and its packages only.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct HeldValue {
    pub(crate) name: String,
    pub(crate) value: String,
    /// Sorted, distinct, non-empty.
    pub(crate) packages: Vec<String>,
}

impl std::fmt::Debug for HeldValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeldValue")
            .field("name", &self.name)
            .field("packages", &self.packages)
            .finish_non_exhaustive()
    }
}

impl HeldValue {
    /// The run's values as the session host takes them.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn environment(
        values: &[Self],
    ) -> Result<scxml_host_wasmtime::ExtensionEnvironment, scxml_host_wasmtime::HostError> {
        scxml_host_wasmtime::ExtensionEnvironment::new(values.iter().map(|held| {
            scxml_host_wasmtime::EnvVariable::new(
                held.name.clone(),
                held.value.clone(),
                held.packages.iter().cloned(),
            )
        }))
    }

    /// `(name, value)` of every value, for masking.
    pub(crate) fn pairs(values: &[Self]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|held| (held.name.clone(), held.value.clone()))
            .collect()
    }
}

/// `snapshot run` and `run` flags for executing extension components.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunFlags {
    /// `--extension-package` manifests.
    pub(crate) packages: Vec<PathBuf>,
    components: BTreeMap<String, PathBuf>,
    digests: BTreeMap<String, String>,
    /// `--env` flags, in order.
    env: Vec<EnvFlag>,
    /// `--env-file` paths, in order.
    env_files: Vec<PathBuf>,
    /// Repository namespace flags.
    pub(crate) repository: RepositoryFlags,
}

impl RunFlags {
    /// Records one flag.
    pub(crate) fn accept(&mut self, flag: &str, value: &str) -> Result<(), String> {
        if self.repository.accept(flag, value)? {
            return Ok(());
        }
        match flag {
            "--extension-package" => {
                self.packages.push(PathBuf::from(value));
                Ok(())
            }
            "--extension-component" => {
                let (uri, path) = split_uri(value, "--extension-component requires URI=PATH")?;
                if self.components.insert(uri, PathBuf::from(path)).is_some() {
                    return Err("--extension-component was given twice for one URI".into());
                }
                Ok(())
            }
            "--extension-component-sha256" => {
                let (uri, digest) =
                    split_uri(value, "--extension-component-sha256 requires URI=HEX")?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(
                        "--extension-component-sha256 requires 64 lowercase hex characters".into(),
                    );
                }
                if self.digests.insert(uri, digest).is_some() {
                    return Err("--extension-component-sha256 was given twice for one URI".into());
                }
                Ok(())
            }
            "--env" => {
                const USAGE: &str =
                    "--env requires NAME or NAME=VALUE, NAME being [A-Za-z_][A-Za-z0-9_]*";
                let (name, value) = match value.split_once('=') {
                    Some((name, value)) => (name, Some(value.to_owned())),
                    None => (value, None),
                };
                if !crate::env_file::is_name(name) {
                    return Err(USAGE.into());
                }
                if self.env.iter().any(|seen| seen.name == name) {
                    return Err("--env was given twice for one NAME".into());
                }
                self.env.push(EnvFlag {
                    name: name.to_owned(),
                    value,
                });
                Ok(())
            }
            "--env-file" => {
                if value.is_empty() {
                    return Err("--env-file requires PATH".into());
                }
                self.env_files.push(PathBuf::from(value));
                Ok(())
            }
            other => Err(format!("unknown option `{other}`")),
        }
    }

    /// Checks that every extension component flag names an
    /// `--extension-package` namespace and that every such package has
    /// exactly one digest.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn check(&self, packages: &Packages) -> Result<(), String> {
        let explicit: BTreeSet<&str> = packages
            .explicit
            .iter()
            .map(|package| package.manifest.namespace_uri.as_str())
            .collect();
        if let Some(uri) = self
            .components
            .keys()
            .chain(self.digests.keys())
            .find(|uri| !explicit.contains(uri.as_str()))
        {
            return Err(format!(
                "`{uri}` is not the namespace of any --extension-package\n  = help: --extension-component and --extension-component-sha256 name a bound namespace URI exactly"
            ));
        }
        if let Some(package) = packages.explicit.iter().find(|package| {
            package.origin != PackageOrigin::Embedded
                && !self.digests.contains_key(&package.manifest.namespace_uri)
        }) {
            return Err(format!(
                "extension package `{}` needs --extension-component-sha256 {}=HEX\n  = help: the SHA-256 of the extension component is the trust anchor snapshot run executes it under",
                package.label(),
                package.manifest.namespace_uri
            ));
        }
        Ok(())
    }

    /// The component path of one `--extension-package` package.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn extension_component_path(&self, package: &Package) -> Result<PathBuf, String> {
        match self.components.get(&package.manifest.namespace_uri) {
            Some(path) => Ok(path.clone()),
            None => package
                .extension_component_path()
                .map_err(|error| format!("extension package `{}`: {error}", package.label())),
        }
    }

    /// The pinned SHA-256 of one `--extension-package` package's extension
    /// component.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn extension_component_sha256(&self, package: &Package) -> Option<&str> {
        if package.origin == PackageOrigin::Embedded {
            return Some(crate::components::embedded::SCX_COMPONENT_SHA256);
        }
        self.digests
            .get(&package.manifest.namespace_uri)
            .map(String::as_str)
    }

    /// The run's values, scoped by the document's `env` attributes
    /// (`uses`): each variable a static slot names goes to the packages of
    /// the `<invoke>`s naming it, and every variable the run
    /// supplies also goes to the package of each `env:<slot>expr` slot. A
    /// variable's value is its flag's `=VALUE`, else the last `--env-file`
    /// entry for it (every file read once, now, in order), else, only for a
    /// variable an `--env NAME` flag names, the calling shell's; a variable
    /// none of them sets, or that no slot can name, holds no value. Sorted
    /// by name.
    ///
    /// # Errors
    ///
    /// An unreadable, oversized, or malformed `--env-file` (the message names
    /// the file and line, never a value).
    pub(crate) fn values(&self, uses: &[SlotUse]) -> Result<Vec<HeldValue>, String> {
        let mut supplied: BTreeMap<String, String> = BTreeMap::new();
        for path in &self.env_files {
            for (name, value) in crate::env_file::read(path)? {
                supplied.insert(name, value);
            }
        }
        for flag in &self.env {
            let value = match &flag.value {
                Some(value) => Some(value.clone()),
                None => supplied
                    .get(&flag.name)
                    .cloned()
                    .or_else(|| std::env::var(&flag.name).ok()),
            };
            if let Some(value) = value {
                supplied.insert(flag.name.clone(), value);
            }
        }
        let expression_packages: BTreeSet<&str> = uses
            .iter()
            .filter(|binding| binding.name == SlotName::Expression)
            .map(|binding| binding.namespace_uri.as_str())
            .collect();
        Ok(supplied
            .into_iter()
            .filter_map(|(name, value)| {
                let packages: BTreeSet<&str> = uses
                    .iter()
                    .filter(|binding| binding.name == SlotName::Variable(name.clone()))
                    .map(|binding| binding.namespace_uri.as_str())
                    .chain(expression_packages.iter().copied())
                    .collect();
                (!packages.is_empty()).then(|| HeldValue {
                    name,
                    value,
                    packages: packages.into_iter().map(str::to_owned).collect(),
                })
            })
            .collect())
    }
}

/// The consent listing of the run's variables: each variable the document
/// names, the `<invoke>`s it goes to (names and URIs only), and
/// whether the run holds a value for it; then each `env:<slot>expr` slot,
/// which may name any variable the run holds when it executes. Never a
/// value.
pub(crate) fn env_summary(uses: &[SlotUse], held: &[HeldValue]) -> Vec<String> {
    let target = |binding: &SlotUse| {
        format!(
            "<invoke type=\"{}\"> env:{}",
            binding.namespace_uri, binding.slot
        )
    };
    let mut names: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut expressions = BTreeSet::new();
    for binding in uses {
        match &binding.name {
            SlotName::Variable(name) => {
                names
                    .entry(name.as_str())
                    .or_default()
                    .insert(target(binding));
            }
            SlotName::Expression => {
                expressions.insert(format!("{}expr", target(binding)));
            }
        }
    }
    let mut lines: Vec<String> = names
        .into_iter()
        .map(|(name, targets)| {
            let value = if held.iter().any(|value| value.name == name) {
                ""
            } else {
                " (no value)"
            };
            format!(
                "env `{name}`{value}\n  = to: {}",
                targets.into_iter().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    for target in expressions {
        lines.push(format!(
            "env (expression)\n  = to: {target}, naming any variable the run holds when it executes"
        ));
    }
    lines
}

/// Every `http-origins` entry the bound service packages declare: public
/// strings (their manifests declare them).
pub(crate) fn declared_origins(services: &[crate::services::ServicePackage]) -> Vec<String> {
    services
        .iter()
        .flat_map(|service| service.manifest.http_origins.iter().cloned())
        .collect()
}

/// Writes `notes` to standard error, masked. A failed write has no further
/// channel to report to; the notes are advisory.
pub(crate) fn write_notes(notes: &[String]) {
    use std::io::Write as _;
    let mut stderr = std::io::stderr().lock();
    for note in notes {
        let _ = writeln!(stderr, "{}", crate::mask::scrub(note));
    }
}

fn split_uri(value: &str, message: &str) -> Result<(String, String), String> {
    value
        .rsplit_once('=')
        .filter(|(left, right)| !left.is_empty() && !right.is_empty())
        .map(|(left, right)| (left.to_owned(), right.to_owned()))
        .ok_or_else(|| message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_validator_core::EXTENSION_PACKAGE_FORMAT;

    const SCHEMA: &str =
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:x"/>"#;

    fn manifest_json(extra: &str) -> String {
        use sha2::Digest as _;
        let digest = format!("{:x}", sha2::Sha256::digest(SCHEMA.as_bytes()));
        format!(
            r#"{{"format":"{EXTENSION_PACKAGE_FORMAT}","namespace-uri":"urn:x","package-version":"1","extension-artifact":"x.wasm","schema-artifact":"x.xsd","schema-sha256":"{digest}","wit-world":"stateforward:scxml/executable-content-extension","wit-version":"0.2.0"{extra}}}"#
        )
    }

    #[test]
    fn manifests_are_exact_versioned_objects() {
        let manifest = ExtensionPackageManifest::from_json(manifest_json("").as_bytes())
            .expect("valid manifest");
        assert_eq!(manifest.namespace_uri, "urn:x");
        assert!(
            ExtensionPackageManifest::from_json(manifest_json(r#","extra":"1""#).as_bytes())
                .expect_err("unknown fields fail closed")
                .contains("unknown field `extra`")
        );
        assert!(ExtensionPackageManifest::from_json(b"[]").is_err());
        assert!(ExtensionPackageManifest::from_json(b"{").is_err());
        let wrong_format = manifest_json("").replace(EXTENSION_PACKAGE_FORMAT, "other/v2");
        assert!(
            ExtensionPackageManifest::from_json(wrong_format.as_bytes())
                .expect_err("unknown formats fail closed")
                .contains("`format`")
        );
        let v1 = manifest_json("").replace(
            EXTENSION_PACKAGE_FORMAT,
            "stateforward.scxml.extension-package/v1",
        );
        assert!(
            ExtensionPackageManifest::from_json(v1.as_bytes()).is_err(),
            "v1 is rejected"
        );
        let numeric =
            manifest_json("").replace(r#""package-version":"1""#, r#""package-version":1"#);
        assert!(ExtensionPackageManifest::from_json(numeric.as_bytes()).is_err());
        for removed in [r#","http-origins":[]"#, r#","env-slots":[]"#] {
            assert!(
                ExtensionPackageManifest::from_json(manifest_json(removed).as_bytes())
                    .expect_err("extensions declare no I/O")
                    .contains("unknown field"),
                "{removed}"
            );
        }
    }

    #[test]
    fn artifacts_stay_inside_the_manifest_directory() {
        let manifest = Path::new("pkg/x.extension.json");
        assert_eq!(
            relative_to_manifest(manifest, "sub/x.xsd", "schema-artifact"),
            Ok(PathBuf::from("pkg/sub/x.xsd"))
        );
        for escape in ["../x.xsd", "/etc/x.xsd", "a/../../x.xsd", ""] {
            assert!(
                relative_to_manifest(manifest, escape, "schema-artifact").is_err(),
                "{escape}"
            );
        }
    }

    #[test]
    fn packages_load_verify_and_reject_conflicts() {
        let dir = std::env::temp_dir().join(format!("scx-namespaces-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temporary directory");
        fs::write(dir.join("x.xsd"), SCHEMA).expect("schema");
        let path = dir.join("x.extension.json");
        fs::write(&path, manifest_json("")).expect("manifest");
        let packages = load(std::slice::from_ref(&path), &[]).expect("the package verifies");
        assert_eq!(packages[0].schema.bytes, SCHEMA.as_bytes());
        assert_eq!(
            packages[0].extension_component_path(),
            Ok(dir.join("x.wasm"))
        );
        assert!(
            load(&[path.clone(), path.clone()], &[]).is_err(),
            "bound twice"
        );
        assert!(
            load(
                std::slice::from_ref(&path),
                &[("urn:x".into(), PathBuf::from("y.xsd"))]
            )
            .is_err(),
            "--schema may not name a package's namespace"
        );
        fs::write(dir.join("x.xsd"), "<changed/>").expect("schema");
        assert!(
            load(std::slice::from_ref(&path), &[])
                .expect_err("a schema that does not match its digest fails")
                .to_string()
                .contains("does not verify")
        );
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_document_scopes_each_variable_to_the_invokes_naming_it() {
        let dir = std::env::temp_dir().join(format!("scx-env-flags-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temporary directory");
        let file = dir.join(".env");
        fs::write(&file, "KEY=from-file\nFILED=kept\nUNNAMED=dropped\n").expect("env file");
        let mut flags = RunFlags::default();
        flags
            .accept("--env-file", &file.display().to_string())
            .expect("PATH");
        flags.accept("--env", "KEY=from-flag").expect("NAME=VALUE");
        flags
            .accept("--env", "SCX_TEST_UNSET_VARIABLE")
            .expect("NAME");
        assert!(flags.accept("--env", "KEY").is_err(), "one flag per NAME");
        assert!(flags.accept("--env", "1A").is_err());
        assert!(
            flags.accept("--env", "KEY@urn:a").is_err(),
            "the NAME@URI grammar is gone"
        );
        let source = br#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:x" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0"><state id="s"><onentry><x:a env:apikey="IGNORED"/></onentry><invoke type="urn:y" env:baseurl="FILED"/><invoke type="urn:svc" env:apikey="KEY"/><invoke type="urn:x" env:apikey="KEY"/><invoke type="scxml" src="c.scx"/></state></scxml>"#;
        let uses = slot_uses(source);
        assert_eq!(uses.len(), 3, "only invokes bind slots: {uses:?}");
        let values = flags.values(&uses).expect("values");
        assert_eq!(
            values,
            [
                HeldValue {
                    name: "FILED".into(),
                    value: "kept".into(),
                    packages: vec!["urn:y".into()],
                },
                HeldValue {
                    name: "KEY".into(),
                    value: "from-flag".into(),
                    packages: vec!["urn:svc".into(), "urn:x".into()],
                },
            ],
            "a flag's value wins over the file; a variable no slot names is dropped"
        );
        assert!(!format!("{values:?}").contains("from-flag"), "Debug masks");
        let expression = br#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0"><state id="s"><invoke type="urn:x" env:apikeyexpr="'UNNAMED'"/></state></scxml>"#;
        let values = flags.values(&slot_uses(expression)).expect("values");
        assert!(
            values
                .iter()
                .all(|held| held.packages == ["urn:x".to_owned()]),
            "an expression slot's package may receive any variable the run holds: {values:?}"
        );
        assert_eq!(values.len(), 3, "{values:?}");
        assert!(
            flags.accept("--extension-http-allow", "x").is_err(),
            "removed"
        );
        assert!(flags.accept("--inherit-env", "").is_err(), "removed");
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_summaries_list_consents_and_never_a_value() {
        let source = br#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0"><state id="s"><invoke type="urn:svc" env:apikey="API_KEY" env:baseurlexpr="b"/></state></scxml>"#;
        let manifest =
            ExtensionPackageManifest::from_json(manifest_json("").as_bytes()).expect("manifest");
        let packages = Packages {
            repository: vec![Package {
                origin: PackageOrigin::Repository {
                    uri: "urn:x".into(),
                    tag: "v1".into(),
                    commit: "c".repeat(40),
                },
                manifest,
                schema: SchemaSource::new("x.xsd", Vec::new()),
                validator: None,
            }],
            slot_uses: slot_uses(source),
            ..Packages::default()
        };
        let summary = packages.summary().join("\n");
        assert!(summary.contains("element `urn:x`"), "{summary}");
        assert!(summary.contains(&format!("release v1 (commit {})", "c".repeat(40))));
        let held = HeldValue {
            name: "API_KEY".into(),
            value: "secret-value".into(),
            packages: vec!["urn:svc".into()],
        };
        let env = env_summary(&packages.slot_uses, &[held]).join("\n");
        assert!(
            env.contains("env `API_KEY`\n  = to: <invoke type=\"urn:svc\"> env:apikey"),
            "{env}"
        );
        assert!(
            env.contains("env (expression)\n  = to: <invoke type=\"urn:svc\"> env:baseurlexpr"),
            "{env}"
        );
        assert!(!env.contains("secret-value"), "never a value: {env}");
        let unheld = env_summary(&packages.slot_uses, &[]).join("\n");
        assert!(unheld.contains("env `API_KEY` (no value)"), "{unheld}");
    }

    #[test]
    fn component_flags_pair_with_digests() {
        let mut flags = RunFlags::default();
        flags
            .accept(
                "--extension-component-sha256",
                &format!("urn:x={}", "a".repeat(64)),
            )
            .expect("URI=HEX");
        assert!(
            flags
                .accept("--extension-component-sha256", "urn:y=ABC")
                .is_err()
        );
        flags
            .accept("--extension-component", "urn:z=z.wasm")
            .expect("URI=PATH");
        assert!(
            flags
                .check(&Packages::default())
                .expect_err("flags naming unbound URIs are rejected")
                .contains("urn:")
        );
    }
}
