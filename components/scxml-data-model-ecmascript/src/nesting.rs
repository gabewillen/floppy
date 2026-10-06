//! Structural pre-parse nesting guard for ECMAScript sources.
//!
//! Boa's parser, scope analysis, bytecode compiler, and AST drop all recurse
//! natively once per syntactic nesting level, and Boa 0.22 exposes no parser
//! depth limit. Every source this crate hands to Boa therefore passes
//! [`check_nesting`] first, so parse recursion is bounded before Boa sees the
//! text and hostile nesting fails with a typed error instead of a native stack
//! overflow.
//!
//! # Contract
//!
//! * Pure, deterministic, and bounded: forward passes over the source bytes
//!   with `O(1)` state per open bracket, at most `1 +`
//!   [`MAX_LEXICAL_FORKS`] passes (see *Soundness*); the verdict depends only
//!   on the source text, never on the build profile, target, or thread.
//! * Cost model: the value bounded is the sum over the open bracket frames
//!   (`(`, `[`, `{`, and template `${`, each [`NESTING_BRACKET_WEIGHT`]
//!   units) of the recursion-inducing tokens not yet closed in each frame:
//!   statement heads (`if`, `else`, `for`, `while`, `do`, `with`, `try`,
//!   `switch`, `return`, `throw`, declarations, labels) and expression
//!   operators (`?`, `:`, assignments, `=>`, `**`, prefix operators, `...`,
//!   `||`, `&&`, `??`, `new`, `typeof`, `void`, `delete`, `await`, `yield`,
//!   `function`, `class`, `extends`; `++`/`--` weigh
//!   [`NESTING_INCREMENT_WEIGHT`] units), plus one unit per
//!   [`CHAIN_ELEMENTS_PER_UNIT`] left-nested chain elements: binary operators
//!   (`a+b+c` builds a left-deep tree that Boa's compiler and AST drop walk
//!   recursively), member accesses, calls, and index or tagged-template
//!   applications after an operand, and sequence-expression commas. Tokens
//!   reset only where the grammar guarantees the construct they opened has
//!   ended: `,` ends expression-level nesting and chains (a list comma in an
//!   array literal, object literal, or argument list also ends the element; a
//!   sequence comma is itself a chain element), and `;`, `case`/`default`,
//!   and a closing `}` followed by a new statement end statement-level
//!   nesting and sequences (never before `else`, `while`, `catch`, `finally`,
//!   or an operator). Wherever a context decision is uncertain the scanner
//!   takes the costlier reading: a comma is a list comma only in a bracket
//!   that is certainly an array literal (never an index, an optional-chain
//!   index `?.[`, or a computed key), a
//!   brace that is certainly an object literal, or a parenthesis that is
//!   certainly an argument list (it follows an operand on the same line); an
//!   uncertain previous token charges both the chain and the prefix-operator
//!   cost. Sources over [`MAX_PARSE_NESTING`] units are declined.
//! * Soundness (the guard's purpose): the bound is an upper bound on the
//!   recursion Boa performs for the source, because the scanner sees every
//!   bracket Boa parses as code. The argument:
//!   1. Every lexical decision except the meaning of `/` is context-free
//!      given the open-bracket stack, and the scanner makes it exactly as
//!      Boa's lexer does (Boa is built without `annex-b`, so `<!--` and
//!      `-->` are operators, not comments): comments, string literals (an
//!      unescaped CR or LF ends one; U+2028/U+2029 do not), template
//!      literals (a `}` closing a `${` frame resumes template characters),
//!      regular-expression bodies (classes, escapes, no line terminators),
//!      numbers, identifiers with Unicode escapes, a hashbang at byte 0, and
//!      Boa's exact white-space and line-terminator sets. A character the
//!      scanner merges into an identifier that Boa does not accept as one
//!      makes Boa fail at that character, and a failing parse stops there.
//!   2. A `/` is a regular expression or division depending only on the
//!      parser's position. The scanner settles it from the previous token
//!      alone, and only where the grammar leaves one reading or a syntax
//!      error at that very token (Boa re-lexes a division token as a
//!      regular expression wherever it parses a primary expression, and a
//!      regular-expression token where an operator is required is an
//!      error):
//!      * regular expression after a punctuator other than `)`, `]`, `}`,
//!        `++`, `--`; after the `)` of an `if`/`while`/`for`/`for await`/
//!        `with`/`switch`/`catch` head spelled without escapes; and after
//!        the unescaped keywords that must be followed by an operand or a
//!        statement (`return`, `typeof`, `case`, `else`, `do`, `break`, …);
//!      * division after an operand (identifier, literal, `this`, the `)` of
//!        any other parenthesis, `]`, a template or regular-expression end)
//!        with no line terminator in between: automatic semicolon insertion
//!        needs a line terminator, so no other reading parses;
//!      * otherwise (after `}` of any block, body, or object, after
//!        `++`/`--`, after contextual words such as `let`, `of`, `yield`,
//!        `await`, `async`, after escaped keywords and their heads, and
//!        after an operand followed by a line terminator — a binding name
//!        such as `var x` or `break l` ends its statement there) the `/` is
//!        ambiguous. If no regular-expression end exists on its line the
//!        regular-expression reading is a syntax error at this token, so only
//!        the division reading can continue. Otherwise the scanner forks:
//!        the regular-expression reading is queued as a separate pass from
//!        this point and the current pass continues as division.
//!   3. By induction over the `/` tokens, the token stream Boa parses up to
//!      its first error (or the end) is the stream of one explored pass, so
//!      the largest nesting over all passes bounds Boa's recursion; the
//!      source is accepted only when every pass stays within
//!      [`MAX_PARSE_NESTING`]. More than [`MAX_LEXICAL_FORKS`] forks decline
//!      the source (the passes are not guessed at).
//! * Failure: [`DataModelError::InvalidInput`], with no source text retained
//!   or reported.
//! * Telemetry: none. [`check_nesting`] is a validation
//!   step of the enclosing data-model operation (initialize, evaluate,
//!   assign, execute, or check-syntax), whose span records their
//!   rejection as that operation's typed failure.

use crate::DataModelError;

