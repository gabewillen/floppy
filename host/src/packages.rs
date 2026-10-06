//! Extension and invoked-service packages: read, verified, authenticated,
//! and bound exactly as `scxml-cli run` binds `--extension-package` and
//! `--service-package`.
//!
//! Every manifest is parsed and verified by scxml.rs's `scxml-validator-core`
//! (unknown fields, formats, permissions, and digests fail closed), its schema
//! and declared event schemas are read relative to the manifest (a relative
//! path inside the manifest's directory only), and its component is
//! authenticated by the SHA-256 the config trusts. Nothing is fetched:
//! repository namespaces and services are bound only from local packages the
//! config names.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use scxml_host_wasmtime::{
    AuthenticatedComponent, ExtensionComponents, ExtensionHttp, ServiceComponents,
    TrustedComponentDigest, WasiGrants,
};
use scxml_validator_core::{
    BoundService, DeclaredEvents, ExtensionPackageManifest, PackageEvents, SchemaSource,
    ServicePackageManifest,
};

use crate::config::{Config, PackageRef, UriGrant};
use crate::env_file::Values;
use crate::telemetry::{Operation, observe};
use crate::{FailureKind, HostFailure};

/// Maximum bytes of one package manifest (as `scxml-cli`).
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Maximum bytes of one schema or declared event schema file (as `scxml-cli`).
pub const MAX_SCHEMA_BYTES: u64 = 4 * 1024 * 1024;

/// One verified extension package and its authenticated component.
#[derive(Clone, Debug)]
pub struct Extension {
    /// The manifest path.
    pub path: PathBuf,
    /// The verified manifest.
    pub manifest: ExtensionPackageManifest,
    /// Its verified schema.
    pub schema: SchemaSource,
    /// Its declared events, every schema verified and resolved.
    pub events: DeclaredEvents,
    component: AuthenticatedComponent,
}

/// One verified service package and its authenticated component.
#[derive(Clone, Debug)]
pub struct Service {
    /// The manifest path.
    pub path: PathBuf,
    /// The verified manifest.
    pub manifest: ServicePackageManifest,
    /// Its declared events, every schema verified and resolved.
    pub events: DeclaredEvents,
    component: AuthenticatedComponent,
}

/// Every package one config binds, with its grants.
#[derive(Clone, Debug, Default)]
pub struct Packages {
    /// Extension packages, in config order.
    pub extensions: Vec<Extension>,
    /// Service packages, in config order.
    pub services: Vec<Service>,
    grants: Vec<UriGrant>,
}

fn package_error(message: impl Into<String>) -> HostFailure {
    HostFailure::new(FailureKind::Package, message)
}

fn grant_error(message: impl Into<String>) -> HostFailure {
    HostFailure::new(FailureKind::Grant, message)
}

/// `artifact` (a relative path inside the manifest's directory) joined to
/// the manifest's directory, every segment percent-decoded.
fn relative_to_manifest(manifest: &Path, artifact: &str, field: &str) -> Result<PathBuf, String> {
    let mut relative = PathBuf::new();
    for segment in artifact.split('/') {
        relative.push(crate::compile::percent_decode(segment).ok_or_else(|| {
            format!("field `{field}` must be a relative path inside the manifest's directory")
        })?);
    }
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
        .map_or_else(|| relative.clone(), |directory| directory.join(&relative)))
}

fn load_events(
    manifest: &Path,
    events: &PackageEvents,
    label: &str,
) -> Result<DeclaredEvents, String> {
    events.check(label)?;
    let mut files = BTreeMap::new();
    for file in events.schema_files() {
        let path = relative_to_manifest(manifest, file, "events")?;
        let bytes = crate::read_bounded(&path, MAX_SCHEMA_BYTES)
            .map_err(|error| format!("{label}: cannot read an event schema {error}"))?;
        files.insert(file.to_owned(), bytes);
    }
    events.resolve(label, &files)
}

