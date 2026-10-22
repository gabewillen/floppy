//! The schema files a document's `event:schema` attributes reference
//! (`https://github.com/stateforward/scxml.rs/namespaces/event`).
//!
//! `validate`, `compile`, `snapshot run`, and `run` read every referenced
//! file before validating, each resolved against the document's base URI
//! (its `file:` URL, RFC 3986 §5, as a `<script src>` or `<invoke src>` is),
//! and hand the bytes to the validator and compiler keyed by the reference
//! as authored.
//! `snapshot run` and `run` also load the files of every `<invoke src>`
//! document, relative to that child's own base URI, for its compile
//! ([`crate::documents`]): a literal one's before the session, a `srcexpr`
//! one's when its `<invoke>` runs. A child loaded from a `file:` URL has
//! them read against its own `file:` URL as the root's are; one loaded from
//! an `https:` URL has them fetched ([`fetch`]).
//! Only a path the vocabulary accepts is read
//! ([`scxml_event_schema::resource_problem`]): relative, without a scheme,
//! query, or empty segment; its `.` and `..` segments resolve as in any
//! relative URI, so a reference may name a file outside the document's
//! directory, as `<script src="../lib/a.js">` does. A file that is not a
//! regular file or cannot be read is left out, and the validator reports it
//! (`EVENT004`).

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
    let base = document_directory(document);
    let mut resources = Resources::new();
    let mut missing = false;
    for reference in references {
        let bytes = base
            .as_ref()
            .and_then(|base| base.join(&reference).ok())
            .and_then(|url| read_file(&url));
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

/// Fetches every schema file `source` (the document at `document`, an
/// `https:` URL) references, each resolved against that URL (RFC 3986 §5,
/// `..` segments included) and fetched by `fetch`. A file `fetch` does not
/// return is left out, and the validator reports it (`EVENT004`).
pub(crate) fn fetch(
    document: &url::Url,
    source: &[u8],
    mut fetch: impl FnMut(&url::Url) -> Option<Vec<u8>>,
) -> Resources {
    let references = references(source);
    if references.is_empty() {
        return Resources::new();
    }
    let span = observe(Module::Cli, Operation::CliReadEventSchema);
    let mut resources = Resources::new();
    let mut missing = false;
    for reference in references {
        match document.join(&reference).ok().and_then(|url| fetch(&url)) {
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

/// The `file:` URL of the canonical directory of the document at
/// `document`, which its file references resolve against; `None` when it
/// cannot be canonicalized.
fn document_directory(document: &Path) -> Option<url::Url> {
    let directory = document
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let directory = fs::canonicalize(if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &directory
    })
    .ok()?;
    url::Url::from_directory_path(directory).ok()
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

/// The bytes of the file `url` (a `file:` URL, its dot segments already
/// removed) names, or `None` when it names no readable regular file.
fn read_file(url: &url::Url) -> Option<Vec<u8>> {
    let path = url.to_file_path().ok()?;
    let (mut file, metadata) = crate::open_regular_file(&path).ok()??;
    // The native CLI reads a file of any size: resource bounds apply to wasm
    // builds only.
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_resolve_against_the_document_directory() {
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
            ["../outside.json", "order%20one.json"]
        );
        assert_eq!(resources["../outside.json"], b"{}");
        assert_eq!(resources["order%20one.json"], br#"{"type":"object"}"#);
        fs::remove_dir_all(&directory).expect("the fixture directory is removed");
    }

    /// A remote document's references resolve against its own URL, each
    /// fetched once; one the fetcher does not return is left out.
    #[test]
    fn a_remote_document_fetches_its_references_against_its_url() {
        let document = url::Url::parse("https://example.com/kits/machine.scxml").expect("a URL");
        let source = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT_NAMESPACE_URI}" version="1.0"><state id="s"><transition event="a" event:schema="schemas/a.json#/$defs/a"/><transition event="b" event:schema="missing.json"/><transition event="c" event:schema="../outside.json"/></state></scxml>"#
        );
        let mut asked = Vec::new();
        let resources = fetch(&document, source.as_bytes(), |url| {
            asked.push(url.to_string());
            (url.path() == "/kits/schemas/a.json").then(|| br#"{"$defs":{"a":{}}}"#.to_vec())
        });
        assert_eq!(
            asked,
            [
                "https://example.com/outside.json",
                "https://example.com/kits/missing.json",
                "https://example.com/kits/schemas/a.json"
            ]
        );
        assert_eq!(
            resources.keys().map(String::as_str).collect::<Vec<_>>(),
            ["schemas/a.json"]
        );
    }

    /// A reference to a FIFO is not a regular file: it is left out (the
    /// validator reports `EVENT004`) without opening it, which would block
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
