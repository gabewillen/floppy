//! The schema files a document's `event:schema` attributes reference
//! (`https://github.com/stateforward/scxml.rs/namespaces/event`).
//!
//! `validate`, `compile`, `snapshot run`, and `run` read every referenced
//! file relative to the document's directory before validating, and hand the
//! bytes to the validator and compiler keyed by the reference as authored.
//! Only a path the vocabulary accepts is read
//! ([`scxml_event_schema::resource_problem`]): relative, without a scheme,
//! query, or `.`/`..` segment, percent-decoded. A file that resolves outside
//! the document's directory (through a symbolic link), is not a regular file,
//! or cannot be read is left out, and the validator reports it (`SCXML088`).
//! Nothing is fetched, and nothing is read while a document runs.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use scxml_event_schema::{
    EVENT_NAMESPACE_URI, Resources, SCHEMA_ATTRIBUTE, Source, resource_problem,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};

/// Reads every schema file `source` (the document at `document`)
/// references. A document that does not parse references none (the
/// validator reports it).
pub(crate) fn load(document: &Path, source: &[u8]) -> Resources {
    let references = references(source);
    if references.is_empty() {
        return Resources::new();
    }
    let span = observe(Module::Cli, Operation::CliReadEventSchema);
    let root = document_directory(document);
    let mut resources = Resources::new();
    let mut missing = false;
    for reference in references {
        let bytes = root.as_ref().and_then(|root| read_below(root, &reference));
        match bytes {
            Some(bytes) => {
                resources.insert(reference, bytes);
            }
            None => missing = true,
        }
    }
    if missing {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::InputRead);
    } else {
        span.event(SignalEvent::ExternalReadCompleted);
    }
    resources
}

/// The canonical directory of the document at `document`, which its file
/// references resolve against; `None` when it cannot be canonicalized.
fn document_directory(document: &Path) -> Option<PathBuf> {
    let directory = document
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    fs::canonicalize(if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &directory
    })
    .ok()
}

/// The distinct resource parts of the document's `event:schema` references
/// that name a file, sorted.
fn references(source: &[u8]) -> Vec<String> {
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
    let mut references = Vec::new();
    document.root.visit(&mut |element| {
        for attribute in &element.attributes {
            if attribute.name.namespace_uri.as_deref() != Some(EVENT_NAMESPACE_URI)
                || attribute.name.local_name != SCHEMA_ATTRIBUTE
            {
                continue;
            }
            if let Source::Reference { resource, .. } = scxml_event_schema::source(&attribute.value)
                && resource_problem(resource).is_none()
            {
                references.push(resource.to_owned());
            }
        }
    });
    references.sort();
    references.dedup();
    references
}

/// The bytes of `reference` (a checked relative URI path) below `root`, or
/// `None` when it does not resolve to a regular file at or below `root`.
fn read_below(root: &Path, reference: &str) -> Option<Vec<u8>> {
    let mut path = root.to_path_buf();
    for segment in reference.split('/') {
        path.push(percent_decode(segment)?);
    }
    let resolved = fs::canonicalize(&path).ok()?;
    if !resolved.starts_with(root) {
        return None;
    }
    // Opening a FIFO or a device can block, so only a regular file is
    // opened; the handle is checked again in case the path changed between.
    if !fs::metadata(&resolved).ok()?.is_file() {
        return None;
    }
    let mut file = fs::File::open(&resolved).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    // The native CLI reads a file of any size: resource bounds apply to wasm
    // builds only.
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// One percent-decoded path segment; `None` when the encoding is invalid,
/// not UTF-8, or decodes to a separator or a `.`/`..` segment.
fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let hex = segment.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    (!decoded.is_empty()
        && decoded != "."
        && decoded != ".."
        && !decoded.contains(['/', '\\', '\0']))
    .then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_name_files_below_the_document() {
        let directory =
            std::env::temp_dir().join(format!("scx-event-schemas-{}", std::process::id()));
        let schemas = directory.join("schemas");
        fs::create_dir_all(&schemas).expect("the fixture directory is created");
        fs::write(schemas.join("order one.json"), br#"{"type":"object"}"#)
            .expect("the schema is written");
        fs::write(directory.join("outside.json"), b"{}").expect("the sibling is written");
        let document = schemas.join("machine.scxml");
        let source = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT_NAMESPACE_URI}" version="1.0"><state id="s"><transition event="a" event:schema="order%20one.json#/x"/><transition event="b" event:schema="../outside.json"/><transition event="c" event:schema="missing.json"/><transition event="d" event:schema='{{"type":"string"}}'/></state></scxml>"#
        );
        let resources = load(&document, source.as_bytes());
        assert_eq!(
            resources.keys().map(String::as_str).collect::<Vec<_>>(),
            ["order%20one.json"]
        );
        assert_eq!(resources["order%20one.json"], br#"{"type":"object"}"#);
        assert_eq!(
            percent_decode("a%2Fb"),
            None,
            "an encoded separator is no segment"
        );
        assert_eq!(percent_decode("%2e%2E"), None);
        fs::remove_dir_all(&directory).expect("the fixture directory is removed");
    }

    /// A reference to a FIFO is not a regular file: it is left out (the
    /// validator reports `SCXML088`) without opening it, which would block
    /// until a writer appears.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_never_opened() {
        let directory =
            std::env::temp_dir().join(format!("scx-event-schemas-fifo-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("the fixture directory is created");
        let status = std::process::Command::new("mkfifo")
            .arg(directory.join("pipe.json"))
            .status()
            .expect("mkfifo runs");
        assert!(status.success(), "the FIFO fixture is created");
        let document = directory.join("machine.scxml");
        let source = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT_NAMESPACE_URI}" version="1.0"><state id="s"><transition event="a" event:schema="pipe.json"/></state></scxml>"#
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(load(&document, source.as_bytes()));
        });
        let resources = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reading the references does not block on a FIFO");
        assert!(resources.is_empty());
        fs::remove_dir_all(&directory).expect("the fixture directory is removed");
    }
}
