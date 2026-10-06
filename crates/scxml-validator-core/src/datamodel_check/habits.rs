//! Operator habits from another language in one expression fragment.
//!
//! Agents write ECMAScript operators in `XPath` (`==`, `===`, `!==`, `&&`,
//! `||`, `!x`, `%`, uppercase `AND`/`OR`, and `not x`), and `XPath`/Python word
//! operators in ECMAScript (`and`, `or`, `not x`). A parser stops at the first
//! one, so a fragment with several habits would otherwise take one blind
//! repair round per habit, and a lexer error can surface on a later habit
//! than the first. [`scan`] finds every habit in one bounded pass over the
//! decoded fragment, outside string literals (and, for ECMAScript, template
//! and regular-expression literals and comments). The caller reports them
//! only when the fragment with every repair applied ([`apply_mapped`]) parses for the
//! same checker, so a recognized habit is never guessed at: either every
//! reported habit is a real mistake whose repairs together make the fragment
//! parse, or none is reported and the checker's own first issue is worded.
//!
//! Contract:
//!
//! * Habits are sorted, non-overlapping decoded byte ranges on character
//!   boundaries, at most [`MAX_HABITS`] per fragment.
//! * A repair whose meaning is certain (the operator has the same precedence
//!   and semantics in the target language) is machine-applicable; a repair
//!   that has to guess an operand's extent (`!x`, `not x`) is maybe-incorrect.
//! * Nothing is evaluated; the pass is linear in the fragment, which the
//!   caller already bounded (`MAX_CHECKED_EXPRESSION_BYTES`).

use crate::Applicability;

use super::xpath::operand_end;

/// Upper bound on habits collected from one fragment.
pub(super) const MAX_HABITS: usize = 64;

/// One recognized habit: the decoded range and its repair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Habit {
    /// Decoded start of the responsible bytes.
    pub start: usize,
    /// Decoded end of the responsible bytes (exclusive).
    pub end: usize,
    /// The repair: sorted, non-overlapping decoded edits.
    pub edits: Vec<Edit>,
    /// Diagnostic message.
    pub message: String,
    /// Concrete repair direction.
    pub help: String,
    /// Fix summary.
    pub fix_message: String,
    /// How certain the repair is.
    pub applicability: Applicability,
}

/// One decoded replacement of a habit repair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Edit {
    /// Decoded start of the replaced bytes.
    pub start: usize,
    /// Decoded end of the replaced bytes (exclusive).
    pub end: usize,
    /// Decoded replacement text (escaped for XML by the caller).
    pub replacement: String,
}

impl Edit {
    fn new(start: usize, end: usize, replacement: impl Into<String>) -> Self {
        Self {
            start,
            end,
            replacement: replacement.into(),
        }
    }
}

/// Every habit of `data_model` in `source`, in source order.
pub(super) fn scan(data_model: &str, source: &str) -> Vec<Habit> {
    match data_model {
        super::xpath::DATA_MODEL => scan_xpath(source),
        "ecmascript" => scan_ecmascript(source),
        _ => Vec::new(),
    }
}

#[cfg(test)]
/// `source` with every habit repaired; `None` when two repairs overlap.
pub(super) fn apply(source: &str, habits: &[Habit]) -> Option<String> {
    apply_mapped(source, habits).map(|(repaired, _)| repaired)
}

/// `source` with every habit repaired, and for every byte boundary of the
/// result (length `repaired.len() + 1`) the `source` offset it comes from;
/// bytes of a replacement map to the start of the range they replace.
/// `None` when two repairs overlap.
pub(super) fn apply_mapped(source: &str, habits: &[Habit]) -> Option<(String, Vec<usize>)> {
    let mut repaired = String::with_capacity(source.len() + 16);
    let mut offsets = Vec::with_capacity(source.len() + 17);
    let mut cursor = 0;
    let keep = |repaired: &mut String, offsets: &mut Vec<usize>, from: usize, to: usize| {
        let text = source.get(from..to)?;
        offsets.extend(from..to);
        repaired.push_str(text);
        Some(())
    };
    let mut edits: Vec<&Edit> = habits.iter().flat_map(|habit| &habit.edits).collect();
    edits.sort_by_key(|edit| (edit.start, edit.end));
    for edit in edits {
        if edit.start < cursor || edit.end < edit.start {
            return None;
        }
        keep(&mut repaired, &mut offsets, cursor, edit.start)?;
        offsets.extend(std::iter::repeat_n(edit.start, edit.replacement.len()));
        repaired.push_str(&edit.replacement);
        cursor = edit.end;
    }
    keep(&mut repaired, &mut offsets, cursor, source.len())?;
    offsets.push(source.len());
    Some((repaired, offsets))
}

