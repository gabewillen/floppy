use std::sync::Mutex;

use scxml_syntax::{SourceId, SourceText, parse_xml};

use super::*;
use crate::{Severity, validate_semantics};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

/// Deterministic stand-in checker: reports `@` as an error at its byte
/// offset, declines sources containing `HUGE`, and records every call.
#[derive(Debug, Default)]
struct MarkerChecker {
    calls: Mutex<Vec<(ExpressionKind, String)>>,
}

impl ExpressionSyntaxChecker for MarkerChecker {
    fn data_model(&self) -> &'static str {
        "ecmascript"
    }

    fn check(&self, kind: ExpressionKind, source: &str) -> ExpressionCheck {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((kind, source.to_owned()));
        }
        if source.contains("HUGE") {
            return ExpressionCheck::NotChecked;
        }
        match source.find('@') {
            Some(offset) => ExpressionCheck::Invalid(vec![SyntaxIssue {
                start: offset,
                end: offset + 1,
                message: "unexpected `@`".to_owned(),
            }]),
            None => ExpressionCheck::Valid,
        }
    }

    fn check_effects(
        &self,
        _fragments: &[super::EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> super::EffectCheck {
        super::EffectCheck::Checked(Vec::new())
    }
}

fn parse(xml: &str) -> Document {
    parse_xml(SourceText::new(SOURCE, "expr.scxml", xml.to_owned()))
        .expect("expression fixture must parse")
}

fn machine(datamodel: &str, body: &str) -> String {
    format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="{datamodel}">{body}</scxml>"#
    )
}

fn text(xml: &str, span: Span) -> &str {
    xml.get(span.start..span.end)
        .expect("span must lie inside the fixture")
}

#[test]
fn every_expression_site_is_checked_with_its_kind() {
    let xml = machine(
        "ecmascript",
        r#"<datamodel><data id="d" expr="1"/></datamodel><state id="s"><onentry>
             <assign location="d" expr="2"/><log expr="d"/><if cond="d"><elseif cond="d"/></if>
             <foreach array="[1]" item="i" index="n"/><send eventexpr="'e'" namelist="d i" idlocation="sid"/>
             <script>var x = 1;</script></onentry><transition cond="true" target="s"/></state>"#,
    );
    let checker = MarkerChecker::default();
    let report = validate_expressions(&parse(&xml), &checker);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    let calls = checker.calls.lock().expect("calls recorded").clone();
    let count = |kind| calls.iter().filter(|(found, _)| *found == kind).count();
    assert_eq!(count(ExpressionKind::Condition), 3, "{calls:?}");
    assert_eq!(count(ExpressionKind::Value), 5, "{calls:?}");
    // location, item, index, idlocation, and two namelist items
    assert_eq!(count(ExpressionKind::Location), 6, "{calls:?}");
    assert_eq!(count(ExpressionKind::Script), 1, "{calls:?}");
}

#[test]
fn issue_spans_map_through_entities_and_cdata() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="a &lt; b &amp;&amp; @" target="s"/><onentry><script><![CDATA[if (a < b) { @ }]]></script></onentry></state>"#,
    );
    let checker = MarkerChecker::default();
    let report = validate_expressions(&parse(&xml), &checker);
    assert_eq!(report.diagnostics().len(), 2, "{report:?}");
    for diagnostic in report.diagnostics() {
        assert_eq!(diagnostic.code, "SCXML038");
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(text(&xml, diagnostic.primary.span), "@");
        assert!(diagnostic.help.is_some());
        assert!(!diagnostic.notes.is_empty());
        assert!(!diagnostic.spec_references.is_empty());
        assert_eq!(diagnostic.secondary.len(), 1, "{diagnostic:?}");
    }
}

#[test]
fn location_issues_use_the_location_code() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><onentry><assign location="a@b" expr="1"/></onentry></state>"#,
    );
    let report = validate_expressions(&parse(&xml), &MarkerChecker::default());
    assert_eq!(report.diagnostics().len(), 1);
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.code, "SCXML039");
    assert_eq!(text(&xml, diagnostic.primary.span), "@");
}

#[test]
fn declined_fragments_are_errors_because_the_runtime_rejects_them() {
    // The checker declines exactly what the runtime's own bounds reject
    // (same size and nesting guard), so a declined fragment can never
    // evaluate: it is an error that stops `compile`, not an advisory.
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="HUGE" target="s"/></state>"#,
    );
    let report = validate_expressions(&parse(&xml), &MarkerChecker::default());
    assert_eq!(report.diagnostics().len(), 1);
    assert_eq!(report.diagnostics()[0].code, "LIMIT003");
    assert_eq!(report.diagnostics()[0].severity, Severity::Error);
    assert!(report.has_errors(), "{report:?}");
}

#[test]
fn other_or_missing_data_models_are_never_skipped_silently() {
    // A recognized data model without a matching checker: one note, no calls.
    let xpath = machine(
        "xpath",
        r#"<state id="s"><transition cond="@" target="s"/></state>"#,
    );
    let checker = MarkerChecker::default();
    let report = validate_expressions(&parse(&xpath), &checker);
    assert_eq!(report.diagnostics().len(), 1, "{report:?}");
    assert_eq!(report.diagnostics()[0].code, "LINT010");
    assert_eq!(report.diagnostics()[0].severity, Severity::Note);
    assert!(checker.calls.lock().expect("calls").is_empty());

    // No `datamodel`: the default ECMAScript data model applies (SCXML 1.0
    // §3.2.1: the default is "platform-specific"), so its checker runs and
    // nothing reports the attribute as missing.
    let missing = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="s"><transition cond="n = = 1" target="s"/></state></scxml>"#
    );
    let checker = MarkerChecker::default();
    let _report = validate_expressions(&parse(&missing), &checker);
    assert_eq!(
        checker
            .calls
            .lock()
            .expect("calls")
            .iter()
            .map(|(_, source)| source.as_str())
            .collect::<Vec<_>>(),
        ["n = = 1"],
        "the default data model's checker sees the condition"
    );
    assert!(
        !validate_semantics(&parse(&missing))
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "LINT010")
    );

    // No expressions: nothing to check, nothing to report.
    let quiet =
        format!(r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><final id="f"/></scxml>"#);
    assert!(
        !validate_semantics(&parse(&quiet))
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "LINT010")
    );
}

/// Stand-in `XPath` checker: reports `!` as an error and records calls.
#[derive(Debug, Default)]
struct XPathMarkerChecker {
    calls: Mutex<Vec<String>>,
}

