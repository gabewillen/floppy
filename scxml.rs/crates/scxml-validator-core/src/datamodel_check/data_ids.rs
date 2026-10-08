//! `LINT015` (warning): an ECMAScript `<data id>` that is not an
//! ECMAScript identifier.
//!
//! SCXML 1.0 §B.2.1: "For each `<data>` element in the document, the SCXML
//! Processor MUST create an ECMAScript variable object whose name is the
//! value of the 'id' attribute of `<data>`." The id is an XML ID, and §5.10
//! reserves only a leading `_` ("A conformant SCXML document MUST NOT
//! contain ids beginning with '_' in the `<data>` element", `SCXML073`), so
//! `retry-count`, `max.tries`, `1st`-style names, and reserved words such as
//! `class` are conformant, and the ECMAScript data-model component
//! (`components/scxml-data-model-ecmascript`, `validate_data_id`) declares
//! each as a property of the global object. No expression can name such a
//! variable, though: `retry-count < 3` subtracts `count` from `retry`. That
//! is the warning. It is reported only for what is certain: an ASCII
//! character outside `[A-Za-z0-9_$]`, a leading ASCII digit, or an
//! ECMA-262 `ReservedWord` of a non-strict script; a non-ASCII letter
//! (`价格`, `é`) is an identifier character and is never reported. An id
//! that is not an XML `NCName` is `SCXML003`'s error instead.
//!
//! The check is always on for `datamodel="ecmascript"` machines (it needs no
//! syntax checker), with a bounded rename fix that edits the declaration and
//! every reference to the name in the machine's expressions.

use std::collections::BTreeSet;

use scxml_syntax::{Attribute, Document, Element};

use super::xpath::decoded_fix;
use super::{ECMASCRIPT_RESERVED_WORDS, SPEC_ECMASCRIPT, Site, collect_sites};
use crate::semantic::{Rule, is_scxml, machine_roots, visit_machine};
use crate::{Applicability, Diagnostic, Fix, FixEdit, Label, Report};

/// Maximum `LINT015` findings per machine.
const MAX_FINDINGS_PER_MACHINE: usize = 64;
/// Maximum reference edits in one rename fix; a larger rename is not offered.
const MAX_RENAME_EDITS: usize = 256;

/// `LINT015`: an ECMAScript `<data id>` the data model cannot declare.
pub(crate) struct EcmaScriptDataIdRule;

impl Rule for EcmaScriptDataIdRule {
    fn code(&self) -> &'static str {
        "LINT015"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            if super::machine_data_model(machine).0 != "ecmascript" {
                continue;
            }
            let declarations = data_ids(machine);
            let declared: BTreeSet<&str> = declarations
                .iter()
                .map(|attribute| attribute.value.as_str())
                .collect();
            let mut sites: Option<Vec<Site<'_>>> = None;
            let mut findings = 0_usize;
            for id in &declarations {
                if !crate::semantic::is_ncname(&id.value) {
                    continue;
                }
                let Some(problem) = problem(&id.value) else {
                    continue;
                };
                if findings == MAX_FINDINGS_PER_MACHINE {
                    break;
                }
                findings += 1;
                let sites = sites.get_or_insert_with(|| collect_sites(document, machine));
                report.push(diagnostic(document, id, problem, &declared, sites));
            }
        }
    }
}

/// `SCXML073`: a `<data id>` that begins with `_` in a machine whose data
/// model is not null (where `<data>` itself is `SCXML037`). SCXML 1.0 §5.10: "Variable
/// names beginning with '_' are reserved for system use. A conformant SCXML
/// document MUST NOT contain ids beginning with '_' in the <data> element."
pub(crate) struct ReservedDataIdRule;

impl Rule for ReservedDataIdRule {
    fn code(&self) -> &'static str {
        "SCXML073"
    }

    fn check(&self, document: &Document, report: &mut Report) {
        for machine in machine_roots(&document.root) {
            if super::machine_data_model(machine).0 == "null" {
                continue;
            }
            for id in data_ids(machine)
                .into_iter()
                .filter(|id| id.value.starts_with('_'))
                .take(MAX_FINDINGS_PER_MACHINE)
            {
                let mut diagnostic = Diagnostic::error(
                    self.code(),
                    format!("`<data id>` `{}` begins with `_`", id.value),
                    id.value_span,
                )
                .with_primary_message("reserved for system variables")
                .with_note("SCXML reserves names that begin with `_` for system variables such as `_event`, `_sessionid`, `_name`, and `_ioprocessors`, and a conformant document declares no data with such an id")
                .with_help("rename the data without the leading `_`, and update every reference to it")
                .with_spec_reference("SCXML 1.0 §5.10 System Variables");
                let renamed = id.value.trim_start_matches('_');
                if !renamed.is_empty()
                    && let Some(source) = document.source.slice(id.value_span)
                    && source == id.value
                {
                    diagnostic = diagnostic.with_fix(Fix::replace(
                        format!("rename the declaration to `{renamed}`"),
                        Applicability::MaybeIncorrect,
                        id.value_span,
                        renamed,
                    ));
                }
                report.push(diagnostic);
            }
        }
    }
}