/// `replacement` with a space added on each side that touches a non-space
/// character, so a word operator never fuses with its operands.
fn spaced(source: &str, start: usize, end: usize, replacement: &str) -> String {
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

/// An operator token replaced in place.
#[allow(
    clippy::too_many_arguments,
    reason = "one flat constructor keeps each habit table entry readable"
)]
fn operator(
    source: &str,
    start: usize,
    end: usize,
    replacement: &str,
    word: bool,
    message: String,
    help: &str,
    fix_message: String,
) -> Habit {
    Habit {
        start,
        end,
        edits: vec![Edit::new(
            start,
            end,
            if word {
                spaced(source, start, end, replacement)
            } else {
                replacement.to_owned()
            },
        )],
        message,
        help: help.to_owned(),
        fix_message,
        applicability: Applicability::MachineApplicable,
    }
}

/// What the previous significant token was.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Previous {
    /// Start of the fragment, an operator, or an opening delimiter: an
    /// operand comes next.
    Operator,
    /// The end of a complete operand: an operator comes next.
    Operand,
}

fn is_xpath_name_start(character: char) -> bool {
    character.is_alphabetic() || character == '_'
}

fn is_xpath_name_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '-' | '_' | '\u{b7}')
}

/// End of the name that starts at `start`.
fn name_end(source: &str, start: usize, is_char: fn(char) -> bool) -> usize {
    source
        .get(start..)
        .and_then(|rest| {
            rest.char_indices()
                .find(|(_, character)| !is_char(*character))
                .map(|(index, _)| start + index)
        })
        .unwrap_or(source.len())
}

/// First non-whitespace offset at or after `start`.
fn skip_whitespace(source: &str, start: usize) -> usize {
    let rest = source.get(start..).unwrap_or_default();
    start + (rest.len() - rest.trim_start().len())
}

/// `!operand` or `not operand` as `not(operand)`: the operator becomes
/// `not(` (or `not` before a parenthesized operand) and a `)` is inserted
/// after the operand, so the operand's own bytes are kept and habits inside
/// it are still found. Returns the habit and the operand's start, where
/// scanning continues.
fn xpath_negation(
    source: &str,
    start: usize,
    operator_end: usize,
    message: &str,
    help: &str,
) -> Option<(Habit, usize)> {
    let operand = skip_whitespace(source, operator_end);
    let edits = if source.get(operand..)?.starts_with('(') {
        vec![Edit::new(start, operand, "not")]
    } else {
        let end = operand_end(source, operand)?;
        vec![Edit::new(start, operand, "not("), Edit::new(end, end, ")")]
    };
    Some((
        Habit {
            start,
            end: operator_end,
            edits,
            message: message.to_owned(),
            help: help.to_owned(),
            fix_message: "negate with `not()`".to_owned(),
            applicability: Applicability::MaybeIncorrect,
        },
        operand,
    ))
}

