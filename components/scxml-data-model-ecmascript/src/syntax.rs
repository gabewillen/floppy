//! Parse-only ECMAScript syntax checks for SCXML authoring.
//!
//! # Contract
//!
//! * Pure: the source is parsed with Boa's parser and never evaluated; no
//!   data-model state, host capability, clock, or I/O is touched, and nothing
//!   is retained after the call.
//! * Fidelity: conditions and value expressions are parsed exactly as
//!   [`crate::EcmaScriptDataModel`] evaluates them (parenthesized, tolerating
//!   one trailing `;`), so a fragment this check accepts cannot fail to parse
//!   at runtime and vice versa. Locations must additionally be a variable name
//!   or a property access. Scripts are parsed as ECMAScript scripts.
//! * Bounds: the exact text handed to Boa at most
//!   [`crate::MAX_PARSE_NESTING`] structural nesting units as
//!   [`crate::check_nesting`] counts them (the same guard, on the same text
//!   shape, as runtime evaluation). Boa's parser recurses natively, so deeper
//!   input is declined instead of risking the stack, and native builds parse
//!   on a dedicated [`crate::PARSE_STACK_BYTES`] thread. Over-bound input
//!   fails with [`DataModelError::InvalidInput`].
//! * Output: at most one issue (Boa stops at the first error) whose byte
//!   offsets index the caller's `source`, `start <= end <= source.len()`, on
//!   UTF-8 boundaries; its message is at most [`MAX_SYNTAX_MESSAGE_BYTES`]
//!   bytes and never contains wrapper coordinates.

use boa_engine::ast::expression::Expression;
use boa_engine::ast::scope::Scope;
use boa_engine::ast::statement::Statement;
use boa_engine::ast::{Position, StatementListItem};
use boa_engine::interner::Interner;
use boa_engine::parser::{Error as ParseError, Parser, Source};
use scxml_telemetry::{Module, Operation, SignalEvent, observe};

use crate::{DataModelError, MAX_PARSE_NESTING, check_nesting, nesting, on_parse_stack};
/// Maximum UTF-8 bytes of one issue message.
pub const MAX_SYNTAX_MESSAGE_BYTES: usize = 512;

/// Syntactic category of one checked fragment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyntaxKind {
    /// A boolean `cond` expression.
    Condition,
    /// A value expression.
    Value,
    /// An assignable location.
    Location,
    /// A `<script>` body.
    Script,
}

/// One syntax problem located in the caller's source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxIssue {
    /// Inclusive UTF-8 byte offset.
    pub start: usize,
    /// Exclusive UTF-8 byte offset.
    pub end: usize,
    /// Parser message without positions.
    pub message: String,
}

/// Parses `source` as `kind` without evaluating it.
///
/// Returns an empty list when the fragment is syntactically valid.
///
/// # Errors
///
/// [`DataModelError::InvalidInput`] when `source` contains NUL, or its parsed text exceeds [`MAX_PARSE_NESTING`] units.
pub fn check_syntax(kind: SyntaxKind, source: &str) -> Result<Vec<SyntaxIssue>, DataModelError> {
    let span = observe(
        Module::DataModelEcmascriptComponent,
        Operation::DataModelCheckSyntax,
    );
    let reject = || {
        span.event(SignalEvent::InputRejected);
        span.fail(scxml_telemetry::ErrorCategory::Validation);
        Err(DataModelError::InvalidInput)
    };
    if source.as_bytes().contains(&0) {
        return reject();
    }
    let parsed = match kind {
        SyntaxKind::Script => Some(Wrapped {
            text: source.to_owned(),
            prefix: 0,
            inner_start: 0,
            inner_end: source.len(),
            shape: Shape::Any,
        }),
        SyntaxKind::Condition | SyntaxKind::Value => {
            if nesting::is_blank(source) {
                None
            } else {
                let (inner_start, inner) = expression_body(source);
                Some(Wrapped {
                    text: format!("({inner})"),
                    prefix: PAREN_PREFIX_BYTES,
                    inner_start,
                    inner_end: inner_start + inner.len(),
                    shape: Shape::Any,
                })
            }
        }
        SyntaxKind::Location => {
            if nesting::is_blank(source) {
                None
            } else {
                Some(Wrapped {
                    text: format!("({source})"),
                    prefix: PAREN_PREFIX_BYTES,
                    inner_start: 0,
                    inner_end: source.len(),
                    shape: Shape::Location,
                })
            }
        }
    };
    let issues = match parsed {
        None if kind == SyntaxKind::Location => {
            vec![whole(source, "expected a location, found nothing")]
        }
        None => vec![whole(source, "expected an expression, found nothing")],
        Some(wrapped) => {
            if check_nesting(&wrapped.text).is_err() {
                return reject();
            }
            match on_parse_stack(&span, || Ok(parse(&wrapped))) {
                Ok(issues) => issues,
                Err(_) => return reject(),
            }
        }
    };
    span.event(if issues.is_empty() {
        SignalEvent::InputAccepted
    } else {
        SignalEvent::InputRejected
    });
    Ok(issues)
}

