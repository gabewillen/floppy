//! The `datamodel` attribute vocabulary
//! ([`scxml_types::DATAMODEL_NAMESPACE_URI`]) and the data model each
//! machine names.
//!
//! The core `datamodel` attribute is the token the SCXML schema requires
//! (an `NMTOKEN`, SCXML 1.0 §3.2.1). A data model other than the built-in
//! `ecmascript`, `null`, and `xpath` is a package, named on the same
//! `<scxml>` by `datamodel:src` (its package URI, optionally `@<tag>`,
//! which the parser split off). These rules check that pairing:
//!
//! * `DATAMODEL001`: an element in the `datamodel` namespace (it has none),
//!   a `datamodel` attribute on any element other than `<scxml>`, or an
//!   attribute name the vocabulary does not define (it defines only
//!   `datamodel:src`; a near miss gets a did-you-mean fix).
//! * `DATAMODEL002`: the `datamodel` token is not the name the package
//!   `datamodel:src` binds declares (judged when the caller supplies its
//!   data-model packages); a machine-applicable fix writes that name.
//! * `PKG005`: the machine's data model is not available: a `datamodel`
//!   token that is no built-in data model with no `datamodel:src` (with a
//!   typo fix to a built-in one), a `datamodel:src` that is no tagless
//!   absolute URI, or one no given data-model package binds.
//!
//! Pure and bounded by the document; it runs inside the validator's
//! boundary span and records no telemetry of its own.

use std::collections::BTreeMap;

use scxml_reporting::Applicability;
use scxml_syntax::{Attribute, Document, Element, Span, collapse_whitespace};
use scxml_types::DATAMODEL_NAMESPACE_URI;

use crate::datamodel_check::{DEFAULT_DATA_MODEL, KNOWN_DATA_MODELS};
use crate::semantic::{is_scxml, machine_roots, nearest};
use crate::spans::{attribute_insertion_point, attribute_removal_span};
use crate::{Diagnostic, DiagnosticCategory, Fix};

/// The one attribute the vocabulary defines.
const SRC: &str = "src";
const SPEC_DATAMODEL: &str = "SCXML 1.0 §5.1 Data Model";

/// Every finding of the `datamodel` vocabulary and of each machine's data
/// model in `document`. `data_models` maps each data-model package URI the
/// caller's packages bind to the name its package declares; `None` judges
/// no binding (a `datamodel:src` is then not checked against a package).
#[must_use]
pub fn datamodel_attribute_diagnostics(
    document: &Document,
    data_models: Option<&BTreeMap<String, String>>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    document.root.visit(&mut |element| {
        if element.name.namespace_uri.as_deref() == Some(DATAMODEL_NAMESPACE_URI) {
            diagnostics.push(datamodel_element(element));
            return;
        }
        let owner = is_scxml(element, "scxml");
        for attribute in element.attributes.iter().filter(|attribute| {
            attribute.name.namespace_uri.as_deref() == Some(DATAMODEL_NAMESPACE_URI)
        }) {
            if !owner {
                diagnostics.push(misplaced(document, element, attribute));
            } else if attribute.name.local_name != SRC {
                diagnostics.push(unknown(element, attribute));
            }
        }
    });
    for machine in machine_roots(&document.root) {
        diagnostics.extend(machine_data_model(document, machine, data_models));
    }
    diagnostics
}

/// Adds [`datamodel_attribute_diagnostics`] to `report`. One mistake, one
/// finding: a machine whose data model is not available (`PKG005`) loses
/// the `LINT010` note that its expressions were not checked.
pub(crate) fn report_data_models(
    document: &Document,
    data_models: Option<&BTreeMap<String, String>>,
    report: &mut crate::Report,
) {
    let diagnostics = datamodel_attribute_diagnostics(document, data_models);
    let within = |tag: Span, span: Span| tag.start <= span.start && span.end <= tag.end;
    let unavailable: Vec<Span> = machine_roots(&document.root)
        .into_iter()
        .map(|machine| machine.start_tag_span)
        .filter(|tag| {
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "PKG005" && within(*tag, diagnostic.primary.span)
            })
        })
        .collect();
    report.retain(|diagnostic| {
        diagnostic.code != "LINT010"
            || !unavailable
                .iter()
                .any(|tag| within(*tag, diagnostic.primary.span))
    });
    for diagnostic in diagnostics {
        report.push(diagnostic);
    }
}