impl ExpressionSyntaxChecker for XPathMarkerChecker {
    fn data_model(&self) -> &'static str {
        "xpath"
    }

    fn check(&self, _kind: ExpressionKind, source: &str) -> ExpressionCheck {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(source.to_owned());
        }
        match source.find('!') {
            Some(offset) => ExpressionCheck::Invalid(vec![SyntaxIssue {
                start: offset,
                end: offset + 1,
                message: "unexpected `!`".to_owned(),
            }]),
            None => ExpressionCheck::Valid,
        }
    }

    fn check_effects(
        &self,
        _fragments: &[super::EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> super::EffectCheck {
        super::EffectCheck::Checked(Vec::new())
    }
}

#[test]
fn each_machine_is_checked_only_by_the_checker_for_its_exact_data_model() {
    let ecmascript = MarkerChecker::default();
    let xpath = XPathMarkerChecker::default();
    let checkers: [&dyn ExpressionSyntaxChecker; 2] = [&ecmascript, &xpath];

    let xpath_xml = machine(
        "xpath",
        r#"<datamodel><data id="a"/></datamodel><state id="s"><transition cond="!1" target="s"/><transition cond="$a = @" target="s"/></state>"#,
    );
    let report = validate_expressions_with(&parse(&xpath_xml), &checkers);
    assert_eq!(report.diagnostics().len(), 1, "{report:?}");
    assert_eq!(report.diagnostics()[0].code, "SCXML038");
    assert_eq!(text(&xpath_xml, report.diagnostics()[0].primary.span), "!");
    assert!(ecmascript.calls.lock().expect("calls").is_empty());
    // Two fragments, plus the re-check of `!1` with its habit repaired.
    assert_eq!(
        *xpath.calls.lock().expect("calls"),
        ["!1", "not(1)", "$a = @"]
    );

    let ecmascript_xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="!done" target="s"/></state>"#,
    );
    let report = validate_expressions_with(&parse(&ecmascript_xml), &checkers);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    assert_eq!(ecmascript.calls.lock().expect("calls").len(), 1);
    assert_eq!(xpath.calls.lock().expect("calls").len(), 3);

    // No checker linked for a recognized data model: one note naming the
    // linked data models, and no checker is called.
    let only_ecmascript: [&dyn ExpressionSyntaxChecker; 1] = [&ecmascript];
    let report = validate_expressions_with(&parse(&xpath_xml), &only_ecmascript);
    assert_eq!(report.diagnostics().len(), 1, "{report:?}");
    assert_eq!(report.diagnostics()[0].code, "LINT010");
    assert_eq!(report.diagnostics()[0].severity, Severity::Note);
    assert_eq!(ecmascript.calls.lock().expect("calls").len(), 1);
}

#[test]
fn a_script_of_other_unicode_white_space_is_checked() {
    // Only XML white space blanks a `<script>` body: U+1680 is content that
    // the data model's checker sees, while an XML-blank body is skipped.
    let xpath = XPathMarkerChecker::default();
    let checkers: [&dyn ExpressionSyntaxChecker; 1] = [&xpath];
    let xml = machine(
        "xpath",
        r#"<state id="s"><onentry><script>&#x1680;</script><script> &#x9;&#xD;&#xA;</script></onentry></state>"#,
    );
    let report = validate_expressions_with(&parse(&xml), &checkers);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    assert_eq!(*xpath.calls.lock().expect("calls"), ["\u{1680}"]);
}

#[test]
fn unknown_data_model_fails_closed_with_a_typo_fix() {
    for (value, suggestion) in [
        ("ecmascipt", Some("ecmascript")),
        ("ECMAScript", Some("ecmascript")),
        ("nul", Some("null")),
        ("lua", None),
    ] {
        let xml = machine(value, r#"<final id="f"/>"#);
        let report = crate::datamodel_attribute_diagnostics(&parse(&xml), None);
        let error = report
            .iter()
            .find(|diagnostic| diagnostic.code == "PKG005")
            .unwrap_or_else(|| panic!("`{value}` must be rejected"));
        assert_eq!(error.severity, Severity::Error);
        assert_eq!(text(&xml, error.primary.span), value);
        assert_eq!(
            error
                .fixes
                .first()
                .map(|fix| (fix.applicability, fix.edits[0].replacement.as_str())),
            suggestion.map(|suggestion| (Applicability::MaybeIncorrect, suggestion)),
            "{value}"
        );
    }
    for known in KNOWN_DATA_MODELS {
        let xml = machine(known, r#"<final id="f"/>"#);
        assert!(
            crate::datamodel_attribute_diagnostics(&parse(&xml), None).is_empty(),
            "{known}"
        );
    }
}

#[test]
fn parser_wording_is_project_owned_with_specific_help() {
    #[derive(Debug)]
    struct BoaLike;
    impl ExpressionSyntaxChecker for BoaLike {
        fn data_model(&self) -> &'static str {
            "ecmascript"
        }
        fn check(&self, _kind: ExpressionKind, source: &str) -> ExpressionCheck {
            // Valid once repaired: the validator re-checks every repair.
            let Some(offset) = source.find("= =").map(|offset| offset + 2) else {
                return ExpressionCheck::Valid;
            };
            ExpressionCheck::Invalid(vec![SyntaxIssue {
                start: offset,
                end: offset + 1,
                message: "unexpected `=`: primary expression".to_owned(),
            }])
        }

        fn check_effects(
            &self,
            _fragments: &[super::EffectFragment<'_>],
            _scripts: ExternalScripts,
        ) -> super::EffectCheck {
            super::EffectCheck::Checked(Vec::new())
        }
    }
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="n = = 1" target="s"/></state>"#,
    );
    let report = validate_expressions(&parse(&xml), &BoaLike);
    let diagnostic = &report.diagnostics()[0];
    assert!(
        !diagnostic.message.contains("primary expression"),
        "{}",
        diagnostic.message
    );
    assert!(diagnostic.message.contains("where an operand was expected"));
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("`==`"))
    );
    let fix = &diagnostic.fixes[0];
    assert_eq!(text(&xml, fix.edits[0].span), " ");
    assert_eq!(fix.edits[0].replacement, "");
    assert_eq!(fix.message, "compare with `==`");
}