fn component(
    reference: &PackageRef,
    artifact: &str,
    authenticate: fn(
        &[u8],
        TrustedComponentDigest,
    ) -> Result<AuthenticatedComponent, scxml_host_wasmtime::HostError>,
) -> Result<AuthenticatedComponent, HostFailure> {
    let shown = reference.manifest.display();
    let path = match &reference.component {
        Some(path) => path.clone(),
        None => relative_to_manifest(&reference.manifest, artifact, "artifact")
            .map_err(|error| package_error(format!("package `{shown}`: {error}")))?,
    };
    let limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)
        .map_err(|_| package_error("the component byte bound does not fit u64"))?;
    let bytes = crate::read_bounded(&path, limit).map_err(|error| {
        package_error(format!(
            "package `{shown}`: cannot read its component {error}"
        ))
    })?;
    let digest =
        TrustedComponentDigest::from_hex(&reference.component_sha256).map_err(|error| {
            package_error(format!(
                "package `{shown}`: invalid component digest: {error}"
            ))
        })?;
    authenticate(&bytes, digest).map_err(|error| {
        package_error(format!(
            "package `{shown}`: the component `{}` does not authenticate: {error}",
            path.display()
        ))
    })
}

impl Extension {
    fn load(reference: &PackageRef) -> Result<Self, HostFailure> {
        let path = &reference.manifest;
        let shown = path.display();
        let bytes = crate::read_bounded(path, MAX_MANIFEST_BYTES)
            .map_err(|error| package_error(format!("cannot read an extension package {error}")))?;
        let manifest = ExtensionPackageManifest::from_json(&bytes).map_err(|error| {
            package_error(format!("extension package `{shown}` is malformed: {error}"))
        })?;
        let schema_path = relative_to_manifest(path, &manifest.schema_artifact, "schema-artifact")
            .map_err(|error| package_error(format!("extension package `{shown}`: {error}")))?;
        let schema_bytes =
            crate::read_bounded(&schema_path, MAX_SCHEMA_BYTES).map_err(|error| {
                package_error(format!(
                    "extension package `{shown}`: cannot read its schema {error}"
                ))
            })?;
        let schema = SchemaSource::new(manifest.schema_artifact.clone(), schema_bytes);
        manifest.verify_schema(&schema).map_err(|error| {
            package_error(format!(
                "extension package `{shown}` does not verify: {error}"
            ))
        })?;
        let events = load_events(path, &manifest.events, "extension package").map_err(|error| {
            package_error(format!(
                "extension package `{shown}` does not verify: {error}"
            ))
        })?;
        let component = component(
            reference,
            &manifest.extension_artifact,
            scxml_host_wasmtime::authenticate_extension_component,
        )?;
        Ok(Self {
            path: path.clone(),
            manifest,
            schema,
            events,
            component,
        })
    }

    fn host_package(&self) -> scxml_host_wasmtime::ExtensionPackage {
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

impl Service {
    fn load(reference: &PackageRef) -> Result<Self, HostFailure> {
        let path = &reference.manifest;
        let shown = path.display();
        let bytes = crate::read_bounded(path, MAX_MANIFEST_BYTES)
            .map_err(|error| package_error(format!("cannot read a service package {error}")))?;
        let manifest = ServicePackageManifest::from_json(&bytes).map_err(|error| {
            package_error(format!("service package `{shown}` is malformed: {error}"))
        })?;
        let schema = match &manifest.schema_artifact {
            Some(artifact) => {
                let schema_path =
                    relative_to_manifest(path, artifact, "schema-artifact").map_err(|error| {
                        package_error(format!("service package `{shown}`: {error}"))
                    })?;
                Some(
                    crate::read_bounded(&schema_path, MAX_SCHEMA_BYTES).map_err(|error| {
                        package_error(format!(
                            "service package `{shown}`: cannot read its schema {error}"
                        ))
                    })?,
                )
            }
            None => None,
        };
        manifest.verify(schema.as_deref()).map_err(|error| {
            package_error(format!(
                "service package `{shown}` does not verify: {error}"
            ))
        })?;
        let events = load_events(path, &manifest.events, "service package").map_err(|error| {
            package_error(format!(
                "service package `{shown}` does not verify: {error}"
            ))
        })?;
        let component = component(
            reference,
            &manifest.service_artifact,
            scxml_host_wasmtime::authenticate_service_component,
        )?;
        Ok(Self {
            path: path.clone(),
            manifest,
            events,
            component,
        })
    }

    fn host_package(&self) -> scxml_host_wasmtime::ServicePackage {
        let manifest = &self.manifest;
        let schemas = |events: &[scxml_validator_core::DeclaredEvent]| {
            events
                .iter()
                .map(|event| scxml_host_wasmtime::ServiceEventSchema {
                    name: event.name.clone(),
                    schema: event.schema.clone(),
                })
                .collect()
        };
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
                emits: schemas(&self.events.emits),
                accepts: schemas(&self.events.accepts),
            },
        }
    }
}

