//! Proof that [`check_nesting`] bounds Boa's native parse recursion.
//!
//! Boa's parser, bytecode compiler, and AST drop recurse natively once per
//! syntactic nesting level. The data model and the syntax check hand Boa only
//! text `check_nesting` accepts, so:
//!
//! * every accepted source parses, compiles, and drops within
//!   [`BOUND_STACK_BYTES`] (an overflow aborts this test binary, so a pass is
//!   the proof), for each recursive construct at its largest accepted size and
//!   for generated programs;
//! * the scanner never loses track of code Boa parses: when Boa accepts a
//!   program whose tail is code nested past the bound, the scanner rejects it
//!   (strings, comments, templates, regular expressions, and escaped keywords
//!   in the head cannot hide the tail's brackets);
//! * grammar-generated programs put regular expressions and divisions after
//!   every statement head, keyword, operand, and block end (with and without
//!   line breaks, in every wrapper context) around code nested past the
//!   bound; whenever the scanner accepts one, the parenthesis depth of Boa's
//!   own AST stays within the bound and Boa parses it within
//!   [`BOUND_STACK_BYTES`].

use std::ops::ControlFlow;

use boa_engine::ast::expression::Parenthesized;
use boa_engine::ast::scope::Scope;
use boa_engine::ast::visitor::{VisitWith, Visitor};
use boa_engine::interner::Interner;
use boa_engine::parser::Parser;
use boa_engine::{Context, Script, Source};
use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use scxml_data_model_ecmascript::nesting::{
    CHAIN_ELEMENTS_PER_UNIT, MAX_LEXICAL_FORKS, NESTING_BRACKET_WEIGHT,
};
use scxml_data_model_ecmascript::{MAX_PARSE_NESTING, check_nesting};

/// The wasm script byte bound. Constructs Boa parses iteratively are bounded
/// only by bytes; the proof searches within the wasm bound, and natively
/// (where no byte bound applies) those constructs cannot exhaust the stack at
/// any size.
const MAX_SCRIPT_BYTES: usize = 65_536;

/// Stack that one parse + compile + drop of any accepted source fits in,
/// including unoptimized test builds (optimized builds need far less). This
/// is deliberately much smaller than the data model's 64 MiB parse stack.
const BOUND_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Parses and compiles `source` with Boa on a thread with `stack` bytes and
/// reports whether Boa accepted it. Nothing is evaluated.
fn boa_accepts(source: String, stack: usize) -> bool {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let mut context = Context::default();
            Script::parse(Source::from_bytes(&source), None, &mut context)
                .and_then(|script| script.codeblock(&mut context).map(drop))
                .is_ok()
        })
        .expect("parse thread starts")
        .join()
        .expect("Boa parse completes without panicking")
}

