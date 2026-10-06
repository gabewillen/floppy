//! extension package flags (SCXML 1.0 section 4.10).
//!
//! * `--extension-package MANIFEST.json` (repeatable; `validate`, `compile`,
//!   `snapshot run`, and `run`) binds one extension package: a
//!   `stateforward.scxml.extension-package/v2` manifest naming the exact
//!   namespace URI, package version, extension artifact, schema artifact and
//!   its SHA-256, WIT world and version, the outbound HTTP origins or URL
//!   prefixes its extension component may reach (`http-origins`: binding the
//!   package consents to them), and the `env` slots its elements accept
//!   (`env-slots`). The schema artifact is resolved relative to the
//!   manifest's directory, read within its byte bound, and checked against
//!   the manifest before use; it then governs the namespace's XML structure
//!   (so `--schema` may not name the same URI). `compile` also hands the
//!   manifest to the compiler, which requires one for every used extension
//!   namespace.
//! * A namespace the document uses whose URI is a repository namespace
//!   (`https://github.com/<owner>/<repo>/namespaces/<name>@<tag>`, see
//!   [`crate::repository`]) and that no flag binds is resolved from the
//!   extension cache or its repository: its package for validation and
//!   compilation, its extension component when a session starts.
//!   `--offline`, `--cache-dir DIR`, and the `--github-*` flags govern that.
//! * `snapshot run` and `run` execute an `--extension-package` namespace's
//!   extension component, resolved relative to the manifest
//!   (`extension-artifact`) or by `--extension-component URI=PATH`, and
//!   authenticated by the SHA-256 in `--extension-component-sha256 URI=HEX`
//!   (required for every such package; the manifest is not a trust anchor).
//! * `--env NAME@URI` and `--env NAME@URI=VALUE` (repeatable) grant the
//!   variable `NAME` to the package whose exact namespace or service type URI
//!   is `URI` (repeat the flag to grant one variable to several packages);
//!   `--env-file PATH` (repeatable; Docker's format, [`crate::env_file`])
//!   supplies values. A granted variable's value is the flag's `VALUE`, else
//!   the last `--env-file` entry for it, else the calling shell's; an
//!   `--env-file` entry no flag grants is dropped. An element (or an
//!   `<invoke>`) names a variable for one of its slots with
//!   `env:<slot>="NAME"`; when the variable is granted to its package, its
//!   component receives an opaque reference, never the value, and the host
//!   substitutes the value only into its admitted outbound requests; a
//!   variable granted only to other packages leaves the slot absent, exactly
//!   like one with no value. The values are masked in
//!   the text the CLI writes ([`crate::mask`]).
//! * Every command that reads a document lists, per bound element, its
//!   namespace, version, declared outbound origins, and the slots the
//!   document binds (with the variables they name): what importing it
//!   consents to. Values never appear.
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
    DeclaredEvents, ExtensionPackageManifest, MAX_COMPONENT_SCHEMA_BYTES, MemorySchemaResolver,
    PackageEvents, SchemaResolveError, SchemaResolver, SchemaSource,
};

use crate::repository::{Repository, RepositoryFlags, RepositoryUri};

/// Byte bound of one manifest file.
const MAX_MANIFEST_BYTES: u64 = crate::repository::MAX_MANIFEST_BYTES;

/// Where a package came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PackageOrigin {
    /// An `--extension-package` manifest file.
    Manifest(PathBuf),
    /// A repository namespace (the extension cache or its repository), at the
    /// commit its release tag pinned.
    Repository {
        /// The release tag (the URI's suffix).
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
    /// The manifest's declared event contract, every schema verified and
    /// resolved.
    pub(crate) events: DeclaredEvents,
}

impl Package {
    /// The package as messages name it: the manifest path, or the
    /// repository namespace's URI.
    pub(crate) fn label(&self) -> String {
        match &self.origin {
            PackageOrigin::Manifest(path) => path.display().to_string(),
            PackageOrigin::Repository { .. } => self.manifest.namespace_uri.clone(),
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
            env_slots: manifest.env_slots.clone(),
            http_origins: manifest.http_origins.clone(),
            wasi_permissions: manifest.wasi_permissions.clone(),
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
    /// Repository namespaces the document uses.
    pub(crate) repository: Vec<Package>,
    /// The repository store, opened only when the document uses one.
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "the session host's resolver reads it, which needs the wasmtime feature"
        )
    )]
    pub(crate) store: Option<Arc<Repository>>,
    /// Every `env` slot binding the document's elements make.
    pub(crate) slot_uses: Vec<SlotUse>,
}