impl Packages {
    /// Reads, verifies, and authenticates every package `config` names, and
    /// checks its grants against what they declare.
    ///
    /// # Errors
    ///
    /// [`FailureKind::Package`] for a package that cannot be read, verified,
    /// or authenticated, or a URI bound twice; [`FailureKind::Grant`] for a
    /// grant that names no bound service or no permission it declares, a
    /// declared permission the config does not grant, or a grant to an
    /// extension package (the scxml.rs session host links no WASI interface
    /// into extension components yet, so such a package fails closed).
    pub fn load(config: &Config) -> Result<Self, HostFailure> {
        let span = observe(Operation::BindPackages);
        let result = Self::load_inner(config);
        span.finish(&result);
        result
    }

    fn load_inner(config: &Config) -> Result<Self, HostFailure> {
        let extensions = config
            .extensions
            .iter()
            .map(Extension::load)
            .collect::<Result<Vec<_>, _>>()?;
        let services = config
            .services
            .iter()
            .map(Service::load)
            .collect::<Result<Vec<_>, _>>()?;
        let mut uris = BTreeSet::new();
        for uri in extensions
            .iter()
            .map(|extension| &extension.manifest.namespace_uri)
            .chain(services.iter().map(|service| &service.manifest.service_uri))
        {
            if !uris.insert(uri.as_str()) {
                return Err(package_error(format!("`{uri}` is bound by two packages")));
            }
        }
        let packages = Self {
            extensions,
            services,
            grants: config.grants.clone(),
        };
        packages.check_grants()?;
        Ok(packages)
    }

    fn check_grants(&self) -> Result<(), HostFailure> {
        for UriGrant { uri, grant } in &self.grants {
            if self
                .extensions
                .iter()
                .any(|extension| &extension.manifest.namespace_uri == uri)
            {
                return Err(grant_error(format!(
                    "a grant names the extension package `{uri}`: the scxml.rs session host links no WASI interface into extension components yet"
                )));
            }
            let Some(service) = self
                .services
                .iter()
                .find(|service| &service.manifest.service_uri == uri)
            else {
                return Err(grant_error(format!(
                    "a grant names `{uri}`, which no bound package declares"
                )));
            };
            if !service
                .manifest
                .wasi_permissions
                .iter()
                .any(|permission| covers(grant, permission))
            {
                return Err(grant_error(format!(
                    "a grant gives `{uri}` {}, which its package does not declare",
                    grant.interface().name()
                )));
            }
        }
        for extension in &self.extensions {
            if let Some(permission) = extension.manifest.wasi_permissions.first() {
                return Err(grant_error(format!(
                    "extension package `{}` declares {}: the scxml.rs session host links no WASI interface into extension components yet",
                    extension.manifest.namespace_uri,
                    permission.describe()
                )));
            }
        }
        for service in &self.services {
            let uri = &service.manifest.service_uri;
            if let Some(permission) = service.manifest.wasi_permissions.iter().find(|permission| {
                !self
                    .grants
                    .iter()
                    .any(|granted| &granted.uri == uri && covers(&granted.grant, permission))
            }) {
                return Err(grant_error(format!(
                    "service package `{uri}` declares {}, which the config does not grant",
                    permission.describe()
                )));
            }
        }
        Ok(())
    }