#[allow(
    clippy::too_many_lines,
    reason = "one lexer loop over every token class; splitting it would scatter the shared operand state"
)]
fn scan_xpath(source: &str) -> Vec<Habit> {
    let mut habits = Vec::new();
    let mut previous = Previous::Operator;
    let mut index = 0;
    while habits.len() < MAX_HABITS {
        let Some(character) = source.get(index..).and_then(|rest| rest.chars().next()) else {
            break;
        };
        let next = source
            .get(index + character.len_utf8()..)
            .and_then(|rest| rest.chars().next());
        if character.is_whitespace() {
            index += character.len_utf8();
            continue;
        }
        match character {
            '\'' | '"' => {
                // XPath 1.0 literals have no escapes.
                let Some(close) = source
                    .get(index + 1..)
                    .and_then(|rest| rest.find(character))
                else {
                    break;
                };
                index += close + 2;
                previous = Previous::Operand;
            }
            '&' if next == Some('&') => {
                habits.push(operator(
                    source,
                    index,
                    index + 2,
                    "and",
                    true,
                    "`&&` is not an XPath operator".to_owned(),
                    "XPath spells logical and as `and`",
                    "use `and`".to_owned(),
                ));
                index += 2;
                previous = Previous::Operator;
            }
            '|' if next == Some('|') => {
                habits.push(operator(
                    source,
                    index,
                    index + 2,
                    "or",
                    true,
                    "`||` is not an XPath operator".to_owned(),
                    "XPath spells logical or as `or`; a single `|` is the node-set union",
                    "use `or`".to_owned(),
                ));
                index += 2;
                previous = Previous::Operator;
            }
            '=' => {
                let run = source
                    .get(index..)
                    .map_or(1, |rest| rest.len() - rest.trim_start_matches('=').len());
                if run >= 2 {
                    let spelled = "=".repeat(run);
                    habits.push(operator(
                        source,
                        index,
                        index + run,
                        "=",
                        false,
                        format!("`{spelled}` is not an XPath operator"),
                        "XPath compares with a single `=` (and `!=` for inequality)",
                        "compare with `=`".to_owned(),
                    ));
                }
                index += run;
                previous = Previous::Operator;
            }
            '!' if next == Some('=') => {
                let run = source
                    .get(index + 1..)
                    .map_or(1, |rest| rest.len() - rest.trim_start_matches('=').len());
                if run >= 2 {
                    let spelled = format!("!{}", "=".repeat(run));
                    habits.push(operator(
                        source,
                        index,
                        index + 1 + run,
                        "!=",
                        false,
                        format!("`{spelled}` is not an XPath operator"),
                        "XPath tests inequality with `!=`",
                        "compare with `!=`".to_owned(),
                    ));
                }
                index += 1 + run;
                previous = Previous::Operator;
            }
            '!' if previous == Previous::Operator => {
                let Some((habit, resume)) = xpath_negation(
                    source,
                    index,
                    index + 1,
                    "`!` is not an XPath operator",
                    "negate with the `not()` function, for example `not($done)`; `!=` is the only XPath operator with `!`",
                ) else {
                    break;
                };
                habits.push(habit);
                // A parenthesized operand is scanned from its `(` (which
                // resets the state); a simple one is skipped as one operand.
                previous = Previous::Operator;
                index = resume;
            }
            '%' if previous == Previous::Operand => {
                habits.push(operator(
                    source,
                    index,
                    index + 1,
                    "mod",
                    true,
                    "`%` is not an XPath operator".to_owned(),
                    "XPath spells the remainder operator `mod`",
                    "use `mod`".to_owned(),
                ));
                index += 1;
                previous = Previous::Operator;
            }
            '<' | '>' => {
                index += if next == Some('=') { 2 } else { 1 };
                previous = Previous::Operator;
            }
            ')' | ']' | '.' => {
                index += 1;
                previous = Previous::Operand;
            }
            '*' => {
                index += 1;
                previous = if previous == Previous::Operand {
                    Previous::Operator
                } else {
                    Previous::Operand
                };
            }
            '$' => {
                index = name_end(source, index + 1, is_xpath_name_char).max(index + 1);
                previous = Previous::Operand;
            }
            _ if character.is_ascii_digit() => {
                index = name_end(source, index, |character| {
                    character.is_ascii_digit() || character == '.'
                });
                previous = Previous::Operand;
            }
            _ if is_xpath_name_start(character) => {
                let end = name_end(source, index, is_xpath_name_char);
                let word = source.get(index..end).unwrap_or_default();
                let lowercase = word.to_ascii_lowercase();
                if previous == Previous::Operand
                    && matches!(lowercase.as_str(), "and" | "or" | "div" | "mod")
                {
                    if word != lowercase {
                        habits.push(operator(
                            source,
                            index,
                            end,
                            &lowercase,
                            false,
                            format!("XPath operators are lowercase: `{word}` is not an operator"),
                            &format!("write `{lowercase}`"),
                            format!("use `{lowercase}`"),
                        ));
                    }
                    index = end;
                    previous = Previous::Operator;
                    continue;
                }
                let after = skip_whitespace(source, end);
                if previous == Previous::Operator
                    && word == "not"
                    && after > end
                    && source
                        .get(after..)
                        .and_then(|rest| rest.chars().next())
                        .is_some_and(|next| {
                            next == '$' || next == '\'' || next == '"' || is_xpath_name_start(next)
                        })
                {
                    let Some((habit, resume)) = xpath_negation(
                        source,
                        index,
                        end,
                        "`not` is a function in XPath, not an operator",
                        "call it with parentheses, for example `not($done)`",
                    ) else {
                        break;
                    };
                    habits.push(Habit {
                        fix_message: "call `not()` with parentheses".to_owned(),
                        ..habit
                    });
                    index = resume;
                    previous = Previous::Operator;
                    continue;
                }
                index = end;
                // A function name or axis is followed by `(` or `::`, which
                // set the state; a name test ends an operand.
                previous = Previous::Operand;
            }
            _ => {
                index += character.len_utf8();
                previous = Previous::Operator;
            }
        }
    }
    habits
}

