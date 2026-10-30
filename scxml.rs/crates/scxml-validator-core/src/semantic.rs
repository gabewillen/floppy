//! Separately testable SCXML semantic rules.

use std::collections::{BTreeMap, BTreeSet};

use scxml_syntax::{
    Attribute, Document, Element, Node, Span, XmlLimits, collapse_whitespace, is_xml_space,
    validate_document_limits, xml_tokens,
};
use scxml_types::wasm_bound;

use crate::spans::{attribute_insertion_point, attribute_removal_span};
use crate::{Applicability, Diagnostic, DiagnosticCategory, Fix, FixEdit, Label, Report};

/// W3C SCXML namespace URI.
pub const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";

const MAX_TYPO_SUGGESTION_IDENTIFIER_CHARS: usize = 128;
const MAX_TYPO_SUGGESTION_CANDIDATES: usize = 65_536;
/// Work units for one typo search (character reads plus distance cells).
const MAX_TYPO_SUGGESTION_EDIT_CELLS: usize = 4_000_000;
/// Work units shared by every typo search over one machine's references.
const MAX_TYPO_SUGGESTION_MACHINE_CELLS: usize = 32_000_000;
const MAX_TYPO_SUGGESTION_DISTANCE: usize = 3;
/// Maximum SCXML document roots per inline-machine path, including the outer
/// root (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_NESTED_SCXML_DEPTH: usize = wasm_bound(64);

/// One deterministic semantic validation rule.
pub trait Rule {
    /// Stable rule code.
    fn code(&self) -> &'static str;

    /// Adds findings to the shared report.
    fn check(&self, document: &Document, report: &mut Report);
}

/// Runs the default validator-first semantic rule inventory.
#[must_use]
pub fn validate_semantics(document: &Document) -> Report {
    if let Err(error) = validate_document_limits(document, XmlLimits::DEFAULT) {
        let mut report = Report::new();
        report.push(Diagnostic::error("XML002", error.message, error.span));
        return report;
    }
    if let Some(machine) = first_machine_over_depth(&document.root) {
        let mut report = Report::new();
        report.push(
            Diagnostic::error(
                "LIMIT001",
                format!("nested SCXML document depth exceeds {MAX_NESTED_SCXML_DEPTH}"),
                machine.start_tag_span,
            )
            .with_primary_message("nested machine validation depth budget exceeded")
            .with_help("reduce inline SCXML nesting or invoke an external source")
            .with_spec_reference("SCXML 1.0 §6.4 <invoke>"),
        );
        return report;
    }
    // Every rule reads the schema's value of a white-space-collapsing
    // attribute (XML Schema Part 2 §4.3.6), as lowering does. `LoweringRule`
    // walks the document as written, because its bounds measure the values
    // the compiler stores, which keep inline XML byte for byte; it reads
    // the schema value of every SCXML attribute it bounds or compares.
    let source = document;
    let document = &*crate::schema_values::schema_values(document);
    let rules: [&dyn Rule; 21] = [
        &RootRule,
        &IdRule,
        &EventDescriptorRule,
        &TransitionRule,
        &ParamRule,
        &CancelRule,
        &SendRule,
        &InvokeRule,
        &FinalizeRule,
        &DonedataRule,
        &ValueSourceRule,
        &InitialRule,
        &StateInitialRule,
        &LivenessRule,
        &crate::graph::GraphRule,
        &crate::datamodel_check::NullDataModelRule,
        &crate::datamodel_check::InPredicateRule,
        &crate::datamodel_check::EcmaScriptDataIdRule,
        &crate::datamodel_check::ReservedDataIdRule,
        &crate::script_check::ScriptSourceRule,
        &crate::text_direction::TextDirectionRule,
    ];
    let mut report = Report::new();
    for rule in rules {
        rule.check(document, &mut report);
    }
    crate::lowering::LoweringRule.check(source, &mut report);
    suppress_liveness_in_unreachable_states(document, &mut report);
    report.sort();
    report
}

/// Drops `LINT002`/`LINT003` findings inside a subtree `LINT005` already
/// reports as unreachable: a state that can never be entered cannot get stuck,
/// so those lints would only restate the one root cause.
fn suppress_liveness_in_unreachable_states(document: &Document, report: &mut Report) {
    let unreachable: BTreeSet<Span> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == "LINT005")
        .map(|diagnostic| diagnostic.primary.span)
        .collect();
    if unreachable.is_empty() {
        return;
    }
    let mut inside: BTreeSet<Span> = BTreeSet::new();
    let mut pending = vec![(&document.root, false)];
    while let Some((element, within)) = pending.pop() {
        let within = within
            || unreachable.contains(&element.start_tag_span)
            || state_id(element).is_some_and(|id| unreachable.contains(&id.value_span));
        if within {
            inside.insert(element.start_tag_span);
        }
        pending.extend(child_elements(element).map(|child| (child, within)));
    }
    report.retain(|diagnostic| {
        !(matches!(diagnostic.code.as_str(), "LINT002" | "LINT003")
            && inside.contains(&diagnostic.primary.span))
    });
}

struct RootRule;

impl Rule for RootRule {
    fn code(&self) -> &'static str {
        "SCXML001"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        let root = &document.root;
        if root.name.is(Some(SCXML_NAMESPACE), "scxml") {
            return;
        }
        let diagnostic = Diagnostic::error(
            self.code(),
            "root element must be `scxml` in the W3C SCXML namespace",
            root.start_tag_span,
        )
        .with_primary_message("this is not the W3C SCXML root")
        .with_spec_reference("SCXML 1.0 §3.2 <scxml>");
        let prefix = root.lexical_name.split_once(':').map(|(prefix, _)| prefix);
        let declaration = root
            .namespaces
            .iter()
            .find(|declaration| declaration.prefix.as_deref() == prefix);
        let diagnostic = match (
            root.name.local_name.as_str(),
            &root.name.namespace_uri,
            declaration,
        ) {
            // `<scxml>` with no namespace at all: the declaration is missing.
            ("scxml", None, None) if prefix.is_none() => diagnostic
                .with_primary_message("`<scxml>` has no namespace")
                .with_help(format!(
                    "declare the SCXML namespace: `<scxml xmlns=\"{SCXML_NAMESPACE}\">`"
                ))
                .with_fix(Fix::replace(
                    "declare the SCXML namespace",
                    Applicability::MachineApplicable,
                    {
                        let name = crate::spans::element_name_span(document, root);
                        Span::new(name.source, name.end, name.end)
                    },
                    format!(" xmlns=\"{SCXML_NAMESPACE}\""),
                )),
            // A near-miss URI (trailing slash, `https`, case): the value is
            // the responsible bytes, and one exact replacement repairs it.
            ("scxml", Some(uri), Some(declaration)) if is_near_scxml_namespace(uri) => {
                Diagnostic::error(
                    self.code(),
                    format!("`{uri}` is not the SCXML namespace"),
                    declaration.value_span,
                )
                .with_primary_message("near miss of the SCXML namespace URI")
                .with_secondary(
                    Label::new(root.start_tag_span)
                        .with_message("so this root is not the W3C SCXML root"),
                )
                .with_help(format!(
                    "the SCXML namespace is exactly `{SCXML_NAMESPACE}`"
                ))
                .with_fix(Fix::replace(
                    "use the exact SCXML namespace URI",
                    Applicability::MachineApplicable,
                    declaration.value_span,
                    SCXML_NAMESPACE,
                ))
                .with_spec_reference("SCXML 1.0 §3.2 <scxml>")
            }
            _ => diagnostic.with_help(format!(
                "use <scxml xmlns=\"{SCXML_NAMESPACE}\"> or an equivalent prefix"
            )),
        };
        report.push(diagnostic);
    }
}

/// Whether `uri` is a near miss of the exact SCXML namespace URI (a trailing
/// slash, `https`, or different letter case). Lookup never rewrites it; this
/// only decides which diagnostic owns the repair.
pub(crate) fn is_near_scxml_namespace(uri: &str) -> bool {
    uri != SCXML_NAMESPACE
        && uri
            .trim_end_matches('/')
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .eq_ignore_ascii_case("www.w3.org/2005/07/scxml")
}

struct IdRule;

impl Rule for IdRule {
    fn code(&self) -> &'static str {
        "SCXML002"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            let mut ids: BTreeMap<String, Span> = BTreeMap::new();
            let ecmascript = crate::datamodel_check::machine_data_model(machine).0 == "ecmascript";
            let data_ids = if ecmascript {
                declared_data_ids(machine)
            } else {
                BTreeSet::new()
            };
            visit_machine(machine, &mut |element| {
                if element.name.namespace_uri.as_deref() != Some(SCXML_NAMESPACE) {
                    return;
                }
                let Some(id) = element.attribute("id") else {
                    return;
                };
                // An ECMAScript `<data id>` is also a variable name, so its
                // NCName repair (`$price`, `max tries`) must stay one.
                let ecmascript_data = ecmascript && is_scxml(element, "data");
                if !is_ncname(&id.value) && ecmascript_data {
                    report.push(ecmascript_data_ncname_diagnostic(
                        document, machine, id, &data_ids,
                    ));
                } else if !is_ncname(&id.value) {
                    let suggestion = ncname_suggestion(&id.value);
                    let references = id_reference_spans(document, machine, &id.value);
                    let mut diagnostic = Diagnostic::error(
                        "SCXML003",
                        format!("identifier `{}` is not an XML NCName", id.value),
                        id.value_span,
                    )
                    .with_primary_message("invalid NCName")
                    .with_help(
                        "start with a letter or underscore and use only NCName characters; colons are not allowed",
                    )
                    .with_spec_reference("SCXML 1.0 §3.14 IDs");
                    for reference in &references {
                        diagnostic = diagnostic
                            .with_secondary(Label::new(*reference).with_message("referenced here"));
                    }
                    if let Some(suggestion) = suggestion {
                        let message = if references.is_empty() {
                            format!("replace with the valid NCName `{suggestion}`")
                        } else {
                            match references.len() {
                                1 => format!(
                                    "rename to the valid NCName `{suggestion}` and update its reference"
                                ),
                                count => format!(
                                    "rename to the valid NCName `{suggestion}` and update its {count} references"
                                ),
                            }
                        };
                        let mut spans: Vec<Span> = std::iter::once(id.value_span)
                            .chain(references.iter().copied())
                            .collect();
                        spans.sort();
                        let edits = spans
                            .into_iter()
                            .map(|span| FixEdit {
                                span,
                                replacement: suggestion.clone(),
                            })
                            .collect();
                        diagnostic = diagnostic.with_fix(Fix {
                            message,
                            applicability: Applicability::MaybeIncorrect,
                            edits,
                        });
                    }
                    report.push(diagnostic);
                }
                if let Some(first) = ids.insert(id.value.clone(), id.value_span) {
                    report.push(
                        Diagnostic::error(
                            self.code(),
                            format!("duplicate identifier `{}`", id.value),
                            id.value_span,
                        )
                        .with_primary_message("duplicate declaration")
                        .with_secondary(Label::new(first).with_message("first declared here"))
                        .with_help("choose a unique identifier and update every reference to it")
                        .with_spec_reference("SCXML 1.0 §3.14 IDs"),
                    );
                }
            });
        }
    }
}

/// Every `<data id>` value of `machine`.
fn declared_data_ids(machine: &Element) -> BTreeSet<&str> {
    let mut ids = BTreeSet::new();
    visit_machine(machine, &mut |element| {
        if is_scxml(element, "data")
            && let Some(id) = element.attribute("id")
        {
            ids.insert(id.value.as_str());
        }
    });
    ids
}

/// `SCXML003` for an ECMAScript `<data id>` the data model can declare but
/// XML cannot (`$price`): the repair renames the variable everywhere.
fn ecmascript_data_ncname_diagnostic(
    document: &Document,
    machine: &Element,
    id: &Attribute,
    declared: &BTreeSet<&str>,
) -> Diagnostic {
    let diagnostic = Diagnostic::error(
        "SCXML003",
        format!("identifier `{}` is not an XML NCName", id.value),
        id.value_span,
    )
    .with_primary_message("invalid NCName")
    .with_help(
        "a `<data id>` is both an XML ID and an ECMAScript variable name: use ASCII letters, digits, and `_`, starting with a letter",
    )
    .with_spec_reference("SCXML 1.0 §3.14 IDs");
    match crate::datamodel_check::data_ids::ncname_rename_fix(document, machine, id, declared) {
        Some(fix) => diagnostic.with_fix(fix),
        None => diagnostic,
    }
}

struct EventDescriptorRule;

impl Rule for EventDescriptorRule {
    fn code(&self) -> &'static str {
        "SCXML013"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "transition") {
                return;
            }
            let Some(event) = element.attribute("event") else {
                return;
            };
            let value = event.value.trim_matches(is_xml_space);
            if value.is_empty() {
                report.push(
                    Diagnostic::error(
                        self.code(),
                        "transition `event` must contain at least one event descriptor",
                        event.value_span,
                    )
                    .with_primary_message("empty event descriptor list")
                    .with_help("remove the attribute for an eventless transition, or add one or more space-separated event descriptors")
                    .with_fix(Fix::replace(
                        "remove the empty `event` attribute",
                        Applicability::MaybeIncorrect,
                        attribute_removal_span(document, event),
                        "",
                    ))
                    .with_spec_reference("SCXML 1.0 §3.12.1 Event Descriptors"),
                );
                return;
            }
            if value.contains(',') {
                let replacement = value
                    .split(',')
                    .map(|token| token.trim_matches(is_xml_space))
                    .filter(|token| !token.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut diagnostic = Diagnostic::error(
                    self.code(),
                    "event descriptors are whitespace-separated, not comma-separated",
                    event.value_span,
                )
                .with_primary_message("comma in event descriptor list")
                .with_help("replace commas with spaces; each descriptor is matched independently")
                .with_spec_reference("SCXML 1.0 §3.12.1 Event Descriptors");
                if !replacement.is_empty() {
                    diagnostic = diagnostic.with_fix(Fix::replace(
                        "replace commas with spaces",
                        Applicability::MachineApplicable,
                        event.value_span,
                        replacement,
                    ));
                }
                report.push(diagnostic);
                return;
            }
            for (token, token_span) in attribute_token_spans(document, event) {
                if !valid_event_descriptor(&token) {
                    report.push(
                        Diagnostic::error(
                            self.code(),
                            format!("event descriptor `{token}` contains an invalid character"),
                            token_span,
                        )
                        .with_primary_message("invalid event descriptor")
                        .with_help("use XML name-style event descriptors containing letters, digits, `_`, `-`, `:`, and `.`; `*` and `.*` are wildcard descriptors")
                        .with_spec_reference("SCXML 1.0 §3.12.1 Event Descriptors"),
                    );
                }
            }
        });
    }
}

