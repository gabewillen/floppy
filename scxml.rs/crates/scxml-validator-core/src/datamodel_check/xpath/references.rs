//! `XPath` variable and path reference lints (SCXML CR 2014 §C.3.1).
//!
//! In the `XPath` data model every `<data>` is a child `<data id="…">` of the
//! `<datamodel>` context node and is bound to the `XPath` variable of the same
//! name; `<foreach item/index>` and `idlocation` names declare variables
//! when they run. A fragment that parses can still be certain to fail or to
//! select nothing:
//!
//! * `SCXML056` (error): `$name` names no declared variable, so evaluating
//!   it raises `error.execution`. `$user.name` is one variable named
//!   `user.name` (`.` is a name character), repaired to `$user/name`; a typo
//!   gets a bounded did-you-mean.
//! * `LINT011` (warning): a relative path at the top level of the fragment
//!   selects children of `<datamodel>` by element name, so `count`, `true`,
//!   or `_event.data.ok` select no node: `$count`, `true()`, and
//!   `$_event/data/ok` were meant. Unless the machine has a `<foreach>`
//!   (whose item binds an element of any name under `<datamodel>`), the
//!   children are exactly `data`, `_sessionid`, `_name`, `_ioprocessors`,
//!   and `_event`, so any other first step selects nothing.
//! * `LINT013` (warning): a numeric predicate that is not a positive
//!   integer (`$list/item[0]`) selects nothing, because positions start at
//!   1; `[0]` is repaired to `[1]`.
//! * `LINT014` (warning): `+` with a non-numeric string literal operand
//!   (`'a' + $b`) is numeric addition and yields `NaN`; an additive chain
//!   without subtraction is repaired to `concat(…)`.
//!
//! Tokens come from one bounded pass that applies the `XPath` 1.0 lexical
//! disambiguation rules (§3.7) to a fragment the checker already accepted.

use std::collections::BTreeSet;

use scxml_syntax::{Document, Element};

use super::decoded_fix;
use crate::datamodel_check::{ExpressionKind, MAX_ISSUES_PER_EXPRESSION, Site};
use crate::semantic::{is_scxml, nearest, visit_machine};
use crate::{Applicability, Diagnostic, DiagnosticCategory, Fix, Label};

/// System variables the `XPath` data model binds (SCXML 5.10).
const SYSTEM_VARIABLES: [&str; 4] = ["_event", "_ioprocessors", "_name", "_sessionid"];

/// Element children of `<datamodel>`. A `<foreach>` item is not one: the
/// item variable denotes the collection's own node (SCXML 4.6).
const DATAMODEL_CHILDREN: [&str; 5] = ["_event", "_ioprocessors", "_name", "_sessionid", "data"];

const SPEC_XPATH_DATA: &str = "SCXML CR 2014 §C.3.1 Data Model";

/// Variables and bindings of one machine, for its `XPath` fragments.
pub(in crate::datamodel_check) struct Scope {
    /// Declared variable names: `<data id>`, `<foreach item/index>`, and
    /// `idlocation` names, in sorted order.
    variables: BTreeSet<String>,
    /// Names a `<foreach item/index>` writes as `$name` although nothing
    /// declares `name`: the `$` is the mistake (`item="$it"` assigns to an
    /// undeclared variable instead of binding `it`), so the name counts as
    /// declared and only the binding site is reported.
    dollar_bindings: BTreeSet<String>,
}

impl Scope {
    /// Collects the variables `machine` declares (nested machines excluded).
    pub(in crate::datamodel_check) fn of(machine: &Element) -> Self {
        let mut variables = BTreeSet::new();
        let mut dollar_bindings = BTreeSet::new();
        visit_machine(machine, &mut |element| {
            let names: &[&str] = if is_scxml(element, "data") {
                &["id"]
            } else if is_scxml(element, "foreach") {
                &["item", "index"]
            } else if is_scxml(element, "send") || is_scxml(element, "invoke") {
                &["idlocation"]
            } else {
                &[]
            };
            for name in names {
                if let Some(attribute) = element.attribute(name) {
                    let value = attribute.value.trim();
                    if is_ncname(value) {
                        variables.insert(value.to_owned());
                    } else if let Some(bound) = dollar_binding(element, name, value) {
                        dollar_bindings.insert(bound.to_owned());
                    }
                }
            }
        });
        dollar_bindings.retain(|name| !variables.contains(name));
        variables.extend(dollar_bindings.iter().cloned());
        Self {
            variables,
            dollar_bindings,
        }
    }

