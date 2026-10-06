//! Embeds the authenticated release session components, and the `scx`
//! namespace's extension component (`namespaces/scx`), in `scx` (the
//! `wasmtime` feature).
//!
//! The release directory is the one `scripts/verify-wasm-capabilities.sh`
//! writes: `SCXML_WASM_ARTIFACTS_DIR` when set, else the script's default,
//! `target/wasm-artifacts/release` under the workspace. Its `manifest.tsv`,
//! the eight session components a session host links, and the `scx`
//! extension component are read, each
//! component's SHA-256 and size are checked against its manifest line, and
//! the verified bytes are copied into `OUT_DIR`, where `src/components.rs`
//! includes them (`include_bytes!`) with the manifest's SHA-256 as the trust
//! anchor. A missing or mismatched artifact fails the build with a message
//! naming it and how to build the release; nothing falls back.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// The session components a session host links, by manifest name.
const SESSION_COMPONENTS: [&str; 8] = [
    "scxml_machine",
    "scxml_compiler",
    "scxml_data_model_ecmascript",
    "scxml_data_model_xpath",
    "scxml_data_model_null",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_clock",
];

/// The extension components `scx` embeds, by manifest name: the `scx`
/// namespace (`namespaces/scx`).
const EXTENSION_COMPONENTS: [&str; 1] = ["scx"];

/// How to produce the release directory.
const HELP: &str = "build the release session components first: `sh scripts/verify-wasm-capabilities.sh` \
(from the scxml.rs workspace; it writes target/wasm-artifacts/release), or point \
SCXML_WASM_ARTIFACTS_DIR at a release directory it wrote. A build without them needs \
`--no-default-features` (no Wasmtime host: run, snapshot run, and conformance run then fail closed)";

fn main() {
    println!("cargo:rerun-if-env-changed=SCXML_WASM_ARTIFACTS_DIR");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_WASMTIME");
    if std::env::var_os("CARGO_FEATURE_WASMTIME").is_none() {
        return;
    }
    if let Err(message) = embed() {
        // A build script reports failure through a nonzero exit; the message
        // is the whole diagnostic.
        eprintln!(
            "error: scx cannot embed the release session components: {message}\n  = help: {HELP}"
        );
        std::process::exit(1);
    }
}

fn embed() -> Result<(), String> {
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR is unset")?);
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?);
    let release = std::env::var_os("SCXML_WASM_ARTIFACTS_DIR").map_or_else(
        || manifest_dir.join("../../target/wasm-artifacts/release"),
        PathBuf::from,
    );
    let manifest_path = release.join("manifest.tsv");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = read(&manifest_path)?;
    let text = std::str::from_utf8(&manifest)
        .map_err(|_| format!("`{}` is not UTF-8", manifest_path.display()))?;
    let embedded = out_dir.join("release");
    fs::create_dir_all(&embedded)
        .map_err(|error| format!("cannot create `{}`: {error}", embedded.display()))?;
    let mut source = String::new();
    let mut artifacts = String::new();
    let mut extension_digest = String::new();
    for name in SESSION_COMPONENTS.into_iter().chain(EXTENSION_COMPONENTS) {
        let mut lines = text.lines().filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            (fields.first() == Some(&"artifact") && fields.get(1) == Some(&name)).then_some(fields)
        });
        let (Some(fields), None) = (lines.next(), lines.next()) else {
            return Err(format!(
                "`{}` does not list `{name}` exactly once",
                manifest_path.display()
            ));
        };
        let [_, _, file, size, digest] = fields[..] else {
            return Err(format!(
                "`{}`: the `{name}` line is not `artifact NAME FILE SIZE SHA256`",
                manifest_path.display()
            ));
        };
        if file != format!("{name}.virtualized.wasm") {
            return Err(format!(
                "`{}`: `{name}` names the unexpected file `{file}`",
                manifest_path.display()
            ));
        }
        let path = release.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = read(&path)?;
        let actual = hex(&Sha256::digest(&bytes));
        if actual != digest || bytes.len().to_string() != size {
            return Err(format!(
                "`{}` does not match its manifest line (SHA-256 {actual}, {} bytes; the manifest says {digest}, {size} bytes)",
                path.display(),
                bytes.len()
            ));
        }
        write(&embedded.join(file), &bytes)?;
        if EXTENSION_COMPONENTS.contains(&name) {
            extension_digest = actual;
            continue;
        }
        writeln!(
            artifacts,
            "    ({file:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/release/{file}\"))),"
        )
        .map_err(|error| error.to_string())?;
    }
    write(&embedded.join("manifest.tsv"), &manifest)?;
    writeln!(
        source,
        "/// The embedded release's `manifest.tsv`.\n\
         pub(crate) const MANIFEST: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/release/manifest.tsv\"));\n\
         /// The SHA-256 of [`MANIFEST`], the embedded release's trust anchor.\n\
         pub(crate) const MANIFEST_SHA256: &str = {:?};\n\
         /// The embedded session components, by artifact file name.\n\
         pub(crate) const ARTIFACTS: &[(&str, &[u8])] = &[\n{artifacts}];\n\
         /// The embedded `scx` namespace's extension component (`namespaces/scx`).\n\
         pub(crate) const SCX_COMPONENT: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/release/scx.virtualized.wasm\"));\n\
         /// The SHA-256 of [`SCX_COMPONENT`], checked against the release manifest.\n\
         pub(crate) const SCX_COMPONENT_SHA256: &str = {extension_digest:?};",
        hex(&Sha256::digest(&manifest))
    )
    .map_err(|error| error.to_string())?;
    write(&out_dir.join("embedded_release.rs"), source.as_bytes())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| format!("cannot write `{}`: {error}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}