struct TransitionRule;

impl Rule for TransitionRule {
    fn code(&self) -> &'static str {
        "SCXML004"
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one pass resolves every target and initial reference of a machine against a single ID table and shared typo budget"
    )]
    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            let ids = collect_ids(machine);
            let mut typo_budget = MAX_TYPO_SUGGESTION_MACHINE_CELLS;
            visit_machine(machine, &mut |element| {
                if !is_scxml(element, "transition") {
                    return;
                }
                // SCXML 1.0 §3.5.2: "A conformant SCXML document MUST
                // specify at least one of 'event', 'cond' or 'target'."
                // Executable content does not count: without them the
                // transition would be selected again in every microstep.
                if ["event", "cond", "target"]
                    .iter()
                    .all(|name| element.attribute(name).is_none())
                    && !parent_is(machine, element, "initial")
                    && !parent_is(machine, element, "history")
                {
                    report.push(
                        Diagnostic::error(
                            self.code(),
                            "transition must have at least one of `event`, `cond`, or `target`",
                            element.start_tag_span,
                        )
                        .with_primary_message("transition has no trigger, guard, or target")
                        .with_help(
                            "add the event that triggers it, a condition, or the target state",
                        )
                        .with_fix(Fix::replace(
                            "add the event that triggers the transition",
                            Applicability::HasPlaceholders,
                            attribute_insertion_point(document, element),
                            " event=\"…\"",
                        ))
                        .with_spec_reference("SCXML 1.0 §3.5 <transition>"),
                    );
                }
                let Some(target) = element.attribute("target") else {
                    return;
                };
                if xml_tokens(&target.value).next().is_none() {
                    report.push(
                        Diagnostic::error(
                            "SCXML005",
                            "transition `target` must contain at least one state identifier",
                            target.value_span,
                        )
                        .with_primary_message("empty target list")
                        .with_help("remove the attribute for a targetless transition, or provide space-separated state IDs")
                        .with_spec_reference("SCXML 1.0 §3.5 <transition>"),
                    );
                }
                for (token, token_span) in attribute_token_spans(document, target) {
                    if !ids.contains_key(token.as_str()) {
                        if let Some(diagnostic) =
                            comma_separated_targets(document, &token, token_span, &ids)
                        {
                            report.push(diagnostic);
                            continue;
                        }
                        if let Some(diagnostic) = split_id_diagnostic(
                            document,
                            "SCXML005",
                            "transition target",
                            target,
                            &token,
                            token_span,
                            &ids,
                        ) {
                            report.push(diagnostic);
                            continue;
                        }
                        let suggestion = nearest_within(
                            &token,
                            ids.keys().map(String::as_str),
                            &mut typo_budget,
                        );
                        let mut diagnostic = Diagnostic::error(
                            "SCXML005",
                            format!("transition target `{token}` does not exist"),
                            token_span,
                        )
                        .with_primary_message("unknown target identifier")
                        .with_help("declare the target state or correct the target identifier")
                        .with_spec_reference("SCXML 1.0 §3.5 <transition>");
                        if let Some(suggestion) = suggestion
                            && let Some(suggestion_span) = ids.get(suggestion)
                        {
                            diagnostic = diagnostic
                                .with_secondary(
                                    Label::new(*suggestion_span)
                                        .with_message("similarly named state"),
                                )
                                .with_help(format!("did you mean `{suggestion}`?"))
                                .with_fix(Fix::replace(
                                    format!("a state with a similar name exists: `{suggestion}`"),
                                    Applicability::MaybeIncorrect,
                                    token_span,
                                    suggestion,
                                ));
                        }
                        report.push(diagnostic);
                    }
                }
            });
            for owner in machine_elements(machine) {
                if !is_scxml(owner, "scxml") && !is_scxml(owner, "state") {
                    continue;
                }
                let Some(initial) = owner.attribute("initial") else {
                    continue;
                };
                if xml_tokens(&initial.value).next().is_none() {
                    if let Some(diagnostic) = empty_initial_diagnostic(owner, initial) {
                        report.push(diagnostic);
                    }
                    continue;
                }
                for (token, token_span) in attribute_token_spans(document, initial) {
                    let Some(target_span) = ids.get(token.as_str()) else {
                        if let Some(diagnostic) = split_id_diagnostic(
                            document,
                            "SCXML006",
                            "initial state",
                            initial,
                            &token,
                            token_span,
                            &ids,
                        ) {
                            report.push(diagnostic);
                            continue;
                        }
                        let suggestion = nearest_within(
                            &token,
                            ids.keys().map(String::as_str),
                            &mut typo_budget,
                        );
                        let mut diagnostic = Diagnostic::error(
                            "SCXML006",
                            format!("initial state `{token}` does not exist"),
                            token_span,
                        )
                        .with_primary_message("unknown initial state")
                        .with_help("declare the initial state or correct the identifier")
                        .with_spec_reference(if is_scxml(owner, "scxml") {
                            "SCXML 1.0 §3.2 <scxml>"
                        } else {
                            "SCXML 1.0 §3.3 <state>"
                        });
                        if let Some(suggestion) = suggestion {
                            if let Some(suggestion_span) = ids.get(suggestion) {
                                diagnostic = diagnostic.with_secondary(
                                    Label::new(*suggestion_span)
                                        .with_message("similarly named state"),
                                );
                            }
                            diagnostic = diagnostic
                                .with_help(format!("did you mean `{suggestion}`?"))
                                .with_fix(Fix::replace(
                                    format!("a state with a similar name exists: `{suggestion}`"),
                                    Applicability::MaybeIncorrect,
                                    token_span,
                                    suggestion,
                                ));
                        }
                        report.push(diagnostic);
                        continue;
                    };
                    if !is_descendant(machine, owner, *target_span) {
                        report.push(
                            Diagnostic::error(
                                "SCXML017",
                                format!(
                                    "initial state `{token}` is not a descendant of this element"
                                ),
                                token_span,
                            )
                            .with_primary_message("initial target escapes its containing state")
                            .with_secondary(
                                Label::new(*target_span).with_message("target declared here"),
                            )
                            .with_help(
                                "target a descendant state owned by this compound state or SCXML root",
                            )
                            .with_spec_reference(if is_scxml(owner, "scxml") {
                                "SCXML 1.0 §3.2 <scxml>"
                            } else {
                                "SCXML 1.0 §3.3 <state>"
                            }),
                        );
                    }
                }
            }
        }
    }
}

/// SCXML005/SCXML006 for a token that is one piece of a declared ID with
/// whitespace: the reference is correct, the declaration is the error.
/// `target="r1b,r2b"`: every comma-separated piece names a state, so the
/// commas were meant as the whitespace that separates target IDs.
/// `SCXML006` for an `initial` attribute that names no state at all (empty
/// or only whitespace) on an `<scxml>` or a compound `<state>`; the
/// compiler has no state to enter. An atomic state's `initial` is
/// `SCXML012` instead. The repair names the child state entered today by
/// document order.
fn empty_initial_diagnostic(owner: &Element, initial: &Attribute) -> Option<Diagnostic> {
    let first_child = child_elements(owner).find(|child| {
        is_scxml(child, "state") || is_scxml(child, "parallel") || is_scxml(child, "final")
    });
    if is_scxml(owner, "state") && first_child.is_none() {
        return None;
    }
    let element = owner.name.local_name.as_str();
    let diagnostic = Diagnostic::error(
        "SCXML006",
        format!("`initial` on `<{element}>` names no state"),
        initial.span,
    )
    .with_primary_message("empty initial state list")
    .with_note("an `initial` attribute lists the states entered first; an empty list enters nothing, so the compiler rejects it (omit the attribute to enter the first child state)")
    .with_help("name the initial state, or remove `initial` to enter the first child state in document order")
    .with_spec_reference(if is_scxml(owner, "scxml") {
        "SCXML 1.0 §3.2 <scxml>"
    } else {
        "SCXML 1.0 §3.3 <state>"
    });
    Some(match first_child.and_then(|child| child.attribute("id")) {
        Some(id) => diagnostic.with_fix(Fix::replace(
            format!("enter `{}`, the first child state", id.value),
            Applicability::MaybeIncorrect,
            initial.value_span,
            id.value.clone(),
        )),
        None => diagnostic,
    })
}

fn comma_separated_targets(
    document: &Document,
    token: &str,
    token_span: Span,
    ids: &BTreeMap<String, Span>,
) -> Option<Diagnostic> {
    if !token.contains(',') {
        return None;
    }
    let pieces: Vec<&str> = token
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect();
    if pieces.is_empty() || !pieces.iter().all(|piece| ids.contains_key(*piece)) {
        return None;
    }
    let joined = pieces.join(" ");
    let diagnostic = Diagnostic::error(
        "SCXML005",
        format!("transition targets are whitespace-separated, not comma-separated: `{token}` is not one state ID"),
        token_span,
    )
    .with_primary_message("comma-separated targets")
    .with_note("`target` is a space-separated list of state IDs; a comma makes the list one ID that no state declares")
    .with_help(format!("separate the target IDs with spaces: `{joined}`"))
    .with_spec_reference("SCXML 1.0 §3.5 <transition>");
    // Only an exact source token (no character references) is rewritten.
    Some(if document.source.slice(token_span) == Some(token) {
        diagnostic.with_fix(Fix::replace(
            "separate the targets with spaces",
            Applicability::MachineApplicable,
            token_span,
            joined,
        ))
    } else {
        diagnostic
    })
}

fn split_id_diagnostic(
    document: &Document,
    code: &str,
    noun: &str,
    attribute: &Attribute,
    token: &str,
    token_span: Span,
    ids: &BTreeMap<String, Span>,
) -> Option<Diagnostic> {
    let (id, declaration) = covering_spaced_id(document, attribute, token_span, ids)?;
    Some(
        Diagnostic::error(code, format!("{noun} `{token}` does not exist"), token_span)
            .with_primary_message(format!("one piece of the reference `{id}`"))
            .with_secondary(
                Label::new(declaration)
                    .with_message("declared here as a single ID containing whitespace"),
            )
            .with_note(format!(
                "ID references are whitespace-separated lists, so `{id}` is read as separate IDs; the root cause is the invalid ID declaration (SCXML003)"
            ))
            .with_help("rename the declaration to a valid NCName and update this reference; the SCXML003 suggestion does both")
            .with_spec_reference("SCXML 1.0 §3.14 IDs"),
    )
}

struct ParamRule;

impl Rule for ParamRule {
    fn code(&self) -> &'static str {
        "SCXML007"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "param") {
                return;
            }
            let name = element.attribute("name");
            let expr = element.attribute("expr");
            let location = element.attribute("location");
            if name.is_none() {
                report.push(
                    Diagnostic::error(
                        self.code(),
                        "param requires a `name` attribute",
                        element.start_tag_span,
                    )
                    .with_primary_message("missing required name")
                    .with_help("add `name` with the parameter name")
                    .with_fix(Fix::replace(
                        "add the required `name` attribute",
                        Applicability::HasPlaceholders,
                        attribute_insertion_point(document, element),
                        " name=\"…\"",
                    ))
                    .with_spec_reference("SCXML 1.0 §5.7 <param>"),
                );
            }
            match (expr, location) {
                (None, None) => report.push(
                    Diagnostic::error(
                        self.code(),
                        "param requires exactly one of `expr` or `location`",
                        element.start_tag_span,
                    )
                    .with_primary_message("no value source")
                    .with_help(
                        "use `expr` for a value expression or `location` for a data-model location",
                    )
                    .with_fix(Fix::replace(
                        "add an `expr` value source",
                        Applicability::HasPlaceholders,
                        attribute_insertion_point(document, element),
                        " expr=\"…\"",
                    ))
                    .with_spec_reference("SCXML 1.0 §5.7 <param>"),
                ),
                (Some(expr), Some(location)) => report.push(
                    Diagnostic::error(
                        self.code(),
                        "param cannot use both `expr` and `location`",
                        expr.span,
                    )
                    .with_primary_message("mutually exclusive attribute")
                    .with_secondary(Label::new(location.span).with_message("conflicting attribute"))
                    .with_help("keep exactly one value source")
                    .with_fix(Fix::replace(
                        "keep `expr` and remove `location`",
                        Applicability::MaybeIncorrect,
                        attribute_removal_span(document, location),
                        "",
                    ))
                    .with_spec_reference("SCXML 1.0 §5.7 <param>"),
                ),
                _ => {}
            }
        });
    }
}

struct CancelRule;

impl Rule for CancelRule {
    fn code(&self) -> &'static str {
        "SCXML008"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "cancel") {
                return;
            }
            let sendid = element.attribute("sendid");
            let sendidexpr = element.attribute("sendidexpr");
            match (sendid, sendidexpr) {
                (None, None) => report.push(
                    Diagnostic::error(
                        self.code(),
                        "cancel requires exactly one of `sendid` or `sendidexpr`",
                        element.start_tag_span,
                    )
                    .with_primary_message("missing send identifier")
                    .with_help(
                        "use `sendid` for a literal send ID or `sendidexpr` for a computed ID",
                    )
                    .with_fix(Fix::replace(
                        "add the `sendid` of the `<send>` to cancel",
                        Applicability::HasPlaceholders,
                        attribute_insertion_point(document, element),
                        " sendid=\"…\"",
                    ))
                    .with_spec_reference("SCXML 1.0 §6.3 <cancel>"),
                ),
                (Some(sendid), Some(sendidexpr)) => report.push(
                    Diagnostic::error(
                        self.code(),
                        "cancel cannot use both `sendid` and `sendidexpr`",
                        sendid.span,
                    )
                    .with_primary_message("mutually exclusive attribute")
                    .with_secondary(
                        Label::new(sendidexpr.span).with_message("conflicting attribute"),
                    )
                    .with_help("keep exactly one send identifier source")
                    .with_fix(Fix::replace(
                        "keep `sendid` and remove `sendidexpr`",
                        Applicability::MaybeIncorrect,
                        attribute_removal_span(document, sendidexpr),
                        "",
                    ))
                    .with_spec_reference("SCXML 1.0 §6.3 <cancel>"),
                ),
                _ => {}
            }
        });
    }
}

