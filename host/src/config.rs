//! The nemobot config file: `stateforward.nemobot.config/v1`.
//!
//! One JSON object, read once and checked before anything is bound:
//!
//! ```json
//! {
//!   "format": "stateforward.nemobot.config/v1",
//!   "components": { "dir": "target/scxml-components", "manifest-sha256": "<hex>" },
//!   "env-file": "../mosfet/mosfet.py/.env",
//!   "env": [
//!     { "name": "OPENAI_API_KEY",
//!       "packages": ["https://github.com/stateforward/nemobot/namespaces/generate@v0.1.0"] },
//!     { "name": "OPENAI_BASE_URL",
//!       "packages": ["https://github.com/stateforward/nemobot/namespaces/generate@v0.1.0"] }
//!   ],
//!   "env-defaults": { "OPENAI_BASE_URL": "https://api.openai.com/v1" },
//!   "extensions": [
//!     { "manifest": "namespaces/generate/package/generate.extension.json",
//!       "component-sha256": "<hex>" }
//!   ],
//!   "services": [
//!     { "manifest": "services/memory/package/memory.service.json",
//!       "component": "target/components/nemobot_memory.virtualized.wasm",
//!       "component-sha256": "<hex>" }
//!   ],
//!   "grants": [
//!     { "uri": "https://github.com/stateforward/nemobot/services/memory@v0.1.0",
//!       "permission": "wasi:filesystem", "mount": "data", "directory": "var/memory" }
//!   ],
//!   "outbound-http": true,
//!   "fuel": 1000000000,
//!   "max-commands": 100000,
//!   "extension-call-fuel": 1000000000
//! }
//! ```
//!
//! * `components`: a `scxml.rs/scripts/verify-wasm-capabilities.sh` artifacts
//!   directory and the trusted SHA-256 of its `manifest.tsv` (required).
//! * `env-file` (optional) and `env`: the run grants exactly the variables
//!   `env` names, read from `env-file` (Docker `--env-file` format), each to
//!   exactly the package URIs (extension namespace URIs or service type URIs)
//!   its `packages` lists. An element or `<invoke>` of any other package that
//!   names the variable gets no reference: its slot is absent, exactly like
//!   an unheld value. `env` without `env-file` is refused, the process
//!   environment is never read for them, and every listed package must be
//!   bound and declare an `env` slot (checked when the packages bind).
//! * `env-defaults` (optional): non-secret values (such as a backend's base
//!   URL) for granted variables the env file does not set; the env file's
//!   value wins. Every name must be one `env` grants, and no key or other
//!   secret may be put here (config files are committed). This is how a
//!   run supplies a default backend while no backend URL is compiled into a
//!   component.
//! * `extensions` and `services`: package manifests, each with its component's
//!   trusted SHA-256; `component` defaults to the manifest's artifact path,
//!   relative to the manifest.
//! * `grants`: the WASI grants, by exact package URI; every permission a bound
//!   package declares must be granted and every grant must name a declared
//!   permission (checked when the packages bind).
//! * `outbound-http` (default `true`): whether admitted extension and service
//!   requests reach the network (each package's declared origins, or an origin
//!   an `env` value supplies entirely); `false` refuses every request.
//! * `fuel` and `max-commands` (optional, non-zero): the store fuel of every
//!   session call and the engine command budget of each call.
//! * `extension-call-fuel` (optional, non-zero; scxml.rs's
//!   `DEFAULT_EXTENSION_CALL_FUEL`, 100 million, without it): the instruction
//!   fuel cap of one extension component call. `<sx:validate>` links the
//!   whole validator and its bundled XSD set, so a session that uses the
//!   `scxml` namespace needs more (namespaces/scxml documents how much).
//!
//! Relative paths resolve against the config file's directory. Unknown fields
//! and unknown permissions are refused.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::env_file::Values;
use crate::telemetry::{Operation, observe};
use crate::{FailureKind, HostFailure};

