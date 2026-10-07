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
//!   package binds (`SCXML094`); invoking it is a runtime action, so the
//!   document still compiles and its `<invoke>` raises `error.execution` when
//!   it runs.
//! * `snapshot run` and `run` execute each bound service's component,
//!   resolved relative to the manifest (`service-artifact`) or by
//!   `--service-component URI=PATH`, and authenticated by the SHA-256 in
//!   `--service-component-sha256 URI=HEX` (required for every package; the
//!   manifest is not a trust anchor).
//! * The WASI interfaces a service may use are its component's own imports.
//!   Only runtime resources need a grant, with wasmtime CLI's option names
//!   scoped to the service's exact type URI (up to the first `=`):
//!   `--dir URI=HOST::GUEST` preopens a host directory at its absolute guest
//!   path for a component that imports `wasi:filesystem`;
//!   `--wasi URI=inherit-network` lets a component that imports
//!   `wasi:sockets/tcp` connect to any address (wasmtime's
//!   `-S inherit-network`); and `--wasi URI=allow-ip-name-lookup` allows
//!   `wasi:sockets/ip-name-lookup`.
//!   An import whose resource is not granted, a grant whose interface the
//!   component does not import, and a `wasi:*` import the host never links
//!   (`wasi:cli`, `wasi:http`) fail closed when the service is bound, before
//!   anything runs.
//! * Every command that reads a document lists, per bound service, its type
//!   URI, version, declared outbound origins, declared slots, and the
//!   resources the run grants it; `snapshot run` and `run` also list the
//!   WASI interfaces its component imports: what binding it consents to.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

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
        }
    }
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
    Ok(ServicePackage {
        origin: ServiceOrigin::Manifest(path.to_path_buf()),
        manifest,
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
/// service (`https://github.com/<owner>/<repo>/services/<name>[@<tag>]`) the
/// document or one of its `children` (the `<invoke src>` documents a run
/// loads) names as a literal `<invoke type>` that none of them binds, once,
/// through the repository flags ([`crate::repository`]: bounded, pinned,
/// content-addressed, `--offline`, fail closed).
///
/// # Errors
///
/// As [`load`], an unopenable extension cache, or a repository service that
/// cannot be resolved.
pub(crate) fn resolve(
    source: &[u8],
    children: &[Vec<u8>],
    manifests: &[PathBuf],
    repository: &crate::repository::RepositoryFlags,
) -> Result<Services, Box<dyn std::error::Error>> {
    let mut packages = load(manifests)?;
    let bound: Vec<String> = packages
        .iter()
        .map(|package| package.manifest.service_uri.clone())
        .collect();
    let mut wanted = crate::repository::used_services(source, &bound);
    for child in children {
        wanted.extend(crate::repository::used_services(child, &bound));
    }
    wanted.sort();
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(Services {
            packages,
            store: None,
        });
    }
    let store = repository.open()?;
    for uri in wanted {
        packages.push(store.resolve_service_package(&uri)?);
    }
    Ok(Services {
        packages,
        store: Some(store),
    })
}

/// The `env` slots each bound service's package declares, by exact type
/// URI (for `SCXML094` and `SCXML083` on `<invoke>`).
pub(crate) fn bound_services(packages: &[ServicePackage]) -> BTreeMap<String, Vec<String>> {
    packages
        .iter()
        .map(|package| {
            (
                package.manifest.service_uri.clone(),
                package.manifest.env_slots.clone(),
            )
        })
        .collect()
}

/// What binding each service consents to: its type URI, version, declared
/// outbound origins, and declared slots; the WASI resources `flags` (the
/// run's, when it runs the services) grant it; and, when `imports` holds
/// them (the components were read), the WASI interfaces its component
/// imports. Never an `env` value.
pub(crate) fn summary(
    packages: &[ServicePackage],
    flags: Option<&ServiceFlags>,
    imports: Option<&BTreeMap<String, Vec<String>>>,
) -> Vec<String> {
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
            let uri = &manifest.service_uri;
            let wasi = imports
                .and_then(|imports| imports.get(uri))
                .map(|names| {
                    let wasi: Vec<String> = names
                        .iter()
                        .filter(|name| name.starts_with("wasi:"))
                        .cloned()
                        .collect();
                    format!("\n  = WASI imports: {}", list(&wasi))
                })
                .unwrap_or_default();
            let granted = flags
                .and_then(|flags| flags.wasi.get(uri))
                .map_or_else(|| "none".to_owned(), WasiFlags::describe);
            format!(
                "service `{uri}`\n  = version: {version}\n  = outbound HTTP: {}\n  = env slots: {}{wasi}\n  = WASI resources: {granted}",
                list(&manifest.http_origins),
                list(&manifest.env_slots),
            )
        })
        .collect()
}