impl Packages {
    /// Loads the `--extension-package` manifests and resolves every
    /// repository namespace `source` uses that neither they nor `--schema`
    /// bind.
    ///
    /// # Errors
    ///
    /// As [`load`], an unopenable extension cache, or a repository namespace
    /// that cannot be resolved.
    pub(crate) fn resolve(
        source: &[u8],
        manifests: &[PathBuf],
        schema_flags: &[(String, PathBuf)],
        repository: &RepositoryFlags,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let explicit = load(manifests, schema_flags)?;
        let used = crate::repository::used_namespaces(source);
        let bound: Vec<String> = explicit
            .iter()
            .map(|package| package.manifest.namespace_uri.clone())
            .chain(schema_flags.iter().map(|(uri, _)| uri.clone()))
            .collect();
        let wanted = used
            .iter()
            .any(|uri| !bound.contains(uri) && RepositoryUri::parse(uri).is_some());
        let (store, repository_packages) = if wanted {
            let store = Arc::new(repository.open()?);
            let packages = crate::repository::resolve_document_packages(&store, source, &bound)?;
            (Some(store), packages)
        } else {
            (None, Vec::new())
        };
        Ok(Self {
            explicit,
            repository: repository_packages,
            store,
            slot_uses: slot_uses(source),
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

    /// The `env-slots` of every package, by exact namespace URI (for the
    /// validator's `SCXML083`).
    pub(crate) fn slots(&self) -> BTreeMap<String, Vec<String>> {
        self.explicit
            .iter()
            .chain(&self.repository)
            .map(|package| {
                (
                    package.manifest.namespace_uri.clone(),
                    package.manifest.env_slots.clone(),
                )
            })
            .collect()
    }

    /// Whether any package declares a WASI permission.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn declares_wasi(&self) -> bool {
        self.explicit
            .iter()
            .chain(&self.repository)
            .any(|package| !package.manifest.wasi_permissions.is_empty())
    }

    /// What importing each element consents to: its namespace, version,
    /// declared outbound origins, the slots the document binds (with the
    /// variables they name and, when `granted` is given, whether the run
    /// holds a value for each granted to that namespace), and its declared
    /// WASI permissions (which this release's host refuses for extension
    /// components). Never a value.
    pub(crate) fn summary(&self, granted: Option<&[HeldValue]>) -> Vec<String> {
        let mut lines = Vec::new();
        for package in self.explicit.iter().chain(&self.repository) {
            let manifest = &package.manifest;
            let uri = &manifest.namespace_uri;
            let version = match &package.origin {
                PackageOrigin::Manifest(path) => format!(
                    "package {} from --extension-package {}",
                    manifest.package_version,
                    path.display()
                ),
                PackageOrigin::Repository { tag, commit } => format!(
                    "release {tag} (commit {commit}), package {}",
                    manifest.package_version
                ),
            };
            let origins = if manifest.http_origins.is_empty() {
                "none".to_owned()
            } else {
                manifest.http_origins.join(", ")
            };
            let mut slots: Vec<String> = self
                .slot_uses
                .iter()
                .filter(|binding| &binding.namespace_uri == uri)
                .map(|binding| match (&binding.name, granted) {
                    (SlotName::Variable(name), Some(granted))
                        if granted.iter().any(|held| held.grants(name, uri)) =>
                    {
                        format!("{} <- {name}", binding.slot)
                    }
                    (SlotName::Variable(name), Some(_)) => format!(
                        "{} <- {name} (no value granted: add --env {name}@{uri})",
                        binding.slot
                    ),
                    (SlotName::Variable(name), None) => format!("{} <- {name}", binding.slot),
                    (SlotName::Expression, _) => format!("{} <- (expression)", binding.slot),
                })
                .collect();
            slots.sort();
            slots.dedup();
            let wasi = if manifest.wasi_permissions.is_empty() {
                "none".to_owned()
            } else {
                manifest
                    .wasi_permissions
                    .iter()
                    .map(scxml_types::WasiPermission::describe)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            lines.push(format!(
                "element `{uri}`\n  = version: {version}\n  = outbound HTTP: {origins}\n  = env slots: {}\n  = WASI: {wasi}{}",
                if slots.is_empty() {
                    "none".to_owned()
                } else {
                    slots.join(", ")
                },
                events_line(&package.events).unwrap_or_default()
            ));
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

/// One `env` attribute of an extension element in the document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SlotUse {
    /// The element's exact namespace URI.
    pub(crate) namespace_uri: String,
    /// The slot (the attribute's local name, `expr` stripped).
    pub(crate) slot: String,
    /// The variable it names.
    pub(crate) name: SlotName,
}

/// Every `env` attribute on an element in another extension namespace, in
/// document order; a document that does not parse binds none here.
fn slot_uses(source: &[u8]) -> Vec<SlotUse> {
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
        let Some(namespace_uri) = element.name.namespace_uri.as_deref() else {
            return;
        };
        if namespace_uri == scxml_validator_core::SCXML_NAMESPACE
            || namespace_uri == scxml_types::ENV_NAMESPACE_URI
        {
            return;
        }
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
    let events = load_package_events(path, &manifest.events, "extension package")
        .map_err(|error| format!("extension package `{shown}` does not verify: {error}"))?;
    Ok(Package {
        origin: PackageOrigin::Manifest(path.to_path_buf()),
        manifest,
        schema,
        events,
    })
}

/// Byte bound of one package event schema file (the component request's
/// per-resource bound for `event:schema` files).
pub(crate) const MAX_EVENT_SCHEMA_BYTES: u64 = 4 * 1024 * 1024;

const _: () = assert!(
    MAX_EVENT_SCHEMA_BYTES == scxml_event_schema::MAX_REQUEST_RESOURCE_BYTES as u64,
    "package event schemas are bounded like event:schema files"
);

/// Reads the schema files a manifest's `events` names, relative to the
/// manifest, and verifies and resolves every declared schema.
///
/// # Errors
///
/// A message for an unreadable or oversized file, or every
/// [`PackageEvents::resolve`] failure.
pub(crate) fn load_package_events(
    manifest: &Path,
    events: &PackageEvents,
    package: &str,
) -> Result<DeclaredEvents, String> {
    events.check(package)?;
    let mut files = BTreeMap::new();
    for file in events.schema_files() {
        let decoded = percent_decoded(file).ok_or_else(|| {
            format!(
                "{package} field `events`: a schema file path is not UTF-8 once percent-decoded"
            )
        })?;
        let path = relative_to_manifest(manifest, &decoded, "events")?;
        let bytes = read_bounded(&path, MAX_EVENT_SCHEMA_BYTES).map_err(|error| {
            format!(
                "{package}: cannot read its event schema `{}`: {error}",
                path.display()
            )
        })?;
        files.insert(file.to_owned(), bytes);
    }
    events.resolve(package, &files)
}

/// `text` with `%XX` escapes decoded, when the result is UTF-8 (a schema
/// file reference is a URI path, like an `event:schema` reference).
pub(crate) fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(value);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The consent line of a declared event contract, or `None` when the
/// package declares none.
pub(crate) fn events_line(events: &DeclaredEvents) -> Option<String> {
    if !events.declared {
        return None;
    }
    let names = |list: &[scxml_validator_core::DeclaredEvent]| {
        if list.is_empty() {
            "none".to_owned()
        } else {
            list.iter()
                .map(|event| event.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    Some(format!(
        "\n  = events: emits {}; accepts {}",
        names(&events.emits),
        names(&events.accepts)
    ))
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
    /// The exact package URI it is granted to.
    uri: String,
    /// `=VALUE`, when given.
    value: Option<String>,
}

/// One variable the run holds a value for, and the exact package URIs
/// (namespace or service type URIs) it is granted to. `Debug` names the
/// variable and its packages only.
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
    /// Whether this is `name`, granted to `uri`.
    pub(crate) fn grants(&self, name: &str, uri: &str) -> bool {
        self.name == name && self.packages.iter().any(|granted| granted == uri)
    }

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
                const USAGE: &str = "--env requires NAME@URI or NAME@URI=VALUE, NAME being [A-Za-z_][A-Za-z0-9_]* and URI the exact namespace or service type URI granted the variable";
                let (name, rest) = value.split_once('@').ok_or(USAGE)?;
                let (uri, value) = match rest.split_once('=') {
                    Some((uri, value)) => (uri, Some(value.to_owned())),
                    None => (rest, None),
                };
                if !crate::env_file::is_name(name) || scxml_types::NamespaceUri::new(uri).is_err() {
                    return Err(USAGE.into());
                }
                if self
                    .env
                    .iter()
                    .any(|seen| seen.name == name && seen.uri == uri)
                {
                    return Err("--env was given twice for one NAME@URI".into());
                }
                if value.is_some()
                    && self
                        .env
                        .iter()
                        .any(|seen| seen.name == name && seen.value.is_some())
                {
                    return Err("--env gave two values for one NAME".into());
                }
                self.env.push(EnvFlag {
                    name: name.to_owned(),
                    uri: uri.to_owned(),
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
        if let Some(package) = packages
            .explicit
            .iter()
            .find(|package| !self.digests.contains_key(&package.manifest.namespace_uri))
        {
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
        self.digests
            .get(&package.manifest.namespace_uri)
            .map(String::as_str)
    }

    /// The run's values: exactly the variables an `--env` flag grants, each
    /// scoped to every URI its flags name. A variable's value is its flag's
    /// `=VALUE`, else the last `--env-file` entry for it (every file read
    /// once, now, in order), else the calling shell's; a variable none of
    /// them sets holds no value. `--env-file` entries no flag grants are
    /// dropped. Sorted by name.
    ///
    /// # Errors
    ///
    /// An unreadable, oversized, or malformed `--env-file` (the message names
    /// the file and line, never a value).
    pub(crate) fn values(&self) -> Result<Vec<HeldValue>, String> {
        let mut files = BTreeMap::new();
        for path in &self.env_files {
            for (name, value) in crate::env_file::read(path)? {
                files.insert(name, value);
            }
        }
        let mut granted: BTreeMap<&str, (Option<&str>, BTreeSet<&str>)> = BTreeMap::new();
        for flag in &self.env {
            let entry = granted.entry(flag.name.as_str()).or_default();
            if let Some(value) = &flag.value {
                entry.0 = Some(value);
            }
            entry.1.insert(flag.uri.as_str());
        }
        Ok(granted
            .into_iter()
            .filter_map(|(name, (flag_value, packages))| {
                let value = match flag_value {
                    Some(value) => Some(value.to_owned()),
                    None => files
                        .get(name)
                        .cloned()
                        .or_else(|| std::env::var(name).ok()),
                }?;
                Some(HeldValue {
                    name: name.to_owned(),
                    value,
                    packages: packages.into_iter().map(str::to_owned).collect(),
                })
            })
            .collect())
    }
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

/// The note for a request an extension component made that its package does
/// not declare and no value supplied.
#[cfg(any(feature = "wasmtime", test))]
pub(crate) fn refused_note(namespace_uri: &str, origin: &str) -> String {
    format!(
        "note: `{namespace_uri}` was refused outbound HTTP to `{origin}`: its package does not declare it, and no --env value supplied it\n  = help: a package declares its origins in its manifest's `http-origins`; a run supplies an origin through an `env` slot's value (a base URL, for example)"
    )
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
            r#"{{"format":"{EXTENSION_PACKAGE_FORMAT}","namespace-uri":"urn:x","package-version":"1","extension-artifact":"x.wasm","schema-artifact":"x.xsd","schema-sha256":"{digest}","wit-world":"stateforward:scxml/executable-content-extension","wit-version":"0.2.0","http-origins":["https://api.example"],"env-slots":["apikey"]{extra}}}"#
        )
    }

    #[test]
    fn manifests_are_exact_versioned_objects() {
        let manifest = ExtensionPackageManifest::from_json(manifest_json("").as_bytes())
            .expect("valid manifest");
        assert_eq!(manifest.namespace_uri, "urn:x");
        assert_eq!(manifest.http_origins, ["https://api.example"]);
        assert_eq!(manifest.env_slots, ["apikey"]);
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
        for (from, to) in [
            (r#""env-slots":["apikey"]"#, r#""env-slots":"apikey""#),
            (r#""env-slots":["apikey"]"#, r#""env-slots":[1]"#),
            (r#","env-slots":["apikey"]"#, ""),
        ] {
            assert!(
                ExtensionPackageManifest::from_json(manifest_json("").replace(from, to).as_bytes())
                    .is_err(),
                "{to}"
            );
        }
        let many = format!(
            r#""env-slots":[{}]"#,
            (0..=scxml_validator_core::MAX_EXTENSION_PACKAGE_LIST_ITEMS)
                .map(|index| format!(r#""s{index}""#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(
            ExtensionPackageManifest::from_json(
                manifest_json("")
                    .replace(r#""env-slots":["apikey"]"#, &many)
                    .as_bytes()
            )
            .is_err()
        );
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
        let dir = std::env::temp_dir().join(format!("scxml-cli-namespaces-{}", std::process::id()));
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
        fs::write(
            &path,
            manifest_json("").replace(r#"["apikey"]"#, r#"["ApiKey"]"#),
        )
        .expect("manifest");
        assert!(
            load(std::slice::from_ref(&path), &[])
                .expect_err("an invalid slot fails verification")
                .to_string()
                .contains("does not verify")
        );
        fs::write(&path, manifest_json("")).expect("manifest");
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
    fn env_flags_grant_each_variable_to_named_packages() {
        let dir = std::env::temp_dir().join(format!("scxml-cli-env-flags-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temporary directory");
        let file = dir.join(".env");
        fs::write(&file, "KEY=from-file\nFILED=kept\nUNGRANTED=dropped\n").expect("env file");
        let mut flags = RunFlags::default();
        flags
            .accept("--env-file", &file.display().to_string())
            .expect("PATH");
        flags
            .accept("--env", "KEY@urn:a=from-flag")
            .expect("NAME@URI=VALUE");
        flags
            .accept("--env", "KEY@urn:b")
            .expect("a second package");
        flags
            .accept("--env", "FILED@https://example.com/ns@v1")
            .expect("a URI holding `@`");
        assert!(
            flags.accept("--env", "KEY@urn:b").is_err(),
            "one NAME@URI once"
        );
        assert!(
            flags.accept("--env", "KEY@urn:c=other").is_err(),
            "one value per NAME"
        );
        assert!(flags.accept("--env", "KEY").is_err(), "no unscoped form");
        assert!(flags.accept("--env", "KEY=v").is_err(), "no unscoped form");
        assert!(flags.accept("--env", "1A@urn:a").is_err());
        assert!(flags.accept("--env", "A@not a uri").is_err());
        assert!(flags.accept("--env", "A@").is_err());
        let values = flags.values().expect("values");
        assert_eq!(
            values,
            [
                HeldValue {
                    name: "FILED".into(),
                    value: "kept".into(),
                    packages: vec!["https://example.com/ns@v1".into()],
                },
                HeldValue {
                    name: "KEY".into(),
                    value: "from-flag".into(),
                    packages: vec!["urn:a".into(), "urn:b".into()],
                },
            ],
            "a flag's value wins over the file; ungranted entries are dropped"
        );
        assert!(values[1].grants("KEY", "urn:b"));
        assert!(!values[1].grants("KEY", "urn:c"), "out of scope");
        assert!(!format!("{values:?}").contains("from-flag"), "Debug masks");
        assert!(
            flags.accept("--extension-http-allow", "x").is_err(),
            "removed"
        );
        assert!(flags.accept("--inherit-env", "").is_err(), "removed");
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_summary_lists_what_an_element_consents_to_and_never_a_value() {
        let source = br#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:x" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0"><state id="s"><onentry><x:a env:apikey="API_KEY" env:baseurlexpr="b"/></onentry></state></scxml>"#;
        let manifest =
            ExtensionPackageManifest::from_json(manifest_json("").as_bytes()).expect("manifest");
        let packages = Packages {
            repository: vec![Package {
                origin: PackageOrigin::Repository {
                    tag: "v1".into(),
                    commit: "c".repeat(40),
                },
                manifest,
                schema: SchemaSource::new("x.xsd", Vec::new()),
                events: DeclaredEvents::default(),
            }],
            slot_uses: slot_uses(source),
            ..Packages::default()
        };
        let summary = packages.summary(Some(&[])).join("\n");
        assert!(summary.contains("element `urn:x`"), "{summary}");
        assert!(summary.contains(&format!("release v1 (commit {})", "c".repeat(40))));
        assert!(summary.contains("outbound HTTP: https://api.example"));
        assert!(
            summary.contains("apikey <- API_KEY (no value granted: add --env API_KEY@urn:x)"),
            "{summary}"
        );
        assert!(summary.contains("baseurl <- (expression)"));
        let held = |uri: &str| HeldValue {
            name: "API_KEY".into(),
            value: "v".into(),
            packages: vec![uri.into()],
        };
        let granted = packages.summary(Some(&[held("urn:x")])).join("\n");
        assert!(!granted.contains("no value"), "{granted}");
        let elsewhere = packages.summary(Some(&[held("urn:y")])).join("\n");
        assert!(
            elsewhere.contains("(no value granted: add --env API_KEY@urn:x)"),
            "a value granted to another package is not this element's: {elsewhere}"
        );
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
        assert!(refused_note("urn:a", "https://x.example").contains("`https://x.example`"));
    }
}