/// Literal/computed attribute pairs of `<send>` (SCXML 1.0 §6.2).
const SEND_EXCLUSIVE_ATTRIBUTES: [XorRule; 5] = [
    XorRule {
        code: "SCXML014",
        left: "event",
        right: "eventexpr",
        subject: "send event name",
        spec: "SCXML 1.0 §6.2 <send>",
    },
    XorRule {
        code: "SCXML014",
        left: "target",
        right: "targetexpr",
        subject: "send target",
        spec: "SCXML 1.0 §6.2 <send>",
    },
    XorRule {
        code: "SCXML014",
        left: "type",
        right: "typeexpr",
        subject: "send processor type",
        spec: "SCXML 1.0 §6.2 <send>",
    },
    XorRule {
        code: "SCXML014",
        left: "delay",
        right: "delayexpr",
        subject: "send delay",
        spec: "SCXML 1.0 §6.2 <send>",
    },
    XorRule {
        code: "SCXML014",
        left: "id",
        right: "idlocation",
        subject: "send ID",
        spec: "SCXML 1.0 §6.2 <send>",
    },
];

struct SendRule;

impl Rule for SendRule {
    fn code(&self) -> &'static str {
        "SCXML014"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "send") {
                return;
            }
            for rule in &SEND_EXCLUSIVE_ATTRIBUTES {
                check_attribute_xor(document, report, element, rule);
            }
            check_send_event_name(document, report, element);
            check_send_internal_delay(document, report, element);
            let content = first_child(element, "content");
            if let Some(content) = content {
                if let Some(namelist) = element.attribute("namelist") {
                    report.push(
                        Diagnostic::error(
                            "SCXML015",
                            "send with `content` cannot also use `namelist`",
                            namelist.span,
                        )
                        .with_primary_message("namelist conflicts with content")
                        .with_secondary(
                            Label::new(content.start_tag_span)
                                .with_message("content declared here"),
                        )
                        .with_help("use `content` or `namelist`, not both")
                        .with_spec_reference("SCXML 1.0 §6.2 <send>"),
                    );
                }
                if let Some(param) = first_child(element, "param") {
                    report.push(
                        Diagnostic::error(
                            "SCXML015",
                            "send with `content` cannot also contain `param`",
                            content.start_tag_span,
                        )
                        .with_primary_message("content conflicts with param data")
                        .with_secondary(
                            Label::new(param.start_tag_span).with_message("param declared here"),
                        )
                        .with_help("use `content` or `param` children, not both")
                        .with_spec_reference("SCXML 1.0 §6.2 <send>"),
                    );
                }
            }
        });
    }
}

/// `SCXML055`: `<finalize>` executable content that raises events or
/// invokes external actions (SCXML 1.0 §6.5 names `<send>` and `<raise>`).
struct FinalizeRule;

impl Rule for FinalizeRule {
    fn code(&self) -> &'static str {
        "SCXML055"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |finalize| {
            if !is_scxml(finalize, "finalize") {
                return;
            }
            // Depth-first over the finalize block (`<if>`/`<foreach>` bodies
            // included); a reported element's own subtree (a `<send>`'s
            // `<content>`) is not searched again.
            let mut pending: Vec<&Element> = child_elements(finalize).collect();
            pending.reverse();
            while let Some(element) = pending.pop() {
                let name = element.name.local_name.as_str();
                if is_scxml(element, "raise") || is_scxml(element, "send") {
                    report.push(finalize_diagnostic(
                        self.code(),
                        document,
                        finalize,
                        element,
                        name,
                    ));
                    continue;
                }
                let mut children: Vec<&Element> = child_elements(element).collect();
                children.reverse();
                pending.extend(children);
            }
        });
    }
}

fn finalize_diagnostic(
    code: &str,
    document: &Document,
    finalize: &Element,
    element: &Element,
    name: &str,
) -> Diagnostic {
    let action = if name == "raise" {
        "raises an event"
    } else {
        "sends an event"
    };
    Diagnostic::error(
        code,
        format!("`<{name}>` inside `<finalize>` {action}, which `<finalize>` must not do"),
        element.start_tag_span,
    )
    .with_primary_message(format!("`<{name}>` is not allowed here"))
    .with_secondary(
        Label::new(finalize.start_tag_span)
            .with_message("inside this `<finalize>` of an `<invoke>`"),
    )
    .with_note("`<finalize>` runs while an event returned by the invoked service is being processed, before transitions are selected, to update the data model from `_event`; SCXML forbids it to raise events or invoke external actions, naming `<send>` and `<raise>` explicitly")
    .with_help(format!(
        "keep only data-model updates (`<assign>`, `<log>`, `<if>`, `<foreach>`) in `<finalize>`; to react to the returned event, move the `<{name}>` into a `<transition>` that matches it"
    ))
    .with_fix(Fix::replace(
        format!("remove the `<{name}>` element"),
        Applicability::MaybeIncorrect,
        crate::graph::removal_span(document, element.span),
        "",
    ))
    .with_spec_reference("SCXML 1.0 §6.5 <finalize>")
}

struct InvokeRule;

impl Rule for InvokeRule {
    fn code(&self) -> &'static str {
        "SCXML016"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "invoke") {
                return;
            }
            check_invoke_namelist_with_param(document, report, element);
            check_attribute_xor(
                document,
                report,
                element,
                &XorRule {
                    code: self.code(),
                    left: "type",
                    right: "typeexpr",
                    subject: "invoke type",
                    spec: "SCXML 1.0 §6.4 <invoke>",
                },
            );
            check_attribute_xor(
                document,
                report,
                element,
                &XorRule {
                    code: self.code(),
                    left: "id",
                    right: "idlocation",
                    subject: "invoke ID",
                    spec: "SCXML 1.0 §6.4 <invoke>",
                },
            );
            let content = first_child(element, "content");
            if let (Some(src), Some(srcexpr), Some(content)) = (
                element.attribute("src"),
                element.attribute("srcexpr"),
                content,
            ) {
                // One mistake (too many sources), one diagnostic: pairwise
                // reports would label the same bytes three times.
                report.push(
                    Diagnostic::error(
                        self.code(),
                        "invoke declares three sources: `src`, `srcexpr`, and inline `<content>`",
                        src.span,
                    )
                    .with_primary_message("first source")
                    .with_secondary(Label::new(srcexpr.span).with_message("second source"))
                    .with_secondary(Label::new(content.start_tag_span).with_message("third source"))
                    .with_help("keep exactly one of `src`, `srcexpr`, or `<content>`")
                    .with_spec_reference("SCXML 1.0 §6.4 <invoke>"),
                );
                return;
            }
            check_attribute_xor(
                document,
                report,
                element,
                &XorRule {
                    code: self.code(),
                    left: "src",
                    right: "srcexpr",
                    subject: "invoke source",
                    spec: "SCXML 1.0 §6.4 <invoke>",
                },
            );
            if let Some(content) = content {
                for attr_name in ["src", "srcexpr"] {
                    if let Some(attribute) = element.attribute(attr_name) {
                        report.push(
                            Diagnostic::error(
                                self.code(),
                                format!("invoke with `content` cannot also use `{attr_name}`"),
                                attribute.span,
                            )
                            .with_primary_message("external source conflicts with inline content")
                            .with_secondary(
                                Label::new(content.start_tag_span)
                                    .with_message("content declared here"),
                            )
                            .with_help("use an external source or inline content, not both")
                            .with_spec_reference("SCXML 1.0 §6.4 <invoke>"),
                        );
                    }
                }
            }
        });
    }
}