/// Each bound service component's imports, by service URI.
#[cfg(feature = "wasmtime")]
type Imports = BTreeMap<String, Vec<String>>;

/// Maximum `--dir` and `--wasi` flags of one run.
const MAX_WASI_FLAGS: usize = 64;

/// The WASI runtime resources a run grants one service (`--dir` and
/// `--wasi`, wasmtime CLI's option names).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WasiFlags {
    /// `--dir URI=HOST::GUEST`: each host directory and its guest path.
    dirs: Vec<(PathBuf, String)>,
    /// `--wasi URI=inherit-network`.
    inherit_network: bool,
    /// `--wasi URI=allow-ip-name-lookup`.
    allow_ip_name_lookup: bool,
}

impl WasiFlags {
    /// What it grants, for the consent listing.
    fn describe(&self) -> String {
        let mut parts: Vec<String> = self
            .dirs
            .iter()
            .map(|(host, guest)| format!("--dir {}::{guest}", host.display()))
            .collect();
        if self.inherit_network {
            parts.push("inherit-network".to_owned());
        }
        if self.allow_ip_name_lookup {
            parts.push("allow-ip-name-lookup".to_owned());
        }
        if parts.is_empty() {
            "none".to_owned()
        } else {
            parts.join(", ")
        }
    }

    /// The resources as the session host takes them.
    #[cfg(feature = "wasmtime")]
    fn host(&self) -> scxml_host_wasmtime::WasiOptions {
        scxml_host_wasmtime::WasiOptions {
            dirs: self.dirs.clone(),
            inherit_network: self.inherit_network,
            allow_ip_name_lookup: self.allow_ip_name_lookup,
        }
    }
}

/// `value` split at its first `=` into a non-empty URI and the rest.
fn split_first(value: &str, usage: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((uri, rest)) if !uri.is_empty() && !rest.is_empty() => {
            Ok((uri.to_owned(), rest.to_owned()))
        }
        _ => Err(usage.to_owned()),
    }
}

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
    /// `--dir` and `--wasi` flags, by service URI.
    wasi: BTreeMap<String, WasiFlags>,
    /// `--dir` and `--wasi` flags given so far.
    wasi_flags: usize,
}

