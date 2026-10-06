use scxml_syntax::{SourceId, SourceText, parse_xml};
use scxml_types::ExternalScripts;

use crate::datamodel_check::{
    EffectCheck, EffectFragment, ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker,
    SyntaxIssue, validate_expressions,
};
use crate::{Diagnostic, SCXML_NAMESPACE};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

/// The real `XPath` data model's parse-only check.
#[derive(Debug)]
pub(super) struct XPathChecker;

impl ExpressionSyntaxChecker for XPathChecker {
    fn data_model(&self) -> &'static str {
        "xpath"
    }

    fn check(&self, kind: ExpressionKind, source: &str) -> ExpressionCheck {
        let kind = match kind {
            ExpressionKind::Condition => scxml_data_model_xpath::SyntaxKind::Condition,
            ExpressionKind::Value => scxml_data_model_xpath::SyntaxKind::Value,
            ExpressionKind::Location => scxml_data_model_xpath::SyntaxKind::Location,
            ExpressionKind::Script => scxml_data_model_xpath::SyntaxKind::Script,
        };
        match scxml_data_model_xpath::check_syntax(kind, source) {
            Ok(issues) if issues.is_empty() => ExpressionCheck::Valid,
            Ok(issues) => ExpressionCheck::Invalid(
                issues
                    .into_iter()
                    .map(|issue| SyntaxIssue {
                        start: issue.start,
                        end: issue.end,
                        message: issue.message,
                    })
                    .collect(),
            ),
            Err(_) => ExpressionCheck::NotChecked,
        }
    }

    fn check_effects(
        &self,
        _fragments: &[EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> EffectCheck {
        EffectCheck::Checked(Vec::new())
    }
}

fn document(body: &str) -> String {
    format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="xpath"><datamodel><data id="count" expr="0"/><data id="done" expr="false()"/></datamodel>{body}</scxml>"#
    )
}

pub(super) fn diagnostics(xml: &str) -> Vec<Diagnostic> {
    let document = parse_xml(SourceText::new(SOURCE, "xpath.scx", xml.to_owned()))
        .expect("XPath fixture must be well-formed");
    validate_expressions(&document, &XPathChecker)
        .diagnostics()
        .to_vec()
}

pub(super) fn apply(xml: &str, diagnostic: &Diagnostic, fix: usize) -> String {
    let fix = &diagnostic.fixes[fix];
    let mut fixed = xml.to_owned();
    for edit in fix.edits.iter().rev() {
        fixed.replace_range(edit.span.start..edit.span.end, &edit.replacement);
    }
    fixed
}

/// Checks one broken fragment: exactly one finding whose message and help
/// contain the expected text, and whose every fix yields `repaired` (in
/// order) and leaves no finding at all.
fn check(body: &str, message: &str, help: &str, repaired: &[&str]) {
    let xml = document(body);
    let found = diagnostics(&xml);
    assert_eq!(found.len(), 1, "{body}: {found:#?}");
    let diagnostic = &found[0];
    assert!(
        diagnostic.message.contains(message),
        "{body}: message {:?}",
        diagnostic.message
    );
    let actual_help = diagnostic.help.as_deref().unwrap_or_default();
    assert!(actual_help.contains(help), "{body}: help {actual_help:?}");
    assert!(
        diagnostic
            .spec_references
            .iter()
            .any(|spec| spec.contains("XPath Data Model")),
        "{body}: {:?}",
        diagnostic.spec_references
    );
    assert_eq!(
        diagnostic.fixes.len(),
        repaired.len(),
        "{body}: {:#?}",
        diagnostic.fixes
    );
    for (index, expected) in repaired.iter().enumerate() {
        let fixed = apply(&xml, diagnostic, index);
        assert!(fixed.contains(expected), "{body}: fix {index} gave {fixed}");
        let after = diagnostics(&fixed);
        assert!(after.is_empty(), "{body}: fix {index} left {after:#?}");
    }
}

