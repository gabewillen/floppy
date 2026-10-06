//! Invoked-service package flags (SCXML 6.4 `<invoke type>`).
//!
//! * `--service-package MANIFEST.json` (repeatable; `validate`, `compile`,
//!   `snapshot run`, and `run`) binds one invoked service: a
//!   `stateforward.scxml.service-package/v1` manifest naming the exact
//!   service type URI, package version, service artifact, optional schema
//!   artifact and its SHA-256, WIT world and version, the outbound HTTP
//!   origins the service component may reach (`http-origins`: binding the
//!   package consents to them), and the `env` slots it accepts. A schema
//!   artifact, when named, is read relative to the manifest and checked
//!   against its digest. `validate` reports a literal `<invoke type>` no
//!   package binds (`SCXML094`); `compile` rejects it (`COMP004`).
//! * `snapshot run` and `run` execute each bound service's component,
//!   resolved relative to the manifest (`service-artifact`) or by
//!   `--service-component URI=PATH`, and authenticated by the SHA-256 in
//!   `--service-component-sha256 URI=HEX` (required for every package; the
//!   manifest is not a trust anchor).
//! * A package may declare WASI permissions (`wasi-permissions`): the WASI
//!   interfaces its component may import. `snapshot run` and `run` link only
//!   those the run grants with `--grant URI=PERMISSION[=VALUE]`
//!   (`wasi:clocks`, `wasi:random`, and `wasi:sockets/ip-name-lookup` take no
//!   value; `wasi:filesystem=MOUNT=DIR` maps a declared mount to an existing
//!   host directory; `wasi:sockets/tcp=IP:PORT[,IP:PORT]...` lists the
//!   addresses it may connect to). Every declared permission must be granted
//!   and every grant must name a declared permission of a bound service,
//!   checked before anything runs. A run that declares any WASI permission
//!   cannot be recorded or replayed (`--record`, `--replay`): its effects
//!   and inputs are outside the log.
//! * Every command that reads a document lists, per bound service, its type
//!   URI, version, declared outbound origins, declared slots, and declared
//!   WASI permissions (with what the run grants each): what binding it
//!   consents to.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use scxml_types::{WasiInterface, WasiPermission};

use scxml_telemetry::{ErrorCategory, Module, Operation, observe};
use scxml_validator_core::ServicePackageManifest;

use crate::extensions::read_bounded;

/// Byte bound of one manifest file.
const MAX_MANIFEST_BYTES: u64 = crate::repository::MAX_MANIFEST_BYTES;
/// Byte bound of one service schema artifact.
pub(crate) const MAX_SCHEMA_BYTES: u64 = 4 * 1024 * 1024;

/// Where a service package came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ServiceOrigin {
    /// A `--service-package` manifest file.
    Manifest(PathBuf),
    /// A repository service (the extension cache or its repository), at the
    /// commit its release tag pinned; its component comes from the release.
    Repository {
        /// The release tag (the URI's suffix).
        tag: String,
        /// The commit the tag named when the cache pinned it.
        commit: String,
    },
}

/// One verified service package: a `--service-package`, or a repository
/// service the document invokes.
#[derive(Clone, Debug)]
pub(crate) struct ServicePackage {
    /// Where it came from.
    pub(crate) origin: ServiceOrigin,
    /// The verified manifest.
    pub(crate) manifest: ServicePackageManifest,
    /// The manifest's declared event contract, every schema verified and
    /// resolved.
    pub(crate) events: scxml_validator_core::DeclaredEvents,
}

impl ServicePackage {
    /// The package as messages name it: the manifest path, or the
    /// repository service's URI.
    pub(crate) fn label(&self) -> String {
        match &self.origin {
            ServiceOrigin::Manifest(path) => path.display().to_string(),
            ServiceOrigin::Repository { .. } => self.manifest.service_uri.clone(),
        }
    }

    /// The service artifact's default path: `service-artifact` relative to
    /// the manifest's directory (a `--service-package` only).
    #[cfg(any(feature = "wasmtime", test))]
    pub(crate) fn component_path(&self) -> Result<PathBuf, String> {
        match &self.origin {
            ServiceOrigin::Manifest(path) => {
                relative_to_manifest(path, &self.manifest.service_artifact, "service-artifact")
            }
            ServiceOrigin::Repository { .. } => {
                Err("a repository service's component comes from its release".into())
            }
        }
    }