/// The only config format this release reads.
pub const CONFIG_FORMAT: &str = "stateforward.nemobot.config/v1";
/// Maximum bytes of one config file.
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;
/// Default engine command budget of one session call.
pub const DEFAULT_MAX_COMMANDS: u64 = 100_000;

/// The release session components.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Components {
    /// The artifacts directory (absolute after [`Config::load`]).
    pub dir: PathBuf,
    /// The trusted SHA-256 of its `manifest.tsv` (lowercase hex).
    pub manifest_sha256: String,
}

/// One package to bind: its manifest and its component's trust anchor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageRef {
    /// The manifest path (absolute after [`Config::load`]).
    pub manifest: PathBuf,
    /// The component path; `None` takes the manifest's artifact path.
    pub component: Option<PathBuf>,
    /// The component's trusted SHA-256 (lowercase hex).
    pub component_sha256: String,
}

/// One WASI grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Grant {
    /// `wasi:clocks`.
    Clocks,
    /// `wasi:random`.
    Random,
    /// `wasi:filesystem` for one declared mount.
    Filesystem {
        /// The declared mount name.
        mount: String,
        /// The host directory behind it (absolute after [`Config::load`]).
        directory: PathBuf,
    },
    /// `wasi:sockets/ip-name-lookup`.
    SocketsIpNameLookup,
    /// `wasi:sockets/tcp` to the listed remote addresses.
    SocketsTcp {
        /// The admitted remote addresses.
        connect: Vec<std::net::SocketAddr>,
    },
}

/// One grant to one package URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UriGrant {
    /// The exact package URI (a service type URI or a namespace URI).
    pub uri: String,
    /// The grant.
    pub grant: Grant,
}

/// One granted variable and the exact package URIs it is granted to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvGrant {
    /// The variable name.
    pub name: String,
    /// The exact package URIs (extension namespace or service type URIs)
    /// whose elements or `<invoke>`s may reference it: non-empty, distinct,
    /// in config order.
    pub packages: Vec<String>,
}

