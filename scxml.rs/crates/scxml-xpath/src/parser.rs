//! Recursive-descent XPath 1.0 parser with bounded nesting, depth, and size.

use alloc::borrow::ToOwned as _;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::dom::XML_NAMESPACE;
use crate::lexer::{Axis, NodeType, Operator, Spanned, Token, tokenize};
use crate::{Error, Limit, SyntaxError, is_ncname, limits};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BinaryOp {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Union,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Function {
    Last,
    Position,
    Count,
    Id,
    LocalName,
    NamespaceUri,
    Name,
    String,
    Concat,
    StartsWith,
    Contains,
    SubstringBefore,
    SubstringAfter,
    Substring,
    StringLength,
    NormalizeSpace,
    Translate,
    Boolean,
    Not,
    True,
    False,
    Lang,
    Number,
    Sum,
    Floor,
    Ceiling,
    Round,
}

impl Function {
    /// Core function by name with its `(min, max)` arity.
    fn core(name: &str) -> Option<(Self, usize, usize)> {
        Some(match name {
            "last" => (Self::Last, 0, 0),
            "position" => (Self::Position, 0, 0),
            "count" => (Self::Count, 1, 1),
            "id" => (Self::Id, 1, 1),
            "local-name" => (Self::LocalName, 0, 1),
            "namespace-uri" => (Self::NamespaceUri, 0, 1),
            "name" => (Self::Name, 0, 1),
            "string" => (Self::String, 0, 1),
            "concat" => (Self::Concat, 2, usize::MAX),
            "starts-with" => (Self::StartsWith, 2, 2),
            "contains" => (Self::Contains, 2, 2),
            "substring-before" => (Self::SubstringBefore, 2, 2),
            "substring-after" => (Self::SubstringAfter, 2, 2),
            "substring" => (Self::Substring, 2, 3),
            "string-length" => (Self::StringLength, 0, 1),
            "normalize-space" => (Self::NormalizeSpace, 0, 1),
            "translate" => (Self::Translate, 3, 3),
            "boolean" => (Self::Boolean, 1, 1),
            "not" => (Self::Not, 1, 1),
            "true" => (Self::True, 0, 0),
            "false" => (Self::False, 0, 0),
            "lang" => (Self::Lang, 1, 1),
            "number" => (Self::Number, 0, 1),
            "sum" => (Self::Sum, 1, 1),
            "floor" => (Self::Floor, 1, 1),
            "ceiling" => (Self::Ceiling, 1, 1),
            "round" => (Self::Round, 1, 1),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NodeTest {
    /// `*`: any node of the principal type.
    Any,
    /// `prefix:*`: principal-type nodes in one namespace.
    Namespace(String),
    /// Expanded name test on the principal type.
    Name(Option<String>, String),
    /// `node()`.
    Node,
    /// `text()`.
    Text,
    /// `comment()`.
    Comment,
    /// `processing-instruction(literal?)`.
    ProcessingInstruction(Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Step {
    pub(crate) axis: Axis,
    pub(crate) test: NodeTest,
    pub(crate) predicates: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PathStart {
    /// Absolute path: the root of the context node's tree.
    Root,
    /// Relative location path: the context node.
    Context,
    /// Filter expression: a primary expression and its predicates.
    Filter(Box<Expr>, Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expr {
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Negate(Box<Expr>),
    Literal(String),
    Number(f64),
    Variable(String),
    Call(Function, Vec<Expr>),
    Extension(String, Vec<Expr>),
    Path(PathStart, Vec<Step>),
}

#[derive(Clone, Debug)]
struct ExtensionFunction {
    name: String,
    min_arguments: usize,
    max_arguments: usize,
}

/// Static context for parsing: namespace prefixes and extension functions.
#[derive(Clone, Debug)]
pub struct ParseOptions {
    namespaces: Vec<(String, String)>,
    functions: Vec<ExtensionFunction>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseOptions {
    /// Only the `xml` prefix; no extension functions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            namespaces: vec![("xml".into(), XML_NAMESPACE.into())],
            functions: Vec::new(),
        }
    }

    /// [`ParseOptions::new`] plus the SCXML `In(stateID)` extension function.
    #[must_use]
    pub fn scxml() -> Self {
        let mut options = Self::new();
        options.functions.push(ExtensionFunction {
            name: "In".into(),
            min_arguments: 1,
            max_arguments: 1,
        });
        options
    }

    /// Binds `prefix` to `uri` for name tests.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] for a non-`NCName` prefix, the reserved `xml`/
    /// `xmlns` prefixes, or an empty URI.
    pub fn with_namespace(mut self, prefix: &str, uri: &str) -> Result<Self, Error> {
        if !is_ncname(prefix) || prefix == "xml" || prefix == "xmlns" || uri.is_empty() {
            return Err(Error::Syntax(SyntaxError {
                start: 0,
                end: 0,
                message: "invalid namespace binding",
            }));
        }
        self.namespaces.retain(|(bound, _)| bound != prefix);
        self.namespaces.push((prefix.into(), uri.into()));
        Ok(self)
    }

    /// Declares extension function `name` (a lexical `QName` that is not a
    /// core function) accepting `min_arguments..=max_arguments` arguments.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] for an invalid or core name or an empty range.
    pub fn with_function(
        mut self,
        name: &str,
        min_arguments: usize,
        max_arguments: usize,
    ) -> Result<Self, Error> {
        let valid_name = match name.split_once(':') {
            Some((prefix, local)) => is_ncname(prefix) && is_ncname(local),
            None => is_ncname(name),
        };
        if !valid_name || Function::core(name).is_some() || min_arguments > max_arguments {
            return Err(Error::Syntax(SyntaxError {
                start: 0,
                end: 0,
                message: "invalid extension function declaration",
            }));
        }
        self.functions.retain(|function| function.name != name);
        self.functions.push(ExtensionFunction {
            name: name.into(),
            min_arguments,
            max_arguments,
        });
        Ok(self)
    }

    fn namespace(&self, prefix: &str) -> Option<&str> {
        self.namespaces
            .iter()
            .find(|(bound, _)| bound == prefix)
            .map(|(_, uri)| uri.as_str())
    }
}

/// A parsed, immutable XPath 1.0 expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub(crate) root: Expr,
}

impl Expression {
    /// Parses `source` against `options`.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] with an exact span for invalid XPath (including
    /// unknown functions, wrong arity, and undeclared prefixes);
    /// [`Error::Limit`] when a bound in [`limits`] is exceeded.
    pub fn parse(source: &str, options: &ParseOptions) -> Result<Self, Error> {
        if scxml_types::exceeds_wasm_bound(source.len(), limits::MAX_EXPRESSION_BYTES) {
            return Err(Error::Limit(Limit::ExpressionBytes));
        }
        let tokens = tokenize(source)?;
        let mut parser = Parser {
            tokens: &tokens,
            position: 0,
            source_len: source.len(),
            options,
            nesting: 0,
            nodes: 0,
        };
        let (root, _) = parser.expression()?;
        if let Some(extra) = parser.peek() {
            return Err(Error::Syntax(SyntaxError {
                start: extra.start,
                end: extra.end,
                message: "unexpected token after expression",
            }));
        }
        Ok(Self { root })
    }

    /// Whether evaluation can yield a node-set (statically): location and
    /// filter paths, unions, variables, `id()`, and extension calls. Used
    /// for SCXML `location` syntax checks.
    #[must_use]
    pub fn may_yield_node_set(&self) -> bool {
        matches!(
            self.root,
            Expr::Path(..)
                | Expr::Variable(_)
                | Expr::Binary(BinaryOp::Union, ..)
                | Expr::Call(Function::Id, _)
                | Expr::Extension(..)
        )
    }
}

/// Parse-only syntax check for validators; never evaluates.
///
/// # Errors
///
/// The first [`SyntaxError`] with its exact span. A bound violation is
/// reported as a syntax error spanning the whole source.
pub fn check_syntax(source: &str, options: &ParseOptions) -> Result<(), SyntaxError> {
    match Expression::parse(source, options) {
        Ok(_) => Ok(()),
        Err(Error::Syntax(error)) => Err(error),
        Err(_) => Err(SyntaxError {
            start: 0,
            end: source.len(),
            message: "expression exceeds a parser bound",
        }),
    }
}

struct Parser<'a> {
    tokens: &'a [Spanned],
    position: usize,
    source_len: usize,
    options: &'a ParseOptions,
    nesting: usize,
    nodes: usize,
}

type Parsed = (Expr, usize);

impl Parser<'_> {
    fn peek(&self) -> Option<&Spanned> {
        self.tokens.get(self.position)
    }

    fn peek_token(&self) -> Option<&Token> {
        self.peek().map(|spanned| &spanned.token)
    }

    fn advance(&mut self) -> Option<&Spanned> {
        let token = self.tokens.get(self.position);
        if token.is_some() {
            self.position = self.position.saturating_add(1);
        }
        token
    }

    fn error_here(&self, message: &'static str) -> Error {
        let (start, end) = self
            .peek()
            .map_or((self.source_len, self.source_len), |token| {
                (token.start, token.end)
            });
        Error::Syntax(SyntaxError {
            start,
            end,
            message,
        })
    }

    fn expect(&mut self, token: &Token, message: &'static str) -> Result<(), Error> {
        if self.peek_token() == Some(token) {
            self.position = self.position.saturating_add(1);
            Ok(())
        } else {
            Err(self.error_here(message))
        }
    }

    fn node(&mut self, depth: usize) -> Result<usize, Error> {
        self.nodes = self.nodes.saturating_add(1);
        if scxml_types::exceeds_wasm_bound(self.nodes, limits::MAX_AST_NODES) {
            return Err(Error::Limit(Limit::AstNodes));
        }
        if depth > limits::MAX_AST_DEPTH {
            return Err(Error::Limit(Limit::AstDepth));
        }
        Ok(depth)
    }

    fn binary(&mut self, op: BinaryOp, left: Parsed, right: Parsed) -> Result<Parsed, Error> {
        let depth = self.node(left.1.max(right.1).saturating_add(1))?;
        Ok((Expr::Binary(op, Box::new(left.0), Box::new(right.0)), depth))
    }

    /// Nested `Expr` (parenthesized, predicate, or argument) with a bounded
    /// recursion depth.
    fn expression(&mut self) -> Result<Parsed, Error> {
        self.nesting = self.nesting.saturating_add(1);
        if self.nesting > limits::MAX_PARSE_NESTING {
            return Err(Error::Limit(Limit::AstDepth));
        }
        let result = self.or_expr();
        self.nesting = self.nesting.saturating_sub(1);
        result
    }

    fn or_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.and_expr()?;
        while self.peek_token() == Some(&Token::Operator(Operator::Or)) {
            self.advance();
            let right = self.and_expr()?;
            left = self.binary(BinaryOp::Or, left, right)?;
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.equality_expr()?;
        while self.peek_token() == Some(&Token::Operator(Operator::And)) {
            self.advance();
            let right = self.equality_expr()?;
            left = self.binary(BinaryOp::And, left, right)?;
        }
        Ok(left)
    }

    fn equality_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.relational_expr()?;
        loop {
            let op = match self.peek_token() {
                Some(Token::Operator(Operator::Equal)) => BinaryOp::Equal,
                Some(Token::Operator(Operator::NotEqual)) => BinaryOp::NotEqual,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.relational_expr()?;
            left = self.binary(op, left, right)?;
        }
    }

    fn relational_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.additive_expr()?;
        loop {
            let op = match self.peek_token() {
                Some(Token::Operator(Operator::Less)) => BinaryOp::Less,
                Some(Token::Operator(Operator::LessEqual)) => BinaryOp::LessEqual,
                Some(Token::Operator(Operator::Greater)) => BinaryOp::Greater,
                Some(Token::Operator(Operator::GreaterEqual)) => BinaryOp::GreaterEqual,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.additive_expr()?;
            left = self.binary(op, left, right)?;
        }
    }

    fn additive_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.multiplicative_expr()?;
        loop {
            let op = match self.peek_token() {
                Some(Token::Operator(Operator::Plus)) => BinaryOp::Add,
                Some(Token::Operator(Operator::Minus)) => BinaryOp::Subtract,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.multiplicative_expr()?;
            left = self.binary(op, left, right)?;
        }
    }

    fn multiplicative_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.unary_expr()?;
        loop {
            let op = match self.peek_token() {
                Some(Token::Operator(Operator::Multiply)) => BinaryOp::Multiply,
                Some(Token::Operator(Operator::Div)) => BinaryOp::Divide,
                Some(Token::Operator(Operator::Mod)) => BinaryOp::Modulo,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.unary_expr()?;
            left = self.binary(op, left, right)?;
        }
    }

    fn unary_expr(&mut self) -> Result<Parsed, Error> {
        let mut negations = 0_usize;
        while self.peek_token() == Some(&Token::Operator(Operator::Minus)) {
            self.advance();
            negations = negations.saturating_add(1);
            if negations > limits::MAX_AST_DEPTH {
                return Err(Error::Limit(Limit::AstDepth));
            }
        }
        let (mut expr, mut depth) = self.union_expr()?;
        for _ in 0..negations {
            depth = self.node(depth.saturating_add(1))?;
            expr = Expr::Negate(Box::new(expr));
        }
        Ok((expr, depth))
    }

    fn union_expr(&mut self) -> Result<Parsed, Error> {
        let mut left = self.path_expr()?;
        while self.peek_token() == Some(&Token::Operator(Operator::Pipe)) {
            self.advance();
            let right = self.path_expr()?;
            left = self.binary(BinaryOp::Union, left, right)?;
        }
        Ok(left)
    }

    fn starts_step(token: Option<&Token>) -> bool {
        matches!(
            token,
            Some(
                Token::AnyName
                    | Token::PrefixAny(_)
                    | Token::Name(..)
                    | Token::NodeType(_)
                    | Token::At
                    | Token::Dot
                    | Token::DotDot
                    | Token::AxisName(_)
            )
        )
    }

    fn path_expr(&mut self) -> Result<Parsed, Error> {
        match self.peek_token() {
            Some(Token::Operator(Operator::Slash)) => {
                self.advance();
                let mut steps = Vec::new();
                let mut depth = self.node(1)?;
                if Self::starts_step(self.peek_token()) {
                    depth = depth.max(self.relative_path(&mut steps)?);
                }
                Ok((Expr::Path(PathStart::Root, steps), depth))
            }
            Some(Token::Operator(Operator::DoubleSlash)) => {
                self.advance();
                let mut steps = vec![descendant_or_self()];
                let depth = self.node(1)?.max(self.relative_path(&mut steps)?);
                Ok((Expr::Path(PathStart::Root, steps), depth))
            }
            token if Self::starts_step(token) => {
                let mut steps = Vec::new();
                let depth = self.node(1)?.max(self.relative_path(&mut steps)?);
                Ok((Expr::Path(PathStart::Context, steps), depth))
            }
            _ => self.filter_path(),
        }
    }

    fn filter_path(&mut self) -> Result<Parsed, Error> {
        let (primary, mut depth) = self.primary_expr()?;
        let mut predicates = Vec::new();
        while self.peek_token() == Some(&Token::LeftBracket) {
            let (predicate, predicate_depth) = self.predicate()?;
            depth = depth.max(predicate_depth);
            predicates.push(predicate);
        }
        let mut steps = Vec::new();
        match self.peek_token() {
            Some(Token::Operator(Operator::Slash)) => {
                self.advance();
                depth = depth.max(self.relative_path(&mut steps)?);
            }
            Some(Token::Operator(Operator::DoubleSlash)) => {
                self.advance();
                steps.push(descendant_or_self());
                depth = depth.max(self.relative_path(&mut steps)?);
            }
            _ => {}
        }
        if predicates.is_empty() && steps.is_empty() {
            return Ok((primary, depth));
        }
        let depth = self.node(depth.saturating_add(1))?;
        Ok((
            Expr::Path(PathStart::Filter(Box::new(primary), predicates), steps),
            depth,
        ))
    }

    /// Parses `Step (('/' | '//') Step)*`, returning the deepest predicate.
    fn relative_path(&mut self, steps: &mut Vec<Step>) -> Result<usize, Error> {
        let mut depth = self.step(steps)?;
        loop {
            match self.peek_token() {
                Some(Token::Operator(Operator::Slash)) => {
                    self.advance();
                }
                Some(Token::Operator(Operator::DoubleSlash)) => {
                    self.advance();
                    steps.push(descendant_or_self());
                }
                _ => return Ok(depth),
            }
            depth = depth.max(self.step(steps)?);
        }
    }

    fn step(&mut self, steps: &mut Vec<Step>) -> Result<usize, Error> {
        self.node(1)?;
        match self.peek_token() {
            Some(Token::Dot) => {
                self.advance();
                steps.push(Step {
                    axis: Axis::SelfNode,
                    test: NodeTest::Node,
                    predicates: Vec::new(),
                });
                return Ok(1);
            }
            Some(Token::DotDot) => {
                self.advance();
                steps.push(Step {
                    axis: Axis::Parent,
                    test: NodeTest::Node,
                    predicates: Vec::new(),
                });
                return Ok(1);
            }
            _ => {}
        }
        let axis = match self.peek_token() {
            Some(Token::AxisName(axis)) => {
                let axis = *axis;
                self.advance();
                self.expect(&Token::ColonColon, "expected '::' after axis name")?;
                axis
            }
            Some(Token::At) => {
                self.advance();
                Axis::Attribute
            }
            _ => Axis::Child,
        };
        let test = self.node_test()?;
        let mut predicates = Vec::new();
        let mut depth = 1;
        while self.peek_token() == Some(&Token::LeftBracket) {
            let (predicate, predicate_depth) = self.predicate()?;
            depth = depth.max(predicate_depth.saturating_add(1));
            predicates.push(predicate);
        }
        steps.push(Step {
            axis,
            test,
            predicates,
        });
        self.node(depth)
    }

    fn node_test(&mut self) -> Result<NodeTest, Error> {
        let Some(spanned) = self.peek().cloned() else {
            return Err(self.error_here("expected a node test"));
        };
        self.advance();
        match &spanned.token {
            Token::AnyName => Ok(NodeTest::Any),
            Token::PrefixAny(prefix) => {
                Ok(NodeTest::Namespace(self.resolve_prefix(prefix, &spanned)?))
            }
            Token::Name(None, local) => Ok(NodeTest::Name(None, local.clone())),
            Token::Name(Some(prefix), local) => {
                let uri = self.resolve_prefix(prefix, &spanned)?;
                Ok(NodeTest::Name(Some(uri), local.clone()))
            }
            Token::NodeType(node_type) => {
                self.expect(&Token::LeftParen, "expected '(' after node type")?;
                let test = match *node_type {
                    NodeType::Comment => NodeTest::Comment,
                    NodeType::Text => NodeTest::Text,
                    NodeType::Node => NodeTest::Node,
                    NodeType::ProcessingInstruction => {
                        if let Some(Token::Literal(target)) = self.peek_token() {
                            let target = target.clone();
                            self.advance();
                            NodeTest::ProcessingInstruction(Some(target))
                        } else {
                            NodeTest::ProcessingInstruction(None)
                        }
                    }
                };
                self.expect(&Token::RightParen, "expected ')' to close node type test")?;
                Ok(test)
            }
            _ => Err(Error::Syntax(SyntaxError {
                start: spanned.start,
                end: spanned.end,
                message: "expected a node test",
            })),
        }
    }

    fn resolve_prefix(&self, prefix: &str, spanned: &Spanned) -> Result<String, Error> {
        self.options
            .namespace(prefix)
            .map(str::to_owned)
            .ok_or(Error::Syntax(SyntaxError {
                start: spanned.start,
                end: spanned.end,
                message: "undeclared namespace prefix",
            }))
    }

    fn predicate(&mut self) -> Result<Parsed, Error> {
        self.expect(&Token::LeftBracket, "expected '['")?;
        let parsed = self.expression()?;
        self.expect(&Token::RightBracket, "expected ']' to close predicate")?;
        Ok(parsed)
    }

    fn primary_expr(&mut self) -> Result<Parsed, Error> {
        let Some(spanned) = self.peek().cloned() else {
            return Err(self.error_here("expected an expression"));
        };
        match &spanned.token {
            Token::Variable(name) => {
                self.advance();
                if let Some((prefix, _)) = name.split_once(':') {
                    self.resolve_prefix(prefix, &spanned)?;
                }
                Ok((Expr::Variable(name.clone()), self.node(1)?))
            }
            Token::LeftParen => {
                self.advance();
                let (expr, depth) = self.expression()?;
                self.expect(&Token::RightParen, "expected ')'")?;
                Ok((expr, depth))
            }
            Token::Literal(text) => {
                self.advance();
                Ok((Expr::Literal(text.clone()), self.node(1)?))
            }
            Token::Number(value) => {
                self.advance();
                Ok((Expr::Number(*value), self.node(1)?))
            }
            Token::Function(name) => {
                self.advance();
                self.function_call(name, &spanned)
            }
            _ => Err(Error::Syntax(SyntaxError {
                start: spanned.start,
                end: spanned.end,
                message: "expected an expression",
            })),
        }
    }

    fn function_call(&mut self, name: &str, spanned: &Spanned) -> Result<Parsed, Error> {
        self.expect(&Token::LeftParen, "expected '(' after function name")?;
        let mut arguments = Vec::new();
        let mut depth = 0;
        if self.peek_token() != Some(&Token::RightParen) {
            loop {
                let (argument, argument_depth) = self.expression()?;
                depth = depth.max(argument_depth);
                arguments.push(argument);
                if self.peek_token() == Some(&Token::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        self.expect(&Token::RightParen, "expected ')' to close function call")?;
        let arity_error = || {
            Error::Syntax(SyntaxError {
                start: spanned.start,
                end: spanned.end,
                message: "wrong number of function arguments",
            })
        };
        let depth = self.node(depth.saturating_add(1))?;
        if let Some((function, min, max)) = Function::core(name) {
            if arguments.len() < min || arguments.len() > max {
                return Err(arity_error());
            }
            return Ok((Expr::Call(function, arguments), depth));
        }
        let Some(extension) = self
            .options
            .functions
            .iter()
            .find(|function| function.name == name)
        else {
            return Err(Error::Syntax(SyntaxError {
                start: spanned.start,
                end: spanned.end,
                message: "unknown function",
            }));
        };
        if arguments.len() < extension.min_arguments || arguments.len() > extension.max_arguments {
            return Err(arity_error());
        }
        Ok((Expr::Extension(name.to_owned(), arguments), depth))
    }
}

fn descendant_or_self() -> Step {
    Step {
        axis: Axis::DescendantOrSelf,
        test: NodeTest::Node,
        predicates: Vec::new(),
    }
}
