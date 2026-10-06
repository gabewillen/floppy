//! `XPath` data-model wording and repairs (SCXML CR 2014 §C.3).
//!
//! The `XPath` checker reports static parser messages with an exact span.
//! This module rewrites each into project-owned wording that names the
//! offending source text, explains the `XPath` rule it breaks, and offers a
//! maybe-incorrect repair when one is certain to parse: the ECMAScript habits
//! agents bring to `XPath` (`==`, `&&`, `||`, `!x`, `not x`, `length()`), a
//! stray space inside `>=`, a trailing argument comma, an apostrophe that
//! ends a string early, a namespace prefix on a core function, and a
//! `<script>`, which the `XPath` data model does not support.
//!
//! Every scan here is one bounded pass over a fragment the caller already
//! bounded (`MAX_CHECKED_EXPRESSION_BYTES`), and nothing is evaluated.

pub(super) mod references;

use scxml_syntax::{Document, Span};

use super::{IssueWording, Site, SyntaxIssue};
use crate::graph::removal_span;
use crate::semantic::nearest;
use crate::{Applicability, Fix};

/// `datamodel` value of the `XPath` data model.
pub(super) const DATA_MODEL: &str = "xpath";
/// Normative citation for the `XPath` data model, which the 2015 SCXML
/// Recommendation dropped; the last W3C text defining it is the 2014
/// Candidate Recommendation.
pub(super) const SPEC_XPATH: &str = "SCXML CR 2014 §C.3 The XPath Data Model";

/// The `XPath` 1.0 core function library and the SCXML `In()` predicate,
/// with the argument count each accepts.
const FUNCTIONS: [(&str, &str); 28] = [
    ("boolean", "exactly 1 argument"),
    ("ceiling", "exactly 1 argument"),
    ("concat", "2 or more arguments"),
    ("contains", "exactly 2 arguments"),
    ("count", "exactly 1 argument"),
    ("false", "no arguments"),
    ("floor", "exactly 1 argument"),
    ("id", "exactly 1 argument"),
    ("In", "exactly 1 argument (a state ID string)"),
    ("lang", "exactly 1 argument"),
    ("last", "no arguments"),
    ("local-name", "0 or 1 arguments"),
    ("name", "0 or 1 arguments"),
    ("namespace-uri", "0 or 1 arguments"),
    ("normalize-space", "0 or 1 arguments"),
    ("not", "exactly 1 argument"),
    ("number", "0 or 1 arguments"),
    ("position", "no arguments"),
    ("round", "exactly 1 argument"),
    ("starts-with", "exactly 2 arguments"),
    ("string", "0 or 1 arguments"),
    ("string-length", "0 or 1 arguments"),
    ("substring", "2 or 3 arguments"),
    ("substring-after", "exactly 2 arguments"),
    ("substring-before", "exactly 2 arguments"),
    ("sum", "exactly 1 argument"),
    ("translate", "exactly 3 arguments"),
    ("true", "no arguments"),
];

/// ECMAScript (and other host-language) function names with the `XPath`
/// functions that do their job, each with a short gloss for the help text.
const FOREIGN_FUNCTIONS: [(&str, &[(&str, &str)]); 14] = [
    (
        "length",
        &[
            ("string-length", "characters in a string"),
            ("count", "nodes in a node-set"),
        ],
    ),
    (
        "len",
        &[
            ("string-length", "characters in a string"),
            ("count", "nodes in a node-set"),
        ],
    ),
    ("size", &[("count", "nodes in a node-set")]),
    ("strlen", &[("string-length", "characters in a string")]),
    ("toString", &[("string", "string value")]),
    ("String", &[("string", "string value")]),
    ("Number", &[("number", "numeric value")]),
    ("parseInt", &[("number", "numeric value")]),
    ("parseFloat", &[("number", "numeric value")]),
    ("Boolean", &[("boolean", "effective boolean value")]),
    ("includes", &[("contains", "substring test")]),
    ("startsWith", &[("starts-with", "prefix test")]),
    ("substr", &[("substring", "part of a string")]),
    ("trim", &[("normalize-space", "trims and collapses spaces")]),
];