/// Largest accepted structural nesting, in nesting units, on every build.
///
/// A native stack guard rather than a resource bound: Boa recurses natively
/// once per level, so an unguarded source would overflow the stack and abort
/// the process. It therefore applies to native builds too (the resource
/// bounds of [`scxml_types::wasm_bound`] do not include it).
///
/// Sized so that one parse, compile, and drop at the bound stays well inside
/// the documented stacks: `tests/parse_nesting_bound.rs` runs every recursive
/// construct and chain at its largest accepted size, plus generated programs,
/// within 8 MiB in unoptimized builds (optimized builds need a fraction of
/// that). Native data-model calls run on a dedicated
/// [`crate::PARSE_STACK_BYTES`] thread.
pub const MAX_PARSE_NESTING: usize = 128;
/// Nesting units charged for each open `(`, `[`, `{`, or template `${`.
pub const NESTING_BRACKET_WEIGHT: usize = 8;
/// Nesting units charged for each `++` or `--`.
pub const NESTING_INCREMENT_WEIGHT: usize = 2;
/// Left-nested chain elements (binary operators, member accesses, calls,
/// sequence commas) charged as one nesting unit. One chain element costs Boa
/// well under a sixteenth of one nesting unit's stack.
pub const CHAIN_ELEMENTS_PER_UNIT: usize = 16;
/// Most context-ambiguous `/` tokens whose regular-expression reading is
/// scanned as a separate pass; a source needing more is declined. Bounds the
/// scan at `1 + MAX_LEXICAL_FORKS` passes over the source.
pub const MAX_LEXICAL_FORKS: usize = 16;

/// Declines `source` when its structural nesting could exceed
/// [`MAX_PARSE_NESTING`] under any lexical reading Boa may take.
///
/// # Errors
///
/// [`DataModelError::InvalidInput`] when the source is over the bound in any
/// explored reading or needs more than [`MAX_LEXICAL_FORKS`] readings.
pub fn check_nesting(source: &str) -> Result<(), DataModelError> {
    scan(source)
}

/// Runs every lexical reading of `source` to its end; see the module
/// contract's soundness argument.
fn scan(source: &str) -> Result<(), DataModelError> {
    let mut forks = Forks {
        pending: vec![Scanner::new(source)],
        remaining: MAX_LEXICAL_FORKS,
    };
    while let Some(mut pass) = forks.pending.pop() {
        pass.run(&mut forks)?;
    }
    Ok(())
}

/// Readings queued for their own pass, and the forks still allowed.
struct Forks<'a> {
    pending: Vec<Scanner<'a>>,
    remaining: usize,
}

/// What the previous significant token lets the next `/`, `+`, `-`, `(`,
/// `[`, or `{` mean.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Prev {
    /// Statement start: a regular expression, a block, or a label may follow.
    Start,
    /// Inside an expression awaiting an operand: a regular expression or an
    /// object literal may follow.
    Operator,
    /// An operand ended: on the same line `/` divides and `+`/`-` are binary.
    ExprEnd,
    /// The scanner cannot tell which of the above applies: every decision
    /// takes the reading that charges more, and a `/` forks.
    Ambiguous,
}

/// What a previous punctuator settles for the next token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum After {
    /// Nothing beyond [`Prev`].
    Other,
    /// `.` or `?.`: the next word is a property name.
    Dot,
    /// `=>`: a `{` opens a function body.
    Arrow,
    /// `{`, `,`, `;`, or `*`, where a class or object member (and so a
    /// computed key) may start.
    MemberStart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BraceKind {
    /// A statement block: a statement starts after its `}`.
    Block,
    /// An object literal (or class body, destructuring pattern).
    Object,
    /// A function, arrow, or class body, or undecidable.
    Unknown,
}

/// Whether a parenthesis is a statement head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Head {
    /// Not a head: a call, grouping, or parameter list.
    No,
    /// The head of an unescaped `if`/`while`/`for`/`for await`/`with`/
    /// `switch`/`catch`: a statement follows its `)`.
    Statement,
    /// After an escaped keyword: Boa rejects it, but the scanner does not
    /// rely on that and treats its `)` as ambiguous.
    Escaped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Opener {
    Root,
    /// `list`: an argument list (the `(` follows an operand on its line).
    Paren {
        head: Head,
        list: bool,
    },
    /// `list`: certainly an array literal or pattern (not an index or a
    /// computed key, whose commas build sequences).
    Bracket {
        list: bool,
    },
    Brace(BraceKind),
    Template,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    None,
    Semicolon,
    BraceClose,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    opener: Opener,
    /// Open statement-level recursion in this frame.
    stmt: usize,
    /// Open expression-level recursion in this frame.
    expr: usize,
    /// Left-nested chain elements of the current expression.
    chain: usize,
    /// Sequence-expression commas of the current statement.
    sequence: usize,
    pending: Pending,
    /// Colons still owed to an open `case`/`default` clause head (each `?`
    /// in the head owes one more).
    case_colons: usize,
}

impl Frame {
    const fn new(opener: Opener) -> Self {
        Self {
            opener,
            stmt: 0,
            expr: 0,
            chain: 0,
            sequence: 0,
            pending: Pending::None,
            case_colons: 0,
        }
    }

    const fn weight(&self) -> usize {
        match self.opener {
            Opener::Root => 0,
            _ => NESTING_BRACKET_WEIGHT,
        }
    }

    const fn cost(&self) -> usize {
        self.weight()
            .saturating_add(self.stmt)
            .saturating_add(self.expr)
            .saturating_add(
                self.chain
                    .saturating_add(self.sequence)
                    .div_ceil(CHAIN_ELEMENTS_PER_UNIT),
            )
    }

    /// Ends the current expression (its nesting and chain).
    const fn end_expression(&mut self) {
        self.expr = 0;
        self.chain = 0;
    }

    /// Ends the current statement.
    const fn end_statement(&mut self) {
        self.stmt = 0;
        self.sequence = 0;
        self.end_expression();
    }

    /// Whether a `,` directly in this frame certainly separates list
    /// elements rather than building a (left-nested) sequence expression.
    const fn comma_is_list(&self) -> bool {
        matches!(
            self.opener,
            Opener::Bracket { list: true }
                | Opener::Brace(BraceKind::Object)
                | Opener::Paren { list: true, .. }
        )
    }
}

