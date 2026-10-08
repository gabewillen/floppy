use scxml_syntax::{SourceId, SourceText, parse_xml};

use super::*;
use crate::{SCXML_NAMESPACE, Severity, validate_semantics};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

fn validate(body: &str) -> (String, Report) {
    let xml = format!(r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0">{body}</scxml>"#);
    let document = parse_xml(SourceText::new(SOURCE, "graph.scxml", xml.clone()))
        .expect("graph-rule XML fixture must parse");
    (xml, validate_semantics(&document))
}

fn with_code<'a>(report: &'a Report, code: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .collect()
}

fn text(xml: &str, span: Span) -> &str {
    xml.get(span.start..span.end)
        .expect("diagnostic span must lie inside the fixture")
}

fn apply(xml: &str, fix: &Fix) -> String {
    let mut output = xml.to_owned();
    for edit in fix.edits.iter().rev() {
        output.replace_range(edit.span.start..edit.span.end, &edit.replacement);
    }
    output
}

fn assert_complete(diagnostic: &Diagnostic) {
    assert!(diagnostic.primary.message.is_some(), "{diagnostic:?}");
    assert!(diagnostic.help.is_some(), "{diagnostic:?}");
    assert!(!diagnostic.spec_references.is_empty(), "{diagnostic:?}");
}

// LINT005 -------------------------------------------------------------------

#[test]
fn unreachable_state_is_reported_once_at_its_id() {
    let (xml, report) = validate(
        r#"<state id="a"><transition event="go" target="b"/></state><state id="b"/><state id="island"><state id="inner"/></state>"#,
    );
    let found = with_code(&report, "LINT005");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_complete(diagnostic);
    assert_eq!(diagnostic.severity, Severity::Warning);
    assert_eq!(text(&xml, diagnostic.primary.span), "island");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("its descendant state is unreachable as well")),
        "{diagnostic:?}"
    );
    assert!(
        diagnostic
            .secondary
            .iter()
            .any(|label| text(&xml, label.span) == "a"),
        "the parent's default entry must be labeled: {diagnostic:?}"
    );
}

#[test]
fn conditional_event_history_and_parallel_entries_count_as_reachable() {
    let (_, report) = validate(
        r#"<state id="a" initial="a1">
             <state id="a1"><transition event="e" cond="false" target="p"/><transition event="back" target="h"/></state>
             <history id="h" type="deep"><transition target="deep"/></history>
             <state id="a2"><state id="deep"/></state>
           </state>
           <parallel id="p"><state id="r1"/><state id="r2"><transition target="a"/></state></parallel>
           <state id="never"/>"#,
    );
    let names: Vec<_> = with_code(&report, "LINT005")
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    // Only `never` is untargeted; the history default makes `a2`/`deep` reachable.
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].contains("`never`"), "{names:?}");
}

#[test]
fn state_targeted_only_from_unreachable_state_labels_that_reference() {
    let (xml, report) = validate(
        r#"<state id="a"/><state id="b"><transition event="e" target="c"/></state><state id="c"/>"#,
    );
    let found = with_code(&report, "LINT005");
    assert_eq!(found.len(), 2, "{report:?}");
    let c = found
        .iter()
        .find(|diagnostic| text(&xml, diagnostic.primary.span) == "c")
        .expect("`c` is unreachable");
    assert!(
        c.secondary.iter().any(|label| {
            label.message.as_deref()
                == Some("targeted here, but the source state is itself unreachable")
        }),
        "{c:?}"
    );
}

// SCXML030 -------------------------------------------------------------------

#[test]
fn multi_target_whose_removal_would_strand_a_state_offers_no_fix() {
    // `a` and `b` are entered only by this transition: removing either token
    // would trade the SCXML030 error for a new LINT005, so no fix is offered
    // and the notes say why.
    let (xml, report) = validate(
        r#"<state id="s"><transition event="go" target="a b"/></state><state id="a"/><state id="b"/>"#,
    );
    let found = with_code(&report, "SCXML030");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_complete(diagnostic);
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(text(&xml, diagnostic.primary.span), "b");
    assert!(
        diagnostic
            .secondary
            .iter()
            .any(|label| text(&xml, label.span) == "a")
    );
    assert!(diagnostic.fixes.is_empty(), "{diagnostic:?}");
    for stranded in [
        "`b` (keeping `a`) would leave `b`",
        "`a` (keeping `b`) would leave `a`",
    ] {
        assert!(
            diagnostic.notes.iter().any(|note| note.contains(stranded)),
            "{diagnostic:?}"
        );
    }
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("add a transition that enters the other state")),
        "{diagnostic:?}"
    );
}