    /// Whether any bound package declares a WASI permission (such a run is
    /// neither recorded nor replayed: its effects are outside the replay log).
    #[must_use]
    pub fn declares_wasi(&self) -> bool {
        self.extensions
            .iter()
            .any(|extension| !extension.manifest.wasi_permissions.is_empty())
            || self
                .services
                .iter()
                .any(|service| !service.manifest.wasi_permissions.is_empty())
    }

    /// What binding consents to, one line per package and declaration: its
    /// URI, version, declared outbound origins, `env` slots, and WASI
    /// permissions with what the config grants them, and the variables the
    /// run grants (names only). An embedder shows it before running.
    #[must_use]
    pub fn consent(&self, values: &Values) -> String {
        // `writeln!` into a `String` cannot fail (`fmt::Write for String`
        // always returns `Ok`), so its results below carry nothing to handle.
        let mut text = String::new();
        let list = |items: &[String]| {
            if items.is_empty() {
                "none".to_owned()
            } else {
                items.join(", ")
            }
        };
        for extension in &self.extensions {
            let manifest = &extension.manifest;
            let _ = writeln!(
                text,
                "extension {} {}\n  http-origins: {}\n  env-slots: {}",
                manifest.namespace_uri,
                manifest.package_version,
                list(&manifest.http_origins),
                list(&manifest.env_slots)
            );
        }
        for service in &self.services {
            let manifest = &service.manifest;
            let _ = writeln!(
                text,
                "service {} {}\n  http-origins: {}\n  env-slots: {}",
                manifest.service_uri,
                manifest.package_version,
                list(&manifest.http_origins),
                list(&manifest.env_slots)
            );
            for permission in &manifest.wasi_permissions {
                let granted: Vec<String> = self
                    .grants
                    .iter()
                    .filter(|granted| {
                        granted.uri == manifest.service_uri && covers(&granted.grant, permission)
                    })
                    .map(|granted| describe_grant(&granted.grant))
                    .collect();
                let _ = writeln!(
                    text,
                    "  wasi: {} granted {}",
                    permission.describe(),
                    list(&granted)
                );
            }
        }
        let names: Vec<String> = values.names().map(str::to_owned).collect();
        let _ = writeln!(text, "env granted: {}", list(&names));
        text
    }

    /// The compiler's package inputs: every extension manifest and its schema.
    #[must_use]
    pub fn compile_inputs(
        &self,
    ) -> (
        Vec<ExtensionPackageManifest>,
        Vec<scxml_compiler_core::NamespaceSchemaBinding>,
    ) {
        self.extensions
            .iter()
            .map(|extension| {
                (
                    extension.manifest.clone(),
                    scxml_compiler_core::NamespaceSchemaBinding {
                        namespace_uri: extension.manifest.namespace_uri.clone(),
                        schema: extension.schema.clone(),
                    },
                )
            })
            .unzip()
    }

    /// What the validator knows of each bound service, by exact type URI.
    #[must_use]
    pub fn bound_services(&self) -> BTreeMap<String, BoundService> {
        self.services
            .iter()
            .map(|service| {
                (
                    service.manifest.service_uri.clone(),
                    BoundService {
                        env_slots: service.manifest.env_slots.clone(),
                        events: service.events.clone(),
                    },
                )
            })
            .collect()
    }