/// Rewrites one `XPath` checker message for `site`.
pub(super) fn word(document: &Document, site: &Site<'_>, issue: &SyntaxIssue) -> IssueWording {
    if let Some(wording) = super::placeholder_wording(site) {
        return wording;
    }
    let raw = issue.message.as_str();
    let found = site
        .source
        .get(issue.start..issue.end)
        .unwrap_or_default()
        .to_owned();
    let context = Context {
        document,
        site,
        at: issue.start,
        end: issue.end,
        found,
    };
    match raw {
        "the XPath data model defines no script language; <script> must be empty" => {
            context.script()
        }
        "a location must be an XPath expression that selects nodes" => context.location(),
        "expected an expression" => context.expected_expression(),
        "unexpected character" => context.unexpected_character(),
        "unexpected token after expression" => context.trailing_token(),
        "expected an operator" => context.expected_operator(),
        "unknown function" => context.unknown_function(),
        "wrong number of function arguments" => context.arity(),
        "expected ')' to close function call" => context.unclosed('(', ')', "function call"),
        "expected ')'" => context.unclosed('(', ')', "parenthesized expression"),
        "expected ']' to close predicate" => context.unclosed('[', ']', "predicate"),
        "unterminated string literal" => context.unterminated(),
        "expected a variable name after '$'" => IssueWording {
            message: "`$` must be followed by a variable name".to_owned(),
            help: "write the variable name directly after `$`, for example `$count`".to_owned(),
            ..IssueWording::default()
        },
        "undeclared namespace prefix" => context.prefix(),
        "unknown axis" => IssueWording {
            message: format!("unknown XPath axis `{}`", context.found),
            help: "XPath 1.0 axes are `ancestor`, `ancestor-or-self`, `attribute`, `child`, `descendant`, `descendant-or-self`, `following`, `following-sibling`, `namespace`, `parent`, `preceding`, `preceding-sibling`, and `self`".to_owned(),
            ..IssueWording::default()
        },
        "expected a node test" => IssueWording {
            message: format!(
                "expected a node test (a name, `*`, `text()`, or `node()`), found {}",
                context.found_phrase()
            ),
            help: "each step of a location path must name what it selects, for example `$order/item` or `$order/*`".to_owned(),
            ..IssueWording::default()
        },
        _ => IssueWording {
            message: format!("{raw}, found {}", context.found_phrase()),
            help: "fix the XPath 1.0 syntax at the marked position".to_owned(),
            ..IssueWording::default()
        },
    }
}

/// One issue being worded.
struct Context<'s, 'a> {
    document: &'s Document,
    site: &'s Site<'a>,
    /// Decoded start of the checker's span.
    at: usize,
    /// Decoded end of the checker's span.
    end: usize,
    /// Decoded text of the checker's span (empty at the end of input).
    found: String,
}

impl Context<'_, '_> {
    fn source(&self) -> &str {
        self.site.source.as_str()
    }

    /// The found token quoted, or `the end of the expression`.
    fn found_phrase(&self) -> String {
        if self.found.is_empty() {
            "the end of the expression".to_owned()
        } else {
            format!("`{}`", self.found)
        }
    }

    /// Text before `offset`, with trailing whitespace removed.
    fn before(&self, offset: usize) -> &str {
        self.source().get(..offset).unwrap_or_default().trim_end()
    }

    /// A maybe-incorrect replacement of decoded `start..end`.
    fn fix(&self, start: usize, end: usize, message: String, replacement: &str) -> Option<Fix> {
        decoded_fix(self.document, self.site, start, end, message, replacement)
    }