    fn declares(&self, name: &str) -> bool {
        self.variables.contains(name) || SYSTEM_VARIABLES.contains(&name)
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.variables.iter().map(String::as_str)
    }
}

/// The name of a `<foreach item/index>` written as `$name`.
fn dollar_binding<'a>(element: &Element, attribute: &str, value: &'a str) -> Option<&'a str> {
    (is_scxml(element, "foreach") && matches!(attribute, "item" | "index"))
        .then(|| value.strip_prefix('$'))
        .flatten()
        .filter(|name| is_ncname(name))
}

/// `SCXML039` for `<foreach item="$it">` when nothing declares `it`: the
/// item names a variable, written without `$`.
fn dollar_binding_finding(document: &Document, site: &Site<'_>, name: &str) -> Option<Diagnostic> {
    let start = site.source.find('$')?;
    let what = site.what.as_str();
    let fix = decoded_fix(
        document,
        site,
        start,
        start + 1,
        "remove the `$`".to_owned(),
        "",
    )?;
    Some(
        Diagnostic::warning(
            "SCXML039",
            format!("`<foreach {what}>` names the variable `{name}`, which is written without `$`"),
            site.span_of(start, start + 1),
        )
        .with_primary_message("`$` is not part of the name")
        .with_secondary(Label::new(site.value_span).with_message(format!("in this `{what}`")))
        .with_note(format!(
            "`<foreach>` binds each item (and its position) to the variable named by `{what}`, declaring it when needed, and expressions then read it as `${name}`; `{what}=\"${name}\"` is instead a location that assigns to an existing variable `${name}`, and none is declared, so the loop raises `error.execution`"
        ))
        .with_help(format!("write the variable name without `$`: `{what}=\"{name}\"`"))
        .with_fix(Fix {
            applicability: Applicability::MachineApplicable,
            ..fix
        })
        .with_spec_reference("SCXML 1.0 §4.6 <foreach>")
        .with_spec_reference("SCXML CR 2014 §C.3.10 <foreach>"),
    )
}

/// Reference findings for one fragment the `XPath` checker accepted.
pub(in crate::datamodel_check) fn lint(
    document: &Document,
    site: &Site<'_>,
    scope: &Scope,
) -> Vec<Diagnostic> {
    if site.kind == ExpressionKind::Script {
        return Vec::new();
    }
    if matches!(site.what.as_str(), "item" | "index")
        && let Some(name) = dollar_binding(site.element, &site.what, site.source.trim())
        && scope.dollar_bindings.contains(name)
    {
        return dollar_binding_finding(document, site, name)
            .into_iter()
            .collect();
    }
    let Some(tokens) = tokenize(&site.source) else {
        return Vec::new();
    };
    // `<foreach item/index>` and `idlocation` bind a plain name as a
    // variable (declaring it), so a bare name there is not a path.
    let binds_name = matches!(site.what.as_str(), "item" | "index" | "idlocation")
        && is_ncname(site.source.trim());
    let mut findings = Vec::new();
    let mut depth = 0_usize;
    // Token index after the last `+` chain already reported.
    let mut chain_end = 0_usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::OpenBracket => {
                if let Some(diagnostic) = positional_finding(document, site, &tokens, index) {
                    findings.push(diagnostic);
                }
                depth += 1;
            }
            Kind::Operator
                if index >= chain_end && site.source.get(token.start..token.end) == Some("+") =>
            {
                if let Some((diagnostic, end)) =
                    string_addition_finding(document, site, &tokens, index)
                {
                    findings.push(diagnostic);
                    chain_end = end;
                }
            }
            Kind::CloseBracket => depth = depth.saturating_sub(1),
            Kind::Variable => {
                if let Some(diagnostic) = variable_finding(document, site, scope, token) {
                    findings.push(diagnostic);
                }
            }
            Kind::Name if !binds_name && depth == 0 && starts_relative_path(&tokens, index) => {
                if let Some(diagnostic) = path_finding(document, site, scope, &tokens, index) {
                    findings.push(diagnostic);
                }
            }
            _ => {}
        }
        if findings.len() >= MAX_ISSUES_PER_EXPRESSION {
            break;
        }
    }
    findings
}