impl ServiceFlags {
    /// The flags this module owns, each taking one value.
    pub(crate) const FLAGS: [&'static str; 5] = [
        "--service-package",
        "--service-component",
        "--service-component-sha256",
        "--dir",
        "--wasi",
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
            "--dir" => {
                const USAGE: &str = "--dir requires URI=HOST::GUEST (wasmtime's --dir HOST::GUEST, scoped to a service URI), GUEST an absolute path";
                let (uri, dir) = split_first(value, USAGE)?;
                let (host, guest) = dir.rsplit_once("::").ok_or(USAGE)?;
                if host.is_empty() || !guest.starts_with('/') {
                    return Err(USAGE.into());
                }
                self.count_wasi()?;
                let flags = self.wasi.entry(uri).or_default();
                if flags.dirs.iter().any(|(_, seen)| seen == guest) {
                    return Err("--dir names one guest path twice for one URI".into());
                }
                flags.dirs.push((PathBuf::from(host), guest.to_owned()));
                Ok(())
            }
            "--wasi" => {
                const USAGE: &str = "--wasi requires URI=inherit-network or URI=allow-ip-name-lookup (wasmtime's -S options, scoped to a service URI)";
                let (uri, option) = split_first(value, USAGE)?;
                self.count_wasi()?;
                let flags = self.wasi.entry(uri).or_default();
                let granted = match option.as_str() {
                    "inherit-network" => &mut flags.inherit_network,
                    "allow-ip-name-lookup" => &mut flags.allow_ip_name_lookup,
                    _ => return Err(USAGE.into()),
                };
                if std::mem::replace(granted, true) {
                    return Err(format!("--wasi {option} was given twice for one URI"));
                }
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
        for (uri, flags) in &self.wasi {
            if !packages
                .iter()
                .any(|package| &package.manifest.service_uri == uri)
            {
                return Err(format!(
                    "`{uri}` is not the service URI of any bound service\n  = help: --dir and --wasi grant WASI resources to a bound service URI exactly"
                ));
            }
            if let Some((host, _)) = flags
                .dirs
                .iter()
                .find(|(host, _)| !std::fs::metadata(host).is_ok_and(|metadata| metadata.is_dir()))
            {
                return Err(format!("--dir {uri}={}: not a directory", host.display()));
            }
        }
        Ok(())
    }

    /// Counts one more `--dir` or `--wasi` flag.
    fn count_wasi(&mut self) -> Result<(), String> {
        if self.wasi_flags >= MAX_WASI_FLAGS {
            return Err(format!(
                "--dir and --wasi are limited to {MAX_WASI_FLAGS} flags"
            ));
        }
        self.wasi_flags = self.wasi_flags.saturating_add(1);
        Ok(())
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
    /// only), the WASI resources `--dir` and `--wasi` grant, and, when
    /// `outbound` is true, the outbound transport each service's admitted
    /// requests go through (none: every request is refused; a replay
    /// answers them from its log instead). Returns the bound components and
    /// each component's imports, by service URI, for the consent listing.
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
    ) -> Result<(scxml_host_wasmtime::ServiceComponents, Imports), Box<dyn std::error::Error>> {
        let packages = &resolved.packages;
        let mut imports = BTreeMap::new();
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
        for (uri, flags) in &self.wasi {
            services = services.with_wasi(uri, flags.host());
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
                imports.insert(uri.clone(), component.imports()?);
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
            imports.insert(uri.clone(), component.imports()?);
            services = services.bind(package.host_package(), &component)?;
        }
        Ok((services, imports))
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
            bound_services(&packages),
            BTreeMap::from([(
                "https://stateforward.org/scxml/services/echo".to_owned(),
                vec!["endpoint".to_owned()]
            )])
        );
        let summary = summary(&packages, None, None).join("\n");
        assert!(summary.contains("service `https://stateforward.org/scxml/services/echo`"));
        assert!(summary.contains("= outbound HTTP: none"));
        assert!(summary.contains("= env slots: endpoint"));
        assert!(summary.contains("= WASI resources: none"));
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
mod wasi_tests {
    use super::*;

    const FILES: &str = "https://stateforward.org/scxml/services/files";

    fn files() -> Vec<ServicePackage> {
        load(&[Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../components/scxml-service-files/package/files.service.json")])
        .expect("the files manifest verifies")
    }

    fn flags(grants: &[(&str, String)]) -> Result<ServiceFlags, String> {
        let mut flags = ServiceFlags::default();
        flags.accept(
            "--service-component-sha256",
            &format!("{FILES}={}", "a".repeat(64)),
        )?;
        for (flag, value) in grants {
            flags.accept(flag, value)?;
        }
        Ok(flags)
    }

    #[test]
    fn dir_and_wasi_flags_grant_resources_to_bound_services_and_are_listed() {
        let packages = files();
        let directory = std::env::temp_dir();
        let granted = flags(&[(
            "--dir",
            format!("{FILES}={}::/workspace", directory.display()),
        )])
        .expect("parses");
        assert_eq!(granted.check(&packages), Ok(()));
        let imports = BTreeMap::from([(
            FILES.to_owned(),
            vec![
                "stateforward:scxml/types@0.2.0".to_owned(),
                "wasi:filesystem/types@0.2.9".to_owned(),
            ],
        )]);
        let listing = summary(&packages, Some(&granted), Some(&imports)).join("\n");
        assert!(
            listing.contains("= WASI imports: wasi:filesystem/types@0.2.9"),
            "{listing}"
        );
        assert!(
            listing.contains(&format!(
                "= WASI resources: --dir {}::/workspace",
                directory.display()
            )),
            "{listing}"
        );
        let missing =
            flags(&[("--dir", format!("{FILES}=/no/such/dir::/workspace"))]).expect("parses");
        assert!(
            missing
                .check(&packages)
                .expect_err("no directory")
                .contains("not a directory")
        );
        let unbound = flags(&[(
            "--wasi",
            "urn:example:other=allow-ip-name-lookup".to_owned(),
        )])
        .expect("parses");
        assert!(unbound.check(&packages).is_err());
    }

    #[test]
    fn dir_and_wasi_syntax_is_closed() {
        for (flag, bad) in [
            ("--dir", "/tmp::/workspace"),
            ("--dir", "urn:x=/tmp"),
            ("--dir", "urn:x=/tmp::workspace"),
            ("--dir", "urn:x=::/workspace"),
            ("--wasi", "urn:x"),
            ("--wasi", "urn:x=tcp"),
            ("--wasi", "urn:x=tcp=127.0.0.1:80"),
            ("--wasi", "urn:x=inherit-network=y"),
            ("--wasi", "urn:x=allow-ip-name-lookup=y"),
        ] {
            assert!(
                ServiceFlags::default().accept(flag, bad).is_err(),
                "{flag} {bad}"
            );
        }
        let mut flags = ServiceFlags::default();
        flags
            .accept("--wasi", "urn:x=inherit-network")
            .expect("inherit-network");
        assert!(
            flags
                .wasi
                .get("urn:x")
                .is_some_and(|flags| flags.inherit_network)
        );
        assert!(
            flags.accept("--wasi", "urn:x=inherit-network").is_err(),
            "once per URI"
        );
        flags
            .accept("--dir", "urn:x=/tmp::/a")
            .expect("one directory");
        assert!(
            flags.accept("--dir", "urn:x=/var::/a").is_err(),
            "one guest path once per URI"
        );
    }
}