/// Recursive constructs, each nested `n` times.
fn construct(name: &str, n: usize) -> String {
    let labels = || {
        (0..n)
            .map(|i| ["l", &i.to_string(), ":"].concat())
            .collect::<String>()
    };
    match name {
        "paren" => format!("{}1{}", "(".repeat(n), ")".repeat(n)),
        "array" => format!("{}1{}", "[".repeat(n), "]".repeat(n)),
        "object" => format!("({}1{})", "{a:".repeat(n), "}".repeat(n)),
        "block" => format!("{}{}", "{".repeat(n), "}".repeat(n)),
        "if" => format!("{}x;", "if(1)".repeat(n)),
        "else-if" => format!("if(1)x;{}", "else if(1)x;".repeat(n)),
        "while" => format!("{}x;", "while(0)".repeat(n)),
        "for" => format!("{}x;", "for(;0;)".repeat(n)),
        "with" => format!("{}x;", "with(a)".repeat(n)),
        "label" => format!("{}x;", labels()),
        "conditional" => format!("{}a", "a?a:".repeat(n)),
        "conditional-middle" => format!("{}a{}", "a?".repeat(n), ":a".repeat(n)),
        "not" => format!("{}x", "!".repeat(n)),
        "negate" => format!("{}x", "- ".repeat(n)),
        "typeof" => format!("{}x", "typeof ".repeat(n)),
        "void" => format!("{}x", "void ".repeat(n)),
        "new" => format!("{}x", "new ".repeat(n)),
        "arrow" => format!("{}1", "a=>".repeat(n)),
        "async-arrow" => format!("{}1", "async a=>".repeat(n)),
        "assign" => format!("{}1", "a=".repeat(n)),
        "exponent" => format!("{}1", "2**".repeat(n)),
        "or" => format!("{}1", "1||".repeat(n)),
        "and" => format!("{}1", "1&&".repeat(n)),
        "nullish" => format!("{}1", "1??".repeat(n)),
        "increment" => format!("{}a", "++".repeat(n)),
        "do" => format!("{}x;{}", "do ".repeat(n), "while(0);".repeat(n)),
        "yield" => format!("function*g(){{{}1}}", "yield ".repeat(n)),
        "await" => format!("async function f(){{{}1}}", "await ".repeat(n)),
        "spread" => format!("{}a{}", "[...".repeat(n), "]".repeat(n)),
        "template" => format!("{}1{}", "`${".repeat(n), "}`".repeat(n)),
        "tagged-template" => format!("{}1{}", "a`${".repeat(n), "}`".repeat(n)),
        "function" => format!("{}{}", "function f(){".repeat(n), "}".repeat(n)),
        "function-expression" => {
            format!("{}1{}", "(function(){return ".repeat(n), "})".repeat(n))
        }
        "method" => format!("{}{}", "({m(){return ".repeat(n), "}})".repeat(n)),
        "class" => format!("{}{}", "class A extends (".repeat(n), "B){}".repeat(n)),
        "try" => format!("{}{}", "try{".repeat(n), "}finally{}".repeat(n)),
        "switch" => format!("{}{}", "switch(1){case 1:".repeat(n), "}".repeat(n)),
        "call-arguments" => format!("{}1{}", "a(".repeat(n), ")".repeat(n)),
        "new-arguments" => format!("{}1{}", "new a(".repeat(n), ")".repeat(n)),
        "nested-index" => format!("{}1{}", "a[".repeat(n), "]".repeat(n)),
        "nested-optional-index" => format!("{}1{}", "a?.[".repeat(n), "]".repeat(n)),
        "destructuring" => format!("({}a{}=1)", "[".repeat(n), "]".repeat(n)),
        "return-conditional" => format!("function f(){{return {}1}}", "a?1:".repeat(n)),
        // Left-nested chains: one nesting unit per CHAIN_ELEMENTS_PER_UNIT.
        "member" => format!("a{}", ".b".repeat(n)),
        "call" => format!("a{}", "()".repeat(n)),
        "index" => format!("a{}", "[0]".repeat(n)),
        "optional-member" => format!("a{}", "?.b".repeat(n)),
        "optional-index" => format!("a{}", "?.[0]".repeat(n)),
        "optional-sequence" => format!("a?.[{}1]", "1,".repeat(n)),
        "optional-call" => format!("a{}", "?.()".repeat(n)),
        "add" => format!("{}1", "1+".repeat(n)),
        "multiply" => format!("{}1", "1*".repeat(n)),
        "divide" => format!("{}1", "1/".repeat(n)),
        "compare" => format!("{}1", "1<".repeat(n)),
        "equal" => format!("{}1", "1===".repeat(n)),
        "shift" => format!("{}1", "1>>>".repeat(n)),
        "bitwise" => format!("{}1", "1|".repeat(n)),
        "in" => format!("{}1", "1 in ".repeat(n)),
        "instanceof" => format!("{}1", "1 instanceof ".repeat(n)),
        "sequence" => format!("{}1", "1,".repeat(n)),
        "grouped-sequence" => format!("({}1)", "1,".repeat(n)),
        "tagged" => format!("a{}", "``".repeat(n)),
        // Iterative in Boa; bounded only by the byte limit.
        "statements" => "if (a) { b(); }\n".repeat(n),
        "array-elements" => format!("[{}1]", "1,".repeat(n)),
        "arguments" => format!("f({}1)", "1,".repeat(n)),
        "object-members" => format!("({{{}z: 1}})", "a: 1,".repeat(n)),
        other => panic!("unknown construct {other}"),
    }
}