    /// The package as the session host binds it.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn host_package(&self) -> scxml_host_wasmtime::ServicePackage {
        let manifest = &self.manifest;
        scxml_host_wasmtime::ServicePackage {
            service_uri: manifest.service_uri.clone(),
            package_version: manifest.package_version.clone(),
            service_artifact: manifest.service_artifact.clone(),
            wit_world: manifest.wit_world.clone(),
            wit_version: manifest.wit_version.clone(),
            http_origins: manifest.http_origins.clone(),
            env_slots: manifest.env_slots.clone(),
            wasi_permissions: manifest.wasi_permissions.clone(),
            events: scxml_host_wasmtime::ServiceEvents {
                emits: host_events(&self.events.emits),
                accepts: host_events(&self.events.accepts),
            },
        }
    }
}

/// Declared events as the session host binds them.
#[cfg(feature = "wasmtime")]
fn host_events(
    events: &[scxml_validator_core::DeclaredEvent],
) -> Vec<scxml_host_wasmtime::ServiceEventSchema> {
    events
        .iter()
        .map(|event| scxml_host_wasmtime::ServiceEventSchema {
            name: event.name.clone(),
            schema: event.schema.clone(),
        })
        .collect()
}

/// Whether any of `packages` declares a WASI permission.
#[cfg(any(feature = "wasmtime", test))]
pub(crate) fn declares_wasi(packages: &[ServicePackage]) -> bool {
    packages
        .iter()
        .any(|package| !package.manifest.wasi_permissions.is_empty())
}

