//! XPath 1.0 language conformance: conversions, operators, comparisons,
//! functions, axes, parsing, variables, extensions, and bounds.

#![allow(
    clippy::doc_markdown,
    clippy::float_cmp,
    reason = "XPath and XML are specification names; exact IEEE 754 results are the tested contract"
)]

mod common;

use std::fmt::Write as _;

use common::{
    SAMPLE, TestEnvironment, boolean, count, document, eval, eval_in, number, string, strings,
};
use scxml_xpath::{
    Error, Expression, Limit, NodeSet, ParseOptions, Store, Value, check_syntax, limits,
    number_to_string, string_to_number,
};

#[test]
fn number_to_string_follows_xpath_rules() {
    assert_eq!(number_to_string(f64::NAN), "NaN");
    assert_eq!(number_to_string(f64::INFINITY), "Infinity");
    assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
    assert_eq!(number_to_string(0.0), "0");
    assert_eq!(number_to_string(-0.0), "0");
    assert_eq!(number_to_string(3.0), "3");
    assert_eq!(number_to_string(-42.0), "-42");
    assert_eq!(number_to_string(0.5), "0.5");
    assert_eq!(number_to_string(-0.25), "-0.25");
    assert_eq!(number_to_string(1e21), "1000000000000000000000");
    assert_eq!(number_to_string(1e-7), "0.0000001");
    assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
}

#[test]
fn string_to_number_accepts_only_the_xpath_grammar() {
    assert!((string_to_number(" 12 ") - 12.0).abs() < f64::EPSILON);
    assert!((string_to_number("-1.5") + 1.5).abs() < f64::EPSILON);
    assert!((string_to_number(".5") - 0.5).abs() < f64::EPSILON);
    assert!((string_to_number("7.") - 7.0).abs() < f64::EPSILON);
    assert!((string_to_number("\n\t3\r") - 3.0).abs() < f64::EPSILON);
    for invalid in [
        "", " ", "+1", "1e3", "Infinity", "NaN", "-", ".", "1.2.3", "0x10", "- 1", "1 2", "١",
    ] {
        assert!(
            string_to_number(invalid).is_nan(),
            "{invalid:?} must be NaN"
        );
    }
}

#[test]
fn arithmetic_follows_ieee_and_xpath_precedence() {
    assert!((number("1 + 2 * 3") - 7.0).abs() < f64::EPSILON);
    assert!((number("(1 + 2) * 3") - 9.0).abs() < f64::EPSILON);
    assert!((number("10 div 4") - 2.5).abs() < f64::EPSILON);
    assert!((number("5 mod 2") - 1.0).abs() < f64::EPSILON);
    assert!((number("5 mod -2") - 1.0).abs() < f64::EPSILON);
    assert!((number("-5 mod 2") + 1.0).abs() < f64::EPSILON);
    assert!((number("1 - -1") - 2.0).abs() < f64::EPSILON);
    assert!((number("--1") - 1.0).abs() < f64::EPSILON);
    assert!((number("3 - 2 - 1")).abs() < f64::EPSILON);
    assert_eq!(number("1 div 0"), f64::INFINITY);
    assert_eq!(number("-1 div 0"), f64::NEG_INFINITY);
    assert!(number("0 div 0").is_nan());
    assert!(number("'a' + 1").is_nan());
    assert_eq!(string("-0"), "0");
    assert_eq!(string("1 div 0"), "Infinity");
    assert_eq!(string("0 div 0"), "NaN");
    assert!((number("//c[1] + 1") - 3.0).abs() < f64::EPSILON);
}

#[test]
fn operator_names_are_disambiguated_by_context() {
    let mut store = Store::new();
    let context = document(
        &mut store,
        "<div><div>6</div><mod>4</mod><and>1</and><or/></div>",
    );
    let mut env = TestEnvironment::default();
    let value =
        eval_in(&store, context, &mut env, "div/div div div/mod").expect("operators must parse");
    assert_eq!(value, Value::Number(1.5));
    let value = eval_in(
        &store,
        context,
        &mut env,
        "count(div/and) = 1 and count(div/or) = 1",
    )
    .expect("names must parse");
    assert_eq!(value, Value::Boolean(true));
    let value = eval_in(&store, context, &mut env, "div/div * 2").expect("multiply must parse");
    assert_eq!(value, Value::Number(12.0));
    let value = eval_in(&store, context, &mut env, "count(div/*)").expect("wildcard must parse");
    assert_eq!(value, Value::Number(4.0));
}

