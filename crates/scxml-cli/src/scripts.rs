//! `compile` flags for external `<script src>` resolution (SCXML 1.0
//! section 5.8).
//!
//! * `--script URI=PATH` supplies one script package: the exact `src` URI
//!   (split at the last `=`, so URIs may contain `=`) and the file holding
//!   its bytes. The media type follows the file extension: `.wasm` is
//!   `application/wasm`; `.js` and `.mjs` are `text/javascript`; any other
//!   extension is rejected.
//! * `--script-sha256 URI=HEX` pins the expected SHA-256; every `--script`
//!   needs exactly one, and the compiler rejects bytes that do not match.
//! * `--script-resolution compile|runtime` selects compile-time pinning
//!   (the default) or leaving every `src` to the host's runtime loader.
//!
//! Files are read with a regular-file check and the script component byte
//! bound before compilation.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use scxml_compiler_core::{ScriptPackage, ScriptResolution};
use scxml_telemetry::{ErrorCategory, Module, Operation, observe};

/// Collected script flags for one `compile` command.
#[derive(Debug, Default)]
pub(crate) struct ScriptFlags {
    files: Vec<(String, PathBuf)>,
    digests: BTreeMap<String, String>,
    resolution: Option<ScriptResolution>,
}

impl ScriptFlags {
    /// Whether `flag` is one of the script flags.
    pub(crate) fn is_script_flag(flag: &str) -> bool {
        matches!(flag, "--script" | "--script-sha256" | "--script-resolution")
    }

    /// Records one script flag and its required value.
    pub(crate) fn accept(&mut self, flag: &str, value: Option<&String>) -> Result<(), String> {
        match flag {
            "--script" => split_last(value, "--script requires URI=PATH")
                .map(|(uri, path)| self.files.push((uri, PathBuf::from(path)))),
            "--script-sha256" => {
                split_last(value, "--script-sha256 requires URI=HEX").and_then(|(uri, digest)| {
                    if self.digests.insert(uri, digest).is_some() {
                        Err("--script-sha256 was given twice for one URI".into())
                    } else {
                        Ok(())
                    }
                })
            }
            _ => match value.map(String::as_str) {
                Some("compile") => {
                    self.resolution = Some(ScriptResolution::Compile);
                    Ok(())
                }
                Some("runtime") => {
                    self.resolution = Some(ScriptResolution::Runtime);
                    Ok(())
                }
                _ => Err("--script-resolution requires compile|runtime".into()),
            },
        }
    }

    /// Whether any script flag was supplied.
    pub(crate) fn is_set(&self) -> bool {
        !self.files.is_empty() || !self.digests.is_empty() || self.resolution.is_some()
    }

    /// Selected resolution; compile-time pinning by default.
    pub(crate) fn resolution(&self) -> ScriptResolution {
        self.resolution.unwrap_or_default()
    }

    /// Reads every script package; each `--script` needs exactly one
    /// `--script-sha256` for its URI and vice versa.
    pub(crate) fn load(&self) -> Result<Vec<ScriptPackage>, Box<dyn std::error::Error>> {
        if self.files.len() != self.digests.len() {
            return Err("every --script needs exactly one --script-sha256 for its URI".into());
        }
        self.files
            .iter()
            .map(|(uri, path)| {
                let sha256 = self
                    .digests
                    .get(uri)
                    .ok_or("every --script needs exactly one --script-sha256 for its URI")?;
                Ok(ScriptPackage {
                    uri: uri.clone(),
                    media_type: media_type(path)?.to_owned(),
                    sha256: sha256.clone(),
                    bytes: read_script(path)?,
                })
            })
            .collect()
    }
}

fn split_last(value: Option<&String>, message: &str) -> Result<(String, String), String> {
    value
        .and_then(|value| value.rsplit_once('='))
        .filter(|(left, right)| !left.is_empty() && !right.is_empty())
        .map(|(left, right)| (left.to_owned(), right.to_owned()))
        .ok_or_else(|| message.to_owned())
}

fn media_type(path: &Path) -> Result<&'static str, String> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("wasm") => Ok(scxml_script::WASM_MEDIA_TYPE),
        Some("js" | "mjs") => Ok("text/javascript"),
        _ => Err("--script PATH must end in .wasm, .js, or .mjs to declare its media type".into()),
    }
}

fn read_script(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let span = observe(Module::Cli, Operation::CliReadScript);
    let result = (|| {
        let limit = u64::try_from(scxml_script::MAX_SCRIPT_COMPONENT_BYTES)?;
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > limit {
            return Err("script must be a regular file within the 4 MiB byte bound".into());
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len())? > limit {
            return Err("script exceeds the 4 MiB byte bound".into());
        }
        Ok(bytes)
    })();
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_flags_pair_uris_digests_and_resolution() {
        let mut flags = ScriptFlags::default();
        assert!(!flags.is_set());
        flags
            .accept("--script", Some(&"a.wasm?x=1=lib/a.wasm".to_owned()))
            .expect("URI=PATH splits at the last `=`");
        assert_eq!(
            flags.files,
            [("a.wasm?x=1".to_owned(), PathBuf::from("lib/a.wasm"))]
        );
        assert!(
            flags
                .accept("--script", Some(&"no-path".to_owned()))
                .is_err()
        );
        flags
            .accept(
                "--script-sha256",
                Some(&format!("a.wasm?x=1={}", "0".repeat(64))),
            )
            .expect("URI=HEX parses");
        assert!(
            flags
                .accept(
                    "--script-sha256",
                    Some(&format!("a.wasm?x=1={}", "1".repeat(64)))
                )
                .is_err(),
            "a URI is pinned once"
        );
        assert_eq!(flags.resolution(), ScriptResolution::Compile);
        flags
            .accept("--script-resolution", Some(&"runtime".to_owned()))
            .expect("runtime parses");
        assert_eq!(flags.resolution(), ScriptResolution::Runtime);
        assert!(flags.accept("--script-resolution", None).is_err());
        assert!(flags.is_set());
        assert!(media_type(Path::new("x.txt")).is_err());
        assert_eq!(media_type(Path::new("x.mjs")), Ok("text/javascript"));
    }
}