/// The `datamodel:src` of `machine`, when it has one.
pub(crate) fn package_source(machine: &Element) -> Option<&Attribute> {
    machine
        .attributes
        .iter()
        .find(|attribute| attribute.name.is(Some(DATAMODEL_NAMESPACE_URI), SRC))
}

fn machine_data_model(
    document: &Document,
    machine: &Element,
    data_models: Option<&BTreeMap<String, String>>,
) -> Option<Diagnostic> {
    let declared = machine.attribute("datamodel");
    let token = declared.map_or(
        std::borrow::Cow::Borrowed(DEFAULT_DATA_MODEL),
        |attribute| collapse_whitespace(&attribute.value),
    );
    let Some(source) = package_source(machine) else {
        let datamodel = declared?;
        return (!KNOWN_DATA_MODELS.contains(&token.as_ref()))
            .then(|| unknown_data_model(datamodel, &token));
    };
    let uri = collapse_whitespace(&source.value);
    if !crate::is_data_model_uri(&uri) {
        return Some(unbound(
            source,
            format!("`{uri}` is not a data-model package URI"),
            "a `datamodel:src` is an absolute URI: `https://github.com/<owner>/<repo>/datamodels/<name>`, optionally followed by one `@<tag>`, or the URI a given data-model package binds",
        ));
    }
    let Some(name) = data_models?.get(uri.as_ref()) else {
        return Some(unbound(
            source,
            format!("no data-model package binds `{uri}`"),
            "the data model's package did not resolve and no `--data-model-package` binds this URI, so none of this machine's expressions can be evaluated",
        ));
    };
    if token.as_ref() == name {
        return None;
    }
    Some(mismatch(document, machine, declared, &token, name, source))
}