#[test]
fn stray_equals_repair_yields_exactly_one_comparison_operator() {
    #[derive(Debug)]
    struct StrayEquals;
    impl ExpressionSyntaxChecker for StrayEquals {
        fn data_model(&self) -> &'static str {
            "ecmascript"
        }
        fn check(&self, _kind: ExpressionKind, source: &str) -> ExpressionCheck {
            // A stray `=` (or an over-long operator a bad repair would
            // leave) is invalid; the validator re-checks every repair.
            if !["= =", "< =", "====", "!==="]
                .iter()
                .any(|broken| source.contains(broken))
            {
                return ExpressionCheck::Valid;
            }
            let offset = source.rfind('=').unwrap_or(0);
            ExpressionCheck::Invalid(vec![SyntaxIssue {
                start: offset,
                end: offset + 1,
                message: "unexpected `=`: primary expression".to_owned(),
            }])
        }

        fn check_effects(
            &self,
            _fragments: &[super::EffectFragment<'_>],
            _scripts: ExternalScripts,
        ) -> super::EffectCheck {
            super::EffectCheck::Checked(Vec::new())
        }
    }
    for (cond, repaired, message) in [
        ("x = = 3", "x == 3", "compare with `==`"),
        ("x == = 3", "x === 3", "compare with `===`"),
        (
            "x === = 3",
            "x === 3",
            "remove the stray `=` and compare with `===`",
        ),
        ("x != = 3", "x !== 3", "compare with `!==`"),
        (
            "x !== = 3",
            "x !== 3",
            "remove the stray `=` and compare with `!==`",
        ),
        ("x &lt; = 3", "x &lt;= 3", "compare with `<=`"),
    ] {
        let xml = machine(
            "ecmascript",
            &format!(r#"<state id="s"><transition cond="{cond}" target="s"/></state>"#),
        );
        let report = validate_expressions(&parse(&xml), &StrayEquals);
        let diagnostic = &report.diagnostics()[0];
        let fix = &diagnostic.fixes[0];
        assert_eq!(fix.message, message, "{cond}");
        let edit = &fix.edits[0];
        let fixed = format!(
            "{}{}{}",
            &xml[..edit.span.start],
            edit.replacement,
            &xml[edit.span.end..]
        );
        assert!(
            fixed.contains(&format!("cond=\"{repaired}\"")),
            "{cond} -> {fixed}"
        );
    }
}

/// Natively expression checking is unbounded: one fragment past the wasm
/// bound (16,384) is checked, with no `LIMIT004`.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_checking_has_no_fragment_bound() {
    let transitions = "<transition cond=\"x\" target=\"s\"/>".repeat(16_385);
    let xml = machine(
        "ecmascript",
        &format!(r#"<state id="s">{transitions}</state>"#),
    );
    let checker = MarkerChecker::default();
    let report = validate_expressions(&parse(&xml), &checker);
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.code != "LIMIT004"),
        "{report:?}"
    );
    assert_eq!(checker.calls.lock().expect("calls").len(), 16_385);
}

#[test]
fn decode_offsets_handles_references_and_rejects_malformed_input() {
    let (decoded, offsets) = decode_offsets("a&lt;&#x1F600;b", 10).expect("decodes");
    assert_eq!(decoded, "a<\u{1F600}b");
    assert_eq!(offsets.first(), Some(&10));
    assert_eq!(offsets.get(1), Some(&11));
    assert_eq!(offsets.get(2), Some(&15));
    assert_eq!(offsets.last(), Some(&25));
    assert!(decode_offsets("a&unterminated", 0).is_none());
    assert!(decode_offsets("&#xD800;", 0).is_none());
}

// SCXML037 -------------------------------------------------------------------

#[test]
fn null_data_model_rejects_expressions_with_removal_and_switch_fixes() {
    let xml = machine(
        "null",
        r#"<state id="s"><onentry><log expr="1"/><script>x</script></onentry><transition cond="x &gt; 1" target="t"/><transition cond="In('s')" event="e" target="t"/></state><final id="t"/>"#,
    );
    let report = validate_semantics(&parse(&xml));
    let found: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML037" | "SCXML079"))
        .collect();
    assert_eq!(found.len(), 3, "{report:?}");
    // SCXML 1.0 §B.1.7: `<script>` is not supported (an error); §5.9.4 gives an
    // ill-formed expression a run-time outcome (a warning).
    for diagnostic in &found {
        let expected = if diagnostic.code == "SCXML037" {
            Severity::Error
        } else {
            Severity::Warning
        };
        assert_eq!(diagnostic.severity, expected, "{diagnostic:?}");
    }
    let log = found
        .iter()
        .find(|diagnostic| text(&xml, diagnostic.primary.span) == r#"expr="1""#)
        .expect("log expr diagnostic");
    assert!(log.message.contains("value-expression"), "{log:?}");
    assert_eq!(log.fixes.len(), 2);
    let removal = log
        .fixes
        .iter()
        .find(|fix| fix.message.starts_with("remove"))
        .expect("removal fix");
    let removed = {
        let edit = &removal.edits[0];
        let mut output = xml.clone();
        output.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        output
    };
    assert!(removed.contains("<log/>"), "{removed}");
    let cond = found
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`cond`"))
        .expect("cond diagnostic");
    assert_eq!(
        cond.fixes.len(),
        1,
        "conditions only offer the data-model switch"
    );
    assert!(
        cond.secondary
            .iter()
            .any(|label| text(&xml, label.span) == "null")
    );
}

/// SCXML 1.0 §B.1.7: "The <foreach> element and the elements defined in 5
/// Data Model and Data Manipulation are not supported in the Null Data
/// Model." Appendix A.1: a conformant document "MUST conform to any
/// additional syntactic constraints that are defined for the data model
/// that it has chosen." Each unsupported element is one error at its start
/// tag; what it contains is not reported again.
#[test]
fn null_data_model_rejects_the_elements_it_does_not_support() {
    for (body, element) in [
        (
            r#"<datamodel><data id="d"/></datamodel><state id="s"/>"#,
            "datamodel",
        ),
        (
            r#"<state id="s"><datamodel><data id="d"/></datamodel></state>"#,
            "datamodel",
        ),
        (
            r#"<state id="s"><onentry><assign location="d"/></onentry></state>"#,
            "assign",
        ),
        (
            r#"<final id="s"><donedata><content/></donedata></final>"#,
            "donedata",
        ),
        (
            r#"<state id="s"><onentry><send event="e"><content>text</content></send></onentry></state>"#,
            "content",
        ),
        (
            r#"<state id="s"><invoke src="child.scxml"><param name="p" location="d"/></invoke></state>"#,
            "param",
        ),
        (
            r#"<state id="s"><onentry><script src="lib.js"/></onentry></state>"#,
            "script",
        ),
        (
            r#"<state id="s"><onentry><foreach array="a" item="i"/></onentry></state>"#,
            "foreach",
        ),
        (
            r#"<state id="s"><invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="c"/></scxml></content></invoke></state>"#,
            "content",
        ),
    ] {
        let xml = machine("null", body);
        let report = validate_semantics(&parse(&xml));
        let found: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML037" | "SCXML079"))
            .collect();
        assert_eq!(found.len(), 1, "{body}: {report:?}");
        let diagnostic = found[0];
        assert_eq!(diagnostic.severity, Severity::Error, "{body}");
        assert_eq!(
            diagnostic.message,
            format!("the null data model does not support `<{element}>`"),
            "{body}"
        );
        assert!(
            text(&xml, diagnostic.primary.span).starts_with(&format!("<{element}")),
            "{body}: {diagnostic:?}"
        );
        assert!(
            diagnostic
                .secondary
                .iter()
                .any(|label| text(&xml, label.span) == "null"),
            "{body}"
        );
        assert!(
            diagnostic
                .fixes
                .iter()
                .any(|fix| fix.message == "select the ECMAScript data model"),
            "{body}"
        );
    }
    // The same elements are legal under a data model that supports them.
    let xml = machine(
        "ecmascript",
        r#"<datamodel><data id="d"/></datamodel><state id="s"><onentry><assign location="d" expr="1"/></onentry></state>"#,
    );
    let report = validate_semantics(&parse(&xml));
    assert!(
        !report
            .diagnostics()
            .iter()
            .any(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML037" | "SCXML079")),
        "{report:?}"
    );
}

#[test]
fn null_in_predicate_forms_are_accepted() {
    // Exactly the null data-model component's grammar: the state ID is
    // quoted (`tests/null_datamodel_agreement.rs` cross-checks the two).
    for accepted in [" In( 's' ) ", "In(\"s.x\")", "In('s)')", "In(\"it's\")"] {
        assert!(is_null_in_predicate(accepted), "{accepted}");
    }
    for rejected in [
        "In(s)",
        "In(s) && In(t)",
        "In()",
        "in(s)",
        "In('a b')",
        "In('')",
        "In('s\\')",
        "In('s'",
        "x",
    ] {
        assert!(!is_null_in_predicate(rejected), "{rejected}");
    }
}

#[test]
fn namelist_tokens_are_decoded_and_mapped_individually() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><onentry><send event="e" namelist="ok &#34;@bad"/></onentry></state>"#,
    );
    let checker = MarkerChecker::default();
    let report = validate_expressions(&parse(&xml), &checker);
    let calls = checker.calls.lock().expect("calls").clone();
    assert_eq!(
        calls,
        vec![
            (ExpressionKind::Location, "ok".to_owned()),
            (ExpressionKind::Location, "\"@bad".to_owned())
        ]
    );
    assert_eq!(report.diagnostics().len(), 1);
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.code, "SCXML039");
    assert_eq!(text(&xml, diagnostic.primary.span), "@");
    assert_eq!(
        text(&xml, diagnostic.secondary[0].span),
        "&#34;@bad",
        "the whole token is the secondary label"
    );
}

