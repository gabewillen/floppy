use crate::SCXML_NAMESPACE;
use crate::datamodel_check::xpath::tests::{apply, diagnostics};

fn machine(body: &str) -> String {
    format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="xpath"><datamodel><data id="count" expr="0"/><data id="user"><name xmlns="">bob</name></data></datamodel>{body}</scxml>"#
    )
}

/// Exactly one finding with `code`, whose message and help contain the
/// expected text and whose fixes (in order) yield `repaired` and leave no
/// finding.
fn check(body: &str, code: &str, message: &str, help: &str, repaired: &[&str]) {
    let xml = machine(body);
    let found = diagnostics(&xml);
    assert_eq!(found.len(), 1, "{body}: {found:#?}");
    let diagnostic = &found[0];
    assert_eq!(diagnostic.code, code, "{body}");
    assert!(
        diagnostic.message.contains(message),
        "{body}: {}",
        diagnostic.message
    );
    let actual = diagnostic.help.as_deref().unwrap_or_default();
    assert!(actual.contains(help), "{body}: {actual}");
    assert_eq!(diagnostic.fixes.len(), repaired.len(), "{body}");
    for (index, expected) in repaired.iter().enumerate() {
        let fixed = apply(&xml, diagnostic, index);
        assert!(fixed.contains(expected), "{body}: {fixed}");
        let after = diagnostics(&fixed);
        assert!(after.is_empty(), "{body}: fix {index} left {after:#?}");
    }
}

fn clean(body: &str) {
    let found = diagnostics(&machine(body));
    assert!(found.is_empty(), "{body}: {found:#?}");
}

#[test]
fn undeclared_variables_are_errors_with_typo_and_member_repairs() {
    check(
        r#"<state id="a"><transition cond="$cuont &gt; 3" target="a"/></state>"#,
        "SCXML056",
        "`$cuont` is not a declared XPath variable",
        "did you mean `$count`?",
        &[r#"cond="$count &gt; 3""#],
    );
    check(
        r#"<state id="a"><transition cond="$user.name = 'bob'" target="a"/></state>"#,
        "SCXML056",
        "one undeclared variable named `user.name`, not a member of `$user`",
        "`$user/name`",
        &[r#"cond="$user/name = 'bob'""#],
    );
    check(
        r#"<state id="a"><onentry><log expr="$_event.data.ok"/></onentry></state>"#,
        "SCXML056",
        "not a member of `$_event`",
        "`$_event/data/ok`",
        &[r#"expr="$_event/data/ok""#],
    );
    check(
        r#"<state id="a"><onentry><log expr="$zzzzzz"/></onentry></state>"#,
        "SCXML056",
        "`$zzzzzz` is not a declared XPath variable",
        "<data id=\"zzzzzz\"/>",
        &[],
    );
}

#[test]
fn declared_system_and_runtime_bound_variables_are_accepted() {
    clean(
        r#"<state id="a"><transition event="e" cond="$_event/name = 'e' and $count = 0 and $_sessionid and $_name and $_ioprocessors" target="a"/></state>"#,
    );
    clean(
        r#"<state id="a"><onentry><foreach array="$user/*" item="it" index="i"><log expr="concat($it, $i)"/></foreach><send event="e" idlocation="sid"/><log expr="$sid"/></onentry></state>"#,
    );
    clean(r#"<state id="a"><onentry><log expr="'$notavariable'"/></onentry></state>"#);
}

#[test]
fn bare_relative_paths_that_mean_a_variable_or_boolean_are_warnings_with_repairs() {
    check(
        r#"<state id="a"><onentry><assign location="count" expr="$count + 1"/></onentry></state>"#,
        "LINT011",
        "`count` selects a child element named `count`, not the variable `$count`",
        "bound to the variable `$count`",
        &[r#"location="$count""#],
    );
    check(
        r#"<state id="a"><transition cond="true" target="a"/></state>"#,
        "LINT011",
        "not the boolean `true()`",
        "`true()`",
        &[r#"cond="true()""#],
    );
    check(
        r#"<state id="a"><transition cond="_event.data.ok" target="a"/></state>"#,
        "LINT011",
        "contains dots, not a member access",
        "`$_event/data/ok`",
        &[r#"cond="$_event/data/ok""#],
    );
    // A `<foreach>` item is not a child of `<datamodel>` (SCXML 4.6: the
    // item variable is the collection's own node), so a bare name in the
    // loop still selects nothing.
    check(
        r#"<state id="a"><onentry><foreach array="$user/*" item="it"><log expr="name"/></foreach></onentry></state>"#,
        "LINT011",
        "`name` selects no node",
        "relative path from `<datamodel>`",
        &[],
    );
    check(
        r#"<state id="a"><onentry><assign location="foo.bar.baz" expr="1"/></onentry></state>"#,
        "LINT011",
        "`foo.bar.baz` selects no node",
        "relative path from `<datamodel>`",
        &[],
    );
}

#[test]
fn datamodel_children_predicates_and_foreach_items_are_not_flagged() {
    clean(
        r#"<state id="a"><transition event="e" cond="_event/data = 1 and data[@id = 'count'] = 0" target="a"/></state>"#,
    );
    clean(
        r#"<state id="a"><transition cond="$user/name[text() = 'bob'] and count($user/*) = 1" target="a"/></state>"#,
    );
    clean(r#"<state id="a"><transition cond="In('a')" target="a"/></state>"#);
    // A `<foreach>` item is the collection's own node (SCXML 4.6), reached
    // through `$it`, never a child of `<datamodel>`.
    clean(
        r#"<state id="a"><onentry><foreach array="$user/*" item="it"><log expr="$it/name"/></foreach></onentry></state>"#,
    );
}
