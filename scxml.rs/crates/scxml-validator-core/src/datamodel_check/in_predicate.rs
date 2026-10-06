//! `SCXML058`: `In(stateID)` calls that can never be true.
//!
//! `In()` is true only while a state with the given ID is active (SCXML 1.0
//! §5.9.1), so a literal argument that names no `<state>`, `<parallel>`, or
//! `<final>` of the machine is always false, and in ECMAScript and `XPath` an
//! unquoted state ID is a variable or a path, not the ID. The check is
//! data-model-neutral text scanning over condition sites: one bounded pass
//! per `cond`, skipping string literals, with a bounded did-you-mean.

use std::collections::{BTreeMap, BTreeSet};

use scxml_syntax::{Document, Element};

use super::xpath::decoded_fix;
use super::xpath::references::is_ncname;
use super::{ExpressionKind, KNOWN_DATA_MODELS, Site, collect_sites};
use crate::semantic::{Rule, child_elements, is_scxml, machine_roots, nearest, visit_machine};
use crate::{Diagnostic, DiagnosticCategory, Label, Report};

/// Maximum `In()` findings per condition.
const MAX_FINDINGS_PER_CONDITION: usize = 4;

/// `SCXML058`: an `In()` argument that names no state of the machine.
pub(crate) struct InPredicateRule;

impl Rule for InPredicateRule {
    fn code(&self) -> &'static str {
        "SCXML058"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            let data_model = Some(super::machine_data_model(machine).0);
            if data_model.is_some_and(|value| !KNOWN_DATA_MODELS.contains(&value)) {
                continue;
            }
            let states = StateIds::of(machine);
            for site in collect_sites(document, machine) {
                if site.kind != ExpressionKind::Condition {
                    continue;
                }
                for call in in_calls(&site.source)
                    .into_iter()
                    .take(MAX_FINDINGS_PER_CONDITION)
                {
                    if let Some(diagnostic) =
                        call_finding(document, &site, data_model, &states, &call)
                    {
                        report.push(diagnostic);
                    }
                }
            }
        }
    }
}

/// IDs of the machine's states, its `<history>` pseudo-states (with their
/// parent state's ID), and its declared `<data>` variables.
pub(super) struct StateIds {
    states: BTreeSet<String>,
    histories: BTreeMap<String, String>,
    variables: BTreeSet<String>,
}

impl StateIds {
    pub(super) fn of(machine: &Element) -> Self {
        let mut ids = Self {
            states: BTreeSet::new(),
            histories: BTreeMap::new(),
            variables: BTreeSet::new(),
        };
        visit_machine(machine, &mut |element| {
            let Some(id) = element.attribute("id") else {
                return;
            };
            if is_scxml(element, "state")
                || is_scxml(element, "parallel")
                || is_scxml(element, "final")
            {
                ids.states.insert(id.value.clone());
                for history in child_elements(element).filter(|child| is_scxml(child, "history")) {
                    if let Some(history_id) = history.attribute("id") {
                        ids.histories
                            .insert(history_id.value.clone(), id.value.clone());
                    }
                }
            } else if is_scxml(element, "data") {
                ids.variables.insert(id.value.clone());
            }
        });
        ids
    }
}

/// One `In(argument)` call with a single literal or bare-name argument.
struct InCall {
    /// Decoded range of the argument, quotes included.
    start: usize,
    end: usize,
    /// The state ID the argument spells (without quotes).
    id: String,
    /// Whether the argument is a quoted string literal.
    quoted: bool,
}

/// `In('id')`, `In("id")`, and `In(id)` calls outside string literals.
fn in_calls(source: &str) -> Vec<InCall> {
    let bytes = source.as_bytes();
    let mut calls = Vec::new();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        let boundary = index == 0
            || bytes.get(index - 1).is_some_and(|before| {
                !(before.is_ascii_alphanumeric()
                    || matches!(before, b'_' | b'.' | b'$' | b'/' | b':' | b'@' | b'-'))
            });
        if boundary
            && source
                .get(index..)
                .is_some_and(|rest| rest.starts_with("In"))
            && let Some(call) = in_call(source, index + 2)
        {
            index = call.end;
            calls.push(call);
            continue;
        }
        index += 1;
    }
    calls
}