/// SCXML071: an `<invoke>` with both a `namelist` and `<param>` children.
/// SCXML 1.0 §6.4.1 constrains `namelist`: "Must not occur with the <param>
/// element." The XSD allows the combination, so only this rule catches it.
/// The fix passes each namelist location as a `<param>` whose `name` and
/// `location` are that location, which is what §6.4.1 says the namelist
/// passes: "(The name of the location is the attribute and the value stored
/// at the location is the value.)" It is maybe-incorrect because the
/// passed values change order (§6.4.3 injects them by name either way).
fn check_invoke_namelist_with_param(document: &Document, report: &mut Report, element: &Element) {
    let (Some(namelist), Some(param)) =
        (element.attribute("namelist"), first_child(element, "param"))
    else {
        return;
    };
    let mut params = String::new();
    for location in namelist
        .value
        .split([' ', '\t', '\r', '\n'])
        .filter(|location| !location.is_empty())
    {
        let location = escape_attribute_value(location);
        params.push_str(r#"<param name=""#);
        params.push_str(&location);
        params.push_str(r#"" location=""#);
        params.push_str(&location);
        params.push_str(r#""/>"#);
    }
    let insertion = Span::new(
        element.start_tag_span.source,
        element.start_tag_span.end,
        element.start_tag_span.end,
    );
    report.push(
        Diagnostic::error(
            "SCXML071",
            "an `<invoke>` with `<param>` children cannot also use `namelist`",
            namelist.span,
        )
        .with_primary_message("namelist conflicts with the param children")
        .with_secondary(Label::new(param.start_tag_span).with_message("param declared here"))
        .with_note("`namelist` and `<param>` both pass data to the invoked service; SCXML allows only one of them on an `<invoke>`")
        .with_help("pass each namelist location as a `<param>` with a `location`")
        .with_fix(Fix {
            message: "pass the namelist locations as `<param>` children".to_owned(),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![
                FixEdit {
                    span: attribute_removal_span(document, namelist),
                    replacement: String::new(),
                },
                FixEdit {
                    span: insertion,
                    replacement: params,
                },
            ],
        })
        .with_spec_reference("SCXML 1.0 §6.4 <invoke>"),
    );
}

/// Escapes a value for a double-quoted XML attribute.
fn escape_attribute_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '"' => escaped.push_str("&quot;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// The SCXML Event I/O Processor type URI (SCXML 1.0 Appendix C.1).
const SCXML_EVENT_PROCESSOR: &str = "http://www.w3.org/TR/scxml/#SCXMLEventProcessor";

/// SCXML052: a `<send>` handled by the SCXML Event I/O Processor (no `type`,
/// or exactly its type URI) must name its event with `event` or `eventexpr`.
fn check_send_event_name(document: &Document, report: &mut Report, element: &Element) {
    if element.attribute("event").is_some()
        || element.attribute("eventexpr").is_some()
        || element.attribute("typeexpr").is_some()
    {
        return;
    }
    let explicit_type = element.attribute("type");
    if explicit_type.is_some_and(|kind| kind.value != SCXML_EVENT_PROCESSOR) {
        return;
    }
    let mut diagnostic = Diagnostic::error(
        "SCXML052",
        "`<send>` to the SCXML event processor names no event",
        element.start_tag_span,
    )
    .with_primary_message("neither `event` nor `eventexpr` is present")
    .with_note("the SCXML Event I/O Processor delivers a named event, so a `<send>` it handles must have `event` or `eventexpr`; without them the send raises `error.execution` instead of delivering anything")
    .with_help("add `event=\"name\"` with the event to deliver, or `eventexpr` to compute it")
    .with_fix(Fix::replace(
        "add the `event` to deliver",
        Applicability::HasPlaceholders,
        attribute_insertion_point(document, element),
        " event=\"…\"",
    ))
    .with_spec_reference("SCXML 1.0 §6.2 <send>")
    .with_spec_reference("SCXML 1.0 §C.1 SCXML Event I/O Processor");
    if let Some(kind) = explicit_type {
        diagnostic = diagnostic.with_secondary(
            Label::new(kind.span).with_message("the SCXML event processor is selected here"),
        );
    }
    report.push(diagnostic);
}

/// SCXML070: a `<send>` whose literal `target` is the internal queue
/// (`#_internal`) with a `delay` or `delayexpr`. SCXML 1.0 §6.2 constrains
/// both attributes: "Must not occur with 'delayexpr' or when the attribute
/// 'target' has the value "_internal"" (the special target §C.1 names
/// '#_internal'), so the document is non-conformant; at run time the
/// processor raises `error.execution` for such a send (§6.2.4). A computed
/// `targetexpr` is not judged.
fn check_send_internal_delay(document: &Document, report: &mut Report, element: &Element) {
    let Some(target) = element
        .attribute("target")
        .filter(|target| target.value == "#_internal")
    else {
        return;
    };
    for name in ["delay", "delayexpr"] {
        let Some(delay) = element.attribute(name) else {
            continue;
        };
        report.push(
            Diagnostic::error(
                "SCXML070",
                format!("a `<send>` to `#_internal` cannot use `{name}`"),
                delay.span,
            )
            .with_primary_message("a delay is not allowed here")
            .with_secondary(
                Label::new(target.span).with_message("the send targets the internal event queue"),
            )
            .with_note("an event sent to `#_internal` joins the internal event queue at once; SCXML forbids a delay on it, and the processor raises `error.execution` instead of delivering the event")
            .with_help(format!("remove `{name}` to raise the event now, or drop the `target` to deliver it after the delay on the session's own external queue"))
            .with_fix(Fix::replace(
                format!("remove `{name}`"),
                Applicability::MaybeIncorrect,
                attribute_removal_span(document, delay),
                "",
            ))
            .with_spec_reference("SCXML 1.0 §6.2 <send>"),
        );
    }
}

/// Span of the first child element (the whole element) or non-whitespace
/// character data.
fn first_child_content(element: &Element) -> Option<Span> {
    element.children.iter().find_map(|child| match child {
        Node::Element(child) => Some(child.span),
        Node::Text(text) => {
            let value = text.value.trim_matches([' ', '\t', '\r', '\n']);
            (!value.is_empty()).then_some(text.span)
        }
    })
}

/// SCXML053: `<data>` takes its value from exactly one of `src`, `expr`, or
/// child content, and `<content expr>` must have no child content.
struct ValueSourceRule;

impl Rule for ValueSourceRule {
    fn code(&self) -> &'static str {
        "SCXML053"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            let (spec, candidates): (&str, &[&str]) = if is_scxml(element, "data") {
                ("SCXML 1.0 §5.3 <data>", &["expr", "src"])
            } else if is_scxml(element, "content") {
                ("SCXML 1.0 §5.6 <content>", &["expr"])
            } else if is_scxml(element, "assign") {
                ("SCXML 1.0 §5.4 <assign>", &["expr"])
            } else {
                return;
            };
            let name = element.name.local_name.as_str();
            let mut sources: Vec<(String, Span, Option<&Attribute>)> = candidates
                .iter()
                .filter_map(|candidate| element.attribute(candidate))
                .map(|attribute| (format!("`{}`", attribute.name.local_name), attribute.span, Some(attribute)))
                .collect();
            if let Some(content) = first_child_content(element) {
                sources.push(("the child content".to_owned(), content, None));
            }
            let [first, second, rest @ ..] = sources.as_slice() else {
                return;
            };
            let listed = if rest.is_empty() {
                format!("{} and {}", first.0, second.0)
            } else {
                let (last, leading) = sources.split_last().unwrap_or((second, &[]));
                let leading = leading
                    .iter()
                    .map(|(label, _, _)| label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{leading}, and {}", last.0)
            };
            let mut diagnostic = Diagnostic::error(
                self.code(),
                format!("`<{name}>` declares more than one value source: {listed}"),
                second.1,
            )
            .with_secondary(Label::new(first.1).with_message(format!("{} is declared here", first.0)));
            // An element child is labeled whole, even when it is
            // self-closing (its span then equals its start tag).
            diagnostic.primary = Label::whole_element(second.1)
                .with_message(format!("{} conflicts with {}", second.0, first.0));
            for (label, span, _) in rest {
                diagnostic = diagnostic
                    .with_secondary(Label::whole_element(*span).with_message(format!("{label} is declared here too")));
            }
            // Keep child content or the first attribute; the removal of an
            // optional attribute always leaves a valid element.
            let removable: Vec<&Attribute> = if sources.iter().any(|(_, _, attribute)| attribute.is_none()) {
                sources.iter().filter_map(|(_, _, attribute)| *attribute).collect()
            } else {
                sources.iter().skip(1).filter_map(|(_, _, attribute)| *attribute).collect()
            };
            let removed = removable
                .iter()
                .map(|attribute| format!("`{}`", attribute.name.local_name))
                .collect::<Vec<_>>()
                .join(" and ");
            report.push(
                diagnostic
                    .with_note(match name {
                        "data" => "`<data>` gets its initial value from exactly one of `src` (a URI), `expr` (an expression), or inline child content; with several the value is ambiguous",
                        "assign" => "`<assign>` takes the value it stores from exactly one of `expr` or inline child content; with both the value is ambiguous",
                        _ => "`<content>` is either computed by `expr` or given literally as child content, never both",
                    })
                    .with_help(match sources.as_slice() {
                        [_, _] => {
                            let kept = sources
                                .iter()
                                .filter(|(_, _, attribute)| {
                                    !attribute.is_some_and(|attribute| removable.iter().any(|removed| std::ptr::eq(*removed, attribute)))
                                })
                                .map(|(label, _, _)| label.as_str())
                                .collect::<Vec<_>>()
                                .join(" and ");
                            format!("keep one value source: remove {removed}, or remove {kept}")
                        }
                        _ => format!(
                            "keep one value source and remove the other {}: for example, remove {removed}",
                            sources.len() - 1
                        ),
                    })
                    .with_fix(Fix {
                        message: format!("remove {removed}"),
                        applicability: Applicability::MaybeIncorrect,
                        edits: removable
                            .iter()
                            .map(|attribute| FixEdit {
                                span: attribute_removal_span(document, attribute),
                                replacement: String::new(),
                            })
                            .collect(),
                    })
                    .with_spec_reference(spec),
            );
        });
    }
}

struct DonedataRule;

impl Rule for DonedataRule {
    fn code(&self) -> &'static str {
        "SCXML018"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "donedata") {
                return;
            }
            if let (Some(content), Some(param)) = (
                first_child(element, "content"),
                first_child(element, "param"),
            ) {
                let removal = |name: &str| {
                    let edits: Vec<FixEdit> = child_elements(element)
                        .filter(|child| is_scxml(child, name))
                        .map(|child| FixEdit {
                            span: crate::graph::removal_span(document, child.span),
                            replacement: String::new(),
                        })
                        .collect();
                    Fix {
                        message: match edits.len() {
                            1 => format!("remove the `<{name}>` element"),
                            count => format!("remove the {count} `<{name}>` elements"),
                        },
                        applicability: Applicability::MaybeIncorrect,
                        edits,
                    }
                };
                report.push(
                    Diagnostic::error(
                        self.code(),
                        "donedata cannot contain both `content` and `param` children",
                        content.start_tag_span,
                    )
                    .with_primary_message("mutually exclusive child")
                    .with_secondary(
                        Label::new(param.start_tag_span).with_message("conflicting child"),
                    )
                    .with_help("return either one content value or parameter children: remove the `<content>` to return the parameters, or the `<param>` children to return the content")
                    .with_fix(removal("content"))
                    .with_fix(removal("param"))
                    .with_spec_reference("SCXML 1.0 §5.5 <donedata>"),
                );
            }
        });
    }
}

struct InitialRule;

impl Rule for InitialRule {
    fn code(&self) -> &'static str {
        "SCXML009"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            let ids = collect_ids(machine);
            visit_machine(machine, &mut |element| {
                if is_scxml(element, "initial") {
                    check_initial_like(document, machine, report, element, &ids, false);
                } else if is_scxml(element, "history") {
                    check_initial_like(document, machine, report, element, &ids, true);
                }
            });
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "`<initial>` and `<history>` share one ordered check sequence whose early returns keep later findings from cascading"
)]
fn check_initial_like(
    document: &Document,
    machine: &Element,
    report: &mut Report,
    element: &Element,
    ids: &BTreeMap<String, Span>,
    history: bool,
) {
    let transitions: Vec<_> = child_elements(element)
        .filter(|child| is_scxml(child, "transition"))
        .collect();
    let code = if history { "SCXML019" } else { "SCXML009" };
    let noun = if history { "history" } else { "initial" };
    let spec = if history {
        "SCXML 1.0 §3.10 <history>"
    } else {
        "SCXML 1.0 §3.6 <initial>"
    };
    if let [first, extra @ ..] = transitions.as_slice()
        && let Some(second) = extra.first()
    {
        let mut diagnostic = Diagnostic::error(
            code,
            format!(
                "{noun} must contain exactly one transition, but has {}",
                transitions.len()
            ),
            second.start_tag_span,
        )
        .with_primary_message("extra transition")
        .with_secondary(
            Label::new(first.start_tag_span).with_message("the first transition is here"),
        );
        for more in extra.iter().skip(1) {
            diagnostic = diagnostic.with_secondary(
                Label::new(more.start_tag_span).with_message("another extra transition"),
            );
        }
        report.push(
            diagnostic
                .with_help(format!(
                    "keep one `<transition>` in `<{noun}>`; to enter several states at once, list them all in its `target` (for example `target=\"a b\"` across parallel regions)"
                ))
                .with_fix(Fix {
                    message: "keep only the first transition".to_owned(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: extra
                        .iter()
                        .map(|transition| FixEdit {
                            span: crate::graph::removal_span(document, transition.span),
                            replacement: String::new(),
                        })
                        .collect(),
                })
                .with_spec_reference(spec),
        );
        return;
    }
    if transitions.len() != 1 {
        report.push(
            Diagnostic::error(
                code,
                format!("{noun} must contain exactly one transition"),
                element.start_tag_span,
            )
            .with_primary_message(format!("found {} transition children", transitions.len()))
            .with_help(format!(
                "add exactly one target-only transition inside `{noun}`"
            ))
            .with_spec_reference(if history {
                "SCXML 1.0 §3.10 <history>"
            } else {
                "SCXML 1.0 §3.6 <initial>"
            }),
        );
        return;
    }
    let Some(transition) = transitions.first().copied() else {
        return;
    };
    let target = transition.attribute("target");
    for forbidden in ["event", "cond"] {
        if let Some(attribute) = transition.attribute(forbidden) {
            report.push(
                Diagnostic::error(
                    if history { "SCXML020" } else { "SCXML010" },
                    format!("{noun} transition must not have `{forbidden}`"),
                    attribute.span,
                )
                .with_primary_message("initialization fallback must be unconditional")
                .with_help(format!("remove `{forbidden}` from this transition"))
                .with_fix(Fix::replace(
                    format!("remove `{forbidden}`"),
                    Applicability::MachineApplicable,
                    attribute_removal_span(document, attribute),
                    "",
                ))
                .with_spec_reference(if history {
                    "SCXML 1.0 §3.10 <history>"
                } else {
                    "SCXML 1.0 §3.6 <initial>"
                }),
            );
        }
    }
    let Some(target) = target.filter(|target| xml_tokens(&target.value).next().is_some()) else {
        let fix = match target {
            Some(blank) => Fix::replace(
                "name the default `target`",
                Applicability::HasPlaceholders,
                blank.value_span,
                "…",
            ),
            None => Fix::replace(
                "add the default `target`",
                Applicability::HasPlaceholders,
                attribute_insertion_point(document, transition),
                " target=\"…\"",
            ),
        };
        report.push(
            Diagnostic::error(
                if history { "SCXML020" } else { "SCXML010" },
                format!("{noun} transition requires a `target`"),
                target.map_or(transition.start_tag_span, |blank| blank.span),
            )
            .with_primary_message(if target.is_some() {
                "empty target"
            } else {
                "missing target"
            })
            .with_help("add a target state owned by the containing state")
            .with_fix(fix)
            .with_spec_reference(if history {
                "SCXML 1.0 §3.10 <history>"
            } else {
                "SCXML 1.0 §3.6 <initial>"
            }),
        );
        return;
    };
    let Some(parent) = parent_element(machine, element) else {
        return;
    };
    for (token, token_span) in attribute_token_spans(document, target) {
        let Some(target_span) = ids.get(token.as_str()) else {
            continue;
        };
        let valid = if history
            && element
                .attribute("type")
                .is_none_or(|kind| kind.value != "deep")
        {
            is_immediate_child(machine, parent, *target_span)
        } else {
            is_descendant(machine, parent, *target_span)
        };
        if !valid {
            let deep = element
                .attribute("type")
                .is_some_and(|kind| kind.value == "deep");
            let (code, message, help) = if history && deep {
                (
                    "SCXML021",
                    format!(
                        "deep history target `{token}` is not a descendant of the history's parent"
                    ),
                    "target a state nested inside the state that contains this `<history>`",
                )
            } else if history {
                (
                    "SCXML021",
                    format!("shallow history target `{token}` is not an immediate child"),
                    "target an immediate child of the history parent, or use `type=\"deep\"` for a deeper descendant",
                )
            } else {
                (
                    "SCXML022",
                    format!(
                        "initial transition target `{token}` is not a descendant of its parent"
                    ),
                    "target a descendant state owned by the containing compound state",
                )
            };
            report.push(
                Diagnostic::error(code, message, token_span)
                    .with_primary_message("target is outside the permitted subtree")
                    .with_secondary(Label::new(*target_span).with_message("target declared here"))
                    .with_help(help)
                    .with_spec_reference(if history {
                        "SCXML 1.0 §3.10 <history>"
                    } else {
                        "SCXML 1.0 §3.6 <initial>"
                    }),
            );
        }
    }
}

struct StateInitialRule;

impl Rule for StateInitialRule {
    fn code(&self) -> &'static str {
        "SCXML011"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        document.root.visit(&mut |element| {
            if !is_scxml(element, "state") && !is_scxml(element, "scxml") {
                return;
            }
            let child_states = child_elements(element)
                .filter(|child| {
                    is_scxml(child, "state")
                        || is_scxml(child, "parallel")
                        || is_scxml(child, "final")
                })
                .count();
            let initial_elements: Vec<_> = child_elements(element)
                .filter(|child| is_scxml(child, "initial"))
                .collect();
            if let (Some(initial), Some(initial_element)) =
                (element.attribute("initial"), initial_elements.first())
            {
                report.push(
                    Diagnostic::error(
                        self.code(),
                        format!(
                            "{} cannot use both the `initial` attribute and an `initial` child",
                            element.name.local_name
                        ),
                        initial.span,
                    )
                    .with_primary_message("duplicate initial configuration")
                    .with_secondary(
                        Label::new(initial_element.start_tag_span)
                            .with_message("conflicting initial child"),
                    )
                    .with_help("keep exactly one form of initial configuration")
                    .with_spec_reference(if is_scxml(element, "scxml") {
                        "SCXML 1.0 §3.2 <scxml>"
                    } else {
                        "SCXML 1.0 §3.3 <state>"
                    }),
                );
            }
            if is_scxml(element, "state")
                && child_states == 0
                && (element.attribute("initial").is_some() || !initial_elements.is_empty())
            {
                let span = element.attribute("initial").map_or_else(
                    || initial_elements[0].start_tag_span,
                    |attribute| attribute.span,
                );
                let mut diagnostic = Diagnostic::error(
                    "SCXML012",
                    "atomic state cannot declare an initial configuration",
                    span,
                )
                .with_primary_message("state has no child states to initialize")
                .with_help("remove the initial configuration or add child states")
                .with_spec_reference("SCXML 1.0 §3.3 <state>");
                if let Some(attribute) = element.attribute("initial") {
                    diagnostic = diagnostic.with_fix(Fix::replace(
                        "remove the `initial` attribute",
                        Applicability::MaybeIncorrect,
                        attribute_removal_span(document, attribute),
                        "",
                    ));
                }
                report.push(diagnostic);
            }
            // With a single child state there is only one possible entry,
            // so document order cannot change behavior.
            if child_states > 1
                && element.attribute("initial").is_none()
                && initial_elements.is_empty()
                && !is_scxml(element, "parallel")
            {
                report.push(implicit_initial_diagnostic(document, element));
            }
        });
    }
}