/// The last word token, for `(` heads, labels, and `In(` calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LastWord {
    None,
    /// `if`, `while`, `with`, `switch`, or `catch` as an unescaped keyword.
    Header,
    /// `await` directly after `for` (`for await (`).
    ForAwait,
    /// `for` itself: its `(` is a head, and a following `await` is too.
    For,
    /// A keyword or contextual word spelled with Unicode escapes.
    Escaped,
    /// Any other word; `label` is true when it may begin a statement.
    Other {
        label: bool,
    },
    /// `case` or `default` keyword.
    CaseHead,
    /// `return` or `throw` (a `{` on the same line is an object literal).
    Return,
}

/// Words the scanner gives a meaning of their own (keywords, reserved
/// words, and contextual words); spelled with escapes, they are ambiguous.
const SPECIAL_WORDS: &[&str] = &[
    "if",
    "while",
    "with",
    "switch",
    "for",
    "catch",
    "else",
    "do",
    "try",
    "finally",
    "case",
    "default",
    "return",
    "throw",
    "var",
    "const",
    "let",
    "function",
    "class",
    "extends",
    "new",
    "typeof",
    "void",
    "delete",
    "await",
    "yield",
    "async",
    "in",
    "instanceof",
    "enum",
    "break",
    "continue",
    "debugger",
    "import",
    "export",
    "this",
    "super",
    "null",
    "true",
    "false",
    "of",
    "get",
    "set",
    "static",
    "as",
    "from",
    "target",
    "meta",
    "accessor",
    "using",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
];

#[derive(Clone, Debug)]
struct Scanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    /// Enclosing frames, outermost first; the root frame is at the bottom.
    frames: Vec<Frame>,
    /// The innermost open frame.
    current: Frame,
    /// Sum of the costs of every frame in `frames`.
    outer: usize,
    prev: Prev,
    /// What the previous significant token, when a punctuator, settles.
    after: After,
    last_word: LastWord,
    /// A line terminator separated the previous token from the next one.
    newline: bool,
}

