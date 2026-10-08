use scxml_syntax::{SourceId, SourceText, parse_xml};

use super::InPredicateRule;
use crate::semantic::Rule;
use crate::{Diagnostic, Report, SCXML_NAMESPACE};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

fn findings(datamodel: Option<&str>, cond: &str) -> (String, Vec<Diagnostic>) {
    let datamodel = datamodel.map_or(String::new(), |value| format!(r#" datamodel="{value}""#));
    let xml = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"{datamodel} initial="idle"><datamodel><data id="target"/></datamodel><state id="idle"><transition cond="{cond}" target="busy"/></state><parallel id="busy"><state id="left"/><state id="right"/></parallel><final id="done"/></scxml>"#
    );
    let document = parse_xml(SourceText::new(SOURCE, "in.scxml", xml.clone()))
        .expect("fixture must be well-formed");
    let mut report = Report::new();
    InPredicateRule.check(&document, &mut report);
    (xml, report.diagnostics().to_vec())
}

fn fixed(xml: &str, diagnostic: &Diagnostic) -> String {
    let mut fixed = xml.to_owned();
    for edit in diagnostic.fixes[0].edits.iter().rev() {
        fixed.replace_range(edit.span.start..edit.span.end, &edit.replacement);
    }
    fixed
}

#[test]
fn an_unknown_literal_state_id_gets_a_did_you_mean_in_every_data_model() {
    for datamodel in [Some("ecmascript"), Some("xpath"), Some("null"), None] {
        let (xml, found) = findings(datamodel, "In('bussy')");
        assert_eq!(found.len(), 1, "{datamodel:?}: {found:#?}");
        assert_eq!(found[0].code, "LINT012");
        assert!(found[0].message.contains("`bussy`"), "{}", found[0].message);
        assert_eq!(found[0].help.as_deref(), Some("did you mean `busy`?"));
        assert!(fixed(&xml, &found[0]).contains(r#"cond="In('busy')""#));
    }
    let (_, found) = findings(Some("ecmascript"), "In('zzzzzzzz')");
    assert_eq!(found.len(), 1);
    assert!(found[0].fixes.is_empty(), "no near state ID");
}

#[test]
fn an_unquoted_state_id_is_quoted_in_ecmascript_and_xpath() {
    for datamodel in ["ecmascript", "xpath"] {
        let (xml, found) = findings(Some(datamodel), "In(left) &amp;&amp; x");
        let found: Vec<_> = found.into_iter().collect();
        assert_eq!(found.len(), 1, "{datamodel}: {found:#?}");
        assert!(
            found[0].message.contains("unquoted"),
            "{}",
            found[0].message
        );
        assert!(fixed(&xml, &found[0]).contains("In('left')"), "{datamodel}");
    }
}

#[test]
fn known_ids_variables_and_other_calls_are_not_reported() {
    for cond in [
        "In('left') || In(&quot;right&quot;)",
        "In(target)",
        "x.In('nope')",
        "'In(nope)'",
        "In('le' + 'ft')",
    ] {
        let (_, found) = findings(Some("ecmascript"), cond);
        assert!(found.is_empty(), "{cond}: {found:#?}");
    }
    // The null data model's `In(id)` form takes the ID unquoted.
    let (_, found) = findings(Some("null"), "In(done)");
    assert!(found.is_empty(), "{found:#?}");
    // An unrecognized data model is LIMIT005's; nothing is claimed.
    let (_, found) = findings(Some("lua"), "In('nope')");
    assert!(found.is_empty(), "{found:#?}");
}