/// LINT001: a compound state (or the root) with several child states and no
/// explicit initial state; entry follows document order.
fn implicit_initial_diagnostic(document: &Document, element: &Element) -> Diagnostic {
    let subject = if is_scxml(element, "scxml") {
        "the `<scxml>` root".to_owned()
    } else {
        state_message(element, "")
            .trim_end()
            .replacen("state", "compound state", 1)
    };
    // The implicit default is the first child state in document
    // order; naming it explicitly keeps behavior identical.
    let first_child_state = child_elements(element).find(|child| {
        is_scxml(child, "state") || is_scxml(child, "parallel") || is_scxml(child, "final")
    });
    let mut diagnostic = Diagnostic::warning(
        "LINT001",
        format!("{subject} has several child states but no explicit initial state"),
        element.start_tag_span,
    )
    .with_category(DiagnosticCategory::Lint)
    .with_primary_message("entry behavior depends on implicit document order")
    .with_spec_reference(if is_scxml(element, "scxml") {
        "SCXML 1.0 §3.2 <scxml>"
    } else {
        "SCXML 1.0 §3.3 <state>"
    });
    match first_child_state.and_then(|child| child.attribute("id")) {
        Some(id) => {
            diagnostic = diagnostic
                .with_secondary(
                    Label::new(id.value_span)
                        .with_message("entered first because it comes first in document order"),
                )
                .with_help(format!(
                    "make the entry explicit with `initial=\"{}\"` (keeps the current behavior), or name the state that should be entered first",
                    id.value
                ))
                .with_fix(Fix::replace(
                    format!("make the current entry state `{}` explicit", id.value),
                    Applicability::MachineApplicable,
                    attribute_insertion_point(document, element),
                    format!(" initial=\"{}\"", id.value),
                ));
        }
        None => {
            diagnostic = diagnostic.with_help("add an `initial` attribute or an `initial` child so the intended entry state is explicit");
        }
    }
    diagnostic
}

struct LivenessRule;

impl Rule for LivenessRule {
    fn code(&self) -> &'static str {
        "LINT002"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            // Depth-first walk carrying whether a proper ancestor can exit:
            // a transition or `<invoke>` on any enclosing `<state>` or
            // `<parallel>` also leaves every descendant atomic state, and a
            // transition anywhere inside an enclosing `<parallel>` may leave
            // the whole `<parallel>`, exiting every sibling region with it.
            let mut pending = vec![(machine, false)];
            // IDs declared earlier in document order: a later declaration of
            // the same ID is `SCXML002`'s finding, and every reference
            // resolves to the first declaration, so the duplicate can never
            // be entered and its liveness is not judged.
            let mut declared: BTreeSet<&str> = BTreeSet::new();
            while let Some((element, ancestor_exits)) = pending.pop() {
                let duplicate =
                    state_id(element).is_some_and(|id| !declared.insert(id.value.as_str()));
                let own_exits = ((is_scxml(element, "state") || is_scxml(element, "parallel"))
                    && child_elements(element)
                        .any(|child| is_scxml(child, "transition") || is_scxml(child, "invoke")))
                    || (is_scxml(element, "parallel") && parallel_may_exit(element));
                for child in child_elements(element)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    if !is_scxml(child, "scxml") {
                        pending.push((child, ancestor_exits || own_exits));
                    }
                }
                // `<state type="final">` is reported by the schema with a
                // rename to `<final>`; its missing exit is the same mistake.
                let meant_final = element
                    .attribute("type")
                    .is_some_and(|kind| kind.name.namespace_uri.is_none() && kind.value == "final");
                if !is_scxml(element, "state")
                    || duplicate
                    || has_state_child(element)
                    || ancestor_exits
                    || has_unknown_scxml_child(element)
                    || meant_final
                {
                    continue;
                }
                let transitions: Vec<_> = child_elements(element)
                    .filter(|child| is_scxml(child, "transition"))
                    .collect();
                if transitions.is_empty() && first_child(element, "invoke").is_none() {
                    report.push(
                        Diagnostic::warning(
                            self.code(),
                            state_message(element, "has no outgoing transition and may never exit"),
                            element.start_tag_span,
                        )
                        .with_category(DiagnosticCategory::Lint)
                        .with_primary_message("non-final atomic state has no exit")
                        .with_note("neither this state nor any enclosing `<state>` or `<parallel>` has a transition or `<invoke>`, and no transition in an enclosing `<parallel>` can leave that `<parallel>`, so once entered the machine can never leave it")
                        .with_help("add an outgoing transition, make this a `final` state, or document an intentional externally terminated wait")
                        .with_spec_reference("SCXML 1.0 §3.3 <state>"),
                    );
                } else if !transitions.is_empty()
                    && transitions.iter().all(|transition| {
                        transition.attribute("event").is_none()
                            && transition.attribute("cond").is_some()
                    })
                    && first_child(element, "invoke").is_none()
                {
                    report.push(
                        Diagnostic::warning(
                            "LINT003",
                            state_message(element, "has only conditional eventless exits"),
                            element.start_tag_span,
                        )
                        .with_category(DiagnosticCategory::Lint)
                        .with_primary_message(
                            "the state may remain active when every exit condition is false",
                        )
                        .with_help("if the state must always progress, add an unconditional eventless fallback; otherwise document the intentional conditional wait")
                        .with_spec_reference("SCXML 1.0 §3.5 <transition>"),
                    );
                }
            }
            detect_eventless_cycles(document, machine, report);
        }
    }
}

/// Whether a transition inside a `<parallel>` may leave the whole
/// `<parallel>` and so exit every region: its source is the region itself
/// (an external transition from a region is scoped above the `<parallel>`
/// unless it is internal), or one of its targets lies outside the region that
/// contains it (or is unresolved). A targetless transition, or one whose
/// targets all stay in its own region, never exits a sibling region.
/// Inline machines are independent documents and are not descended into.
fn parallel_may_exit(parallel: &Element) -> bool {
    for region in child_elements(parallel) {
        let mut region_ids = BTreeSet::new();
        let mut transitions = Vec::new();
        let mut pending = vec![(region, true)];
        while let Some((element, is_region)) = pending.pop() {
            if let Some(id) = state_id(element) {
                region_ids.insert(id.value.as_str());
            }
            for child in child_elements(element) {
                if is_scxml(child, "transition") {
                    transitions.push((child, is_region));
                } else if !is_scxml(child, "scxml") {
                    pending.push((child, false));
                }
            }
        }
        for (transition, from_region) in transitions {
            let Some(target) = transition.attribute("target") else {
                continue;
            };
            let internal = transition
                .attribute("type")
                .is_some_and(|kind| kind.value == "internal");
            if (from_region && !internal)
                || xml_tokens(&target.value).any(|token| !region_ids.contains(token))
            {
                return true;
            }
        }
    }
    false
}

/// One eventless, conditionless transition edge of a detected cycle.
struct CycleEdge {
    from: String,
    to: String,
    /// Exact span of the `to` token inside the transition's `target`.
    span: Span,
}

fn detect_eventless_cycles(document: &Document, machine: &Element, report: &mut Report) {
    let mut graph: BTreeMap<String, Vec<(String, Span)>> = BTreeMap::new();
    let mut declarations = BTreeMap::new();
    visit_machine(machine, &mut |element| {
        if let Some(id) = state_id(element) {
            declarations.insert(id.value.clone(), id.value_span);
            // Only the first eventless, conditionless transition can ever be
            // selected; any later one is shadowed by it (`LINT007`), so it
            // is never an edge of a cycle the interpreter can take.
            if let Some(transition) = child_elements(element).find(|child| {
                is_scxml(child, "transition")
                    && child.attribute("event").is_none()
                    && child.attribute("cond").is_none()
            }) && let Some(target) = transition.attribute("target")
            {
                for (token, token_span) in attribute_token_spans(document, target) {
                    graph
                        .entry(id.value.clone())
                        .or_default()
                        .push((token, token_span));
                }
            }
        }
    });
    for edges in graph.values_mut() {
        edges.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
    }

    let mut global_visited = BTreeSet::new();
    for start in graph.keys() {
        if global_visited.contains(start) {
            continue;
        }
        let Some(cycle) = cycle_from(start, &graph, &mut global_visited) else {
            continue;
        };
        let Some(first) = cycle.first() else {
            continue;
        };
        let cycle_text = cycle
            .iter()
            .map(|edge| edge.from.as_str())
            .chain(std::iter::once(first.from.as_str()))
            .collect::<Vec<_>>()
            .join(" -> ");
        let mut diagnostic = Diagnostic::warning(
            "LINT004",
            format!("unconditional eventless transition cycle: {cycle_text}"),
            first.span,
        )
        .with_primary_message(format!(
            "`{}` enters `{}` without consuming an event",
            first.from, first.to
        ))
        .with_help("add an event or condition to one transition, or remove a target so the macrostep can reach a stable configuration")
        .with_spec_reference("SCXML 1.0 §3.13 Selecting and Executing Transitions");
        let last = cycle.len().saturating_sub(1);
        for (index, edge) in cycle.iter().enumerate().skip(1) {
            let message = if index == last {
                format!(
                    "then `{}` enters `{}`, closing the cycle",
                    edge.from, edge.to
                )
            } else {
                format!("then `{}` enters `{}`", edge.from, edge.to)
            };
            diagnostic = diagnostic.with_secondary(Label::new(edge.span).with_message(message));
        }
        if let Some(declaration) = declarations.get(&first.from) {
            diagnostic = diagnostic.with_secondary(
                Label::new(*declaration).with_message(format!("`{}` declared here", first.from)),
            );
        }
        report.push(diagnostic);
    }
}

/// Iterative depth-first search from `start` for one cycle. Every state is
/// expanded at most once across all searches, so total work is linear in the
/// graph. The returned edges are in cycle order and form a closed walk.
fn cycle_from(
    start: &str,
    graph: &BTreeMap<String, Vec<(String, Span)>>,
    global_visited: &mut BTreeSet<String>,
) -> Option<Vec<CycleEdge>> {
    // Each frame is a state on the current path and the next edge to try.
    let mut frames: Vec<(String, usize)> = vec![(start.to_owned(), 0)];
    // Edge taken out of each frame, parallel to `frames`.
    let mut taken: Vec<Option<(String, Span)>> = vec![None];
    let mut on_path: BTreeMap<String, usize> = BTreeMap::from([(start.to_owned(), 0)]);
    global_visited.insert(start.to_owned());
    while let Some((state, edge_index)) = frames.last_mut() {
        let next_edge = graph
            .get(state.as_str())
            .and_then(|edges| edges.get(*edge_index));
        let Some((next, edge_span)) = next_edge else {
            if let Some((finished, _)) = frames.pop() {
                on_path.remove(&finished);
            }
            taken.pop();
            continue;
        };
        *edge_index = edge_index.saturating_add(1);
        if let Some(slot) = taken.last_mut() {
            *slot = Some((next.clone(), *edge_span));
        }
        if let Some(index) = on_path.get(next).copied() {
            let cycle = frames
                .iter()
                .zip(&taken)
                .skip(index)
                .filter_map(|((from, _), edge)| {
                    edge.as_ref().map(|(to, span)| CycleEdge {
                        from: from.clone(),
                        to: to.clone(),
                        span: *span,
                    })
                })
                .collect();
            return Some(cycle);
        }
        if global_visited.insert(next.clone()) {
            on_path.insert(next.clone(), frames.len());
            frames.push((next.clone(), 0));
            taken.push(None);
        }
    }
    None
}

/// One pair of mutually exclusive attributes and how to report it.
#[derive(Clone, Copy)]
struct XorRule {
    code: &'static str,
    left: &'static str,
    right: &'static str,
    subject: &'static str,
    spec: &'static str,
}

fn check_attribute_xor(
    document: &Document,
    report: &mut Report,
    element: &Element,
    rule: &XorRule,
) {
    let XorRule {
        code,
        left: left_name,
        right: right_name,
        subject,
        spec,
    } = *rule;
    if let (Some(left), Some(right)) = (element.attribute(left_name), element.attribute(right_name))
    {
        report.push(
            Diagnostic::error(
                code,
                format!("{subject} cannot use both `{left_name}` and `{right_name}`"),
                left.span,
            )
            .with_primary_message("mutually exclusive attribute")
            .with_secondary(Label::new(right.span).with_message("conflicting attribute"))
            .with_help(format!(
                "keep exactly one of `{left_name}` or `{right_name}`"
            ))
            .with_fix(Fix::replace(
                format!("keep `{left_name}` and remove `{right_name}`"),
                Applicability::MaybeIncorrect,
                attribute_removal_span(document, right),
                "",
            ))
            .with_spec_reference(spec),
        );
    }
}

fn collect_ids(machine: &Element) -> BTreeMap<String, Span> {
    let mut ids = BTreeMap::new();
    visit_machine(machine, &mut |element| {
        if let Some(id) = state_id(element) {
            ids.entry(id.value.clone()).or_insert(id.value_span);
        }
    });
    ids
}

