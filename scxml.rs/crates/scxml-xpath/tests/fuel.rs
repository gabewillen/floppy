//! Fuel is charged in proportion to string work.
//!
//! String functions, comparisons, and conversions are charged per
//! `STRING_BYTES_PER_FUEL` bytes of operand, so evaluation work is bounded
//! by the fuel budget whatever the operands' length. Each case below processes a large string with a budget that covers the
//! same expression over a short string many times over: it must run out of
//! fuel, which is independent of wall-clock speed.

mod common;

use common::{TestEnvironment, document};
use scxml_xpath::{Error, Expression, Limit, ParseOptions, Store, Value, limits};

/// Bytes of the large text node: 16 KiB of string work.
const LARGE: usize = 16 * 1024;
/// A budget that the short-text control never approaches but that the
/// large text exceeds for a single linear pass at the documented rate.
const BUDGET: u64 = (LARGE / limits::STRING_BYTES_PER_FUEL / 2) as u64;

fn evaluate(xml: &str, source: &str, fuel: u64) -> Result<Value, Error> {
    let mut store = Store::new();
    let context = document(&mut store, xml);
    let mut env = TestEnvironment::default();
    env.variables
        .insert("big".into(), Value::String("a".repeat(LARGE)));
    env.variables
        .insert("small".into(), Value::String("a".into()));
    Expression::parse(source, &ParseOptions::scxml())?.evaluate(&store, context, &mut env, fuel)
}

fn tree(text: &str) -> String {
    format!("<r><d>{text}</d><e>{text}</e><m><t>{text}</t><t>{text}</t></m></r>")
}

/// Every string-processing form, over `/r/d` (or `$v`, the variable twin).
const FORMS: &[&str] = &[
    "string(/r/d)",
    "string-length(/r/d)",
    "string-length(/r/m)",
    "concat(/r/d, 'x')",
    "contains(/r/d, 'b')",
    "starts-with(/r/d, 'b')",
    "substring-before(/r/d, 'b')",
    "substring-after(/r/d, 'b')",
    "substring(/r/d, 2)",
    "normalize-space(/r/d)",
    "translate(/r/d, 'b', 'c')",
    "number(/r/d)",
    "sum(/r/d)",
    "/r/d = 'b'",
    "/r/d = /r/e",
    "/r/d != /r/e",
    "/r/d < /r/e",
    "/r/d = 1",
    "boolean(/r/m[. = 'b'])",
    "lang(/r/d)",
];

#[test]
fn string_work_is_charged_per_byte() {
    let small = tree("a");
    let large = tree(&"a".repeat(LARGE));
    for form in FORMS {
        assert!(
            evaluate(&small, form, BUDGET).is_ok(),
            "{form}: the short-text control fits the budget"
        );
        assert_eq!(
            evaluate(&large, form, BUDGET),
            Err(Error::Limit(Limit::Fuel)),
            "{form}: a {LARGE}-byte operand must cost more than {BUDGET} units"
        );
    }
}

#[test]
fn string_values_from_variables_and_literals_are_charged_per_byte() {
    let xml = tree("a");
    for (small, large) in [
        ("string($small)", "string($big)"),
        ("string-length($small)", "string-length($big)"),
        ("translate($small, 'a', 'b')", "translate($big, 'a', 'b')"),
        ("concat($small, $small)", "concat($big, $small)"),
        ("$small = 'b'", "$big = 'b'"),
    ] {
        assert!(evaluate(&xml, small, BUDGET).is_ok(), "{small}");
        assert_eq!(
            evaluate(&xml, large, BUDGET),
            Err(Error::Limit(Limit::Fuel)),
            "{large}"
        );
    }
    let literal = format!("string-length('{}')", "a".repeat(12_000));
    assert_eq!(
        evaluate(&xml, &literal, BUDGET),
        Err(Error::Limit(Limit::Fuel))
    );
}

/// Bytes of the `/r/d` text in the amplification fixture.
const AMPLIFIED_TEXT: usize = 65_000;
/// Copies of `/r/d` each predicate evaluation concatenates.
const AMPLIFIED_COPIES: usize = 16;
/// `/r/y/x` nodes the amplification predicate runs over.
const AMPLIFIED_NODES: usize = 14_000;

/// `count(/r/y/x[string-length(translate(concat(/r/d, ...), ...))=0])`
/// over a tree whose `/r/d` holds `text` bytes and `/r/y` holds `nodes` `x`
/// elements.
fn amplification(text: usize, nodes: usize) -> (String, String) {
    let xml = format!(
        "<r><d>{}</d><y>{}</y></r>",
        "a".repeat(text),
        "<x/>".repeat(nodes)
    );
    let from: String = ('\u{4E00}'..).take(4_000).collect();
    let parts = ["/r/d"; AMPLIFIED_COPIES].join(",");
    let source =
        format!("count(/r/y/x[string-length(translate(concat({parts}),'{from}a','b'))=0])");
    (xml, source)
}

/// A translate/concat predicate that amplifies string work per node is cut
/// off by `DEFAULT_FUEL` within its first node: `translate` is charged one
/// unit per input byte, and one node's input alone is more than
/// `DEFAULT_FUEL` bytes, so the work done is bounded by the fuel budget, not
/// by the fixture's node count.
#[test]
fn string_amplification_exhausts_default_fuel_within_the_first_node() {
    let translate_charge_per_node = u64::try_from(AMPLIFIED_TEXT * AMPLIFIED_COPIES)
        .expect("the per-node translate charge fits u64");
    assert!(translate_charge_per_node > limits::DEFAULT_FUEL);
    let (one_node, source) = amplification(AMPLIFIED_TEXT, 1);
    assert_eq!(
        evaluate(&one_node, &source, limits::DEFAULT_FUEL),
        Err(Error::Limit(Limit::Fuel)),
        "one node's predicate exceeds DEFAULT_FUEL"
    );
    let (control, source) = amplification(1, 1);
    assert_eq!(
        evaluate(&control, &source, limits::DEFAULT_FUEL),
        Ok(Value::Number(1.0)),
        "the same predicate over a one-byte text fits DEFAULT_FUEL"
    );
    let (xml, source) = amplification(AMPLIFIED_TEXT, AMPLIFIED_NODES);
    assert_eq!(
        evaluate(&xml, &source, limits::DEFAULT_FUEL),
        Err(Error::Limit(Limit::Fuel))
    );
}

#[test]
fn ordinary_expressions_keep_their_cost() {
    // Short SCXML-style guards stay far inside a small budget.
    let xml = "<data><cart><item price='3'>apple</item><item price='4'>pear</item></cart></data>";
    for source in [
        "sum(/data/cart/item/@price) = 7",
        "concat(/data/cart/item[1], '-', /data/cart/item[2]) = 'apple-pear'",
        "contains(normalize-space(/data/cart), 'pear')",
        "translate(/data/cart/item[1], 'ap', 'AP') = 'APPle'",
        "substring-after(string(/data/cart/item[2]), 'p') = 'ear'",
    ] {
        assert_eq!(
            evaluate(xml, source, 200),
            Ok(Value::Boolean(true)),
            "{source}"
        );
    }
}