const CONSTRUCTS: &[&str] = &[
    "paren",
    "array",
    "object",
    "block",
    "if",
    "else-if",
    "while",
    "for",
    "with",
    "label",
    "conditional",
    "conditional-middle",
    "not",
    "negate",
    "typeof",
    "void",
    "new",
    "arrow",
    "async-arrow",
    "assign",
    "exponent",
    "or",
    "and",
    "nullish",
    "increment",
    "do",
    "yield",
    "await",
    "spread",
    "template",
    "tagged-template",
    "function",
    "function-expression",
    "method",
    "class",
    "try",
    "switch",
    "call-arguments",
    "new-arguments",
    "nested-index",
    "nested-optional-index",
    "destructuring",
    "return-conditional",
    "member",
    "call",
    "index",
    "optional-member",
    "optional-index",
    "optional-sequence",
    "optional-call",
    "add",
    "multiply",
    "divide",
    "compare",
    "equal",
    "shift",
    "bitwise",
    "in",
    "instanceof",
    "sequence",
    "grouped-sequence",
    "tagged",
    "statements",
    "array-elements",
    "arguments",
    "object-members",
];

/// How the scanner charges a construct.
#[derive(Debug, Eq, PartialEq)]
enum Charge {
    /// Recursive nesting: at most `MAX_PARSE_NESTING` levels.
    Nesting,
    /// Left-nested chain: more than `MAX_PARSE_NESTING` elements, but at most
    /// `MAX_PARSE_NESTING * CHAIN_ELEMENTS_PER_UNIT`.
    Chain,
    /// Iterative in Boa: only the byte bound applies.
    Bytes,
}

fn charge(name: &str) -> Charge {
    match name {
        "member" | "call" | "index" | "optional-member" | "optional-index"
        | "optional-sequence" | "optional-call" | "add" | "multiply" | "divide" | "compare"
        | "equal" | "shift" | "bitwise" | "in" | "instanceof" | "sequence" | "grouped-sequence"
        | "tagged" => Charge::Chain,
        "statements" | "array-elements" | "arguments" | "object-members" => Charge::Bytes,
        _ => Charge::Nesting,
    }
}

fn accepted(name: &str, n: usize) -> bool {
    let source = construct(name, n);
    source.len() <= MAX_SCRIPT_BYTES && check_nesting(&source).is_ok()
}