#[test]
fn equality_and_relational_comparisons_follow_section_3_4() {
    // node-set vs number: existential over string-values converted to numbers
    assert!(boolean("//c = 3"));
    assert!(boolean("//c != 3"));
    assert!(!boolean("//c = 4"));
    assert!(boolean("//c > 2"));
    assert!(!boolean("//c > 3"));
    assert!(boolean("2 < //c"));
    assert!(boolean("4 > //c"));
    // node-set vs string
    assert!(boolean("//c = 'x'"));
    assert!(boolean("'x' = //c"));
    assert!(boolean("//a = 'one'"));
    // node-set vs boolean
    assert!(boolean("//c = true()"));
    assert!(boolean("//missing = false()"));
    assert!(boolean("true() = //c"));
    // node-set vs node-set
    assert!(boolean("//c = //b/c"));
    assert!(!boolean("//a = //c"));
    assert!(boolean("//a != //c"));
    assert!(!boolean("//missing = //c"));
    assert!(!boolean("//missing != //c"));
    assert!(boolean("//c < //c"));
    assert!(!boolean("//a < //c"));
    // empty node-set comparisons are all false
    assert!(!boolean("//missing = 0"));
    assert!(!boolean("//missing != 0"));
    assert!(!boolean("//missing < 1"));
    // scalars: boolean wins, then number, then string
    assert!(boolean("1 = '1'"));
    assert!(boolean("'1.0' = 1"));
    assert!(!boolean("'1.0' = '1'"));
    assert!(boolean("true() = 'x'"));
    assert!(boolean("false() = ''"));
    assert!(boolean("true() = 2"));
    assert!(boolean("true() > false()"));
    assert!(!boolean("'2' > '10'"));
    assert!(!boolean("0 div 0 = 0 div 0"));
    assert!(boolean("0 div 0 != 0 div 0"));
    assert!(!boolean("'abc' < 'abd'"));
}

#[test]
fn boolean_operators_short_circuit() {
    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    assert_eq!(
        eval_in(&store, context, &mut env, "true() or $undefined").expect("or must short-circuit"),
        Value::Boolean(true)
    );
    assert_eq!(
        eval_in(&store, context, &mut env, "false() and $undefined")
            .expect("and must short-circuit"),
        Value::Boolean(false)
    );
    assert_eq!(
        eval_in(&store, context, &mut env, "true() and $undefined"),
        Err(Error::UndefinedVariable)
    );
}

#[test]
fn string_functions_match_the_specification() {
    assert_eq!(string("concat('a', 1, true(), //a)"), "a1trueone");
    assert!(boolean("starts-with('abc', 'ab')"));
    assert!(boolean("starts-with('abc', '')"));
    assert!(boolean("contains('abc', 'bc')"));
    assert!(!boolean("contains('abc', 'x')"));
    assert_eq!(string("substring-before('1999/04/01', '/')"), "1999");
    assert_eq!(string("substring-after('1999/04/01', '/')"), "04/01");
    assert_eq!(string("substring-after('abc', '')"), "abc");
    assert_eq!(string("substring-before('abc', '')"), "");
    assert_eq!(string("substring-before('abc', 'x')"), "");
    // XPath 1.0 §4.2 substring examples
    assert_eq!(string("substring('12345', 2, 3)"), "234");
    assert_eq!(string("substring('12345', 2)"), "2345");
    assert_eq!(string("substring('12345', 1.5, 2.6)"), "234");
    assert_eq!(string("substring('12345', 0, 3)"), "12");
    assert_eq!(string("substring('12345', 0 div 0, 3)"), "");
    assert_eq!(string("substring('12345', 1, 0 div 0)"), "");
    assert_eq!(string("substring('12345', -42, 1 div 0)"), "12345");
    assert_eq!(string("substring('12345', -1 div 0, 1 div 0)"), "");
    assert_eq!(string("substring('héllo', 2, 2)"), "él");
    assert!((number("string-length('héllo')") - 5.0).abs() < f64::EPSILON);
    assert!((number("string-length(//a)") - 3.0).abs() < f64::EPSILON);
    assert_eq!(string("normalize-space('  a \t b\n c  ')"), "a b c");
    assert_eq!(string("translate('bar', 'abc', 'ABC')"), "BAr");
    assert_eq!(string("translate('--aaa--', 'abc-', 'ABC')"), "AAA");
    assert_eq!(string("translate('aab', 'aa', 'xy')"), "xxb");
    assert_eq!(string("string(1.50)"), "1.5");
    assert_eq!(string("string(true())"), "true");
    assert_eq!(string("string(//missing)"), "");
    assert_eq!(string("string(//c)"), "2");
}