    /// `replacement` with a space added on each side that touches a
    /// non-space character, so a word operator never fuses with its operands.
    fn spaced(&self, start: usize, end: usize, replacement: &str) -> String {
        let source = self.source();
        let open = source
            .get(..start)
            .and_then(|head| head.chars().next_back())
            .is_some_and(|character| !character.is_whitespace() && character != '(');
        let close = source
            .get(end..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(|character| !character.is_whitespace() && character != ')');
        format!(
            "{}{replacement}{}",
            if open { " " } else { "" },
            if close { " " } else { "" }
        )
    }

    fn script(&self) -> IssueWording {
        let site = self.site;
        IssueWording {
            message: "`<script>` is not supported by the XPath data model".to_owned(),
            help: "move each assignment into `<assign location=\"$var\" expr=\"…\"/>` (and each message into `<log>`), then remove the `<script>`".to_owned(),
            note: Some("the XPath data model defines no script language (SCXML CR 2014 §C.3.9 and §C.3.11), so a `<script>` raises `error.execution` and aborts the rest of its executable-content block".to_owned()),
            label: Some("not supported by `datamodel=\"xpath\"`".to_owned()),
            fix: Some(Fix::replace(
                "remove the `<script>` element",
                Applicability::MaybeIncorrect,
                removal_span(self.document, site.element.span),
                "",
            )),
            ..IssueWording::default()
        }
    }

    fn location(&self) -> IssueWording {
        IssueWording {
            message: format!(
                "`{}` selects no node, so nothing can be assigned to it",
                self.source().trim()
            ),
            help: "write a `$`-rooted path to a declared `<data>` variable, such as `$count` or `$order/item/@qty`".to_owned(),
            ..IssueWording::default()
        }
    }

    fn expected_expression(&self) -> IssueWording {
        let source = self.source();
        let at = self.at;
        let before = self.before(at);
        let previous = before.chars().next_back();
        let found = self.found.as_str();
        if at >= source.len() || found.is_empty() {
            return IssueWording {
                message: "the expression ends before it is complete".to_owned(),
                help: match previous {
                    Some(',') => "remove the trailing `,`, or add the missing argument".to_owned(),
                    Some('(') => {
                        "write the expression inside the `(`, and close it with `)`".to_owned()
                    }
                    _ => "add the missing operand after the last operator".to_owned(),
                },
                ..IssueWording::default()
            };
        }
        match (found, previous) {
            ("=", Some('=')) => {
                // `$a == 3` or `$a === 3`: keep the first `=`.
                let first = before.len() - 1;
                let mut run_end = at;
                while source.get(run_end..).is_some_and(|rest| rest.starts_with('=')) {
                    run_end += 1;
                }
                let spelled = source.get(first..run_end).unwrap_or("==");
                IssueWording {
                    message: format!("`{spelled}` is not an XPath operator"),
                    help: "XPath compares with a single `=` (and `!=` for inequality)".to_owned(),
                    fix: self.fix(first + 1, run_end, "compare with `=`".to_owned(), ""),
                    token: Some((first, run_end)),
                    ..IssueWording::default()
                }
            }
            ("=", Some(operator @ ('<' | '>'))) => {
                let operator_at = before.len() - 1;
                IssueWording {
                    message: format!("`{operator} =` is not an operator"),
                    help: format!("write `{operator}=` without the space"),
                    fix: self.fix(before.len(), at, format!("compare with `{operator}=`"), ""),
                    token: Some((operator_at, at + 1)),
                    ..IssueWording::default()
                }
            }
            ("|", Some('|')) => {
                let first = before.len() - 1;
                IssueWording {
                    message: "`||` is not an XPath operator".to_owned(),
                    help: "XPath spells logical or as `or`; a single `|` is the node-set union"
                        .to_owned(),
                    fix: self.fix(
                        first,
                        at + 1,
                        "use `or`".to_owned(),
                        &self.spaced(first, at + 1, "or"),
                    ),
                    token: Some((first, at + 1)),
                    ..IssueWording::default()
                }
            }
            (")", Some(',')) => {
                let comma = before.len() - 1;
                IssueWording {
                    message: "trailing `,` before `)`".to_owned(),
                    help: "remove the `,`, or add the missing argument after it".to_owned(),
                    fix: self.fix(comma, at, "remove the trailing `,`".to_owned(), ""),
                    token: Some((comma, comma + 1)),
                    ..IssueWording::default()
                }
            }
            _ => IssueWording {
                message: format!("expected an operand, found `{found}`"),
                help: "an operand is a number, a quoted string, a `$variable`, a location path, or a function call such as `count($items/*)`".to_owned(),
                ..IssueWording::default()
            },
        }
    }

    fn unexpected_character(&self) -> IssueWording {
        let source = self.source();
        let at = self.at;
        let found = self.found.as_str();
        match found {
            "&" => {
                let doubled = source.get(at + 1..).is_some_and(|rest| rest.starts_with('&'));
                let end = if doubled { at + 2 } else { at + 1 };
                let spelled = if doubled { "&&" } else { "&" };
                IssueWording {
                    message: format!("`{spelled}` is not an XPath operator"),
                    help: "XPath spells logical and as `and`".to_owned(),
                    fix: self.fix(at, end, "use `and`".to_owned(), &self.spaced(at, end, "and")),
                    token: Some((at, end)),
                    ..IssueWording::default()
                }
            }
            "!" => {
                let start = skip_whitespace(source, at + 1);
                let fix = operand_end(source, start).and_then(|end| {
                    let operand = source.get(start..end)?;
                    self.fix(
                        at,
                        end,
                        "negate with `not()`".to_owned(),
                        &not_call(operand),
                    )
                });
                IssueWording {
                    message: "`!` is not an XPath operator".to_owned(),
                    help: "negate with the `not()` function, for example `not($done)`; `!=` is the only XPath operator with `!`".to_owned(),
                    fix,
                    ..IssueWording::default()
                }
            }
            "%" => IssueWording {
                message: "`%` is not an XPath operator".to_owned(),
                help: "XPath spells the remainder operator `mod`".to_owned(),
                fix: self.fix(at, at + 1, "use `mod`".to_owned(), &self.spaced(at, at + 1, "mod")),
                ..IssueWording::default()
            },
            ";" if source.get(at + 1..).is_some_and(|rest| rest.trim().is_empty()) => {
                IssueWording {
                    message: "`;` cannot end an XPath expression".to_owned(),
                    help: "an XPath expression is not a statement: remove the `;`".to_owned(),
                    fix: self.fix(
                        self.before(at).len(),
                        source.len(),
                        "remove the `;`".to_owned(),
                        "",
                    ),
                    ..IssueWording::default()
                }
            }
            "?" => IssueWording {
                message: "`?` is not an XPath operator".to_owned(),
                help: "XPath 1.0 has no `?:` conditional; split the logic into `<if>`/`<else>` branches".to_owned(),
                ..IssueWording::default()
            },
            "{" | "}" => IssueWording {
                message: format!("`{found}` cannot appear in an XPath expression"),
                help: "XPath has no object or block syntax; build XML values as `<data>` content and address them with paths such as `$order/item`".to_owned(),
                ..IssueWording::default()
            },
            _ => IssueWording {
                message: format!("`{found}` cannot appear in an XPath expression"),
                help: "remove the character, or quote it inside a string literal".to_owned(),
                ..IssueWording::default()
            },
        }
    }

    /// A complete expression followed by more text.
    fn trailing_token(&self) -> IssueWording {
        let source = self.source();
        let at = self.at;
        let before = self.before(at);
        let found = self.found.as_str();
        // `not` as a whole word (not the tail of `$x/not` or `knot`).
        if let Some(word_start) = before.len().checked_sub(3)
            && before.get(word_start..) == Some("not")
            && before
                .get(..word_start)
                .and_then(|head| head.chars().next_back())
                .is_none_or(|character| {
                    character.is_whitespace()
                        || matches!(character, '(' | ',' | '[' | '=' | '<' | '>')
                })
        {
            let fix = operand_end(source, at).and_then(|end| {
                let operand = source.get(at..end)?;
                self.fix(
                    word_start,
                    end,
                    "call `not()` with parentheses".to_owned(),
                    &not_call(operand),
                )
            });
            return IssueWording {
                message: "`not` is a function in XPath, not an operator".to_owned(),
                help: "call it with parentheses, for example `not($done)`".to_owned(),
                fix,
                token: Some((word_start, word_start + 3)),
                ..IssueWording::default()
            };
        }
        IssueWording {
            message: format!("unexpected `{found}` after a complete expression"),
            help: "two expressions follow each other with no operator between them: join them with an operator such as `and`, `or`, `=`, or `+`, or quote text as a string literal".to_owned(),
            ..IssueWording::default()
        }
    }

    /// A name where an operator must follow a complete operand.
    fn expected_operator(&self) -> IssueWording {
        let source = self.source();
        let at = self.at;
        let found = self.found.as_str();
        let quote = source
            .get(..at)
            .and_then(|head| head.chars().next_back())
            .filter(|quote| matches!(quote, '\'' | '"'));
        if let Some(quote) = quote {
            return self.early_string_end(quote);
        }
        if let Some(operator) = ["and", "or", "div", "mod"]
            .into_iter()
            .find(|operator| operator.eq_ignore_ascii_case(found))
        {
            return IssueWording {
                message: format!("XPath operators are lowercase: `{found}` is not an operator"),
                help: format!("write `{operator}`"),
                fix: self.fix(at, self.end, format!("use `{operator}`"), operator),
                ..IssueWording::default()
            };
        }
        IssueWording {
            message: format!("`{found}` is not an XPath operator"),
            help: "an operator must follow a complete operand here: `and`, `or`, `=`, `!=`, `<`, `<=`, `>`, `>=`, `+`, `-`, `*`, `div`, `mod`, or `|`; quote text as a string literal".to_owned(),
            ..IssueWording::default()
        }
    }

    /// `'it's done'`: `XPath` 1.0 literals have no escapes, so the
    /// apostrophe ends the literal.
    fn early_string_end(&self, quote: char) -> IssueWording {
        let source = self.source();
        let quote_at = self.at - quote.len_utf8();
        let other = if quote == '\'' { '"' } else { '\'' };
        let trimmed = source.trim();
        let leading = source.len() - source.trim_start().len();
        let whole_literal = trimmed.len() >= 2
            && trimmed.starts_with(quote)
            && trimmed.ends_with(quote)
            && !trimmed.contains(other);
        let fix = whole_literal
            .then(|| {
                let inner = trimmed.get(1..trimmed.len() - 1)?;
                self.fix(
                    leading,
                    leading + trimmed.len(),
                    format!("delimit the string with `{other}`"),
                    &format!("{other}{inner}{other}"),
                )
            })
            .flatten();
        IssueWording {
            message: format!("the string literal ends early at this `{quote}`"),
            help: format!(
                "XPath 1.0 string literals have no escape sequences: delimit a string that contains `{quote}` with `{other}` (written `{}` inside an attribute delimited by `{other}`), or build it with `concat()`",
                if other == '"' { "&quot;" } else { "&apos;" }
            ),
            fix,
            token: Some((quote_at, self.at)),
            ..IssueWording::default()
        }
    }

    fn unknown_function(&self) -> IssueWording {
        let name = self.found.as_str();
        // `fn:concat()`: the parser declares no function namespaces, so a
        // prefixed call is unknown; the prefix is the mistake.
        if name.contains(':') {
            return self.prefix();
        }
        let mut suggestions: Vec<(&str, Option<&str>)> = FOREIGN_FUNCTIONS
            .iter()
            .find(|(foreign, _)| *foreign == name)
            .map(|(_, replacements)| {
                replacements
                    .iter()
                    .map(|(function, gloss)| (*function, Some(*gloss)))
                    .collect()
            })
            .unwrap_or_default();
        if suggestions.is_empty()
            && let Some(function) = FUNCTIONS
                .iter()
                .map(|(function, _)| *function)
                .find(|function| function.eq_ignore_ascii_case(name))
                .or_else(|| nearest(name, FUNCTIONS.iter().map(|(function, _)| *function)))
        {
            suggestions.push((function, None));
        }
        let mut fixes = suggestions.iter().filter_map(|(function, _)| {
            self.fix(self.at, self.end, format!("use `{function}()`"), function)
        });
        let fix = fixes.next();
        let alternatives = fixes.collect();
        let help = match suggestions.as_slice() {
            [] => "XPath 1.0 core functions (such as `count()`, `string-length()`, `contains()`, and `concat()`) and the SCXML `In()` predicate are available; no other function is defined".to_owned(),
            [(function, None)] => format!("did you mean `{function}()`?"),
            options => format!(
                "XPath has no `{name}()`; use {}",
                options
                    .iter()
                    .map(|(function, gloss)| match gloss {
                        Some(gloss) => format!("`{function}()` ({gloss})"),
                        None => format!("`{function}()`"),
                    })
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        };
        IssueWording {
            message: format!("unknown XPath function `{name}()`"),
            help,
            label: Some("unknown function".to_owned()),
            fix,
            alternatives,
            ..IssueWording::default()
        }
    }

    fn arity(&self) -> IssueWording {
        let name = self.found.as_str();
        let accepted = FUNCTIONS
            .iter()
            .find(|(function, _)| *function == name)
            .map_or("a different number of arguments", |(_, arity)| arity);
        IssueWording {
            message: format!("wrong number of arguments to `{name}()`"),
            help: format!("`{name}()` takes {accepted}"),
            label: Some("called with the wrong number of arguments".to_owned()),
            ..IssueWording::default()
        }
    }

    /// `count($a` or `$a[1`: the checker stops where the closing delimiter
    /// is missing; the opener is labeled so the author sees what is open.
    fn unclosed(&self, open: char, close: char, what: &str) -> IssueWording {
        // `concat('a' 'b')`: a complete argument followed by another one is
        // a missing `,`, not a missing `)`.
        let before = self.before(self.at);
        let operand_ends = before.chars().next_back().is_some_and(|character| {
            character.is_alphanumeric() || matches!(character, '\'' | '"' | ')' | ']' | '_')
        });
        let operand_starts = self.found.chars().next().is_some_and(|character| {
            character.is_alphanumeric() || matches!(character, '\'' | '"' | '$' | '(' | '_')
        });
        if what == "function call" && operand_ends && operand_starts {
            return IssueWording {
                message: format!(
                    "expected `,` between the function's arguments, found {}",
                    self.found_phrase()
                ),
                help:
                    "separate function arguments with `,`, for example `concat($first, ' ', $last)`"
                        .to_owned(),
                fix: self.fix(
                    before.len(),
                    self.at,
                    "separate the arguments with `,`".to_owned(),
                    ", ",
                ),
                label: Some("expected `,` before this argument".to_owned()),
                ..IssueWording::default()
            };
        }
        let related = unmatched_opener(self.source(), self.at, open, close)
            .map(|opener| {
                vec![(
                    opener,
                    opener + open.len_utf8(),
                    format!("this `{open}` is never closed"),
                )]
            })
            .unwrap_or_default();
        let found = self.found_phrase();
        IssueWording {
            message: format!("expected `{close}` to close the {what}, found {found}"),
            help: format!("add the missing `{close}` where the {what} ends"),
            related,
            ..IssueWording::default()
        }
    }

    fn unterminated(&self) -> IssueWording {
        let quote = self.found.chars().next().unwrap_or('\'');
        IssueWording {
            message: "unterminated string literal".to_owned(),
            help: format!(
                "close the string with a matching `{quote}`; XPath 1.0 string literals have no escape sequences"
            ),
            token: Some((self.at, self.at + quote.len_utf8())),
            label: Some("string starts here".to_owned()),
            ..IssueWording::default()
        }
    }

    fn prefix(&self) -> IssueWording {
        let name = self.found.as_str();
        let (prefix, local) = name.split_once(':').unwrap_or((name, ""));
        let core = FUNCTIONS.iter().any(|(function, _)| *function == local);
        IssueWording {
            message: format!("namespace prefix `{prefix}` is not declared"),
            help: if core {
                format!("`{local}()` is an XPath 1.0 core function; call it without a prefix")
            } else {
                "this XPath data model declares no namespace prefixes: call XPath 1.0 core functions without a prefix, and name elements without one".to_owned()
            },
            fix: core
                .then(|| {
                    self.fix(
                        self.at,
                        self.end,
                        format!("call `{local}()` without the prefix"),
                        local,
                    )
                })
                .flatten(),
            ..IssueWording::default()
        }
    }
}

/// Skips whitespace from `start`.
fn skip_whitespace(source: &str, start: usize) -> usize {
    let rest = source.get(start..).unwrap_or_default();
    start + (rest.len() - rest.trim_start().len())
}

/// `not(operand)`, without doubling parentheses the operand already has.
pub(super) fn not_call(operand: &str) -> String {
    if operand.starts_with('(') && operand.ends_with(')') {
        format!("not{operand}")
    } else {
        format!("not({operand})")
    }
}

/// End of the operand that starts at `start`: a variable, path, literal,
/// number, function call, or parenthesized expression, up to the first
/// whitespace or binary operator outside brackets and string literals.
pub(super) fn operand_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start;
    let mut depth = 0_usize;
    let mut quote: Option<u8> = None;
    while let Some(&byte) = bytes.get(index) {
        if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ if depth > 0 => {}
            b'=' | b'<' | b'>' | b'!' | b'|' | b'+' | b',' | b'&' | b'%' => break,
            _ if byte.is_ascii_whitespace() => break,
            _ => {}
        }
        index += 1;
    }
    (quote.is_none() && depth == 0 && index > start).then_some(index)
}

