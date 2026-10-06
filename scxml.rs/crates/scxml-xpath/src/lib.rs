//! Bounded, deterministic XPath 1.0 evaluator over a mutable XML data tree.
//!
//! This crate is the expression engine behind the SCXML XPath data model
//! (`components/scxml-data-model-xpath`). It provides:
//!
//! * [`Store`]: a flat arena of XML trees (document, element, attribute,
//!   text, comment, and processing-instruction nodes, plus synthesized
//!   namespace nodes): fixed-size index-linked node records, names interned
//!   once per store, and leaf values that keep their XPath `number()` next to
//!   their text. It has a bounded fragment parser, a canonical serializer,
//!   deep copy, and the structural mutations SCXML `<assign type="...">`
//!   needs ([`Store::assign`]).
//! * [`Expression`]: the complete XPath 1.0 language (all 13 axes, node
//!   tests, predicates, the 27 core functions, operators, and the §3.4
//!   comparison rules) plus caller-declared extension functions such as
//!   SCXML `In()` and `$name` variables supplied through [`Environment`].
//! * [`number_to_string`] and [`string_to_number`]: the XPath 1.0 number
//!   conversions.
//!
//! # Example
//!
//! Parse an XML fragment into a [`Store`], then evaluate an expression that
//! uses a `$name` variable and the SCXML `In()` extension supplied by an
//! [`Environment`]:
//!
//! ```
//! use scxml_xpath::{Environment, Error, Expression, NodeRef, ParseOptions, Store, Value, limits};
//!
//! /// `$threshold` plus `In()` over a fixed active configuration.
//! struct Session {
//!     active: Vec<&'static str>,
//! }
//!
//! impl Environment for Session {
//!     fn variable(&self, name: &str) -> Option<Value> {
//!         (name == "threshold").then_some(Value::Number(2.0))
//!     }
//!
//!     fn call_extension(
//!         &mut self,
//!         name: &str,
//!         arguments: &[Value],
//!         store: &Store,
//!     ) -> Result<Value, Error> {
//!         let ("In", [state]) = (name, arguments) else {
//!             return Err(Error::Extension);
//!         };
//!         let state = state.to_string(store)?;
//!         Ok(Value::Boolean(self.active.contains(&state.as_str())))
//!     }
//! }
//!
//! # fn main() -> Result<(), Error> {
//! let mut store = Store::new();
//! let root = store.create_root()?;
//! for node in store.parse_fragment("<cart><item price='3'/><item price='4'/></cart>")? {
//!     store.append_child(root, node)?;
//! }
//! let mut session = Session { active: vec!["checkout"] };
//! let options = ParseOptions::scxml();
//!
//! let guard = Expression::parse("count(//item) >= $threshold and In('checkout')", &options)?;
//! let value = guard.evaluate(&store, NodeRef::node(root), &mut session, limits::DEFAULT_FUEL)?;
//! assert_eq!(value, Value::Boolean(true));
//!
//! let total = Expression::parse("sum(//item/@price)", &options)?
//!     .evaluate(&store, NodeRef::node(root), &mut session, limits::DEFAULT_FUEL)?;
//! assert_eq!(total.to_string(&store)?, "7");
//! # Ok(())
//! # }
//! ```
//!
//! # Contract
//!
//! * Pure: no I/O, clocks, randomness, environment, or telemetry. Callers own
//!   the observability boundary; the evaluator runs on hot guard paths.
//! * Portable: `#![no_std]`, using only `core` and `alloc` (no math library:
//!   `floor()` and `ceiling()` are computed exactly in `core`), so it builds
//!   for targets without an operating system.
//! * Deterministic: identical inputs yield identical values and bytes.
//! * Bounded: every input and every intermediate result is bounded by the
//!   constants in [`limits`]; evaluation work is bounded by caller fuel,
//!   charged in proportion to string bytes (see [`limits::DEFAULT_FUEL`]).
//!   Parser recursion is bounded by [`limits::MAX_PARSE_NESTING`], evaluator
//!   recursion by [`limits::MAX_AST_DEPTH`], and XML parsing, tree walks,
//!   copying, and serialization are iterative.
//! * Fail-closed: every failure is a typed [`Error`] with static text; no
//!   input is reflected into messages. Reachable paths never panic.
//!
//! # Build versus buy
//!
//! Existing pure-Rust XPath 1.0 crates were evaluated and rejected:
//! `sxd-xpath` 0.4.2 (last release 2019) sits on `sxd-document`, whose DOM
//! uses arena raw pointers and exposes no depth, size, or work bounds;
//! `xrust`/`xee-xpath` implement XPath 2/3 with far larger dependency
//! surfaces and different (typed) semantics; younger XPath 1.0 crates carry
//! no bounded-evaluation or fuel contract. SCXML also needs in-place tree
//! mutation for `<assign>` which none of them provides. A purpose-built,
//! dependency-free evaluator keeps the Wasm component small and makes every
//! bound explicit and testable.