/// One checked nemobot config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// The release session components.
    pub components: Components,
    /// The env file the granted variables come from.
    pub env_file: Option<PathBuf>,
    /// The variables the run grants and their packages, in config order,
    /// distinct by name.
    pub env: Vec<EnvGrant>,
    /// Non-secret defaults for granted variables the env file does not set
    /// (`Debug` shows names only, like every [`Values`]).
    pub env_defaults: Values,
    /// Extension packages.
    pub extensions: Vec<PackageRef>,
    /// Invoked-service packages.
    pub services: Vec<PackageRef>,
    /// WASI grants.
    pub grants: Vec<UriGrant>,
    /// Whether admitted extension and service requests reach the network.
    pub outbound_http: bool,
    /// Store fuel of every session call.
    pub fuel: u64,
    /// Engine command budget of each session call.
    pub max_commands: u64,
    /// Instruction fuel cap of one extension component call.
    pub extension_call_fuel: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawConfig {
    format: String,
    components: RawComponents,
    #[serde(default)]
    env_file: Option<String>,
    #[serde(default)]
    env: Vec<RawEnvGrant>,
    #[serde(default)]
    env_defaults: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    extensions: Vec<RawPackage>,
    #[serde(default)]
    services: Vec<RawPackage>,
    #[serde(default)]
    grants: Vec<RawGrant>,
    #[serde(default)]
    outbound_http: Option<bool>,
    #[serde(default)]
    fuel: Option<u64>,
    #[serde(default)]
    max_commands: Option<u64>,
    #[serde(default)]
    extension_call_fuel: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawEnvGrant {
    name: String,
    packages: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawComponents {
    dir: String,
    manifest_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPackage {
    manifest: String,
    #[serde(default)]
    component: Option<String>,
    component_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawGrant {
    uri: String,
    permission: String,
    #[serde(default)]
    mount: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    connect: Option<Vec<String>>,
}

fn config_error(message: impl Into<String>) -> HostFailure {
    HostFailure::new(FailureKind::Config, message)
}

/// An optional non-zero budget field, or its default.
fn positive(value: Option<u64>, field: &str, default: u64) -> Result<u64, HostFailure> {
    match value {
        Some(0) => Err(config_error(format!("field `{field}` must be positive"))),
        Some(value) => Ok(value),
        None => Ok(default),
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn resolve(base: &Path, value: &str, field: &str) -> Result<PathBuf, HostFailure> {
    if value.is_empty() {
        return Err(config_error(format!("field `{field}` is empty")));
    }
    let path = Path::new(value);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}

impl Config {
    /// Reads and checks the config file at `path`.
    ///
    /// # Errors
    ///
    /// [`FailureKind::Config`] for an unreadable or oversized file, invalid
    /// JSON, an unknown field or format, a malformed digest, a duplicate
    /// variable, an invalid grant, or a zero limit; the message names the
    /// field, never a value from the env file.
    pub fn load(path: &Path) -> Result<Self, HostFailure> {
        let span = observe(Operation::LoadConfig);
        let result = Self::load_inner(path);
        span.finish(&result);
        result
    }

    fn load_inner(path: &Path) -> Result<Self, HostFailure> {
        let bytes = crate::read_bounded(path, MAX_CONFIG_BYTES)
            .map_err(|error| config_error(format!("cannot read the config file {error}")))?;
        let absolute = std::path::absolute(path).map_err(|error| {
            config_error(format!("cannot resolve `{}`: {error}", path.display()))
        })?;
        let base = absolute
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        Self::parse(&bytes, &base)
    }

    /// Parses config bytes, resolving relative paths against `base`.
    ///
    /// # Errors
    ///
    /// As [`Self::load`], without the read.
    pub fn parse(bytes: &[u8], base: &Path) -> Result<Self, HostFailure> {
        let raw: RawConfig = serde_json::from_slice(bytes)
            .map_err(|error| config_error(format!("the config is invalid: {error}")))?;
        if raw.format != CONFIG_FORMAT {
            return Err(config_error(format!(
                "field `format` must be `{CONFIG_FORMAT}`"
            )));
        }
        if !is_sha256_hex(&raw.components.manifest_sha256) {
            return Err(config_error(
                "field `components.manifest-sha256` must be 64 lowercase hex digits",
            ));
        }
        let components = Components {
            dir: resolve(base, &raw.components.dir, "components.dir")?,
            manifest_sha256: raw.components.manifest_sha256,
        };
        let env_file = raw
            .env_file
            .as_deref()
            .map(|value| resolve(base, value, "env-file"))
            .transpose()?;
        let (env, env_defaults) = Self::env(raw.env, raw.env_defaults, env_file.is_some())?;
        let packages = |list: Vec<RawPackage>, field: &str| {
            list.into_iter()
                .enumerate()
                .map(|(index, package)| {
                    if !is_sha256_hex(&package.component_sha256) {
                        return Err(config_error(format!(
                            "field `{field}[{index}].component-sha256` must be 64 lowercase hex digits"
                        )));
                    }
                    Ok(PackageRef {
                        manifest: resolve(
                            base,
                            &package.manifest,
                            &format!("{field}[{index}].manifest"),
                        )?,
                        component: package
                            .component
                            .as_deref()
                            .map(|value| {
                                resolve(base, value, &format!("{field}[{index}].component"))
                            })
                            .transpose()?,
                        component_sha256: package.component_sha256,
                    })
                })
                .collect::<Result<Vec<_>, HostFailure>>()
        };
        let extensions = packages(raw.extensions, "extensions")?;
        let services = packages(raw.services, "services")?;
        let grants = raw
            .grants
            .into_iter()
            .enumerate()
            .map(|(index, grant)| Self::grant(base, index, grant))
            .collect::<Result<Vec<_>, _>>()?;
        let fuel = positive(raw.fuel, "fuel", scxml_host_wasmtime::DEFAULT_FUEL)?;
        let max_commands = positive(raw.max_commands, "max-commands", DEFAULT_MAX_COMMANDS)?;
        let extension_call_fuel = positive(
            raw.extension_call_fuel,
            "extension-call-fuel",
            scxml_host_wasmtime::DEFAULT_EXTENSION_CALL_FUEL,
        )?;
        Ok(Self {
            components,
            env_file,
            env,
            env_defaults,
            extensions,
            services,
            grants,
            outbound_http: raw.outbound_http.unwrap_or(true),
            fuel,
            max_commands,
            extension_call_fuel,
        })
    }

    /// The granted variables (`env`) and their non-secret defaults
    /// (`env-defaults`), checked.
    fn env(
        raw_env: Vec<RawEnvGrant>,
        raw_defaults: std::collections::BTreeMap<String, String>,
        has_env_file: bool,
    ) -> Result<(Vec<EnvGrant>, Values), HostFailure> {
        let mut env: Vec<EnvGrant> = Vec::with_capacity(raw_env.len());
        for (index, RawEnvGrant { name, packages }) in raw_env.into_iter().enumerate() {
            if !crate::env_file::is_name(&name) {
                return Err(config_error(format!(
                    "field `env[{index}].name` is not a variable name ([A-Za-z_][A-Za-z0-9_]*)"
                )));
            }
            if env.iter().any(|granted| granted.name == name) {
                return Err(config_error(format!("field `env` grants `{name}` twice")));
            }
            if packages.is_empty() {
                return Err(config_error(format!(
                    "field `env[{index}].packages` must list at least one package URI"
                )));
            }
            for (position, uri) in packages.iter().enumerate() {
                if uri.is_empty() {
                    return Err(config_error(format!(
                        "field `env[{index}].packages` lists an empty URI"
                    )));
                }
                if packages.iter().take(position).any(|seen| seen == uri) {
                    return Err(config_error(format!(
                        "field `env[{index}].packages` lists `{uri}` twice"
                    )));
                }
            }
            env.push(EnvGrant { name, packages });
        }
        if !env.is_empty() && !has_env_file {
            return Err(config_error(
                "field `env` needs `env-file`: granted variables come only from the env file",
            ));
        }
        let mut defaults = Vec::with_capacity(raw_defaults.len());
        for (name, value) in raw_defaults {
            if !env.iter().any(|granted| granted.name == name) {
                return Err(config_error(format!(
                    "field `env-defaults` names `{name}`, which `env` does not grant"
                )));
            }
            if value.is_empty()
                || value.len() > crate::env_file::MAX_VALUE_BYTES
                || value.contains('\0')
            {
                return Err(config_error(format!(
                    "field `env-defaults.{name}` must be non-empty, at most {} bytes, without NUL",
                    crate::env_file::MAX_VALUE_BYTES
                )));
            }
            defaults.push((name, value));
        }
        Ok((env, Values::from_entries(defaults)))
    }

    fn grant(base: &Path, index: usize, raw: RawGrant) -> Result<UriGrant, HostFailure> {
        let field = format!("grants[{index}]");
        let no_extra = |mount: bool, directory: bool, connect: bool| {
            if (raw.mount.is_some() && !mount)
                || (raw.directory.is_some() && !directory)
                || (raw.connect.is_some() && !connect)
            {
                Err(config_error(format!(
                    "field `{field}` has a field its permission `{}` does not take",
                    raw.permission
                )))
            } else {
                Ok(())
            }
        };
        let grant = match raw.permission.as_str() {
            "wasi:clocks" => {
                no_extra(false, false, false)?;
                Grant::Clocks
            }
            "wasi:random" => {
                no_extra(false, false, false)?;
                Grant::Random
            }
            "wasi:sockets/ip-name-lookup" => {
                no_extra(false, false, false)?;
                Grant::SocketsIpNameLookup
            }
            "wasi:filesystem" => {
                no_extra(true, true, false)?;
                let (Some(mount), Some(directory)) = (&raw.mount, &raw.directory) else {
                    return Err(config_error(format!(
                        "field `{field}`: `wasi:filesystem` needs `mount` and `directory`"
                    )));
                };
                if !scxml_types::is_wasi_mount(mount) {
                    return Err(config_error(format!(
                        "field `{field}.mount` is not a mount name ([a-z][a-z0-9-]*)"
                    )));
                }
                Grant::Filesystem {
                    mount: mount.clone(),
                    directory: resolve(base, directory, &format!("{field}.directory"))?,
                }
            }
            "wasi:sockets/tcp" => {
                no_extra(false, false, true)?;
                let addresses = raw.connect.as_deref().unwrap_or_default();
                if addresses.is_empty() {
                    return Err(config_error(format!(
                        "field `{field}`: `wasi:sockets/tcp` needs a non-empty `connect`"
                    )));
                }
                let connect = addresses
                    .iter()
                    .map(|address| {
                        address.parse().map_err(|_| {
                            config_error(format!(
                                "field `{field}.connect` lists an entry that is not IP:PORT"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Grant::SocketsTcp { connect }
            }
            _ => {
                return Err(config_error(format!(
                    "field `{field}.permission` is not a known WASI permission"
                )));
            }
        };
        Ok(UriGrant {
            uri: raw.uri,
            grant,
        })
    }
}

impl Config {
    /// The granted variable names, in config order.
    #[must_use]
    pub fn env_names(&self) -> Vec<String> {
        self.env
            .iter()
            .map(|granted| granted.name.clone())
            .collect()
    }
}

impl Grant {
    /// The grant as the session host takes it.
    #[must_use]
    pub fn host(&self) -> scxml_host_wasmtime::WasiGrant {
        match self {
            Self::Clocks => scxml_host_wasmtime::WasiGrant::Clocks,
            Self::Random => scxml_host_wasmtime::WasiGrant::Random,
            Self::Filesystem { mount, directory } => scxml_host_wasmtime::WasiGrant::Filesystem {
                mount: mount.clone(),
                directory: directory.clone(),
            },
            Self::SocketsIpNameLookup => scxml_host_wasmtime::WasiGrant::SocketsIpNameLookup,
            Self::SocketsTcp { connect } => scxml_host_wasmtime::WasiGrant::SocketsTcp {
                connect: connect.clone(),
            },
        }
    }

    /// The interface it grants.
    #[must_use]
    pub fn interface(&self) -> scxml_types::WasiInterface {
        self.host().interface()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn parse(text: &str) -> Result<Config, HostFailure> {
        Config::parse(text.as_bytes(), Path::new("/base"))
    }

    fn minimal(extra: &str) -> String {
        format!(
            r#"{{"format":"{CONFIG_FORMAT}","components":{{"dir":"c","manifest-sha256":"{DIGEST}"}}{extra}}}"#
        )
    }

    #[test]
    fn a_minimal_config_takes_the_defaults_and_resolves_paths() {
        let config = parse(&minimal("")).expect("a minimal config is valid");
        assert_eq!(config.components.dir, Path::new("/base/c"));
        assert!(config.env.is_empty() && config.env_file.is_none());
        assert!(config.outbound_http, "outbound HTTP defaults on");
        assert_eq!(config.fuel, scxml_host_wasmtime::DEFAULT_FUEL);
        assert_eq!(config.max_commands, DEFAULT_MAX_COMMANDS);
        assert_eq!(
            config.extension_call_fuel,
            scxml_host_wasmtime::DEFAULT_EXTENSION_CALL_FUEL
        );
    }

    #[test]
    fn packages_and_grants_parse() {
        let config = parse(&minimal(&format!(
            r#","env-file":"/x/.env","env":[{{"name":"OPENAI_API_KEY","packages":["urn:a","urn:b"]}}],"services":[{{"manifest":"s.json","component-sha256":"{DIGEST}"}}],"grants":[{{"uri":"u","permission":"wasi:filesystem","mount":"data","directory":"d"}},{{"uri":"u","permission":"wasi:sockets/tcp","connect":["127.0.0.1:80"]}}],"outbound-http":false,"max-commands":7,"extension-call-fuel":9"#
        )))
        .expect("valid");
        assert_eq!(config.env_file.as_deref(), Some(Path::new("/x/.env")));
        assert_eq!(
            config.env,
            [EnvGrant {
                name: "OPENAI_API_KEY".into(),
                packages: vec!["urn:a".into(), "urn:b".into()],
            }]
        );
        assert_eq!(config.env_names(), ["OPENAI_API_KEY"]);
        assert_eq!(config.env_defaults, Values::none());
        assert_eq!(config.services[0].manifest, Path::new("/base/s.json"));
        assert_eq!(config.services[0].component, None);
        assert_eq!(
            config.grants[0].grant,
            Grant::Filesystem {
                mount: "data".into(),
                directory: PathBuf::from("/base/d")
            }
        );
        assert!(!config.outbound_http);
        assert_eq!(config.max_commands, 7);
        assert_eq!(config.extension_call_fuel, 9);
    }

    #[test]
    fn env_defaults_name_granted_variables_and_debug_shows_names_only() {
        let config = parse(&minimal(
            r#","env-file":"e","env":[{"name":"KEY","packages":["urn:a"]},{"name":"BASE","packages":["urn:a"]}],"env-defaults":{"BASE":"https://api.example.invalid/v1"}"#,
        ))
        .expect("valid");
        assert_eq!(config.env_defaults.names().collect::<Vec<_>>(), ["BASE"]);
        assert!(
            !format!("{config:?}").contains("api.example.invalid"),
            "a default's value is not shown"
        );
    }

    #[test]
    fn invalid_configs_are_refused() {
        for (extra, expected) in [
            (r#","unknown":1"#, "unknown field"),
            (
                r#","env":[{"name":"A","packages":["urn:a"]}]"#,
                "needs `env-file`",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":["urn:a"]},{"name":"A","packages":["urn:b"]}]"#,
                "grants `A` twice",
            ),
            (
                r#","env-file":"e","env":[{"name":"1A","packages":["urn:a"]}]"#,
                "not a variable name",
            ),
            (r#","env-file":"e","env":["A"]"#, "the config is invalid"),
            (
                r#","env-file":"e","env":[{"name":"A"}]"#,
                "missing field `packages`",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":["urn:a"],"scope":1}]"#,
                "unknown field",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":[]}]"#,
                "at least one package URI",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":[""]}]"#,
                "empty URI",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":["urn:a","urn:a"]}]"#,
                "lists `urn:a` twice",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":["urn:a"]}],"env-defaults":{"B":"x"}"#,
                "`env` does not grant",
            ),
            (
                r#","env-file":"e","env":[{"name":"A","packages":["urn:a"]}],"env-defaults":{"A":""}"#,
                "must be non-empty",
            ),
            (r#","fuel":0"#, "`fuel` must be positive"),
            (
                r#","extension-call-fuel":0"#,
                "`extension-call-fuel` must be positive",
            ),
            (
                r#","grants":[{"uri":"u","permission":"wasi:http"}]"#,
                "not a known WASI permission",
            ),
            (
                r#","grants":[{"uri":"u","permission":"wasi:clocks","mount":"m"}]"#,
                "does not take",
            ),
            (
                r#","grants":[{"uri":"u","permission":"wasi:sockets/tcp","connect":[]}]"#,
                "non-empty `connect`",
            ),
            (
                r#","services":[{"manifest":"m","component-sha256":"AB"}]"#,
                "64 lowercase hex",
            ),
        ] {
            let error = parse(&minimal(extra)).expect_err(extra);
            assert_eq!(error.kind(), FailureKind::Config);
            assert!(
                error.message().contains(expected),
                "{extra}: {}",
                error.message()
            );
        }
        let error = parse(r#"{"format":"other","components":{"dir":"c","manifest-sha256":"x"}}"#)
            .expect_err("unknown format");
        assert!(error.message().contains("field `format`"));
    }
}
