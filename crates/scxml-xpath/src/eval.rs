//! XPath 1.0 evaluation: values, axes, node tests, predicates, the core
//! function library, and the §3.4 comparison rules. Work is fuel-bounded
//! (string work in proportion to its bytes; see `limits::DEFAULT_FUEL`);
//! recursion follows the parsed tree, whose depth the parser bounds.
//!
//! Evaluation runs on an internal value type whose strings borrow from the
//! store and the parsed expression, so string-values, literals, and
//! comparisons copy nothing; node numbers come from the store's kept leaf
//! numbers. Every fuel charge is the one the XPath 1.0 cost model in
//! [`crate::limits::DEFAULT_FUEL`] documents, in the same order, whether or
//! not a copy is made.

use alloc::borrow::Cow;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::dom::{
    Kind, NIL, Node, NodeId, NodeRef, XML_NAMESPACE, burn, burn_bytes, burn_units, push_bounded,
};
use crate::lexer::Axis;
use crate::number::{ceiling, floor, number_to_string, round, string_to_number};
use crate::parser::{BinaryOp, Expr, Expression, Function, NodeTest, PathStart, Step};
use crate::{Error, Limit, NodeKind, Store, is_xml_space, limits};

/// Caller-supplied dynamic context: variable bindings and extension
/// functions declared through [`crate::ParseOptions::with_function`].
pub trait Environment {
    /// Value of `$name` (the lexical `QName` as written), or `None` when
    /// unbound (evaluation then fails with [`Error::UndefinedVariable`]).
    fn variable(&self, name: &str) -> Option<Value>;

    /// Evaluates extension function `name` over already-evaluated arguments.
    ///
    /// # Errors
    ///
    /// Any [`Error`]; [`Error::Extension`] is the conventional failure (for
    /// example a failed SCXML `In()` host query).
    fn call_extension(
        &mut self,
        name: &str,
        arguments: &[Value],
        store: &Store,
    ) -> Result<Value, Error>;
}

/// A node-set: nodes in document order without duplicates.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeSet(Vec<NodeRef>);

impl NodeSet {
    /// The empty node-set.
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Builds a node-set, sorting into document order and removing
    /// duplicates.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] for a stale handle; [`Error::Limit`] above
    /// [`limits::MAX_NODE_SET`] nodes.
    pub fn from_nodes(store: &Store, mut nodes: Vec<NodeRef>) -> Result<Self, Error> {
        if scxml_types::exceeds_wasm_bound(nodes.len(), limits::MAX_NODE_SET) {
            return Err(Error::Limit(Limit::NodeSet));
        }
        let mut fuel = u64::MAX;
        for node in &nodes {
            store.kind(*node)?;
        }
        store.sort_document_order(&mut nodes, &mut fuel)?;
        Ok(Self(nodes))
    }

    /// Nodes in document order.
    #[must_use]
    pub fn nodes(&self) -> &[NodeRef] {
        &self.0
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// First node in document order.
    #[must_use]
    pub fn first(&self) -> Option<NodeRef> {
        self.0.first().copied()
    }
}

/// An XPath 1.0 value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Node-set.
    NodeSet(NodeSet),
    /// Boolean.
    Boolean(bool),
    /// IEEE 754 double.
    Number(f64),
    /// String.
    String(String),
}

impl Value {
    /// XPath `boolean()`.
    #[must_use]
    pub fn to_boolean(&self) -> bool {
        match self {
            Self::NodeSet(nodes) => !nodes.is_empty(),
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
        }
    }

    /// XPath `number()`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] or [`Error::Limit`] while reading a node-set's
    /// string-value.
    pub fn to_number(&self, store: &Store) -> Result<f64, Error> {
        let mut fuel = u64::MAX;
        Ok(match self {
            Self::Number(value) => *value,
            Self::Boolean(value) => bool_number(*value),
            Self::String(value) => string_to_number(value),
            Self::NodeSet(nodes) => match nodes.first() {
                Some(node) => store.number_value_with(node, &mut fuel)?,
                None => string_to_number(""),
            },
        })
    }

    /// XPath `string()`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNode`] or [`Error::Limit`] while reading a node-set's
    /// string-value.
    pub fn to_string(&self, store: &Store) -> Result<String, Error> {
        let mut fuel = u64::MAX;
        Ok(match self {
            Self::NodeSet(nodes) => match nodes.first() {
                Some(node) => store.string_value_with(node, &mut fuel)?.into_owned(),
                None => String::new(),
            },
            Self::Boolean(value) => bool_str(*value).into(),
            Self::Number(value) => number_to_string(*value),
            Self::String(value) => value.clone(),
        })
    }
}