#[test]
fn ecmascript_habits_get_targeted_xpath_repairs() {
    check(
        r#"<state id="a"><transition cond="$count == 3" target="a"/></state>"#,
        "`==` is not an XPath operator",
        "single `=`",
        &[r#"cond="$count = 3""#],
    );
    check(
        r#"<state id="a"><transition cond="$count === 3" target="a"/></state>"#,
        "`===` is not an XPath operator",
        "single `=`",
        &[r#"cond="$count = 3""#],
    );
    check(
        r#"<state id="a"><transition cond="$count &gt; 1 &amp;&amp; $done" target="a"/></state>"#,
        "`&&` is not an XPath operator",
        "`and`",
        &[r#"cond="$count &gt; 1 and $done""#],
    );
    check(
        r#"<state id="a"><transition cond="$count&gt;1&amp;&amp;$done" target="a"/></state>"#,
        "`&&` is not an XPath operator",
        "`and`",
        &[r#"cond="$count&gt;1 and $done""#],
    );
    check(
        r#"<state id="a"><transition cond="$count = 1 || $done" target="a"/></state>"#,
        "`||` is not an XPath operator",
        "`or`",
        &[r#"cond="$count = 1 or $done""#],
    );
    check(
        r#"<state id="a"><transition cond="!$done" target="a"/></state>"#,
        "`!` is not an XPath operator",
        "`not()`",
        &[r#"cond="not($done)""#],
    );
    check(
        r#"<state id="a"><transition cond="!($done or $count = 1)" target="a"/></state>"#,
        "`!` is not an XPath operator",
        "`not()`",
        &[r#"cond="not($done or $count = 1)""#],
    );
    check(
        r#"<state id="a"><transition cond="not $done" target="a"/></state>"#,
        "`not` is a function in XPath",
        "parentheses",
        &[r#"cond="not($done)""#],
    );
    check(
        r#"<state id="a"><transition cond="$count = 1 and not $done" target="a"/></state>"#,
        "`not` is a function in XPath",
        "parentheses",
        &[r#"cond="$count = 1 and not($done)""#],
    );
    check(
        r#"<state id="a"><transition cond="$count AND $done" target="a"/></state>"#,
        "lowercase",
        "`and`",
        &[r#"cond="$count and $done""#],
    );
    check(
        r#"<state id="a"><transition cond="$count % 2 = 0" target="a"/></state>"#,
        "`%` is not an XPath operator",
        "`mod`",
        &[r#"cond="$count mod 2 = 0""#],
    );
}

#[test]
fn unknown_functions_suggest_the_xpath_function_that_does_the_job() {
    check(
        r#"<state id="a"><transition cond="length($done) &gt; 0" target="a"/></state>"#,
        "unknown XPath function `length()`",
        "`string-length()` (characters in a string) or `count()` (nodes in a node-set)",
        // The canonical report orders alternative fixes by message.
        &[
            r#"cond="count($done) &gt; 0""#,
            r#"cond="string-length($done) &gt; 0""#,
        ],
    );
    check(
        r#"<state id="a"><transition cond="cuont($done) = 1" target="a"/></state>"#,
        "unknown XPath function `cuont()`",
        "did you mean `count()`?",
        &[r#"cond="count($done) = 1""#],
    );
    check(
        r#"<state id="a"><transition cond="fn:concat('a', 'b') = 'ab'" target="a"/></state>"#,
        "namespace prefix `fn` is not declared",
        "without a prefix",
        &[r#"cond="concat('a', 'b') = 'ab'""#],
    );
    check(
        r#"<state id="a"><transition cond="contains('a')" target="a"/></state>"#,
        "wrong number of arguments to `contains()`",
        "exactly 2 arguments",
        &[],
    );
}

#[test]
fn spacing_commas_quotes_and_delimiters_are_worded_for_xpath() {
    check(
        r#"<state id="a"><transition cond="$count &gt; = 3" target="a"/></state>"#,
        "`> =` is not an operator",
        "`>=` without the space",
        &[r#"cond="$count &gt;= 3""#],
    );
    check(
        r#"<state id="a"><onentry><log expr="concat('n=', $count, )"/></onentry></state>"#,
        "trailing `,` before `)`",
        "remove the `,`",
        &[r#"expr="concat('n=', $count)""#],
    );
    check(
        r#"<state id="a"><onentry><log expr="'it's done'"/></onentry></state>"#,
        "the string literal ends early at this `'`",
        "no escape sequences",
        &[r#"expr="&quot;it's done&quot;""#],
    );
    let xml =
        document(r#"<state id="a"><transition cond="count($done &gt; 0" target="a"/></state>"#);
    let found = diagnostics(&xml);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .message
            .contains("expected `)` to close the function call"),
        "{}",
        found[0].message
    );
    assert!(
        found[0].fixes.is_empty(),
        "the closing position is ambiguous"
    );
    let opener = found[0]
        .secondary
        .iter()
        .find(|label| label.message.as_deref() == Some("this `(` is never closed"))
        .expect("the unclosed `(` is labeled");
    assert_eq!(&xml[opener.span.start..opener.span.end], "(");
}

#[test]
fn a_script_is_reported_as_unsupported_with_a_removal() {
    check(
        r#"<state id="a"><onentry><script>count = count + 1;</script></onentry></state>"#,
        "`<script>` is not supported by the XPath data model",
        "`<assign location=\"$var\"",
        &["<onentry></onentry>"],
    );
}