/// The largest `n` whose construct is accepted within the script byte bound
/// (acceptance is monotonic in `n`).
fn largest_accepted(name: &str) -> usize {
    let (mut low, mut high) = (0, MAX_SCRIPT_BYTES);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if accepted(name, middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

#[test]
fn every_construct_at_its_bound_fits_the_documented_stack() {
    for name in CONSTRUCTS {
        let n = largest_accepted(name);
        assert!(n > 0, "{name}: the shallowest form must be accepted");
        let at_bound = construct(name, n);
        // Boa may reject the program (for example a `yield` outside a
        // generator); either way it must not exhaust the stack.
        let _accepted = boa_accepts(at_bound, BOUND_STACK_BYTES);
        assert!(
            !accepted(name, n + 1),
            "{name}: one level past the bound must be declined"
        );
    }
}

#[test]
fn recursive_constructs_are_declined_long_before_the_byte_bound() {
    // Recursive constructs stop within MAX_PARSE_NESTING levels, chains
    // within MAX_PARSE_NESTING * CHAIN_ELEMENTS_PER_UNIT elements, and only
    // the constructs Boa handles iteratively reach the byte bound.
    let chain_bound = MAX_PARSE_NESTING * CHAIN_ELEMENTS_PER_UNIT;
    for name in CONSTRUCTS {
        let n = largest_accepted(name);
        let observed = if n <= MAX_PARSE_NESTING {
            Charge::Nesting
        } else if n <= chain_bound {
            Charge::Chain
        } else {
            Charge::Bytes
        };
        assert_eq!(observed, charge(name), "{name}: largest accepted {n}");
        if observed == Charge::Bytes {
            assert!(
                construct(name, n + 1).len() > MAX_SCRIPT_BYTES,
                "{name}: only the byte bound stops it"
            );
        }
    }
}

/// Lexemes that switch lexical state: quotes, comments, templates, regular
/// expressions, division, escaped keywords, and property names spelled like
/// keywords.
const TRICKY: &[&str] = &[
    "'(\"'",
    "\"'[\"",
    "'\\''",
    "`(${a}'`",
    "`${`${'('}`}`",
    "/[/(']+/g",
    "/'/",
    "/*'(*/a",
    "/*`*/a",
    "a/b/c",
    "a / 'x' / b",
    "x.if",
    "o.return",
    "\\u0061",
    "a++ / 2",
    "{a: 1}.a",
    "void 0",
    "typeof a",
    "In('s')",
    "a?.[1, 2]",
    "a?.b?.[0]",
    "a?.(1, 2)",
];

fn expression() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        Just("a".to_owned()),
        Just("1".to_owned()),
        proptest::sample::select(TRICKY).prop_map(str::to_owned),
    ];
    atom.prop_recursive(6, 48, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|e| format!("({e})")),
            inner.clone().prop_map(|e| format!("[{e}]")),
            inner.clone().prop_map(|e| format!("({{k: {e}}})")),
            inner.clone().prop_map(|e| format!("`${{{e}}}`")),
            inner.clone().prop_map(|e| format!("!{e}")),
            inner.clone().prop_map(|e| format!("f({e})")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("{a} + {b}")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("{a} / {b}")),
            (inner.clone(), inner.clone(), inner.clone())
                .prop_map(|(a, b, c)| format!("{a} ? {b} : {c}")),
            inner.clone().prop_map(|e| format!("(x => {e})")),
            inner.prop_map(|e| format!("function () {{ return {e}; }}")),
        ]
    })
}

fn statement() -> impl Strategy<Value = String> {
    let simple = expression().prop_map(|e| format!("{e};"));
    simple.prop_recursive(4, 24, 3, |inner| {
        prop_oneof![
            (expression(), inner.clone()).prop_map(|(c, s)| format!("if ({c}) {s}")),
            (expression(), inner.clone(), inner.clone())
                .prop_map(|(c, a, b)| format!("if ({c}) {a} else {b}")),
            proptest::collection::vec(inner.clone(), 0..4)
                .prop_map(|body| format!("{{ {} }}", body.join(" "))),
            (expression(), inner.clone()).prop_map(|(c, s)| format!("while ({c}) {s}")),
            inner.clone().prop_map(|s| format!("label: {s}")),
            inner.prop_map(|s| format!("function g() {{ {s} }}")),
        ]
    })
}

fn program() -> impl Strategy<Value = String> {
    proptest::collection::vec(statement(), 1..6).prop_map(|body| body.join("\n"))
}

/// Arbitrary lexeme soup: mostly invalid programs.
fn soup() -> impl Strategy<Value = String> {
    let lexeme = prop_oneof![
        proptest::sample::select(TRICKY).prop_map(str::to_owned),
        proptest::sample::select(
            &[
                "(", ")", "[", "]", "{", "}", "`", "${", "'", "\"", "/", "*", "//", "/*", "*/",
                "\n", " ", ";", ",", "?", ":", "=", "=>", "if", "else", "while", "do", "function",
                "return", "new", "typeof", "...", "++", "a", "1", "case", "switch", "yield",
                "await", "of",
            ][..]
        )
        .prop_map(str::to_owned),
    ];
    proptest::collection::vec(lexeme, 0..200).prop_map(|parts| parts.concat())
}