#[test]
fn empty_expressions_are_a_warning_with_attribute_removal() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><onentry><log label="x" expr=""/></onentry></state>"#,
    );
    let checker = MarkerChecker::default();
    let report = validate_expressions(&parse(&xml), &checker);
    assert_eq!(report.diagnostics().len(), 1, "{report:?}");
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.code, "LINT009");
    assert_eq!(diagnostic.severity, Severity::Warning);
    assert_eq!(text(&xml, diagnostic.primary.span), r#"expr="""#);
    let edit = &diagnostic.fixes[0].edits[0];
    let mut fixed = xml.clone();
    fixed.replace_range(edit.span.start..edit.span.end, &edit.replacement);
    assert!(fixed.contains(r#"<log label="x"/>"#), "{fixed}");
    assert!(checker.calls.lock().expect("calls").is_empty());
}

#[test]
fn only_a_blank_log_expr_is_absent_so_only_its_removal_is_exact() {
    // Lowering maps a blank `<log expr>` to the absent attribute (W3C IRP
    // 307), so its removal fix preserves behavior. A blank `<transition
    // cond>` is an invalid expression (SCXML 3.13, 5.9): it raises
    // `error.execution` and is false, so removing it (unconditional) is only
    // maybe-correct, like every other empty expression.
    let checker = MarkerChecker::default();
    for (body, applicability, treated_as_absent) in [
        (
            r#"<state id="s"><onentry><log label="x" expr=" "/></onentry></state>"#,
            Applicability::MachineApplicable,
            true,
        ),
        (
            r#"<state id="s"><transition cond="" target="s"/></state>"#,
            Applicability::MaybeIncorrect,
            false,
        ),
        (
            r#"<state id="s"><onentry><send event="e" delayexpr=""/></onentry></state>"#,
            Applicability::MaybeIncorrect,
            false,
        ),
    ] {
        let xml = machine("ecmascript", body);
        let report = validate_expressions(&parse(&xml), &checker);
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == "LINT009")
            .unwrap_or_else(|| panic!("{body}: empty expression warning: {report:?}"));
        assert_eq!(
            diagnostic.message.ends_with("is treated as absent"),
            treated_as_absent,
            "{body}: {}",
            diagnostic.message
        );
        assert_eq!(diagnostic.fixes.len(), 1, "{body}");
        assert_eq!(diagnostic.fixes[0].applicability, applicability, "{body}");
    }
}

#[test]
fn unknown_elements_are_not_expression_sites() {
    // vdx/05: `<lgo expr>` is a schema error (did you mean `<log>`); its
    // attribute must not also drive LINT010 or a syntax check.
    let missing = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="s"><onentry><lgo expr="'hi'"/></onentry></state></scxml>"#
    );
    assert!(
        !validate_semantics(&parse(&missing))
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "LINT010"),
        "unknown element must not be an expression site"
    );
    let with_checker = machine(
        "ecmascript",
        r#"<state id="s"><onentry><lgo expr="@"/><send sendidexpr="@"/></onentry></state>"#,
    );
    let checker = MarkerChecker::default();
    assert!(
        validate_expressions(&parse(&with_checker), &checker)
            .diagnostics()
            .is_empty()
    );
    assert!(checker.calls.lock().expect("calls").is_empty());
}

