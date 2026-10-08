//! The `env` attribute vocabulary ([`scxml_types::ENV_NAMESPACE_URI`]).
//!
//! The `<invoke>` of an invoked service may carry `env:<slot>="NAME"` or
//! `env:<slot>expr="EXPR"` attributes naming the variable whose value the
//! host binds to one of the service's slots, as an opaque reference. The
//! slots an `<invoke>` accepts are the ones its service package declares
//! (`env-slots`). These rules check them:
//!
//! * `ENV001`: an `env` attribute names a slot the service's package does
//!   not declare, names both `<slot>` and `<slot>expr`, or names a variable
//!   that is not a valid name. Checked against the package for a literal
//!   service type the validator was given ([`crate::Validator::with_services`]);
//!   otherwise (an unbound type, or an `<invoke typeexpr>`, which names its
//!   service only at run time) only what Machine IR requires of any binding.
//! * `ENV002`: an element in the `env` namespace (it has none), or an `env`
//!   attribute on any element other than an SCXML `<invoke>` of a service:
//!   an element in no namespace, an executable-content extension element
//!   (executable content does no I/O, so it has no slots), or an `<invoke>`
//!   of an SCXML session (an absent `type` or an SCXML invoke type), or any
//!   other SCXML element.

use std::collections::BTreeMap;

use scxml_reporting::Applicability;
use scxml_syntax::{Attribute, Document, Element, Span};
use scxml_types::ENV_NAMESPACE_URI;

use crate::{Diagnostic, DiagnosticCategory, Fix};

/// The suffix of an `env:<slot>expr` attribute.
const EXPR_SUFFIX: &str = "expr";
/// Maximum bytes of one variable name an `env:<slot>` attribute names.
pub const MAX_ENV_NAME_BYTES: usize = 256;

/// Whether `name` is a variable name an `env:<slot>` attribute may name:
/// `[A-Za-z_][A-Za-z0-9_]*`, at most [`MAX_ENV_NAME_BYTES`] bytes.
#[must_use]
pub fn is_env_name(name: &str) -> bool {
    name.len() <= MAX_ENV_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// One `env` attribute read against its element's declared slots: the slot
/// and whether it is the `expr` form, or `None` when it names no declared
/// slot.
#[must_use]
pub fn env_slot<'a>(local_name: &'a str, declared: &[String]) -> Option<(&'a str, bool)> {
    if declared.iter().any(|slot| slot == local_name) {
        return Some((local_name, false));
    }
    local_name
        .strip_suffix(EXPR_SUFFIX)
        .filter(|slot| declared.iter().any(|declared| declared == slot))
        .map(|slot| (slot, true))
}

/// Every `ENV001` and `ENV002` finding of `document`; `services` maps an
/// invoked-service type URI to its package's `env` slots.
#[must_use]
pub fn env_attribute_diagnostics(
    document: &Document,
    services: Option<&BTreeMap<String, Vec<String>>>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    document.root.visit(&mut |element| {
        let namespace = element.name.namespace_uri.as_deref();
        if namespace == Some(ENV_NAMESPACE_URI) {
            diagnostics.push(env_element(element));
            return;
        }
        let env_attributes: Vec<&Attribute> = element
            .attributes
            .iter()
            .filter(|attribute| attribute.name.namespace_uri.as_deref() == Some(ENV_NAMESPACE_URI))
            .collect();
        if env_attributes.is_empty() {
            return;
        }
        match namespace {
            Some(crate::SCXML_NAMESPACE) if element.name.local_name == "invoke" => {
                invoke_slots(element, &env_attributes, services, &mut diagnostics);
            }
            None | Some(_) => {
                for attribute in env_attributes {
                    diagnostics.push(unplaced(element, attribute));
                }
            }
        }
    });
    diagnostics
}