pub(crate) fn attribute_token_spans(
    document: &Document,
    attribute: &Attribute,
) -> Vec<(String, Span)> {
    // A value written with character or entity references (XML 1.0 §4.4)
    // has no token-exact source bytes: each token then spans the whole
    // value. A list value's items are separated by XML white space only
    // (XML Schema Part 2 §4.3.6), and its source bytes collapse to it.
    let Some(lexical_value) = document
        .source
        .slice(attribute.value_span)
        .filter(|lexical| collapse_whitespace(lexical) == attribute.value.as_str())
    else {
        return xml_tokens(&attribute.value)
            .map(|token| (token.to_owned(), attribute.value_span))
            .collect();
    };
    let mut tokens = Vec::new();
    let mut token_start = None;
    for (offset, character) in lexical_value.char_indices() {
        if is_xml_space(character) {
            if let Some(start) = token_start.take() {
                tokens.push((
                    lexical_value[start..offset].to_owned(),
                    Span::new(
                        attribute.value_span.source,
                        attribute.value_span.start + start,
                        attribute.value_span.start + offset,
                    ),
                ));
            }
        } else if token_start.is_none() {
            token_start = Some(offset);
        }
    }
    if let Some(start) = token_start {
        tokens.push((
            lexical_value[start..].to_owned(),
            Span::new(
                attribute.value_span.source,
                attribute.value_span.start + start,
                attribute.value_span.end,
            ),
        ));
    }
    tokens
}

/// Spans where `value` occurs as a whole whitespace-delimited token run in an
/// attribute's exact source bytes (an ID with spaces spans several tokens).
fn token_run_spans(document: &Document, attribute: &Attribute, value: &str) -> Vec<Span> {
    let Some(lexical) = document.source.slice(attribute.value_span) else {
        return Vec::new();
    };
    if value.is_empty() || value.trim_matches(is_xml_space) != value {
        return Vec::new();
    }
    let mut spans = Vec::new();
    for (offset, _) in lexical.match_indices(value) {
        let end = offset + value.len();
        let starts_token = lexical
            .get(..offset)
            .and_then(|head| head.chars().next_back())
            .is_none_or(is_xml_space);
        let ends_token = lexical
            .get(end..)
            .and_then(|tail| tail.chars().next())
            .is_none_or(is_xml_space);
        if starts_token && ends_token {
            spans.push(Span::new(
                attribute.value_span.source,
                attribute.value_span.start + offset,
                attribute.value_span.start + end,
            ));
        }
    }
    spans
}

/// Every `target` and `initial` reference to the ID `value` in one machine.
fn id_reference_spans(document: &Document, machine: &Element, value: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    visit_machine(machine, &mut |element| {
        let attribute = if is_scxml(element, "transition") {
            element.attribute("target")
        } else if is_scxml(element, "scxml") || is_scxml(element, "state") {
            element.attribute("initial")
        } else {
            None
        };
        if let Some(attribute) = attribute {
            spans.extend(token_run_spans(document, attribute, value));
        }
    });
    spans.sort();
    spans
}

/// The declared multi-token (therefore invalid) ID whose reference run in
/// `attribute` covers `token`, if any.
fn covering_spaced_id<'a>(
    document: &Document,
    attribute: &Attribute,
    token: Span,
    ids: &'a BTreeMap<String, Span>,
) -> Option<(&'a str, Span)> {
    ids.iter()
        .filter(|(id, _)| xml_tokens(id).nth(1).is_some())
        .find(|(id, _)| {
            token_run_spans(document, attribute, id)
                .iter()
                .any(|run| run.start <= token.start && token.end <= run.end)
        })
        .map(|(id, span)| (id.as_str(), *span))
}

pub(crate) fn machine_roots(root: &Element) -> Vec<&Element> {
    let mut machines = Vec::new();
    root.visit(&mut |element| {
        if is_scxml(element, "scxml") {
            machines.push(element);
        }
    });
    machines
}

fn first_machine_over_depth(root: &Element) -> Option<&Element> {
    let root_depth = usize::from(is_scxml(root, "scxml"));
    let mut pending = vec![(root, root_depth)];
    while let Some((element, depth)) = pending.pop() {
        if scxml_types::exceeds_wasm_bound(depth, MAX_NESTED_SCXML_DEPTH) {
            return Some(element);
        }
        for child in child_elements(element) {
            let child_depth = depth.saturating_add(usize::from(is_scxml(child, "scxml")));
            pending.push((child, child_depth));
        }
    }
    None
}

pub(crate) fn visit_machine<'a>(machine: &'a Element, visitor: &mut impl FnMut(&'a Element)) {
    let mut pending = vec![machine];
    while let Some(element) = pending.pop() {
        visitor(element);
        for child in element.children.iter().rev() {
            if let Node::Element(child) = child
                && !is_scxml(child, "scxml")
            {
                pending.push(child);
            }
        }
    }
}

fn machine_elements(machine: &Element) -> Vec<&Element> {
    let mut result = Vec::new();
    visit_machine(machine, &mut |element| result.push(element));
    result
}

pub(crate) fn child_elements(element: &Element) -> impl Iterator<Item = &Element> {
    element.children.iter().filter_map(|child| match child {
        Node::Element(element) => Some(element),
        Node::Text(_) => None,
    })
}

pub(crate) fn first_child<'a>(element: &'a Element, name: &str) -> Option<&'a Element> {
    child_elements(element).find(|child| is_scxml(child, name))
}

pub(crate) fn is_scxml(element: &Element, name: &str) -> bool {
    element.name.is(Some(SCXML_NAMESPACE), name)
}

fn state_id(element: &Element) -> Option<&Attribute> {
    (is_scxml(element, "state")
        || is_scxml(element, "parallel")
        || is_scxml(element, "final")
        || is_scxml(element, "history"))
    .then(|| element.attribute("id"))
    .flatten()
}

/// Every element name the SCXML 1.0 namespace defines.
const SCXML_ELEMENTS: [&str; 26] = [
    "assign",
    "cancel",
    "content",
    "data",
    "datamodel",
    "donedata",
    "else",
    "elseif",
    "final",
    "finalize",
    "foreach",
    "history",
    "if",
    "initial",
    "invoke",
    "log",
    "onentry",
    "onexit",
    "parallel",
    "param",
    "raise",
    "script",
    "scxml",
    "send",
    "state",
    "transition",
];

/// Whether `element` has a child in the SCXML namespace that SCXML does not
/// define (a misspelled element the schema already reports). Graph lints
/// about that element's parent would only cascade from the misspelling.
pub(crate) fn has_unknown_scxml_child(element: &Element) -> bool {
    child_elements(element).any(|child| {
        child.name.namespace_uri.as_deref() == Some(SCXML_NAMESPACE)
            && !SCXML_ELEMENTS.contains(&child.name.local_name.as_str())
    })
}

fn has_state_child(element: &Element) -> bool {
    child_elements(element).any(|child| {
        is_scxml(child, "state") || is_scxml(child, "parallel") || is_scxml(child, "final")
    })
}

fn parent_element<'a>(machine: &'a Element, child: &Element) -> Option<&'a Element> {
    let target_start = child.span.start;
    let mut pending = vec![machine];
    while let Some(current) = pending.pop() {
        for candidate in child_elements(current) {
            if candidate.span.start == target_start {
                return Some(current);
            }
            if !is_scxml(candidate, "scxml") {
                pending.push(candidate);
            }
        }
    }
    None
}

fn parent_is(machine: &Element, child: &Element, name: &str) -> bool {
    parent_element(machine, child).is_some_and(|parent| is_scxml(parent, name))
}

fn element_by_span(machine: &Element, span: Span) -> Option<&Element> {
    machine_elements(machine).into_iter().find(|element| {
        element
            .attribute("id")
            .is_some_and(|id| id.value_span == span)
    })
}

fn is_descendant(machine: &Element, ancestor: &Element, target_span: Span) -> bool {
    let Some(target) = element_by_span(machine, target_span) else {
        return false;
    };
    target.span.start > ancestor.span.start && target.span.end <= ancestor.span.end
}

fn is_immediate_child(machine: &Element, parent: &Element, target_span: Span) -> bool {
    let Some(target) = element_by_span(machine, target_span) else {
        return false;
    };
    child_elements(parent).any(|child| child.span == target.span)
}

fn state_message(element: &Element, suffix: &str) -> String {
    element.attribute("id").map_or_else(
        || format!("anonymous state {suffix}"),
        |id| format!("state `{}` {suffix}", id.value),
    )
}

pub(crate) fn is_ncname(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !is_ncname_start(first) {
        return false;
    }
    characters.all(is_ncname_continue)
}

fn is_ncname_start(character: char) -> bool {
    matches!(
        character,
        'A'..='Z'
            | '_'
            | 'a'..='z'
            | '\u{00C0}'..='\u{00D6}'
            | '\u{00D8}'..='\u{00F6}'
            | '\u{00F8}'..='\u{02FF}'
            | '\u{0370}'..='\u{037D}'
            | '\u{037F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

fn is_ncname_continue(character: char) -> bool {
    is_ncname_start(character)
        || character.is_ascii_digit()
        || matches!(character, '-' | '.' | '\u{00B7}')
        || ('\u{0300}'..='\u{036F}').contains(&character)
        || ('\u{203F}'..='\u{2040}').contains(&character)
}

fn ncname_suggestion(value: &str) -> Option<String> {
    let mut result = String::new();
    for (index, character) in value.chars().enumerate() {
        if (index == 0 && is_ncname_start(character))
            || (index > 0 && is_ncname_continue(character))
        {
            result.push(character);
        } else if is_xml_space(character) || character == ':' {
            result.push('_');
        } else if index == 0 && character.is_ascii_digit() {
            result.push('_');
            result.push(character);
        } else {
            result.push('_');
        }
    }
    (!result.is_empty() && result != value && is_ncname(&result)).then_some(result)
}

pub(crate) fn valid_event_descriptor(token: &str) -> bool {
    if token == "*" || token == ".*" {
        return true;
    }
    let token = token.strip_suffix(".*").unwrap_or(token);
    let token = token.strip_suffix('.').unwrap_or(token);
    !token.is_empty()
        && !token.starts_with('.')
        && token.split('.').all(|segment| {
            // The official schema's segments are `(\i|\d|\-)+`; `\i`
            // includes `:` (`scxml-datatypes.xsd`, `EventTypes.datatype`).
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|character| character == ':' || is_ncname_continue(character))
        })
}

/// Returns the closest candidate within a length-scaled bound of at most
/// [`MAX_TYPO_SUGGESTION_DISTANCE`] optimal-string-alignment edits (a transposition counts as one edit), with
/// ties broken by the smallest candidate. The search spends at most
/// [`MAX_TYPO_SUGGESTION_EDIT_CELLS`] units.
pub(crate) fn nearest<'a>(
    needle: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut budget = MAX_TYPO_SUGGESTION_EDIT_CELLS;
    nearest_within(needle, candidates, &mut budget)
}

