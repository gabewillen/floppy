//! Side-effect analysis of conditions and `<log expr>` (SCXML 1.0 §4.7.3:
//! "The SCXML processor MUST ensure that <log> has no side-effects on
//! document interpretation."; Appendix B, of a data model's condition
//! language: "This language MUST not have side effects"; §5.9.1:
//! "Conditional expressions in conformant SCXML documents SHOULD NOT have
//! side effects.").

use scxml_data_model_ecmascript::{
    Certainty, EffectFinding, EffectFragment, EffectKind, EffectRole, check_effects,
};
use scxml_types::ExternalScripts;

fn condition(source: &str) -> EffectFragment<'_> {
    EffectFragment {
        role: EffectRole::Condition,
        source,
    }
}

fn script(source: &str) -> EffectFragment<'_> {
    EffectFragment {
        role: EffectRole::Script,
        source,
    }
}

fn analyze(fragments: &[EffectFragment<'_>]) -> Vec<EffectFinding> {
    check_effects(fragments, ExternalScripts::Absent)
        .expect("request is within the analysis bounds")
}

/// The findings of the one condition after `scripts`.
fn findings(scripts: &[&str], cond: &str) -> Vec<EffectFinding> {
    let mut fragments: Vec<EffectFragment<'_>> = scripts.iter().map(|text| script(text)).collect();
    fragments.push(condition(cond));
    analyze(&fragments)
}

fn kinds(found: &[EffectFinding]) -> Vec<(Certainty, EffectKind, String)> {
    found
        .iter()
        .map(|finding| (finding.certainty, finding.kind, finding.subject.clone()))
        .collect()
}

fn certain(kind: EffectKind, subject: &str) -> Vec<(Certainty, EffectKind, String)> {
    vec![(Certainty::Certain, kind, subject.to_owned())]
}

fn undecidable(kind: EffectKind, subject: &str) -> Vec<(Certainty, EffectKind, String)> {
    vec![(Certainty::Undecidable, kind, subject.to_owned())]
}

#[test]
fn writes_outside_the_condition_are_certain_effects() {
    assert_eq!(
        kinds(&findings(&[], "x = 2")),
        certain(EffectKind::Assignment, "x")
    );
    assert_eq!(
        kinds(&findings(&[], "x += 2")),
        certain(EffectKind::Assignment, "x")
    );
    // Official IRP 444 writes its condition this way.
    assert_eq!(
        kinds(&findings(&[], "++var1==2")),
        certain(EffectKind::Update, "var1")
    );
    assert_eq!(
        kinds(&findings(&[], "count-- > 0")),
        certain(EffectKind::Update, "count")
    );
    assert_eq!(
        kinds(&findings(&[], "delete data.flag")),
        certain(EffectKind::Deletion, "data.flag")
    );
    assert_eq!(
        kinds(&findings(&[], "(data.n = 1) == 1")),
        certain(EffectKind::Assignment, "data.n")
    );
    assert_eq!(
        kinds(&findings(&[], "this.x = 1")),
        certain(EffectKind::Assignment, "this.x")
    );
    assert_eq!(
        kinds(&findings(&[], "[a] = [1]")),
        certain(EffectKind::Assignment, "a")
    );
}

#[test]
fn a_plain_top_level_assignment_names_its_operator() {
    let found = findings(&[], "x = 2");
    let operator = found[0].operator.expect("the `=` is reported");
    assert_eq!((operator.start, operator.end), (2, 3));
    assert_eq!(
        findings(&[], "x  =  2")[0].operator.map(|at| at.start),
        Some(3)
    );
    assert_eq!(findings(&[], "(x = 2) && y")[0].operator, None);
    assert_eq!(findings(&[], "x += 2")[0].operator, None);
    assert_eq!(findings(&[], "x = /* c */ 2")[0].operator, None);
}

#[test]
fn mutating_builtins_are_certain_effects() {
    let found = findings(&[], "list.push(1) > 0");
    assert_eq!(kinds(&found), certain(EffectKind::MutatingCall, "push"));
    assert_eq!((found[0].span.start, found[0].span.end), (0, 12));
    assert_eq!(
        kinds(&findings(&[], "data.items.sort()")),
        certain(EffectKind::MutatingCall, "sort")
    );
    assert_eq!(
        kinds(&findings(&[], "m.set('k', 1)")),
        certain(EffectKind::MutatingCall, "set")
    );
    assert_eq!(
        kinds(&findings(&[], "Object.assign(data, {a: 1})")),
        certain(EffectKind::MutatingCall, "Object.assign")
    );
    assert_eq!(
        kinds(&findings(&[], "Object.freeze(data)")),
        certain(EffectKind::MutatingCall, "Object.freeze")
    );
    assert_eq!(
        kinds(&findings(&[], "Array.prototype.push.call(data, 1)")),
        certain(EffectKind::MutatingCall, "push")
    );
    assert_eq!(
        kinds(&findings(&[], "Math.random() < 0.5")),
        certain(EffectKind::Random, "Math.random")
    );
}

#[test]
fn fresh_and_local_values_are_not_effects() {
    for pure in [
        "In('s1') && x > 1",
        "typeof Var1 === 'undefined' ",
        "'name' in _event && 'data' in _event",
        "_event.data.getElementsByTagName('book')[1].getAttribute('title') == 'title2'",
        "_event.raw.search(/Var1=2/) !== -1",
        "_event.raw.search('POST') !== -1",
        "(function(str, starts){if (starts === '') return true;if (str == null || starts == null) return false;str = String(str); starts = String(starts);return str.length >= starts.length && str.slice(0, starts.length) === starts;})(Var1, Var2)",
        "(function () { var t = {}; t.a = 1; t.a++; return t.a; })() == 2",
        "[3, 1, 2].sort()[0] == 1",
        "data.slice().sort().length > 0",
        "[1, 2, 3].map(function (v) { return v * 2; }).length == 3",
        "list.filter(v => v > 1).length",
        "JSON.stringify(data).length > 2",
        "Math.max(a, b) > 3",
        "new Date(0).getTime() == 0",
        "Object.keys(data).length == 2",
        "/a/g.test(text)",
        "(function () { var o = JSON.parse('{}'); o.k = 1; return o.k; })()",
    ] {
        assert_eq!(kinds(&findings(&[], pure)), Vec::new(), "{pure}");
    }
}

#[test]
fn document_functions_are_analyzed_interprocedurally() {
    let bump = "function bump() { count++; return true; }";
    let found = findings(&[bump], "bump()");
    assert_eq!(kinds(&found), certain(EffectKind::FunctionCall, "bump"));
    assert_eq!((found[0].span.start, found[0].span.end), (0, 6));
    let cause = found[0]
        .cause
        .clone()
        .expect("the write in the body is the cause");
    assert_eq!(cause.kind, EffectKind::Update);
    assert_eq!(cause.subject, "count");
    assert_eq!(cause.function, "bump");
    assert_eq!(cause.span.fragment, 0);
    assert_eq!(&bump[cause.span.start..cause.span.end], "count");

    let chain = "function outer() { return inner(); } function inner() { total = 1; return true; }";
    let found = findings(&[chain], "outer() && x");
    assert_eq!(kinds(&found), certain(EffectKind::FunctionCall, "outer"));
    let cause = found[0].cause.clone().expect("cause");
    assert_eq!(
        (cause.kind, cause.subject.as_str(), cause.function.as_str()),
        (EffectKind::Assignment, "total", "inner")
    );

    let pure = "function pure(a) { var b = a + 1; var o = {n: b}; o.n++; return o.n; }";
    assert_eq!(kinds(&findings(&[pure], "pure(1) == 3")), Vec::new());

    let add = "function add(list) { list.push(1); return true; }";
    assert_eq!(kinds(&findings(&[add], "add([])")), Vec::new());
    assert_eq!(
        kinds(&findings(&[add], "add(items)")),
        certain(EffectKind::FunctionCall, "add")
    );

    let global_this = "function mark() { this.marked = true; return 1; }";
    assert_eq!(
        kinds(&findings(&[global_this], "mark()")),
        certain(EffectKind::FunctionCall, "mark")
    );
    let strict_this = "function mark() { 'use strict'; this.marked = true; return 1; }";
    assert_eq!(kinds(&findings(&[strict_this], "mark()")), Vec::new());

    let constructor = "function Point(x) { this.x = x; }";
    assert_eq!(
        kinds(&findings(&[constructor], "new Point(1).x == 1")),
        Vec::new()
    );
    let counting = "function Point(x) { Point.made++; this.x = x; }";
    assert_eq!(
        kinds(&findings(&[counting], "new Point(1).x == 1")),
        certain(EffectKind::FunctionCall, "Point")
    );

    let recursive = "function down(n) { return n > 0 ? down(n - 1) : true; }";
    assert_eq!(kinds(&findings(&[recursive], "down(3)")), Vec::new());
    let recursive_write = "function down(n) { seen++; return n > 0 ? down(n - 1) : true; }";
    assert_eq!(
        kinds(&findings(&[recursive_write], "down(3)")),
        certain(EffectKind::FunctionCall, "down")
    );

    let callback =
        "function each(f) { return [1].every(f); } function check(v) { hits++; return v; }";
    assert_eq!(
        kinds(&findings(&[callback], "[1].every(check)")),
        certain(EffectKind::FunctionCall, "check")
    );
}

#[test]
fn a_data_function_is_a_document_function() {
    let fragments = [
        EffectFragment {
            role: EffectRole::Data("var1"),
            source: "function(invar) {return invar + 1;}",
        },
        condition("var1(2) == 3"),
    ];
    assert_eq!(kinds(&analyze(&fragments)), Vec::new());
    let fragments = [
        EffectFragment {
            role: EffectRole::Data("f"),
            source: "function(a) { hits++; return a; }",
        },
        EffectFragment {
            role: EffectRole::Log,
            source: "f(1)",
        },
    ];
    assert_eq!(
        kinds(&analyze(&fragments)),
        certain(EffectKind::FunctionCall, "f")
    );
}

#[test]
fn log_expressions_are_checked_and_other_expressions_are_not() {
    let fragments = [
        EffectFragment {
            role: EffectRole::Log,
            source: "x++",
        },
        EffectFragment {
            role: EffectRole::Value,
            source: "y++",
        },
        script("z = 1;"),
    ];
    let found = analyze(&fragments);
    assert_eq!(kinds(&found), certain(EffectKind::Update, "x"));
    assert_eq!(found[0].span.fragment, 0);
}

#[test]
fn what_the_analysis_cannot_see_is_undecidable() {
    assert_eq!(
        kinds(&findings(&[], "foo()")),
        undecidable(EffectKind::UnknownCall, "foo")
    );
    assert_eq!(
        kinds(&findings(&[], "eval('x = 1')")),
        undecidable(EffectKind::DynamicCode, "eval")
    );
    assert_eq!(
        kinds(&findings(&[], "Function('x = 1')()")),
        vec![
            (
                Certainty::Undecidable,
                EffectKind::UnknownCall,
                "Function('x = 1')".to_owned()
            ),
            (
                Certainty::Undecidable,
                EffectKind::DynamicCode,
                "Function".to_owned()
            ),
        ]
    );
    assert_eq!(
        kinds(&findings(&[], "data.frobnicate()")),
        undecidable(EffectKind::UnknownCall, "frobnicate")
    );
    assert_eq!(
        kinds(&findings(&[], "re.test(text)")),
        undecidable(EffectKind::MutatingCall, "test")
    );
    // A function the document may reassign.
    let fragments = [
        script("function f() { return 1; }"),
        EffectFragment {
            role: EffectRole::Location,
            source: "f",
        },
        condition("f()"),
    ];
    assert_eq!(
        kinds(&analyze(&fragments)),
        undecidable(EffectKind::UnknownCall, "f")
    );
    // An unread `<script src>` may replace any built-in.
    let found = check_effects(&[condition("Math.max(1, 2) > 1")], ExternalScripts::Present)
        .expect("bounded");
    assert_eq!(
        kinds(&found),
        undecidable(EffectKind::UnseenScript, "Math.max")
    );
    // A `<script>` of U+0085, which ECMAScript does not read as white
    // space, does not parse, so it may replace any built-in.
    assert_eq!(
        kinds(&findings(&["\u{85}"], "Math.max(1, 2) > 1")),
        undecidable(EffectKind::UnseenScript, "Math.max")
    );
    // A `<script>` of ECMAScript white space is empty.
    assert!(findings(&["\u{FEFF}\u{2028}"], "Math.max(1, 2) > 1").is_empty());
    // A document that replaces a built-in may replace any of them.
    assert_eq!(
        kinds(&findings(
            &["Math.max = function () { x++; return 0; };"],
            "Math.max(1, 2)"
        )),
        undecidable(EffectKind::UnknownCall, "Math.max")
    );
    // A getter with an effect runs on any read.
    let found = findings(&["var o = { get v() { hits++; return 1; } };"], "o.v == 1");
    assert_eq!(kinds(&found), undecidable(EffectKind::ImplicitCall, "v"));
    assert_eq!(
        found[0].cause.as_ref().map(|cause| cause.subject.as_str()),
        Some("hits")
    );
    // A pure getter does not.
    assert_eq!(
        kinds(&findings(
            &["var o = { get v() { return 1; } };"],
            "o.v == 1"
        )),
        Vec::new()
    );
    let with = "function scoped(o) { with (o) { return a; } }";
    let found = findings(&[with], "scoped(data)");
    assert_eq!(
        kinds(&found),
        undecidable(EffectKind::FunctionCall, "scoped")
    );
    assert_eq!(
        found[0].cause.as_ref().map(|cause| cause.kind),
        Some(EffectKind::DynamicScope)
    );
}

#[test]
fn findings_are_bounded_and_sorted() {
    let found = findings(&[], "a++ + b++ + c++ + d++ + e++ + f++");
    assert_eq!(found.len(), 4);
    assert!(found.windows(2).all(|pair| pair[0] <= pair[1]));
    // One request past the wasm fragment bound (16,384) is analyzed
    // natively, where the bound does not apply; the host's validator tests
    // check that the shipped data model declines it (LIMIT004).
    let request = vec![condition("x"); 16_385];
    assert!(check_effects(&request, ExternalScripts::Absent).is_ok());
}

/// 5.10: "The Processor MUST cause any attempt to change the value of a
/// system variable to fail". An assignment, compound assignment, update, or
/// `delete` of a system variable itself is reported in every fragment,
/// scripts included; a function's own variable of that name is not one.
#[test]
fn writes_of_a_system_variable_itself_are_reported_in_every_fragment() {
    let fragments = [
        script(
            "delete _event; function f() { _sessionid += 'x'; } function g(_name) { _name = 1; }",
        ),
        EffectFragment {
            role: EffectRole::Value,
            source: "_ioprocessors = {}",
        },
        EffectFragment {
            role: EffectRole::Location,
            source: "_event",
        },
        condition("(_x = 1) && [_name] == 1"),
        condition("_event.name == 'e' && typeof _sessionid == 'string'"),
    ];
    let found: Vec<(usize, EffectKind, String)> = analyze(&fragments)
        .into_iter()
        .map(|finding| (finding.span.fragment, finding.kind, finding.subject))
        .collect();
    let write = |fragment, name: &str| (fragment, EffectKind::SystemVariableWrite, name.to_owned());
    let mut expected = vec![
        write(0, "_event"),
        write(0, "_sessionid"),
        write(1, "_ioprocessors"),
        write(3, "_x"),
    ];
    expected.sort();
    let mut found = found;
    found.sort();
    assert_eq!(found, expected);
    let span = analyze(&[script("x = 1; delete _event;")])[0].span;
    assert_eq!((span.start, span.end), (14, 20));
}

/// A block, `catch`, or `for` binding of a system variable's name is a
/// local variable, not the system variable, so writes to it are not
/// reported (B.2.10: "The SCXML Processor MUST accept any ECMAScript
/// program").
#[test]
fn locally_scoped_bindings_named_like_a_system_variable_are_not_system_variables() {
    for source in [
        "{ let _event = 1; _event = 2; }",
        "{ const _name = 1; } ",
        "try { throw 1; } catch (_event) { _event = 3; }",
        "for (let _x = 0; _x < 2; _x++) {}",
        "for (const _event of [1]) { }",
        "for (let _sessionid in {a: 1}) { _sessionid = 'b'; }",
        "switch (1) { case 1: let _event = 1; _event++; }",
        "function f(_event) { _event = 1; }",
        "(function () { var _name; _name = 2; })()",
    ] {
        let found = analyze(&[script(source)]);
        assert!(found.is_empty(), "{source}: {found:?}");
    }
    // The same names outside those scopes are the system variables.
    let found = analyze(&[script(
        "{ let a = 1; } _event = 2; try {} catch (e) { _name = 1; }",
    )]);
    assert_eq!(
        found
            .iter()
            .map(|finding| finding.subject.as_str())
            .collect::<Vec<_>>(),
        ["_event", "_name"]
    );
}

/// The static forms that change a system variable through the global
/// object are reported too (5.10).
#[test]
fn static_writes_through_the_global_object_are_reported() {
    for (source, name) in [
        ("Reflect.deleteProperty(globalThis, '_event')", "_event"),
        (
            "Reflect.defineProperty(globalThis, \"_name\", {value: 1})",
            "_name",
        ),
        ("Reflect.set(this, '_sessionid', 1)", "_sessionid"),
        (
            "Object.defineProperty(globalThis, '_ioprocessors', {value: 1})",
            "_ioprocessors",
        ),
        ("delete globalThis._event", "_event"),
        ("delete globalThis['_x']", "_x"),
        ("globalThis._name = 1", "_name"),
        ("this._event++", "_event"),
    ] {
        let found = analyze(&[script(source)]);
        assert_eq!(
            found
                .iter()
                .map(|finding| (finding.kind, finding.subject.as_str()))
                .collect::<Vec<_>>(),
            [(EffectKind::SystemVariableWrite, name)],
            "{source}"
        );
    }
    for source in [
        "Reflect.deleteProperty(other, '_event')",
        "function f() { delete this._event; }",
        "var globalThis2 = {}; globalThis2._event = 1",
        "Reflect.deleteProperty(globalThis, 'event')",
        "(function (globalThis) { globalThis._event = 1; })({})",
    ] {
        let found = analyze(&[script(source)]);
        assert!(found.is_empty(), "{source}: {found:?}");
    }
}

fn log(source: &str) -> EffectFragment<'_> {
    EffectFragment {
        role: EffectRole::Log,
        source,
    }
}