#[test]
fn foreach_item_is_worded_as_a_variable_name_with_a_prefix_fix() {
    // vdx/25: `item="1bad"` must read as a variable-name error, not parser
    // internals, and offer a reviewable `item1bad` repair.
    #[derive(Debug)]
    struct DigitChecker;
    impl ExpressionSyntaxChecker for DigitChecker {
        fn data_model(&self) -> &'static str {
            "ecmascript"
        }
        fn check(&self, _kind: ExpressionKind, source: &str) -> ExpressionCheck {
            if source.starts_with('1') {
                ExpressionCheck::Invalid(vec![SyntaxIssue {
                    start: 0,
                    end: 1,
                    message: "a numeric literal must not be followed by an alphanumeric, $ or _ characters".to_owned(),
                }])
            } else {
                ExpressionCheck::Valid
            }
        }

        fn check_effects(
            &self,
            _fragments: &[super::EffectFragment<'_>],
            _scripts: ExternalScripts,
        ) -> super::EffectCheck {
            super::EffectCheck::Checked(Vec::new())
        }
    }
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><onentry><foreach array="[1]" item="1bad"/></onentry></state>"#,
    );
    let report = validate_expressions(&parse(&xml), &DigitChecker);
    assert_eq!(report.diagnostics().len(), 1, "{report:?}");
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.code, "SCXML039");
    assert!(
        diagnostic.message.contains("starts with a digit"),
        "{diagnostic:?}"
    );
    assert!(
        !diagnostic.message.contains("numeric literal"),
        "{diagnostic:?}"
    );
    // The ECMAScript data model reserves a leading `_`, so the repair
    // prefixes the attribute name instead.
    assert_eq!(diagnostic.fixes[0].edits[0].replacement, "item1bad");
    assert_eq!(
        diagnostic.fixes[0].applicability,
        Applicability::MaybeIncorrect
    );
}

/// Applies one fix's edits to `xml`.
fn apply(xml: &str, fix: &Fix) -> String {
    let mut edits: Vec<_> = fix.edits.iter().collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
    let mut output = xml.to_owned();
    for edit in edits {
        output.replace_range(edit.span.start..edit.span.end, &edit.replacement);
    }
    output
}

#[test]
fn null_data_model_reports_one_finding_per_element_with_schema_safe_fixes() {
    let xml = machine(
        "null",
        r#"<state id="s"><onentry><assign location="count" expr="0"/></onentry><transition event="go" target="t"/></state><final id="t"/>"#,
    );
    let report = validate_semantics(&parse(&xml));
    let found: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML037" | "SCXML079"))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "one `<assign>` mistake is one finding: {found:?}"
    );
    let assign = found[0];
    assert!(
        assign.message.contains("does not support `<assign>`"),
        "the unsupported element is the one mistake: {assign:?}"
    );
    let removal = assign
        .fixes
        .iter()
        .find(|fix| fix.message.starts_with("remove"))
        .expect("removal fix");
    let fixed = apply(&xml, removal);
    assert!(
        fixed.contains("<onentry></onentry>"),
        "`location` is required, so the whole `<assign>` is removed: {fixed}"
    );

    for (body, removed, kept) in [
        (
            r#"<state id="s"><onentry><send eventexpr="'e'"/></onentry></state>"#,
            "<send",
            "<onentry></onentry>",
        ),
        (
            r#"<state id="s"><onentry><send event="e"><param name="p" expr="1"/></send></onentry></state>"#,
            "<param",
            r#"<send event="e"></send>"#,
        ),
        (
            r#"<state id="s"><onentry><cancel sendidexpr="'x'"/></onentry></state>"#,
            "<cancel",
            "<onentry></onentry>",
        ),
    ] {
        let xml = machine("null", body);
        let report = validate_semantics(&parse(&xml));
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML037" | "SCXML079"))
            .expect("null data model diagnostic");
        let removal = diagnostic
            .fixes
            .iter()
            .find(|fix| fix.message.starts_with("remove"))
            .expect("removal fix");
        let fixed = apply(&xml, removal);
        assert!(!fixed.contains(removed), "{body}: {fixed}");
        assert!(fixed.contains(kept), "{body}: {fixed}");
    }

    // An optional attribute whose removal leaves a valid element is removed alone.
    let xml = machine(
        "null",
        r#"<state id="s"><onentry><send event="e" delayexpr="'1s'"/></onentry></state>"#,
    );
    let report = validate_semantics(&parse(&xml));
    let diagnostic = report
        .diagnostics()
        .iter()
        .find(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML037" | "SCXML079"))
        .expect("delayexpr error");
    let removal = diagnostic
        .fixes
        .iter()
        .find(|fix| fix.message.starts_with("remove"))
        .expect("removal fix");
    assert!(apply(&xml, removal).contains(r#"<send event="e"/>"#));
}

#[test]
fn empty_expression_fixes_never_remove_a_required_attribute() {
    let checker = MarkerChecker::default();
    for (body, element) in [
        // Blank but not empty: an empty required attribute is the
        // `SCXML066` error instead (asserted below).
        (
            r#"<state id="s"><onentry><if cond=" "><log expr="1"/></if></onentry></state>"#,
            "if",
        ),
        (
            r#"<state id="s"><onentry><if cond="true"/><elseif cond=" "/></onentry></state>"#,
            "elseif",
        ),
        (
            r#"<state id="s"><onentry><foreach array=" " item="i"/></onentry></state>"#,
            "foreach",
        ),
        (
            r#"<state id="s"><onentry><assign location="x" expr=""/></onentry></state>"#,
            "assign",
        ),
        (
            r#"<state id="s"><onentry><send event="e"><param name="p" expr=""/></send></onentry></state>"#,
            "param",
        ),
    ] {
        let xml = machine("ecmascript", body);
        let report = validate_expressions(&parse(&xml), &checker);
        let diagnostic = report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == "LINT009")
            .unwrap_or_else(|| panic!("{body}: empty expression warning: {report:?}"));
        assert!(
            diagnostic.fixes.is_empty(),
            "{body}: removing a required attribute would create a schema or semantic error: {:?}",
            diagnostic.fixes
        );
        let help = diagnostic.help.as_deref().unwrap_or_default();
        assert!(
            !help.contains("unconditional") || element == "transition",
            "{body}: help must fit `<{element}>`: {help}"
        );
    }
    for body in [
        r#"<state id="s"><onentry><if cond=""><log expr="1"/></if></onentry></state>"#,
        r#"<state id="s"><onentry><foreach array="" item="i"/></onentry></state>"#,
        r#"<state id="s"><onentry><foreach array="[1]" item=""/></onentry></state>"#,
        r#"<state id="s"><onentry><assign location="" expr="1"/></onentry></state>"#,
    ] {
        let report = validate_expressions(&parse(&machine("ecmascript", body)), &checker);
        assert!(
            report
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code != "LINT009"),
            "{body}: an empty required attribute is SCXML066, not also LINT009: {report:?}"
        );
    }
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="" target="s"/></state>"#,
    );
    let report = validate_expressions(&parse(&xml), &checker);
    let diagnostic = &report.diagnostics()[0];
    assert!(
        diagnostic
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("unconditional"),
        "{diagnostic:?}"
    );
    assert_eq!(diagnostic.fixes.len(), 1, "`<transition cond>` is optional");
}

