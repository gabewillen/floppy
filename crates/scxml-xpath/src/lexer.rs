//! XPath 1.0 tokenizer with the §3.7 lexical disambiguation rules.

use alloc::borrow::ToOwned as _;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{SyntaxError, is_name_char, is_name_start, is_xml_space};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Axis {
    Ancestor,
    AncestorOrSelf,
    Attribute,
    Child,
    Descendant,
    DescendantOrSelf,
    Following,
    FollowingSibling,
    Namespace,
    Parent,
    Preceding,
    PrecedingSibling,
    SelfNode,
}

impl Axis {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "ancestor" => Self::Ancestor,
            "ancestor-or-self" => Self::AncestorOrSelf,
            "attribute" => Self::Attribute,
            "child" => Self::Child,
            "descendant" => Self::Descendant,
            "descendant-or-self" => Self::DescendantOrSelf,
            "following" => Self::Following,
            "following-sibling" => Self::FollowingSibling,
            "namespace" => Self::Namespace,
            "parent" => Self::Parent,
            "preceding" => Self::Preceding,
            "preceding-sibling" => Self::PrecedingSibling,
            "self" => Self::SelfNode,
            _ => return None,
        })
    }

    pub(crate) const fn is_reverse(self) -> bool {
        matches!(
            self,
            Self::Ancestor | Self::AncestorOrSelf | Self::Preceding | Self::PrecedingSibling
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeType {
    Comment,
    Text,
    ProcessingInstruction,
    Node,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operator {
    And,
    Or,
    Mod,
    Div,
    Multiply,
    Slash,
    DoubleSlash,
    Pipe,
    Plus,
    Minus,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token {
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Dot,
    DotDot,
    At,
    Comma,
    ColonColon,
    /// `*`
    AnyName,
    /// `prefix:*`
    PrefixAny(String),
    /// `prefix:local` or `local` as a name test.
    Name(Option<String>, String),
    NodeType(NodeType),
    Operator(Operator),
    /// Function name (lexical QName) followed by `(`.
    Function(String),
    AxisName(Axis),
    Literal(String),
    Number(f64),
    /// Variable reference: lexical QName after `$`.
    Variable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spanned {
    pub(crate) token: Token,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

fn error(start: usize, end: usize, message: &'static str) -> SyntaxError {
    SyntaxError {
        start,
        end,
        message,
    }
}

/// Whether the token before position allows an operator reading (§3.7).
fn operator_context(previous: Option<&Token>) -> bool {
    match previous {
        None => false,
        Some(token) => !matches!(
            token,
            Token::At
                | Token::ColonColon
                | Token::LeftParen
                | Token::LeftBracket
                | Token::Comma
                | Token::Operator(_)
        ),
    }
}

pub(crate) fn tokenize(source: &str) -> Result<Vec<Spanned>, SyntaxError> {
    let mut tokens: Vec<Spanned> = Vec::new();
    let bytes = source.as_bytes();
    let mut position = 0;
    while position < source.len() {
        let rest = source.get(position..).unwrap_or_default();
        let Some(character) = rest.chars().next() else {
            break;
        };
        if is_xml_space(character) {
            position = position.saturating_add(1);
            continue;
        }
        let start = position;
        let previous = tokens.last().map(|spanned| &spanned.token);
        let operator_mode = operator_context(previous);
        let next_byte = bytes.get(position.saturating_add(1)).copied();
        let (token, length) = match character {
            '(' => (Token::LeftParen, 1),
            ')' => (Token::RightParen, 1),
            '[' => (Token::LeftBracket, 1),
            ']' => (Token::RightBracket, 1),
            '@' => (Token::At, 1),
            ',' => (Token::Comma, 1),
            ':' if next_byte == Some(b':') => (Token::ColonColon, 2),
            '.' if next_byte == Some(b'.') => (Token::DotDot, 2),
            '.' if next_byte.is_some_and(|byte| byte.is_ascii_digit()) => number(rest, start)?,
            '.' => (Token::Dot, 1),
            '0'..='9' => number(rest, start)?,
            '"' | '\'' => {
                let body = rest.get(1..).unwrap_or_default();
                let end = body
                    .find(character)
                    .ok_or_else(|| error(start, source.len(), "unterminated string literal"))?;
                (
                    Token::Literal(body.get(..end).unwrap_or_default().to_owned()),
                    end.saturating_add(2),
                )
            }
            '/' if next_byte == Some(b'/') => (Token::Operator(Operator::DoubleSlash), 2),
            '/' => (Token::Operator(Operator::Slash), 1),
            '|' => (Token::Operator(Operator::Pipe), 1),
            '+' => (Token::Operator(Operator::Plus), 1),
            '-' => (Token::Operator(Operator::Minus), 1),
            '=' => (Token::Operator(Operator::Equal), 1),
            '!' if next_byte == Some(b'=') => (Token::Operator(Operator::NotEqual), 2),
            '<' if next_byte == Some(b'=') => (Token::Operator(Operator::LessEqual), 2),
            '<' => (Token::Operator(Operator::Less), 1),
            '>' if next_byte == Some(b'=') => (Token::Operator(Operator::GreaterEqual), 2),
            '>' => (Token::Operator(Operator::Greater), 1),
            '*' if operator_mode => (Token::Operator(Operator::Multiply), 1),
            '*' => (Token::AnyName, 1),
            '$' => {
                let (name, length) = qname(rest.get(1..).unwrap_or_default()).ok_or_else(|| {
                    error(
                        start,
                        start.saturating_add(1),
                        "expected a variable name after '$'",
                    )
                })?;
                (Token::Variable(name), length.saturating_add(1))
            }
            _ if is_name_start(character) => name_token(source, rest, start, operator_mode)?,
            _ => {
                return Err(error(
                    start,
                    start.saturating_add(character.len_utf8()),
                    "unexpected character",
                ));
            }
        };
        position = position.saturating_add(length);
        tokens.push(Spanned {
            token,
            start,
            end: position,
        });
    }
    Ok(tokens)
}

fn number(rest: &str, start: usize) -> Result<(Token, usize), SyntaxError> {
    let mut length = 0;
    let mut seen_dot = false;
    for character in rest.chars() {
        if character.is_ascii_digit() {
            length += 1;
        } else if character == '.' && !seen_dot {
            seen_dot = true;
            length += 1;
        } else {
            break;
        }
    }
    let text = rest.get(..length).unwrap_or_default();
    let value = text
        .parse::<f64>()
        .map_err(|_| error(start, start.saturating_add(length), "malformed number"))?;
    Ok((Token::Number(value), length))
}

/// Length in bytes of the `NCName` at the start of `text`.
fn ncname_length(text: &str) -> usize {
    let mut length = 0;
    for (index, character) in text.char_indices() {
        let valid = if index == 0 {
            is_name_start(character)
        } else {
            is_name_char(character)
        };
        if !valid {
            break;
        }
        length = index.saturating_add(character.len_utf8());
    }
    length
}

/// A lexical `QName` at the start of `text` (no whitespace around `:`).
fn qname(text: &str) -> Option<(String, usize)> {
    let first = ncname_length(text);
    if first == 0 {
        return None;
    }
    let after = text.get(first..).unwrap_or_default();
    if after.starts_with(':') && !after.starts_with("::") {
        let second = ncname_length(after.get(1..).unwrap_or_default());
        if second > 0 {
            let length = first.saturating_add(1).saturating_add(second);
            return Some((text.get(..length)?.to_owned(), length));
        }
    }
    Some((text.get(..first)?.to_owned(), first))
}

fn name_token(
    source: &str,
    rest: &str,
    start: usize,
    operator_mode: bool,
) -> Result<(Token, usize), SyntaxError> {
    let first = ncname_length(rest);
    let local = rest.get(..first).unwrap_or_default();
    if operator_mode {
        let operator = match local {
            "and" => Operator::And,
            "or" => Operator::Or,
            "mod" => Operator::Mod,
            "div" => Operator::Div,
            _ => {
                return Err(error(
                    start,
                    start.saturating_add(first),
                    "expected an operator",
                ));
            }
        };
        return Ok((Token::Operator(operator), first));
    }
    let after = rest.get(first..).unwrap_or_default();
    // prefix:* or prefix:local
    if after.starts_with(':') && !after.starts_with("::") {
        let tail = after.get(1..).unwrap_or_default();
        if tail.starts_with('*') {
            return Ok((Token::PrefixAny(local.to_owned()), first.saturating_add(2)));
        }
        let second = ncname_length(tail);
        if second == 0 {
            return Err(error(
                start,
                start.saturating_add(first).saturating_add(1),
                "malformed qualified name",
            ));
        }
        let length = first.saturating_add(1).saturating_add(second);
        let local_part = tail.get(..second).unwrap_or_default();
        let lookahead = source
            .get(start.saturating_add(length)..)
            .unwrap_or_default()
            .trim_start_matches(is_xml_space);
        if lookahead.starts_with('(') {
            return Ok((
                Token::Function(rest.get(..length).unwrap_or_default().to_owned()),
                length,
            ));
        }
        return Ok((
            Token::Name(Some(local.to_owned()), local_part.to_owned()),
            length,
        ));
    }
    let lookahead = after.trim_start_matches(is_xml_space);
    if lookahead.starts_with('(') {
        let node_type = match local {
            "comment" => Some(NodeType::Comment),
            "text" => Some(NodeType::Text),
            "processing-instruction" => Some(NodeType::ProcessingInstruction),
            "node" => Some(NodeType::Node),
            _ => None,
        };
        return Ok((
            node_type.map_or_else(|| Token::Function(local.to_owned()), Token::NodeType),
            first,
        ));
    }
    if lookahead.starts_with("::") {
        let axis = Axis::from_name(local)
            .ok_or_else(|| error(start, start.saturating_add(first), "unknown axis"))?;
        return Ok((Token::AxisName(axis), first));
    }
    Ok((Token::Name(None, local.to_owned()), first))
}