/// SCXML 1.0 §4.7.3: "The SCXML processor MUST ensure that <log> has no
/// side-effects on document interpretation." Every expression below changes
/// state outside itself when it runs (each was shown to do so through the
/// shipped components): a function literal that a built-in calls back
/// (`sort`), an inline getter or conversion method that the expression
/// triggers, a generator the expression drains, a global regular
/// expression whose `lastIndex` advances, and built-ins reached through an
/// alias. None may pass silently: each is at least undecidable (`LINT019`),
/// in a `<log expr>` as in a condition.
#[test]
fn effects_reached_through_callbacks_accessors_iterators_and_aliases_are_never_silent() {
    let globals = "var n = 0; var o = { v: 0 }; var arr = []; \
                   var it = (function* () { n = 1; yield 1; })(); \
                   var re = /a/g; var rnd = Math.random;";
    let mut silent = Vec::new();
    for expression in [
        "[2, 1].sort(function (a, b) { n = 18; return a - b; })",
        "[3, 1, 2].sort(function (a, b) { o.v++; return a - b; })",
        "[2, 1].sort((a, b) => (n = a) - b)",
        "[1].map(function (x) { o.v = x; return x; })",
        "({ get p() { o.v = 7; return 1; } }).p",
        "'' + { toString() { n = 6; return ''; } }",
        "1 + { valueOf() { n = 8; return 1; } }",
        "`${{ toString() { n = 9; return ''; } }}`",
        "[...it]",
        "Array.from(it)",
        "it.next()",
        "re.test('a')",
        "re.exec('a')",
        "'aaa'.match(re)",
        "(0, Math.random)()",
        "rnd()",
        "(0, arr.push)(1)",
    ] {
        for role in ["log", "cond"] {
            let fragment = if role == "log" {
                log(expression)
            } else {
                condition(expression)
            };
            if analyze(&[script(globals), fragment]).is_empty() {
                silent.push(format!("{role} `{expression}`"));
            }
        }
    }
    assert_eq!(
        silent,
        Vec::<String>::new(),
        "effects the analysis must report"
    );
}