fn check_slots(
    element: &Element,
    namespace: &str,
    attributes: &[&Attribute],
    declared: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen: Vec<(&str, Span)> = Vec::new();
    for attribute in attributes {
        let local = attribute.name.local_name.as_str();
        let Some((slot, expr)) = env_slot(local, declared) else {
            diagnostics.push(undeclared(element, namespace, attribute, declared));
            continue;
        };
        if let Some((_, first)) = seen.iter().find(|(seen, _)| *seen == slot) {
            diagnostics.push(
                Diagnostic::error(
                    "ENV001",
                    format!("`env` slot `{slot}` is named twice on `<{}>`", element.lexical_name),
                    attribute.span,
                )
                .with_category(DiagnosticCategory::Semantic)
                .with_primary_message("a second binding of the same slot")
                .with_secondary(
                    crate::Label::new(*first).with_message("the slot is first bound here"),
                )
                .with_help(format!(
                    "keep either `env:{slot}` (a variable name) or `env:{slot}expr` (an expression that evaluates to one)"
                )),
            );
            continue;
        }
        seen.push((slot, attribute.span));
        if !expr && !is_env_name(&attribute.value) {
            diagnostics.push(
                Diagnostic::error(
                    "ENV001",
                    format!("`env:{slot}` does not name a variable"),
                    attribute.value_span,
                )
                .with_category(DiagnosticCategory::Semantic)
                .with_primary_message("not a variable name")
                .with_note(format!(
                    "an `env:<slot>` attribute names the variable whose value the host binds to the slot: `[A-Za-z_][A-Za-z0-9_]*`, at most {MAX_ENV_NAME_BYTES} bytes; the value itself never appears in a document"
                ))
                .with_help(format!(
                    "name the variable (for example `env:{slot}=\"API_TOKEN\"`), or use `env:{slot}expr` for an expression"
                )),
            );
        }
    }
}

/// The `env` attributes of one `<invoke>`: judged against the bound service
/// package of a literal service type, `ENV002` on an SCXML session's, and
/// left to the host for a `typeexpr`.
fn invoke_slots(
    element: &Element,
    attributes: &[&Attribute],
    services: Option<&BTreeMap<String, Vec<String>>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let literal = |name: &str| {
        element
            .attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace_uri.is_none() && attribute.name.local_name == name
            })
            .map(|attribute| attribute.value.as_str())
    };
    match literal("type") {
        Some(type_uri) if !scxml_types::is_scxml_invoke_type(type_uri) => {
            match services.and_then(|services| services.get(type_uri)) {
                Some(slots) => {
                    check_slots(element, type_uri, attributes, slots, diagnostics);
                }
                None => check_slot_syntax(element, attributes, diagnostics),
            }
        }
        None if literal("typeexpr").is_some() => {
            check_slot_syntax(element, attributes, diagnostics);
        }
        _ => {
            for attribute in attributes {
                diagnostics.push(
                    Diagnostic::error(
                        "ENV002",
                        format!(
                            "`{}` is on an `<invoke>` of an SCXML session, which has no slots",
                            attribute.lexical_name
                        ),
                        attribute.span,
                    )
                    .with_category(DiagnosticCategory::Semantic)
                    .with_primary_message("only an invoked service's `<invoke>` has slots")
                    .with_note("an absent `type`, `scxml`, or `http://www.w3.org/TR/scxml/` invokes an SCXML session; `env` slots belong to invoked services (an `<invoke type>` naming a service package's `service-uri`)")
                    .with_help("remove the attribute, or invoke the service whose package declares the slot"),
                );
            }
        }
    }
}

