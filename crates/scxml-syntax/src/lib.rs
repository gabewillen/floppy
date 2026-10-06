//! Bounded, span-preserving XML source syntax for SCXML documents.
//!
//! This crate is the only owner of XML parsing (`quick-xml`). It resolves
//! namespaces to exact expanded names, keeps exact UTF-8 byte spans for every
//! node, and preserves extension-namespace content verbatim. It processes the
//! general entities of the internal DTD subset as XML 1.0 §5.1 requires of a
//! non-validating processor, within fixed bounds, never reads an external
//! entity, and fails closed on the DTD constructs it does not process
//! (markup in entity replacement text, parameter-entity references,
//! attribute defaults). It performs no SCXML validation or lowering: `scxml-validator-core` and `scxml-compiler-core`
//! own those.
//!
//! # Example
//!
//! Parse bounded, untrusted XML into a namespace-resolved document whose
//! nodes keep exact UTF-8 byte spans into the source:
//!
//! ```
//! use scxml_syntax::{SourceId, SourceText, parse_xml};
//!
//! let source = SourceText::new(
//!     SourceId::new(1),
//!     "machine.scxml",
//!     r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="idle"/>"#,
//! );
//! let document = parse_xml(source)?;
//! assert!(document.root.name.is(Some("http://www.w3.org/2005/07/scxml"), "scxml"));
//! let initial = document.root.attribute("initial").map(|attribute| attribute.value.as_str());
//! assert_eq!(initial, Some("idle"));
//! assert_eq!(document.used_namespaces(), ["http://www.w3.org/2005/07/scxml"]);
//!
//! // Malformed XML fails closed with a typed error rather than a panic.
//! assert!(parse_xml(SourceText::new(SourceId::new(2), "bad.xml", "<a><b></a>")).is_err());
//! # Ok::<(), scxml_syntax::XmlError>(())
//! ```

#![forbid(unsafe_code)]

mod dtd;
pub mod source;
mod whitespace;
pub mod xml;

/// Shared bounded telemetry facade for syntax-dependent validation crates.
pub mod telemetry {
    pub use scxml_telemetry::{
        ErrorCategory, FunctionSpan, Module, Operation, Recorder, SignalEvent, install_recorder,
        observe,
    };
}

pub use source::{
    Attribute, Document, Element, ExpandedName, LineColumn, NamespaceDeclaration, Node, SourceId,
    SourceText, Span, Text,
};
pub use whitespace::{collapse_whitespace, is_xml_blank, is_xml_space, xml_tokens};
pub use xml::{
    XmlError, XmlErrorDetail, XmlErrorKind, XmlLimits, parse_xml, parse_xml_with_limits,
    validate_document_limits,
};