#[test]
fn context_functions_default_to_the_context_node() {
    assert_eq!(strings("//c[string() = '3']"), vec!["3"]);
    assert_eq!(strings("//a[string-length() = 3]"), vec!["one"]);
    assert_eq!(strings("//c[number() = 2]"), vec!["2"]);
    assert_eq!(strings("//c[normalize-space() = 'x']"), vec!["x"]);
}

#[test]
fn number_and_boolean_functions_match_the_specification() {
    assert!((number("number('  4 ')") - 4.0).abs() < f64::EPSILON);
    assert!(number("number('4a')").is_nan());
    assert!((number("number(true())") - 1.0).abs() < f64::EPSILON);
    assert!((number("sum(//c[. != 'x'])") - 5.0).abs() < f64::EPSILON);
    assert!(number("sum(//c)").is_nan());
    assert!((number("sum(//missing)")).abs() < f64::EPSILON);
    assert!((number("floor(-1.5)") + 2.0).abs() < f64::EPSILON);
    assert!((number("ceiling(-1.5)") + 1.0).abs() < f64::EPSILON);
    assert!((number("round(2.5)") - 3.0).abs() < f64::EPSILON);
    assert!((number("round(-2.5)") + 2.0).abs() < f64::EPSILON);
    assert!((number("round(1.4999)") - 1.0).abs() < f64::EPSILON);
    assert_eq!(string("round(-0.4)"), "0");
    assert!(number("round(-0.4)").is_sign_negative());
    assert!(number("round(0 div 0)").is_nan());
    assert_eq!(number("round(1 div 0)"), f64::INFINITY);
    assert!(boolean("boolean('a')"));
    assert!(!boolean("boolean('')"));
    assert!(!boolean("boolean(0)"));
    assert!(!boolean("boolean(0 div 0)"));
    assert!(boolean("boolean(-1)"));
    assert!(boolean("boolean(//a)"));
    assert!(!boolean("boolean(//missing)"));
    assert!(boolean("not(false())"));
    assert!(boolean("true()"));
    assert!(!boolean("false()"));
}

#[test]
fn node_set_functions_match_the_specification() {
    assert!((number("count(//c)") - 3.0).abs() < f64::EPSILON);
    assert_eq!(strings("//c[last()]"), vec!["x"]);
    assert_eq!(strings("//c[position() = 2]"), vec!["3"]);
    assert_eq!(strings("//c[2]"), vec!["3"]);
    assert_eq!(strings("//c[position() > 1][1]"), vec!["3"]);
    assert_eq!(string("local-name(//q:x)"), "x");
    assert_eq!(string("name(//q:x)"), "p:x");
    assert_eq!(string("namespace-uri(//q:x)"), "urn:p");
    assert_eq!(string("name(//q:x/@q:attr)"), "p:attr");
    assert_eq!(string("local-name(//processing-instruction())"), "pi");
    assert_eq!(string("name(//comment())"), "");
    assert_eq!(string("local-name(//missing)"), "");
    assert_eq!(string("name()"), "");
    assert_eq!(string("local-name(/doc)"), "doc");
    assert_eq!(count("id('1')"), 0);
    assert!(boolean("boolean(//a[lang('en')])"));
    assert!(boolean("boolean(//a[lang('EN-us')])"));
    assert!(!boolean("boolean(//a[lang('fr')])"));
    assert!(!boolean("boolean(//a[lang('en-GB')])"));
}