fn is_ecmascript_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$')
}

/// ECMAScript keywords after which an operand follows.
const OPERAND_KEYWORDS: [&str; 12] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
];

/// Whether an ECMAScript operand can start at `offset`.
fn starts_ecmascript_operand(source: &str, offset: usize) -> bool {
    source
        .get(offset..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|character| {
            is_ecmascript_identifier_char(character)
                || matches!(
                    character,
                    '\'' | '"' | '`' | '(' | '[' | '!' | '-' | '+' | '{'
                )
        })
}

/// End of the ECMAScript string or template literal that starts at `start`.
fn ecmascript_literal_end(source: &str, start: usize, quote: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'\\' {
            index += 2;
            continue;
        }
        if byte == quote {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

#[allow(
    clippy::too_many_lines,
    reason = "one lexer loop over every token class; splitting it would scatter the shared operand state"
)]
fn scan_ecmascript(source: &str) -> Vec<Habit> {
    let bytes = source.as_bytes();
    let mut habits = Vec::new();
    let mut previous = Previous::Operator;
    let mut index = 0;
    while habits.len() < MAX_HABITS {
        let Some(character) = source.get(index..).and_then(|rest| rest.chars().next()) else {
            break;
        };
        if character.is_whitespace() {
            index += character.len_utf8();
            continue;
        }
        match character {
            '\'' | '"' | '`' => {
                let Some(end) = ecmascript_literal_end(source, index, character as u8) else {
                    break;
                };
                index = end;
                previous = Previous::Operand;
            }
            '/' if bytes.get(index + 1) == Some(&b'/') => {
                index = source
                    .get(index..)
                    .and_then(|rest| rest.find('\n'))
                    .map_or(source.len(), |newline| index + newline);
            }
            '/' if bytes.get(index + 1) == Some(&b'*') => {
                let Some(close) = source.get(index + 2..).and_then(|rest| rest.find("*/")) else {
                    break;
                };
                index += close + 4;
            }
            '/' if previous == Previous::Operator => {
                // Regular-expression literal, honoring escapes and classes.
                let mut class = false;
                index += 1;
                while let Some(&inner) = bytes.get(index) {
                    match inner {
                        b'\\' => index += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        b'/' if !class => break,
                        b'\n' => break,
                        _ => {}
                    }
                    index += 1;
                }
                index += 1;
                index = name_end(
                    source,
                    index.min(source.len()),
                    is_ecmascript_identifier_char,
                );
                previous = Previous::Operand;
            }
            ')' | ']' | '}' => {
                index += 1;
                previous = Previous::Operand;
            }
            _ if character.is_ascii_digit() => {
                index = name_end(source, index, |character| {
                    character.is_alphanumeric() || matches!(character, '.' | '_')
                });
                previous = Previous::Operand;
            }
            _ if is_ecmascript_identifier_char(character) => {
                let end = name_end(source, index, is_ecmascript_identifier_char);
                let word = source.get(index..end).unwrap_or_default();
                let after = skip_whitespace(source, end);
                let member = source
                    .get(..index)
                    .and_then(|head| head.trim_end().chars().next_back())
                    == Some('.');
                let spelled = word.to_ascii_lowercase();
                if !member
                    && previous == Previous::Operand
                    && matches!(spelled.as_str(), "and" | "or")
                    && after > end
                    && starts_ecmascript_operand(source, after)
                {
                    let (replacement, help) = if spelled == "and" {
                        (
                            "&&",
                            "write `&&` (escaped as `&amp;&amp;` inside an XML attribute)",
                        )
                    } else {
                        ("||", "write `||`")
                    };
                    habits.push(operator(
                        source,
                        index,
                        end,
                        replacement,
                        true,
                        format!("`{word}` is not an ECMAScript operator"),
                        help,
                        format!("use `{replacement}`"),
                    ));
                    index = end;
                    previous = Previous::Operator;
                    continue;
                }
                if !member
                    && previous == Previous::Operator
                    && word == "not"
                    && after > end
                    && starts_ecmascript_operand(source, after)
                    && !source.get(after..).is_some_and(|rest| {
                        rest.starts_with("in ") || rest.starts_with("instanceof ")
                    })
                {
                    habits.push(Habit {
                        start: index,
                        end,
                        edits: vec![Edit::new(index, after, "!")],
                        message: "`not` is not an ECMAScript operator".to_owned(),
                        help: "write `!` before the operand; `!` binds tighter than a comparison, so negate a comparison as `!(a == b)`".to_owned(),
                        fix_message: "use `!`".to_owned(),
                        applicability: Applicability::MaybeIncorrect,
                    });
                    index = after;
                    previous = Previous::Operator;
                    continue;
                }
                index = end;
                previous = if !member && OPERAND_KEYWORDS.contains(&word) {
                    Previous::Operator
                } else {
                    Previous::Operand
                };
            }
            _ => {
                index += character.len_utf8();
                previous = Previous::Operator;
            }
        }
    }
    habits
}