#![no_std]
#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    reason = "XPath, SCXML, and XML are specification names, not code identifiers"
)]

extern crate alloc;

mod dom;
mod eval;
mod lexer;
mod number;
mod parser;
mod xml;

use core::fmt;

pub use dom::{AssignMode, AssignValue, NodeId, NodeKind, NodeRef, QName, Siblings, Store};
pub use eval::{Environment, NodeSet, Value};
pub use number::{number_to_string, string_to_number};
pub use parser::{Expression, ParseOptions, check_syntax};

/// Every bound enforced by this crate.
///
/// Resource bounds apply to wasm builds only ([`scxml_types::wasm_bound`]);
/// the parse nesting and expression-tree depth are native stack guards (the
/// parser, evaluator, and drop recurse once per level) and apply to every
/// build.
pub mod limits {
    use scxml_types::wasm_bound;

    /// Maximum source bytes accepted by [`crate::Expression::parse`]
    /// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
    pub const MAX_EXPRESSION_BYTES: usize = wasm_bound(16_384);
    /// Maximum nesting depth of a parsed expression tree (binary operator
    /// chains count one level per operator), on every build.
    pub const MAX_AST_DEPTH: usize = 128;
    /// Maximum syntactic nesting (parentheses, predicates, function
    /// arguments) the recursive-descent parser enters, on every build.
    pub const MAX_PARSE_NESTING: usize = 32;
    /// Maximum expression-tree nodes in one parsed expression
    /// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
    pub const MAX_AST_NODES: usize = wasm_bound(4_096);
    /// Maximum node slots (live and garbage) in one [`crate::Store`]
    /// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
    pub const MAX_STORE_NODES: usize = wasm_bound(262_144);
    /// Maximum text, name, and value bytes retained by one [`crate::Store`]
    /// (live and garbage; reclaimed by [`crate::Store::compact`])
    /// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
    pub const MAX_STORE_BYTES: usize = wasm_bound(16 * 1_048_576);
    /// Maximum element depth of any tree.
    pub const MAX_TREE_DEPTH: usize = 256;
    /// Maximum bytes accepted by [`crate::Store::parse_fragment`].
    pub const MAX_XML_BYTES: usize = 1_048_576;
    /// Maximum attributes on one element.
    pub const MAX_ATTRIBUTES: usize = 256;
    /// Maximum bytes of one name (element, attribute, prefix, target).
    pub const MAX_NAME_BYTES: usize = 1_024;
    /// Maximum nodes in one node-set value (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
    pub const MAX_NODE_SET: usize = wasm_bound(65_536);
    /// Maximum bytes of one string produced by evaluation or serialization.
    pub const MAX_STRING_BYTES: usize = 1_048_576;
    /// Default evaluation fuel. The cost model:
    ///
    /// * one unit per expression-node evaluation (so per predicate
    ///   evaluation and function call), axis node visit, string-value node
    ///   visit, and document-order sort step;
    /// * one unit per started [`STRING_BYTES_PER_FUEL`] bytes of string
    ///   work: every string materialized (a node's string-value, a literal,
    ///   a variable, a number or boolean converted to a string), scanned
    ///   (`contains`, `starts-with`, `substring-before`, `substring-after`,
    ///   `substring`, `string-length`, `normalize-space`, `lang`, string to
    ///   number conversion, string comparison), or produced (`concat`,
    ///   `substring*`, `normalize-space`, `translate`);
    /// * one unit per byte of `translate`'s input and `from` strings, whose
    ///   per-character mapping costs far more than a copy;
    /// * one unit per node-set string compared in `=`/`!=` between two
    ///   node-sets (ordered-set insertion and lookup).
    ///
    /// Every such cost is charged before or as the work happens, so no
    /// single call does more than a constant multiple of the fuel it is
    /// charged, whatever its operands' length.
    pub const DEFAULT_FUEL: u64 = 1_000_000;
    /// String bytes processed per fuel unit; see [`DEFAULT_FUEL`].
    pub const STRING_BYTES_PER_FUEL: usize = 16;
}