/// Reads and verifies every `--service-package` manifest.
///
/// # Errors
///
/// A message naming the manifest for an unreadable, oversized, malformed,
/// or unverifiable manifest or schema, or a service URI bound twice.
pub(crate) fn load(
    manifests: &[PathBuf],
) -> Result<Vec<ServicePackage>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadServicePackage);
    let result = (|| {
        let mut packages: Vec<ServicePackage> = Vec::with_capacity(manifests.len());
        for path in manifests {
            let package = load_one(path)?;
            if packages
                .iter()
                .any(|bound| bound.manifest.service_uri == package.manifest.service_uri)
            {
                return Err(format!(
                    "service package `{}` binds a service URI another --service-package already binds",
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

fn load_one(path: &Path) -> Result<ServicePackage, Box<dyn std::error::Error>> {
    let shown = path.display();
    let bytes = read_bounded(path, MAX_MANIFEST_BYTES)
        .map_err(|error| format!("cannot read service package `{shown}`: {error}"))?;
    let manifest = ServicePackageManifest::from_json(&bytes)
        .map_err(|error| format!("service package `{shown}` is malformed: {error}"))?;
    let schema = match &manifest.schema_artifact {
        Some(artifact) => {
            let schema_path = relative_to_manifest(path, artifact, "schema-artifact")
                .map_err(|error| format!("service package `{shown}`: {error}"))?;
            Some(
                read_bounded(&schema_path, MAX_SCHEMA_BYTES).map_err(|error| {
                    format!(
                        "service package `{shown}`: cannot read its schema `{}`: {error}",
                        schema_path.display()
                    )
                })?,
            )
        }
        None => None,
    };
    manifest
        .verify(schema.as_deref())
        .map_err(|error| format!("service package `{shown}` does not verify: {error}"))?;
    let events = crate::extensions::load_package_events(path, &manifest.events, "service package")
        .map_err(|error| format!("service package `{shown}` does not verify: {error}"))?;
    Ok(ServicePackage {
        origin: ServiceOrigin::Manifest(path.to_path_buf()),
        manifest,
        events,
    })
}

/// The services of one command: the `--service-package` ones, then the
/// repository services the document invokes that none of them binds.
#[derive(Debug, Default)]
pub(crate) struct Services {
    /// Every package, `--service-package` ones first.
    pub(crate) packages: Vec<ServicePackage>,
    /// The repository store, opened only when the document invokes a
    /// repository service.
    #[cfg_attr(
        not(feature = "wasmtime"),
        allow(
            dead_code,
            reason = "only a run binds repository components, which needs the wasmtime feature"
        )
    )]
    pub(crate) store: Option<std::sync::Arc<crate::repository::Repository>>,
}

/// Loads the `--service-package` manifests and resolves every repository
/// service (`https://github.com/<owner>/<repo>/services/<name>@<tag>`) the
/// document names as a literal `<invoke type>` that none of them binds,
/// through the repository flags ([`crate::repository`]: bounded, pinned,
/// content-addressed, `--offline`, fail closed).
///
/// # Errors
///
/// As [`load`], an unopenable extension cache, or a repository service that
/// cannot be resolved.
pub(crate) fn resolve(
    source: &[u8],
    manifests: &[PathBuf],
    repository: &crate::repository::RepositoryFlags,
) -> Result<Services, Box<dyn std::error::Error>> {
    let mut packages = load(manifests)?;
    let bound: Vec<String> = packages
        .iter()
        .map(|package| package.manifest.service_uri.clone())
        .collect();
    let wanted = crate::repository::used_services(source, &bound);
    if wanted.is_empty() {
        return Ok(Services {
            packages,
            store: None,
        });
    }
    let store = std::sync::Arc::new(repository.open()?);
    for uri in wanted {
        packages.push(store.resolve_service_package(&uri)?);
    }
    Ok(Services {
        packages,
        store: Some(store),
    })
}

/// The exact service type URIs `packages` bind (for `SCXML094` and
/// `COMP004`).
pub(crate) fn bound_types(packages: &[ServicePackage]) -> BTreeSet<String> {
    packages
        .iter()
        .map(|package| package.manifest.service_uri.clone())
        .collect()
}

/// What the validator knows of each bound service, by exact type URI: its
/// declared `env` slots and event contract.
pub(crate) fn bound_services(
    packages: &[ServicePackage],
) -> BTreeMap<String, scxml_validator_core::BoundService> {
    packages
        .iter()
        .map(|package| {
            (
                package.manifest.service_uri.clone(),
                scxml_validator_core::BoundService {
                    env_slots: package.manifest.env_slots.clone(),
                    events: package.events.clone(),
                },
            )
        })
        .collect()
}

/// What binding each service consents to: its type URI, version, declared
/// outbound origins, declared slots, and declared WASI permissions, each
/// with what `grants` (the run's, when it runs the services) grants it.
/// Never an `env` value.
pub(crate) fn summary(packages: &[ServicePackage], grants: Option<&ServiceFlags>) -> Vec<String> {
    packages
        .iter()
        .map(|package| {
            let manifest = &package.manifest;
            let list = |items: &[String]| {
                if items.is_empty() {
                    "none".to_owned()
                } else {
                    items.join(", ")
                }
            };
            let wasi: Vec<String> = manifest
                .wasi_permissions
                .iter()
                .map(|permission| {
                    let granted = grants.map(|flags| {
                        flags
                            .grants
                            .iter()
                            .find(|(uri, grant)| {
                                uri == &manifest.service_uri && grant.covers(permission)
                            })
                            .map(|(_, grant)| grant)
                    });
                    match granted {
                        None => permission.describe(),
                        Some(Some(grant)) => format!("{} <- {}", permission.describe(), grant.value()),
                        Some(None) => format!(
                            "{} (not granted: add --grant {}={})",
                            permission.describe(),
                            manifest.service_uri,
                            grant_hint(permission)
                        ),
                    }
                })
                .collect();
            let version = match &package.origin {
                ServiceOrigin::Manifest(path) => format!(
                    "package {} from --service-package {}",
                    manifest.package_version,
                    path.display()
                ),
                ServiceOrigin::Repository { tag, commit } => format!(
                    "release {tag} (commit {commit}), package {}",
                    manifest.package_version
                ),
            };
            format!(
                "service `{}`\n  = version: {}\n  = outbound HTTP: {}\n  = env slots: {}\n  = WASI: {}{}",
                manifest.service_uri,
                version,
                list(&manifest.http_origins),
                list(&manifest.env_slots),
                list(&wasi),
                crate::extensions::events_line(&package.events).unwrap_or_default(),
            )
        })
        .collect()
}

/// The `--grant` value that would grant `permission` (with placeholders).
fn grant_hint(permission: &WasiPermission) -> String {
    match permission {
        WasiPermission::Filesystem { mount, .. } => format!("wasi:filesystem={mount}=DIR"),
        WasiPermission::SocketsTcp => "wasi:sockets/tcp=IP:PORT".to_owned(),
        other => other.interface().name().to_owned(),
    }
}

/// Maximum `--grant` flags of one run.
const MAX_GRANTS: usize = 64;

/// One `--grant` of one service URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Grant {
    /// `wasi:clocks`, `wasi:random`, or `wasi:sockets/ip-name-lookup`.
    Plain(WasiInterface),
    /// `wasi:filesystem=MOUNT=DIR`.
    Filesystem {
        /// The declared mount.
        mount: String,
        /// The host directory.
        directory: PathBuf,
    },
    /// `wasi:sockets/tcp=IP:PORT[,IP:PORT]...`.
    Tcp(Vec<SocketAddr>),
}

impl Grant {
    /// Parses `PERMISSION[=VALUE]`.
    fn parse(text: &str) -> Result<Self, String> {
        let (name, value) = match text.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (text, None),
        };
        let interface = WasiInterface::from_name(name).ok_or_else(|| {
            format!(
                "--grant names an unknown WASI permission `{}` (the permissions are {})",
                name.escape_debug(),
                WasiInterface::ALL.map(WasiInterface::name).join(", ")
            )
        })?;
        match (interface, value) {
            (
                WasiInterface::Clocks | WasiInterface::Random | WasiInterface::SocketsIpNameLookup,
                None,
            ) => Ok(Self::Plain(interface)),
            (
                WasiInterface::Clocks | WasiInterface::Random | WasiInterface::SocketsIpNameLookup,
                Some(_),
            ) => Err(format!("--grant {name} takes no value")),
            (WasiInterface::Filesystem, Some(value)) => match value.split_once('=') {
                Some((mount, directory))
                    if scxml_types::is_wasi_mount(mount) && !directory.is_empty() =>
                {
                    Ok(Self::Filesystem {
                        mount: mount.to_owned(),
                        directory: PathBuf::from(directory),
                    })
                }
                _ => Err(
                    "--grant URI=wasi:filesystem=MOUNT=DIR requires a declared mount name ([a-z][a-z0-9-]*) and a directory"
                        .into(),
                ),
            },
            (WasiInterface::SocketsTcp, Some(value)) => {
                let addresses = value
                    .split(',')
                    .map(str::parse::<SocketAddr>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        "--grant URI=wasi:sockets/tcp=IP:PORT[,IP:PORT]... requires socket addresses (a bracketed IPv6 address for IPv6)".to_owned()
                    })?;
                if addresses.len() > MAX_TCP_GRANT_ADDRESSES {
                    return Err(format!(
                        "--grant wasi:sockets/tcp lists more than {MAX_TCP_GRANT_ADDRESSES} addresses"
                    ));
                }
                Ok(Self::Tcp(addresses))
            }
            (WasiInterface::Filesystem | WasiInterface::SocketsTcp, None) => Err(format!(
                "--grant {name} requires a value ({})",
                if interface == WasiInterface::Filesystem {
                    "wasi:filesystem=MOUNT=DIR"
                } else {
                    "wasi:sockets/tcp=IP:PORT[,IP:PORT]..."
                }
            )),
        }
    }

    /// The interface it grants.
    const fn interface(&self) -> WasiInterface {
        match self {
            Self::Plain(interface) => *interface,
            Self::Filesystem { .. } => WasiInterface::Filesystem,
            Self::Tcp(_) => WasiInterface::SocketsTcp,
        }
    }

    fn key(&self) -> (WasiInterface, Option<&str>) {
        match self {
            Self::Filesystem { mount, .. } => (self.interface(), Some(mount)),
            _ => (self.interface(), None),
        }
    }

    /// Whether it grants exactly `permission`.
    fn covers(&self, permission: &WasiPermission) -> bool {
        self.key() == permission.key()
    }

    /// What it grants, for the consent listing.
    fn value(&self) -> String {
        match self {
            Self::Plain(_) => "granted".to_owned(),
            Self::Filesystem { directory, .. } => directory.display().to_string(),
            Self::Tcp(addresses) => addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// The grant as the host takes it.
    #[cfg(feature = "wasmtime")]
    fn host(&self) -> scxml_host_wasmtime::WasiGrant {
        match self {
            Self::Plain(WasiInterface::Clocks) => scxml_host_wasmtime::WasiGrant::Clocks,
            Self::Plain(WasiInterface::Random) => scxml_host_wasmtime::WasiGrant::Random,
            Self::Plain(_) => scxml_host_wasmtime::WasiGrant::SocketsIpNameLookup,
            Self::Filesystem { mount, directory } => scxml_host_wasmtime::WasiGrant::Filesystem {
                mount: mount.clone(),
                directory: directory.clone(),
            },
            Self::Tcp(addresses) => scxml_host_wasmtime::WasiGrant::SocketsTcp {
                connect: addresses.clone(),
            },
        }
    }
}

/// Maximum addresses of one `wasi:sockets/tcp` grant: the host's
/// `MAX_WASI_TCP_ADDRESSES`, restated so `--grant` is checked the same way
/// with or without the `wasmtime` feature.
const MAX_TCP_GRANT_ADDRESSES: usize = 32;

#[cfg(feature = "wasmtime")]
const _: () = assert!(MAX_TCP_GRANT_ADDRESSES == scxml_host_wasmtime::MAX_WASI_TCP_ADDRESSES);

/// `artifact` inside the manifest's directory: a relative path of plain
/// components only, so a manifest cannot reach outside its own tree.
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

/// `snapshot run` and `run` flags for executing invoked services.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ServiceFlags {
    /// `--service-package` manifests.
    pub(crate) packages: Vec<PathBuf>,
    components: BTreeMap<String, PathBuf>,
    digests: BTreeMap<String, String>,
    /// `--grant` flags, by service URI, in order.
    grants: Vec<(String, Grant)>,
}

impl ServiceFlags {
    /// The flags this module owns, each taking one value.
    pub(crate) const FLAGS: [&'static str; 4] = [
        "--service-package",
        "--service-component",
        "--service-component-sha256",
        "--grant",
    ];

    /// Records one flag.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn accept(&mut self, flag: &str, value: &str) -> Result<(), String> {
        match flag {
            "--service-package" => {
                if value.is_empty() {
                    return Err("--service-package requires MANIFEST.json".into());
                }
                self.packages.push(PathBuf::from(value));
                Ok(())
            }
            "--service-component" => {
                let (uri, path) = split_uri(value, "--service-component requires URI=PATH")?;
                if self.components.insert(uri, PathBuf::from(path)).is_some() {
                    return Err("--service-component was given twice for one URI".into());
                }
                Ok(())
            }
            "--service-component-sha256" => {
                let (uri, digest) =
                    split_uri(value, "--service-component-sha256 requires URI=HEX")?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(
                        "--service-component-sha256 requires 64 lowercase hex characters".into(),
                    );
                }
                if self.digests.insert(uri, digest).is_some() {
                    return Err("--service-component-sha256 was given twice for one URI".into());
                }
                Ok(())
            }
            "--grant" => {
                // The URI ends where the permission (`=wasi:`) begins.
                let Some(at) = value.find("=wasi:").filter(|at| *at > 0) else {
                    return Err("--grant requires URI=PERMISSION[=VALUE] (for example URI=wasi:filesystem=MOUNT=DIR)".into());
                };
                let (uri, permission) = value.split_at(at);
                let grant = Grant::parse(permission.get(1..).unwrap_or_default())?;
                if self
                    .grants
                    .iter()
                    .any(|(bound, granted)| bound == uri && granted.key() == grant.key())
                {
                    return Err("--grant was given twice for one URI and permission".into());
                }
                if self.grants.len() >= MAX_GRANTS {
                    return Err(format!("--grant is limited to {MAX_GRANTS} flags"));
                }
                self.grants.push((uri.to_owned(), grant));
                Ok(())
            }
            other => Err(format!("unknown option `{other}`")),
        }
    }

    /// Checks that every component flag names a `--service-package` URI and
    /// that every package has exactly one digest.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn check(&self, packages: &[ServicePackage]) -> Result<(), String> {
        // A repository service's component comes from its release,
        // authenticated by its `SHA256SUMS`: no component flag names it.
        let bound: BTreeSet<String> = packages
            .iter()
            .filter(|package| matches!(package.origin, ServiceOrigin::Manifest(_)))
            .map(|package| package.manifest.service_uri.clone())
            .collect();
        if let Some(uri) = self
            .components
            .keys()
            .chain(self.digests.keys())
            .find(|uri| !bound.contains(*uri))
        {
            return Err(format!(
                "`{uri}` is not the service URI of any --service-package\n  = help: --service-component and --service-component-sha256 name a bound service URI exactly"
            ));
        }
        if let Some(package) = packages.iter().find(|package| {
            matches!(package.origin, ServiceOrigin::Manifest(_))
                && !self.digests.contains_key(&package.manifest.service_uri)
        }) {
            return Err(format!(
                "service package `{}` needs --service-component-sha256 {}=HEX\n  = help: the SHA-256 of the service component is the trust anchor the run executes it under",
                package.label(),
                package.manifest.service_uri
            ));
        }
        for (uri, grant) in &self.grants {
            let Some(package) = packages
                .iter()
                .find(|package| &package.manifest.service_uri == uri)
            else {
                return Err(format!(
                    "--grant names `{uri}`, which is not the service URI of any --service-package\n  = help: only invoked services get WASI permissions in this release"
                ));
            };
            if !package
                .manifest
                .wasi_permissions
                .iter()
                .any(|permission| grant.covers(permission))
            {
                return Err(format!(
                    "--grant gives `{uri}` {}, which its service package `{}` does not declare\n  = help: a run grants only what a package declares in `wasi-permissions`",
                    grant.interface().name(),
                    package.label()
                ));
            }
            if let Grant::Filesystem { directory, .. } = grant
                && !std::fs::metadata(directory).is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(format!(
                    "--grant {uri}=wasi:filesystem: `{}` is not a directory",
                    directory.display()
                ));
            }
        }
        for package in packages {
            let uri = &package.manifest.service_uri;
            if let Some(permission) = package.manifest.wasi_permissions.iter().find(|permission| {
                !self
                    .grants
                    .iter()
                    .any(|(granted, grant)| granted == uri && grant.covers(permission))
            }) {
                return Err(format!(
                    "service package `{}` declares {}, which the run does not grant\n  = help: grant it with --grant {uri}={}",
                    package.label(),
                    permission.describe(),
                    grant_hint(permission)
                ));
            }
        }
        Ok(())
    }

    /// The run's grants as the host takes them.
    ///
    /// # Errors
    ///
    /// A grant the host refuses (checked like [`Self::check`]).
    #[cfg(feature = "wasmtime")]
    fn host_grants(&self) -> Result<scxml_host_wasmtime::WasiGrants, Box<dyn std::error::Error>> {
        let mut grants = scxml_host_wasmtime::WasiGrants::new();
        for (uri, grant) in &self.grants {
            grants = grants.grant(uri, grant.host())?;
        }
        Ok(grants)
    }

    /// The run's service identity for a `--record`/`--replay` log: each
    /// bound service's URI with the SHA-256 its component is authenticated
    /// under (`--service-component-sha256`, or the digest a repository
    /// service's release pins), sorted.
    ///
    /// # Errors
    ///
    /// A package without its digest, or a repository service whose digest
    /// cannot be pinned.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn identity(
        &self,
        resolved: &Services,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let mut identity = Vec::new();
        for package in &resolved.packages {
            let uri = &package.manifest.service_uri;
            let sha256 = if let ServiceOrigin::Repository { .. } = package.origin {
                resolved
                    .store
                    .as_ref()
                    .ok_or("a repository service was resolved without a repository store")?
                    .service_component_sha256(uri)?
            } else {
                self.digests
                    .get(uri)
                    .ok_or("every service package needs --service-component-sha256")?
                    .clone()
            };
            identity.push((uri.clone(), sha256));
        }
        identity.sort();
        Ok(identity)
    }

    /// Reads, authenticates, and binds every package's service component,
    /// with the run's values for the services' `env` slots (references
    /// only) and, when `outbound` is true, the outbound transport each
    /// service's admitted requests go through (none: every request is
    /// refused).
    ///
    /// # Errors
    ///
    /// An unreadable or oversized component, a digest mismatch, invalid
    /// values, or a binding the host refuses.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn components(
        &self,
        resolved: &Services,
        values: &[crate::extensions::HeldValue],
        outbound: bool,
    ) -> Result<scxml_host_wasmtime::ServiceComponents, Box<dyn std::error::Error>> {
        let packages = &resolved.packages;
        let mut services = scxml_host_wasmtime::ServiceComponents::new()
            .with_values(crate::extensions::HeldValue::environment(values)?);
        if outbound {
            services = services.with_outbound_http(std::sync::Arc::new(
                scxml_host_wasmtime::ExtensionHttpClient::new(
                    scxml_host_wasmtime::ExtensionHttpConfig::new(),
                )
                .map_err(|error| format!("[extension-http-invalid] {error}"))?,
            ));
        }
        for package in packages {
            let uri = &package.manifest.service_uri;
            if let ServiceOrigin::Repository { .. } = package.origin {
                // The release's component, authenticated by the SHA-256 its
                // `SHA256SUMS` lists (pinned on first use).
                let store = resolved
                    .store
                    .as_ref()
                    .ok_or("a repository service was resolved without a repository store")?;
                let fetched = store.resolve_service_component(uri)?;
                let component = scxml_host_wasmtime::authenticate_service_component(
                    &fetched.bytes,
                    scxml_host_wasmtime::TrustedComponentDigest::from_hex(&fetched.sha256)?,
                )
                .map_err(|error| {
                    format!("repository service `{uri}`: its component is rejected: {error}")
                })?;
                services = services.bind(fetched.package.host_package(), &component)?;
                continue;
            }
            let path = match self.components.get(uri) {
                Some(path) => path.clone(),
                None => package
                    .component_path()
                    .map_err(|error| format!("service package `{}`: {error}", package.label()))?,
            };
            let digest = self
                .digests
                .get(uri)
                .ok_or("every service package needs --service-component-sha256")?;
            let limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)?;
            let bytes = read_bounded(&path, limit).map_err(|error| {
                format!(
                    "cannot read the service component `{}`: {error}",
                    path.display()
                )
            })?;
            let component = scxml_host_wasmtime::authenticate_service_component(
                &bytes,
                scxml_host_wasmtime::TrustedComponentDigest::from_hex(digest)?,
            )
            .map_err(|error| {
                format!(
                    "the service component `{}` does not authenticate: {error}",
                    path.display()
                )
            })?;
            services = services.bind(package.host_package(), &component)?;
        }
        Ok(services.with_wasi_grants(self.host_grants()?))
    }
}