#[test]
fn every_axis_selects_in_the_specified_order() {
    assert_eq!(strings("/doc/b/child::c"), vec!["2", "3", "x"]);
    assert_eq!(count("/doc/descendant::*"), 7);
    assert_eq!(count("/doc/descendant-or-self::*"), 8);
    assert_eq!(strings("//c[2]/parent::*"), vec!["23x"]);
    assert_eq!(count("//c[1]/ancestor::*"), 2);
    assert_eq!(count("//c[1]/ancestor-or-self::*"), 3);
    assert_eq!(strings("//c[1]/following-sibling::c"), vec!["3", "x"]);
    assert_eq!(strings("//c[3]/preceding-sibling::c"), vec!["2", "3"]);
    assert_eq!(strings("//c[3]/preceding-sibling::c[1]"), vec!["3"]);
    assert_eq!(strings("//c[1]/following::*"), vec!["3", "x", "ns", ""]);
    assert_eq!(strings("//e/preceding::c"), vec!["2", "3", "x"]);
    assert_eq!(strings("//e/preceding::c[1]"), vec!["x"]);
    assert_eq!(strings("//e/preceding::*[1]"), vec!["ns"]);
    assert_eq!(strings("//a/attribute::id"), vec!["1"]);
    assert_eq!(strings("//a/@*"), vec!["1"]);
    assert_eq!(strings("//a/self::a"), vec!["one"]);
    assert_eq!(count("//a/self::b"), 0);
    assert_eq!(strings("//q:x/namespace::*[name() = 'p']"), vec!["urn:p"]);
    assert_eq!(
        strings("//q:x/namespace::xml"),
        vec!["http://www.w3.org/XML/1998/namespace"]
    );
    assert_eq!(count("//a/namespace::*"), 2);
    // attribute and namespace nodes have no children or siblings
    assert_eq!(count("//a/@id/node()"), 0);
    assert_eq!(count("//a/@id/following-sibling::node()"), 0);
    // following of an attribute includes its element's descendants
    assert_eq!(strings("//a/@id/following::text()[1]"), vec!["one"]);
    assert_eq!(count("//a/@id/preceding::node()"), 0);
    assert_eq!(strings("//a/@id/.."), vec!["one"]);
    assert_eq!(count("//a/@id/ancestor::*"), 2);
}

#[test]
fn node_tests_and_abbreviations() {
    assert_eq!(count("//text()"), 5);
    assert_eq!(count("//comment()"), 1);
    assert_eq!(count("//processing-instruction()"), 1);
    assert_eq!(count("//processing-instruction('pi')"), 1);
    assert_eq!(count("//processing-instruction('other')"), 0);
    assert_eq!(count("/doc/node()"), 6);
    assert_eq!(count("//q:*"), 1);
    assert_eq!(count("//x"), 0);
    assert_eq!(count("//@q:*"), 1);
    assert_eq!(count("/"), 1);
    assert_eq!(count("/.."), 0);
    assert_eq!(count("//b//c"), 3);
    assert_eq!(count("//b/c/."), 3);
    assert_eq!(count("//b/c/.."), 1);
    assert_eq!(count("//c | //a | //c"), 4);
    assert_eq!(strings("(//c | //a)[1]"), vec!["one"]);
    assert_eq!(strings("(//c)[last()]"), vec!["x"]);
    assert_eq!(strings("//b/c[. = 3]/preceding-sibling::*"), vec!["2"]);
}

#[test]
fn reverse_axis_predicates_use_proximity_positions_and_results_are_document_ordered() {
    assert_eq!(strings("//c[3]/preceding-sibling::*[1]"), vec!["3"]);
    assert_eq!(strings("//c[3]/preceding-sibling::*[last()]"), vec!["2"]);
    assert_eq!(strings("//c[3]/preceding-sibling::*"), vec!["2", "3"]);
    assert_eq!(count("//c[1]/ancestor::*[1]/self::b"), 1);
}

#[test]
fn variables_and_extension_functions() {
    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    let nodes = match eval_in(&store, context, &mut env, "//c").expect("//c must evaluate") {
        Value::NodeSet(nodes) => nodes,
        other => panic!("expected node-set, got {other:?}"),
    };
    env.variables.insert("cs".into(), Value::NodeSet(nodes));
    env.variables.insert("n".into(), Value::Number(2.0));
    env.active = vec!["s1".into()];
    assert_eq!(
        eval_in(&store, context, &mut env, "$cs[$n]").and_then(|v| v.to_string(&store)),
        Ok("3".into())
    );
    assert_eq!(
        eval_in(&store, context, &mut env, "count($cs/..)"),
        Ok(Value::Number(1.0))
    );
    assert_eq!(
        eval_in(&store, context, &mut env, "In('s1') and not(In('s2'))"),
        Ok(Value::Boolean(true))
    );
    assert_eq!(env.in_calls, 2);
    assert_eq!(
        eval_in(&store, context, &mut env, "$missing"),
        Err(Error::UndefinedVariable)
    );
    assert_eq!(eval_in(&store, context, &mut env, "$n/a"), Err(Error::Type));
    assert_eq!(
        eval_in(&store, context, &mut env, "$n | $cs"),
        Err(Error::Type)
    );
    assert_eq!(
        eval_in(&store, context, &mut env, "count(1)"),
        Err(Error::Type)
    );
    assert_eq!(
        eval_in(&store, context, &mut env, "$n[1]"),
        Err(Error::Type)
    );
    // In() is only available when declared
    assert!(matches!(
        Expression::parse("In('s1')", &ParseOptions::new()),
        Err(Error::Syntax(_))
    ));
}