/// The same shapes with no effect outside the expression stay clean: a
/// callback that writes only its own locals, accessors and conversion
/// methods that write nothing.
#[test]
fn pure_callbacks_and_accessors_stay_clean() {
    for expression in [
        "[2, 1].slice().sort(function (a, b) { var d = a - b; return d; })",
        "[1, 2].map(function (x) { return x * 2; }).length",
        "({ get p() { return 1; } }).p",
        "'' + { toString() { return 'x'; } }",
    ] {
        assert_eq!(
            analyze(&[log(expression)]),
            Vec::new(),
            "`{expression}` has no effect"
        );
    }
}

/// Local functions are walked at every call, so a chain of functions that
/// each call the previous one ten times would take exponential work; the
/// step budget ends the walk on every build with one undecidable finding.
#[test]
fn exponential_local_call_chains_end_as_too_complex() {
    let mut body = String::from("function f0() { return 1; }");
    for level in 1..=15 {
        let call = format!("f{}()", level - 1);
        let calls = vec![call; 10].join(" + ");
        std::fmt::Write::write_fmt(
            &mut body,
            format_args!(" function f{level}() {{ return {calls}; }}"),
        )
        .expect("writing to a String cannot fail");
    }
    let cond = format!("(function () {{ {body} return f15(); }})() > 0");
    let found = findings(&[], &cond);
    assert!(
        found
            .iter()
            .any(|finding| finding.certainty == Certainty::Undecidable
                && finding.kind == EffectKind::TooComplex),
        "{:?}",
        kinds(&found)
    );
}
