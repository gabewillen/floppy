//! Shared fixtures for the XPath integration tests.

#![allow(
    dead_code,
    clippy::doc_markdown,
    reason = "each test binary uses a different subset of the helpers; XPath is a specification name"
)]

use std::collections::BTreeMap;

use scxml_xpath::{Environment, Error, Expression, NodeRef, ParseOptions, Store, Value, limits};

/// Variables plus the SCXML `In()` extension over a fixed configuration.
#[derive(Default)]
pub struct TestEnvironment {
    pub variables: BTreeMap<String, Value>,
    pub active: Vec<String>,
    pub in_calls: usize,
}

impl Environment for TestEnvironment {
    fn variable(&self, name: &str) -> Option<Value> {
        self.variables.get(name).cloned()
    }

    fn call_extension(
        &mut self,
        name: &str,
        arguments: &[Value],
        store: &Store,
    ) -> Result<Value, Error> {
        if name != "In" {
            return Err(Error::Extension);
        }
        self.in_calls += 1;
        let [argument] = arguments else {
            return Err(Error::Extension);
        };
        let state = argument.to_string(store)?;
        Ok(Value::Boolean(self.active.contains(&state)))
    }
}

/// A document node whose children are the parsed `xml` fragment.
pub fn document(store: &mut Store, xml: &str) -> NodeRef {
    let root = store.create_root().expect("fixture root must allocate");
    for node in store.parse_fragment(xml).expect("fixture XML must parse") {
        store
            .append_child(root, node)
            .expect("fixture node must attach");
    }
    NodeRef::node(root)
}

pub const SAMPLE: &str = r#"<doc xmlns:p="urn:p" xml:lang="en-US"><a id="1">one</a><b><c>2</c><c>3</c><c>x</c></b><!--note--><?pi data?><p:x p:attr="v">ns</p:x><e/></doc>"#;

pub fn eval_in(
    store: &Store,
    context: NodeRef,
    env: &mut TestEnvironment,
    source: &str,
) -> Result<Value, Error> {
    let options = ParseOptions::scxml().with_namespace("q", "urn:p")?;
    Expression::parse(source, &options)?.evaluate(store, context, env, limits::DEFAULT_FUEL)
}

pub fn eval(source: &str) -> Result<Value, Error> {
    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    eval_in(&store, context, &mut env, source)
}

pub fn string(source: &str) -> String {
    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    eval_in(&store, context, &mut env, source)
        .and_then(|value| value.to_string(&store))
        .unwrap_or_else(|error| panic!("{source} must evaluate: {error:?}"))
}

pub fn number(source: &str) -> f64 {
    match eval(source) {
        Ok(Value::Number(value)) => value,
        other => panic!("{source} must evaluate to a number, got {other:?}"),
    }
}

pub fn boolean(source: &str) -> bool {
    match eval(source) {
        Ok(Value::Boolean(value)) => value,
        other => panic!("{source} must evaluate to a boolean, got {other:?}"),
    }
}

pub fn count(source: &str) -> usize {
    match eval(source) {
        Ok(Value::NodeSet(nodes)) => nodes.len(),
        other => panic!("{source} must evaluate to a node-set, got {other:?}"),
    }
}

/// String-values of a node-set result, in document order.
pub fn strings(source: &str) -> Vec<String> {
    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    match eval_in(&store, context, &mut env, source) {
        Ok(Value::NodeSet(nodes)) => nodes
            .nodes()
            .iter()
            .map(|node| store.string_value(*node).expect("string-value must read"))
            .collect(),
        other => panic!("{source} must evaluate to a node-set, got {other:?}"),
    }
}