/// The substring the runtime parenthesizes: `value_expression_source`
/// strips one trailing `;` unless the head starts with a statement keyword.
pub(crate) fn expression_body(source: &str) -> (usize, &str) {
    let leading = source.len() - nesting::trim_start(source).len();
    let trimmed = nesting::trim(source);
    if let Some(stripped) = trimmed.strip_suffix(';') {
        let head = nesting::trim(stripped);
        if !head.is_empty()
            && !["for", "if", "while", "do", "switch", "try"]
                .iter()
                .any(|keyword| head.starts_with(keyword))
        {
            return (leading, head);
        }
    }
    (0, source)
}

/// Bytes the runtime's `(` wrapper puts before the caller's text.
const PAREN_PREFIX_BYTES: usize = 1;

/// Whether a parsed fragment must also be an assignable location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shape {
    /// Any syntactically valid fragment.
    Any,
    /// A variable name or property access.
    Location,
}

/// The exact text handed to Boa for one fragment, and how its bytes map back
/// to the caller's `source`: `text` bytes `prefix..` correspond to `source`
/// bytes `inner_start..inner_end`.
struct Wrapped {
    text: String,
    prefix: usize,
    inner_start: usize,
    inner_end: usize,
    shape: Shape,
}

/// Where a parse error lands in the caller's `source`.
struct IssueSpan {
    start: usize,
    end: usize,
    /// The error lies past the fragment: the input ended too early.
    at_end: bool,
}

/// Parses `wrapped.text` and reports at most one issue in `source` bytes.
fn parse(wrapped: &Wrapped) -> Vec<SyntaxIssue> {
    let Wrapped {
        text,
        prefix,
        inner_start,
        inner_end,
        shape,
    } = wrapped;
    let (prefix, inner_start, inner_end) = (*prefix, *inner_start, *inner_end);
    let mut interner = Interner::default();
    let result =
        Parser::new(Source::from_bytes(text)).parse_script(&Scope::new_global(), &mut interner);
    let map = |position: Option<Position>| -> IssueSpan {
        let Some(position) = position else {
            return IssueSpan {
                start: inner_start,
                end: inner_end,
                at_end: false,
            };
        };
        let offset = byte_offset(text, position)
            .saturating_sub(prefix)
            .saturating_add(inner_start);
        if offset >= inner_end {
            IssueSpan {
                start: inner_end,
                end: inner_end,
                at_end: true,
            }
        } else {
            IssueSpan {
                start: offset,
                end: offset,
                at_end: false,
            }
        }
    };
    match result {
        Ok(script) => {
            if *shape == Shape::Location && !is_location(&script) {
                vec![SyntaxIssue {
                    start: inner_start,
                    end: inner_end,
                    message: "expected a variable name or property access".to_owned(),
                }]
            } else {
                Vec::new()
            }
        }
        Err(error) => {
            let (message, position) = describe(&error);
            let IssueSpan { start, end, at_end } = map(position);
            vec![SyntaxIssue {
                start,
                end,
                message: bounded_message(if at_end {
                    "unexpected end of input".to_owned()
                } else {
                    message
                }),
            }]
        }
    }
}

/// ECMA-262 `ReservedWord` outside strict mode code: never a variable name
/// in a script.
const RESERVED_WORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
];

