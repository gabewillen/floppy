//! Event I/O Processor package flags (SCXML 6.2 `<send type>`, Appendix C).
//!
//! * `--processor-package MANIFEST.json` (repeatable; `run` only) binds
//!   one Event I/O Processor: a `stateforward.scxml.processor-package/v1`
//!   manifest naming the exact processor type URI, its short aliases, the
//!   package version, the processor artifact, the WIT world and version,
//!   and the outbound origins the processor component may reach over HTTP
//!   or WebSocket (`origins`: binding the package consents to them). A
//!   `<send type>` naming the URI or one of its aliases goes to the bound
//!   component; one no package binds raises `error.execution` when it runs.
//! * Each bound processor's component is resolved relative to the manifest
//!   (`processor-artifact`) or by `--processor-component URI=PATH`, and
//!   authenticated by the SHA-256 in `--processor-component-sha256 URI=HEX`
//!   (required for every package; the manifest is not a trust anchor).
//! * `--websocket-proxy http://HOST:PORT` tunnels every WebSocket connection
//!   a processor opens through that proxy with `CONNECT`; the environment is
//!   never read for one.
//! * `run` lists, per bound processor, its type URI, aliases, version, and
//!   declared outbound origins on stderr: what binding it consents to. The
//!   run's log header records each binding, so a replay must bind the same.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use scxml_telemetry::{ErrorCategory, Module, Operation, observe};
use scxml_validator_core::ProcessorPackageManifest;

use crate::extensions::read_bounded;

/// Byte bound of one manifest file.
const MAX_MANIFEST_BYTES: u64 = crate::repository::MAX_MANIFEST_BYTES;

/// One verified `--processor-package`.
#[derive(Clone, Debug)]
pub(crate) struct ProcessorPackage {
    /// The manifest file.
    pub(crate) path: PathBuf,
    /// The verified manifest.
    pub(crate) manifest: ProcessorPackageManifest,
}

impl ProcessorPackage {
    /// The processor artifact's default path: `processor-artifact` relative
    /// to the manifest's directory.
    #[cfg(any(feature = "wasmtime", test))]
    fn component_path(&self) -> Result<PathBuf, String> {
        let relative = Path::new(&self.manifest.processor_artifact);
        if self.manifest.processor_artifact.is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(
                "field `processor-artifact` must be a relative path inside the manifest's directory"
                    .into(),
            );
        }
        Ok(self
            .path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(relative))
    }

    /// The package as the session host binds it.
    #[cfg(feature = "wasmtime")]
    fn host_package(&self) -> scxml_host_wasmtime::ProcessorPackage {
        let manifest = &self.manifest;
        scxml_host_wasmtime::ProcessorPackage {
            processor_uri: manifest.processor_uri.clone(),
            aliases: manifest.aliases.clone(),
            package_version: manifest.package_version.clone(),
            processor_artifact: manifest.processor_artifact.clone(),
            wit_world: manifest.wit_world.clone(),
            wit_version: manifest.wit_version.clone(),
            origins: manifest.origins.clone(),
        }
    }
}

/// Reads and verifies every `--processor-package` manifest.
///
/// # Errors
///
/// A message naming the manifest for an unreadable, oversized, malformed, or
/// unverifiable manifest, or a type another package already answers.
pub(crate) fn load(
    manifests: &[PathBuf],
) -> Result<Vec<ProcessorPackage>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadProcessorPackage);
    let result = (|| {
        let mut packages: Vec<ProcessorPackage> = Vec::with_capacity(manifests.len());
        for path in manifests {
            let shown = path.display();
            let bytes = read_bounded(path, MAX_MANIFEST_BYTES)
                .map_err(|error| format!("cannot read processor package `{shown}`: {error}"))?;
            let manifest = ProcessorPackageManifest::from_json(&bytes)
                .map_err(|error| format!("processor package `{shown}` is malformed: {error}"))?;
            manifest
                .verify()
                .map_err(|error| format!("processor package `{shown}` does not verify: {error}"))?;
            let names = std::iter::once(&manifest.processor_uri).chain(manifest.aliases.iter());
            if let Some(name) = names
                .clone()
                .find(|name| packages.iter().any(|bound| bound.manifest.answers(name)))
            {
                return Err(format!(
                    "processor package `{shown}` binds `{name}`, which another --processor-package already binds"
                )
                .into());
            }
            packages.push(ProcessorPackage {
                path: path.clone(),
                manifest,
            });
        }
        Ok(packages)
    })();
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    result
}