/// `$name` that names no declared variable.
fn variable_finding(
    document: &Document,
    site: &Site<'_>,
    scope: &Scope,
    token: &Token,
) -> Option<Diagnostic> {
    let name = site.source.get(token.start + 1..token.end)?;
    if name.contains(':') || scope.declares(name) {
        return None;
    }
    let span = site.span_of(token.start, token.end);
    let what = site.what.as_str();
    // `$user.name`: `.` is a name character, so this is one variable.
    let member = name
        .split_once('.')
        .filter(|(head, rest)| scope.declares(head) && !rest.is_empty());
    // `$a1-1`: `-` is a name character too, so the subtraction was meant.
    // The longest declared prefix that ends before a `-` wins.
    let subtraction = name
        .match_indices('-')
        .map(|(at, _)| at)
        .filter_map(|at| Some((name.get(..at)?, name.get(at + 1..)?)))
        .rfind(|(head, rest)| scope.declares(head) && !rest.is_empty());
    let (message, help, fix) = if let Some((head, rest)) = subtraction {
        let expression = format!("${head} - {rest}");
        (
            format!(
                "`${name}` is one undeclared variable named `{name}`, not `${head}` minus `{rest}`"
            ),
            format!(
                "`-` is part of an XPath name, so `${name}` is one variable; put spaces around `-` to subtract: `{expression}`"
            ),
            decoded_fix(
                document,
                site,
                token.start,
                token.end,
                format!("subtract with `{expression}`"),
                &expression,
            ),
        )
    } else if let Some((head, rest)) = member {
        let path = format!("${head}/{}", rest.replace('.', "/"));
        (
            format!(
                "`${name}` is one undeclared variable named `{name}`, not a member of `${head}`"
            ),
            format!("`.` is part of an XPath name; select the child element with a path: `{path}`"),
            decoded_fix(
                document,
                site,
                token.start,
                token.end,
                format!("select the child with `{path}`"),
                &path,
            ),
        )
    } else if let Some(suggestion) = nearest(name, scope.names()) {
        (
            format!("`${name}` is not a declared XPath variable"),
            format!("did you mean `${suggestion}`?"),
            decoded_fix(
                document,
                site,
                token.start + 1,
                token.end,
                format!("use the declared variable `${suggestion}`"),
                suggestion,
            ),
        )
    } else {
        (
            format!("`${name}` is not a declared XPath variable"),
            format!(
                "declare it in the machine's `<datamodel>` as `<data id=\"{name}\"/>`, or use a declared variable"
            ),
            None,
        )
    };
    let mut diagnostic = Diagnostic::warning("SCXML056", message, span)
        .with_primary_message("undeclared variable")
        .with_note("an XPath variable exists only for a declared `<data id>` (and `<foreach item/index>` or `idlocation` names once they run) or a system variable; evaluating an undeclared one raises `error.execution`")
        .with_help(help)
        .with_spec_reference(SPEC_XPATH_DATA)
        .with_spec_reference("SCXML 1.0 §5.10 System Variables");
    if let Some(fix) = fix {
        diagnostic = diagnostic.with_fix(fix);
    }
    if span != site.value_span {
        diagnostic = diagnostic
            .with_secondary(Label::new(site.value_span).with_message(format!("in this `{what}`")));
    }
    Some(diagnostic)
}

/// Whether the name at `index` is the first step of a relative location
/// path evaluated against `<datamodel>` (not a step after `/`, an axis, or
/// `@`, and not the argument of `In()`, which `LINT012` owns).
fn starts_relative_path(tokens: &[Token], index: usize) -> bool {
    let previous = index.checked_sub(1).and_then(|before| tokens.get(before));
    let first = previous
        .is_none_or(|token| matches!(token.kind, Kind::Operator | Kind::OpenParen | Kind::Comma));
    let in_argument = previous.is_some_and(|token| token.kind == Kind::OpenParen)
        && index
            .checked_sub(2)
            .and_then(|before| tokens.get(before))
            .is_some_and(|token| token.kind == Kind::Function && token.text == "In");
    first && !in_argument
}