/// Codes of every finding in `report`, with multiplicity.
fn codes(report: &Report) -> Vec<&str> {
    let mut codes: Vec<&str> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    codes.sort_unstable();
    codes
}

#[test]
fn multi_target_removal_fix_strands_no_state() {
    // `b` is entered only here, so only removing `a` (also entered by
    // `back`) is offered; applying it resolves the error and adds nothing.
    let (xml, report) = validate(
        r#"<state id="s"><transition event="go" target="a b"/><transition event="back" target="a"/></state><state id="a"/><state id="b"/>"#,
    );
    let found = with_code(&report, "SCXML030");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_eq!(diagnostic.fixes.len(), 1, "{diagnostic:?}");
    assert_eq!(
        diagnostic.fixes[0].message,
        "remove the target `a`, keeping `b`"
    );
    assert_eq!(
        diagnostic.fixes[0].applicability,
        Applicability::MaybeIncorrect
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("removing `b` (keeping `a`) would leave `b` unreachable")),
        "{diagnostic:?}"
    );
    let fixed = apply(&xml, &diagnostic.fixes[0]);
    assert!(fixed.contains(r#"event="go" target="b""#), "{fixed}");
    let body = fixed
        .split_once('>')
        .and_then(|(_, rest)| rest.rsplit_once("</scxml>"))
        .map(|(body, _)| body)
        .expect("fixture keeps its root");
    let (_, after) = validate(body);
    let mut before = codes(&report);
    before.retain(|code| *code != "SCXML030");
    assert_eq!(
        codes(&after),
        before,
        "the fix must add no finding: {after:?}"
    );
}

#[test]
fn multi_target_with_two_safe_removals_offers_both() {
    let (xml, report) = validate(
        r#"<state id="s"><transition event="go" target="a b"/><transition event="x" target="a"/><transition event="y" target="b"/></state><state id="a"/><state id="b"/>"#,
    );
    let found = with_code(&report, "SCXML030");
    assert_eq!(found.len(), 1, "{report:?}");
    let messages: Vec<_> = found[0]
        .fixes
        .iter()
        .map(|fix| fix.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "remove the target `a`, keeping `b`",
            "remove the target `b`, keeping `a`"
        ]
    );
    for (fix, expected) in found[0]
        .fixes
        .iter()
        .zip([r#"target="b""#, r#"target="a""#])
    {
        let fixed = apply(&xml, fix);
        assert!(
            fixed.contains(&format!(r#"event="go" {expected}"#)),
            "{fixed}"
        );
    }
}

#[test]
fn orthogonal_parallel_targets_are_accepted_and_nested_or_repeated_ones_are_not() {
    let (_, report) = validate(
        r#"<state id="s"><transition event="go" target="x1 y1"/></state>
           <parallel id="p"><state id="x"><state id="x1"/></state><state id="y"><state id="y1"/></state></parallel>"#,
    );
    assert!(with_code(&report, "SCXML030").is_empty(), "{report:?}");

    // The compiler requires every pair of distinct targets to lie in
    // different regions of a common `<parallel>`: a state and its own
    // descendant is rejected at compile time, so it is SCXML030. One state
    // listed twice is legal (SCXML 1.0 §3.11: a legal state specification
    // is "a set of states such that 1) no state is an ancestor of any other
    // state on the list"), so it is only the LINT018 warning.
    let (xml, report) = validate(
        r#"<state id="s"><transition event="up" target="p x1"/><transition event="again" target="y1 y1"/></state>
           <parallel id="p"><state id="x"><state id="x1"/></state><state id="y"><state id="y1"/></state></parallel>"#,
    );
    let found = with_code(&report, "SCXML030");
    assert_eq!(found.len(), 1, "{report:?}");
    assert!(found[0].message.contains("are nested"), "{:?}", found[0]);
    assert_eq!(text(&xml, found[0].primary.span), "x1");
    let repeated = with_code(&report, "LINT018");
    assert_eq!(repeated.len(), 1, "{report:?}");
    assert_eq!(repeated[0].severity, Severity::Warning);
    assert!(
        repeated[0].message.contains("listed twice"),
        "{:?}",
        repeated[0]
    );
    let repeat = repeated[0]
        .fixes
        .first()
        .expect("a repeated target is removed exactly");
    assert_eq!(repeat.applicability, Applicability::MachineApplicable);
    assert!(apply(&xml, repeat).contains(r#"target="y1""#));

    let (_, report) = validate(
        r#"<state id="s" initial="s1 s1"><state id="s1"><transition event="e" target="s1 s1"/></state></state>"#,
    );
    assert_eq!(codes(&report), ["LINT018", "LINT018"], "{report:?}");
}

#[test]
fn initial_attribute_and_history_targets_are_checked_for_orthogonality() {
    let (xml, report) =
        validate(r#"<state id="c" initial="c1 c2"><state id="c1"/><state id="c2"/></state>"#);
    let found = with_code(&report, "SCXML030");
    assert_eq!(found.len(), 1, "{report:?}");
    assert_eq!(text(&xml, found[0].primary.span), "c2");

    let (_, report) = validate(
        r#"<parallel id="p"><state id="a"><history id="ha"><transition target="a1"/></history><state id="a1"/></state><state id="b"><history id="hb"><transition target="b1"/></history><state id="b1"/></state></parallel>
           <state id="s"><transition event="e" target="ha hb"/></state>"#,
    );
    assert!(
        with_code(&report, "SCXML030").is_empty(),
        "history targets in different regions are legal: {report:?}"
    );
}

// SCXML031 / SCXML032 --------------------------------------------------------

#[test]
fn final_state_children_are_rejected_with_rename_and_removal_fixes() {
    let (xml, report) =
        validate(r#"<final id="done"><transition event="again" target="done"/><onentry/></final>"#);
    let found = with_code(&report, "SCXML031");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_complete(diagnostic);
    assert!(text(&xml, diagnostic.primary.span).starts_with("<transition"));
    let rename = diagnostic
        .fixes
        .iter()
        .find(|fix| fix.message.contains("`<state>`"))
        .expect("rename fix");
    let renamed = apply(&xml, rename);
    assert!(
        renamed.contains(r#"<state id="done">"#) && renamed.contains("</state></scxml>"),
        "{renamed}"
    );
}

#[test]
fn final_initial_attribute_has_machine_applicable_removal() {
    let (xml, report) = validate(r#"<final id="f" initial="x"/>"#);
    let found = with_code(&report, "SCXML031");
    assert_eq!(found.len(), 1, "{report:?}");
    assert_eq!(
        found[0].fixes[0].applicability,
        Applicability::MachineApplicable
    );
    assert!(apply(&xml, &found[0].fixes[0]).contains(r#"<final id="f"/>"#));
}

#[test]
fn donedata_outside_final_and_duplicates_are_rejected() {
    let (xml, report) = validate(
        r#"<state id="s"><donedata/></state><final id="f"><donedata/><donedata/></final>"#,
    );
    let found = with_code(&report, "SCXML032");
    assert_eq!(found.len(), 2, "{report:?}");
    for diagnostic in &found {
        assert_complete(diagnostic);
        assert_eq!(text(&xml, diagnostic.primary.span), "<donedata/>");
    }
    let (_, accepted) = validate(r#"<state id="s"/><final id="f"><donedata/></final>"#);
    assert!(with_code(&accepted, "SCXML032").is_empty());
}

// LINT006 -------------------------------------------------------------------

#[test]
fn single_region_parallel_offers_a_rename_to_state() {
    let (xml, report) = validate(r#"<parallel id="p"><state id="only"/></parallel>"#);
    let found = with_code(&report, "LINT006");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_complete(diagnostic);
    assert_eq!(
        diagnostic.fixes[0].applicability,
        Applicability::MaybeIncorrect
    );
    let fixed = apply(&xml, &diagnostic.fixes[0]);
    assert!(
        fixed.contains(r#"<state id="p"><state id="only"/></state>"#),
        "{fixed}"
    );
    let (_, two) = validate(r#"<parallel id="p"><state id="a"/><state id="b"/></parallel>"#);
    assert!(with_code(&two, "LINT006").is_empty());
}

// LINT007 -------------------------------------------------------------------

#[test]
fn shadowed_transition_points_at_its_event_and_offers_reorder() {
    let xml_body = "<state id=\"s\">\n  <transition event=\"error\" target=\"f\"/>\n  <transition event=\"error.execution\" target=\"g\"/>\n</state><final id=\"f\"/><final id=\"g\"/>";
    let (xml, report) = validate(xml_body);
    let found = with_code(&report, "LINT007");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_complete(diagnostic);
    assert_eq!(text(&xml, diagnostic.primary.span), "error.execution");
    let reorder = &diagnostic.fixes[0];
    let reordered = apply(&xml, reorder);
    let first = reordered
        .find("error.execution")
        .expect("moved transition present");
    let second = reordered.find("\"error\"").expect("original transition");
    assert!(first < second, "{reordered}");
    assert_eq!(reordered.matches("<transition").count(), 2, "{reordered}");
}

#[test]
fn conditional_or_narrower_earlier_transitions_do_not_shadow() {
    let (_, report) = validate(
        r#"<state id="s">
             <transition event="a" cond="x" target="f"/>
             <transition event="a" target="f"/>
             <transition event="b.c" target="f"/>
             <transition event="b" target="f"/>
           </state><final id="f"/>"#,
    );
    assert!(with_code(&report, "LINT007").is_empty(), "{report:?}");
    let (_, wildcard) = validate(
        r#"<state id="s"><transition event="*" target="f"/><transition event="x.y" target="f"/></state><final id="f"/>"#,
    );
    assert_eq!(with_code(&wildcard, "LINT007").len(), 1);
}

// LINT008 -------------------------------------------------------------------

#[test]
fn eventless_cycle_through_default_entry_is_reported() {
    let (xml, report) = validate(
        r#"<state id="a"><transition target="b"/></state>
           <state id="b" initial="b1"><state id="b1"><transition target="a"/></state></state>"#,
    );
    let found = with_code(&report, "LINT008");
    assert_eq!(found.len(), 1, "{report:?}");
    let diagnostic = found[0];
    assert_complete(diagnostic);
    assert!(
        diagnostic.message.contains("`a` -> `b1` -> `a`"),
        "{diagnostic:?}"
    );
    assert_eq!(text(&xml, diagnostic.primary.span), "b");
    assert!(
        with_code(&report, "LINT004").is_empty(),
        "the direct detector cannot see this cycle"
    );
}

#[test]
fn inherited_eventless_cycle_labels_the_ancestor_transition() {
    let (xml, report) = validate(
        r#"<state id="a" initial="a1"><transition target="b"/><state id="a1"/></state>
           <state id="b" initial="b1"><state id="b1"><transition target="a"/></state></state>"#,
    );
    let found = with_code(&report, "LINT008");
    assert_eq!(found.len(), 1, "{report:?}");
    assert!(
        found[0]
            .secondary
            .iter()
            .any(|label| text(&xml, label.span).starts_with("<transition")),
        "{:?}",
        found[0]
    );
}

#[test]
fn direct_cycles_stay_with_lint004_and_conditional_cycles_are_not_claimed() {
    let (_, direct) = validate(
        r#"<state id="a"><transition target="b"/></state><state id="b"><transition target="a"/></state>"#,
    );
    assert!(with_code(&direct, "LINT008").is_empty(), "{direct:?}");
    assert_eq!(with_code(&direct, "LINT004").len(), 1);

    let (_, conditional) = validate(
        r#"<state id="a"><transition cond="x" target="b"/></state>
           <state id="b" initial="b1"><state id="b1"><transition target="a"/></state></state>"#,
    );
    assert!(with_code(&conditional, "LINT008").is_empty());

    let (_, halting) =
        validate(r#"<state id="a"><transition target="done"/></state><final id="done"/>"#);
    assert!(with_code(&halting, "LINT008").is_empty());
}

/// SCXML 1.0 Appendix D, "Termination": "A microstep always terminates. A
/// macrostep may not. A macrostep that does not terminate may be said to
/// consist of an infinitely long sequence of microsteps. This is currently
/// allowed." A document whose eventless transitions loop is conforming, so
/// every eventless-loop finding is a warning and the document compiles. (A
/// transition with neither `event`, `cond`, nor `target` is the SCXML004
/// error instead: SCXML 1.0 §3.5.2 requires one of them.)
#[test]
fn eventless_loops_are_warnings_because_a_non_terminating_macrostep_is_allowed() {
    for (body, code) in [
        (
            r#"<state id="a"><transition target="b"/></state><state id="b"><transition target="a"/></state>"#,
            "LINT004",
        ),
        (
            r#"<state id="a"><transition target="b"/></state><state id="b" initial="b1"><state id="b1"><transition target="a"/></state></state>"#,
            "LINT008",
        ),
    ] {
        let (_, report) = validate(body);
        let found = with_code(&report, code);
        assert_eq!(found.len(), 1, "{report:?}");
        assert_eq!(found[0].severity, Severity::Warning, "{code}");
        assert_eq!(found[0].category, DiagnosticCategory::Lint, "{code}");
        assert!(!report.has_errors(), "{code}: {report:?}");
    }
}

// LIMIT002 -------------------------------------------------------------------

#[test]
fn exhausted_budget_fails_closed_with_one_error_and_no_partial_findings() {
    let xml =
        format!(r#"<scxml xmlns="{SCXML_NAMESPACE}"><state id="a"/><state id="island"/></scxml>"#);
    let document =
        parse_xml(SourceText::new(SOURCE, "budget.scxml", xml)).expect("budget fixture parses");
    let mut findings = Report::new();
    let mut budget = Budget::new(3);
    assert_eq!(
        analyze(&document, &document.root, &mut budget, &mut findings),
        Err(Exhausted)
    );
    assert_eq!(
        budget_diagnostic("LIMIT002", &document.root).severity,
        Severity::Error
    );
    // With the full budget the same machine analyzes normally.
    let mut findings = Report::new();
    let mut budget = Budget::new(MAX_GRAPH_WORK);
    assert_eq!(
        analyze(&document, &document.root, &mut budget, &mut findings),
        Ok(())
    );
    assert_eq!(with_code(&findings, "LINT005").len(), 1);
}

/// Natively graph analysis is unbounded: a chain one state past the wasm
/// node bound (65,536) is analyzed in full, with no `LIMIT002`.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_graph_analysis_has_no_size_budget() {
    let states: String = (0..65_537)
        .map(|index| {
            format!(
                r#"<state id="s{index}"><transition target="s{}"/></state>"#,
                index + 1
            )
        })
        .chain(std::iter::once(r#"<final id="s65537"/>"#.to_owned()))
        .collect();
    let (_, report) = validate(&states);
    assert!(with_code(&report, "LIMIT002").is_empty(), "{report:?}");
    assert!(with_code(&report, "LINT005").is_empty(), "{report:?}");
}

/// The work budget grows with the machine, so linear analyses never exhaust
/// it, but it applies to every build: pairwise checks over one state's
/// transitions grow quadratically and are cut off with `LIMIT002`.
#[test]
fn quadratic_graph_work_is_bounded_on_every_build() {
    use std::fmt::Write as _;

    let mut transitions = String::new();
    for index in 0..3_000 {
        write!(transitions, r#"<transition event="e{index}" target="b"/>"#)
            .expect("writing to a String cannot fail");
    }
    let (_, report) = validate(&format!(
        r#"<state id="a">{transitions}</state><final id="b"/>"#
    ));
    assert_eq!(with_code(&report, "LIMIT002").len(), 1, "{report:?}");
    // A machine of the same size without the quadratic pairs is analyzed.
    let mut states = String::new();
    for index in 0..10_000 {
        write!(
            states,
            r#"<state id="s{index}"><transition event="e{index}" target="s{}"/></state>"#,
            index + 1
        )
        .expect("writing to a String cannot fail");
    }
    let (_, report) = validate(&format!(r#"{states}<final id="s10000"/>"#));
    assert!(with_code(&report, "LIMIT002").is_empty(), "{report:?}");
    // Deep nesting makes every ancestor walk long, and still fits the grant:
    // 40 chains of 200 nested states, each state targeting the deepest state
    // of the next chain.
    let mut chains = String::new();
    for chain in 0..40 {
        let next = (chain + 1) % 40;
        for depth in 0..200 {
            write!(
                chains,
                r#"<state id="c{chain}d{depth}"><transition event="go" target="c{next}d199"/>"#
            )
            .expect("writing to a String cannot fail");
        }
        chains.push_str(&"</state>".repeat(200));
    }
    let (_, report) = validate(&chains);
    assert!(with_code(&report, "LIMIT002").is_empty(), "{report:?}");
}

#[test]
fn graph_work_budget_scales_with_the_machine() {
    assert_eq!(graph_work_budget(0), MAX_GRAPH_WORK);
    assert_eq!(graph_work_budget(1), MAX_GRAPH_WORK);
    // Natively the grant grows with the machine; a wasm build caps it at
    // `MAX_GRAPH_WORK`, which the host's validator component tests check
    // (`the_wasm_validator_keeps_the_bounds_the_native_cli_lifts`, LIMIT002).
    #[cfg(not(target_arch = "wasm32"))]
    {
        let large = MAX_GRAPH_WORK / GRAPH_WORK_PER_ITEM * 4;
        assert_eq!(graph_work_budget(large), large * GRAPH_WORK_PER_ITEM);
        assert_eq!(graph_work_budget(usize::MAX), usize::MAX);
    }
}

#[test]
fn budget_spend_saturates_at_zero() {
    let mut budget = Budget::new(2);
    assert_eq!(budget.spend(2), Ok(()));
    assert_eq!(budget.spend(1), Err(Exhausted));
    assert_eq!(budget.remaining, 0);
}

#[test]
fn nested_machines_are_analyzed_independently() {
    let (_, report) = validate(
        r#"<state id="s"><invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml"><state id="c"/><state id="orphan"/></scxml></content></invoke></state>"#,
    );
    let found = with_code(&report, "LINT005");
    assert_eq!(found.len(), 1, "{report:?}");
    assert!(found[0].message.contains("`orphan`"));
}

#[test]
fn explain_examples_for_graph_and_expression_codes_are_self_consistent() {
    for code in [
        "LINT005", "SCXML030", "SCXML031", "SCXML032", "LINT006", "LINT007", "LINT008", "SCXML037",
        "LINT018",
    ] {
        let entry = crate::explain::lookup(code).expect("every new code is documented");
        assert!(entry.self_checking, "{code}");
        let check = |xml: &str| {
            let document = parse_xml(SourceText::new(SOURCE, "explain.scxml", xml))
                .expect("explain example parses");
            validate_semantics(&document)
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == code)
        };
        assert!(check(entry.bad), "{code} bad example must emit {code}");
        assert!(
            !check(entry.fixed),
            "{code} fixed example must not emit {code}"
        );
    }
    for code in [
        "LIMIT002", "SCXML038", "SCXML039", "LIMIT003", "LIMIT004", "LINT009",
    ] {
        assert!(
            crate::explain::lookup(code).is_some(),
            "{code} is documented"
        );
    }
}

#[test]
fn unresolved_references_suppress_the_unreachable_cascade() {
    // vdx/44: a typo in `target` leaves the intended state unreferenced; the
    // SCXML005 error is the root cause, so LINT005 must not also claim the
    // intended state can never become active.
    let (_, report) =
        validate(r#"<state id="a"><transition event="go" target="bb"/></state><final id="b"/>"#);
    assert!(with_code(&report, "SCXML005").len() == 1, "{report:?}");
    assert!(with_code(&report, "LINT005").is_empty(), "{report:?}");
    let (_, report) =
        validate(r#"<state id="p" initial="missing"><state id="c"/></state><state id="q"/>"#);
    assert!(with_code(&report, "LINT005").is_empty(), "{report:?}");
}