/// What binding each processor consents to: its type URI, aliases,
/// version, and declared outbound origins.
pub(crate) fn summary(packages: &[ProcessorPackage]) -> Vec<String> {
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    };
    packages
        .iter()
        .map(|package| {
            let manifest = &package.manifest;
            format!(
                "processor `{}`\n  = aliases: {}\n  = version: package {} from --processor-package {}\n  = outbound HTTP and WebSocket: {}",
                manifest.processor_uri,
                list(&manifest.aliases),
                manifest.package_version,
                package.path.display(),
                list(&manifest.origins),
            )
        })
        .collect()
}

/// `snapshot run` and `run` flags for executing Event I/O Processors.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProcessorFlags {
    /// `--processor-package` manifests.
    pub(crate) packages: Vec<PathBuf>,
    components: BTreeMap<String, PathBuf>,
    digests: BTreeMap<String, String>,
    /// `--websocket-proxy`.
    websocket_proxy: Option<String>,
}

impl ProcessorFlags {
    /// The flags this module owns, each taking one value.
    pub(crate) const FLAGS: [&'static str; 4] = [
        "--processor-package",
        "--processor-component",
        "--processor-component-sha256",
        "--websocket-proxy",
    ];

    /// Records one flag.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn accept(&mut self, flag: &str, value: &str) -> Result<(), String> {
        match flag {
            "--processor-package" => {
                if value.is_empty() {
                    return Err("--processor-package requires MANIFEST.json".into());
                }
                self.packages.push(PathBuf::from(value));
                Ok(())
            }
            "--processor-component" => {
                let (uri, path) = split_uri(value, "--processor-component requires URI=PATH")?;
                if self.components.insert(uri, PathBuf::from(path)).is_some() {
                    return Err("--processor-component was given twice for one URI".into());
                }
                Ok(())
            }
            "--processor-component-sha256" => {
                let (uri, digest) =
                    split_uri(value, "--processor-component-sha256 requires URI=HEX")?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(
                        "--processor-component-sha256 requires 64 lowercase hex characters".into(),
                    );
                }
                if self.digests.insert(uri, digest).is_some() {
                    return Err("--processor-component-sha256 was given twice for one URI".into());
                }
                Ok(())
            }
            "--websocket-proxy" => {
                if value.is_empty() {
                    return Err("--websocket-proxy requires http://HOST:PORT".into());
                }
                if self.websocket_proxy.replace(value.to_owned()).is_some() {
                    return Err("--websocket-proxy was given twice".into());
                }
                Ok(())
            }
            other => Err(format!("unknown option `{other}`")),
        }
    }

    /// Checks that every component flag names a `--processor-package` URI
    /// and that every package has exactly one digest.
    ///
    /// # Errors
    ///
    /// A usage message.
    pub(crate) fn check(&self, packages: &[ProcessorPackage]) -> Result<(), String> {
        if let Some(uri) = self
            .components
            .keys()
            .chain(self.digests.keys())
            .find(|uri| {
                !packages
                    .iter()
                    .any(|package| &package.manifest.processor_uri == *uri)
            })
        {
            return Err(format!(
                "`{uri}` is not the processor URI of any --processor-package\n  = help: --processor-component and --processor-component-sha256 name a bound processor URI exactly"
            ));
        }
        if let Some(package) = packages
            .iter()
            .find(|package| !self.digests.contains_key(&package.manifest.processor_uri))
        {
            return Err(format!(
                "processor package `{}` needs --processor-component-sha256 {}=HEX\n  = help: the SHA-256 of the processor component is the trust anchor the run executes it under",
                package.path.display(),
                package.manifest.processor_uri
            ));
        }
        Ok(())
    }

    /// The run's processor identity for a `--record`/`--replay` log: each
    /// bound processor's URI with the SHA-256 its component is
    /// authenticated under, sorted.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn identity(&self) -> Vec<(String, String)> {
        self.digests
            .iter()
            .map(|(uri, digest)| (uri.clone(), digest.clone()))
            .collect()
    }

    /// The WebSocket transport of the run's processor instances and
    /// invoked services: through `--websocket-proxy` when one is given.
    ///
    /// # Errors
    ///
    /// An invalid `--websocket-proxy`, or a trust store that cannot be read.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn websocket_client(
        &self,
    ) -> Result<
        std::sync::Arc<dyn scxml_host_wasmtime::WebSocketTransport>,
        Box<dyn std::error::Error>,
    > {
        Ok(std::sync::Arc::new(
            scxml_host_wasmtime::WebSocketClient::new(
                &scxml_host_wasmtime::WebSocketClientConfig {
                    proxy: self.websocket_proxy.clone(),
                    ..scxml_host_wasmtime::WebSocketClientConfig::default()
                },
            )?,
        ))
    }

    /// Reads, authenticates, and binds every package's processor component,
    /// and, when `websocket` is given, the outbound HTTP transport and that
    /// WebSocket transport its admitted requests and connections go through
    /// (none: every one is refused; a replay answers them from its log
    /// instead).
    ///
    /// # Errors
    ///
    /// An unreadable or oversized component, a digest mismatch, or a binding
    /// the host refuses.
    #[cfg(feature = "wasmtime")]
    pub(crate) fn components(
        &self,
        packages: &[ProcessorPackage],
        websocket: Option<&std::sync::Arc<dyn scxml_host_wasmtime::WebSocketTransport>>,
    ) -> Result<scxml_host_wasmtime::ProcessorComponents, Box<dyn std::error::Error>> {
        let mut processors = scxml_host_wasmtime::ProcessorComponents::new();
        if packages.is_empty() {
            return Ok(processors);
        }
        if let Some(websocket) = websocket {
            processors = processors
                .with_outbound_http(std::sync::Arc::new(
                    scxml_host_wasmtime::ExtensionHttpClient::new(),
                ))
                .with_outbound_websocket(std::sync::Arc::clone(websocket));
        }
        for package in packages {
            let uri = &package.manifest.processor_uri;
            let path = match self.components.get(uri) {
                Some(path) => path.clone(),
                None => package.component_path().map_err(|error| {
                    format!("processor package `{}`: {error}", package.path.display())
                })?,
            };
            let digest = self
                .digests
                .get(uri)
                .ok_or("every processor package needs --processor-component-sha256")?;
            let limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)?;
            let bytes = read_bounded(&path, limit).map_err(|error| {
                format!(
                    "cannot read the processor component `{}`: {error}",
                    path.display()
                )
            })?;
            let component = scxml_host_wasmtime::authenticate_processor_component(
                &bytes,
                scxml_host_wasmtime::TrustedComponentDigest::from_hex(digest)?,
            )
            .map_err(|error| {
                format!(
                    "the processor component `{}` does not authenticate: {error}",
                    path.display()
                )
            })?;
            processors = processors.bind(package.host_package(), &component)?;
        }
        Ok(processors)
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
            .join("../../components/scxml-ioprocessor-websocket/package/websocket.processor.json")
    }

    #[test]
    fn the_websocket_package_loads_and_lists_its_consent() {
        let packages = load(&[example()]).expect("the WebSocket package loads");
        let listed = summary(&packages);
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].contains(
                "processor `https://github.com/stateforward/scxml.rs/processors/websocket`"
            )
        );
        assert!(listed[0].contains("aliases: websocket"));
        assert!(listed[0].contains("outbound HTTP and WebSocket: none"));
        assert!(
            packages[0]
                .component_path()
                .expect("relative artifact")
                .ends_with("scxml_ioprocessor_websocket.virtualized.wasm")
        );
        let twice = load(&[example(), example()]).expect_err("one type bound twice");
        assert!(twice.to_string().contains("already binds"));
    }

    #[test]
    fn component_flags_must_name_bound_processors_and_every_package_needs_a_digest() {
        let packages = load(&[example()]).expect("the WebSocket package loads");
        let uri = &packages[0].manifest.processor_uri;
        let mut flags = ProcessorFlags::default();
        assert!(flags.check(&packages).is_err());
        flags
            .accept(
                "--processor-component-sha256",
                &format!("{uri}={}", "a".repeat(64)),
            )
            .expect("digest");
        flags.check(&packages).expect("bound and authenticated");
        flags
            .accept("--processor-component", "https://example.com/other=x.wasm")
            .expect("syntax");
        assert!(flags.check(&packages).is_err());
    }

    #[test]
    fn flag_syntax_is_closed() {
        for (flag, bad) in [
            ("--processor-package", ""),
            ("--processor-component", "no-equals"),
            ("--processor-component-sha256", "uri=ABC"),
            ("--websocket-proxy", ""),
            ("--processor-unknown", "x"),
        ] {
            assert!(
                ProcessorFlags::default().accept(flag, bad).is_err(),
                "{flag} {bad}"
            );
        }
        let mut flags = ProcessorFlags::default();
        flags
            .accept("--websocket-proxy", "http://127.0.0.1:8080")
            .expect("proxy");
        assert!(
            flags
                .accept("--websocket-proxy", "http://127.0.0.1:8080")
                .is_err()
        );
    }
}