const fn bool_number(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

const fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// Node lists up to this length live inline (no allocation).
const INLINE_NODES: usize = 2;

/// An evaluation-time node list: small lists live inline, larger ones on
/// the heap. Dereferences to a slice.
#[derive(Clone, Debug)]
enum NodeList {
    Inline {
        len: usize,
        nodes: [NodeRef; INLINE_NODES],
    },
    Heap(Vec<NodeRef>),
}

impl Default for NodeList {
    fn default() -> Self {
        Self::Inline {
            len: 0,
            nodes: [NodeRef::PLACEHOLDER; INLINE_NODES],
        }
    }
}

impl NodeList {
    fn one(node: NodeRef) -> Self {
        let mut list = Self::default();
        list.push(node);
        list
    }

    fn from_vec(nodes: Vec<NodeRef>) -> Self {
        Self::Heap(nodes)
    }

    fn into_vec(self) -> Vec<NodeRef> {
        match self {
            Self::Inline { len, nodes } => nodes.get(..len).unwrap_or_default().to_vec(),
            Self::Heap(nodes) => nodes,
        }
    }

    fn push(&mut self, node: NodeRef) {
        match self {
            Self::Inline { len, nodes } => {
                if let Some(slot) = nodes.get_mut(*len) {
                    *slot = node;
                    *len = len.saturating_add(1);
                } else {
                    let mut heap = Vec::with_capacity(INLINE_NODES.saturating_mul(2));
                    heap.extend_from_slice(nodes);
                    heap.push(node);
                    *self = Self::Heap(heap);
                }
            }
            Self::Heap(nodes) => nodes.push(node),
        }
    }

    fn truncate(&mut self, length: usize) {
        match self {
            Self::Inline { len, .. } => *len = (*len).min(length),
            Self::Heap(nodes) => nodes.truncate(length),
        }
    }

    fn extend_from_slice(&mut self, more: &[NodeRef]) {
        for node in more {
            self.push(*node);
        }
    }
}

impl core::ops::Deref for NodeList {
    type Target = [NodeRef];

    fn deref(&self) -> &[NodeRef] {
        match self {
            Self::Inline { len, nodes } => nodes.get(..*len).unwrap_or_default(),
            Self::Heap(nodes) => nodes,
        }
    }
}

impl core::ops::DerefMut for NodeList {
    fn deref_mut(&mut self) -> &mut [NodeRef] {
        match self {
            Self::Inline { len, nodes } => nodes.get_mut(..*len).unwrap_or_default(),
            Self::Heap(nodes) => nodes,
        }
    }
}

/// An evaluation-time value: strings borrow from the store or the parsed
/// expression whenever they can.
#[derive(Clone, Debug)]
enum Val<'a> {
    Nodes(NodeList),
    Bool(bool),
    Num(f64),
    Str(Cow<'a, str>),
}

impl Val<'_> {
    fn from_value(value: Value) -> Self {
        match value {
            Value::NodeSet(nodes) => Self::Nodes(NodeList::from_vec(nodes.0)),
            Value::Boolean(value) => Self::Bool(value),
            Value::Number(value) => Self::Num(value),
            Value::String(value) => Self::Str(Cow::Owned(value)),
        }
    }

    fn into_value(self) -> Value {
        match self {
            Self::Nodes(nodes) => Value::NodeSet(NodeSet(nodes.into_vec())),
            Self::Bool(value) => Value::Boolean(value),
            Self::Num(value) => Value::Number(value),
            Self::Str(value) => Value::String(value.into_owned()),
        }
    }

    fn to_boolean(&self) -> bool {
        match self {
            Self::Nodes(nodes) => !nodes.is_empty(),
            Self::Bool(value) => *value,
            Self::Num(value) => *value != 0.0 && !value.is_nan(),
            Self::Str(value) => !value.is_empty(),
        }
    }

    const fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    const fn is_num(&self) -> bool {
        matches!(self, Self::Num(_))
    }
}

impl Expression {
    /// Evaluates this expression with `context` as the context node
    /// (position 1, size 1), consuming at most `fuel` work units.
    ///
    /// # Errors
    ///
    /// [`Error::UndefinedVariable`], [`Error::Type`], [`Error::Extension`]
    /// (or any error an extension returns), [`Error::InvalidNode`] for a
    /// stale context, and [`Error::Limit`] for fuel, node-set, and string
    /// bounds.
    pub fn evaluate(
        &self,
        store: &Store,
        context: NodeRef,
        environment: &mut dyn Environment,
        fuel: u64,
    ) -> Result<Value, Error> {
        store.kind(context)?;
        let mut evaluator = Evaluator {
            store,
            environment,
            fuel,
        };
        evaluator
            .eval(
                &self.root,
                Focus {
                    node: context,
                    position: 1,
                    size: 1,
                },
            )
            .map(Val::into_value)
    }
}

#[derive(Clone, Copy)]
struct Focus {
    node: NodeRef,
    position: usize,
    size: usize,
}

struct Evaluator<'a, 'e> {
    store: &'a Store,
    environment: &'e mut dyn Environment,
    fuel: u64,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "positions, sizes, counts, and lengths are bounded far below 2^53"
)]
fn usize_to_f64(value: usize) -> f64 {
    value as f64
}

/// The direction a run of siblings is walked in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Siblings {
    /// From a node through its following siblings.
    Following,
    /// From a node through its preceding siblings.
    Preceding,
}

