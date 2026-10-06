//! Generative hostile-input tests: the parser, evaluator, and XML reader
//! never panic, always fail with typed errors, respect spans and bounds, and
//! serialization is a fixed point.

#![allow(
    clippy::doc_markdown,
    reason = "XPath and XML are specification names, not code identifiers"
)]

mod common;

use common::{SAMPLE, TestEnvironment, document};
use proptest::prelude::*;
use scxml_xpath::{
    AssignMode, AssignValue, Expression, NodeRef, ParseOptions, QName, Store, Value, check_syntax,
    limits, number_to_string, string_to_number,
};

const VOCABULARY: &[&str] = &[
    "/",
    "//",
    ".",
    "..",
    "@",
    "*",
    "(",
    ")",
    "[",
    "]",
    ",",
    "::",
    "|",
    "+",
    "-",
    "=",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    " and ",
    " or ",
    " div ",
    " mod ",
    "a",
    "b",
    "c",
    "doc",
    "q:x",
    "q:*",
    "text()",
    "node()",
    "comment()",
    "processing-instruction('pi')",
    "child",
    "descendant",
    "ancestor",
    "following",
    "preceding",
    "parent",
    "self",
    "attribute",
    "namespace",
    "following-sibling",
    "preceding-sibling",
    "ancestor-or-self",
    "descendant-or-self",
    "$v",
    "$n",
    "'s'",
    "\"t\"",
    "1",
    "2.5",
    ".5",
    "0",
    "count(",
    "last()",
    "position()",
    "string(",
    "concat(",
    "substring(",
    "translate(",
    "normalize-space(",
    "sum(",
    "round(",
    "floor(",
    "not(",
    "true()",
    "false()",
    "lang(",
    "name(",
    "local-name(",
    "number(",
    "boolean(",
    "In(",
    "id(",
    "string-length(",
    "contains(",
];

const MARKUP: &[&str] = &[
    "<a>",
    "</a>",
    "<b x='1'>",
    "</b>",
    "<c/>",
    "text",
    "&amp;",
    "&lt;",
    "&#65;",
    "&bad;",
    "<!--c-->",
    "<?p d?>",
    "<![CDATA[z]]>",
    "<p:q xmlns:p='urn:p'>",
    "</p:q>",
    "<d xmlns=''>",
    "</d>",
    " ",
    "\n",
    "<",
    ">",
    "\"",
    "'",
    "=",
    "<!DOCTYPE",
    "]]>",
    "<e xml:lang='en'/>",
];

fn soup(vocabulary: &'static [&'static str], max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(vocabulary), 0..max).prop_map(|parts| parts.concat())
}

fn environment(store: &Store, context: NodeRef) -> TestEnvironment {
    let mut env = TestEnvironment::default();
    let options = ParseOptions::scxml()
        .with_namespace("q", "urn:p")
        .expect("valid binding");
    let nodes = Expression::parse("//c", &options)
        .and_then(|expression| expression.evaluate(store, context, &mut env, limits::DEFAULT_FUEL))
        .expect("fixture selection evaluates");
    env.variables.insert("v".into(), nodes);
    env.variables.insert("n".into(), Value::Number(2.0));
    env.active = vec!["s".into()];
    env
}