/// Code nested past the bound: visible to Boa unless the head hides it.
fn deep_tail() -> String {
    let levels = MAX_PARSE_NESTING / 8 + 2;
    format!("\n;{}1{};", "(".repeat(levels), ")".repeat(levels))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn accepted_programs_fit_the_documented_stack(source in program()) {
        if check_nesting(&source).is_ok() {
            let _accepted = boa_accepts(source, BOUND_STACK_BYTES);
        }
    }

    #[test]
    fn accepted_soup_fits_the_documented_stack(source in soup()) {
        if check_nesting(&source).is_ok() {
            let _accepted = boa_accepts(source, BOUND_STACK_BYTES);
        }
    }

    #[test]
    fn nothing_boa_parses_as_code_escapes_the_count(head in program()) {
        let source = format!("{head}{}", deep_tail());
        if boa_accepts(source.clone(), 64 * 1024 * 1024) {
            prop_assert!(check_nesting(&source).is_err(), "{source}");
        }
    }

    #[test]
    fn nothing_in_soup_hides_code_from_the_count(head in soup()) {
        let source = format!("{head}{}", deep_tail());
        if boa_accepts(source.clone(), 64 * 1024 * 1024) {
            prop_assert!(check_nesting(&source).is_err(), "{source}");
        }
    }
}

/// Largest parenthesis nesting the scanner can accept: each open `(` costs
/// [`NESTING_BRACKET_WEIGHT`] units.
const MAX_ACCEPTED_PARENS: usize = MAX_PARSE_NESTING / NESTING_BRACKET_WEIGHT;

/// Parenthesized expressions nested one level past what the scanner accepts:
/// if Boa parses this as code, its AST is deeper than any accepted source.
fn deep_parens() -> String {
    let levels = MAX_ACCEPTED_PARENS + 1;
    format!("{}1{}", "(".repeat(levels), ")".repeat(levels))
}

/// Deepest nesting of parenthesized expressions in Boa's AST.
#[derive(Default)]
struct ParenDepth {
    depth: usize,
    deepest: usize,
}

impl<'ast> Visitor<'ast> for ParenDepth {
    type BreakTy = ();

    fn visit_parenthesized(&mut self, node: &'ast Parenthesized) -> ControlFlow<()> {
        self.depth += 1;
        self.deepest = self.deepest.max(self.depth);
        let flow = node.visit_with(self);
        self.depth -= 1;
        flow
    }
}

/// Boa's parse of `source` (on a large stack): the deepest parenthesis
/// nesting in its AST, or `None` when Boa rejects the program.
fn boa_paren_depth(source: &str) -> Option<usize> {
    let mut interner = Interner::default();
    let script = Parser::new(Source::from_bytes(source))
        .parse_script(&Scope::new_global(), &mut interner)
        .ok()?;
    let mut depth = ParenDepth::default();
    let _flow = script.visit_with(&mut depth);
    Some(depth.deepest)
}

/// Text before a `/`: every statement head, keyword, operand, block end,
/// contextual word, escaped keyword, and operator position.
const SLASH_PREFIXES: &[&str] = &[
    "if (a)",
    "while (a)",
    "for (;a;)",
    "for (x of y)",
    "for (x in y)",
    "for (let x of y)",
    "for await (x of y)",
    "with (a)",
    "switch (a) { case 1:",
    "switch (a) { default:",
    "if (a) b; else",
    "do",
    "try {} catch {}",
    "try {} catch (e) {}",
    "try {} finally {}",
    "{}",
    "if (a) {}",
    "function f() {}",
    "x => {}",
    "async () => {}",
    "class A {}",
    "x = {}",
    "x = class {}",
    "x = function () {}",
    "l:",
    "return",
    "typeof",
    "void",
    "delete",
    "throw",
    "new",
    "a in",
    "a instanceof",
    "yield",
    "await",
    "a++",
    "a--",
    "++a",
    "x",
    "var x",
    "let x",
    "const x = 1",
    "var a = 1, b",
    "break",
    "continue",
    "break l",
    "continue l",
    "debugger",
    "this",
    "null",
    "1",
    "'s'",
    "`t`",
    "`${a}`",
    "/r/g",
    "f()",
    "a[0]",
    "x = {a: 1}",
    "a ?",
    "a ? b :",
    "a =",
    "a,",
    ";",
    "(",
    "[",
    "{",
    "a &&",
    "!",
    "...",
    "x =>",
    "of",
    "let",
    "async",
    "get",
    "static",
    "In",
    "o.if",
    "o.return",
    "\\u0069f (a)",
    "\\u0066or (;;)",
    "v\\u0061r x",
    "try {} c\\u0061tch {}",
    "r\\u0065turn",
    "a <!--",
    "a -->",
    "#!x\n",
];