/// Whether `name` is exactly one legal ECMAScript variable name (B.2.11:
/// "The legal values for the 'item' attribute are legal ECMAScript variable
/// names"): Boa parses `(name)` as a script whose only statement is one
/// identifier reference spelled exactly `name` (no escapes, comments, or
/// white space), so reserved words and every other expression are not.
/// Pure; no telemetry (a check inside the calling operation).
pub(crate) fn is_variable_name(name: &str) -> bool {
    if name.is_empty() || name.contains('\\') {
        return false;
    }
    if name.is_ascii() {
        return is_ascii_variable_name(name);
    }
    parses_as_variable_name(name)
}

/// The ASCII case of [`is_variable_name`], without a parse: an identifier
/// is `[A-Za-z_$]` then `[A-Za-z0-9_$]*`, and it is a variable name unless
/// it is a reserved word of a non-strict script (ECMA-262 `ReservedWord`).
fn is_ascii_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && characters.all(|next| next.is_ascii_alphanumeric() || matches!(next, '_' | '$'))
        && !RESERVED_WORDS.contains(&name)
}

/// Whether Boa parses `(name)` as one identifier reference spelled exactly
/// `name`.
fn parses_as_variable_name(name: &str) -> bool {
    let text = format!("({name})");
    if check_nesting(&text).is_err() {
        return false;
    }
    let mut interner = Interner::default();
    let Ok(script) =
        Parser::new(Source::from_bytes(&text)).parse_script(&Scope::new_global(), &mut interner)
    else {
        return false;
    };
    let [StatementListItem::Statement(statement)] = script.statements().statements() else {
        return false;
    };
    let Statement::Expression(Expression::Parenthesized(inner)) = statement.as_ref() else {
        return false;
    };
    let Expression::Identifier(identifier) = inner.expression() else {
        return false;
    };
    interner
        .resolve(identifier.sym())
        .is_some_and(|resolved| resolved.to_string() == name)
}

/// The source text of the function `new Function(...parameters, body)`
/// creates, when it is one: ECMA-262 `CreateDynamicFunction` parses the
/// parameters alone as `FormalParameters` and the body alone as a
/// `FunctionBody`, then the source text
/// `function anonymous(<parameters>\n) {\n<body>\n}`. This checks the
/// same: each part parses on its own, and the whole text is exactly one
/// parenthesized function expression. `None` otherwise (a `SyntaxError`).
/// Every text handed to the parser has passed [`check_nesting`]; `None`
/// for any it declines. Pure; no telemetry (a step of the calling
/// operation).
pub(crate) fn dynamic_function_source(parameters: &str, body: &str) -> Option<String> {
    let text = format!("(function anonymous({parameters}\n) {{\n{body}\n}})");
    let parts_parse = [
        format!("(function ({parameters}\n) {{}})"),
        format!("(function () {{\n{body}\n}})"),
    ]
    .iter()
    .all(|part| is_single_function_expression(part));
    (parts_parse && is_single_function_expression(&text)).then_some(text)
}

/// Whether `text` is exactly one parenthesized function expression.
fn is_single_function_expression(text: &str) -> bool {
    if check_nesting(text).is_err() {
        return false;
    }
    let mut interner = Interner::default();
    let Ok(script) =
        Parser::new(Source::from_bytes(text)).parse_script(&Scope::new_global(), &mut interner)
    else {
        return false;
    };
    let [StatementListItem::Statement(statement)] = script.statements().statements() else {
        return false;
    };
    let Statement::Expression(Expression::Parenthesized(inner)) = statement.as_ref() else {
        return false;
    };
    matches!(inner.expression(), Expression::FunctionExpression(_))
}

fn is_location(script: &boa_engine::ast::Script) -> bool {
    let [StatementListItem::Statement(statement)] = script.statements().statements() else {
        return false;
    };
    let Statement::Expression(expression) = statement.as_ref() else {
        return false;
    };
    let mut expression = expression;
    for _ in 0..=MAX_PARSE_NESTING {
        match expression {
            Expression::Parenthesized(inner) => expression = inner.expression(),
            Expression::Identifier(_) | Expression::PropertyAccess(_) => return true,
            _ => return false,
        }
    }
    false
}