/// Bounded typo search against a caller-owned work budget.
///
/// The result is exact or absent: when the candidate count or `budget` cuts
/// the search short, no suggestion is returned, so a skipped closer candidate
/// can never lose to a worse one that happened to be examined first.
pub(crate) fn nearest_within<'a>(
    needle: &str,
    candidates: impl Iterator<Item = &'a str>,
    budget: &mut usize,
) -> Option<&'a str> {
    let needle_chars: Vec<_> = needle
        .chars()
        .take(MAX_TYPO_SUGGESTION_IDENTIFIER_CHARS + 1)
        .collect();
    if needle_chars.is_empty() || needle_chars.len() > MAX_TYPO_SUGGESTION_IDENTIFIER_CHARS {
        return None;
    }
    // Short names tolerate fewer edits, so `lua` is not "corrected" to `null`.
    let threshold = match needle_chars.len() {
        0..=3 => 1,
        4..=7 => 2,
        _ => MAX_TYPO_SUGGESTION_DISTANCE,
    };
    let longest = needle_chars.len() + threshold;
    let mut best: Option<(usize, &'a str)> = None;
    for (examined, candidate) in candidates.enumerate() {
        if examined >= MAX_TYPO_SUGGESTION_CANDIDATES {
            return None;
        }
        // Reading at most `longest + 1` characters decides the length filter.
        let candidate_chars: Vec<_> = candidate.chars().take(longest + 1).collect();
        *budget = budget.checked_sub(candidate_chars.len() + 1)?;
        if candidate_chars.len().abs_diff(needle_chars.len()) > threshold {
            continue;
        }
        let cells = (needle_chars.len() + 1).checked_mul(candidate_chars.len() + 1)?;
        *budget = budget.checked_sub(cells)?;
        let distance = osa_distance(&needle_chars, &candidate_chars);
        if distance > 0
            && distance <= threshold
            && best.is_none_or(|current| (distance, candidate) < current)
        {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Optimal-string-alignment distance: insertions, deletions, substitutions,
/// and adjacent transpositions each cost one edit.
pub(crate) fn osa_distance(left: &[char], right: &[char]) -> usize {
    let width = right.len() + 1;
    let mut rows = vec![vec![0_usize; width]; 3];
    let cell = |rows: &Vec<Vec<usize>>, row: usize, column: usize| -> usize {
        rows.get(row % 3)
            .and_then(|values| values.get(column))
            .copied()
            .unwrap_or(usize::MAX / 2)
    };
    if let Some(first) = rows.first_mut() {
        for (column, value) in first.iter_mut().enumerate() {
            *value = column;
        }
    }
    for row in 1..=left.len() {
        for column in 0..width {
            let value = if column == 0 {
                row
            } else {
                let left_character = left.get(row - 1);
                let right_character = right.get(column - 1);
                let substitution = usize::from(left_character != right_character);
                let mut value = (cell(&rows, row - 1, column) + 1)
                    .min(cell(&rows, row, column - 1) + 1)
                    .min(cell(&rows, row - 1, column - 1) + substitution);
                if row > 1
                    && column > 1
                    && left_character == right.get(column - 2)
                    && left.get(row - 2) == right_character
                {
                    value = value.min(cell(&rows, row - 2, column - 2) + 1);
                }
                value
            };
            if let Some(slot) = rows
                .get_mut(row % 3)
                .and_then(|values| values.get_mut(column))
            {
                *slot = value;
            }
        }
    }
    cell(&rows, left.len(), right.len())
}

#[cfg(test)]
mod tests {
    use scxml_syntax::{SourceId, SourceText, parse_xml};

    use super::*;

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);

    fn validate(xml: &str) -> Report {
        let document = parse_xml(SourceText::new(SOURCE, "test.scxml", xml))
            .expect("semantic-validation XML fixture must parse");
        validate_semantics(&document)
    }

    fn codes(report: &Report) -> BTreeSet<&str> {
        report
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    #[test]
    fn raise_and_send_inside_finalize_are_errors_with_a_removal() {
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="a"><state id="a"><invoke type="http://www.w3.org/TR/scxml/" src="c.scxml"><finalize><log expr="1"/><if cond="true"><raise event="x"/></if><send event="y"><content><scxml version="1.0"><final id="z"/></scxml></content></send></finalize></invoke><transition event="x" target="b"/></state><final id="b"><onentry><raise event="ok"/></onentry></final></scxml>"#
        );
        let report = validate(&xml);
        let findings: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML055")
            .collect();
        assert_eq!(findings.len(), 2, "{report:?}");
        for (finding, element) in findings.iter().zip(["<raise", "<send"]) {
            assert_eq!(finding.severity, crate::Severity::Error);
            assert!(xml[finding.primary.span.start..].starts_with(element));
            assert!(xml[finding.secondary[0].span.start..].starts_with("<finalize"));
            let edit = &finding.fixes[0].edits[0];
            assert!(xml[edit.span.start..edit.span.end].starts_with(element));
            assert!(edit.replacement.is_empty());
        }
    }

    #[test]
    fn duplicate_and_unknown_ids_have_related_spans_and_fixes() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="ready"><state id="same"/><state id="same"/><state id="ready"><transition event="go" target="don"/></state><final id="done"/></scxml>"#
        ));
        let duplicate = report
            .diagnostics()
            .iter()
            .find(|item| item.code == "SCXML002")
            .expect("duplicate-ID diagnostic must be present");
        assert_eq!(duplicate.secondary.len(), 1);
        let target = report
            .diagnostics()
            .iter()
            .find(|item| item.code == "SCXML005")
            .expect("unknown-target diagnostic must be present");
        assert_eq!(target.help.as_deref(), Some("did you mean `done`?"));
        assert_eq!(target.fixes[0].applicability, Applicability::MaybeIncorrect);
    }

    #[test]
    fn event_and_mutual_exclusion_inventory_is_actionable() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="s"><transition event="bad,token" target="s"/><onentry><param expr="1" location="x"/><cancel sendid="a" sendidexpr="b"/><send event="e" eventexpr="computed"><content/></send><send namelist="x"><content/></send><invoke src="a" srcexpr="b"><content/></invoke></onentry></state><final id="f"><donedata><content/><param name="p" expr="1"/></donedata></final></scxml>"#
        ));
        let codes = codes(&report);
        for expected in [
            "SCXML007", "SCXML008", "SCXML013", "SCXML014", "SCXML015", "SCXML016", "SCXML018",
        ] {
            assert!(codes.contains(expected), "missing {expected}: {codes:?}");
        }
        assert!(report.diagnostics().iter().all(|diagnostic| {
            diagnostic.help.is_some() || diagnostic.severity == crate::Severity::Note
        }));
    }

    /// SCXML 1.0 §6.2: `delay` and `delayexpr` "Must not occur ... when the
    /// attribute 'target' has the value "_internal"".
    #[test]
    fn a_delayed_send_to_the_internal_queue_is_reported() {
        let on_entry = |body: &str| {
            format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="s"><state id="s"><onentry>{body}</onentry></state></scxml>"#
            )
        };
        for (source, expected) in [
            (r##"<send event="e" target="#_internal" delay="1s"/>"##, 1),
            (
                r##"<send event="e" target="#_internal" delayexpr="'1s'"/>"##,
                1,
            ),
            (r##"<send event="e" target="#_internal"/>"##, 0),
            (r#"<send event="e" delay="1s"/>"#, 0),
            (
                r#"<send event="e" targetexpr="'#_internal'" delay="1s"/>"#,
                0,
            ),
        ] {
            let report = validate(&on_entry(source));
            let found: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == "SCXML070")
                .collect();
            assert_eq!(found.len(), expected, "{source}");
            for diagnostic in found {
                assert_eq!(diagnostic.secondary.len(), 1, "the target is labeled");
                assert_eq!(
                    diagnostic.fixes[0].applicability,
                    Applicability::MaybeIncorrect
                );
            }
        }
    }

    /// SCXML 1.0 exclusivity and required-attribute constraints the W3C XSD
    /// cannot express: each bad form yields exactly its code, each legal
    /// neighbour none of them.
    #[test]
    fn spec_exclusivity_rules_not_expressible_in_the_schema() {
        let wrap = |body: &str| {
            format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="s"><datamodel>{body}</datamodel><state id="s"/></scxml>"#
            )
        };
        let on_entry = |body: &str| {
            format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="s"><state id="s"><onentry>{body}</onentry></state></scxml>"#
            )
        };
        let in_state = |body: &str| {
            format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="s"><state id="s">{body}</state></scxml>"#
            )
        };
        let rules = ["SCXML014", "SCXML016", "SCXML052", "SCXML053"];
        let cases: Vec<(String, Option<&str>)> = vec![
            (
                wrap(r#"<data id="d" expr="1" src="d.json"/>"#),
                Some("SCXML053"),
            ),
            (wrap(r#"<data id="d" expr="1">3</data>"#), Some("SCXML053")),
            (
                wrap(r#"<data id="d" src="d.json"><x/></data>"#),
                Some("SCXML053"),
            ),
            (
                wrap(r#"<data id="d" expr="1" src="d.json">3</data>"#),
                Some("SCXML053"),
            ),
            (wrap(r#"<data id="d" expr="1"/>"#), None),
            (wrap(r#"<data id="d">3</data>"#), None),
            (wrap(r#"<data id="d" src="d.json"/>"#), None),
            (wrap("<data id=\"d\" expr=\"1\">\n  </data>"), None),
            (
                on_entry(r#"<send event="e"><content expr="1">literal</content></send>"#),
                Some("SCXML053"),
            ),
            (
                on_entry(r#"<send event="e"><content expr="1"><p/></content></send>"#),
                Some("SCXML053"),
            ),
            (
                on_entry(r#"<send event="e"><content expr="1"/></send>"#),
                None,
            ),
            (
                on_entry(r#"<send event="e"><content>literal</content></send>"#),
                None,
            ),
            (
                on_entry(r#"<send event="e" id="a" idlocation="b"/>"#),
                Some("SCXML014"),
            ),
            (on_entry(r#"<send event="e" id="a"/>"#), None),
            (
                in_state(r#"<invoke src="c.scxml" id="a" idlocation="b"/>"#),
                Some("SCXML016"),
            ),
            (in_state(r#"<invoke src="c.scxml" idlocation="b"/>"#), None),
            (
                on_entry(r##"<send target="#_internal"/>"##),
                Some("SCXML052"),
            ),
            (
                on_entry(
                    r#"<send type="http://www.w3.org/TR/scxml/#SCXMLEventProcessor"><content>x</content></send>"#,
                ),
                Some("SCXML052"),
            ),
            (
                on_entry(r#"<send type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor"/>"#),
                None,
            ),
            (on_entry(r#"<send typeexpr="kind"/>"#), None),
            (on_entry(r#"<send eventexpr="'e'"/>"#), None),
        ];
        for (xml, expected) in cases {
            let report = validate(&xml);
            let found: Vec<&str> = report
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .filter(|code| rules.contains(code))
                .collect();
            assert_eq!(found, expected.into_iter().collect::<Vec<_>>(), "{xml}");
            for diagnostic in report.diagnostics() {
                if rules.contains(&diagnostic.code.as_str()) {
                    assert!(diagnostic.help.is_some() && !diagnostic.spec_references.is_empty());
                    assert!(
                        !diagnostic.fixes.is_empty(),
                        "{}: every exclusivity finding offers a repair",
                        diagnostic.code
                    );
                }
            }
        }
    }

    #[test]
    fn one_mistake_one_finding_for_pseudo_states_and_state_type_attributes() {
        for (noun, code) in [("initial", "SCXML009"), ("history", "SCXML019")] {
            let xml = format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="p"><state id="p"><{noun}><transition target="a"/><transition target="b"/></{noun}><state id="a"><transition event="x" target="b"/></state><state id="b"><transition event="x" target="a"/></state></state></scxml>"#
            );
            let report = validate(&xml);
            let found: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == code)
                .collect();
            assert_eq!(found.len(), 1, "{report:?}");
            let diagnostic = found[0];
            assert!(
                xml[diagnostic.primary.span.start..].starts_with(r#"<transition target="b"/>"#),
                "the extra transition is primary: {diagnostic:?}"
            );
            let fix = &diagnostic.fixes[0];
            let mut fixed = xml.clone();
            for edit in fix.edits.iter().rev() {
                fixed.replace_range(edit.span.start..edit.span.end, &edit.replacement);
            }
            assert!(!codes(&validate(&fixed)).contains(code), "{fixed}");
        }
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="a"><state id="a"><transition event="go" target="f"/></state><state id="f" type="final"/></scxml>"#
        ));
        assert!(
            !codes(&report).contains("LINT002"),
            "`type=\"final\"` is one schema finding with a rename, not also a liveness lint: {report:?}"
        );
    }

    #[test]
    fn send_namelist_and_param_are_compatible_but_content_payload_sources_conflict() {
        let compatible = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="s"><onentry><send namelist="first second"><param name="third" expr="3"/></send></onentry></state></scxml>"#
        ));
        assert!(
            !compatible
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "SCXML015"),
            "namelist and param contribute compatible send payload entries"
        );

        for conflicting_send in [
            r#"<send namelist="first"><content expr="payload"/></send>"#,
            r#"<send><param name="first" expr="1"/><content expr="payload"/></send>"#,
        ] {
            let report = validate(&format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="s"><onentry>{conflicting_send}</onentry></state></scxml>"#
            ));
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.code == "SCXML015"),
                "content must remain exclusive with other send payload sources"
            );
        }
    }

    #[test]
    fn send_identity_can_combine_with_each_legal_payload_form() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="s"><onentry>
                <send event="literal"><content expr="payload"/></send>
                <send eventexpr="computed" namelist="first"><param name="second" expr="2"/></send>
            </onentry></state></scxml>"#
        ));

        assert!(
            !report
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML014" | "SCXML015")),
            "event identity and payload selection are independent: {report:?}"
        );
    }

    /// SCXML 1.0 §6.4.1, `<invoke>` attribute `namelist`: "Must not occur
    /// with the <param> element."
    #[test]
    fn an_invoke_namelist_with_param_children_is_reported_with_a_param_fix() {
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><datamodel><data id="first"/><data id="third"/></datamodel><state id="parent"><invoke namelist="first third"><param name="second" expr="2"/><content><scxml xmlns="{SCXML_NAMESPACE}" initial="child"><final id="child"/></scxml></content></invoke></state></scxml>"#
        );
        let report = validate(&xml);
        let found: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML071")
            .collect();
        assert_eq!(found.len(), 1, "{report:?}");
        let diagnostic = found[0];
        assert!(
            xml[diagnostic.primary.span.start..].starts_with(r#"namelist="first third""#),
            "the namelist is primary: {diagnostic:?}"
        );
        assert_eq!(diagnostic.secondary.len(), 1, "the first param is labeled");
        assert!(diagnostic.help.is_some() && !diagnostic.spec_references.is_empty());
        let fix = &diagnostic.fixes[0];
        assert_eq!(fix.applicability, Applicability::MaybeIncorrect);
        let mut fixed = xml.clone();
        for edit in fix.edits.iter().rev() {
            fixed.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        }
        assert!(
            fixed.contains(r#"<invoke><param name="first" location="first"/><param name="third" location="third"/><param name="second" expr="2"/>"#),
            "{fixed}"
        );
        assert!(!codes(&validate(&fixed)).contains("SCXML071"), "{fixed}");
    }

    #[test]
    fn invoke_initialization_data_can_combine_with_inline_source_content() {
        for data in [
            r#"<invoke namelist="first">"#,
            r#"<invoke><param name="second" expr="2"/>"#,
        ] {
            let report = validate(&format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="parent">{data}
                <content><scxml xmlns="{SCXML_NAMESPACE}" initial="child"><final id="child"/></scxml></content>
            </invoke></state></scxml>"#
            ));
            assert!(
                !report
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML016" | "SCXML071")),
                "invoke init data does not select the invoked source: {report:?}"
            );
        }

        for conflicting_invoke in [
            r#"<invoke src="child.scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml"/></content></invoke>"#,
            r#"<invoke srcexpr="child"><content><scxml xmlns="http://www.w3.org/2005/07/scxml"/></content></invoke>"#,
        ] {
            let report = validate(&format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="parent">{conflicting_invoke}</state></scxml>"#
            ));
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.code == "SCXML016"),
                "external and inline invoke sources remain exclusive"
            );
        }
    }

    #[test]
    fn parallel_sibling_exits_and_unreachable_states_do_not_trigger_lint002() {
        // IRP test451: `s1` has no transition, but its sibling region `s0`
        // leaves the whole `<parallel>`, which exits `s1` too.
        let sibling = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="p"><parallel id="p"><state id="s0"><transition target="done"/></state><state id="s1"/></parallel><final id="done"/></scxml>"#
        );
        assert!(!codes(&validate(&sibling)).contains("LINT002"));
        // A region transition whose targets stay inside its own region never
        // exits the sibling regions, so a stuck sibling is still reported.
        let contained = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="p"><parallel id="p"><state id="r1" initial="a"><state id="a"><transition event="e" target="b"/></state><state id="b"/></state><state id="r2"/></parallel></scxml>"#
        );
        let report = validate(&contained);
        let stuck: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "LINT002")
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert_eq!(stuck.len(), 2, "{report:?}");
        // IRP test278: an unreachable state is reported once, by LINT005;
        // "may never exit" would only repeat the same root cause.
        let unreachable = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="a"><state id="a"><transition event="go" target="done"/></state><state id="island"><state id="inner"/></state><final id="done"/></scxml>"#
        );
        let report = validate(&unreachable);
        assert!(codes(&report).contains("LINT005"), "{report:?}");
        assert!(!codes(&report).contains("LINT002"), "{report:?}");
    }

    #[test]
    fn three_invoke_sources_are_one_diagnostic() {
        // vdx/29: `src`, `srcexpr`, and `<content>` together are one mistake
        // (too many sources), not three overlapping pairwise errors.
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="a"><invoke src="c.scxml" srcexpr="'c.scxml'"><content/></invoke></state></scxml>"#
        );
        let report = validate(&xml);
        let found: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML016")
            .collect();
        assert_eq!(found.len(), 1, "{report:?}");
        let diagnostic = found[0];
        assert_eq!(diagnostic.secondary.len(), 2, "{diagnostic:?}");
        assert!(diagnostic.message.contains("three"), "{diagnostic:?}");
    }

    #[test]
    fn nested_scxml_roots_have_independent_id_and_target_scopes() {
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="shared">
                <state id="shared">
                    <transition event="child" target="child-only"/>
                    <invoke><content><scxml xmlns="{SCXML_NAMESPACE}" initial="shared">
                        <state id="shared"><transition event="parent" target="parent-only"/></state>
                        <final id="child-only"/>
                    </scxml></content></invoke>
                </state>
                <final id="parent-only"/>
            </scxml>"#
        );
        let report = validate(&xml);
        assert_eq!(
            report,
            validate(&xml),
            "machine-scoped diagnostics must be byte-position deterministic"
        );

        assert_eq!(
            report
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == "SCXML005")
                .count(),
            2,
            "each machine must reject a target declared only in the other machine"
        );
        assert!(
            !report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "SCXML002"),
            "the repeated `shared` ID is legal across document roots"
        );

        let duplicate = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="same"/><final id="same"/></scxml>"#
        ));
        assert!(
            duplicate
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "SCXML002"),
            "duplicate IDs in one machine remain invalid"
        );
    }

    /// `depth` nested `<scxml>` roots below the outer one.
    #[cfg(not(target_arch = "wasm32"))]
    fn nested_scxml(depth: usize) -> String {
        let mut xml = format!(r#"<scxml xmlns="{SCXML_NAMESPACE}">"#);
        for _ in 0..depth {
            xml.push_str(r#"<scxml xmlns="http://www.w3.org/2005/07/scxml">"#);
        }
        for _ in 0..=depth {
            xml.push_str("</scxml>");
        }
        xml
    }

    /// Natively the inline-machine depth is unbounded: a path one root past
    /// the wasm bound (64) validates without `LIMIT001`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_nested_scxml_validation_has_no_depth_budget() {
        let document = parse_xml(SourceText::new(SOURCE, "nested.scxml", nested_scxml(64)))
            .expect("nested-machine fixture stays inside the XML depth guard");
        let report = validate_semantics(&document);
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code != "LIMIT001"),
            "{report:?}"
        );
    }

    /// SCXML 1.0 §3.5.2: "A conformant SCXML document MUST specify at least
    /// one of 'event', 'cond' or 'target'." Executable content does not
    /// count.
    #[test]
    fn a_transition_with_only_executable_content_is_not_conformant() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="ready"><transition><log label="step" expr="1"/></transition></state></scxml>"#
        ));
        let found: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML004")
            .collect();
        assert_eq!(found.len(), 1, "{report:?}");
        assert_eq!(
            found[0].fixes[0].applicability,
            Applicability::HasPlaceholders
        );
        for legal in [
            r#"<transition event="e"><log label="step"/></transition>"#,
            r#"<transition cond="x"><log label="step"/></transition>"#,
            r#"<transition target="ready"/>"#,
        ] {
            let report = validate(&format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="ready">{legal}</state></scxml>"#
            ));
            assert!(
                !report
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.code == "SCXML004"),
                "{legal}"
            );
        }
    }

    #[test]
    fn exact_official_fixtures_avoid_targeted_semantic_false_positives() {
        let fixtures = [
            (
                "179",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/179/test179.txml"
                ),
            ),
            (
                "240",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/240/test240.txml"
                ),
            ),
            (
                "241",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/241/test241.txml"
                ),
            ),
            (
                "244",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/244/test244.txml"
                ),
            ),
            (
                "245",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/245/test245.txml"
                ),
            ),
            (
                "311",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/311/test311.txml"
                ),
            ),
            (
                "312",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/312/test312.txml"
                ),
            ),
            (
                "554",
                include_str!(
                    "../../../tests/conformance/w3c-irp/upstream/sources/554/test554.txml"
                ),
            ),
        ];

        for (case_id, fixture) in fixtures {
            let report = validate(fixture);
            let forbidden = match case_id {
                "179" => &["SCXML014", "SCXML015"][..],
                "240" | "241" | "244" | "245" | "554" => &["SCXML016"][..],
                "311" | "312" => &["SCXML013"][..],
                _ => &[],
            };
            assert!(
                !report
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| forbidden.contains(&diagnostic.code.as_str())),
                "official fixture {case_id} produced a targeted false positive: {report:?}"
            );
        }
    }

    #[test]
    fn ncname_uses_normative_xml_name_character_boundaries_without_colon() {
        let accepted_starts = [
            'A',
            '_',
            'z',
            '\u{00C0}',
            '\u{00D6}',
            '\u{00D8}',
            '\u{00F6}',
            '\u{00F8}',
            '\u{02FF}',
            '\u{0370}',
            '\u{037D}',
            '\u{037F}',
            '\u{1FFF}',
            '\u{200C}',
            '\u{200D}',
            '\u{2070}',
            '\u{218F}',
            '\u{2C00}',
            '\u{2FEF}',
            '\u{3001}',
            '\u{D7FF}',
            '\u{F900}',
            '\u{FDCF}',
            '\u{FDF0}',
            '\u{FFFD}',
            '\u{10000}',
            '\u{EFFFF}',
        ];
        for character in accepted_starts {
            assert!(
                is_ncname(&format!("{character}tail")),
                "U+{:04X} is an NCNameStartChar",
                character as u32
            );
        }

        let rejected_starts = [
            ':',
            '-',
            '.',
            '0',
            '\u{00B7}',
            '\u{00BF}',
            '\u{00D7}',
            '\u{00F7}',
            '\u{0300}',
            '\u{036F}',
            '\u{037E}',
            '\u{200B}',
            '\u{200E}',
            '\u{206F}',
            '\u{2190}',
            '\u{2FF0}',
            '\u{3000}',
            '\u{FDD0}',
            '\u{FFFE}',
            '\u{F0000}',
        ];
        for character in rejected_starts {
            assert!(
                !is_ncname(&format!("{character}tail")),
                "U+{:04X} is not an NCNameStartChar",
                character as u32
            );
        }

        for character in [
            '-', '.', '0', '\u{00B7}', '\u{0300}', '\u{036F}', '\u{203F}', '\u{2040}',
        ] {
            assert!(
                is_ncname(&format!("a{character}")),
                "U+{:04X} is an NCNameChar",
                character as u32
            );
        }
        for character in [':', '\u{02FF0}', '\u{2041}', '\u{F0000}'] {
            assert!(
                !is_ncname(&format!("a{character}")),
                "U+{:04X} is not an NCNameChar",
                character as u32
            );
        }
    }

    #[test]
    fn typo_suggestions_bound_identifier_candidate_and_aggregate_work() {
        use std::cell::Cell;

        let long_needle = "n".repeat(129);
        let long_needle_pulls = Cell::new(0usize);
        assert_eq!(
            nearest(
                &long_needle,
                ["candidate"].into_iter().inspect(|_| {
                    long_needle_pulls.set(long_needle_pulls.get() + 1);
                })
            ),
            None
        );
        assert_eq!(long_needle_pulls.get(), 0);

        // More candidates than the bound: the search stops and suggests
        // nothing rather than the best of an arbitrary prefix.
        let many_candidates: Vec<String> = (0..=MAX_TYPO_SUGGESTION_CANDIDATES)
            .map(|index| format!("s{index}"))
            .collect();
        let candidate_pulls = Cell::new(0usize);
        assert_eq!(
            nearest(
                "s1x",
                many_candidates.iter().map(String::as_str).inspect(|_| {
                    candidate_pulls.set(candidate_pulls.get() + 1);
                }),
            ),
            None
        );
        assert_eq!(candidate_pulls.get(), MAX_TYPO_SUGGESTION_CANDIDATES + 1);

        // An exhausted work budget also yields no suggestion.
        let mut budget = 10;
        assert_eq!(
            nearest_within("done", ["dune", "done1"].into_iter(), &mut budget),
            None
        );

        assert_eq!(
            nearest("don", ["done", "down", "other"].into_iter()),
            Some("done"),
            "bounded matching must preserve deterministic useful help"
        );
    }

    #[test]
    fn typo_search_finds_the_exact_best_candidate_in_large_machines() {
        let ids: Vec<String> = (0..20_001).map(|index| format!("s{index}")).collect();
        let mut sorted: Vec<&str> = ids.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        assert_eq!(nearest("s1999x", sorted.into_iter()), Some("s1999"));
    }

    #[test]
    fn typo_distance_counts_a_transposition_as_one_edit() {
        let chars = |value: &str| value.chars().collect::<Vec<_>>();
        assert_eq!(osa_distance(&chars("lgo"), &chars("log")), 1);
        assert_eq!(osa_distance(&chars("transiton"), &chars("transition")), 1);
        assert_eq!(osa_distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(osa_distance(&chars(""), &chars("abc")), 3);
    }

    #[test]
    fn event_descriptors_accept_spec_wildcard_and_trailing_dot_forms() {
        let report = validate(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml">
                 <state id="ready">
                   <transition event="error error. error.* .* *" target="done"/>
                   <transition event="error* error.*.more error..more" target="done"/>
                 </state>
                 <final id="done"/>
               </scxml>"#,
        );
        let event_diagnostics: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML013")
            .collect();
        assert_eq!(event_diagnostics.len(), 3);
    }

    /// The official schema's `EventTypes.datatype` builds each token from
    /// `(\i|\d|\-)+` segments (`scxml-datatypes.xsd`), and `\i` (an XML
    /// name start character) includes `:` and `_`; SCXML 1.0 §3.12.1 calls
    /// the characters "alphanumeric" without excluding them. A descriptor
    /// the schema accepts is not rejected.
    #[test]
    fn event_descriptors_accept_every_schema_name_character() {
        let report = validate(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml">
                 <state id="ready">
                   <transition event="a:b ns:event.done x_y-z.1 é.ü" target="done"/>
                   <transition event="a;b" target="done"/>
                 </state>
                 <final id="done"/>
               </scxml>"#,
        );
        let event_diagnostics: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML013")
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        assert_eq!(
            event_diagnostics,
            ["event descriptor `a;b` contains an invalid character"]
        );
    }

    #[test]
    fn initial_history_and_state_relationship_rules_are_precise() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="outer" initial="leaf"><initial><transition event="go" target="outside"/></initial><state id="mid"><state id="leaf"/></state><history id="h"><transition target="leaf"/></history></state><state id="outside"/><state id="atomic" initial="outside"/></scxml>"#
        ));
        let codes = codes(&report);
        for expected in [
            "SCXML010", "SCXML011", "SCXML012", "SCXML017", "SCXML021", "SCXML022",
        ] {
            assert!(codes.contains(expected), "missing {expected}: {codes:?}");
        }
    }

    #[test]
    fn detects_deadlocks_no_exit_and_eventless_cycles_without_faking_invoke_states() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="a"><state id="a"><transition target="b"/></state><state id="b"><transition target="a"/><transition event="c" target="conditional"/><transition event="s" target="stuck"/><transition event="i" target="invoked"/></state><state id="conditional"><transition cond="ready" target="final"/></state><state id="stuck"/><state id="invoked"><invoke src="child.scxml"/></state><final id="final"/></scxml>"#
        ));
        let codes = codes(&report);
        assert!(codes.contains("LINT002"));
        assert!(codes.contains("LINT003"));
        assert!(codes.contains("LINT004"));
        assert!(!report.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == "LINT002" && diagnostic.message.contains("invoked")
        }));
    }

    #[test]
    fn reactive_state_waiting_for_external_events_is_not_a_deadlock() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="waiting"><state id="waiting"><transition event="done" target="final"/><transition event="retry" cond="ready" target="waiting"/></state><final id="final"/></scxml>"#
        ));

        assert!(
            !report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "LINT003")
        );
    }

    #[test]
    fn liveness_rule_records_no_telemetry_of_its_own_on_diagnostic_path() {
        use scxml_syntax::telemetry::{Recorder, install_recorder};

        let document = parse_xml(SourceText::new(
            SOURCE,
            "test.scxml",
            format!(
                r#"<scxml xmlns="{SCXML_NAMESPACE}" initial="conditional"><state id="conditional"><transition cond="ready" target="final"/></state><final id="final"/></scxml>"#
            ),
        ))
        .expect("conditional-only liveness fixture must parse");
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let mut report = Report::new();

        LivenessRule.check(&document, &mut report);

        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "LINT003")
        );
        assert!(
            recorder.observations().is_empty(),
            "the liveness rule runs inside the validate span and records nothing of its own"
        );
    }

    #[test]
    fn executable_content_extensions_are_not_rejected_by_semantic_rules() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" xmlns:x="urn:custom" initial="ready"><state id="ready"><onentry><x:item/></onentry><transition target="done"/></state><final id="done"/></scxml>"#
        ));
        assert!(!report.has_errors());
    }

    #[test]
    fn extension_ids_do_not_define_scxml_targets_or_trigger_scxml_id_rules() {
        let report = validate(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" xmlns:x="urn:custom" initial="ready"><x:item id="not:a:name" initial="ghost"/><state id="ready"><transition event="go" target="ghost"/></state><final id="done"/></scxml>"#
        ));

        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "SCXML005")
        );
        assert!(!report.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == "SCXML003" && diagnostic.message.contains("not:a:name")
        }));
        assert!(!report.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == "SCXML006" && diagnostic.message.contains("ghost")
        }));
    }
}
