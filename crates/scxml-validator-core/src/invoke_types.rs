//! Literal `<invoke type>` values that name invoked services (SCXML 6.4).
//!
//! An `<invoke>` whose `type` is absent or an SCXML invoke type
//! ([`scxml_types::is_scxml_invoke_type`]) starts an invoked SCXML session.
//! Any other literal type names an invoked service, which a host runs only
//! through the service component it binds to that exact URI:
//!
//! * `SCXML093` (error): the type is neither an SCXML invoke type nor an
//!   absolute URI of at most [`scxml_types::MAX_SERVICE_TYPE_URI_BYTES`]
//!   bytes ([`scxml_types::is_service_type_uri`]). No host can bind it, so
//!   the invocation could only fail with `error.execution`; lowering rejects
//!   it.
//! * `SCXML094` (warning): the type is a well-formed service URI that no
//!   service package the validator was given binds
//!   ([`crate::Validator::with_services`]). The session start fails
//!   closed unless the running host binds it; `compile` rejects it
//!   (`COMP004`). Checked only when the validator was given its bound
//!   service types.
//!
//! `typeexpr` values are evaluated at run time and are not judged here.

use std::collections::BTreeMap;

use scxml_syntax::Document;
use scxml_types::{MAX_SERVICE_TYPE_URI_BYTES, is_scxml_invoke_type, is_service_type_uri};

use crate::{Diagnostic, DiagnosticCategory, SCXML_NAMESPACE};

const SPEC_INVOKE: &str = "SCXML 1.0 §6.4 <invoke>";

/// Every `SCXML093` and `SCXML094` finding of `document`, in document order;
/// `bound` holds the services bound by the service packages the validator
/// was given, by type URI (`None`: `SCXML094` is not judged).
#[must_use]
pub fn invoke_type_diagnostics(
    document: &Document,
    bound: Option<&BTreeMap<String, crate::BoundService>>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    document.root.visit(&mut |element| {
        if !element.name.is(Some(SCXML_NAMESPACE), "invoke") {
            return;
        }
        let Some(attribute) = element
            .attributes
            .iter()
            .find(|attribute| attribute.name.namespace_uri.is_none() && attribute.name.local_name == "type")
        else {
            return;
        };
        let value = attribute.value.as_str();
        if is_scxml_invoke_type(value) {
            return;
        }
        if !is_service_type_uri(value) {
            diagnostics.push(
                Diagnostic::error(
                    "SCXML093",
                    "this `<invoke type>` is not an SCXML invoke type or an absolute service type URI",
                    attribute.value_span,
                )
                .with_category(DiagnosticCategory::Semantic)
                .with_primary_message("not a type any host can bind")
                .with_note(format!(
                    "an absent `type`, `scxml`, or `http://www.w3.org/TR/scxml/` invokes an SCXML session; any other type names an invoked service by an exact absolute URI (a scheme, `:`, no white space, at most {MAX_SERVICE_TYPE_URI_BYTES} bytes), which the host binds to a service component"
                ))
                .with_help("write the service's absolute type URI exactly as its service package declares it (`service-uri`), or `scxml` for an SCXML session")
                .with_spec_reference(SPEC_INVOKE),
            );
            return;
        }
        if let Some(bound) = bound
            && !bound.contains_key(value)
        {
            diagnostics.push(
                Diagnostic::warning(
                    "SCXML094",
                    "no service package binds this `<invoke type>`",
                    attribute.value_span,
                )
                .with_category(DiagnosticCategory::Package)
                .with_primary_message("no service component is bound to this URI")
                .with_note("an invoked service runs only in the service component its host binds to the exact type URI; without one the session start fails closed, and `compile` rejects the document (`COMP004`)")
                .with_help("bind the service with its package manifest (`--service-package MANIFEST.json`), or correct the URI to one a bound package declares")
                .with_spec_reference(SPEC_INVOKE),
            );
        }
    });
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::invoke_type_diagnostics;
    use std::collections::BTreeMap;

    fn codes(source: &str, bound: Option<&BTreeMap<String, crate::BoundService>>) -> Vec<String> {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(0),
            "t.scxml",
            source,
        ))
        .expect("well formed");
        invoke_type_diagnostics(&document, bound)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    fn machine(invoke: &str) -> String {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><state id="s">{invoke}</state></scxml>"#
        )
    }

    #[test]
    fn scxml_types_and_typeexpr_are_not_judged() {
        let bound = BTreeMap::new();
        for invoke in [
            r#"<invoke src="c.scxml"/>"#,
            r#"<invoke type="scxml" src="c.scxml"/>"#,
            r#"<invoke type="http://www.w3.org/TR/scxml/" src="c.scxml"/>"#,
            r#"<invoke type="http://www.w3.org/TR/scxml" src="c.scxml"/>"#,
            r#"<invoke typeexpr="'echo'"/>"#,
        ] {
            assert!(codes(&machine(invoke), Some(&bound)).is_empty(), "{invoke}");
        }
    }

    #[test]
    fn malformed_types_are_errors_and_unbound_services_warnings() {
        assert_eq!(
            codes(&machine(r#"<invoke type="echo"/>"#), None),
            ["SCXML093"]
        );
        assert_eq!(
            codes(&machine(r#"<invoke type="urn:a b"/>"#), None),
            ["SCXML093"]
        );
        let uri = "https://stateforward.org/scxml/services/echo";
        let invoke = format!(r#"<invoke type="{uri}"/>"#);
        assert!(codes(&machine(&invoke), None).is_empty());
        assert_eq!(
            codes(&machine(&invoke), Some(&BTreeMap::new())),
            ["SCXML094"]
        );
        let bound = BTreeMap::from([(uri.to_owned(), crate::BoundService::default())]);
        assert!(codes(&machine(&invoke), Some(&bound)).is_empty());
    }
}