fn describe(error: &ParseError) -> (String, Option<Position>) {
    match error {
        ParseError::Expected {
            expected,
            found,
            span,
            ..
        } => {
            let expected = expected
                .iter()
                .map(|token| format!("`{token}`"))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!("expected {expected}, found `{found}`"),
                Some(span.start()),
            )
        }
        ParseError::Unexpected {
            found,
            span,
            message,
        } => (
            format!("unexpected `{found}`: {message}"),
            Some(span.start()),
        ),
        ParseError::AbruptEnd => ("unexpected end of input".to_owned(), None),
        ParseError::General { message, position } => (message.to_string(), Some(*position)),
        ParseError::Lex { err } => match err {
            boa_engine::parser::lexer::error::Error::Syntax(message, position) => {
                (message.to_string(), Some(*position))
            }
            boa_engine::parser::lexer::error::Error::IO(error) => (error.to_string(), None),
        },
        ParseError::ScopeAnalysis { err } => (format!("invalid declaration scope: {err}"), None),
    }
}

/// Converts a one-based Boa line/column into a byte offset of `text`.
pub(crate) fn byte_offset(text: &str, position: Position) -> usize {
    let line = usize::try_from(position.line_number()).unwrap_or(usize::MAX);
    let column = usize::try_from(position.column_number()).unwrap_or(usize::MAX);
    let mut line_start = 0;
    for _ in 1..line {
        match text.get(line_start..).and_then(|rest| rest.find('\n')) {
            Some(newline) => line_start += newline + 1,
            None => return text.len(),
        }
    }
    let rest = text.get(line_start..).unwrap_or("");
    rest.char_indices()
        .nth(column.saturating_sub(1))
        .map_or(text.len(), |(offset, _)| line_start + offset)
}

fn whole(source: &str, message: &str) -> SyntaxIssue {
    SyntaxIssue {
        start: 0,
        end: source.len(),
        message: message.to_owned(),
    }
}