/// Returns a fixed checker message for chosen sources (Boa's wording as
/// relayed by the ECMAScript data model, including its synthetic `)`).
#[derive(Debug)]
struct ScriptedChecker(Vec<(&'static str, usize, usize, &'static str)>);

impl ExpressionSyntaxChecker for ScriptedChecker {
    fn data_model(&self) -> &'static str {
        "ecmascript"
    }

    fn check(&self, _kind: ExpressionKind, source: &str) -> ExpressionCheck {
        match self.0.iter().find(|(scripted, ..)| *scripted == source) {
            Some((_, start, end, message)) => ExpressionCheck::Invalid(vec![SyntaxIssue {
                start: *start,
                end: *end,
                message: (*message).to_owned(),
            }]),
            None => ExpressionCheck::Valid,
        }
    }

    fn check_effects(
        &self,
        _fragments: &[super::EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> super::EffectCheck {
        super::EffectCheck::Checked(Vec::new())
    }
}

#[test]
fn wrapper_tokens_never_leak_and_common_mistakes_get_targeted_repairs() {
    let checker = ScriptedChecker(vec![
        ("hello world", 6, 7, "expected `)`, `,`, found `world`"),
        (
            "ok === true and x",
            12,
            13,
            "expected `)`, `,`, found `and`",
        ),
        ("a or b", 2, 3, "expected `)`, `,`, found `or`"),
        ("'it's done'", 4, 5, "expected `)`, `,`, found `s`"),
        ("a1,b1", 0, 5, "expected a variable name or property access"),
    ]);
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><onentry><log expr="hello world"/><log expr="'it's done'"/><send event="t" namelist="a1,b1"/></onentry><transition cond="ok === true and x" target="s"/><transition cond="a or b" target="s"/></state>"#,
    );
    let report = validate_expressions(&parse(&xml), &checker);
    let by_source = |needle: &str| {
        report
            .diagnostics()
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .secondary
                    .iter()
                    .chain(std::iter::once(&diagnostic.primary))
                    .any(|label| text(&xml, label.span).contains(needle))
            })
            .unwrap_or_else(|| panic!("diagnostic for {needle}: {report:?}"))
    };
    for diagnostic in report.diagnostics() {
        assert!(
            !diagnostic.message.contains("`)`"),
            "the checker's wrapping parenthesis must not leak: {}",
            diagnostic.message
        );
    }

    let prose = by_source("hello world");
    assert!(
        prose.message.contains("unexpected `world`"),
        "{}",
        prose.message
    );
    let fixed = apply(&xml, &prose.fixes[0]);
    assert!(fixed.contains(r#"<log expr="'hello world'"/>"#), "{fixed}");

    let and = by_source("true and x");
    assert!(
        and.help.as_deref().unwrap_or_default().contains("&&"),
        "{and:?}"
    );
    assert!(apply(&xml, &and.fixes[0]).contains(r#"cond="ok === true &amp;&amp; x""#));

    let or = by_source("a or b");
    assert!(apply(&xml, &or.fixes[0]).contains(r#"cond="a || b""#));

    let quote = by_source("'it's done'");
    assert!(
        quote.help.as_deref().unwrap_or_default().contains(r"\'"),
        "{quote:?}"
    );
    assert!(apply(&xml, &quote.fixes[0]).contains(r#"<log expr="'it\'s done'"/>"#));

    let namelist = by_source("a1,b1");
    assert!(namelist.message.contains("comma"), "{}", namelist.message);
    assert_eq!(
        namelist.fixes[0].applicability,
        Applicability::MachineApplicable
    );
    assert!(apply(&xml, &namelist.fixes[0]).contains(r#"namelist="a1 b1""#));
}

/// Scripted effect analysis: every fragment parses; `check_effects` records
/// the request and answers `findings`.
#[derive(Debug, Default)]
struct EffectChecker {
    findings: Vec<EffectFinding>,
    requests: Mutex<Vec<EffectRequest>>,
}

/// One recorded `check_effects` request: each fragment's role and source,
/// and whether the machine has an unread script.
type EffectRequest = (Vec<(String, String)>, ExternalScripts);

impl ExpressionSyntaxChecker for EffectChecker {
    fn data_model(&self) -> &'static str {
        "ecmascript"
    }

    fn check(&self, _kind: ExpressionKind, _source: &str) -> ExpressionCheck {
        ExpressionCheck::Valid
    }

    fn check_effects(
        &self,
        fragments: &[EffectFragment<'_>],
        scripts: ExternalScripts,
    ) -> EffectCheck {
        let request = fragments
            .iter()
            .map(|fragment| (format!("{:?}", fragment.role), fragment.source.to_owned()))
            .collect();
        if let Ok(mut requests) = self.requests.lock() {
            requests.push((request, scripts));
        }
        EffectCheck::Checked(self.findings.clone())
    }
}

fn effect_span(fragment: usize, start: usize, end: usize) -> EffectSpan {
    EffectSpan {
        fragment,
        start,
        end,
    }
}

#[test]
fn every_fragment_reaches_the_effect_analysis_with_its_role() {
    let checker = EffectChecker::default();
    let xml = machine(
        "ecmascript",
        r#"<datamodel><data id="f" expr="function () { return 1; }"/></datamodel><state id="s"><onentry><assign location="d" expr="2"/><log expr="d"/><script>var x = 1;</script><script src="lib.js"/></onentry><transition cond="f()" target="s"/></state>"#,
    );
    let _ = validate_expressions(&parse(&xml), &checker);
    let requests = checker.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let (fragments, scripts) = &requests[0];
    assert_eq!(
        *scripts,
        ExternalScripts::Present,
        "a `<script src>` is reported as unread"
    );
    let roles: Vec<(&str, &str)> = fragments
        .iter()
        .map(|(role, source)| (role.as_str(), source.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("Data(\"f\")", "function () { return 1; }"),
            ("Location", "d"),
            ("Value", "2"),
            ("Log", "d"),
            ("Script", "var x = 1;"),
            ("Condition", "f()"),
        ]
    );
}

#[test]
fn a_certain_condition_effect_is_a_warning_with_the_comparison_fix() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="x = 2" target="s"/></state>"#,
    );
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: true,
            kind: EffectKind::Assignment,
            span: effect_span(0, 0, 1),
            subject: "x".to_owned(),
            cause: None,
            operator: Some(effect_span(0, 2, 3)),
        }],
        ..EffectChecker::default()
    };
    let report = validate_expressions(&parse(&xml), &checker);
    let [error] = report.diagnostics() else {
        panic!("one diagnostic expected: {report:?}");
    };
    // §5.9.1's SHOULD NOT governs documents (Appendix B's MUST contradicts
    // official IRP test 444; docs/spec-errata.md).
    assert_eq!(
        (error.code.as_str(), error.severity),
        ("LINT020", Severity::Warning)
    );
    assert_eq!(error.message, "`cond` on `<transition>` has a side effect");
    assert_eq!(text(&xml, error.primary.span), "=");
    assert_eq!(
        error.primary.message.as_deref(),
        Some("`=` assigns `x` instead of comparing it")
    );
    assert!(apply(&xml, &error.fixes[0]).contains(r#"cond="x === 2""#));
}

#[test]
fn a_call_effect_labels_its_cause_in_the_script() {
    let xml = machine(
        "ecmascript",
        r#"<script>function bump() { count++; return true; }</script><state id="s"><onentry><log expr="bump()"/></onentry></state>"#,
    );
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: true,
            kind: EffectKind::FunctionCall,
            span: effect_span(1, 0, 6),
            subject: "bump".to_owned(),
            cause: Some(EffectCause {
                kind: EffectKind::Update,
                span: effect_span(0, 18, 23),
                subject: "count".to_owned(),
                function: "bump".to_owned(),
            }),
            operator: None,
        }],
        ..EffectChecker::default()
    };
    let report = validate_expressions(&parse(&xml), &checker);
    let [error] = report.diagnostics() else {
        panic!("one diagnostic expected: {report:?}");
    };
    assert_eq!(error.message, "`expr` on `<log>` has a side effect");
    assert_eq!(text(&xml, error.primary.span), "bump()");
    assert_eq!(error.secondary.len(), 1);
    assert_eq!(text(&xml, error.secondary[0].span), "count");
    assert_eq!(
        error.secondary[0].message.as_deref(),
        Some("`bump` changes `count` here")
    );
    assert!(
        error
            .spec_references
            .iter()
            .any(|reference| reference.contains("4.7.3"))
    );
}

/// The script files `scripts` names, as a caller that read them supplies.
fn script_files(files: &[(&str, &str)]) -> ScriptResources {
    files
        .iter()
        .map(|(src, text)| ((*src).to_owned(), (*text).to_owned()))
        .collect()
}

#[test]
fn a_supplied_script_src_file_reaches_the_effect_analysis_as_a_script() {
    let checker = EffectChecker::default();
    let xml = machine(
        "ecmascript",
        r#"<script src=" lib.js "/><state id="s"><onentry><script src="lib.js"/></onentry><transition cond="f()" target="s"/></state>"#,
    );
    let scripts = script_files(&[("lib.js", "function f() { return true; }")]);
    let _ = validate_expressions_with_scripts(&parse(&xml), &[&checker], &scripts);
    let requests = checker.requests.lock().expect("requests lock");
    let [(fragments, unread)] = requests.as_slice() else {
        panic!("one request expected: {requests:?}");
    };
    assert_eq!(
        *unread,
        ExternalScripts::Absent,
        "every `<script src>` was read, so none is reported as unread"
    );
    let roles: Vec<(&str, &str)> = fragments
        .iter()
        .map(|(role, source)| (role.as_str(), source.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("Condition", "f()"),
            ("Script", "function f() { return true; }"),
        ],
        "the file follows the document's fragments, once however often it is named"
    );
}

#[test]
fn a_script_src_without_its_file_stays_unread() {
    let checker = EffectChecker::default();
    let xml = machine(
        "ecmascript",
        r#"<script src="lib.js"/><script src="other.js"/><state id="s"><transition cond="f()" target="s"/></state>"#,
    );
    let scripts = script_files(&[("lib.js", "function f() { return true; }")]);
    let _ = validate_expressions_with_scripts(&parse(&xml), &[&checker], &scripts);
    let requests = checker.requests.lock().expect("requests lock");
    let [(fragments, unread)] = requests.as_slice() else {
        panic!("one request expected: {requests:?}");
    };
    assert_eq!(
        *unread,
        ExternalScripts::Present,
        "`other.js` was not supplied, so the machine still has an unread script"
    );
    assert_eq!(
        fragments.len(),
        2,
        "the condition and `lib.js`: {fragments:?}"
    );
}

#[test]
fn a_call_effect_in_a_script_src_file_labels_its_src() {
    let xml = machine(
        "ecmascript",
        r#"<script src="lib.js"/><state id="s"><onentry><log expr="bump()"/></onentry></state>"#,
    );
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: true,
            kind: EffectKind::FunctionCall,
            span: effect_span(0, 0, 6),
            subject: "bump".to_owned(),
            cause: Some(EffectCause {
                kind: EffectKind::Update,
                span: effect_span(1, 18, 23),
                subject: "count".to_owned(),
                function: "bump".to_owned(),
            }),
            operator: None,
        }],
        ..EffectChecker::default()
    };
    let scripts = script_files(&[("lib.js", "function bump() { count++; return true; }")]);
    let report = validate_expressions_with_scripts(&parse(&xml), &[&checker], &scripts);
    let [error] = report.diagnostics() else {
        panic!("one diagnostic expected: {report:?}");
    };
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("SCXML074", "`expr` on `<log>` has a side effect")
    );
    assert_eq!(text(&xml, error.primary.span), "bump()");
    assert_eq!(error.secondary.len(), 1);
    assert_eq!(text(&xml, error.secondary[0].span), "lib.js");
    assert_eq!(
        error.secondary[0].message.as_deref(),
        Some("`bump` changes `count` here (in `lib.js`)")
    );
}

