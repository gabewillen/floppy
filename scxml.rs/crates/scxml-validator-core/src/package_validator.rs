//! Package validators: the optional component an extension or service
//! package ships (manifest `validator-artifact` and `validator-sha256`) to
//! check what its XSD cannot (`stateforward:scxml-validator`
//! `package-validator`; docs/validator-rules.md, "Package validators").
//!
//! After the XSD and built-in checks, the validator builds the document's
//! read-only [`document::Context`] once and calls the linked
//! [`PackageValidator`] once per package that has targets: the elements in
//! an extension namespace, or the `<invoke>` elements whose literal `type`
//! is a service type URI. The package's diagnostics are reported as
//! `<prefix>::<code>` ([`package_prefix`]). Everything a package returns is
//! untrusted: a failure, more than [`MAX_PACKAGE_DIAGNOSTICS`] diagnostics,
//! a code that is not `[a-z][a-z0-9_]*` of at most
//! [`MAX_PACKAGE_CODE_BYTES`] bytes, a span outside the source or off a
//! character boundary, overlapping edits within one fix, or an edit outside
//! every target element discards all of that package's diagnostics for one
//! `PKG002` error naming the package. Pure and bounded; it runs inside the
//! validator's boundary span and records no telemetry of its own.

use scxml_syntax::{Document, Element, Node, Span};
use scxml_types::{ENV_NAMESPACE_URI, EVENT_NAMESPACE_URI};

use crate::{Diagnostic, DiagnosticCategory, EventSchemaResources, Fix, FixEdit, Label};

/// Upper bound on the diagnostics one package validator call returns.
pub const MAX_PACKAGE_DIAGNOSTICS: usize = 256;
/// Upper bound on the bytes of a package diagnostic's code.
pub const MAX_PACKAGE_CODE_BYTES: usize = 64;

const SCXML_NAMESPACE: &str = crate::SCXML_NAMESPACE;
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// The read-only parsed document a package validator reads
/// (`stateforward:scxml-validator/document`).
pub mod document {
    /// An expanded XML name: the exact, tagless namespace URI and the local
    /// name.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct QualifiedName {
        /// Namespace URI; `None` for no namespace.
        pub namespace_uri: Option<String>,
        /// Local name.
        pub local_name: String,
    }

    /// One attribute as parsed.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Attribute {
        /// Expanded name.
        pub name: QualifiedName,
        /// Value after XML attribute-value normalization.
        pub value: String,
        /// The whole attribute.
        pub span: super::types::Span,
    }

    /// One element.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Element {
        /// Expanded name.
        pub name: QualifiedName,
        /// The whole element, start tag to end tag.
        pub span: super::types::Span,
        /// Attributes in source order.
        pub attributes: Vec<Attribute>,
        /// Child element indices in source order.
        pub children: Vec<u32>,
        /// Parent element index; `None` for the root.
        pub parent: Option<u32>,
    }

    /// One SCXML `<transition>`, in document order.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct TransitionInfo {
        /// The source state's `id`; empty when it has none.
        pub state: String,
        /// The `event` attribute.
        pub event: Option<String>,
        /// The resolved canonical JSON of a literal `event:schema` that
        /// resolves; `None` without one, for `event:schemaexpr`, and for a
        /// schema that does not resolve (reported by `SCXML087`-`SCXML092`).
        pub event_schema: Option<String>,
    }

    /// The whole document.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct Context {
        /// Every element in document order; the root is index 0.
        pub elements: Vec<Element>,
        /// The `id` of every SCXML `<data>`, in document order.
        pub data_ids: Vec<String>,
        /// The `id` of every SCXML state, parallel, final, and history, in
        /// document order.
        pub state_ids: Vec<String>,
        /// Every SCXML `<transition>`, in document order.
        pub transitions: Vec<TransitionInfo>,
        /// The root's `datamodel` attribute.
        pub datamodel: Option<String>,
    }
}

/// What a package validator returns (`stateforward:scxml-validator/types`).
pub mod types {
    pub use scxml_reporting::{Applicability, Severity};