fn split_uri(value: &str, usage: &str) -> Result<(String, String), String> {
    match value.rsplit_once('=') {
        Some((uri, rest)) if !uri.is_empty() && !rest.is_empty() => {
            Ok((uri.to_owned(), rest.to_owned()))
        }
        _ => Err(usage.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../components/scxml-service-example/package/echo.service.json")
    }

    #[test]
    fn the_example_package_loads_and_lists_its_consent() {
        let packages = load(&[example()]).expect("the example manifest verifies");
        assert_eq!(
            bound_types(&packages),
            BTreeSet::from(["https://stateforward.org/scxml/services/echo".to_owned()])
        );
        let summary = summary(&packages, None).join("\n");
        assert!(summary.contains("service `https://stateforward.org/scxml/services/echo`"));
        assert!(summary.contains("= outbound HTTP: none"));
        assert!(summary.contains("= env slots: endpoint"));
        assert!(summary.contains("= WASI: none"));
        assert!(
            packages[0]
                .component_path()
                .expect("relative")
                .ends_with("scxml_service_example.virtualized.wasm")
        );
        assert!(
            load(&[example(), example()]).is_err(),
            "one URI bound twice"
        );
    }

    #[test]
    fn component_flags_must_name_bound_services_and_every_package_needs_a_digest() {
        let packages = load(&[example()]).expect("loads");
        let uri = "https://stateforward.org/scxml/services/echo";
        let mut flags = ServiceFlags::default();
        assert!(flags.check(&packages).is_err(), "no digest");
        flags
            .accept(
                "--service-component-sha256",
                &format!("{uri}={}", "a".repeat(64)),
            )
            .expect("digest");
        assert_eq!(flags.check(&packages), Ok(()));
        flags
            .accept("--service-component", "urn:other=x.wasm")
            .expect("component");
        assert!(flags.check(&packages).is_err(), "unbound URI");
        assert!(
            ServiceFlags::default()
                .accept("--service-component-sha256", "urn:x=ABC")
                .is_err()
        );
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;

    /// A temporary package directory: a service manifest declaring events
    /// and the schema file they name.
    fn package(schema: &[u8], declared: &[u8]) -> (PathBuf, PathBuf) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "scxml-cli-service-events-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temporary directory");
        std::fs::write(dir.join("events.json"), schema).expect("schema file");
        let digest = scxml_event_schema::sha256_hex(declared);
        let manifest = format!(
            r#"{{"format":"stateforward.scxml.service-package/v1","service-uri":"urn:example:echo","package-version":"1","service-artifact":"echo.wasm","schema-artifact":null,"schema-sha256":null,"wit-world":"stateforward:scxml/invoked-service","wit-version":"0.2.0","http-origins":[],"env-slots":[],"events":{{"emits":[{{"name":"echo.reply","schema-artifact":"events.json#/$defs/Reply","schema-sha256":"{digest}"}}],"accepts":[{{"name":"echo.say","schema-artifact":"events.json","schema-sha256":"{digest}"}}]}}}}"#
        );
        let path = dir.join("echo.service.json");
        std::fs::write(&path, manifest).expect("manifest");
        (dir, path)
    }

    const SCHEMA: &[u8] = br#"{"$defs":{"Reply":{"type":"string"}}}"#;

    #[test]
    fn declared_event_schemas_are_verified_resolved_and_listed() {
        let (dir, path) = package(SCHEMA, SCHEMA);
        let packages = load(std::slice::from_ref(&path)).expect("verifies");
        let reply = packages[0].events.emitted("echo.reply").expect("declared");
        assert_eq!(reply.schema, r#"{"type":"string"}"#);
        let listing = summary(&packages, None).join("\n");
        assert!(
            listing.contains("= events: emits echo.reply; accepts echo.say"),
            "{listing}"
        );
        std::fs::remove_dir_all(dir).expect("cleanup");

        let (dir, path) = package(br#"{"$defs":{"Reply":{"type":"number"}}}"#, SCHEMA);
        let error = load(&[path]).expect_err("the file does not match its digest");
        assert!(error.to_string().contains("schema-sha256"), "{error}");
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}

#[cfg(test)]
mod grant_tests {
    use super::*;

    const FILES: &str = "https://stateforward.org/scxml/services/files";

    fn files() -> Vec<ServicePackage> {
        load(&[Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../components/scxml-service-files/package/files.service.json")])
        .expect("the files manifest verifies")
    }

    fn flags(grants: &[String]) -> Result<ServiceFlags, String> {
        let mut flags = ServiceFlags::default();
        flags.accept(
            "--service-component-sha256",
            &format!("{FILES}={}", "a".repeat(64)),
        )?;
        for grant in grants {
            flags.accept("--grant", grant)?;
        }
        Ok(flags)
    }

    #[test]
    fn a_declared_permission_must_be_granted_with_an_existing_directory() {
        let packages = files();
        assert!(declares_wasi(&packages));
        let ungranted = flags(&[]).expect("no grant parses");
        let error = ungranted.check(&packages).expect_err("ungranted");
        assert!(
            error.contains("declares wasi:filesystem (mount `workspace`, read-write)"),
            "{error}"
        );
        assert!(
            error.contains(&format!("--grant {FILES}=wasi:filesystem=workspace=DIR")),
            "{error}"
        );
        let listing = summary(&packages, Some(&ungranted)).join("\n");
        assert!(listing.contains("(not granted: add --grant"), "{listing}");

        let directory = std::env::temp_dir();
        let granted = flags(&[format!(
            "{FILES}=wasi:filesystem=workspace={}",
            directory.display()
        )])
        .expect("parses");
        assert_eq!(granted.check(&packages), Ok(()));
        let listing = summary(&packages, Some(&granted)).join("\n");
        assert!(
            listing.contains(&format!(
                "= WASI: wasi:filesystem (mount `workspace`, read-write) <- {}",
                directory.display()
            )),
            "{listing}"
        );

        let missing =
            flags(&[format!("{FILES}=wasi:filesystem=workspace=/no/such/dir")]).expect("parses");
        assert!(
            missing
                .check(&packages)
                .expect_err("no directory")
                .contains("not a directory")
        );
    }

    #[test]
    fn grants_must_name_a_declared_permission_of_a_bound_service() {
        let packages = files();
        let directory = std::env::temp_dir();
        let workspace = format!("{FILES}=wasi:filesystem=workspace={}", directory.display());
        for extra in [
            format!("{FILES}=wasi:clocks"),
            format!("{FILES}=wasi:filesystem=other={}", directory.display()),
            "urn:example:other=wasi:random".to_owned(),
        ] {
            let flags = flags(&[workspace.clone(), extra.clone()]).expect("parses");
            assert!(flags.check(&packages).is_err(), "{extra}");
        }
    }

    #[test]
    fn grant_syntax_is_closed() {
        for bad in [
            "wasi:clocks",
            "=wasi:clocks",
            "urn:x=wasi:http/outgoing-handler",
            "urn:x=wasi:clocks=1",
            "urn:x=wasi:filesystem",
            "urn:x=wasi:filesystem=Bad=/tmp",
            "urn:x=wasi:filesystem=ok=",
            "urn:x=wasi:sockets/tcp",
            "urn:x=wasi:sockets/tcp=localhost:80",
            "urn:x=random",
        ] {
            assert!(
                ServiceFlags::default().accept("--grant", bad).is_err(),
                "{bad}"
            );
        }
        let mut flags = ServiceFlags::default();
        flags
            .accept("--grant", "urn:x=wasi:sockets/tcp=127.0.0.1:80,[::1]:443")
            .expect("tcp addresses");
        assert_eq!(
            flags.grants,
            [(
                "urn:x".to_owned(),
                Grant::Tcp(vec![
                    "127.0.0.1:80".parse().expect("v4"),
                    "[::1]:443".parse().expect("v6")
                ])
            )]
        );
        assert!(
            flags
                .accept("--grant", "urn:x=wasi:sockets/tcp=127.0.0.1:81")
                .is_err(),
            "one permission once per URI"
        );
        let many = vec!["127.0.0.1:1"; 33].join(",");
        assert!(
            ServiceFlags::default()
                .accept("--grant", &format!("urn:y=wasi:sockets/tcp={many}"))
                .is_err()
        );
    }
}