type Scan = Result<(), DataModelError>;

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            frames: Vec::new(),
            current: Frame::new(Opener::Root),
            outer: 0,
            prev: Prev::Start,
            after: After::Other,
            last_word: LastWord::None,
            newline: false,
        }
    }

    fn byte(&self, offset: usize) -> Option<u8> {
        self.pos
            .checked_add(offset)
            .and_then(|index| self.bytes.get(index).copied())
    }

    fn char_at(&self, index: usize) -> Option<char> {
        self.text.get(index..).and_then(|rest| rest.chars().next())
    }

    fn top(&mut self) -> &mut Frame {
        &mut self.current
    }

    const fn total(&self) -> usize {
        self.outer.saturating_add(self.current.cost())
    }

    fn check(&mut self) -> Scan {
        if self.total() > MAX_PARSE_NESTING {
            Err(DataModelError::InvalidInput)
        } else {
            Ok(())
        }
    }

    fn add_expr(&mut self, units: usize) -> Scan {
        let top = self.top();
        top.expr = top.expr.saturating_add(units);
        self.check()
    }

    fn add_chain(&mut self) -> Scan {
        let top = self.top();
        top.chain = top.chain.saturating_add(1);
        self.check()
    }

    fn add_stmt(&mut self) -> Scan {
        let top = self.top();
        top.stmt = top.stmt.saturating_add(1);
        self.check()
    }

    /// Whether the previous token may have ended an operand, so a following
    /// `(`, `[`, template, or binary operator may apply to it.
    const fn operand_may_end(&self) -> bool {
        matches!(self.prev, Prev::ExprEnd | Prev::Ambiguous)
    }

    fn push(&mut self, opener: Opener) -> Scan {
        self.outer = self.outer.saturating_add(self.current.cost());
        let parent = std::mem::replace(&mut self.current, Frame::new(opener));
        self.frames.push(parent);
        self.after = After::Other;
        self.last_word = LastWord::None;
        self.check()
    }

    /// Pops the top frame (never the root) and returns its opener.
    fn pop(&mut self) -> Option<Opener> {
        let parent = self.frames.pop()?;
        let closed = std::mem::replace(&mut self.current, parent);
        self.outer = self.outer.saturating_sub(self.current.cost());
        Some(closed.opener)
    }

    fn run(&mut self, forks: &mut Forks<'a>) -> Scan {
        if self.pos == 0 && self.bytes.starts_with(b"#!") {
            self.skip_line();
        }
        while let Some(byte) = self.byte(0) {
            match byte {
                b' ' | b'\t' | 0x0B | 0x0C => self.pos += 1,
                b'\n' | b'\r' => {
                    self.pos += 1;
                    self.newline = true;
                }
                b'/' if self.byte(1) == Some(b'/') => self.skip_line(),
                b'/' if self.byte(1) == Some(b'*') => self.skip_block_comment(),
                b'/' => self.slash(forks)?,
                b'\'' | b'"' => {
                    self.token(None);
                    self.skip_string(byte);
                    self.operand();
                }
                b'`' => {
                    self.token(None);
                    if self.operand_may_end() {
                        // A tagged template applies the tag like a call.
                        self.add_chain()?;
                    }
                    self.pos += 1;
                    self.template()?;
                }
                b'0'..=b'9' => {
                    self.token(None);
                    self.skip_number();
                    self.operand();
                }
                b'.' if self.byte(1).is_some_and(|next| next.is_ascii_digit()) => {
                    self.token(None);
                    self.skip_number();
                    self.operand();
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'$' | b'_' | b'\\' | b'#' => self.word()?,
                0x80..=0xFF => {
                    let character = self.char_at(self.pos).unwrap_or('\u{FFFD}');
                    if is_line_terminator(character) {
                        self.pos += character.len_utf8();
                        self.newline = true;
                    } else if is_whitespace(character) {
                        self.pos += character.len_utf8();
                    } else {
                        self.word()?;
                    }
                }
                _ => self.punctuator()?,
            }
        }
        Ok(())
    }

    /// Settles the frame's pending reset before a new significant token;
    /// `word` is the decoded word when the token is one.
    fn token(&mut self, word: Option<&str>) {
        let top = self.top();
        let continues = word.is_some_and(|word| {
            matches!(
                word,
                "else" | "while" | "catch" | "finally" | "in" | "instanceof" | "of"
            )
        });
        match top.pending {
            Pending::None => {}
            Pending::Semicolon => {
                if word.is_some_and(|word| matches!(word, "else" | "while")) {
                    top.sequence = 0;
                    top.end_expression();
                } else {
                    top.end_statement();
                }
            }
            Pending::BraceClose => {
                if word.is_some() && !continues {
                    top.end_statement();
                }
            }
        }
        top.pending = Pending::None;
    }

    fn operand(&mut self) {
        self.prev = Prev::ExprEnd;
        self.after = After::Other;
        self.last_word = LastWord::None;
        self.newline = false;
    }

    fn operator(&mut self) {
        self.prev = Prev::Operator;
        self.after = After::Other;
        self.last_word = LastWord::None;
        self.newline = false;
    }

    fn skip_line(&mut self) {
        while let Some(byte) = self.byte(0) {
            if byte == b'\n' || byte == b'\r' {
                return;
            }
            if byte >= 0x80 {
                let character = self.char_at(self.pos).unwrap_or('\u{FFFD}');
                if is_line_terminator(character) {
                    return;
                }
                self.pos += character.len_utf8();
            } else {
                self.pos += 1;
            }
        }
    }

    fn skip_block_comment(&mut self) {
        self.pos += 2;
        while let Some(byte) = self.byte(0) {
            if byte == b'*' && self.byte(1) == Some(b'/') {
                self.pos += 2;
                return;
            }
            if byte == b'\n' || byte == b'\r' {
                self.newline = true;
                self.pos += 1;
            } else if byte >= 0x80 {
                let character = self.char_at(self.pos).unwrap_or('\u{FFFD}');
                if is_line_terminator(character) {
                    self.newline = true;
                }
                self.pos += character.len_utf8();
            } else {
                self.pos += 1;
            }
        }
    }

    /// Skips a string literal; an unescaped CR or LF ends it (Boa reports the
    /// unterminated literal there, so nothing after it is parsed as part of
    /// the literal).
    fn skip_string(&mut self, quote: u8) {
        self.pos += 1;
        while let Some(byte) = self.byte(0) {
            match byte {
                b'\\' => {
                    self.pos += 1;
                    self.skip_escaped_char();
                }
                b'\n' | b'\r' => return,
                _ if byte == quote => {
                    self.pos += 1;
                    return;
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Skips the character after a backslash; CR LF counts as one.
    fn skip_escaped_char(&mut self) {
        match self.byte(0) {
            Some(b'\r') if self.byte(1) == Some(b'\n') => self.pos += 2,
            Some(byte) if byte >= 0x80 => {
                let character = self.char_at(self.pos).unwrap_or('\u{FFFD}');
                self.pos += character.len_utf8();
            }
            Some(_) => self.pos += 1,
            None => {}
        }
    }

    fn skip_number(&mut self) {
        while let Some(byte) = self.byte(0) {
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
                self.pos += 1;
            } else {
                return;
            }
        }
    }

    /// Scans template characters after a backtick or a closing `}` of a
    /// substitution, until the closing backtick or the next `${`.
    fn template(&mut self) -> Scan {
        while let Some(byte) = self.byte(0) {
            match byte {
                b'\\' => {
                    self.pos += 1;
                    self.skip_escaped_char();
                }
                b'`' => {
                    self.pos += 1;
                    self.operand();
                    return Ok(());
                }
                b'$' if self.byte(1) == Some(b'{') => {
                    self.pos += 2;
                    self.push(Opener::Template)?;
                    self.prev = Prev::Operator;
                    self.newline = false;
                    return Ok(());
                }
                _ => self.pos += 1,
            }
        }
        // Unterminated template: Boa reports it and parses nothing after it.
        self.operand();
        Ok(())
    }

    /// The end of a regular-expression body starting at `start` (just after
    /// the opening `/`) on the same line, if any.
    fn regex_end(&self, start: usize) -> Option<usize> {
        let mut index = start;
        let mut class = false;
        while let Some(&byte) = self.bytes.get(index) {
            match byte {
                b'\\' => {
                    let next = index.checked_add(1)?;
                    match self.bytes.get(next) {
                        None | Some(b'\n' | b'\r') => return None,
                        Some(&escaped) if escaped >= 0x80 => {
                            let character = self.char_at(next)?;
                            if is_line_terminator(character) {
                                return None;
                            }
                            index = next.checked_add(character.len_utf8())?;
                        }
                        Some(_) => index = next.checked_add(1)?,
                    }
                }
                b'\n' | b'\r' => return None,
                b'[' => {
                    class = true;
                    index += 1;
                }
                b']' => {
                    class = false;
                    index += 1;
                }
                b'/' if !class => return Some(index),
                _ if byte >= 0x80 => {
                    let character = self.char_at(index)?;
                    if is_line_terminator(character) {
                        return None;
                    }
                    index = index.checked_add(character.len_utf8())?;
                }
                _ => index += 1,
            }
        }
        None
    }

    /// Consumes a regular-expression literal whose closing `/` is at `end`
    /// (or, unterminated, the rest of the source: Boa reports it and parses
    /// nothing after it), with its flags.
    fn regex_literal(&mut self, end: Option<usize>) {
        self.pos = end.map_or(self.bytes.len(), |end| end.saturating_add(1));
        while let Some(byte) = self.byte(0) {
            if byte.is_ascii_alphanumeric() || byte == b'$' || byte == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.operand();
    }

    fn slash(&mut self, forks: &mut Forks<'a>) -> Scan {
        let newline = self.newline;
        self.token(None);
        match self.prev {
            Prev::Start | Prev::Operator => {
                let end = self.regex_end(self.pos + 1);
                self.regex_literal(end);
                return Ok(());
            }
            Prev::ExprEnd if !newline => {}
            Prev::ExprEnd | Prev::Ambiguous => {
                if let Some(end) = self.regex_end(self.pos + 1) {
                    forks.remaining = forks
                        .remaining
                        .checked_sub(1)
                        .ok_or(DataModelError::InvalidInput)?;
                    let mut regex = self.clone();
                    regex.regex_literal(Some(end));
                    forks.pending.push(regex);
                }
            }
        }
        let assign = self.byte(1) == Some(b'=');
        self.pos += if assign { 2 } else { 1 };
        if assign {
            self.add_expr(1)?;
        } else {
            self.add_chain()?;
        }
        self.operator();
        Ok(())
    }

    /// Reads one identifier, keyword, or private name, decoding Unicode
    /// escapes as Boa does.
    fn word(&mut self) -> Scan {
        let start = self.pos;
        let mut escaped = false;
        if self.byte(0) == Some(b'#') {
            self.pos += 1;
        }
        while let Some(byte) = self.byte(0) {
            match byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'$' | b'_' => self.pos += 1,
                b'\\' => {
                    escaped = true;
                    self.pos += 1;
                    self.skip_unicode_escape();
                }
                0x80..=0xFF => {
                    let character = self.char_at(self.pos).unwrap_or('\u{FFFD}');
                    if is_whitespace(character) || is_line_terminator(character) {
                        break;
                    }
                    self.pos += character.len_utf8();
                }
                _ => break,
            }
        }
        if self.pos == start {
            // A lone `#` or `\` that starts nothing: step over it (Boa errors).
            self.pos += 1;
            self.operator();
            return Ok(());
        }
        let raw = self.text.get(start..self.pos).unwrap_or("");
        if escaped {
            let decoded = decode_escapes(raw);
            if self.after != After::Dot && SPECIAL_WORDS.contains(&decoded.as_str()) {
                return self.escaped_keyword(&decoded);
            }
            self.keyword(&decoded)
        } else {
            self.keyword(raw)
        }
    }

    fn skip_unicode_escape(&mut self) {
        if self.byte(0) != Some(b'u') {
            return;
        }
        self.pos += 1;
        if self.byte(0) == Some(b'{') {
            while let Some(byte) = self.byte(0) {
                self.pos += 1;
                if byte == b'}' || !(byte == b'{' || byte.is_ascii_hexdigit()) {
                    return;
                }
            }
        } else {
            for _ in 0..4 {
                if self.byte(0).is_some_and(|byte| byte.is_ascii_hexdigit()) {
                    self.pos += 1;
                }
            }
        }
    }

    /// A keyword spelled with escapes. Boa rejects these, but the scanner
    /// does not rely on it: the word charges one statement level and leaves
    /// every following decision ambiguous.
    fn escaped_keyword(&mut self, word: &str) -> Scan {
        self.token(Some(word));
        self.add_stmt()?;
        self.operator();
        self.prev = Prev::Ambiguous;
        self.last_word = LastWord::Escaped;
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // one flat keyword table
    fn keyword(&mut self, word: &str) -> Scan {
        let after_dot = self.after == After::Dot;
        let before = self.prev;
        let newline = self.newline;
        let last = self.last_word;
        if after_dot {
            // A property name, even when spelled like a keyword.
            self.token(None);
            self.operand();
            self.last_word = LastWord::Other { label: false };
            return Ok(());
        }
        self.token(Some(word));
        let label = matches!(before, Prev::Start | Prev::Ambiguous) || newline;
        match word {
            "if" | "while" | "with" | "switch" => {
                self.add_stmt()?;
                self.operator();
                self.last_word = LastWord::Header;
            }
            "for" => {
                self.add_stmt()?;
                self.operator();
                self.last_word = LastWord::For;
            }
            "catch" => {
                // `catch {` (no binding) opens a block.
                self.operator();
                self.prev = Prev::Start;
                self.last_word = LastWord::Header;
            }
            "else" | "do" | "try" | "finally" => {
                if word != "finally" {
                    self.add_stmt()?;
                }
                self.operator();
                self.prev = Prev::Start;
            }
            "case" | "default" => {
                let top = self.top();
                top.end_statement();
                top.case_colons = 1;
                self.operator();
                self.last_word = LastWord::CaseHead;
            }
            "return" | "throw" => {
                self.add_stmt()?;
                self.operator();
                self.last_word = LastWord::Return;
            }
            "var" | "const" => {
                self.add_stmt()?;
                self.operator();
            }
            "let" => {
                self.add_stmt()?;
                self.operator();
                self.prev = Prev::Ambiguous;
                self.last_word = LastWord::Other { label };
            }
            "function" | "class" | "extends" | "new" | "typeof" | "void" | "delete" => {
                self.add_expr(1)?;
                self.operator();
            }
            "await" => {
                self.add_expr(1)?;
                self.operator();
                self.prev = Prev::Ambiguous;
                self.last_word = if last == LastWord::For {
                    LastWord::ForAwait
                } else {
                    LastWord::Other { label }
                };
            }
            "yield" | "async" => {
                self.add_expr(1)?;
                self.operator();
                self.prev = Prev::Ambiguous;
                self.last_word = LastWord::Other { label };
            }
            "in" | "instanceof" => {
                self.add_chain()?;
                self.operator();
            }
            "break" | "continue" | "debugger" => {
                // The statement may end here (a line break inserts `;`), so
                // a `{` opens a block and a `/` starts a regular expression;
                // any other reading of either is a syntax error.
                self.operator();
                self.prev = Prev::Start;
            }
            "enum" | "import" | "export" => {
                self.operator();
            }
            "this" | "super" | "null" | "true" | "false" => self.operand(),
            "of" | "get" | "set" | "static" | "as" | "from" | "target" | "meta" | "accessor"
            | "using" | "implements" | "interface" | "package" | "private" | "protected"
            | "public" => {
                self.operand();
                self.prev = Prev::Ambiguous;
                self.last_word = LastWord::Other { label };
            }
            _ => {
                self.operand();
                self.last_word = LastWord::Other { label };
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // one flat punctuator table
    fn punctuator(&mut self) -> Scan {
        self.token(None);
        let rest = self.bytes.get(self.pos..).unwrap_or(&[]);
        let length = punctuator_length(rest);
        let text = rest.get(..length).unwrap_or(&[]);
        let prev = self.prev;
        let operand_may_end = self.operand_may_end();
        let last_word = self.last_word;
        let newline = self.newline;
        let after_arrow = self.after == After::Arrow;
        let after_dot = self.after == After::Dot;
        let member_start = self.after == After::MemberStart;
        self.pos += length.max(1);
        match text {
            b"(" => {
                let head = match last_word {
                    LastWord::Header | LastWord::For | LastWord::ForAwait => Head::Statement,
                    LastWord::Escaped => Head::Escaped,
                    _ => Head::No,
                };
                let list = prev == Prev::ExprEnd && !newline && head == Head::No;
                if operand_may_end && head == Head::No {
                    self.add_chain()?;
                }
                self.push(Opener::Paren { head, list })?;
                self.prev = Prev::Operator;
            }
            b"[" => {
                if operand_may_end {
                    self.add_chain()?;
                }
                let in_brace = matches!(self.current.opener, Opener::Brace(_));
                // `?.` leaves the scanner awaiting an operand, yet `?.[`
                // opens a computed member whose commas build a sequence.
                let list = matches!(prev, Prev::Start | Prev::Operator)
                    && !after_dot
                    && !(in_brace && (member_start || newline));
                self.push(Opener::Bracket { list })?;
                self.prev = Prev::Operator;
            }
            b"{" => {
                let kind = if after_arrow {
                    BraceKind::Unknown
                } else {
                    match prev {
                        Prev::Start => BraceKind::Block,
                        Prev::Operator
                            if !(last_word == LastWord::Return && newline)
                                && last_word != LastWord::CaseHead =>
                        {
                            BraceKind::Object
                        }
                        _ => BraceKind::Unknown,
                    }
                };
                self.push(Opener::Brace(kind))?;
                self.prev = if kind == BraceKind::Object {
                    Prev::Operator
                } else {
                    Prev::Start
                };
                self.after = After::MemberStart;
            }
            b")" | b"]" => {
                let opener = self.pop();
                self.operand();
                match opener {
                    Some(Opener::Paren {
                        head: Head::Statement,
                        ..
                    }) => self.prev = Prev::Start,
                    Some(Opener::Paren {
                        head: Head::Escaped,
                        ..
                    }) => self.prev = Prev::Ambiguous,
                    _ => {}
                }
            }
            b"}" => {
                if self.pop() == Some(Opener::Template) {
                    self.newline = false;
                    self.template()?;
                } else {
                    // A block, body, or object ended: what follows is
                    // settled by nothing the scanner can see here.
                    self.top().pending = Pending::BraceClose;
                    self.operand();
                    self.prev = Prev::Ambiguous;
                }
            }
            b";" => {
                self.top().pending = Pending::Semicolon;
                self.operator();
                self.prev = Prev::Start;
                self.after = After::MemberStart;
            }
            b"," => {
                let top = self.top();
                top.end_expression();
                if !top.comma_is_list() {
                    top.sequence = top.sequence.saturating_add(1);
                    self.check()?;
                }
                self.operator();
                self.after = After::MemberStart;
            }
            b"." | b"?." => {
                self.add_chain()?;
                self.operator();
                self.after = After::Dot;
            }
            b":" => {
                let top = self.top();
                if top.case_colons > 0 {
                    top.case_colons -= 1;
                    if top.case_colons == 0 {
                        self.operator();
                        self.prev = Prev::Start;
                        return Ok(());
                    }
                }
                if matches!(last_word, LastWord::Other { label: true }) {
                    self.add_stmt()?;
                    self.operator();
                    self.prev = Prev::Start;
                } else {
                    self.add_expr(1)?;
                    self.operator();
                }
            }
            b"?" => {
                let top = self.top();
                if top.case_colons > 0 {
                    top.case_colons = top.case_colons.saturating_add(1);
                }
                self.add_expr(1)?;
                self.operator();
            }
            b"++" | b"--" => {
                self.add_expr(NESTING_INCREMENT_WEIGHT)?;
                self.operator();
                self.prev = Prev::Ambiguous;
            }
            b"+" | b"-" => {
                if prev == Prev::ExprEnd {
                    self.add_chain()?;
                } else {
                    self.add_expr(1)?;
                }
                self.operator();
            }
            b"=>" => {
                self.add_expr(1)?;
                self.operator();
                self.after = After::Arrow;
            }
            b"=" | b"+=" | b"-=" | b"*=" | b"%=" | b"**=" | b"<<=" | b">>=" | b">>>=" | b"&="
            | b"|=" | b"^=" | b"&&=" | b"||=" | b"??=" | b"**" | b"!" | b"~" | b"..." | b"||"
            | b"&&" | b"??" => {
                self.add_expr(1)?;
                self.operator();
            }
            // Every remaining punctuator after an operand is a binary
            // operator (`*`, `%`, comparisons, equality, bitwise, shifts).
            _ => {
                if operand_may_end && is_binary_operator(text) {
                    self.add_chain()?;
                }
                self.operator();
                // `*` may start a generator member (`*[key]() {}`).
                if text == b"*" {
                    self.after = After::MemberStart;
                }
            }
        }
        Ok(())
    }
}

/// Left-associative binary punctuators not handled individually.
fn is_binary_operator(text: &[u8]) -> bool {
    matches!(
        text,
        b"*" | b"%"
            | b"<"
            | b">"
            | b"<="
            | b">="
            | b"=="
            | b"!="
            | b"==="
            | b"!=="
            | b"&"
            | b"|"
            | b"^"
            | b"<<"
            | b">>"
            | b">>>"
    )
}

/// Length of the longest punctuator at the start of `rest` (at least 1).
fn punctuator_length(rest: &[u8]) -> usize {
    const FOUR: [&[u8]; 1] = [b">>>="];
    const THREE: [&[u8]; 8] = [
        b"...", b"===", b"!==", b"**=", b"<<=", b">>=", b">>>", b"&&=",
    ];
    const THREE_MORE: [&[u8]; 2] = [b"||=", b"??="];
    const TWO: [&[u8]; 19] = [
        b"=>", b"==", b"!=", b"<=", b">=", b"&&", b"||", b"??", b"++", b"--", b"+=", b"-=", b"*=",
        b"%=", b"&=", b"|=", b"^=", b"**", b"<<",
    ];
    const TWO_MORE: [&[u8]; 1] = [b">>"];
    if FOUR.iter().any(|token| rest.starts_with(token)) {
        return 4;
    }
    if THREE
        .iter()
        .chain(THREE_MORE.iter())
        .any(|token| rest.starts_with(token))
    {
        return 3;
    }
    if rest.starts_with(b"?.") && !rest.get(2).is_some_and(u8::is_ascii_digit) {
        return 2;
    }
    if TWO
        .iter()
        .chain(TWO_MORE.iter())
        .any(|token| rest.starts_with(token))
    {
        return 2;
    }
    usize::from(!rest.is_empty())
}

/// Decodes `\uXXXX` and `\u{X..}` escapes in an identifier; malformed
/// escapes decode to U+FFFD (Boa rejects them).
fn decode_escapes(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('\\') {
        output.push_str(rest.get(..index).unwrap_or(""));
        let tail = rest.get(index + 1..).unwrap_or("");
        let Some(tail) = tail.strip_prefix('u') else {
            output.push('\u{FFFD}');
            rest = tail;
            continue;
        };
        let (digits, consumed) = if let Some(braced) = tail.strip_prefix('{') {
            let end = braced.find('}').unwrap_or(braced.len());
            (braced.get(..end).unwrap_or(""), end.saturating_add(2))
        } else {
            let end = tail
                .char_indices()
                .take(4)
                .take_while(|(_, character)| character.is_ascii_hexdigit())
                .count();
            (tail.get(..end).unwrap_or(""), end)
        };
        let decoded = u32::from_str_radix(digits, 16)
            .ok()
            .and_then(char::from_u32)
            .unwrap_or('\u{FFFD}');
        output.push(decoded);
        rest = tail.get(consumed.min(tail.len())..).unwrap_or("");
    }
    output.push_str(rest);
    output
}

/// Whether `source` is ECMAScript white space and line terminators only
/// (ECMA-262 §12.2, §12.3), the text a parser reads as no program.
pub(crate) fn is_blank(source: &str) -> bool {
    source.chars().all(is_white_space_or_line_terminator)
}

/// `source` without leading and trailing ECMAScript white space and line
/// terminators.
pub(crate) fn trim(source: &str) -> &str {
    source.trim_matches(is_white_space_or_line_terminator)
}

/// `source` without leading ECMAScript white space and line terminators.
pub(crate) fn trim_start(source: &str) -> &str {
    source.trim_start_matches(is_white_space_or_line_terminator)
}

const fn is_white_space_or_line_terminator(character: char) -> bool {
    is_whitespace(character) || is_line_terminator(character)
}

/// ECMAScript white space exactly as Boa's lexer defines it.
const fn is_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{FEFF}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

const fn is_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(source: &str) -> bool {
        check_nesting(source).is_ok()
    }

    #[test]
    fn ordinary_programs_are_accepted() {
        for source in [
            "x > 1 && In('s')",
            "function f(a) { if (a) { return g(a[0]) ? 1 : 2; } else { return 0; } }",
            "var a = {b: {c: [1, 2, {d: 3}]}}; a.b.c[2].d += 1;",
            "for (var i = 0; i < 10; i++) { if (i % 2) continue; x += i; }",
            "switch (x) { case 1: y = 2; break; case 2: case 3: y = 3; break; default: y = 0; }",
            "s = `a${b + `c${d}`}e`; r = /[/(]+/g.test(s) ? 1 : 0;",
            "if (x) /re/.test(y); while (z) /q/.exec(w);",
            "var o = {}; o.if = 1; o.return = 2; x = a / b / c;",
            "try { f(); } catch (e) { g(e); } finally { h(); }",
            "label: for (;;) { break label; }",
            "x = a ? b : c ? d : e;",
            "'a' + b + 'c' + d + 'e' + f + 'g' + h + 'i' + j + 'k' + l + 'm'",
        ] {
            assert!(accepted(source), "{source}");
        }
    }

    #[test]
    fn long_flat_sequences_do_not_accumulate() {
        let statements = "if (a) { b(); }\n".repeat(500);
        assert!(accepted(&statements));
        let declarations = "function f(x) { return x; }\n".repeat(500);
        assert!(accepted(&declarations));
        let semicolons = "a = b ? c : d;".repeat(500);
        assert!(accepted(&semicolons));
        let array = format!("[{}1]", "1, ".repeat(2000));
        assert!(accepted(&array));
        let object = format!("({{{}z: 1}})", "a: 1, ".repeat(2000));
        assert!(accepted(&object));
        let cases = format!(
            "switch (x) {{ {} }}",
            (0..300)
                .map(|i| ["case ", &i.to_string(), ": y = 1; break; "].concat())
                .collect::<String>()
        );
        assert!(accepted(&cases));
    }

    #[test]
    fn every_recursive_construct_is_bounded() {
        let over = MAX_PARSE_NESTING + 1;
        for source in [
            format!("{}x;", "if(1)".repeat(over)),
            format!("if(1)x;{}", "else if(1)x;".repeat(over)),
            format!("if(1){{}}{}", "else if(1){}".repeat(over)),
            format!("{}x;", "while(0)".repeat(over)),
            format!("{}x;", "for(;0;)".repeat(over)),
            format!("{}x;", "with(a)".repeat(over)),
            (0..over)
                .map(|i| ["l", &i.to_string(), ":"].concat())
                .collect::<String>()
                + "x;",
            format!("{}a", "a?a:".repeat(over)),
            format!("{}a{}", "a?".repeat(over), ":a".repeat(over)),
            format!("{}x", "!".repeat(over)),
            format!("{}x", "- ".repeat(over)),
            format!("{}x", "typeof ".repeat(over)),
            format!("{}x", "void ".repeat(over)),
            format!("{}x", "new ".repeat(over)),
            format!("{}1", "a=>".repeat(over)),
            format!("{}1", "a=".repeat(over)),
            format!("{}1", "2**".repeat(over)),
            format!("{}1", "1||".repeat(over)),
            format!("{}a", "++".repeat(over)),
            format!("{}x;{}", "do ".repeat(over), "while(0);".repeat(over)),
            format!("function*g(){{{}1}}", "yield ".repeat(over)),
            format!("async function f(){{{}1}}", "await ".repeat(over)),
            format!("{}a{}", "[...".repeat(over), "]".repeat(over)),
            "(".repeat(over),
            "[".repeat(over),
            "{".repeat(over),
            "`${".repeat(over),
            format!("({})", "{a:".repeat(over)),
            format!("{}1", "a[".repeat(over)),
            "function f(){".repeat(over),
            "try{".repeat(over),
            "switch(1){case 1:".repeat(over),
        ] {
            assert_eq!(
                check_nesting(&source),
                Err(DataModelError::InvalidInput),
                "{}",
                source.get(..40).unwrap_or(&source)
            );
        }
    }

    #[test]
    fn nothing_can_hide_brackets_from_the_count() {
        let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        for prefix in [
            "/*'*/ ",
            "/*\"*/ ",
            "/*`*/ ",
            "// '\n",
            "'\\'' + ",
            "\"\\\"\" + ",
            "`${'`'}` + ",
            "/'/.source + ",
            "/[/']/.source + ",
            "a = /`/; ",
            // `/` after an operand divides, so the quote opens a string.
            "x = 1 /'/ 1;' + ",
            "\\u0069f (1) /'/; ",
            "'\\\r\n' + ",
        ] {
            let source = format!("{prefix}{deep}");
            assert_eq!(
                check_nesting(&source),
                Err(DataModelError::InvalidInput),
                "{prefix:?}"
            );
        }
    }

    #[test]
    fn ambiguous_slashes_fail_closed() {
        // The regular-expression reading exposes the deep tail.
        for head in [
            "function f(){} /'/; ",
            "a++ /'/; ",
            "of /'/; ",
            "x = () => {} /'/; ",
        ] {
            declined(&format!("{head}{}", deep()));
        }
        // Without a possible regular-expression end on the line, `/` divides.
        assert!(accepted("a++ / 2"));
        assert!(accepted("x = {a: 1}.a / 2"));
    }

    /// Deep code a hidden string would swallow if the scanner misread the
    /// `/` before it as division.
    fn deep() -> String {
        format!("{}1{}", "(".repeat(40), ")".repeat(40))
    }

    fn declined(source: &str) {
        assert_eq!(
            check_nesting(source),
            Err(DataModelError::InvalidInput),
            "{}",
            source.get(..60).unwrap_or(source)
        );
    }

    #[test]
    fn for_heads_are_statement_heads() {
        // Regression: `for(...)` heads were not marked as statement heads, so the
        // `/` after `)` read as division and `'` hid the tail from the count.
        for head in [
            "for(;0;) /'/; ",
            "for (x of y) /'/; ",
            "for (x in y) /'/; ",
            "for (;;)\n/'/; ",
            "async function f() { for await (x of y) /'/; ",
        ] {
            declined(&format!("{head}{}", deep()));
        }
        declined(&format!(
            "for(;0;) /'/; {}1{}",
            "(".repeat(6000),
            ")".repeat(6000)
        ));
    }

    #[test]
    fn binding_names_before_a_line_break_do_not_settle_a_slash() {
        // Automatic semicolon insertion ends these statements, so Boa reads
        // the next line's `/` as a regular expression.
        for head in [
            "var x\n/'/; ",
            "let x\n/'/; ",
            "var a = 1, b\n/'/; ",
            "l: { break l\n/'/; ",
            "l: for (;;) { continue l\n/'/; ",
        ] {
            declined(&format!("{head}{}", deep()));
        }
    }

    #[test]
    fn blocks_after_catch_and_jump_statements_are_blocks() {
        for head in [
            "try {} catch {} /'/; ",
            "do { break\n{} /'/; ",
            "for (;;) { continue\n{} /'/; ",
            "debugger\n{} /'/; ",
        ] {
            declined(&format!("{head}{}", deep()));
        }
    }

    #[test]
    fn escaped_keywords_do_not_settle_a_slash() {
        for head in [
            "\\u0066or(;0;) /'/; ",
            "try {} c\\u0061tch {} /'/; ",
            "v\\u0061r x\n/'/; ",
        ] {
            declined(&format!("{head}{}", deep()));
        }
    }

    #[test]
    fn sequence_commas_are_charged_wherever_brackets_hold_expressions() {
        let commas = "1,".repeat(MAX_PARSE_NESTING * CHAIN_ELEMENTS_PER_UNIT + 1);
        for source in [
            format!("a[{commas}1]"),
            format!("({{[{commas}1]: 1}})"),
            format!("class A {{ x = 1; [{commas}1]() {{}} }}"),
            format!("try {{}} catch {{ {commas}1 }}"),
            format!("do {{ break\n{{ {commas}1 }} }} while (0)"),
            format!("var x\n({commas}1)"),
            // An optional-chain index is a computed member, not an array
            // literal, although `?.` leaves the scanner awaiting an operand.
            format!("var a=[];a?.[{commas}1]"),
            format!("a?.b?.[{commas}1]"),
            format!("a?.\n[{commas}1]"),
            format!("x = a?.[{commas}1]"),
            format!("f(a?.[{commas}1])"),
            format!("[a?.[{commas}1]]"),
            format!("({{[a?.[{commas}1]]: 1}})"),
            format!("({{k: a?.[{commas}1]}})"),
            format!("class A {{ [a?.[{commas}1]]() {{}} }}"),
            format!("class A {{ x = a?.[{commas}1] }}"),
            format!("a?.({commas}1)"),
        ] {
            declined(&source);
        }
        // Array literals and argument lists stay element lists.
        let elements = "1,".repeat(4000);
        assert!(accepted(&format!("x = [{elements}1]")));
        assert!(accepted(&format!("f({elements}1)")));
        assert!(accepted(&format!("({{a: [{elements}1]}})")));
    }

    #[test]
    fn ambiguous_slashes_explore_both_readings() {
        // Benign in both readings: accepted.
        for source in [
            "a++ / 2 / 3",
            "if (x) {} /re/.test(y)",
            "x = y\n/re/g.exec(z)",
            "function f() {} /q/",
        ] {
            assert!(accepted(source), "{source}");
        }
        // Deep code hidden under either reading is found.
        for head in [
            // regex reading: `/'/` then code; division reading hides it.
            "if (x) {} /'/; ",
            "a++ /'/; ",
        ] {
            declined(&format!("{head}{}", deep()));
        }
        // The division reading parses what the regex reading hides.
        declined(&format!("x = y\n/ {} /g", deep()));
        declined(&format!("if (x) {{}} / {} /g", deep()));
    }

    #[test]
    fn exhausting_the_fork_budget_is_declined() {
        let within = "x\n/ 1 /g\n".repeat(4);
        assert!(accepted(&within));
        let beyond = "x\n/ 1 /g\n".repeat(MAX_LEXICAL_FORKS + 1);
        declined(&beyond);
    }

    #[test]
    fn punctuators_use_longest_match() {
        assert_eq!(punctuator_length(b">>>=1"), 4);
        assert_eq!(punctuator_length(b"...a"), 3);
        assert_eq!(punctuator_length(b"?.5"), 1);
        assert_eq!(punctuator_length(b"?.b"), 2);
        assert_eq!(punctuator_length(b"=>"), 2);
        assert_eq!(punctuator_length(b""), 0);
        assert_eq!(decode_escapes("\\u0069f"), "if");
        assert_eq!(decode_escapes("\\u{69}\\u{66}"), "if");
        assert_eq!(decode_escapes("a\\x"), "a\u{FFFD}x");
    }
}
