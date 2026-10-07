//! Example package validator component
//! (`stateforward:scxml-validator/package-validator`) for the example
//! extension namespace (`components/scxml-extension-example`,
//! `https://stateforward.org/scxml/example`).
//!
//! It checks what the namespace's XSD cannot: the `in` attribute of
//! `<example:raise>` must name a state of the document (the `unknown_state`
//! error, reported as `<prefix>::unknown_state`, with a machine-applicable
//! fix to the one closest state id when there is exactly one within two
//! edits). Like the extension's `<example:fail/>` and `<example:spin/>`,
//! two `event` values exercise the host's bounds: `example.validator.trap`
//! traps the call and `example.validator.spin` loops until the host's fuel
//! bound traps it, so the validator fails closed (`PKG002`).
//!
//! The decision is the portable [`raise_verdict`], tested natively; the
//! guest maps it onto the WIT types. The world imports nothing.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

/// The example extension namespace this validator checks.
pub const NAMESPACE_URI: &str = "https://stateforward.org/scxml/example";
/// The `event` that traps the call.
pub const TRAP_EVENT: &str = "example.validator.trap";
/// The `event` that loops until the fuel bound traps the call.
pub const SPIN_EVENT: &str = "example.validator.spin";

/// What the validator does with one `<example:raise>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Nothing to report.
    Clean,
    /// `in` names no state; `closest` is the one state id within two edits.
    UnknownState {
        /// The repair, when exactly one state id is that close.
        closest: Option<String>,
    },
    /// Trap the call.
    Trap,
    /// Loop until the fuel bound traps the call.
    Spin,
}

/// The verdict for one `<example:raise event in>`.
#[must_use]
pub fn raise_verdict(event: Option<&str>, in_state: Option<&str>, state_ids: &[String]) -> Verdict {
    match event {
        Some(TRAP_EVENT) => return Verdict::Trap,
        Some(SPIN_EVENT) => return Verdict::Spin,
        _ => {}
    }
    let Some(name) = in_state else {
        return Verdict::Clean;
    };
    if state_ids.iter().any(|id| id == name) {
        return Verdict::Clean;
    }
    let mut close = state_ids
        .iter()
        .filter(|id| distance(name, id) <= 2)
        .collect::<Vec<_>>();
    close.sort();
    close.dedup();
    Verdict::UnknownState {
        closest: match close.as_slice() {
            [only] => Some((*only).clone()),
            _ => None,
        },
    }
}

/// Levenshtein distance over characters.
fn distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = vec![row + 1; right.len() + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitute = previous[column] + usize::from(left_char != *right_char);
            current[column + 1] = substitute
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        previous = current;
    }
    previous[right.len()]
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "package-validator",
        path: "../../crates/scxml-wit/wit/validator",
    });

    use exports::stateforward::scxml_validator::package_validator_api::Guest;
    use stateforward::scxml_validator::document::{Context, Element};
    use stateforward::scxml_validator::types::{
        Applicability, ComponentError, Diagnostic, Edit, Fix, Label, Severity,
    };

    use super::{NAMESPACE_URI, Verdict, raise_verdict};

    struct Component;

    /// The value of an unqualified attribute and the attribute.
    fn attribute<'a>(
        element: &'a Element,
        name: &str,
    ) -> Option<&'a stateforward::scxml_validator::document::Attribute> {
        element.attributes.iter().find(|attribute| {
            attribute.name.namespace_uri.is_none() && attribute.name.local_name == name
        })
    }

    impl Guest for Component {
        fn validate(
            document: Context,
            targets: Vec<u32>,
        ) -> Result<Vec<Diagnostic>, ComponentError> {
            let mut diagnostics = Vec::new();
            for target in targets {
                let Some(element) = usize::try_from(target)
                    .ok()
                    .and_then(|index| document.elements.get(index))
                else {
                    continue;
                };
                if element.name.namespace_uri.as_deref() != Some(NAMESPACE_URI)
                    || element.name.local_name != "raise"
                {
                    continue;
                }
                let event = attribute(element, "event").map(|attribute| attribute.value.as_str());
                let state = attribute(element, "in");
                match raise_verdict(
                    event,
                    state.map(|attribute| attribute.value.as_str()),
                    &document.state_ids,
                ) {
                    Verdict::Clean => {}
                    Verdict::Trap => core::arch::wasm32::unreachable(),
                    Verdict::Spin => loop {
                        core::hint::black_box(());
                    },
                    Verdict::UnknownState { closest } => {
                        let Some(state) = state else { continue };
                        diagnostics.push(Diagnostic {
                            code: "unknown_state".to_owned(),
                            severity: Severity::Error,
                            message: format!(
                                "`in` names `{}`, which is not a state of this document",
                                state.value
                            ),
                            primary: Label {
                                span: state.span,
                                message: Some("no state has this id".to_owned()),
                            },
                            secondary: Vec::new(),
                            help: Some(closest.as_ref().map_or_else(
                                || "name the id of a `<state>`, `<parallel>`, `<final>`, or `<history>`".to_owned(),
                                |closest| format!("did you mean `{closest}`?"),
                            )),
                            fixes: closest
                                .into_iter()
                                .map(|closest| Fix {
                                    message: format!("use `{closest}`"),
                                    applicability: Applicability::MachineApplicable,
                                    edits: vec![Edit {
                                        span: state.span,
                                        replacement: format!("in=\"{closest}\""),
                                    }],
                                })
                                .collect(),
                        });
                    }
                }
            }
            Ok(diagnostics)
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn in_must_name_a_state_and_one_close_id_is_the_fix() {
        let states = ids(&["idle", "busy", "done"]);
        assert_eq!(raise_verdict(None, None, &states), Verdict::Clean);
        assert_eq!(raise_verdict(None, Some("busy"), &states), Verdict::Clean);
        assert_eq!(
            raise_verdict(Some("e"), Some("bsy"), &states),
            Verdict::UnknownState {
                closest: Some("busy".to_owned())
            }
        );
        assert_eq!(
            raise_verdict(None, Some("elsewhere"), &states),
            Verdict::UnknownState { closest: None }
        );
        // Two ids equally close: no machine-applicable guess.
        assert_eq!(
            raise_verdict(None, Some("ab"), &ids(&["aa", "bb"])),
            Verdict::UnknownState { closest: None }
        );
    }

    #[test]
    fn the_bound_hooks_are_exact_event_names() {
        assert_eq!(raise_verdict(Some(TRAP_EVENT), None, &[]), Verdict::Trap);
        assert_eq!(raise_verdict(Some(SPIN_EVENT), None, &[]), Verdict::Spin);
        assert_eq!(
            raise_verdict(Some("example.validator.trapped"), None, &[]),
            Verdict::Clean
        );
    }
}