/// A syntax failure with an exact UTF-8 byte span into the checked source.
///
/// `start <= end <= source.len()` and both lie on character boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyntaxError {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
    /// Static description of the problem (no input text, no positions).
    pub message: &'static str,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl core::error::Error for SyntaxError {}

/// The bound an operation exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    /// [`limits::MAX_EXPRESSION_BYTES`].
    ExpressionBytes,
    /// [`limits::MAX_AST_DEPTH`] or [`limits::MAX_PARSE_NESTING`].
    AstDepth,
    /// [`limits::MAX_AST_NODES`].
    AstNodes,
    /// [`limits::MAX_STORE_NODES`].
    StoreNodes,
    /// [`limits::MAX_STORE_BYTES`].
    StoreBytes,
    /// [`limits::MAX_TREE_DEPTH`].
    TreeDepth,
    /// [`limits::MAX_XML_BYTES`].
    XmlBytes,
    /// [`limits::MAX_ATTRIBUTES`].
    Attributes,
    /// [`limits::MAX_NAME_BYTES`].
    NameBytes,
    /// [`limits::MAX_NODE_SET`].
    NodeSet,
    /// [`limits::MAX_STRING_BYTES`].
    StringBytes,
    /// Caller-supplied evaluation fuel.
    Fuel,
}

/// Why an XML fragment was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmlError {
    /// The text is not a well-formed XML fragment.
    Malformed,
    /// A document type declaration was present (never processed).
    Doctype,
    /// An element or attribute used an undeclared namespace prefix.
    UndeclaredPrefix,
    /// An entity reference other than the five predefined entities or a
    /// valid character reference.
    Entity,
}

/// A typed, bounded failure. Messages are static.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The expression is not valid XPath 1.0 (including unknown functions,
    /// wrong arity, and undeclared prefixes).
    Syntax(SyntaxError),
    /// A `$name` reference the [`Environment`] does not bind.
    UndefinedVariable,
    /// An XPath type error, e.g. a path step applied to a non-node-set.
    Type,
    /// An extension function reported failure.
    Extension,
    /// A documented bound was exceeded.
    Limit(Limit),
    /// An XML fragment was rejected.
    Xml(XmlError),
    /// A stale, foreign, or wrong-kind node handle.
    InvalidNode,
    /// A structurally impossible mutation (cycle, sibling of a parentless
    /// node, attribute where a child is required).
    InvalidMutation,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(formatter, "XPath syntax error: {error}"),
            Self::UndefinedVariable => formatter.write_str("XPath variable is not bound"),
            Self::Type => formatter.write_str("XPath type error"),
            Self::Extension => formatter.write_str("XPath extension function failed"),
            Self::Limit(limit) => write!(formatter, "XPath bound exceeded: {limit:?}"),
            Self::Xml(error) => write!(formatter, "XML fragment rejected: {error:?}"),
            Self::InvalidNode => formatter.write_str("invalid XML node handle"),
            Self::InvalidMutation => formatter.write_str("invalid XML tree mutation"),
        }
    }
}

impl core::error::Error for Error {}

impl From<SyntaxError> for Error {
    fn from(error: SyntaxError) -> Self {
        Self::Syntax(error)
    }
}

impl From<Limit> for Error {
    fn from(limit: Limit) -> Self {
        Self::Limit(limit)
    }
}

impl From<XmlError> for Error {
    fn from(error: XmlError) -> Self {
        Self::Xml(error)
    }
}

/// Whether `name` is an XML `NCName` (no colon) within
/// [`limits::MAX_NAME_BYTES`].
#[must_use]
pub fn is_ncname(name: &str) -> bool {
    let mut characters = name.chars();
    name.len() <= limits::MAX_NAME_BYTES
        && characters.next().is_some_and(is_name_start)
        && characters.all(is_name_char)
}

/// XML 1.0 (fifth edition) `NameStartChar` without `:`.
pub(crate) fn is_name_start(character: char) -> bool {
    matches!(character,
        'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// XML 1.0 (fifth edition) `NameChar` without `:`.
pub(crate) fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || matches!(character,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// XML whitespace (`S` production).
pub(crate) fn is_xml_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}