/// Every `<data id>` attribute of `machine`, in document order.
fn data_ids(machine: &Element) -> Vec<&Attribute> {
    let mut ids = Vec::new();
    visit_machine(machine, &mut |element| {
        if is_scxml(element, "data")
            && let Some(id) = element.attribute("id")
        {
            ids.push(id);
        }
    });
    ids
}

/// Why no ECMAScript expression can name a `<data id>` variable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Problem {
    /// An ASCII character other than a letter, digit, `_`, or `$`.
    Character(char),
    /// Starts with an ASCII digit.
    LeadingDigit,
    /// An ECMA-262 `ReservedWord` of a non-strict script.
    Reserved,
}

/// Why `name` cannot be written as an ECMAScript identifier, or `None` when
/// it can or when that is not certain: only ASCII characters are judged
/// (every non-ASCII letter is an identifier character, and the data model's
/// parser is the authority for the rest), and a leading `_` is `SCXML073`'s.
pub(crate) fn problem(name: &str) -> Option<Problem> {
    let first = name.chars().next()?;
    if first == '_' {
        return None;
    }
    if let Some(character) = name.chars().find(|character| {
        character.is_ascii()
            && !(character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
    }) {
        return Some(Problem::Character(character));
    }
    if first.is_ascii_digit() {
        return Some(Problem::LeadingDigit);
    }
    ECMASCRIPT_RESERVED_WORDS
        .contains(&name)
        .then_some(Problem::Reserved)
}

/// A variable name for `name` that is also an XML `NCName`, so the rename
/// clears `LINT015` without introducing `SCXML003`: separators (including
/// `$`, which `NCName` excludes) become camelCase boundaries (`retry-count`
/// to `retryCount`, `$price` to `price`), leading `_` is dropped, a leading
/// digit gets a `v` prefix (`1st` to `v1st`), and a keyword gets a `Value`
/// suffix. `None` when no valid, unused name results, including when
/// nothing ASCII survives (`é`, `价格`, `_`, the `…` placeholder): an empty
/// name is neither a variable nor an XML ID.
pub(crate) fn rename(name: &str, declared: &BTreeSet<&str>) -> Option<String> {
    let mut renamed = String::with_capacity(name.len());
    let mut upper = false;
    for character in name.trim_start_matches(['_', '$']).chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if upper && !renamed.is_empty() {
                renamed.push(character.to_ascii_uppercase());
            } else {
                renamed.push(character);
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    if renamed.starts_with(|first: char| first.is_ascii_digit()) {
        renamed.insert(0, 'v');
    }
    if ECMASCRIPT_RESERVED_WORDS.contains(&renamed.as_str()) {
        renamed.push_str("Value");
    }
    (!renamed.is_empty()
        && problem(&renamed).is_none()
        && renamed != name
        && !declared.contains(renamed.as_str()))
    .then_some(renamed)
}

/// Decoded ranges of every reference to `name` in `source`: occurrences at
/// identifier boundaries, outside string and template literals, and not
/// after `.` (a property of another object). One bounded pass.
pub(super) fn references(source: &str, name: &str) -> Vec<(usize, usize)> {
    let is_name_char =
        |character: char| character.is_ascii_alphanumeric() || matches!(character, '_' | '$');
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if let Some(open) = quote {
            if byte == b'\\' {
                index = index.saturating_add(2);
                continue;
            }
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        let end = index + name.len();
        let before = source
            .get(..index)
            .and_then(|head| head.chars().next_back());
        let after = source.get(end..).and_then(|tail| tail.chars().next());
        if source.get(index..end) == Some(name)
            && before.is_none_or(|before| !(is_name_char(before) || before == '.'))
            && after.is_none_or(|after| !is_name_char(after))
        {
            found.push((index, end));
            index = end;
            continue;
        }
        index += 1;
    }
    found
}

/// The rename fix: the declaration and every reference in `sites`, or
/// `None` when a reference cannot be mapped exactly onto the source.
fn rename_fix(
    document: &Document,
    id: &Attribute,
    renamed: &str,
    sites: &[Site<'_>],
) -> Option<Fix> {
    let mut edits = vec![FixEdit {
        span: id.value_span,
        replacement: renamed.to_owned(),
    }];
    for site in sites {
        for (start, end) in references(&site.source, &id.value) {
            let fix = decoded_fix(document, site, start, end, String::new(), renamed)?;
            edits.extend(fix.edits);
            if edits.len() > MAX_RENAME_EDITS {
                return None;
            }
        }
    }
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    edits.dedup_by_key(|edit| (edit.span.start, edit.span.end));
    let references = edits.len() - 1;
    Some(Fix {
        message: if references == 0 {
            format!("rename the variable to `{renamed}`")
        } else {
            format!(
                "rename the variable to `{renamed}` here and in {references} reference{}",
                if references == 1 { "" } else { "s" }
            )
        },
        applicability: Applicability::MaybeIncorrect,
        edits,
    })
}

fn diagnostic(
    document: &Document,
    id: &Attribute,
    problem: Problem,
    declared: &BTreeSet<&str>,
    sites: &[Site<'_>],
) -> Diagnostic {
    let name = id.value.as_str();
    let (message, label) = match problem {
        Problem::Character(character) => (
            format!("`{name}` is not an ECMAScript identifier: it contains `{character}`"),
            format!("`{character}` cannot appear in an identifier"),
        ),
        Problem::LeadingDigit => (
            format!("`{name}` is not an ECMAScript identifier: it starts with a digit"),
            "an identifier cannot start with a digit".to_owned(),
        ),
        Problem::Reserved => (
            format!("`{name}` is an ECMAScript reserved word, not an identifier"),
            "reserved word".to_owned(),
        ),
    };
    let renamed = rename(name, declared);
    let fix = renamed
        .as_deref()
        .and_then(|renamed| rename_fix(document, id, renamed, sites));
    let reference = sites.iter().find_map(|site| {
        references(&site.source, name)
            .first()
            .map(|(start, end)| (site, site.span_of(*start, *end)))
    });
    let access = format!("globalThis['{name}']");
    let mut diagnostic = Diagnostic::warning("LINT015", message, id.value_span)
        .with_primary_message(label)
        .with_note(match problem {
            Problem::Character('-') => format!(
                "the ECMAScript data model creates this variable (SCXML 1.0 §B.2.1) as a property of the global object, reachable only as `{access}`; `{name}` in an expression is not this variable: `-` subtracts, so `{name}` reads two variables"
            ),
            Problem::Character('.') => format!(
                "the ECMAScript data model creates this variable (SCXML 1.0 §B.2.1) as a property of the global object, reachable only as `{access}`; `{name}` in an expression is a property access, not this variable"
            ),
            _ => format!(
                "the ECMAScript data model creates this variable (SCXML 1.0 §B.2.1) as a property of the global object, reachable only as `{access}`; no expression or `<assign location>` can name it directly"
            ),
        })
        .with_help(match &renamed {
            Some(renamed) => format!(
                "to use the variable in expressions, name it with an identifier that is also an XML ID, such as `{renamed}`, and rename every reference to it"
            ),
            None => "to use the variable in expressions, name it with an identifier that is also an XML ID (letters, digits, and `_`, starting with a letter), and rename every reference to it".to_owned(),
        })
        .with_spec_reference(SPEC_ECMASCRIPT)
        .with_spec_reference("SCXML 1.0 §5.3 <data>");
    if let Some((_, span)) = reference {
        diagnostic = diagnostic
            .with_secondary(Label::new(span).with_message(format!("`{name}` is referenced here")));
    }
    if let Some(fix) = fix {
        diagnostic = diagnostic.with_fix(fix);
    }
    diagnostic
}

/// The `SCXML003` repair of an ECMAScript `<data id>` that the data model can
/// declare but XML cannot (`$price`): the [`rename`] of the declaration and
/// of every reference to it in `machine`'s expressions, so the repaired
/// name is both an XML ID and the same variable everywhere.
pub(crate) fn ncname_rename_fix(
    document: &Document,
    machine: &Element,
    id: &Attribute,
    declared: &BTreeSet<&str>,
) -> Option<Fix> {
    let renamed = rename(&id.value, declared)?;
    let sites = collect_sites(document, machine);
    rename_fix(document, id, &renamed, &sites)
}

/// The `<data id>` declaring `name` in the machine that owns `element`, when
/// `name` is a data ID no ECMAScript expression can name
/// ([`EcmaScriptDataIdRule`] reports it), with its rename.
pub(super) fn undeclarable_data<'a>(
    document: &'a Document,
    element: &Element,
    name: &str,
) -> Option<(&'a Attribute, Option<String>)> {
    problem(name)?;
    let ids = owning_machine_ids(document, element)?;
    let declared: BTreeSet<&str> = ids.iter().map(|id| id.value.as_str()).collect();
    let id = ids.iter().find(|id| id.value == name)?;
    Some((id, rename(name, &declared)))
}

/// The `<data id>` attributes of the machine that owns `element`.
fn owning_machine_ids<'a>(document: &'a Document, element: &Element) -> Option<Vec<&'a Attribute>> {
    machine_roots(&document.root)
        .into_iter()
        .find_map(|machine| {
            let mut owns = false;
            visit_machine(machine, &mut |candidate| {
                if std::ptr::eq(candidate, element) {
                    owns = true;
                }
            });
            owns.then(|| data_ids(machine))
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::rename;

    #[test]
    fn rename_never_derives_an_empty_name() {
        let declared = BTreeSet::new();
        for name in ["é", "价格", "_", "__", "$", "…", "-", "_é"] {
            assert_eq!(rename(name, &declared), None, "{name}");
        }
        assert_eq!(
            rename("retry-count", &declared).as_deref(),
            Some("retryCount")
        );
        assert_eq!(rename("_total", &declared).as_deref(), Some("total"));
        assert_eq!(rename("价格2", &declared).as_deref(), Some("v2"));
    }
}