/// Offset of the innermost `open` before `limit` that no `close` matches,
/// ignoring delimiters inside string literals.
fn unmatched_opener(source: &str, limit: usize, open: char, close: char) -> Option<usize> {
    let mut stack = Vec::new();
    let mut quote: Option<char> = None;
    for (index, character) in source.get(..limit)?.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character == open {
            stack.push(index);
        } else if character == close {
            stack.pop();
        }
    }
    stack.pop()
}

/// A maybe-incorrect replacement of the decoded range `start..end` of
/// `site`, mapped through character and entity references onto exact source
/// bytes; the replacement is escaped for its context (attribute value with
/// its delimiter). `None` when no exact mapping exists or the replacement
/// cannot be written in that context.
pub(super) fn decoded_fix(
    document: &Document,
    site: &Site<'_>,
    start: usize,
    end: usize,
    message: String,
    replacement: &str,
) -> Option<Fix> {
    let offsets = site.offsets.as_ref()?;
    if end < start
        || (start == end && replacement.is_empty())
        || !site.source.is_char_boundary(start)
        || !site.source.is_char_boundary(end)
    {
        return None;
    }
    let (source_start, source_end) = (*offsets.get(start)?, *offsets.get(end)?);
    let replacement = escape(document, site, replacement)?;
    Some(Fix::replace(
        message,
        Applicability::MaybeIncorrect,
        Span::new(site.value_span.source, source_start, source_end),
        replacement,
    ))
}

/// Escapes `text` for the site's XML context: `&`, `<`, and the attribute
/// delimiter in an attribute value. Text content (a `<script>` body) takes
/// no replacement containing markup characters.
fn escape(document: &Document, site: &Site<'_>, text: &str) -> Option<String> {
    if site.attribute.is_none() {
        return (!text.contains(['&', '<'])).then(|| text.to_owned());
    }
    let delimiter = document
        .source
        .text()
        .get(..site.value_span.start)
        .and_then(|head| head.chars().next_back())
        .filter(|delimiter| matches!(delimiter, '"' | '\''))?;
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '"' if delimiter == '"' => escaped.push_str("&quot;"),
            '\'' if delimiter == '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    Some(escaped)
}

#[cfg(test)]
mod tests;