/// A relative path whose first step selects no `<datamodel>` child.
fn path_finding(
    document: &Document,
    site: &Site<'_>,
    scope: &Scope,
    tokens: &[Token],
    index: usize,
) -> Option<Diagnostic> {
    let token = tokens.get(index)?;
    let name = token.text.as_str();
    if name.contains(':') || DATAMODEL_CHILDREN.contains(&name) {
        return None;
    }
    let next = tokens.get(index + 1).map(|token| token.kind);
    let single_step = !matches!(next, Some(Kind::Slash | Kind::OpenBracket));
    let what = site.what.as_str();
    let span = site.span_of(token.start, token.end);
    // (message, help, replacement): the repair keeps the path's remaining
    // steps, so only the first step is replaced.
    let repair: Option<(String, String, String)> = if scope.variables.contains(name) {
        Some((
            format!("`{name}` selects a child element named `{name}`, not the variable `${name}`"),
            format!(
                "`<data id=\"{name}\">` is bound to the variable `${name}`; a bare name is a relative path from `<datamodel>`"
            ),
            format!("${name}"),
        ))
    } else if single_step
        && matches!(name, "true" | "false")
        && site.kind != ExpressionKind::Location
    {
        Some((
            format!("`{name}` selects a child element named `{name}`, not the boolean `{name}()`"),
            format!("XPath writes the boolean constants as function calls: `{name}()`"),
            format!("{name}()"),
        ))
    } else if let Some((head, rest)) = name
        .split_once('.')
        .filter(|(head, rest)| scope.declares(head) && !rest.is_empty())
    {
        let path = format!("${head}/{}", rest.replace('.', "/"));
        Some((
            format!("`{name}` is one element name that contains dots, not a member access"),
            format!("`.` is part of an XPath name; select the child element with a path: `{path}`"),
            path,
        ))
    } else {
        None
    };
    let (message, help, fix) = match repair {
        Some((message, help, replacement)) => {
            let fix = decoded_fix(
                document,
                site,
                token.start,
                token.end,
                format!("use `{replacement}`"),
                &replacement,
            );
            (message, help, fix)
        }
        None => (
            format!("`{name}` selects no node: `<datamodel>` has no child element named `{name}`"),
            match nearest(name, scope.names()) {
                Some(suggestion) => format!(
                    "a bare name is a relative path from `<datamodel>`; did you mean the variable `${suggestion}`?"
                ),
                None => "a bare name is a relative path from `<datamodel>`, whose children are the `<data>` elements and the system variables; reference a declared variable as `$name`".to_owned(),
            },
            None,
        ),
    };
    let consequence = match site.kind {
        ExpressionKind::Location => {
            "assigning to a location that selects no node raises `error.execution`"
        }
        ExpressionKind::Condition => "an empty node-set is false, so this condition never holds",
        ExpressionKind::Value | ExpressionKind::Script => {
            "an empty node-set yields no value (the empty string, `NaN`, or `false`)"
        }
    };
    let mut diagnostic = Diagnostic::warning("LINT011", message, span)
        .with_category(DiagnosticCategory::Lint)
        .with_primary_message("relative path from `<datamodel>`")
        .with_note(format!(
            "the context node of every XPath expression is `<datamodel>`, whose children are `<data id=\"…\">` elements and the system variables; {consequence}"
        ))
        .with_help(help)
        .with_spec_reference(SPEC_XPATH_DATA)
        .with_spec_reference("SCXML CR 2014 §C.3.3 Location Expressions");
    if let Some(fix) = fix {
        diagnostic = diagnostic.with_fix(fix);
    }
    if span != site.value_span {
        diagnostic = diagnostic
            .with_secondary(Label::new(site.value_span).with_message(format!("in this `{what}`")));
    }
    Some(diagnostic)
}