/// Parses `( argument )` after `In` at `after`.
fn in_call(source: &str, after: usize) -> Option<InCall> {
    let skip = |offset: usize| {
        let rest = source.get(offset..).unwrap_or_default();
        offset + (rest.len() - rest.trim_start().len())
    };
    let open = skip(after);
    if source.get(open..open + 1)? != "(" {
        return None;
    }
    let start = skip(open + 1);
    let rest = source.get(start..)?;
    let first = rest.chars().next()?;
    let (end, id, quoted) = if matches!(first, '\'' | '"') {
        let length = rest.get(1..)?.find(first)?;
        (start + length + 2, rest.get(1..=length)?.to_owned(), true)
    } else {
        let length = rest
            .char_indices()
            .find(|(_, character)| {
                !(character.is_alphanumeric() || matches!(character, '_' | '-' | '.'))
            })
            .map_or(rest.len(), |(index, _)| index);
        let id = rest.get(..length)?;
        if !is_ncname(id) {
            return None;
        }
        (start + length, id.to_owned(), false)
    };
    let close = skip(end);
    (source.get(close..close + 1)? == ")").then_some(InCall {
        start,
        end,
        id,
        quoted,
    })
}

fn call_finding(
    document: &Document,
    site: &Site<'_>,
    data_model: Option<&str>,
    states: &StateIds,
    call: &InCall,
) -> Option<Diagnostic> {
    let id = call.id.as_str();
    let known = states.states.contains(id);
    let span = site.span_of(call.start, call.end);
    let (message, primary, note, help, fix) = if call.quoted {
        if known {
            return None;
        }
        if let Some(parent) = states.histories.get(id) {
            // W3C IRP 580 relies on exactly this: a history pseudo-state
            // is never part of the configuration.
            return Some(finish(
                site,
                span,
                format!("`In()` names the `<history>` `{id}`, which is never an active state"),
                "a history pseudo-state",
                "a `<history>` only records and restores its parent's configuration and is never itself in the configuration, so `In()` of it is always false".to_owned(),
                format!("to test whether the history's parent is active, use `In('{parent}')`"),
                None,
            ));
        }
        let suggestion = nearest(id, states.states.iter().map(String::as_str));
        let (start, end) = (call.start + 1, call.end - 1);
        (
            format!("`In()` names `{id}`, which is not a state of this machine"),
            "no state has this ID",
            "`In()` is true only while a `<state>`, `<parallel>`, or `<final>` with this ID is active, so this condition is always false".to_owned(),
            match suggestion {
                Some(suggestion) => format!("did you mean `{suggestion}`?"),
                None => "name the ID of a `<state>`, `<parallel>`, or `<final>` of this machine".to_owned(),
            },
            suggestion.and_then(|suggestion| {
                decoded_fix(
                    document,
                    site,
                    start,
                    end,
                    format!("use the state ID `{suggestion}`"),
                    suggestion,
                )
            }),
        )
    } else {
        // ECMAScript or XPath: an unquoted argument is a variable or a
        // path. A declared variable may hold a state ID, so only a name that
        // is a state ID and no variable is reported.
        if !known
            || states.variables.contains(id)
            || !matches!(data_model, Some("ecmascript" | "xpath"))
        {
            return None;
        }
        let meaning = match data_model {
            Some("xpath") => format!(
                "in XPath, `{id}` is a relative path from `<datamodel>` that selects no node, so `In()` receives the empty string"
            ),
            _ => format!(
                "in ECMAScript, `{id}` is a variable reference, not the state ID (no `<data id=\"{id}\">` declares it, so evaluating it raises `error.execution` unless a script defines it)"
            ),
        };
        (
            format!("`In({id})` passes `{id}` unquoted, not the state ID `'{id}'`"),
            "not a string",
            format!("{meaning}; `In()` takes the state ID as a string"),
            format!("quote the state ID: `In('{id}')`"),
            decoded_fix(
                document,
                site,
                call.start,
                call.end,
                format!("quote the state ID `'{id}'`"),
                &format!("'{id}'"),
            ),
        )
    };
    Some(finish(site, span, message, primary, note, help, fix))
}