#[test]
fn syntax_errors_carry_exact_spans() {
    let check = |source: &str| check_syntax(source, &ParseOptions::scxml());
    assert!(check("$Var1/text() = 1").is_ok());
    assert!(check("In('s1') or //a[@b='c']/text()").is_ok());
    let error = check("1 +").expect_err("dangling operator must fail");
    assert_eq!((error.start, error.end), (3, 3));
    let error = check("foo(1)").expect_err("unknown function must fail");
    assert_eq!((error.start, error.end), (0, 3));
    assert_eq!(error.message, "unknown function");
    let error = check("count()").expect_err("arity must fail");
    assert_eq!(error.message, "wrong number of function arguments");
    let error = check("//z:a").expect_err("undeclared prefix must fail");
    assert_eq!((error.start, error.end), (2, 5));
    let error = check("'unterminated").expect_err("unterminated literal must fail");
    assert_eq!(error.start, 0);
    for invalid in [
        "",
        "(",
        ")",
        "==!*",
        "++var1==2",
        "a b",
        "1 2",
        "//",
        "@",
        "a[",
        "a]",
        "$",
        "$1",
        "child::",
        "bogus::a",
        "a/",
        "processing-instruction(1)",
        "text(1)",
        "1 !2",
        "a:*:b",
        "f(",
        "concat('a')",
        "not()",
        "()",
        "[1]",
        "#",
    ] {
        let result = check(invalid);
        let error = result.expect_err(invalid);
        assert!(
            error.start <= error.end && error.end <= invalid.len(),
            "{invalid}: span {error:?}"
        );
        assert!(invalid.is_char_boundary(error.start) && invalid.is_char_boundary(error.end));
    }
    assert!(check("é").is_ok(), "non-ASCII NCName is a valid name test");
}

#[test]
fn location_expressions_are_classified_statically() {
    let options = ParseOptions::scxml();
    for location in [
        "$Var1", "$Var1/a", "/a", "a|b", "id('x')", "In('s')", "(//a)[1]", ".",
    ] {
        assert!(
            Expression::parse(location, &options)
                .expect(location)
                .may_yield_node_set(),
            "{location} may yield a node-set"
        );
    }
    for value in [
        "1",
        "'a'",
        "1 + $a",
        "count(a)",
        "-$a",
        "$a = 1",
        "string(a)",
    ] {
        assert!(
            !Expression::parse(value, &options)
                .expect(value)
                .may_yield_node_set(),
            "{value} never yields a node-set"
        );
    }
}

/// One byte past the wasm expression bound (16 KiB) fails to parse, and the
/// syntax check spans the whole source.
#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_only_an_expression_over_the_byte_bound_is_declined() {
    let options = ParseOptions::scxml();
    let long = "1".repeat(16_385);
    assert_eq!(
        Expression::parse(&long, &options),
        Err(Error::Limit(Limit::ExpressionBytes))
    );
    let error = check_syntax(&long, &options).expect_err("over-bound source must fail");
    assert_eq!((error.start, error.end), (0, long.len()));
}