/// What an empty result means for the fragment's kind.
fn empty_consequence(kind: ExpressionKind) -> &'static str {
    match kind {
        ExpressionKind::Location => {
            "assigning to a location that selects no node raises `error.execution`"
        }
        ExpressionKind::Condition => "an empty node-set is false, so this condition never holds",
        ExpressionKind::Value | ExpressionKind::Script => {
            "an empty node-set yields no value (the empty string, `NaN`, or `false`)"
        }
    }
}

/// `$list/x[0]`: a numeric predicate keeps the node whose position equals
/// the number, and positions start at 1, so a number that is not a positive
/// integer selects nothing (the ECMAScript habit of indexing from 0).
fn positional_finding(
    document: &Document,
    site: &Site<'_>,
    tokens: &[Token],
    open: usize,
) -> Option<Diagnostic> {
    let (number, close) = (tokens.get(open + 1)?, tokens.get(open + 2)?);
    if number.kind != Kind::Number || close.kind != Kind::CloseBracket {
        return None;
    }
    let text = site.source.get(number.start..number.end)?;
    let value: f64 = text.parse().ok()?;
    if value >= 1.0 && value.fract() == 0.0 {
        return None;
    }
    let start = tokens.get(open)?.start;
    let span = site.span_of(start, close.end);
    let what = site.what.as_str();
    let zero = value == 0.0;
    let mut diagnostic = Diagnostic::warning(
        "LINT013",
        format!("the predicate `[{text}]` selects no node: XPath positions start at 1"),
        span,
    )
    .with_category(DiagnosticCategory::Lint)
    .with_primary_message("always empty")
    .with_note(format!(
        "a numeric predicate `[n]` keeps only the node at position `n`, and XPath counts positions from 1, so no node is at position {text}; {}",
        empty_consequence(site.kind)
    ))
    .with_help(if zero {
        "XPath positions start at 1: select the first node with `[1]` (and the last with `[last()]`)".to_owned()
    } else {
        "a position is a whole number from 1: select the first node with `[1]` or the last with `[last()]`".to_owned()
    })
    .with_spec_reference("XPath 1.0 §2.4 Predicates")
    .with_spec_reference(super::SPEC_XPATH);
    if zero
        && let Some(fix) = decoded_fix(
            document,
            site,
            number.start,
            number.end,
            "select the first node with `[1]`".to_owned(),
            "1",
        )
    {
        diagnostic = diagnostic.with_fix(fix);
    }
    if span != site.value_span {
        diagnostic = diagnostic
            .with_secondary(Label::new(site.value_span).with_message(format!("in this `{what}`")));
    }
    Some(diagnostic)
}

/// Operators that bind more loosely than `+`, so they end an additive chain.
fn ends_additive_chain(site: &Site<'_>, token: &Token) -> bool {
    matches!(
        token.kind,
        Kind::Comma | Kind::OpenParen | Kind::OpenBracket | Kind::CloseParen | Kind::CloseBracket
    ) || (token.kind == Kind::Operator
        && matches!(
            site.source.get(token.start..token.end),
            Some("=" | "!=" | "<" | "<=" | ">" | ">=" | "and" | "or")
        ))
}

/// Token range `[first, last]` of the additive chain around the `+` at
/// `plus`, with the `+` separators at its top level; `None` when the chain
/// also subtracts (a `concat()` rewrite would change its meaning).
fn additive_chain(
    site: &Site<'_>,
    tokens: &[Token],
    plus: usize,
) -> Option<(usize, usize, Vec<usize>)> {
    let mut separators = vec![plus];
    let mut first = plus;
    let mut depth = 0_usize;
    while let Some(before) = first.checked_sub(1) {
        let token = tokens.get(before)?;
        match token.kind {
            Kind::CloseParen | Kind::CloseBracket => depth += 1,
            Kind::OpenParen | Kind::OpenBracket if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ if ends_additive_chain(site, token) => break,
            Kind::Operator => match site.source.get(token.start..token.end)? {
                "+" => separators.push(before),
                "-" => return None,
                _ => {}
            },
            _ => {}
        }
        first = before;
    }
    let mut last = plus;
    depth = 0;
    while let Some(token) = tokens.get(last + 1) {
        match token.kind {
            Kind::OpenParen | Kind::OpenBracket => depth += 1,
            Kind::CloseParen | Kind::CloseBracket if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ if ends_additive_chain(site, token) => break,
            Kind::Operator => match site.source.get(token.start..token.end)? {
                "+" => separators.push(last + 1),
                "-" => return None,
                _ => {}
            },
            _ => {}
        }
        last += 1;
    }
    separators.sort_unstable();
    (first < plus && last > plus).then_some((first, last, separators))
}