#[cfg(test)]
mod tests {
    use super::{apply, scan};
    use crate::Applicability;

    fn repaired(data_model: &str, source: &str) -> String {
        apply(source, &scan(data_model, source)).expect("habits do not overlap")
    }

    #[test]
    fn every_xpath_habit_is_found_and_repaired_together() {
        assert_eq!(
            repaired("xpath", "$a > 1 && $a < 9 || $a == 0"),
            "$a > 1 and $a < 9 or $a = 0"
        );
        assert_eq!(
            repaired("xpath", "$count == 1 && true()"),
            "$count = 1 and true()"
        );
        assert_eq!(
            repaired("xpath", "$a === 1 AND $b !== 2"),
            "$a = 1 and $b != 2"
        );
        assert_eq!(repaired("xpath", "$a%2 = 0"), "$a mod 2 = 0");
        assert_eq!(repaired("xpath", "!($a == 1)"), "not($a = 1)");
        assert_eq!(
            repaired("xpath", "!$done and not $x"),
            "not($done) and not($x)"
        );
        let habits = scan("xpath", "$count == 1 && true()");
        assert_eq!((habits[0].start, habits[0].end), (7, 9), "`==` comes first");
        assert!(
            habits
                .iter()
                .all(|habit| habit.applicability == Applicability::MachineApplicable)
        );
    }

    #[test]
    fn xpath_literals_and_valid_operators_are_not_habits() {
        for source in [
            "'a && b' = $x",
            "\"x == y\"",
            "$a != 1",
            "$a <= 1 or $a >= 2",
            "not($a)",
            "$a | $b",
            "$and-or",
            "count($a/and)",
            "$x mod 2",
            "10 div 2",
            "$a/*",
            "2 * 3",
        ] {
            assert!(scan("xpath", source).is_empty(), "{source}");
        }
    }

    #[test]
    fn every_ecmascript_word_operator_is_found() {
        assert_eq!(
            repaired("ecmascript", "x > 1 or x < 0 and y"),
            "x > 1 || x < 0 && y"
        );
        assert_eq!(
            repaired("ecmascript", "x == 1 and not ready"),
            "x == 1 && !ready"
        );
        let habits = scan("ecmascript", "a and not b");
        assert_eq!(habits[0].applicability, Applicability::MachineApplicable);
        assert_eq!(habits[1].applicability, Applicability::MaybeIncorrect);
    }

    #[test]
    fn ecmascript_identifiers_and_literals_are_not_habits() {
        for source in [
            "and",
            "or + 1",
            "x.and",
            "'a and b'",
            "`x or y`",
            "/ and /.test(s)",
            "not",
            "f(and, or)",
            "x // and y",
            "typeof and",
        ] {
            assert!(scan("ecmascript", source).is_empty(), "{source}");
        }
    }
}