fn datamodel_element(element: &Element) -> Diagnostic {
    Diagnostic::error(
        "DATAMODEL001",
        format!(
            "`<{}>` is an element in the `datamodel` namespace, which has none",
            element.lexical_name
        ),
        element.start_tag_span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("not an element of any vocabulary")
    .with_note(format!(
        "`{DATAMODEL_NAMESPACE_URI}` is an attribute vocabulary only: `datamodel:src` on `<scxml>` names the package of the data model its `datamodel` token names"
    ))
    .with_help("remove the element, and name the data model's package with `datamodel:src` on `<scxml>`")
}

fn misplaced(document: &Document, element: &Element, attribute: &Attribute) -> Diagnostic {
    Diagnostic::error(
        "DATAMODEL001",
        format!(
            "`{}` is not allowed on `<{}>`",
            attribute.lexical_name, element.lexical_name
        ),
        attribute.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("a `datamodel` attribute on an element that names no data model")
    .with_note("only `<scxml>` names a data model (SCXML 1.0 §3.2.1), so only `<scxml>` may name its package; an invoked document declares its own `datamodel` and `datamodel:src`")
    .with_help("move the attribute to the `<scxml>` whose data model it names, or remove it")
    .with_fix(Fix::replace(
        format!("remove `{}`", attribute.lexical_name),
        Applicability::MaybeIncorrect,
        attribute_removal_span(document, attribute),
        "",
    ))
}

fn unknown(element: &Element, attribute: &Attribute) -> Diagnostic {
    let local = attribute.name.local_name.as_str();
    let diagnostic = Diagnostic::error(
        "DATAMODEL001",
        format!(
            "`{}` is not an attribute of the `datamodel` vocabulary",
            attribute.lexical_name
        ),
        attribute.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!("unknown on `<{}>`", element.lexical_name))
    .with_note("the `datamodel` vocabulary has exactly one attribute: `datamodel:src`, the URI of the data model's package");
    let typed: Vec<char> = local.chars().collect();
    let candidate: Vec<char> = SRC.chars().collect();
    if crate::semantic::osa_distance(&typed, &candidate) <= 1 {
        let prefix_len = attribute.lexical_name.len().saturating_sub(local.len());
        let name_span = Span::new(
            attribute.span.source,
            attribute.span.start.saturating_add(prefix_len),
            attribute
                .span
                .start
                .saturating_add(attribute.lexical_name.len()),
        );
        diagnostic
            .with_help(format!("did you mean `{SRC}`?"))
            .with_fix(Fix::replace(
                format!("use `{SRC}`"),
                Applicability::MaybeIncorrect,
                name_span,
                SRC,
            ))
    } else {
        diagnostic.with_help("use `datamodel:src`, or remove the attribute")
    }
}

fn unknown_data_model(datamodel: &Attribute, token: &str) -> Diagnostic {
    let suggestion = KNOWN_DATA_MODELS
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(token))
        .or_else(|| nearest(token, KNOWN_DATA_MODELS.iter().copied()));
    let diagnostic = Diagnostic::error(
        "PKG005",
        format!("unknown data model `{token}`"),
        datamodel.value_span,
    )
    .with_category(DiagnosticCategory::Package)
    .with_primary_message("neither a built-in data model nor one a `datamodel:src` names")
    .with_note("the built-in data models are `ecmascript`, `null`, and `xpath`; any other data model is a package, named on the same `<scxml>` by `datamodel:src`, and without one none of this machine's expressions can be evaluated")
    .with_spec_reference(SPEC_DATAMODEL);
    match suggestion {
        Some(suggestion) => diagnostic
            .with_help(format!(
                "did you mean `{suggestion}`? Or name the data model's package with `datamodel:src`"
            ))
            .with_fix(Fix::replace(
                format!("use the `{suggestion}` data model"),
                Applicability::MaybeIncorrect,
                datamodel.value_span,
                suggestion,
            )),
        None => diagnostic.with_help(format!(
            "name the data model's package with `datamodel:src=\"https://github.com/<owner>/<repo>/datamodels/{token}\"` (`xmlns:datamodel=\"{DATAMODEL_NAMESPACE_URI}\"`), or use a built-in data model"
        )),
    }
}

fn unbound(source: &Attribute, message: String, note: &str) -> Diagnostic {
    Diagnostic::error("PKG005", message, source.value_span)
        .with_category(DiagnosticCategory::Package)
        .with_primary_message("no data model is available for this machine")
        .with_note(note)
        .with_help("bind the package with `--data-model-package MANIFEST.json`, or name a repository data model whose release resolves")
        .with_spec_reference(SPEC_DATAMODEL)
}

fn mismatch(
    document: &Document,
    machine: &Element,
    declared: Option<&Attribute>,
    token: &str,
    name: &str,
    source: &Attribute,
) -> Diagnostic {
    let (span, fix) = match declared {
        Some(datamodel) => (
            datamodel.value_span,
            Fix::replace(
                format!("use `{name}`"),
                Applicability::MachineApplicable,
                datamodel.value_span,
                name,
            ),
        ),
        None => (
            machine.start_tag_span,
            Fix::replace(
                format!("declare `datamodel=\"{name}\"`"),
                Applicability::MachineApplicable,
                attribute_insertion_point(document, machine),
                format!(" datamodel=\"{name}\""),
            ),
        ),
    };
    Diagnostic::error(
        "DATAMODEL002",
        format!("the data model is `{token}`, but its package names it `{name}`"),
        span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message(format!("not `{name}`"))
    .with_secondary(
        crate::Label::new(source.value_span).with_message(format!("this package declares `{name}`")),
    )
    .with_note("the `datamodel` token names the data model the machine's expressions are written in, and the package `datamodel:src` names declares which one it implements; they must agree")
    .with_help(format!("write `datamodel=\"{name}\"`, or name the package of `{token}`"))
    .with_spec_reference(SPEC_DATAMODEL)
    .with_fix(fix)
}