fn check_spans(source: &str) {
    if let Err(error) = check_syntax(source, &ParseOptions::scxml()) {
        assert!(error.start <= error.end, "{source:?}: {error:?}");
        assert!(error.end <= source.len(), "{source:?}: {error:?}");
        assert!(
            source.is_char_boundary(error.start) && source.is_char_boundary(error.end),
            "{source:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_text_never_panics_the_parser(source in "\\PC{0,64}") {
        check_spans(&source);
        let _ = Expression::parse(&source, &ParseOptions::scxml());
    }

    #[test]
    fn token_soup_parses_and_evaluates_within_bounds(source in soup(VOCABULARY, 24)) {
        check_spans(&source);
        let options = ParseOptions::scxml().with_namespace("q", "urn:p").expect("valid binding");
        if let Ok(expression) = Expression::parse(&source, &options) {
            let mut store = Store::new();
            let context = document(&mut store, SAMPLE);
            let mut env = environment(&store, context);
            // Any typed result is acceptable; the property is "no panic, bounded".
            if let Ok(value) = expression.evaluate(&store, context, &mut env, 20_000) {
                let text = value.to_string(&store).expect("string conversion is total within bounds");
                prop_assert!(text.len() <= limits::MAX_STRING_BYTES);
                if let Value::NodeSet(nodes) = value {
                    prop_assert!(!scxml_types::exceeds_wasm_bound(nodes.len(), limits::MAX_NODE_SET));
                    let rebuilt = scxml_xpath::NodeSet::from_nodes(&store, nodes.nodes().to_vec())
                        .expect("evaluated nodes are valid");
                    prop_assert_eq!(rebuilt, nodes, "node-sets are document-ordered and unique");
                }
            }
        }
    }

    #[test]
    fn arbitrary_text_never_panics_the_xml_reader(xml in "\\PC{0,96}") {
        let mut store = Store::new();
        let before = store.len();
        if store.parse_fragment(&xml).is_err() {
            prop_assert_eq!(store.len(), before);
        }
    }

    #[test]
    fn markup_soup_round_trips_through_the_serializer(xml in soup(MARKUP, 16)) {
        let mut store = Store::new();
        if let Ok(nodes) = store.parse_fragment(&xml) {
            let mut first = String::new();
            for node in &nodes {
                store.serialize(NodeRef::node(*node), &mut first).expect("parsed trees serialize");
            }
            let reparsed = store.parse_fragment(&first).expect("serializer output reparses");
            let mut second = String::new();
            for node in &reparsed {
                store.serialize(NodeRef::node(*node), &mut second).expect("reparsed trees serialize");
            }
            prop_assert_eq!(first, second);
        }
    }

    #[test]
    fn finite_numbers_round_trip_through_xpath_strings(value in any::<f64>().prop_filter("finite", |v| v.is_finite())) {
        let text = number_to_string(value);
        prop_assert!(!text.contains('e') && !text.contains('E'));
        let parsed = string_to_number(&text);
        if value == 0.0 {
            prop_assert!(parsed == 0.0);
        } else {
            prop_assert_eq!(parsed.to_bits(), value.to_bits());
        }
    }

    /// The evaluator computes `floor()` and `ceiling()` without a math
    /// library; they must match IEEE 754 bit for bit, signed zeros, NaN,
    /// and infinities included.
    #[test]
    fn floor_and_ceiling_match_ieee_754(
        value in prop_oneof![
            any::<f64>(),
            -1.0e16_f64..1.0e16,
            -2.0_f64..2.0,
            Just(-0.0_f64),
            Just(0.0_f64),
            Just(-0.5_f64),
            Just(4_503_599_627_370_495.5_f64),
            Just(-4_503_599_627_370_495.5_f64),
            Just(f64::NAN),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
        ]
    ) {
        let mut store = Store::new();
        let context = document(&mut store, "<a/>");
        let mut env = TestEnvironment::default();
        env.variables.insert("v".into(), Value::Number(value));
        for (function, expected) in [("floor", value.floor()), ("ceiling", value.ceil())] {
            let source = format!("{function}($v)");
            let result = Expression::parse(&source, &ParseOptions::scxml())
                .and_then(|expression| expression.evaluate(&store, context, &mut env, limits::DEFAULT_FUEL));
            let Ok(Value::Number(actual)) = result else {
                return Err(TestCaseError::fail(format!("{source} did not yield a number")));
            };
            prop_assert!(
                actual.to_bits() == expected.to_bits() || (actual.is_nan() && expected.is_nan()),
                "{source} of {value:e}: {actual:e} != {expected:e}"
            );
        }
    }

    #[test]
    fn random_assign_sequences_keep_the_tree_valid(
        operations in prop::collection::vec((0_usize..8, 0_usize..6, 0_usize..6), 0..24)
    ) {
        let mut store = Store::new();
        let context = document(&mut store, "<dm><d><x>1</x><y a='v'>2</y></d><e>t</e><f/></dm>");
        let options = ParseOptions::scxml();
        for (mode, target, source) in operations {
            let mut env = TestEnvironment::default();
            let all = match Expression::parse("//node() | //@*", &options)
                .and_then(|expression| expression.evaluate(&store, context, &mut env, limits::DEFAULT_FUEL))
            {
                Ok(Value::NodeSet(nodes)) => nodes.nodes().to_vec(),
                // Self-copying assigns grow the tree geometrically; the
                // evaluator must then fail closed on its bounds.
                Err(scxml_xpath::Error::Limit(_)) => break,
                other => panic!("selection must succeed or hit a bound: {other:?}"),
            };
            let Some(target) = all.get(target % all.len().max(1)).and_then(|node| node.id()) else {
                continue;
            };
            let value = all.get(source % all.len().max(1)).copied().into_iter().collect::<Vec<_>>();
            let mode = match mode {
                0 => AssignMode::ReplaceChildren,
                1 => AssignMode::FirstChild,
                2 => AssignMode::LastChild,
                3 => AssignMode::PreviousSibling,
                4 => AssignMode::NextSibling,
                5 => AssignMode::Replace,
                6 => AssignMode::Delete,
                _ => AssignMode::AddAttribute(QName::local("k").expect("valid name")),
            };
            let before = {
                let mut out = String::new();
                store.serialize(context, &mut out).expect("tree serializes");
                (store.len(), out)
            };
            if store.assign(target, mode, AssignValue::Nodes(&value)).is_err() {
                let mut out = String::new();
                store.serialize(context, &mut out).expect("tree serializes");
                prop_assert_eq!((store.len(), out), before, "failed assigns roll back");
            }
            let mut out = String::new();
            store.serialize(context, &mut out).expect("tree stays serializable");
            prop_assert!(store.parse_fragment(&out).is_ok(), "tree stays well-formed");
        }
    }
}