    /// A half-open UTF-8 byte range of the document.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Span {
        /// Inclusive start.
        pub start: u32,
        /// Exclusive end.
        pub end: u32,
    }

    /// One labeled range.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Label {
        /// The range.
        pub span: Span,
        /// Optional explanation.
        pub message: Option<String>,
    }

    /// One replacement.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Edit {
        /// The replaced range.
        pub span: Span,
        /// Replacement text.
        pub replacement: String,
    }

    /// One suggested repair.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Fix {
        /// Summary.
        pub message: String,
        /// Confidence.
        pub applicability: Applicability,
        /// Edits.
        pub edits: Vec<Edit>,
    }

    /// One diagnostic; `code` is the package's own, reported as
    /// `<prefix>::<code>`.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Diagnostic {
        /// The package's own code.
        pub code: String,
        /// Severity.
        pub severity: Severity,
        /// Summary.
        pub message: String,
        /// Primary label.
        pub primary: Label,
        /// Related labels.
        pub secondary: Vec<Label>,
        /// Actionable help.
        pub help: Option<String>,
        /// Suggested repairs.
        pub fixes: Vec<Fix>,
    }
}

/// A package validator call that produced no diagnostics: the component
/// trapped (fuel exhaustion included) or returned an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageValidatorFailure;

/// The linked package validators. The validator calls [`Self::validate`]
/// once per document for each package with targets; an implementation
/// answers an empty list for a package that ships no validator.
pub trait PackageValidator: std::fmt::Debug + Send + Sync {
    /// Checks `targets` (indices into `document.elements`, in document
    /// order, never empty) with the validator of the package bound to
    /// `package` (an extension namespace URI or a service type URI).
    ///
    /// # Errors
    ///
    /// The package validator's failure.
    fn validate(
        &self,
        package: &str,
        document: &document::Context,
        targets: &[u32],
    ) -> Result<Vec<types::Diagnostic>, PackageValidatorFailure>;
}

/// The prefix of one package's codes: for an extension namespace the
/// prefix its first target is written with, for a service type URI (or a
/// namespace declared as the default namespace) its last non-empty path
/// segment (a repository package's `<name>`).
#[must_use]
pub fn package_prefix(package: &str, first_target: Option<&Element>) -> String {
    if let Some(element) = first_target
        && element.name.namespace_uri.as_deref() == Some(package)
        && let Some((prefix, _)) = element.lexical_name.split_once(':')
    {
        return prefix.to_owned();
    }
    package
        .split(['/', ':', '#'])
        .rfind(|segment| !segment.is_empty())
        .unwrap_or(package)
        .to_owned()
}

/// Every package validator finding for `document`.
pub(crate) fn package_validator_diagnostics(
    document: &Document,
    resources: &EventSchemaResources,
    validator: &dyn PackageValidator,
) -> Vec<Diagnostic> {
    let packages = package_targets(document);
    if packages.is_empty() {
        return Vec::new();
    }
    let Some((context, elements)) = build_context(document, resources) else {
        return packages
            .iter()
            .filter_map(|(package, targets)| {
                let first = targets.first()?;
                Some(failed(
                    package,
                    first.start_tag_span,
                    "the document is too large for the package validator interface",
                ))
            })
            .collect();
    };
    let mut diagnostics = Vec::new();
    for (package, targets) in &packages {
        let Some(first) = targets.first() else {
            continue;
        };
        let indices: Vec<u32> = targets
            .iter()
            .filter_map(|target| {
                elements
                    .iter()
                    .position(|element| std::ptr::eq(*element, *target))
                    .and_then(|index| u32::try_from(index).ok())
            })
            .collect();
        let target_spans: Vec<Span> = targets.iter().map(|target| target.span).collect();
        let result = validator.validate(package, &context, &indices);
        let checked = result
            .map_err(|PackageValidatorFailure| {
                "it trapped (fuel exhaustion included) or returned an error".to_owned()
            })
            .and_then(|returned| {
                let prefix = package_prefix(package, Some(first));
                accept(document, &prefix, &target_spans, returned)
            });
        match checked {
            Ok(accepted) => diagnostics.extend(accepted),
            Err(reason) => diagnostics.push(failed(package, first.start_tag_span, &reason)),
        }
    }
    diagnostics
}