#[test]
fn parser_and_evaluator_bounds_fail_closed() {
    let options = ParseOptions::scxml();
    // The expression byte bound applies to wasm builds only: one byte past
    // it (16 KiB) parses natively.
    #[cfg(not(target_arch = "wasm32"))]
    assert!(Expression::parse(&"1".repeat(16_385), &options).is_ok());
    let nested = format!(
        "{}1{}",
        "(".repeat(limits::MAX_PARSE_NESTING + 1),
        ")".repeat(limits::MAX_PARSE_NESTING + 1)
    );
    assert_eq!(
        Expression::parse(&nested, &options),
        Err(Error::Limit(Limit::AstDepth))
    );
    let chain = vec!["1"; limits::MAX_AST_DEPTH + 2].join(" + ");
    assert_eq!(
        Expression::parse(&chain, &options),
        Err(Error::Limit(Limit::AstDepth))
    );
    let negations = format!("{}1", "-".repeat(limits::MAX_AST_DEPTH + 2));
    assert_eq!(
        Expression::parse(&negations, &options),
        Err(Error::Limit(Limit::AstDepth))
    );
    let ok_chain = vec!["1"; limits::MAX_AST_DEPTH / 2].join(" + ");
    assert!(Expression::parse(&ok_chain, &options).is_ok());

    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    let expensive =
        Expression::parse("count(//node()[count(//node()) > 0])", &options).expect("must parse");
    assert_eq!(
        expensive.evaluate(&store, context, &mut env, 50),
        Err(Error::Limit(Limit::Fuel))
    );
    assert!(
        expensive
            .evaluate(&store, context, &mut env, limits::DEFAULT_FUEL)
            .is_ok()
    );
}

#[test]
fn string_results_are_bounded() {
    let mut store = Store::new();
    let big = "x".repeat(limits::MAX_STRING_BYTES / 2 + 1);
    let context = document(&mut store, &format!("<a>{big}</a>"));
    let mut env = TestEnvironment::default();
    assert_eq!(
        eval_in(&store, context, &mut env, "concat(/a, /a)"),
        Err(Error::Limit(Limit::StringBytes))
    );
}

#[test]
fn node_set_constructor_orders_and_deduplicates() {
    let mut store = Store::new();
    let context = document(&mut store, SAMPLE);
    let mut env = TestEnvironment::default();
    let Value::NodeSet(cs) = eval_in(&store, context, &mut env, "//c").expect("must evaluate")
    else {
        panic!("expected node-set");
    };
    let mut reversed: Vec<_> = cs.nodes().iter().rev().copied().collect();
    reversed.extend(cs.nodes().iter().copied());
    let rebuilt = NodeSet::from_nodes(&store, reversed).expect("valid nodes must build");
    assert_eq!(rebuilt, cs);
    assert_eq!(rebuilt.first(), cs.first());
    assert!(NodeSet::new().is_empty());
}

#[test]
fn evaluation_is_deterministic() {
    let expressions = [
        "//node()",
        "//c[. > 1]",
        "string(//q:x/namespace::*)",
        "sum(//c[. != 'x']) div 3",
    ];
    for source in expressions {
        let first = format!("{:?}", eval(source));
        for _ in 0..3 {
            assert_eq!(format!("{:?}", eval(source)), first, "{source}");
        }
    }
}

#[test]
fn wide_trees_sort_in_linear_work() {
    let mut store = Store::new();
    let mut xml = String::from("<r>");
    for index in 0..5_000 {
        write!(xml, "<i n='{index}'>{index}</i>").expect("writing to a String cannot fail");
    }
    xml.push_str("</r>");
    let context = document(&mut store, &xml);
    let mut env = TestEnvironment::default();
    let options = ParseOptions::scxml();
    // Multi-input steps and unions over thousands of siblings stay well
    // inside the default fuel (a fan-out-proportional ordering would not).
    let evaluate = |source: &str, env: &mut TestEnvironment| {
        Expression::parse(source, &options)
            .and_then(|expression| expression.evaluate(&store, context, env, 200_000))
    };
    assert_eq!(
        evaluate("count(/r/i/text())", &mut env),
        Ok(Value::Number(5_000.0))
    );
    assert_eq!(
        evaluate("count(/r/i/@n | /r/i)", &mut env),
        Ok(Value::Number(10_000.0))
    );
    let last = evaluate(
        "(/r/i[position() mod 2 = 0] | /r/i[position() mod 2 = 1])[last()]",
        &mut env,
    )
    .and_then(|value| value.to_string(&store));
    assert_eq!(last, Ok("4999".into()));
    let first = evaluate("(/r/i[. > 4990] | /r/i[. < 3])[1]/@n", &mut env)
        .and_then(|value| value.to_string(&store));
    assert_eq!(first, Ok("0".into()));
}