fn bounded_message(mut message: String) -> String {
    if message.len() > MAX_SYNTAX_MESSAGE_BYTES {
        let mut end = MAX_SYNTAX_MESSAGE_BYTES;
        while end > 0 && !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issues(kind: SyntaxKind, source: &str) -> Vec<SyntaxIssue> {
        check_syntax(kind, source).expect("fixture is within the syntax-check bounds")
    }

    #[test]
    fn valid_fragments_have_no_issues() {
        assert!(issues(SyntaxKind::Condition, "x > 1 && In('s')").is_empty());
        assert!(issues(SyntaxKind::Value, " new Test(); ").is_empty());
        assert!(issues(SyntaxKind::Value, "function () { return 1; }").is_empty());
        assert!(issues(SyntaxKind::Location, "data.items[0]").is_empty());
        assert!(issues(SyntaxKind::Location, "x").is_empty());
        assert!(issues(SyntaxKind::Script, "var a = 1; function f() { return a; }").is_empty());
        assert!(issues(SyntaxKind::Script, "   ").is_empty());
    }

    /// Only ECMAScript white space and line terminators make a fragment
    /// empty: U+FEFF does, U+0085 is a character the parser rejects.
    #[test]
    fn an_empty_fragment_is_ecmascript_white_space_only() {
        const ECMASCRIPT_BLANK: &str = "\u{FEFF}\u{2028}";
        const NEXT_LINE: &str = "\u{85}";
        let messages = |kind: SyntaxKind, source: &str| -> Vec<String> {
            issues(kind, source)
                .into_iter()
                .map(|issue| issue.message)
                .collect()
        };
        assert_eq!(
            messages(SyntaxKind::Condition, ECMASCRIPT_BLANK),
            ["expected an expression, found nothing"]
        );
        assert_eq!(
            messages(SyntaxKind::Location, ECMASCRIPT_BLANK),
            ["expected a location, found nothing"]
        );
        for kind in [
            SyntaxKind::Condition,
            SyntaxKind::Value,
            SyntaxKind::Location,
        ] {
            let found = messages(kind, NEXT_LINE);
            assert_eq!(found.len(), 1, "{kind:?}");
            assert!(!found[0].contains("found nothing"), "{kind:?}: {found:?}");
        }
        assert_eq!(
            messages(SyntaxKind::Value, &format!("1;{NEXT_LINE}")).len(),
            1
        );
        assert_eq!(
            messages(SyntaxKind::Value, &format!("1{NEXT_LINE};")).len(),
            1
        );
        let found = issues(SyntaxKind::Value, &format!("{ECMASCRIPT_BLANK}a b;"));
        let b = ECMASCRIPT_BLANK.len() + "a ".len();
        assert_eq!((found[0].start, found[0].end), (b, b), "{found:?}");
        assert!(
            messages(
                SyntaxKind::Value,
                &format!("{ECMASCRIPT_BLANK}1;{ECMASCRIPT_BLANK}")
            )
            .is_empty()
        );
    }

    #[test]
    fn errors_point_into_the_callers_source() {
        let found = issues(SyntaxKind::Condition, "x ===");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].start, found[0].end), (5, 5));
        assert_eq!(found[0].message, "unexpected end of input");

        let found = issues(SyntaxKind::Value, "a b");
        assert_eq!((found[0].start, found[0].end), (2, 2), "{found:?}");
        assert!(!found[0].message.contains("line"), "{found:?}");

        let found = issues(SyntaxKind::Script, "var = 1;");
        assert_eq!(found.len(), 1);
        assert!(found[0].start <= 4, "{found:?}");

        let found = issues(SyntaxKind::Script, "a = 1;\nb = )");
        assert_eq!(found[0].start, 11, "{found:?}");
    }

    #[test]
    fn ascii_variable_names_agree_with_the_parser() {
        let mut names = RESERVED_WORDS.to_vec();
        names.extend([
            "let",
            "yield",
            "await",
            "static",
            "async",
            "of",
            "get",
            "set",
            "implements",
            "package",
            "private",
            "protected",
            "public",
            "interface",
            "undefined",
            "NaN",
            "arguments",
            "eval",
            "$",
            "_",
            "_a1",
            "a$",
            "a1",
            "1a",
            "a-b",
            "a b",
            "a.b",
            "(a)",
            "a;",
            "",
            "0",
            "$1",
        ]);
        for name in names {
            assert_eq!(
                is_ascii_variable_name(name),
                parses_as_variable_name(name),
                "{name:?}"
            );
        }
    }

    #[test]
    fn locations_must_be_assignable() {
        for bad in ["1 + x", "f()", "a, b", "'text'"] {
            let found = issues(SyntaxKind::Location, bad);
            assert_eq!(found.len(), 1, "{bad}");
            assert_eq!((found[0].start, found[0].end), (0, bad.len()), "{bad}");
        }
        assert_eq!(issues(SyntaxKind::Location, "").len(), 1);
        let unterminated = issues(SyntaxKind::Location, "\"foo");
        assert_eq!(unterminated.len(), 1);
        assert!(!unterminated[0].message.is_empty(), "{unterminated:?}");
        assert_eq!(issues(SyntaxKind::Value, "  ").len(), 1);
    }

    /// Natively the expression and script byte bounds do not apply: one
    /// byte past the wasm bound (16 KiB) checks, with no decline.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_checks_have_no_byte_bound() {
        assert!(issues(SyntaxKind::Value, &"x".repeat(16_385)).is_empty());
        assert!(issues(SyntaxKind::Script, &"x;".repeat(32_769)).is_empty());
    }

    #[test]
    fn deep_nesting_and_over_bound_input_are_declined() {
        // Value expressions are parsed parenthesized: one bracket level of the
        // bound belongs to that wrapper.
        let deepest = MAX_PARSE_NESTING / crate::nesting::NESTING_BRACKET_WEIGHT - 1;
        let nested = |levels: usize| format!("{}1{}", "(".repeat(levels), ")".repeat(levels));
        assert_eq!(
            check_syntax(SyntaxKind::Value, &nested(deepest + 1)),
            Err(DataModelError::InvalidInput)
        );
        assert_eq!(
            check_syntax(
                SyntaxKind::Value,
                &format!("{}x", "!".repeat(MAX_PARSE_NESTING))
            ),
            Err(DataModelError::InvalidInput)
        );
        // At the bound the parse completes on the caller's (default test)
        // thread: native builds parse on the dedicated parse stack.
        assert!(issues(SyntaxKind::Value, &nested(deepest)).is_empty());
        // The expression byte bound applies to wasm builds only; the host's
        // component tests check it through the shipped data model
        // (`an_expression_over_the_ecmascript_byte_bound_raises_error_execution`).
    }

    #[test]
    fn strings_and_comments_do_not_count_as_nesting() {
        let quoted = format!("'{}'", "(".repeat(100));
        assert!(issues(SyntaxKind::Value, &quoted).is_empty());
        let commented = format!("// {}\nvar a = 1;", "[".repeat(100));
        assert!(issues(SyntaxKind::Script, &commented).is_empty());
    }
}