/// A node test resolved against one store: name tests carry the store's
/// expanded-name id (`None` when no node in the store has that name).
#[derive(Clone, Copy)]
enum Test<'t> {
    Node,
    Text,
    Comment,
    ProcessingInstruction(Option<&'t str>),
    Any,
    Namespace(&'t str),
    Name {
        uri: Option<&'t str>,
        local: &'t str,
        expanded: Option<u32>,
    },
}

impl<'a> Evaluator<'a, '_> {
    fn burn(&mut self) -> Result<(), Error> {
        burn(&mut self.fuel)
    }

    fn resolve<'t>(&self, test: &'t NodeTest) -> Test<'t> {
        match test {
            NodeTest::Node => Test::Node,
            NodeTest::Text => Test::Text,
            NodeTest::Comment => Test::Comment,
            NodeTest::ProcessingInstruction(target) => {
                Test::ProcessingInstruction(target.as_deref())
            }
            NodeTest::Any => Test::Any,
            NodeTest::Namespace(uri) => Test::Namespace(uri),
            NodeTest::Name(uri, local) => Test::Name {
                uri: uri.as_deref(),
                local,
                expanded: self.store.names.expanded(uri.as_deref(), local),
            },
        }
    }

    fn eval(&mut self, expr: &'a Expr, focus: Focus) -> Result<Val<'a>, Error> {
        self.burn()?;
        match expr {
            Expr::Literal(text) => {
                burn_bytes(&mut self.fuel, text.len())?;
                Ok(Val::Str(Cow::Borrowed(text)))
            }
            Expr::Number(value) => Ok(Val::Num(*value)),
            Expr::Variable(name) => {
                let value = self
                    .environment
                    .variable(name)
                    .ok_or(Error::UndefinedVariable)?;
                match &value {
                    Value::String(text) => burn_bytes(&mut self.fuel, text.len())?,
                    Value::NodeSet(nodes) => burn_units(&mut self.fuel, nodes.len())?,
                    Value::Boolean(_) | Value::Number(_) => {}
                }
                Ok(Val::from_value(value))
            }
            Expr::Negate(inner) => {
                let value = self.eval(inner, focus)?;
                Ok(Val::Num(-self.number(&value)?))
            }
            Expr::Binary(op, left, right) => self.binary(*op, left, right, focus),
            Expr::Call(function, arguments) => self.call(*function, arguments, focus),
            Expr::Extension(name, arguments) => {
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.eval(argument, focus)?.into_value());
                }
                self.environment
                    .call_extension(name, &values, self.store)
                    .map(Val::from_value)
            }
            Expr::Path(start, steps) => self.path(start, steps, focus),
        }
    }

    /// XPath `string()`, charging fuel for the bytes it materializes.
    fn string(&mut self, value: &Val<'a>) -> Result<Cow<'a, str>, Error> {
        Ok(match value {
            Val::Nodes(nodes) => match nodes.first() {
                Some(node) => self.store.string_value_with(*node, &mut self.fuel)?,
                None => Cow::Borrowed(""),
            },
            Val::Bool(value) => Cow::Borrowed(bool_str(*value)),
            Val::Num(value) => {
                let text = number_to_string(*value);
                burn_bytes(&mut self.fuel, text.len())?;
                Cow::Owned(text)
            }
            Val::Str(value) => {
                burn_bytes(&mut self.fuel, value.len())?;
                value.clone()
            }
        })
    }

    /// [`Self::string`] consuming its value, so owned strings move.
    fn owned_string(&mut self, value: Val<'a>) -> Result<Cow<'a, str>, Error> {
        match value {
            Val::Str(value) => {
                burn_bytes(&mut self.fuel, value.len())?;
                Ok(value)
            }
            other => self.string(&other),
        }
    }

    /// XPath `number()`, charging fuel for the bytes it scans.
    fn number(&mut self, value: &Val<'a>) -> Result<f64, Error> {
        Ok(match value {
            Val::Num(value) => *value,
            Val::Bool(value) => bool_number(*value),
            Val::Str(value) => {
                burn_bytes(&mut self.fuel, value.len())?;
                string_to_number(value)
            }
            Val::Nodes(nodes) => match nodes.first() {
                Some(node) => self.store.number_value_with(*node, &mut self.fuel)?,
                None => string_to_number(""),
            },
        })
    }

    fn string_value(&mut self, node: NodeRef) -> Result<Cow<'a, str>, Error> {
        self.store.string_value_with(node, &mut self.fuel)
    }

    fn number_value(&mut self, node: NodeRef) -> Result<f64, Error> {
        self.store.number_value_with(node, &mut self.fuel)
    }

    fn node_set(value: Val<'_>) -> Result<NodeList, Error> {
        match value {
            Val::Nodes(nodes) => Ok(nodes),
            _ => Err(Error::Type),
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        left: &'a Expr,
        right: &'a Expr,
        focus: Focus,
    ) -> Result<Val<'a>, Error> {
        match op {
            BinaryOp::Or => {
                if self.eval(left, focus)?.to_boolean() {
                    return Ok(Val::Bool(true));
                }
                Ok(Val::Bool(self.eval(right, focus)?.to_boolean()))
            }
            BinaryOp::And => {
                if !self.eval(left, focus)?.to_boolean() {
                    return Ok(Val::Bool(false));
                }
                Ok(Val::Bool(self.eval(right, focus)?.to_boolean()))
            }
            BinaryOp::Union => {
                let mut nodes = Self::node_set(self.eval(left, focus)?)?;
                let right = Self::node_set(self.eval(right, focus)?)?;
                nodes.extend_from_slice(&right);
                if scxml_types::exceeds_wasm_bound(nodes.len(), limits::MAX_NODE_SET) {
                    return Err(Error::Limit(Limit::NodeSet));
                }
                self.sort(&mut nodes)?;
                Ok(Val::Nodes(nodes))
            }
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo => {
                let left = self.eval(left, focus)?;
                let right = self.eval(right, focus)?;
                let left = self.number(&left)?;
                let right = self.number(&right)?;
                Ok(Val::Num(match op {
                    BinaryOp::Add => left + right,
                    BinaryOp::Subtract => left - right,
                    BinaryOp::Multiply => left * right,
                    BinaryOp::Divide => left / right,
                    _ => left % right,
                }))
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual => {
                let left = self.eval(left, focus)?;
                let right = self.eval(right, focus)?;
                Ok(Val::Bool(self.compare(op, &left, &right)?))
            }
        }
    }

    /// XPath 1.0 §3.4.
    fn compare(&mut self, op: BinaryOp, left: &Val<'a>, right: &Val<'a>) -> Result<bool, Error> {
        match (left, right) {
            (Val::Nodes(left), Val::Nodes(right)) => self.compare_sets(op, left, right),
            (Val::Nodes(set), other) => self.compare_set_scalar(op, set, other),
            (other, Val::Nodes(set)) => self.compare_set_scalar(mirror(op), set, other),
            _ => {
                if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
                    let equal = if left.is_bool() || right.is_bool() {
                        left.to_boolean() == right.to_boolean()
                    } else if left.is_num() || right.is_num() {
                        #[allow(
                            clippy::float_cmp,
                            reason = "XPath equality is exact IEEE 754 equality"
                        )]
                        let equal = self.number(left)? == self.number(right)?;
                        equal
                    } else {
                        self.string(left)? == self.string(right)?
                    };
                    Ok(if op == BinaryOp::Equal { equal } else { !equal })
                } else {
                    let left = self.number(left)?;
                    let right = self.number(right)?;
                    Ok(relational(op, left, right))
                }
            }
        }
    }

    fn set_strings(&mut self, set: &[NodeRef]) -> Result<Vec<Cow<'a, str>>, Error> {
        let mut strings = Vec::with_capacity(set.len());
        for node in set {
            strings.push(self.string_value(*node)?);
        }
        Ok(strings)
    }

    /// The numbers of a node-set's string-values that are not NaN, charged
    /// as reading those string-values.
    fn set_numbers(&mut self, set: &[NodeRef]) -> Result<Vec<f64>, Error> {
        let mut numbers = Vec::with_capacity(set.len());
        for node in set {
            let value = self.number_value(*node)?;
            if !value.is_nan() {
                numbers.push(value);
            }
        }
        Ok(numbers)
    }

    fn compare_sets(
        &mut self,
        op: BinaryOp,
        left: &[NodeRef],
        right: &[NodeRef],
    ) -> Result<bool, Error> {
        if left.is_empty() || right.is_empty() {
            return Ok(false);
        }
        if !matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
            let left = self.set_numbers(left)?;
            let right = self.set_numbers(right)?;
            let (Some(left_min), Some(left_max)) = (min(&left), max(&left)) else {
                return Ok(false);
            };
            let (Some(right_min), Some(right_max)) = (min(&right), max(&right)) else {
                return Ok(false);
            };
            return Ok(match op {
                BinaryOp::Less => left_min < right_max,
                BinaryOp::LessEqual => left_min <= right_max,
                BinaryOp::Greater => left_max > right_min,
                _ => left_max >= right_min,
            });
        }
        let left = self.set_strings(left)?;
        let right = self.set_strings(right)?;
        // Ordered-set insertion and lookup: one unit per string.
        burn_units(&mut self.fuel, left.len().saturating_add(right.len()))?;
        if op == BinaryOp::Equal {
            let lookup: BTreeSet<&str> = left.iter().map(AsRef::as_ref).collect();
            Ok(right.iter().any(|value| lookup.contains(value.as_ref())))
        } else {
            let distinct: BTreeSet<&str> =
                left.iter().chain(right.iter()).map(AsRef::as_ref).collect();
            Ok(distinct.len() > 1)
        }
    }

    /// `set op scalar` with the node-set on the left.
    fn compare_set_scalar(
        &mut self,
        op: BinaryOp,
        set: &[NodeRef],
        scalar: &Val<'a>,
    ) -> Result<bool, Error> {
        match scalar {
            Val::Bool(value) => {
                let set_value = Val::Bool(!set.is_empty());
                self.compare(op, &set_value, &Val::Bool(*value))
            }
            Val::Num(number) => {
                for node in set {
                    let value = self.number_value(*node)?;
                    if relational_or_equal(op, value, *number) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Val::Str(text) => {
                if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
                    for node in set {
                        let value = self.string_value(*node)?;
                        if (value == *text) == (op == BinaryOp::Equal) {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else {
                    let number = string_to_number(text);
                    for node in set {
                        let value = self.number_value(*node)?;
                        if relational(op, value, number) {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
            }
            Val::Nodes(other) => self.compare_sets(op, set, other),
        }
    }

    fn path(
        &mut self,
        start: &'a PathStart,
        steps: &'a [Step],
        focus: Focus,
    ) -> Result<Val<'a>, Error> {
        let origin = match start {
            PathStart::Root => Some(NodeRef::node(self.store.tree_root(focus.node)?)),
            PathStart::Context => Some(focus.node),
            PathStart::Filter(..) => None,
        };
        let (mut current, steps) = match (origin, steps.split_first()) {
            // A location path starts from one node: its first step selects
            // directly from it.
            (Some(origin), Some((first, rest))) => (self.step_from(first, origin)?, rest),
            (Some(origin), None) => (NodeList::one(origin), steps),
            (None, _) => {
                let PathStart::Filter(primary, predicates) = start else {
                    return Err(Error::Type);
                };
                let mut nodes = Self::node_set(self.eval(primary, focus)?)?;
                for predicate in predicates {
                    nodes = self.filter(nodes, predicate)?;
                }
                (nodes, steps)
            }
        };
        for step in steps {
            current = self.step(step, &current)?;
        }
        Ok(Val::Nodes(current))
    }

    /// Nodes one input selects through `step`: axis, predicates, and
    /// document order.
    fn select(&mut self, step: &'a Step, node: NodeRef) -> Result<NodeList, Error> {
        let mut selected = self.axis(step.axis, node, &step.test)?;
        for predicate in &step.predicates {
            selected = self.filter(selected, predicate)?;
        }
        if step.axis.is_reverse() {
            selected.reverse();
        }
        Ok(selected)
    }

    /// `step` applied to a single input node.
    fn step_from(&mut self, step: &'a Step, input: NodeRef) -> Result<NodeList, Error> {
        let selected = self.select(step, input)?;
        if scxml_types::exceeds_wasm_bound(selected.len(), limits::MAX_NODE_SET) {
            return Err(Error::Limit(Limit::NodeSet));
        }
        Ok(selected)
    }

    fn step(&mut self, step: &'a Step, inputs: &[NodeRef]) -> Result<NodeList, Error> {
        if let [input] = inputs {
            return self.step_from(step, *input);
        }
        let mut output = NodeList::default();
        for node in inputs {
            let selected = self.select(step, *node)?;
            if scxml_types::exceeds_wasm_bound(
                output.len().saturating_add(selected.len()),
                limits::MAX_NODE_SET,
            ) {
                return Err(Error::Limit(Limit::NodeSet));
            }
            output.extend_from_slice(&selected);
        }
        if inputs.len() > 1 {
            self.sort(&mut output)?;
        }
        Ok(output)
    }

    /// Sorts a node list into document order without duplicates.
    fn sort(&mut self, nodes: &mut NodeList) -> Result<(), Error> {
        if nodes.len() <= 1 {
            return Ok(());
        }
        let mut sorted = core::mem::take(nodes).into_vec();
        self.store
            .sort_document_order(&mut sorted, &mut self.fuel)?;
        *nodes = NodeList::from_vec(sorted);
        Ok(())
    }

    /// Applies one predicate to nodes in axis order (proximity positions),
    /// keeping the survivors in place.
    fn filter(&mut self, mut nodes: NodeList, predicate: &'a Expr) -> Result<NodeList, Error> {
        let size = nodes.len();
        let mut kept = 0;
        for index in 0..size {
            let Some(node) = nodes.get(index).copied() else {
                break;
            };
            let position = index.saturating_add(1);
            let value = self.eval(
                predicate,
                Focus {
                    node,
                    position,
                    size,
                },
            )?;
            #[allow(
                clippy::float_cmp,
                reason = "XPath numeric predicates compare exactly with position()"
            )]
            let keep = match value {
                Val::Num(number) => number == usize_to_f64(position),
                other => other.to_boolean(),
            };
            if keep {
                if let Some(slot) = nodes.get_mut(kept) {
                    *slot = node;
                }
                kept = kept.saturating_add(1);
            }
        }
        nodes.truncate(kept);
        Ok(nodes)
    }

    fn push_matching(
        &mut self,
        out: &mut NodeList,
        node: NodeRef,
        test: Test<'_>,
        principal: NodeKind,
    ) -> Result<(), Error> {
        self.burn()?;
        if self.matches(node, test, principal)? {
            if scxml_types::exceeds_wasm_bound(out.len().saturating_add(1), limits::MAX_NODE_SET) {
                return Err(Error::Limit(Limit::NodeSet));
            }
            out.push(node);
        }
        Ok(())
    }

    fn matches(&self, node: NodeRef, test: Test<'_>, principal: NodeKind) -> Result<bool, Error> {
        if node.namespace_parts().is_some() {
            let kind = self.store.kind(node)?;
            return Ok(match test {
                Test::Node => true,
                Test::Any => kind == principal,
                Test::Name { uri, local, .. } => {
                    kind == principal
                        && uri.is_none()
                        && self.store.namespace_binding(node)?.0.unwrap_or_default() == local
                }
                Test::Text
                | Test::Comment
                | Test::ProcessingInstruction(_)
                | Test::Namespace(_) => false,
            });
        }
        let slot = self.store.node(node.owner())?;
        let kind = slot.node_kind();
        Ok(match test {
            Test::Node => true,
            Test::Text => kind == NodeKind::Text,
            Test::Comment => kind == NodeKind::Comment,
            Test::ProcessingInstruction(target) => {
                kind == NodeKind::ProcessingInstruction
                    && target.is_none_or(|target| {
                        self.store
                            .qname(slot)
                            .is_some_and(|name| name.local_name == target)
                    })
            }
            Test::Any => kind == principal,
            Test::Namespace(uri) => {
                kind == principal
                    && self
                        .tree_name(slot)
                        .is_some_and(|name| name.namespace_uri.as_deref() == Some(uri))
            }
            Test::Name { expanded, .. } => {
                kind == principal
                    && expanded.is_some_and(|expanded| {
                        matches!(slot.kind, Kind::Element | Kind::Attribute)
                            && self
                                .store
                                .name_entry(slot)
                                .is_some_and(|entry| entry.expanded == expanded)
                    })
            }
        })
    }

    /// The name of an element or attribute tree node.
    fn tree_name(&self, slot: &Node) -> Option<&crate::QName> {
        match slot.kind {
            Kind::Element | Kind::Attribute => self.store.qname(slot),
            Kind::Root | Kind::Text | Kind::Comment | Kind::Pi => None,
        }
    }

    /// Pushes `first` and its following or preceding siblings that pass
    /// `test`.
    fn push_siblings(
        &mut self,
        out: &mut NodeList,
        first: u32,
        direction: Siblings,
        test: Test<'_>,
        principal: NodeKind,
    ) -> Result<(), Error> {
        let store = self.store;
        let mut cursor = first;
        while let Some(id) = NodeId::from_link(cursor) {
            self.push_matching(out, NodeRef::node(id), test, principal)?;
            let slot = store.node(id)?;
            cursor = match direction {
                Siblings::Following => slot.next,
                Siblings::Preceding => slot.prev,
            };
        }
        Ok(())
    }

    /// Nodes on `axis` from `node` passing `test`, in axis order.
    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive match keeps all thirteen axes and their orders together"
    )]
    fn axis(&mut self, axis: Axis, node: NodeRef, test: &NodeTest) -> Result<NodeList, Error> {
        let principal = match axis {
            Axis::Attribute => NodeKind::Attribute,
            Axis::Namespace => NodeKind::Namespace,
            _ => NodeKind::Element,
        };
        let test = self.resolve(test);
        let mut out = NodeList::default();
        let store = self.store;
        match axis {
            Axis::SelfNode => self.push_matching(&mut out, node, test, principal)?,
            Axis::Child => {
                if let Some(id) = node.id() {
                    let first = store.node(id)?.first_child;
                    self.push_siblings(&mut out, first, Siblings::Following, test, principal)?;
                }
            }
            Axis::Descendant | Axis::DescendantOrSelf => {
                if axis == Axis::DescendantOrSelf {
                    self.push_matching(&mut out, node, test, principal)?;
                }
                if let Some(id) = node.id() {
                    self.push_descendants(&mut out, id, test, principal)?;
                }
            }
            Axis::Parent => {
                if let Some(parent) = store.parent(node)? {
                    self.push_matching(&mut out, NodeRef::node(parent), test, principal)?;
                }
            }
            Axis::Ancestor | Axis::AncestorOrSelf => {
                if axis == Axis::AncestorOrSelf {
                    self.push_matching(&mut out, node, test, principal)?;
                }
                let mut current = store.parent(node)?;
                while let Some(id) = current {
                    self.push_matching(&mut out, NodeRef::node(id), test, principal)?;
                    current = NodeId::from_link(store.node(id)?.parent);
                }
            }
            Axis::FollowingSibling | Axis::PrecedingSibling => {
                if let Some(id) = node.id() {
                    let slot = store.node(id)?;
                    if slot.kind != Kind::Attribute && slot.has_parent() {
                        if axis == Axis::FollowingSibling {
                            self.push_siblings(
                                &mut out,
                                slot.next,
                                Siblings::Following,
                                test,
                                principal,
                            )?;
                        } else {
                            self.push_siblings(
                                &mut out,
                                slot.prev,
                                Siblings::Preceding,
                                test,
                                principal,
                            )?;
                        }
                    }
                }
            }
            Axis::Following => {
                let start = match self.attribute_like_owner(node)? {
                    Some(element) => {
                        self.push_descendants(&mut out, element, test, principal)?;
                        element
                    }
                    None => node.id().ok_or(Error::InvalidNode)?,
                };
                let mut current = start;
                while let Some(parent) = NodeId::from_link(store.node(current)?.parent) {
                    let mut sibling = store.node(current)?.next;
                    while let Some(id) = NodeId::from_link(sibling) {
                        self.push_matching(&mut out, NodeRef::node(id), test, principal)?;
                        self.push_descendants(&mut out, id, test, principal)?;
                        sibling = store.node(id)?.next;
                    }
                    current = parent;
                }
            }
            Axis::Preceding => {
                let mut current = match self.attribute_like_owner(node)? {
                    Some(element) => element,
                    None => node.id().ok_or(Error::InvalidNode)?,
                };
                while let Some(parent) = NodeId::from_link(store.node(current)?.parent) {
                    let mut sibling = store.node(current)?.prev;
                    while let Some(id) = NodeId::from_link(sibling) {
                        let mut subtree = NodeList::one(NodeRef::node(id));
                        self.push_descendants(&mut subtree, id, Test::Node, principal)?;
                        for member in subtree.iter().rev().copied() {
                            self.push_matching(&mut out, member, test, principal)?;
                        }
                        sibling = store.node(id)?.prev;
                    }
                    current = parent;
                }
            }
            Axis::Attribute => {
                if let Some(id) = node.id() {
                    let first = store.node(id)?.first_attribute;
                    self.push_siblings(&mut out, first, Siblings::Following, test, principal)?;
                }
            }
            Axis::Namespace => {
                if let Some(id) = node.id()
                    && store.node(id)?.kind == Kind::Element
                {
                    let count = store.in_scope_namespaces(id)?.len();
                    for index in 0..count {
                        self.push_matching(
                            &mut out,
                            NodeRef::namespace_of(id, index)?,
                            test,
                            principal,
                        )?;
                    }
                }
            }
        }
        Ok(out)
    }

    /// The element owning an attribute or namespace node.
    fn attribute_like_owner(&self, node: NodeRef) -> Result<Option<NodeId>, Error> {
        match self.store.kind(node)? {
            NodeKind::Attribute | NodeKind::Namespace => self.store.parent(node),
            _ => Ok(None),
        }
    }

    /// Descendants of tree node `id` in document order.
    fn push_descendants(
        &mut self,
        out: &mut NodeList,
        id: NodeId,
        test: Test<'_>,
        principal: NodeKind,
    ) -> Result<(), Error> {
        let store = self.store;
        let mut current = NodeId::from_link(store.node(id)?.first_child).map(|child| (child, 1));
        while let Some((node, depth)) = current {
            self.push_matching(out, NodeRef::node(node), test, principal)?;
            current = store.next_in_subtree(id, node, depth)?;
        }
        Ok(())
    }

    fn argument_string(
        &mut self,
        arguments: &'a [Expr],
        index: usize,
        focus: Focus,
    ) -> Result<Cow<'a, str>, Error> {
        match arguments.get(index) {
            Some(argument) => {
                let value = self.eval(argument, focus)?;
                self.owned_string(value)
            }
            None => self.string_value(focus.node),
        }
    }

    fn argument_number(
        &mut self,
        arguments: &'a [Expr],
        index: usize,
        focus: Focus,
    ) -> Result<f64, Error> {
        match arguments.get(index) {
            Some(argument) => {
                let value = self.eval(argument, focus)?;
                self.number(&value)
            }
            None => self.number_value(focus.node),
        }
    }

    /// The node a name function inspects: first of the argument node-set or
    /// the context node.
    fn name_subject(
        &mut self,
        arguments: &'a [Expr],
        focus: Focus,
    ) -> Result<Option<NodeRef>, Error> {
        match arguments.first() {
            Some(argument) => Ok(Self::node_set(self.eval(argument, focus)?)?
                .first()
                .copied()),
            None => Ok(Some(focus.node)),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive match keeps the whole XPath 1.0 core function library together"
    )]
    fn call(
        &mut self,
        function: Function,
        arguments: &'a [Expr],
        focus: Focus,
    ) -> Result<Val<'a>, Error> {
        let argument = |index: usize| arguments.get(index).ok_or(Error::Type);
        Ok(match function {
            Function::Last => Val::Num(usize_to_f64(focus.size)),
            Function::Position => Val::Num(usize_to_f64(focus.position)),
            Function::Count => {
                let set = Self::node_set(self.eval(argument(0)?, focus)?)?;
                Val::Num(usize_to_f64(set.len()))
            }
            Function::Id => {
                // No DTD attribute-type information exists in this data
                // model, so no attribute is of type ID (XPath 1.0 §4.1).
                self.eval(argument(0)?, focus)?;
                Val::Nodes(NodeList::default())
            }
            Function::LocalName | Function::NamespaceUri | Function::Name => {
                let Some(node) = self.name_subject(arguments, focus)? else {
                    return Ok(Val::Str(Cow::Borrowed("")));
                };
                let name = self.store.name(node)?;
                Val::Str(Cow::Owned(match (function, name) {
                    (_, None) => String::new(),
                    (Function::LocalName, Some(name)) => name.local_name,
                    (Function::NamespaceUri, Some(name)) => name.namespace_uri.unwrap_or_default(),
                    (_, Some(name)) => name.lexical(),
                }))
            }
            Function::String => Val::Str(self.argument_string(arguments, 0, focus)?),
            Function::Concat => {
                let mut out = String::new();
                for index in 0..arguments.len() {
                    let part = self.argument_string(arguments, index, focus)?;
                    burn_bytes(&mut self.fuel, part.len())?;
                    push_bounded(&mut out, &part)?;
                }
                Val::Str(Cow::Owned(out))
            }
            Function::StartsWith
            | Function::Contains
            | Function::SubstringBefore
            | Function::SubstringAfter => {
                let text = self.argument_string(arguments, 0, focus)?;
                let pattern = self.argument_string(arguments, 1, focus)?;
                // A linear search over both; the result is a slice of `text`.
                burn_bytes(&mut self.fuel, text.len().saturating_add(pattern.len()))?;
                let pattern = pattern.as_ref();
                match function {
                    Function::StartsWith => Val::Bool(text.starts_with(pattern)),
                    Function::Contains => Val::Bool(text.contains(pattern)),
                    Function::SubstringBefore => Val::Str(Cow::Owned(
                        text.find(pattern)
                            .and_then(|index| text.get(..index))
                            .unwrap_or_default()
                            .into(),
                    )),
                    _ => Val::Str(Cow::Owned(
                        text.find(pattern)
                            .and_then(|index| text.get(index.saturating_add(pattern.len())..))
                            .unwrap_or_default()
                            .into(),
                    )),
                }
            }
            Function::Substring => {
                let text = self.argument_string(arguments, 0, focus)?;
                let start = round(self.argument_number(arguments, 1, focus)?);
                let end = if arguments.len() > 2 {
                    Some(start + round(self.argument_number(arguments, 2, focus)?))
                } else {
                    None
                };
                burn_bytes(&mut self.fuel, text.len())?;
                let mut out = String::new();
                for (index, character) in text.chars().enumerate() {
                    let position = usize_to_f64(index.saturating_add(1));
                    if position >= start && end.is_none_or(|end| position < end) {
                        out.push(character);
                    }
                }
                Val::Str(Cow::Owned(out))
            }
            Function::StringLength => {
                let text = self.argument_string(arguments, 0, focus)?;
                burn_bytes(&mut self.fuel, text.len())?;
                Val::Num(usize_to_f64(text.chars().count()))
            }
            Function::NormalizeSpace => {
                let text = self.argument_string(arguments, 0, focus)?;
                burn_bytes(&mut self.fuel, text.len())?;
                Val::Str(Cow::Owned(
                    text.split(is_xml_space)
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" "),
                ))
            }
            Function::Translate => {
                let text = self.argument_string(arguments, 0, focus)?;
                let from = self.argument_string(arguments, 1, focus)?;
                let to = self.argument_string(arguments, 2, focus)?;
                // Building the map and mapping each input character cost a
                // unit per byte, and collecting `to` a linear pass.
                burn_units(&mut self.fuel, text.len().saturating_add(from.len()))?;
                burn_bytes(&mut self.fuel, to.len())?;
                let to: Vec<char> = to.chars().collect();
                // First occurrence in `from` wins; characters past the end of
                // `to` are removed.
                let mut mapping: BTreeMap<char, Option<char>> = BTreeMap::new();
                for (index, character) in from.chars().enumerate() {
                    mapping
                        .entry(character)
                        .or_insert_with(|| to.get(index).copied());
                }
                let mut out = String::with_capacity(text.len());
                for character in text.chars() {
                    match mapping.get(&character) {
                        Some(Some(replacement)) => out.push(*replacement),
                        Some(None) => {}
                        None => out.push(character),
                    }
                }
                Val::Str(Cow::Owned(out))
            }
            Function::Boolean => Val::Bool(self.eval(argument(0)?, focus)?.to_boolean()),
            Function::Not => Val::Bool(!self.eval(argument(0)?, focus)?.to_boolean()),
            Function::True => Val::Bool(true),
            Function::False => Val::Bool(false),
            Function::Lang => {
                let wanted = self.argument_string(arguments, 0, focus)?;
                burn_bytes(&mut self.fuel, wanted.len())?;
                let wanted = wanted.to_ascii_lowercase();
                let language = self.language(focus.node)?;
                burn_bytes(&mut self.fuel, language.map_or(0, str::len))?;
                Val::Bool(language.is_some_and(|language| {
                    let language = language.to_ascii_lowercase();
                    language == wanted
                        || language
                            .strip_prefix(wanted.as_str())
                            .is_some_and(|rest| rest.starts_with('-'))
                }))
            }
            Function::Number => Val::Num(self.argument_number(arguments, 0, focus)?),
            Function::Sum => {
                let set = Self::node_set(self.eval(argument(0)?, focus)?)?;
                let mut total = 0.0;
                for node in set.iter() {
                    total += self.number_value(*node)?;
                }
                Val::Num(total)
            }
            Function::Floor => Val::Num(floor(self.argument_number(arguments, 0, focus)?)),
            Function::Ceiling => Val::Num(ceiling(self.argument_number(arguments, 0, focus)?)),
            Function::Round => Val::Num(round(self.argument_number(arguments, 0, focus)?)),
        })
    }

    /// Nearest `xml:lang` on the context node's ancestor-or-self elements.
    fn language(&self, node: NodeRef) -> Result<Option<&'a str>, Error> {
        let store = self.store;
        let mut current = match store.kind(node)? {
            NodeKind::Element => node.id(),
            _ => store.parent(node)?,
        };
        while let Some(id) = current {
            let slot = store.node(id)?;
            if slot.kind == Kind::Element {
                let mut cursor = slot.first_attribute;
                while cursor != NIL {
                    let attribute =
                        store.node(NodeId::from_link(cursor).ok_or(Error::InvalidNode)?)?;
                    if let Some(name) = store.qname(attribute)
                        && name.local_name == "lang"
                        && name.namespace_uri.as_deref() == Some(XML_NAMESPACE)
                    {
                        return Ok(Some(store.leaf_text(attribute)));
                    }
                    cursor = attribute.next;
                }
            }
            current = NodeId::from_link(slot.parent);
        }
        Ok(None)
    }
}

/// Swaps operand order: `a op b` == `b mirror(op) a`.
fn mirror(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Less => BinaryOp::Greater,
        BinaryOp::LessEqual => BinaryOp::GreaterEqual,
        BinaryOp::Greater => BinaryOp::Less,
        BinaryOp::GreaterEqual => BinaryOp::LessEqual,
        other => other,
    }
}

fn relational(op: BinaryOp, left: f64, right: f64) -> bool {
    match op {
        BinaryOp::Less => left < right,
        BinaryOp::LessEqual => left <= right,
        BinaryOp::Greater => left > right,
        BinaryOp::GreaterEqual => left >= right,
        _ => false,
    }
}

#[allow(
    clippy::float_cmp,
    reason = "XPath equality is exact IEEE 754 equality"
)]
fn relational_or_equal(op: BinaryOp, left: f64, right: f64) -> bool {
    match op {
        BinaryOp::Equal => left == right,
        BinaryOp::NotEqual => left != right,
        _ => relational(op, left, right),
    }
}

fn min(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::min)
}

fn max(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::max)
}