#[test]
fn a_system_variable_write_in_a_script_src_file_is_reported_at_its_src() {
    let xml = machine("ecmascript", r#"<script src="lib.js"/><state id="s"/>"#);
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: true,
            kind: EffectKind::SystemVariableWrite,
            span: effect_span(0, 7, 13),
            subject: "_event".to_owned(),
            cause: None,
            operator: None,
        }],
        ..EffectChecker::default()
    };
    let scripts = script_files(&[("lib.js", "delete _event;")]);
    let report = validate_expressions_with_scripts(&parse(&xml), &[&checker], &scripts);
    let [error] = report.diagnostics() else {
        panic!("one diagnostic expected: {report:?}");
    };
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        (
            "SCXML077",
            "the `<script src>` file `lib.js` changes the system variable `_event`"
        )
    );
    assert_eq!(text(&xml, error.primary.span), "lib.js");
}

#[test]
fn a_write_of_a_system_variable_is_a_warning_in_any_fragment() {
    let xml = machine(
        "ecmascript",
        r#"<script>delete _event;</script><state id="s"/>"#,
    );
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: true,
            kind: EffectKind::SystemVariableWrite,
            span: effect_span(0, 7, 13),
            subject: "_event".to_owned(),
            cause: None,
            operator: None,
        }],
        ..EffectChecker::default()
    };
    let report = validate_expressions(&parse(&xml), &checker);
    let [error] = report.diagnostics() else {
        panic!("one diagnostic expected: {report:?}");
    };
    assert_eq!(
        (error.code.as_str(), error.severity),
        ("SCXML077", Severity::Warning)
    );
    assert_eq!(
        error.message,
        "`<script>` changes the system variable `_event`"
    );
    assert_eq!(text(&xml, error.primary.span), "_event");
    // Only a system-variable write may be reported for an unchecked fragment.
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: true,
            kind: EffectKind::Assignment,
            span: effect_span(0, 7, 13),
            subject: "_event".to_owned(),
            cause: None,
            operator: None,
        }],
        ..EffectChecker::default()
    };
    let report = validate_expressions(&parse(&xml), &checker);
    assert!(report.diagnostics().is_empty(), "{report:?}");
}