/// `'a' + 'b'`: `+` converts both operands to numbers, so adding a string
/// that is not numeric yields `NaN` (the ECMAScript habit of joining strings
/// with `+`). Returns the finding and the token index after its chain.
fn string_addition_finding(
    document: &Document,
    site: &Site<'_>,
    tokens: &[Token],
    plus: usize,
) -> Option<(Diagnostic, usize)> {
    let (first, last, separators) = additive_chain(site, tokens, plus)?;
    // Operand token ranges between the separators.
    let mut bounds = Vec::with_capacity(separators.len() + 1);
    let mut start = first;
    for separator in &separators {
        bounds.push((start, separator.checked_sub(1)?));
        start = separator + 1;
    }
    bounds.push((start, last));
    let literal = bounds.iter().find_map(|(from, to)| {
        let token = tokens.get(*from)?;
        (from == to && token.kind == Kind::Literal)
            .then(|| site.source.get(token.start..token.end))
            .flatten()
            .filter(|text| {
                text.get(1..text.len() - 1)
                    .is_some_and(|inner| inner.trim().parse::<f64>().is_err())
            })
    })?;
    let operands = bounds
        .iter()
        .map(|(from, to)| {
            let (from, to) = (tokens.get(*from)?, tokens.get(*to)?);
            site.source.get(from.start..to.end)
        })
        .collect::<Option<Vec<_>>>()?;
    let joined = format!("concat({})", operands.join(", "));
    let token = tokens.get(plus)?;
    let span = site.span_of(token.start, token.end);
    let what = site.what.as_str();
    let mut diagnostic = Diagnostic::warning(
        "LINT014",
        format!("`+` adds numbers in XPath, so adding the string {literal} yields `NaN`"),
        span,
    )
    .with_category(DiagnosticCategory::Lint)
    .with_primary_message("numeric addition, not string concatenation")
    .with_note("XPath converts both operands of `+` to numbers; a string that is not a number converts to `NaN`, and `NaN` plus anything is `NaN`")
    .with_help(format!("join strings with `concat()`: `{joined}`"))
    .with_spec_reference("XPath 1.0 §3.5 Numbers")
    .with_spec_reference(super::SPEC_XPATH);
    let (chain_start, chain_end) = (tokens.get(first)?.start, tokens.get(last)?.end);
    if let Some(fix) = decoded_fix(
        document,
        site,
        chain_start,
        chain_end,
        "join the strings with `concat()`".to_owned(),
        &joined,
    ) {
        diagnostic = diagnostic.with_fix(fix);
    }
    let chain = site.span_of(chain_start, chain_end);
    if chain != span {
        diagnostic = diagnostic.with_secondary(Label::new(chain).with_message("this sum is `NaN`"));
    }
    if chain != site.value_span {
        diagnostic = diagnostic
            .with_secondary(Label::new(site.value_span).with_message(format!("in this `{what}`")));
    }
    Some((diagnostic, last + 1))
}

/// XML `NCName` (ASCII-exact; other letters approximated by Unicode
/// alphabetic classes).
pub(in crate::datamodel_check) fn is_ncname(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && characters.all(is_name_char)
}

fn is_name_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '-' | '_' | '\u{b7}')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Literal,
    Number,
    Variable,
    Name,
    Function,
    Axis,
    Operator,
    Slash,
    At,
    ColonColon,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    Comma,
    Dot,
}

#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
    /// Name text (for names, functions, and variables without `$`).
    text: String,
}