    /// The `env` slots each bound extension namespace declares.
    #[must_use]
    pub fn extension_slots(&self) -> BTreeMap<String, Vec<String>> {
        self.extensions
            .iter()
            .map(|extension| {
                (
                    extension.manifest.namespace_uri.clone(),
                    extension.manifest.env_slots.clone(),
                )
            })
            .collect()
    }

    /// The extension components as the session host binds them.
    ///
    /// # Errors
    ///
    /// [`FailureKind::Package`] for a binding the host refuses;
    /// [`FailureKind::EnvFile`] for values it refuses.
    pub fn extension_components(
        &self,
        values: &Values,
        http: Option<&Arc<dyn ExtensionHttp>>,
    ) -> Result<ExtensionComponents, HostFailure> {
        let mut components = ExtensionComponents::new().with_values(values.environment()?);
        if let Some(http) = http {
            components = components.with_outbound_http(Arc::clone(http));
        }
        for extension in &self.extensions {
            components = components
                .bind(extension.host_package(), &extension.component)
                .map_err(|error| {
                    package_error(format!(
                        "the session host refuses extension package `{}`: {error}",
                        extension.path.display()
                    ))
                })?;
        }
        Ok(components)
    }

    /// The service components as the session host binds them, with the
    /// config's WASI grants.
    ///
    /// # Errors
    ///
    /// [`FailureKind::Package`] for a binding the host refuses,
    /// [`FailureKind::Grant`] for a grant it refuses, and
    /// [`FailureKind::EnvFile`] for values it refuses.
    pub fn service_components(
        &self,
        values: &Values,
        http: Option<&Arc<dyn ExtensionHttp>>,
    ) -> Result<ServiceComponents, HostFailure> {
        let mut grants = WasiGrants::new();
        for UriGrant { uri, grant } in &self.grants {
            grants = grants.grant(uri, grant.host()).map_err(|error| {
                grant_error(format!(
                    "the session host refuses a grant to `{uri}`: {error}"
                ))
            })?;
        }
        let mut components = ServiceComponents::new()
            .with_values(values.environment()?)
            .with_wasi_grants(grants);
        if let Some(http) = http {
            components = components.with_outbound_http(Arc::clone(http));
        }
        for service in &self.services {
            components = components
                .bind(service.host_package(), &service.component)
                .map_err(|error| {
                    package_error(format!(
                        "the session host refuses service package `{}`: {error}",
                        service.path.display()
                    ))
                })?;
        }
        Ok(components)
    }
}

/// Whether `grant` grants `permission` (same interface, and the same mount
/// for a filesystem).
fn covers(grant: &crate::config::Grant, permission: &scxml_types::WasiPermission) -> bool {
    match (grant, permission) {
        (
            crate::config::Grant::Filesystem { mount, .. },
            scxml_types::WasiPermission::Filesystem {
                mount: declared, ..
            },
        ) => mount == declared,
        _ => grant.interface() == permission.interface(),
    }
}

fn describe_grant(grant: &crate::config::Grant) -> String {
    match grant {
        crate::config::Grant::Filesystem { directory, .. } => {
            format!("directory `{}`", directory.display())
        }
        crate::config::Grant::SocketsTcp { connect } => format!(
            "connect {}",
            connect
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.interface().name().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_stay_inside_the_manifest_directory() {
        let manifest = Path::new("/p/pkg/x.service.json");
        assert_eq!(
            relative_to_manifest(manifest, "a%20b/c.wasm", "f").expect("relative"),
            Path::new("/p/pkg/a b/c.wasm")
        );
        for bad in ["", "../c.wasm", "/abs.wasm", "a/%2e%2e/c", "a%2Fb"] {
            assert!(relative_to_manifest(manifest, bad, "f").is_err(), "{bad}");
        }
    }

    #[test]
    fn an_empty_config_binds_nothing_and_consents_to_nothing() {
        let packages = Packages::default();
        assert!(!packages.declares_wasi());
        assert_eq!(packages.consent(&Values::none()), "env granted: none\n");
        assert!(packages.bound_services().is_empty());
    }
}