#[test]
fn an_undecidable_effect_is_a_warning() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="foo()" target="s"/></state>"#,
    );
    let checker = EffectChecker {
        findings: vec![EffectFinding {
            certain: false,
            kind: EffectKind::UnknownCall,
            span: effect_span(0, 0, 5),
            subject: "foo".to_owned(),
            cause: None,
            operator: None,
        }],
        ..EffectChecker::default()
    };
    let report = validate_expressions(&parse(&xml), &checker);
    let [warning] = report.diagnostics() else {
        panic!("one diagnostic expected: {report:?}");
    };
    assert_eq!(
        (warning.code.as_str(), warning.severity),
        ("LINT019", Severity::Warning)
    );
    assert_eq!(
        warning.primary.message.as_deref(),
        Some("calls `foo`, which the validator cannot see")
    );
}

#[test]
fn an_answer_outside_the_contract_is_declined() {
    let xml = machine(
        "ecmascript",
        r#"<state id="s"><transition cond="x" target="s"/></state>"#,
    );
    let finding = |span: EffectSpan, subject: String| EffectFinding {
        certain: true,
        kind: EffectKind::Assignment,
        span,
        subject,
        cause: None,
        operator: None,
    };
    for bad in [
        // past the end of its fragment
        finding(effect_span(0, 0, 9), "x".to_owned()),
        // a fragment that does not exist
        finding(effect_span(3, 0, 1), "x".to_owned()),
        // an over-long subject
        finding(effect_span(0, 0, 1), "x".repeat(257)),
    ] {
        let checker = EffectChecker {
            findings: vec![bad],
            ..EffectChecker::default()
        };
        let report = validate_expressions(&parse(&xml), &checker);
        let [warning] = report.diagnostics() else {
            panic!("one diagnostic expected: {report:?}");
        };
        assert_eq!(warning.code, "LINT019");
        assert_eq!(
            warning.primary.message.as_deref(),
            Some("the linked checker declined the side-effect analysis")
        );
    }
}

#[derive(Debug)]
struct NoSchema;

impl crate::SchemaResolver for NoSchema {
    fn resolve(
        &self,
        _namespace_uri: &str,
    ) -> Result<crate::SchemaSource, crate::SchemaResolveError> {
        Err(crate::SchemaResolveError::NotFound)
    }
}

#[derive(Debug)]
struct NoBackend;

impl crate::SchemaValidator for NoBackend {
    fn validate(
        &self,
        _document: &Document,
        _schemas: &[crate::SchemaSource],
    ) -> Result<Report, crate::SchemaBackendError> {
        Ok(Report::new())
    }
}

/// A machine without `datamodel` is validated exactly as one that declares
/// the default, `ecmascript`: the same findings, and no `LINT010` warning
/// or declaration fix.
#[test]
fn a_machine_without_datamodel_is_validated_as_the_default_data_model() {
    let source = |attribute: &str| {
        format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"{attribute}><state id="s"><onentry><log expr="1 @ 2"/></onentry><transition event="e" target="s"/></state></scxml>"#
        )
    };
    let findings = |xml: String| {
        crate::Validator::new(NoSchema, NoBackend)
            .with_schema_mode(crate::SchemaMode::Off)
            .with_expression_checker(std::sync::Arc::new(MarkerChecker::default()))
            .with_expression_checker(std::sync::Arc::new(XPathMarkerChecker::default()))
            .validate_source(SOURCE, "machine.scxml", xml)
            .report
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.code.clone(), diagnostic.message.clone()))
            .collect::<Vec<_>>()
    };
    let missing = findings(source(""));
    assert!(
        missing.iter().any(|(code, _)| code == "SCXML038"),
        "the ECMAScript checker ran: {missing:?}"
    );
    assert!(
        !missing.iter().any(|(code, _)| code == "LINT010"),
        "{missing:?}"
    );
    assert_eq!(missing, findings(source(r#" datamodel="ecmascript""#)));
}

/// SCXML 1.0 §5.10: "A conformant SCXML document MUST NOT contain ids
/// beginning with '_' in the <data> element." Under every data model other
/// than null (`SCXML037`).
#[test]
fn data_ids_beginning_with_an_underscore_are_rejected_under_every_data_model() {
    for datamodel in [Some("ecmascript"), Some("xpath"), None, Some("other")] {
        let attribute =
            datamodel.map_or_else(String::new, |name| format!(r#" datamodel="{name}""#));
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"{attribute}><datamodel><data id="_count"/><data id="count2"/></datamodel><state id="s"/></scxml>"#
        );
        let report = validate_semantics(&parse(&xml));
        let found: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "SCXML073")
            .collect();
        assert_eq!(found.len(), 1, "{datamodel:?}: {report:?}");
        assert_eq!(text(&xml, found[0].primary.span), "_count");
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(
            apply(&xml, &found[0].fixes[0])
                .matches("id=\"count\"")
                .count(),
            1
        );
    }
    // Under the null data model `<data>` itself is `SCXML037`.
    let xml = machine(
        "null",
        r#"<datamodel><data id="_count"/></datamodel><state id="s"/>"#,
    );
    let report = validate_semantics(&parse(&xml));
    assert!(
        !report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "SCXML073"),
        "null: reported by its own rule"
    );
}