/// The packages with targets, in order of first target: each extension
/// namespace with its elements, and each service type URI with the
/// `<invoke>` elements naming it literally.
fn package_targets(document: &Document) -> Vec<(String, Vec<&Element>)> {
    let mut packages: Vec<(String, Vec<&Element>)> = Vec::new();
    document.root.visit(&mut |element| {
        let package = match element.name.namespace_uri.as_deref() {
            Some(SCXML_NAMESPACE) if element.name.local_name == "invoke" => element
                .attribute("type")
                .map(|attribute| attribute.value.as_str())
                .filter(|value| scxml_types::is_service_type_uri(value))
                .filter(|_| element.attribute("typeexpr").is_none()),
            Some(SCXML_NAMESPACE | XML_NAMESPACE | ENV_NAMESPACE_URI | EVENT_NAMESPACE_URI)
            | None => None,
            Some(namespace) => Some(namespace),
        };
        if let Some(package) = package {
            match packages.iter_mut().find(|(uri, _)| uri == package) {
                Some((_, targets)) => targets.push(element),
                None => packages.push((package.to_owned(), vec![element])),
            }
        }
    });
    packages
}

/// The document's context and its elements in the same order; `None` when
/// a span or count does not fit the interface's `u32`.
fn build_context<'a>(
    document: &'a Document,
    resources: &EventSchemaResources,
) -> Option<(document::Context, Vec<&'a Element>)> {
    let mut elements: Vec<&Element> = Vec::new();
    let mut parents: Vec<Option<u32>> = Vec::new();
    let mut pending: Vec<(&Element, Option<u32>)> = vec![(&document.root, None)];
    while let Some((element, parent)) = pending.pop() {
        elements.push(element);
        parents.push(parent);
        let index = u32::try_from(elements.len() - 1).ok()?;
        for child in element.children.iter().rev() {
            if let Node::Element(child) = child {
                pending.push((child, Some(index)));
            }
        }
    }
    let mut context = document::Context {
        datamodel: document
            .root
            .attribute("datamodel")
            .map(|attribute| attribute.value.clone()),
        ..document::Context::default()
    };
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); elements.len()];
    for (child, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children
                .get_mut(usize::try_from(*parent).ok()?)?
                .push(u32::try_from(child).ok()?);
        }
    }
    for (index, (element, children)) in elements.iter().zip(children).enumerate() {
        let attributes = element
            .attributes
            .iter()
            .map(|attribute| {
                Some(document::Attribute {
                    name: qualified(&attribute.name),
                    value: attribute.value.clone(),
                    span: span32(attribute.span)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        context.elements.push(document::Element {
            name: qualified(&element.name),
            span: span32(element.span)?,
            attributes,
            children,
            parent: *parents.get(index)?,
        });
        if element.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
            continue;
        }
        let id = element
            .attribute("id")
            .map(|attribute| attribute.value.clone());
        match element.name.local_name.as_str() {
            "data" => context.data_ids.extend(id),
            "state" | "parallel" | "final" | "history" => context.state_ids.extend(id),
            "transition" => {
                let state = parents
                    .get(index)
                    .copied()
                    .flatten()
                    .and_then(|parent| elements.get(usize::try_from(parent).ok()?))
                    .and_then(|parent| parent.attribute("id"))
                    .map(|attribute| attribute.value.clone())
                    .unwrap_or_default();
                context.transitions.push(document::TransitionInfo {
                    state,
                    event: element
                        .attribute("event")
                        .map(|attribute| attribute.value.clone()),
                    event_schema: crate::event_payloads::literal_schema(element, resources)
                        .map(|resolved| resolved.schema),
                });
            }
            _ => {}
        }
    }
    Some((context, elements))
}

fn qualified(name: &scxml_syntax::ExpandedName) -> document::QualifiedName {
    document::QualifiedName {
        namespace_uri: name.namespace_uri.clone(),
        local_name: name.local_name.clone(),
    }
}

fn span32(span: Span) -> Option<types::Span> {
    Some(types::Span {
        start: u32::try_from(span.start).ok()?,
        end: u32::try_from(span.end).ok()?,
    })
}

/// Checks and renders one package's diagnostics, or the reason they are
/// refused.
fn accept(
    document: &Document,
    prefix: &str,
    targets: &[Span],
    returned: Vec<types::Diagnostic>,
) -> Result<Vec<Diagnostic>, String> {
    if returned.len() > MAX_PACKAGE_DIAGNOSTICS {
        return Err(format!(
            "it returned more than {MAX_PACKAGE_DIAGNOSTICS} diagnostics"
        ));
    }
    let text = document.source.text();
    let source = document.root.span.source;
    let span_of = |span: types::Span| -> Result<Span, String> {
        let (start, end) = (
            usize::try_from(span.start).map_err(|_| "a span is out of range")?,
            usize::try_from(span.end).map_err(|_| "a span is out of range")?,
        );
        if start > end || end > text.len() {
            return Err("a span is outside the document".to_owned());
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err("a span is not on character boundaries".to_owned());
        }
        Ok(Span::new(source, start, end))
    };
    let label = |label: &types::Label| -> Result<Label, String> {
        let span = span_of(label.span)?;
        Ok(match &label.message {
            Some(message) => Label::whole_element(span).with_message(message.clone()),
            None => Label::whole_element(span),
        })
    };
    let mut accepted = Vec::with_capacity(returned.len());
    for diagnostic in returned {
        let code = diagnostic.code.as_str();
        if code.is_empty()
            || code.len() > MAX_PACKAGE_CODE_BYTES
            || !code.starts_with(|first: char| first.is_ascii_lowercase())
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("a diagnostic code is not `[a-z][a-z0-9_]*` within 64 bytes".to_owned());
        }
        let mut fixes = Vec::with_capacity(diagnostic.fixes.len());
        for fix in &diagnostic.fixes {
            let mut edits = Vec::with_capacity(fix.edits.len());
            for edit in &fix.edits {
                let span = span_of(edit.span)?;
                if !targets
                    .iter()
                    .any(|target| target.start <= span.start && span.end <= target.end)
                {
                    return Err("an edit is outside the package's elements".to_owned());
                }
                edits.push(FixEdit {
                    span,
                    replacement: edit.replacement.clone(),
                });
            }
            edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
            if edits
                .windows(2)
                .any(|pair| matches!(pair, [left, right] if left.span.end > right.span.start))
            {
                return Err("a fix has overlapping edits".to_owned());
            }
            fixes.push(Fix {
                message: fix.message.clone(),
                applicability: fix.applicability,
                edits,
            });
        }
        let mut rendered = Diagnostic::error(
            format!("{prefix}::{code}"),
            diagnostic.message.clone(),
            span_of(diagnostic.primary.span)?,
        )
        .with_category(DiagnosticCategory::Package);
        rendered.severity = diagnostic.severity;
        rendered.primary = label(&diagnostic.primary)?;
        for secondary in &diagnostic.secondary {
            rendered = rendered.with_secondary(label(secondary)?);
        }
        if let Some(help) = diagnostic.help {
            rendered = rendered.with_help(help);
        }
        rendered.fixes = fixes;
        accepted.push(rendered);
    }
    Ok(accepted)
}

/// The fail-closed `PKG002` error for one package.
fn failed(package: &str, span: Span, reason: &str) -> Diagnostic {
    Diagnostic::error(
        "PKG002",
        format!("the package validator of `{package}` failed: {reason}"),
        span,
    )
    .with_category(DiagnosticCategory::Package)
    .with_primary_message("the first element this package validator checks")
    .with_note("a package validator's diagnostics are untrusted: when one call fails or returns anything out of contract, all of its diagnostics are discarded and the document is rejected")
    .with_help("fix or update the package's validator component, or bind a package release whose validator works")
}

#[cfg(test)]
mod tests;