/// Tokens of an accepted `XPath` 1.0 fragment, with the §3.7 operator and
/// name disambiguation; `None` for anything unexpected.
fn tokenize(source: &str) -> Option<Vec<Token>> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut index = 0;
    while let Some(character) = source.get(index..)?.chars().next() {
        if character.is_whitespace() {
            index += character.len_utf8();
            continue;
        }
        let start = index;
        let operator_mode = tokens.last().is_some_and(|token| {
            !matches!(
                token.kind,
                Kind::At
                    | Kind::ColonColon
                    | Kind::OpenParen
                    | Kind::OpenBracket
                    | Kind::Comma
                    | Kind::Operator
                    | Kind::Slash
            )
        });
        let rest = source.get(index..)?;
        let (kind, length, text) = match character {
            '(' => (Kind::OpenParen, 1, String::new()),
            ')' => (Kind::CloseParen, 1, String::new()),
            '[' => (Kind::OpenBracket, 1, String::new()),
            ']' => (Kind::CloseBracket, 1, String::new()),
            ',' => (Kind::Comma, 1, String::new()),
            '@' => (Kind::At, 1, String::new()),
            ':' if rest.starts_with("::") => (Kind::ColonColon, 2, String::new()),
            '.' if rest.starts_with("..") => (Kind::Dot, 2, String::new()),
            '.' if rest.as_bytes().get(1).is_some_and(u8::is_ascii_digit) => {
                (Kind::Number, number_length(rest), String::new())
            }
            '.' => (Kind::Dot, 1, String::new()),
            '0'..='9' => (Kind::Number, number_length(rest), String::new()),
            '"' | '\'' => {
                let end = rest.get(1..)?.find(character)?;
                (Kind::Literal, end + 2, String::new())
            }
            '/' if rest.starts_with("//") => (Kind::Slash, 2, String::new()),
            '/' => (Kind::Slash, 1, String::new()),
            '!' | '<' | '>' if rest.get(1..).is_some_and(|tail| tail.starts_with('=')) => {
                (Kind::Operator, 2, String::new())
            }
            '|' | '+' | '-' | '=' | '<' | '>' => (Kind::Operator, 1, String::new()),
            '*' if operator_mode => (Kind::Operator, 1, String::new()),
            '*' => (Kind::Name, 1, "*".to_owned()),
            '$' => {
                let length = qname_length(rest.get(1..)?);
                if length == 0 {
                    return None;
                }
                (Kind::Variable, length + 1, rest.get(1..=length)?.to_owned())
            }
            _ if character.is_alphabetic() || character == '_' => {
                let length = qname_length(rest);
                let name = rest.get(..length)?;
                let after = rest.get(length..)?.trim_start();
                let kind = if operator_mode {
                    Kind::Operator
                } else if after.starts_with("::") {
                    Kind::Axis
                } else if after.starts_with('(') {
                    Kind::Function
                } else {
                    Kind::Name
                };
                // `prefix:*`
                let length = if rest.get(length..)?.starts_with(":*") {
                    length + 2
                } else {
                    length
                };
                (kind, length, name.to_owned())
            }
            _ => return None,
        };
        index = start + length;
        tokens.push(Token {
            kind,
            start,
            end: index,
            text,
        });
    }
    Some(tokens)
}

fn number_length(text: &str) -> usize {
    let mut seen_dot = false;
    text.char_indices()
        .find(|(_, character)| {
            if *character == '.' && !seen_dot {
                seen_dot = true;
                return false;
            }
            !character.is_ascii_digit()
        })
        .map_or(text.len(), |(index, _)| index)
}

/// Length of the `QName` (`NCName` or `prefix:NCName`) at the start of `text`.
fn qname_length(text: &str) -> usize {
    let ncname = |text: &str| {
        let mut length = 0;
        for (index, character) in text.char_indices() {
            let valid = if index == 0 {
                character.is_alphabetic() || character == '_'
            } else {
                is_name_char(character)
            };
            if !valid {
                break;
            }
            length = index + character.len_utf8();
        }
        length
    };
    let first = ncname(text);
    if first == 0 {
        return 0;
    }
    match text.get(first..) {
        Some(after) if after.starts_with(':') && !after.starts_with("::") => {
            let second = ncname(after.get(1..).unwrap_or_default());
            if second > 0 {
                first + 1 + second
            } else {
                first
            }
        }
        _ => first,
    }
}

#[cfg(test)]
mod tests;