/// Separators between the prefix and the `/`.
const SLASH_SEPARATORS: &[&str] = &["", " ", "\n", " /*\n*/ ", " /* */ ", " // c\n", "\u{2028}"];

/// The `/` and what follows it on its line: regular-expression bodies that
/// hide quotes, brackets, or the deep code, and divisions into strings.
const SLASH_FORMS: &[&str] = &[
    "/'/",
    "/\"/",
    "/`/",
    "/[/']/",
    "/=/",
    "/ 1 /",
    "/(/",
    "/)/",
    "/ DEEP /g",
    "/'/ + '",
    "/ 2",
    "/= 2",
    "/}/",
];

/// What follows the slash form: the deep code as code, or hidden by a quote
/// that only one reading opens.
const SLASH_TAILS: &[&str] = &[
    "; DEEP",
    " DEEP",
    "'; DEEP",
    "\"; DEEP",
    "`; DEEP",
    "; x = 'DEEP'",
    "; DEEP;'",
    "\n; DEEP",
    "'\n; DEEP",
    "\\\n; DEEP",
    " } DEEP",
];

/// Contexts around the program, so heads such as `yield`, `await`,
/// `break`, and `case` are valid.
const SLASH_WRAPPERS: &[(&str, &str)] = &[
    ("", ""),
    ("function* g() { l: for (;;) { ", "\n} }"),
    ("async function f() { l: do { ", "\n} while (0) }"),
    ("switch (a) { case 0: ", "\n}"),
    ("x = [", "\n]"),
    ("f(", "\n)"),
    ("`${", "\n}`"),
    ("({k: ", "\n})"),
    ("class C { m() { ", "\n} }"),
];

fn slash_program(prefix: &str, separator: &str, form: &str, tail: &str) -> String {
    format!("{prefix}{separator}{form}{tail}").replace("DEEP", &deep_parens())
}

/// Asserts the guard's soundness for one generated source: accepted means
/// Boa's AST nests no deeper than the bound, and Boa parses it within the
/// documented stack.
fn assert_sound(source: &str) {
    if check_nesting(source).is_err() {
        return;
    }
    if let Some(depth) = boa_paren_depth(source) {
        assert!(
            depth <= MAX_ACCEPTED_PARENS,
            "accepted, but Boa nests {depth} parentheses: {source:?}"
        );
    }
    let _accepted = boa_accepts(source.to_owned(), BOUND_STACK_BYTES);
}

#[test]
fn every_slash_position_is_sound() {
    // Exhaustive over prefixes, separators, forms, and tails at top level;
    // one large-stack thread runs the whole sweep.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            for prefix in SLASH_PREFIXES {
                for separator in SLASH_SEPARATORS {
                    for form in SLASH_FORMS {
                        for tail in SLASH_TAILS {
                            let source = slash_program(prefix, separator, form, tail);
                            if check_nesting(&source).is_ok()
                                && let Some(depth) = boa_paren_depth(&source)
                            {
                                assert!(
                                    depth <= MAX_ACCEPTED_PARENS,
                                    "accepted, but Boa nests {depth} parentheses: {source:?}"
                                );
                            }
                        }
                    }
                }
            }
        })
        .expect("sweep thread starts")
        .join()
        .expect("every accepted slash program stays within the bound");
}