/// `ENV001` for the `env` attributes of an `<invoke>` whose package the
/// validator was not given (or whose type is a `typeexpr`): what Machine IR
/// requires of any slot binding, whatever the package declares. A slot is an
/// `env-slots` name, bound once, and a literal binding names a variable.
fn check_slot_syntax(
    element: &Element,
    attributes: &[&Attribute],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let declared: Vec<String> = attributes
        .iter()
        .map(|attribute| {
            let local = attribute.name.local_name.as_str();
            match local.strip_suffix(EXPR_SUFFIX) {
                Some(slot) if !slot.is_empty() => slot.to_owned(),
                _ => local.to_owned(),
            }
        })
        .collect();
    for (attribute, slot) in attributes.iter().zip(&declared) {
        if !crate::is_env_slot(slot) {
            diagnostics.push(
                Diagnostic::error(
                    "ENV001",
                    format!("`{}` names no valid `env` slot", attribute.lexical_name),
                    attribute.span,
                )
                .with_category(DiagnosticCategory::Semantic)
                .with_primary_message("not an `env-slots` name")
                .with_note("a slot is `[a-z][a-z0-9-]*`, at most 64 bytes, never ending in `expr` (`env:<slot>expr` is the expression form)")
                .with_help("bind a slot the service's package declares (`env-slots`)"),
            );
        }
    }
    let valid: Vec<String> = declared
        .into_iter()
        .filter(|slot| crate::is_env_slot(slot))
        .collect();
    let attributes: Vec<&Attribute> = attributes
        .iter()
        .copied()
        .filter(|attribute| {
            let local = attribute.name.local_name.as_str();
            let slot = match local.strip_suffix(EXPR_SUFFIX) {
                Some(slot) if !slot.is_empty() => slot,
                _ => local,
            };
            crate::is_env_slot(slot)
        })
        .collect();
    check_slots(element, "", &attributes, &valid, diagnostics);
}