/// Builds one `SCXML058` finding.
fn finish(
    site: &Site<'_>,
    span: scxml_syntax::Span,
    message: String,
    primary: &str,
    note: String,
    help: String,
    fix: Option<crate::Fix>,
) -> Diagnostic {
    let what = &site.element.name.local_name;
    let mut diagnostic = Diagnostic::warning("SCXML058", message, span)
        .with_category(DiagnosticCategory::Lint)
        .with_primary_message(primary)
        .with_note(note)
        .with_help(help)
        .with_spec_reference("SCXML 1.0 §5.9.1 Conditional Expressions");
    if let Some(fix) = fix {
        diagnostic = diagnostic.with_fix(fix);
    }
    if span != site.value_span {
        diagnostic = diagnostic.with_secondary(
            Label::new(site.value_span).with_message(format!("in this `cond` on `<{what}>`")),
        );
    }
    diagnostic
}

/// `SCXML079` for `cond="In(s)"` under `datamodel="null"`: the null data
/// model's only condition takes the state ID as a quoted string, and its
/// component rejects the unquoted form at run time. The fix quotes the ID
/// (machine-applicable when it names a state; otherwise the nearest state ID,
/// maybe-incorrect); `SCXML058` does not also judge the unquoted argument.
pub(super) fn unquoted_null_in(
    document: &Document,
    datamodel: &scxml_syntax::Attribute,
    states: &StateIds,
    site: &Site<'_>,
) -> Option<Diagnostic> {
    let source = site.source.as_str();
    let leading = source.len() - source.trim_start().len();
    if !source.get(leading..)?.starts_with("In") {
        return None;
    }
    let call = in_call(source, leading + 2)?;
    if call.quoted || source.get(call.end..)?.trim() != ")" {
        return None;
    }
    let id = call.id.as_str();
    let span = site.span_of(call.start, call.end);
    let (help, fix) = if states.states.contains(id) {
        (
            format!("quote the state ID: `In('{id}')`"),
            decoded_fix(
                document,
                site,
                call.start,
                call.end,
                format!("quote the state ID `'{id}'`"),
                &format!("'{id}'"),
            )
            .map(|mut fix| {
                fix.applicability = crate::Applicability::MachineApplicable;
                fix
            }),
        )
    } else {
        let target = states
            .histories
            .get(id)
            .map(String::as_str)
            .or_else(|| nearest(id, states.states.iter().map(String::as_str)));
        match target {
            Some(target) => (
                if states.histories.contains_key(id) {
                    format!(
                        "`{id}` is a `<history>`, which is never active; to test whether its parent is active, write `In('{target}')`"
                    )
                } else {
                    format!("`{id}` is not a state of this machine; did you mean `In('{target}')`?")
                },
                decoded_fix(
                    document,
                    site,
                    call.start,
                    call.end,
                    format!("use the quoted state ID `'{target}'`"),
                    &format!("'{target}'"),
                ),
            ),
            None => (
                format!(
                    "`{id}` is not a state of this machine: quote the ID of a `<state>`, `<parallel>`, or `<final>`, for example `In('stateID')`"
                ),
                None,
            ),
        }
    };
    let mut diagnostic = Diagnostic::warning(
        "SCXML079",
        format!(
            "`In({id})` passes `{id}` unquoted, but the null data model takes the state ID as a quoted string"
        ),
        span,
    )
    .with_primary_message("not a quoted string")
    .with_secondary(
        Label::new(datamodel.value_span).with_message("the null data model is selected here"),
    )
    .with_note("the null data model's only condition is `In('stateID')` with the state ID in single or double quotes; it rejects any other condition, `In(stateID)` without quotes included, so evaluating it raises `error.execution` and the condition is false")
    .with_help(help)
    .with_spec_reference(super::SPEC_NULL);
    if let Some(fix) = fix {
        diagnostic = diagnostic.with_fix(fix);
    }
    Some(diagnostic)
}

#[cfg(test)]
mod tests;