fn wrapped_slash_program() -> impl Strategy<Value = String> {
    (
        proptest::sample::select(SLASH_WRAPPERS),
        proptest::collection::vec(
            (
                proptest::sample::select(SLASH_PREFIXES),
                proptest::sample::select(SLASH_SEPARATORS),
                proptest::sample::select(SLASH_FORMS),
                proptest::sample::select(SLASH_TAILS),
            ),
            1..4,
        ),
    )
        .prop_map(|((open, close), parts)| {
            let body = parts
                .iter()
                .map(|(prefix, separator, form, tail)| slash_program(prefix, separator, form, tail))
                .collect::<Vec<_>>()
                .join("\n;");
            format!("{open}{body}{close}")
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2048,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x5c_4d_1e_c3),
        ..ProptestConfig::default()
    })]

    #[test]
    fn wrapped_slash_programs_are_sound(source in wrapped_slash_program()) {
        let checked = source.clone();
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || assert_sound(&checked))
            .expect("check thread starts")
            .join()
            .map_err(|_| TestCaseError::fail(format!("unsound: {source:?}")))?;
    }
}

/// Hostile nesting shapes: each is declined (or, for the benign controls,
/// handled) without a native stack overflow.
#[test]
fn hostile_nesting_shapes_are_declined() {
    let parens = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));
    for source in [
        // A `/` after a for, if, or while head, or after an operand.
        format!("for(;0;) /'/; {}", parens(20)),
        format!("for(;0;) /'/; {}", parens(2000)),
        format!("for(;0;) /'/; {}", parens(6000)),
        format!("for(;0;) /'/; {}", parens(50000)),
        format!("if(0) /'/; {}", parens(6000)),
        format!("while(0) /'/; {}", parens(6000)),
        format!("while(0) /'/; {}", parens(50000)),
        format!("x /'\n{}", parens(5000)),
        format!("x /'; {}", parens(5000)),
        // A quote in a comment, an if chain, a ternary chain, typeof chains.
        format!("(/*'*/ {})", parens(3000)),
        format!("{};", "if(1)".repeat(12000)),
        format!("{};", "if(1)".repeat(1000)),
        format!("({}a)", "a?a:".repeat(4000)),
        format!("({}a)", "typeof ".repeat(2200)),
    ] {
        let verdict = check_nesting(&source);
        if verdict.is_ok() {
            // Only a source whose deep part Boa cannot reach may pass.
            assert_sound(&source);
        }
    }
    // The ones whose deep part is code are declined outright.
    for source in [
        format!("for(;0;) /'/; {}", parens(20)),
        format!("for(;0;) /'/; {}", parens(6000)),
        format!("if(0) /'/; {}", parens(6000)),
        format!("while(0) /'/; {}", parens(50000)),
        format!("(/*'*/ {})", parens(3000)),
        format!("{};", "if(1)".repeat(12000)),
        format!("({}a)", "a?a:".repeat(4000)),
        format!("({}a)", "typeof ".repeat(2200)),
        // A 32 000-element sequence inside an optional-chain index.
        format!("var a=[];a?.[{}1]", "1,".repeat(32000)),
    ] {
        assert!(
            check_nesting(&source).is_err(),
            "{}",
            source.get(..40).unwrap_or(&source)
        );
    }
}

#[test]
fn boa_is_built_without_html_like_comments() {
    // The scanner lexes `<!--` and `-->` as operators, as Boa does without
    // `annex-b`; if a dependency ever enables it, this fails first.
    assert!(!boa_accepts("x = 1 <!-- 2\n".to_owned(), BOUND_STACK_BYTES));
    assert!(!boa_accepts("x\n--> ((\n".to_owned(), BOUND_STACK_BYTES));
}

#[test]
fn the_fork_budget_bounds_the_passes() {
    let fork = "x\n/ 1 /g\n";
    assert!(check_nesting(&fork.repeat(4)).is_ok());
    assert!(check_nesting(&fork.repeat(MAX_LEXICAL_FORKS + 1)).is_err());
}