fn env_element(element: &Element) -> Diagnostic {
    Diagnostic::error(
        "ENV002",
        format!(
            "`<{}>` is an element in the `env` namespace, which has none",
            element.lexical_name
        ),
        element.start_tag_span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("not an element of any vocabulary")
    .with_note(format!(
        "`{ENV_NAMESPACE_URI}` is an attribute vocabulary only: an invoked service's `<invoke env:<slot>=\"NAME\">` names the variable whose value the host binds to that slot"
    ))
    .with_help("remove the element, and put an `env:<slot>` attribute on the `<invoke>` of the service that needs the value")
}

fn unplaced(element: &Element, attribute: &Attribute) -> Diagnostic {
    let message = if element.name.namespace_uri.is_none() {
        format!(
            "`{}` is on an element in no namespace, which has no slots",
            attribute.lexical_name
        )
    } else if element.name.namespace_uri.as_deref() == Some(crate::SCXML_NAMESPACE) {
        format!(
            "`{}` is on `<{}>`, which has no slots",
            attribute.lexical_name, element.lexical_name
        )
    } else {
        format!(
            "`{}` is on the executable-content extension `<{}>`, which has no slots",
            attribute.lexical_name, element.lexical_name
        )
    };
    Diagnostic::error("ENV002", message, attribute.span)
        .with_category(DiagnosticCategory::Semantic)
        .with_primary_message("only an invoked service's `<invoke>` has slots")
        .with_note("executable content runs synchronously inside a microstep and does no I/O, so it never holds a value; work that needs one (a token for a network request, for example) belongs in an invoked service (SCXML 6.4)")
        .with_help("remove the attribute, and bind the slot on the `<invoke>` of the service that needs the value")
}

fn undeclared(
    element: &Element,
    namespace: &str,
    attribute: &Attribute,
    declared: &[String],
) -> Diagnostic {
    let local = attribute.name.local_name.as_str();
    let mut diagnostic = Diagnostic::error(
        "ENV001",
        format!(
            "`<{}>` has no `env` slot `{}`",
            element.lexical_name,
            local.strip_suffix(EXPR_SUFFIX).unwrap_or(local)
        ),
        attribute.span,
    )
    .with_category(DiagnosticCategory::Semantic)
    .with_primary_message("not a slot its package declares")
    .with_note(if declared.is_empty() {
        format!("the package of `{namespace}` declares no `env-slots`")
    } else {
        format!(
            "the package of `{namespace}` declares the slots {}",
            declared
                .iter()
                .map(|slot| format!("`{slot}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    let (stem, suffix) = match local.strip_suffix(EXPR_SUFFIX) {
        Some(stem) if !stem.is_empty() => (stem, EXPR_SUFFIX),
        _ => (local, ""),
    };
    let closest = declared
        .iter()
        .map(|slot| {
            let stem: Vec<char> = stem.chars().collect();
            let candidate: Vec<char> = slot.chars().collect();
            (crate::semantic::osa_distance(&stem, &candidate), slot)
        })
        .filter(|(distance, _)| *distance <= 2)
        .min();
    if let Some((_, slot)) = closest {
        let prefix_len = attribute.lexical_name.len().saturating_sub(local.len());
        let name_span = Span::new(
            attribute.span.source,
            attribute.span.start.saturating_add(prefix_len),
            attribute
                .span
                .start
                .saturating_add(attribute.lexical_name.len()),
        );
        diagnostic = diagnostic
            .with_help(format!("did you mean `{slot}{suffix}`?"))
            .with_fix(Fix::replace(
                format!("bind the declared slot `{slot}`"),
                Applicability::MaybeIncorrect,
                name_span,
                format!("{slot}{suffix}"),
            ));
    } else {
        diagnostic = diagnostic.with_help("bind only a slot the service's package declares");
    }
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(1),
            "d.scxml",
            text,
        ))
        .expect("the fixture parses")
    }

    fn machine(body: &str) -> String {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:x="urn:x" xmlns:env="{ENV_NAMESPACE_URI}" version="1.0"><state id="s"><onentry>{body}</onentry></state></scxml>"#
        )
    }

    #[test]
    fn only_a_service_invoke_has_slots() {
        for body in [
            r#"<env:get name="K"/>"#,
            r#"<x:a env:key="K"/>"#,
            r#"<x:a env:keyexpr="'K'"/>"#,
            r#"<x:a><b xmlns="" env:key="K"/></x:a>"#,
        ] {
            assert_eq!(
                env_attribute_diagnostics(&document(&machine(body)), None)
                    .iter()
                    .map(|diagnostic| diagnostic.code.as_str())
                    .collect::<Vec<_>>(),
                ["ENV002"],
                "{body}"
            );
        }
        assert_eq!(
            env_attribute_diagnostics(&document(&machine(r#"<log env:key="K"/>"#)), None)
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            ["ENV002"]
        );
    }

    #[test]
    fn invoked_services_have_the_slots_their_package_declares() {
        let services = BTreeMap::from([("urn:svc".to_owned(), vec!["apikey".to_owned()])]);
        let codes = |invoke: &str| -> Vec<String> {
            let text = format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="{ENV_NAMESPACE_URI}" version="1.0"><state id="s">{invoke}</state></scxml>"#
            );
            env_attribute_diagnostics(&document(&text), Some(&services))
                .into_iter()
                .map(|diagnostic| diagnostic.code)
                .collect()
        };
        assert!(codes(r#"<invoke type="urn:svc" env:apikey="KEY"/>"#).is_empty());
        assert!(codes(r#"<invoke type="urn:svc" env:apikeyexpr="'KEY'"/>"#).is_empty());
        assert_eq!(
            codes(r#"<invoke type="urn:svc" env:apikye="KEY"/>"#),
            ["ENV001"]
        );
        assert_eq!(
            codes(r#"<invoke type="urn:svc" env:apikey="KEY" env:apikeyexpr="'K'"/>"#),
            ["ENV001"]
        );
        // An unbound service is PKG004's, and a `typeexpr`'s slots the
        // host's; only what Machine IR requires of any binding is judged.
        assert!(codes(r#"<invoke type="urn:other" env:x="K"/>"#).is_empty());
        assert!(codes(r#"<invoke typeexpr="'urn:svc'" env:x="K"/>"#).is_empty());
        for bad in [
            r#"<invoke type="urn:other" env:x="K" env:xexpr="'K'"/>"#,
            r#"<invoke typeexpr="'urn:svc'" env:x="1K"/>"#,
            r#"<invoke typeexpr="'urn:svc'" env:Bad="K"/>"#,
        ] {
            assert_eq!(codes(bad), ["ENV001"], "{bad}");
        }
        for session in [
            r#"<invoke src="c.scxml" env:apikey="K"/>"#,
            r#"<invoke type="scxml" src="c.scxml" env:apikey="K"/>"#,
        ] {
            assert_eq!(codes(session), ["ENV002"], "{session}");
        }
    }
}
