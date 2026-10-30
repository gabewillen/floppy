//! `validate` is a superset of `compile`'s structural rejections.
//!
//! Every place lowering or Machine IR validation rejects a document (every
//! `LowerError` construction and bounded-check helper call in
//! `src/lower.rs`, `src/ir.rs`, and `src/lib.rs`) is listed below by its
//! error key with its exact number of source sites; a source scan fails the
//! suite when a site is added, removed, or renamed without updating the
//! table. Each key either names documents that reach it (lowering must fail
//! with that key, the validator composed as `scx validate` must report
//! the expected error code, and `compile` must stop at validation instead of
//! reporting `COMP001`), or states why no parsed document can reach it.

#![allow(
    clippy::format_collect,
    reason = "the test builds each large document once from small fragments"
)]

mod common;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use scxml_compiler_core::lower;
use scxml_syntax::{SourceId, SourceText, is_xml_space, parse_xml};
use scxml_validator_core::Severity;

const LOWER_RS: &str = include_str!("../src/lower.rs");
const IR_RS: &str = include_str!("../src/ir.rs");
const LIB_RS: &str = include_str!("../src/lib.rs");

const HEAD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">"#;
/// The source id of every parsed test document.
const SOURCE: SourceId = SourceId::new(1);
const XPATH_HEAD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath" initial="s">"#;
/// A string one byte over the compiler's 1 MiB string bound.
const OVER_MIB: usize = 1024 * 1024 + 1;

/// Why a rejection site cannot be reached from a parsed, lowered document.
const INVARIANT: &str = "a Machine IR invariant that lowering constructs itself; only untrusted IR decoded by `MachineIr::from_json` can violate it";
const EVENT: &str = scxml_types::EVENT_NAMESPACE_URI;
const ENV: &str = scxml_types::ENV_NAMESPACE_URI;
const DECODE_ONLY: &str = "checked only when decoding untrusted Machine IR JSON";
const STRING_IMPLIED: &str = "a Machine IR string bound on an SCXML attribute value that lowering already bounds to 1 MiB (`source attribute value`) and, when required, rejects empty (`required_attr`)";
const COUNT_IMPLIED: &str = "a Machine IR count of items that lowering already bounds through the child nodes of the element that holds them";
const PINS_ONLY: &str = "only compile-time script packages pin a script, and the compiler pins every reference to one URI with the same package";
const OVERFLOW: &str =
    "a u32 overflow; an 8 MiB source has far fewer elements, states, or transitions";
const NAME_IMPLIED: &str = "a name the schema types as `xsd:ID` or `xsd:NMTOKEN` (a `done.` event appends one), so the XSD check already rejects it when empty or holding white space, and names have no length bound";
const GRAPH_IMPLIED: &str = "the validator's semantic graph bound (65,536 nodes or transitions, `LIMIT002`) is below the compiler's 100,000 (asserted below)";

/// One lowering rejection key with its source sites and coverage.
struct Entry {
    key: &'static str,
    sites: usize,
    documents: Vec<(String, &'static str)>,
    /// Why the key (or its sites the documents do not reach) cannot be
    /// reached from a parsed document; empty when every site is covered.
    unreachable: &'static str,
}

fn covered(key: &'static str, sites: usize, documents: Vec<(String, &'static str)>) -> Entry {
    Entry {
        key,
        sites,
        documents,
        unreachable: "",
    }
}

fn partly(
    key: &'static str,
    sites: usize,
    documents: Vec<(String, &'static str)>,
    unreachable: &'static str,
) -> Entry {
    Entry {
        key,
        sites,
        documents,
        unreachable,
    }
}

fn never(key: &'static str, sites: usize, unreachable: &'static str) -> Entry {
    Entry {
        key,
        sites,
        documents: Vec::new(),
        unreachable,
    }
}

fn machine(body: &str) -> String {
    format!(r#"{HEAD}{body}<final id="f"/></scxml>"#)
}

fn xpath_machine(body: &str) -> String {
    format!(r#"{XPATH_HEAD}<datamodel><data id="x"/></datamodel>{body}<final id="f"/></scxml>"#)
}

fn state(inner: &str) -> String {
    machine(&format!(
        r#"<state id="s">{inner}<transition event="go" target="f"/></state>"#
    ))
}

fn action(content: &str) -> String {
    state(&format!("<onentry>{content}</onentry>"))
}

fn xpath_action(content: &str) -> String {
    xpath_machine(&format!(
        r#"<state id="s"><onentry>{content}</onentry><transition event="go" target="f"/></state>"#
    ))
}

fn repeat(text: &str, count: usize) -> String {
    text.repeat(count)
}

fn nested(open: &str, close: &str, levels: usize, inner: &str) -> String {
    format!("{}{inner}{}", open.repeat(levels), close.repeat(levels))
}

#[allow(
    clippy::too_many_lines,
    reason = "one table lists every lowering rejection key"
)]
fn table() -> Vec<Entry> {
    let long = "x".repeat(OVER_MIB);
    let targets = |count: usize| vec!["f"; count].join(" ");
    vec![
        // ---------------------------------------------------------------
        // Variant keys
        // ---------------------------------------------------------------
        partly(
            "DuplicateStateId",
            2,
            vec![
                (
                    machine(r#"<state id="s"><transition event="go" target="f"/></state><state id="s"/>"#),
                    "SCXML002",
                ),
                (
                    machine(r#"<state id="s"><transition event="go" target="f"/></state><state id=" s&#9;"/>"#),
                    "SCXML002",
                ),
            ],
            "the Machine IR check repeats the lowering check, which rejects the duplicate first",
        ),
        covered(
            "InvalidAttributeValue",
            5,
            vec![
                (
                    state(r#"<history id="h" type="wide"><transition target="a"/></history><state id="a"/>"#),
                    "XSD-cvc-enumeration-valid",
                ),
                (
                    machine(r#"<state id="s"><transition event="go" type="sideways" target="f"/></state>"#),
                    "XSD-cvc-enumeration-valid",
                ),
                (
                    state(r#"<invoke type="scxml" src="c.scxml" autoforward="1"/>"#),
                    "XSD-cvc-enumeration-valid",
                ),
                (
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="2.0" initial="s"><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#.to_owned(),
                    "XSD-",
                ),
                (
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" binding="lazy" initial="s"><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#.to_owned(),
                    "XSD-cvc-enumeration-valid",
                ),
            ],
        ),
        never("InvalidIrSchema", 2, DECODE_ONLY),
        never("UnsupportedIrVersion", 2, DECODE_ONLY),
        never(
            "IrSerialization",
            1,
            "serializing validated Machine IR to JSON cannot fail",
        ),
        covered(
            "MissingAttribute",
            4,
            vec![
                (
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" initial="s"><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#.to_owned(),
                    "XSD-cvc-complex-type.4",
                ),
                (action(r#"<if><log expr="1"/></if>"#), "XSD-"),
                (
                    machine(r#"<datamodel><data id=""/></datamodel><state id="s"><transition event="go" target="f"/></state>"#),
                    "SCXML003",
                ),
                (action(r#"<raise event=""/>"#), "XSD-"),
                (action(r#"<raise event="  "/>"#), "XSD-"),
            ],
        ),
        partly(
            "NoStates",
            3,
            vec![(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><datamodel/></scxml>"#.to_owned(),
                "SCXML067",
            )],
            "a root whose only children are histories is a schema error before its default initial state is looked up, and lowered IR always has states",
        ),
        never("TooManySourceElements", 1, OVERFLOW),
        never("TooManyStates", 4, OVERFLOW),
        never("TooManyTransitions", 2, OVERFLOW),
        covered(
            "TransitionWithoutTarget",
            1,
            vec![(
                state(r#"<state id="i"><initial><transition target=" "/></initial><state id="a"/></state>"#),
                "SCXML010",
            )],
        ),
        covered(
            "UnknownInitial",
            1,
            vec![(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial=" "><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#.to_owned(),
                "SCXML006",
            )],
        ),
        covered(
            "UnknownTarget",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="nowhere"/></state>"#),
                "SCXML005",
            )],
        ),
        covered(
            "UnsupportedAttribute",
            1,
            vec![
                (
                    machine(r#"<state id="s" final="true"><transition event="go" target="f"/></state>"#),
                    "XSD-cvc-complex-type.3.2.2",
                ),
            ],
        ),
        partly(
            "UnsupportedElement",
            14,
            vec![
                (state(r#"<note xmlns=""/>"#), "XSD-"),
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" event="e"><event:hold/></transition>"#
                    )),
                    "EVENT001",
                ),
                (state(r#"<raise event="e"/>"#), "XSD-cvc-complex-type.2.4"),
                (
                    state(r#"<state id="i"><initial><final id="g"/></initial><state id="a"/></state>"#),
                    "XSD-",
                ),
                (
                    machine(r#"<datamodel><final id="g"/></datamodel><state id="s"><transition event="go" target="f"/></state>"#),
                    "XSD-",
                ),
                (
                    state(r#"<invoke type="scxml" src="c.scxml"><note xmlns=""/></invoke>"#),
                    "XSD-",
                ),
                (
                    state(r#"<invoke type="scxml" src="c.scxml"><raise event="e"/></invoke>"#),
                    "XSD-",
                ),
                (
                    machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><donedata><raise event="e"/></donedata></final>"#),
                    "XSD-",
                ),
                (action(r#"<note xmlns=""/>"#), "XSD-"),
                (action(r#"<final id="g"/>"#), "XSD-"),
                (
                    action(r#"<send event="e"><note xmlns=""/></send>"#),
                    "XSD-",
                ),
                (
                    action(r#"<send event="e"><raise event="x"/></send>"#),
                    "XSD-",
                ),
                (machine(r#"<note xmlns=""/><state id="s"><transition event="go" target="f"/></state>"#), "XSD-"),
                (machine(r#"<onentry/><state id="s"><transition event="go" target="f"/></state>"#), "XSD-"),
            ],
            "the fallback arm of `lower_state` is called only for the four state elements",
        ),
        // ---------------------------------------------------------------
        // Lowering structure
        // ---------------------------------------------------------------
        covered(
            "document root must be the canonical SCXML element",
            1,
            vec![(
                r#"<machine xmlns="http://www.w3.org/2005/07/scxml" version="1.0"/>"#.to_owned(),
                "SCXML001",
            )],
        ),
        covered(
            "state contains more than one initial element",
            1,
            vec![(
                state(r#"<state id="p"><initial><transition target="a"/></initial><initial><transition target="a"/></initial><state id="a"/></state>"#),
                "SCXML064",
            )],
        ),
        covered(
            "final state contains more than one donedata element",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><donedata/><donedata/></final>"#),
                "SCXML032",
            )],
        ),
        covered(
            "parallel state forbids an initial transition",
            1,
            vec![(
                state(r#"<parallel id="p"><initial><transition target="a"/></initial><state id="a"/><state id="b"/></parallel>"#),
                "XSD-",
            )],
        ),
        covered(
            "state cannot combine initial attribute and initial element",
            1,
            vec![(
                state(r#"<state id="p" initial="a"><initial><transition target="a"/></initial><state id="a"/></state>"#),
                "SCXML011",
            )],
        ),
        covered(
            "compound state has no initializable child",
            1,
            vec![(
                state(r#"<state id="c"><history id="h"><transition target="c"/></history></state>"#),
                "SCXML021",
            )],
        ),
        covered(
            "initial element requires exactly one transition",
            2,
            vec![
                (
                    state(r#"<state id="p"><initial><transition target="a"/><transition target="a"/></initial><state id="a"/></state>"#),
                    "SCXML009",
                ),
                (
                    state(r#"<state id="p"><initial/><state id="a"/></state>"#),
                    "SCXML009",
                ),
            ],
        ),
        covered(
            "structural SCXML content cannot contain free text",
            1,
            vec![(state("free text"), "XSD-cvc-complex-type.2.3")],
        ),
        covered(
            "executable content cannot contain free text",
            1,
            vec![(action("free text"), "XSD-cvc-complex-type.2.3")],
        ),
        covered(
            "if executable content cannot contain free text",
            1,
            vec![(action(r#"<if cond="true">free text</if>"#), "XSD-cvc-complex-type.2.3")],
        ),
        covered(
            "element does not permit child content",
            1,
            vec![
                (action(r#"<raise event="e">text</raise>"#), "XSD-cvc-complex-type.2.3"),
            ],
        ),
        covered(
            "elseif appears after else",
            1,
            vec![(
                action(r#"<if cond="true"><else/><elseif cond="false"/></if>"#),
                "XSD-cvc-complex-type.2.4",
            )],
        ),
        covered(
            "if contains more than one else",
            1,
            vec![(action(r#"<if cond="true"><else/><else/></if>"#), "XSD-cvc-complex-type.2.4")],
        ),
        never("active elseif branch is missing", 1, INVARIANT),
        covered(
            "assign type is not an SCXML assign type",
            1,
            vec![(
                xpath_action(r#"<assign location="$x" type="sideways" expr="1"/>"#),
                "XSD-cvc-enumeration-valid",
            )],
        ),
        partly(
            "assign attr is required exactly for type addattribute",
            2,
            vec![
                (
                    xpath_action(r#"<assign location="$x" type="addattribute" expr="1"/>"#),
                    "SCXML065",
                ),
                (
                    xpath_action(r#"<assign location="$x" attr="a" expr="1"/>"#),
                    "SCXML065",
                ),
                (
                    xpath_action(r#"<assign location="$x" type=" addattribute " expr="1"/>"#),
                    "SCXML065",
                ),
            ],
            "the Machine IR check repeats the lowering check, which rejects it first",
        ),
        partly(
            "assign cannot combine expr and inline body",
            2,
            vec![(action(r#"<assign location="x" expr="1">2</assign>"#), "SCXML053")],
            "the Machine IR check repeats the lowering check, which rejects it first",
        ),
        never("assign type delete takes no value", 1, INVARIANT),
        covered(
            "assign requires expr or inline body",
            1,
            vec![(action(r#"<assign location="x"/>"#), "SCXML065")],
        ),
        covered(
            "script cannot contain child elements",
            1,
            vec![
                (action("<script><b/></script>"), "XSD-"),
            ],
        ),
        covered(
            "send contains more than one content element",
            1,
            vec![(
                action(r#"<send event="e"><content>1</content><content>2</content></send>"#),
                "SCXML064",
            )],
        ),
        partly(
            "invoke permits at most one content and finalize child",
            2,
            vec![
                (
                    state(r#"<invoke type="scxml"><content expr="1"/><content expr="2"/></invoke>"#),
                    "SCXML064",
                ),
                (
                    state(r#"<invoke type="scxml" src="c.scxml"><finalize/><finalize/></invoke>"#),
                    "SCXML064",
                ),
            ],
            "the Machine IR check repeats the lowering check, which rejects it first",
        ),
        partly(
            "history fallback transition cannot have event or condition",
            2,
            vec![
                (
                    state(r#"<state id="p"><history id="h"><transition cond="true" target="a"/></history><state id="a"/></state>"#),
                    "SCXML020",
                ),
                (
                    state(r#"<state id="p"><history id="h"><transition event="e" target="a"/></history><state id="a"/></state>"#),
                    "SCXML020",
                ),
            ],
            "the Machine IR check repeats the lowering check, which rejects it first",
        ),
        partly(
            "script cannot combine src and inline body",
            2,
            vec![(action(r#"<script src="a.js">x = 1;</script>"#), "SCXML050")],
            "the pinning site is reached only with a compile-time script package, after validation",
        ),
        covered(
            "InvalidEventSchema",
            3,
            vec![
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" event="e" event:schema="/order.json"/>"#
                    )),
                    "EVENT004",
                ),
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" event="e" event:schema="order.json"/>"#
                    )),
                    "EVENT004",
                ),
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" event="e" event:schema='{{"type":}}'/>"#
                    )),
                    "EVENT003",
                ),
            ],
        ),
        covered(
            "defer has no attributes and no content",
            1,
            vec![
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" event="e"><event:defer>x</event:defer></transition>"#
                    )),
                    "EVENT001",
                ),
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" event="e"><event:defer event:schema="true"/></transition>"#
                    )),
                    "EVENT001",
                ),
            ],
        ),
        covered(
            "defer is only transition content of a transition with an event",
            1,
            vec![
                (
                    action(&format!(r#"<event:defer xmlns:event="{EVENT}"/>"#)),
                    "EVENT001",
                ),
                (
                    state(&format!(
                        r#"<transition xmlns:event="{EVENT}" target="f"><event:defer/></transition>"#
                    )),
                    "EVENT001",
                ),
            ],
        ),
        covered(
            "event:schema and event:schemaexpr are mutually exclusive",
            1,
            vec![(
                state(&format!(
                    r#"<transition xmlns:event="{EVENT}" event="e" event:schema="true" event:schemaexpr="s"/>"#
                )),
                "EVENT002",
            )],
        ),
        never("event schema sha256 does not match its schema", 1, DECODE_ONLY),
        never("event schema is not JSON", 1, DECODE_ONLY),
        covered(
            "script src URI is invalid",
            1,
            vec![(action(r#"<script src=""/>"#), "SCXML051")],
        ),
        // ---------------------------------------------------------------
        // Machine IR state and transition structure
        // ---------------------------------------------------------------
        never("atomic children", 1, INVARIANT),
        covered(
            "atomic donedata",
            1,
            vec![(state(r#"<state id="a"><donedata/></state>"#), "SCXML032")],
        ),
        covered(
            "atomic initial",
            1,
            vec![(
                machine(r#"<state id="s" initial="f"><transition event="go" target="f"/></state>"#),
                "SCXML012",
            )],
        ),
        never(
            "compound state requires children and an initial transition",
            1,
            INVARIANT,
        ),
        covered(
            "final children",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><state id="h"/></final>"#),
                "SCXML031",
            )],
        ),
        covered(
            "final data",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><datamodel><data id="d"/></datamodel></final>"#),
                "SCXML031",
            )],
        ),
        covered(
            "final initial",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><initial><transition target="g"/></initial></final>"#),
                "SCXML031",
            )],
        ),
        covered(
            "final invokes",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><invoke type="scxml" src="c.scxml"/></final>"#),
                "SCXML031",
            )],
        ),
        covered(
            "final transitions",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><transition target="s"/></final>"#),
                "SCXML031",
            )],
        ),
        covered(
            "history children",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><state id="x"/><transition target="a"/></history><state id="a"/></state>"#),
                "XSD-",
            )],
        ),
        covered(
            "history data",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><datamodel><data id="d"/></datamodel><transition target="a"/></history><state id="a"/></state>"#),
                "XSD-",
            )],
        ),
        covered(
            "history donedata",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><donedata/><transition target="a"/></history><state id="a"/></state>"#),
                "SCXML032",
            )],
        ),
        covered(
            "history initial",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><initial><transition target="a"/></initial><transition target="a"/></history><state id="a"/></state>"#),
                "XSD-",
            )],
        ),
        covered(
            "history invokes",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><invoke type="scxml" src="c.scxml"/><transition target="a"/></history><state id="a"/></state>"#),
                "XSD-",
            )],
        ),
        covered(
            "history onentry",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><onentry/><transition target="a"/></history><state id="a"/></state>"#),
                "XSD-",
            )],
        ),
        covered(
            "history onexit",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><onexit/><transition target="a"/></history><state id="a"/></state>"#),
                "XSD-",
            )],
        ),
        covered(
            "history requires a parent and at most one fallback transition",
            1,
            vec![
                (
                    state(r#"<state id="p"><history id="h"><transition target="a"/><transition target="a"/></history><state id="a"/></state>"#),
                    "SCXML019",
                ),
                (
                    r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s"><history id="h"><transition target="s"/></history><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#.to_owned(),
                    "XSD-",
                ),
            ],
        ),
        covered(
            "history fallback target must be a descendant of its parent",
            1,
            vec![(
                state(r#"<state id="p"><history id="h"><transition target="f"/></history><state id="a"/></state>"#),
                "SCXML021",
            )],
        ),
        covered(
            "state initial target must be a descendant",
            1,
            vec![
                (
                    state(r#"<state id="p" initial="f"><state id="a"/></state>"#),
                    "SCXML017",
                ),
                (
                    state(r#"<state id="p"><initial><transition target="f"/></initial><state id="a"/></state>"#),
                    "SCXML022",
                ),
            ],
        ),
        covered(
            "initial transition requires at least one target",
            1,
            vec![(
                state(r#"<state id="p" initial=" "><state id="a"/></state>"#),
                "SCXML006",
            )],
        ),
        covered(
            "multi-target transition cannot target ancestor-related states",
            1,
            vec![(
                machine(r#"<parallel id="s"><state id="r1"><state id="a"><transition event="go" target="r1 a"/></state></state><state id="r2"/></parallel>"#),
                "SCXML030",
            )],
        ),
        covered(
            "multi-target transition requires a common parallel ancestor",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="s f"/></state>"#),
                "SCXML030",
            )],
        ),
        covered(
            "multi-target transition requires a parallel least common ancestor",
            1,
            vec![(
                state(r#"<state id="p"><state id="a"><transition event="go" target="a b"/></state><state id="b"/></state>"#),
                "SCXML030",
            )],
        ),
        never("multi-target adjacency window must contain two targets", 1, INVARIANT),
        never(
            "multi-target adjacency checks",
            1,
            "compiled only into the crate's unit tests",
        ),
        covered(
            "duplicate data id in scope",
            1,
            vec![(
                machine(r#"<datamodel><data id="d"/><data id="d"/></datamodel><state id="s"><transition event="go" target="f"/></state>"#),
                "SCXML002",
            )],
        ),
        covered(
            "data may have only one initializer form",
            1,
            vec![(
                machine(r#"<datamodel><data id="d" expr="1" src="d.json"/></datamodel><state id="s"><transition event="go" target="f"/></state>"#),
                "SCXML053",
            )],
        ),
        covered(
            "content cannot combine expr and inline body",
            1,
            vec![(
                action(r#"<send event="e"><content expr="1">2</content></send>"#),
                "SCXML053",
            )],
        ),
        covered(
            "donedata permits at most one content child",
            1,
            vec![(
                machine(r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><donedata><content expr="1"/><content expr="2"/></donedata></final>"#),
                "XSD-cvc-complex-type.2.4",
            )],
        ),
        covered(
            "parameter requires exactly one of expr or location",
            1,
            vec![(
                action(r#"<send event="e"><param name="p"/></send>"#),
                "SCXML007",
            )],
        ),
        covered(
            "cancel sendid/sendidexpr",
            1,
            vec![(action("<cancel/>"), "SCXML008")],
        ),
        covered(
            "send event/eventexpr",
            2,
            vec![
                (action(r#"<send event="e" eventexpr="'e'"/>"#), "SCXML014"),
                (action(r##"<send target="#_internal"/>"##), "SCXML052"),
            ],
        ),
        covered(
            "send target/targetexpr",
            1,
            vec![(
                action(r##"<send event="e" target="#_internal" targetexpr="'x'"/>"##),
                "SCXML014",
            )],
        ),
        covered(
            "send type/typeexpr",
            1,
            vec![(
                action(r#"<send event="e" type="http://www.w3.org/TR/scxml/#SCXMLEventProcessor" typeexpr="'x'"/>"#),
                "SCXML014",
            )],
        ),
        covered(
            "send id/idlocation",
            1,
            vec![(action(r#"<send event="e" id="a" idlocation="x"/>"#), "SCXML014")],
        ),
        covered(
            "send delay/delayexpr",
            1,
            vec![(action(r#"<send event="e" delay="1s" delayexpr="'1s'"/>"#), "SCXML014")],
        ),
        covered(
            "send content cannot be combined with namelist or params",
            1,
            vec![(
                action(r#"<send event="e"><param name="p" expr="1"/><content>1</content></send>"#),
                "SCXML015",
            )],
        ),
        covered(
            "invoke type/typeexpr",
            1,
            vec![(
                state(r#"<invoke type="scxml" typeexpr="'scxml'" src="c.scxml"/>"#),
                "SCXML016",
            )],
        ),
        covered(
            "invoke src/srcexpr",
            1,
            vec![(
                state(r#"<invoke type="scxml" src="c.scxml" srcexpr="'c.scxml'"/>"#),
                "SCXML016",
            )],
        ),
        covered(
            "invoke id/idlocation",
            1,
            vec![(
                state(r#"<invoke type="scxml" src="c.scxml" id="i" idlocation="x"/>"#),
                "SCXML016",
            )],
        ),
        covered(
            "an invoke binds one env slot twice",
            1,
            vec![(
                state(&format!(
                    r#"<invoke xmlns:env="{ENV}" type="urn:svc" env:key="K" env:keyexpr="'K'"/>"#
                )),
                "ENV001",
            )],
        ),
        covered(
            "an invoke env slot is not an env-slots name bound to a variable name",
            1,
            vec![
                (
                    state(&format!(
                        r#"<invoke xmlns:env="{ENV}" type="urn:svc" env:Key="K"/>"#
                    )),
                    "ENV001",
                ),
                (
                    state(&format!(
                        r#"<invoke xmlns:env="{ENV}" typeexpr="'urn:svc'" env:key="1K"/>"#
                    )),
                    "ENV001",
                ),
            ],
        ),
        partly(
            "only an invoked service's invoke binds env slots",
            2,
            vec![
                (
                    state(&format!(
                        r#"<invoke xmlns:env="{ENV}" src="c.scxml" env:key="K"/>"#
                    )),
                    "ENV002",
                ),
                (
                    state(&format!(
                        r#"<invoke xmlns:env="{ENV}" type="scxml" src="c.scxml" env:key="K"/>"#
                    )),
                    "ENV002",
                ),
            ],
            DECODE_ONLY,
        ),
        never(
            "invoke env attributes must be sorted by local name",
            1,
            INVARIANT,
        ),
        never(
            "an invoke attribute is not in the env namespace",
            1,
            INVARIANT,
        ),
        never(
            "an invoke env attribute binds no env-slots name, or one slot twice",
            1,
            DECODE_ONLY,
        ),
        never(
            "an invoke env attribute names no valid variable",
            1,
            DECODE_ONLY,
        ),
        never("invoke env attributes", 1, COUNT_IMPLIED),
        covered(
            "state id",
            1,
            vec![
                (
                    machine(r#"<state id="s"><transition event="go" target="f"/><final id=""/></state>"#),
                    "SCXML003",
                ),
            ],
        ),
        // ---------------------------------------------------------------
        // Names the engine converts (no length bound)
        // ---------------------------------------------------------------
        never("state done event", 1, NAME_IMPLIED),
        never("raise event", 1, NAME_IMPLIED),
        never("send event", 1, NAME_IMPLIED),
        never("send id", 1, NAME_IMPLIED),
        never("invoke id", 1, NAME_IMPLIED),
        never("invoke done event", 1, NAME_IMPLIED),
        // ---------------------------------------------------------------
        // Machine IR bounds (LIMIT005)
        // ---------------------------------------------------------------
        partly(
            "state hierarchy depth",
            3,
            vec![(
                machine(&nested(r#"<state id="d">"#, "</state>", 66, "")
                    .replacen(r#"<state id="d">"#, r#"<state id="s">"#, 1)
                    .replace(r#"<state id="d">"#, "<state>")),
                "LIMIT005",
            )],
            "the two Machine IR hierarchy-index sites repeat the lowering bound, which rejects the depth first",
        ),
        covered(
            "source executable content depth",
            1,
            vec![(
                action(&nested(r#"<foreach array="[1]" item="i">"#, "</foreach>", 65, "")),
                "LIMIT005",
            )],
        ),
        covered(
            "executable content depth",
            1,
            vec![(action(&nested(r#"<if cond="true">"#, "</if>", 65, "")), "LIMIT005")],
        ),
        partly(
            "source XML depth",
            2,
            vec![
                (
                    action(&format!(
                        r#"<send event="e"><content>{}</content></send>"#,
                        nested("<a>", "</a>", 33, "")
                    )),
                    "LIMIT005",
                ),
                (
                    action(&format!(
                        r#"<x:act xmlns:x="urn:x">{}</x:act>"#,
                        nested("<x:a>", "</x:a>", 32, "")
                    )),
                    "LIMIT005",
                ),
            ],
            "",
        ),
        partly(
            "inline XML depth",
            2,
            vec![(
                action(&nested(
                    r#"<if cond="true">"#,
                    "</if>",
                    40,
                    &format!(
                        r#"<send event="e"><content>{}</content></send>"#,
                        nested("<a>", "</a>", 30, "")
                    ),
                )),
                "LIMIT005",
            )],
            "the site in `xml_markup` serializes inline XML at run time, after compilation",
        ),
        never(
            "inline XML markup bytes",
            1,
            "`xml_markup` serializes inline XML at run time, after compilation",
        ),
        covered(
            "source attribute value",
            1,
            vec![(action(&format!(r#"<log label="{long}"/>"#)), "LIMIT005")],
        ),
        covered(
            "inline XML text",
            1,
            vec![(
                action(&format!(r#"<send event="e"><content>{long}</content></send>"#)),
                "LIMIT005",
            )],
        ),
        partly(
            "script body",
            3,
            vec![(action(&format!("<script>//{long}</script>")), "LIMIT005")],
            "the second lowering site guards the same sum against overflow, and the Machine IR site repeats the bound",
        ),
        covered(
            "attribute value",
            1,
            vec![(
                action(&format!(r#"<send event="e"><content><a v="{long}"/></content></send>"#)),
                "LIMIT005",
            )],
        ),
        covered(
            "attribute local name",
            1,
            vec![(
                action(&format!(
                    r#"<send event="e"><content><a b{long}="1"/></content></send>"#
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "element local name",
            1,
            vec![(
                action(&format!(r#"<send event="e"><content><a{long}/></content></send>"#)),
                "LIMIT005",
            )],
        ),
        never(
            "attribute namespace",
            1,
            "every namespace a document uses is recorded as an extension namespace and bounded first (`extension namespaces`)",
        ),
        never(
            "element namespace",
            1,
            "every namespace a document uses is recorded as an extension namespace and bounded first (`extension namespaces`)",
        ),
        covered(
            "extension namespaces",
            2,
            vec![
                (
                    action(&format!(
                        r#"<send event="e"><content><a xmlns:q="urn:{long}" q:v="1"/></content></send>"#
                    )),
                    "LIMIT005",
                ),
                (
                    action(&format!(
                        r#"<send event="e"><content><q:a xmlns:q="urn:{long}"/></content></send>"#
                    )),
                    "LIMIT005",
                ),
                (
                    action(&format!(
                        r#"<send event="e"><content>{}</content></send><send event="e"><content>{}</content></send>"#,
                        (0..5_001)
                            .map(|index| format!(r#"<n:a xmlns:n="urn:a{index}"/>"#))
                            .collect::<String>(),
                        (0..5_001)
                            .map(|index| format!(r#"<n:a xmlns:n="urn:b{index}"/>"#))
                            .collect::<String>()
                    )),
                    "LIMIT005",
                ),
            ],
        ),
        covered(
            "XML attributes",
            1,
            vec![(
                action(&format!(
                    r#"<send event="e"><content><a {}/></content></send>"#,
                    (0..10_001)
                        .map(|index| format!(r#"v{index}="1""#))
                        .collect::<Vec<_>>()
                        .join(" ")
                )),
                "LIMIT005",
            )],
        ),
        never(
            "XML children",
            1,
            "lowering bounds the same child list first (`inline XML nodes`)",
        ),
        covered(
            "inline XML nodes",
            1,
            vec![(
                action(&format!(
                    r#"<send event="e"><content>{}</content></send>"#,
                    repeat("<a/>", 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "assign body",
            1,
            vec![(
                xpath_action(&format!(
                    r#"<assign location="$x">{}</assign>"#,
                    repeat("<a/>", 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "element attributes",
            1,
            vec![(
                machine(&format!(
                    r#"<state id="s" xmlns:q="urn:q" {}><transition event="go" target="f"/></state>"#,
                    (0..10_001)
                        .map(|index| format!(r#"q:a{index}="1""#))
                        .collect::<Vec<_>>()
                        .join(" ")
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "root child nodes",
            1,
            vec![(
                format!(
                    r#"{HEAD}<state id="s"><transition event="go" target="f"/></state>{}<final id="f"/></scxml>"#,
                    repeat("<final/>", 10_001)
                ),
                "LIMIT005",
            )],
        ),
        covered(
            "state child nodes",
            1,
            vec![(
                state(&repeat(r#"<transition event="e"/>"#, 10_001)),
                "LIMIT005",
            )],
        ),
        covered(
            "initial child nodes",
            1,
            vec![(
                state(&format!(
                    r#"<state id="p"><initial>{}</initial><state id="a"/></state>"#,
                    repeat(r#"<transition target="a"/>"#, 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "datamodel child nodes",
            1,
            vec![(
                machine(&format!(
                    r#"<datamodel>{}</datamodel><state id="s"><transition event="go" target="f"/></state>"#,
                    (0..10_001)
                        .map(|index| format!(r#"<data id="d{index}"/>"#))
                        .collect::<String>()
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "root data",
            1,
            vec![(
                machine(&format!(
                    r#"<datamodel>{}</datamodel><datamodel>{}</datamodel><state id="s"><transition event="go" target="f"/></state>"#,
                    (0..5_001)
                        .map(|index| format!(r#"<data id="a{index}"/>"#))
                        .collect::<String>(),
                    (0..5_001)
                        .map(|index| format!(r#"<data id="b{index}"/>"#))
                        .collect::<String>()
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "state data",
            1,
            vec![(
                state(&format!(
                    "<datamodel>{}</datamodel><datamodel>{}</datamodel>",
                    (0..5_001)
                        .map(|index| format!(r#"<data id="a{index}"/>"#))
                        .collect::<String>(),
                    (0..5_001)
                        .map(|index| format!(r#"<data id="b{index}"/>"#))
                        .collect::<String>()
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "invoke child nodes",
            1,
            vec![(
                state(&format!(
                    r#"<invoke type="scxml" src="c.scxml">{}</invoke>"#,
                    repeat(r#"<param name="p" expr="1"/>"#, 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "donedata child nodes",
            1,
            vec![(
                machine(&format!(
                    r#"<state id="s"><transition event="go" target="f"/></state><final id="g"><donedata>{}</donedata></final>"#,
                    repeat(r#"<param name="p" expr="1"/>"#, 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "send child nodes",
            1,
            vec![(
                action(&format!(
                    r#"<send event="e">{}</send>"#,
                    repeat(r#"<param name="p" expr="1"/>"#, 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "executable child nodes",
            1,
            vec![(action(&repeat(r#"<raise event="e"/>"#, 10_001)), "LIMIT005")],
        ),
        covered(
            "executable actions",
            1,
            vec![(
                action(&format!(
                    r#"<if cond="true">{}</if>"#,
                    repeat(r#"<raise event="e"/>"#, 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "elseif branches",
            1,
            vec![(
                action(&format!(
                    r#"<if cond="true">{}</if>"#,
                    repeat(r#"<elseif cond="true"/>"#, 10_001)
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "event descriptors",
            1,
            vec![(
                machine(&format!(
                    r#"<state id="s"><transition event="{}" target="f"/></state>"#,
                    vec!["e"; 10_001].join(" ")
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "name list",
            1,
            vec![(
                action(&format!(
                    r#"<send event="e" namelist="{}"/>"#,
                    vec!["n"; 10_001].join(" ")
                )),
                "LIMIT005",
            )],
        ),
        covered(
            "target list",
            1,
            vec![
                (
                    machine(&format!(
                        r#"<state id="s"><transition event="go" target="{}"/></state>"#,
                        targets(10_001)
                    )),
                    "LIMIT005",
                ),
                (
                    format!(
                        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="{}"><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#,
                        targets(10_001)
                    ),
                    "LIMIT005",
                ),
            ],
        ),
        never(
            "transition targets",
            1,
            "lowering bounds every target list (`target list`) before it drops repeated states",
        ),
        never(
            "initial targets",
            1,
            "lowering bounds every target list (`target list`) before it drops repeated states",
        ),
        covered(
            "multi-target validation items",
            1,
            vec![(multi_target_budget_document(), "LIMIT005")],
        ),
        never("source states", 1, GRAPH_IMPLIED),
        never("source transitions", 1, GRAPH_IMPLIED),
        never("states", 1, GRAPH_IMPLIED),
        never("transitions", 1, GRAPH_IMPLIED),
        // ---------------------------------------------------------------
        // Counts and strings Machine IR re-checks after lowering bounds
        // ---------------------------------------------------------------
        never("action blocks", 1, COUNT_IMPLIED),
        never("content body", 1, COUNT_IMPLIED),
        never("data declarations", 1, "the root and state data counts (`root data`, `state data`) are checked first"),
        never("donedata children", 1, COUNT_IMPLIED),
        never("invoke children", 1, COUNT_IMPLIED),
        never("invokes", 1, COUNT_IMPLIED),
        never("onentry blocks", 1, COUNT_IMPLIED),
        never("onexit blocks", 1, COUNT_IMPLIED),
        never("parameters", 1, COUNT_IMPLIED),
        never("root scripts", 1, COUNT_IMPLIED),
        never("state children", 1, COUNT_IMPLIED),
        never("state invokes", 1, COUNT_IMPLIED),
        never("state transitions", 1, COUNT_IMPLIED),
        never("assign attr", 1, STRING_IMPLIED),
        never("assign expression", 1, STRING_IMPLIED),
        never("cancel sendid", 1, STRING_IMPLIED),
        never("cancel sendidexpr", 1, STRING_IMPLIED),
        never("condition", 1, STRING_IMPLIED),
        never("content expression", 1, STRING_IMPLIED),
        never("data expr", 1, STRING_IMPLIED),
        never("data id", 1, STRING_IMPLIED),
        never("data model", 1, STRING_IMPLIED),
        never("data src", 1, STRING_IMPLIED),
        never("foreach index", 1, STRING_IMPLIED),
        never("inline text", 1, "lowering bounds the same text first (`inline XML text`)"),
        never("log expression", 1, STRING_IMPLIED),
        never("log label", 1, STRING_IMPLIED),
        never("machine name", 1, STRING_IMPLIED),
        never("parameter expression", 1, STRING_IMPLIED),
        never("parameter location", 1, STRING_IMPLIED),
        never("parameter name", 1, STRING_IMPLIED),
        never("event descriptor", 1, "lowering splits `event` on white space, so no descriptor is empty, and the attribute is bounded to 1 MiB"),
        never("name list item", 1, "lowering splits `namelist` on white space, so no item is empty, and the attribute is bounded to 1 MiB"),
        never("blank log expression is not canonical Machine IR", 1, INVARIANT),
        never("executable content extension must be an element", 1, INVARIANT),
        never(
            "executable content extension requires a namespace",
            1,
            "lowering rejects an executable element without a namespace first (`UnsupportedElement`)",
        ),
        never(
            "encoded IR bytes",
            1,
            "the size of the encoded Machine IR exists only after lowering; `compile` reports it as the documented `COMP001` (see `--explain COMP001`)",
        ),
        never("IR bytes", 1, DECODE_ONLY),
        never("one script src URI carries two different pins", 1, PINS_ONLY),
        never("pinned Wasm script requires the script world and no embedded body", 1, PINS_ONLY),
        never("pinned script SHA-256 must be 64 lowercase hex characters", 1, PINS_ONLY),
        never("pinned script text must be embedded and match its SHA-256", 1, PINS_ONLY),
        never("script WIT world", 1, PINS_ONLY),
        never("script media type", 1, PINS_ONLY),
        // ---------------------------------------------------------------
        // Internal Machine IR invariants
        // ---------------------------------------------------------------
        never("action block document order must be strictly increasing", 1, INVARIANT),
        never("ancestor", 1, INVARIANT),
        never("common ancestor", 1, INVARIANT),
        never("default initial child", 2, INVARIANT),
        never("default initial owner", 2, INVARIANT),
        never("event descriptors must be individually normalized", 1, INVARIANT),
        never("history parent", 1, INVARIANT),
        never("history target", 1, INVARIANT),
        never("initial target", 1, INVARIANT),
        never("name list items must be individually normalized", 1, INVARIANT),
        never("new state", 10, INVARIANT),
        never("new state initial", 2, INVARIANT),
        never("only history states may omit an id", 1, INVARIANT),
        never("sorted string table is not strictly increasing", 1, INVARIANT),
        never("state ancestor", 1, INVARIANT),
        never("state ancestor index distance", 1, INVARIANT),
        never("state ancestor index is empty", 1, INVARIANT),
        never("state ancestor index level", 1, INVARIANT),
        never("state child", 1, INVARIANT),
        never("state children do not match parent references", 1, INVARIANT),
        never("state document order must be strictly increasing", 1, INVARIANT),
        never("state hierarchy contains a cycle", 1, INVARIANT),
        never("state hierarchy traversal", 3, INVARIANT),
        never("state hierarchy traversal must cover every state", 1, INVARIANT),
        never("state parent", 3, INVARIANT),
        never("state parent order", 2, INVARIANT),
        never("state transition", 1, INVARIANT),
        never("state transition source", 1, INVARIANT),
        never("target ancestor", 4, INVARIANT),
        never("target depth", 2, INVARIANT),
        never("target preorder", 1, INVARIANT),
        never("transition document order must be strictly increasing", 1, INVARIANT),
        never("transition source", 1, INVARIANT),
        never("transition source ownership", 1, INVARIANT),
        never("transition target", 1, INVARIANT),
    ]
}

/// 5,100 transitions of 20 targets each (102,000 multi-target references,
/// over 100,000 but under the graph's 131,072) into the 20 regions of one
/// `<parallel>`, spread over 51 states so no element exceeds 10,000 child
/// nodes and the graph's shadowing analysis stays within its budget.
fn multi_target_budget_document() -> String {
    let regions: String = (0..20)
        .map(|index| format!(r#"<state id="r{index}"/>"#))
        .collect();
    let set = (0..20)
        .map(|index| format!("r{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let states: String = (0..51)
        .map(|state| {
            let transitions: String = (0..100)
                .map(|index| format!(r#"<transition event="e{index}" target="{set}"/>"#))
                .collect();
            format!(r#"<state id="t{state}">{transitions}</state>"#)
        })
        .collect();
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="p"><parallel id="p">{regions}</parallel>{states}</scxml>"#
    )
}

// ---------------------------------------------------------------------------
// Source scan
// ---------------------------------------------------------------------------

const HELPERS: [&str; 12] = [
    "checked_name",
    "ensure_count",
    "bounded_count",
    "bounded_required",
    "bounded_optional",
    "require_empty",
    "require_none",
    "require_at_most_one",
    "require_exactly_one",
    "checked_index",
    "validate_index_list",
    "validate_sorted_unique_strings",
];

/// Production source: without the unit-test module and comment lines.
fn production(source: &str) -> String {
    let end = source
        .find("#[cfg(test)]\nmod tests")
        .unwrap_or(source.len());
    source[..end]
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The string literal starting at `text` after optional white space.
fn literal(text: &str) -> Option<&str> {
    let text = text.trim_start().strip_prefix('"')?;
    text.find('"').map(|end| &text[..end])
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Every rejection key in `source`, one per site.
fn rejection_sites(source: &str) -> Vec<String> {
    let mut sites = Vec::new();
    for (offset, _) in source.match_indices("LowerError::") {
        let rest = &source[offset + "LowerError::".len()..];
        let variant_end = rest
            .bytes()
            .position(|byte| !is_identifier_byte(byte))
            .unwrap_or(rest.len());
        let variant = &rest[..variant_end];
        let after = &rest[variant_end..];
        match variant {
            "InvalidStructure" | "InvalidReference" | "InvalidName" => {
                if let Some(key) = after.trim_start().strip_prefix('(').and_then(literal) {
                    sites.push(key.to_owned());
                }
            }
            "LimitExceeded" => {
                if let Some(key) = after
                    .trim_start()
                    .strip_prefix('{')
                    .map(str::trim_start)
                    .and_then(|field| field.strip_prefix("field:"))
                    .and_then(literal)
                {
                    sites.push(key.to_owned());
                }
            }
            _ => sites.push(variant.to_owned()),
        }
    }
    for helper in HELPERS {
        for (offset, _) in source.match_indices(&format!("{helper}(")) {
            if offset > 0 && is_identifier_byte(source.as_bytes()[offset - 1]) {
                continue;
            }
            if let Some(key) = literal(&source[offset + helper.len() + 1..]) {
                sites.push(key.to_owned());
            }
        }
    }
    for (offset, _) in source.match_indices("reject_blank(") {
        if offset > 0 && is_identifier_byte(source.as_bytes()[offset - 1]) {
            continue;
        }
        let rest = &source[offset + "reject_blank(".len()..];
        if let Some(key) = rest.find(',').and_then(|comma| literal(&rest[comma + 1..])) {
            sites.push(key.to_owned());
        }
    }
    sites
}

#[test]
fn every_lowering_rejection_site_is_classified() {
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for source in [LOWER_RS, IR_RS, LIB_RS] {
        for key in rejection_sites(&production(source)) {
            *found.entry(key).or_insert(0) += 1;
        }
    }
    let table = table();
    let mut listed: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &table {
        assert!(
            listed.insert(entry.key, entry.sites).is_none(),
            "{} is listed twice",
            entry.key
        );
        assert!(
            !entry.documents.is_empty() || !entry.unreachable.is_empty(),
            "{}: give documents that reach it or the reason none can",
            entry.key
        );
    }
    let mut failures = Vec::new();
    for (key, sites) in &found {
        match listed.get(key.as_str()) {
            Some(listed_sites) if listed_sites == sites => {}
            Some(listed_sites) => failures.push(format!(
                "`{key}`: {sites} source site(s), the table lists {listed_sites}"
            )),
            None => failures.push(format!("`{key}`: new rejection site, not classified")),
        }
    }
    for key in listed.keys() {
        if !found.contains_key(*key) {
            failures.push(format!("`{key}`: listed but no longer a rejection site"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // GRAPH_IMPLIED: the validator's graph bounds are at or below the
    // compiler's (both are `usize::MAX` natively, where neither applies).
    assert!(!scxml_types::exceeds_wasm_bound(
        scxml_validator_core::MAX_GRAPH_NODES,
        scxml_compiler_core::MAX_TABLE_ITEMS
    ));
    assert!(!scxml_types::exceeds_wasm_bound(
        scxml_validator_core::MAX_GRAPH_TRANSITIONS,
        scxml_compiler_core::MAX_TABLE_ITEMS
    ));
}

#[test]
fn the_validator_mirrors_the_compiler_bounds() {
    use scxml_validator_core::lowering_bounds;
    assert_eq!(
        lowering_bounds::MAX_CONTENT_DEPTH,
        scxml_compiler_core::MAX_CONTENT_DEPTH
    );
    assert_eq!(
        lowering_bounds::MAX_NESTED_ITEMS,
        scxml_compiler_core::MAX_NESTED_ITEMS
    );
    assert_eq!(
        lowering_bounds::MAX_STRING_BYTES,
        scxml_compiler_core::MAX_STRING_BYTES
    );
    assert_eq!(
        lowering_bounds::MAX_MULTI_TARGET_ITEMS,
        scxml_compiler_core::MAX_MULTI_TARGET_VALIDATION_ITEMS
    );
}

/// Checks one document of `key`'s entry and returns what it found wrong.
fn check_rejection_document(key: &str, label: &str, source: &str, code: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let document = match parse_xml(SourceText::new(SOURCE, "case.scxml", source)) {
        Ok(document) => document,
        Err(error) => {
            failures.push(format!("{label}: not well-formed: {error:?}"));
            return failures;
        }
    };
    if !cfg!(target_arch = "wasm32") && is_bound_key(key) {
        native_bound_document(label, source, &document, &mut failures);
        return failures;
    }
    match lower(&document) {
        Ok(_) => failures.push(format!("{label}: lowering accepted it")),
        Err(error) if !format!("{error:?}").contains(key) => {
            failures.push(format!("{label}: lowering failed with {error:?} first"));
        }
        Err(_) => {}
    }
    let report = common::validate(source);
    let errors: Vec<&str> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    if !errors.iter().any(|found| found.starts_with(code)) {
        failures.push(format!(
            "{label}: validate reports no {code} error, only {errors:?}"
        ));
    }
    let output = common::compile(source);
    if output
        .validation
        .report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == "COMP001")
    {
        failures.push(format!("{label}: compile reached lowering (COMP001)"));
    }
    failures
}

#[test]
fn every_reachable_lowering_rejection_is_a_validator_error() {
    let table = table();
    let documents: Vec<(&str, String, &str, &str)> = table
        .iter()
        .flat_map(|entry| {
            entry
                .documents
                .iter()
                .enumerate()
                .map(|(index, (source, code))| {
                    (
                        entry.key,
                        format!("`{}` document {index}", entry.key),
                        source.as_str(),
                        *code,
                    )
                })
        })
        .collect();
    // Every document is checked on its own, so they run on a pool of
    // threads; the findings are reported in table order.
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Vec<String>>> = documents.iter().map(|_| Mutex::default()).collect();
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((key, label, source, code)) = documents.get(index) else {
                        break;
                    };
                    *results[index]
                        .lock()
                        .expect("no check panicked holding its slot") =
                        check_rejection_document(key, label, source, code);
                }
            });
        }
    });
    let failures: Vec<String> = results
        .into_iter()
        .flat_map(|slot| {
            slot.into_inner()
                .expect("no check panicked holding its slot")
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Whether `key` names a Machine IR bound: a `LimitExceeded` field, passed
/// to a bounded-count or bounded-string helper or a `field:` initializer.
fn is_bound_key(key: &str) -> bool {
    let quoted = format!("\"{key}\"");
    [LOWER_RS, IR_RS, LIB_RS].iter().any(|source| {
        source.match_indices(&quoted).any(|(at, _)| {
            let before = source[..at].trim_end();
            [
                "ensure_count(",
                "bounded_count(",
                "bounded_required(",
                "bounded_optional(",
                "field:",
            ]
            .iter()
            .any(|helper| before.ends_with(helper))
        })
    })
}

/// Machine IR bounds apply to wasm builds only: natively lowering never
/// rejects a document over one, `validate` reports no `LIMIT005`, and
/// `compile` reaches no `COMP001`. A fixture that is also invalid for an
/// SCXML rule still fails lowering on that rule, which `validate` reports.
fn native_bound_document(
    label: &str,
    source: &str,
    document: &scxml_syntax::Document,
    failures: &mut Vec<String>,
) {
    let report = common::validate(source);
    if let Err(error) = lower(document) {
        let error = format!("{error:?}");
        if error.contains("LimitExceeded") {
            failures.push(format!("{label}: native lowering hit a bound: {error}"));
        } else if !report.has_errors() {
            failures.push(format!(
                "{label}: native lowering rejected it ({error}) but validate is clean"
            ));
        }
    }
    if report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == "LIMIT005")
    {
        failures.push(format!("{label}: native validate reported LIMIT005"));
    }
    let output = common::compile(source);
    if output
        .validation
        .report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == "COMP001")
    {
        failures.push(format!("{label}: native compile reported COMP001"));
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one list names every padded attribute case"
)]
fn padded_cases() -> Vec<(&'static str, String)> {
    let root = |attributes: &str| {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" {attributes}><state id="s"/></scxml>"#
        )
    };
    let xpath_assign = |attributes: &str| {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath"><datamodel><data id="d"><a/></data></datamodel><state id="s"><onentry><assign {attributes} expr="1"/></onentry></state></scxml>"#
        )
    };
    vec![
        ("<raise event>", action(r#"<raise event=" go "/>"#)),
        ("<send event>", action(r#"<send event=" go&#9;"/>"#)),
        (
            "<send target>",
            action(r#"<send event="go" target=" #_internal "/>"#),
        ),
        (
            "<cancel sendid>",
            action(r#"<send event="go" id="i" delay="1s"/><cancel sendid=" i "/>"#),
        ),
        (
            "<param name>",
            action(r#"<send event="go"><param name=" p " expr="1"/></send>"#),
        ),
        (
            "<transition event, target, type>",
            state(r#"<transition event=" go  done.* " target=" s " type=" internal "/>"#),
        ),
        (
            "<state initial>",
            machine(r#"<state id="s" initial=" a "><state id="a"/></state>"#),
        ),
        (
            "<invoke src, autoforward>",
            state(r#"<invoke id="inv" src=" c.scxml " autoforward=" true "/>"#),
        ),
        (
            "<data src>",
            machine(r#"<datamodel><data id="d" src=" d.json "/></datamodel><state id="s"/>"#),
        ),
        (
            "<script src>",
            machine(r#"<script src=" lib.js "/><state id="s"/>"#),
        ),
        (
            "<scxml version, initial, name, binding>",
            root(r#"version=" 1.0 " initial=" s " name=" n " binding=" late ""#),
        ),
        (
            "<assign type>",
            xpath_assign(r#"location="$d" type=" lastchild ""#),
        ),
        (
            "<assign attr>",
            xpath_assign(r#"location="$d/a" type="addattribute" attr=" x ""#),
        ),
        (
            "<assign type> addattribute",
            xpath_assign(r#"location="$d/a" type=" addattribute&#9;" attr="x""#),
        ),
        (
            "<assign type> delete",
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath"><datamodel><data id="d"><a/></data></datamodel><state id="s"><onentry><assign location="$d/a" type=" delete "/></onentry></state></scxml>"#.to_owned(),
        ),
        (
            "<state id> (target, initial, In())",
            machine(
                r#"<state id=" s "><transition event="go" cond="In('s')" target="s2"/></state><state id="s2&#10;" initial=" a"><state id="a  "><transition event="go" target=" f "/></state></state>"#,
            ),
        ),
        (
            "<parallel, final, history id>",
            machine(
                r#"<state id="s"><transition event="go" target="h"/></state><state id="w"><parallel id=" p "><state id="r"><history id=" h "><transition target="f2"/></history><final id=" f2 "/></state></parallel><transition event="done.state.p" target="f"/></state>"#,
            ),
        ),
        (
            "<initial><transition target>",
            machine(
                r#"<state id="s"><initial><transition target=" a "/></initial><state id="a"/><transition event="go" target="f"/></state>"#,
            ),
        ),
        (
            "<data id>",
            machine(
                r#"<datamodel><data id=" d " expr="1"/></datamodel><state id="s"><transition event="go" cond="d === 1" target="f"/></state>"#,
            ),
        ),
        (
            "<send id>, <cancel sendid>",
            action(r#"<send event="go" id=" i " delay="1s"/><cancel sendid="i"/>"#),
        ),
        (
            "<invoke id>",
            state(r#"<invoke id=" inv " src="c.scxml"/><transition event="done.invoke.inv" target="f"/>"#),
        ),
        (
            "<scxml datamodel>",
            root(r#"version="1.0" datamodel=" ecmascript&#10;""#),
        ),
        (
            "<raise event> at the name bound",
            action(&format!(r#"<raise event="  {}  "/>"#, "e".repeat(1_024))),
        ),
        (
            "<send event, id> at the name bound",
            action(&format!(
                r#"<send event=" {} " id="&#10;{}&#10;"/>"#,
                "e".repeat(1_024),
                "i".repeat(1_024)
            )),
        ),
        (
            "<invoke id> at the done.invoke bound",
            state(&format!(r#"<invoke id="  {}  " src="c.scxml"/>"#, "i".repeat(1_012))),
        ),
        (
            "<state id> at the name and done.state bounds",
            machine(&format!(
                r#"<state id="s"><transition event="go" target="f"/></state><state id=" {} "/><state id="&#9;{}&#9;"><final id="g"/></state>"#,
                "a".repeat(1_024),
                "c".repeat(1_013)
            )),
        ),
        (
            "<scxml datamodel> xpath",
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel=" xpath "><datamodel><data id=" d "/></datamodel><state id="s"><onentry><assign location="$d" expr="1"/></onentry></state></scxml>"#.to_owned(),
        ),
    ]
}

/// An attribute whose XSD type collapses white space (XML Schema 1.1 Part 2
/// §4.3.6: `xsd:token` and its derived types, `xsd:anyURI`, `xsd:decimal`)
/// is valid with leading, trailing, or repeated white space, and its value
/// is the collapsed one. Each document below validates with no error, and
/// lowering accepts it and carries only the collapsed value into Machine IR
/// (no IR string begins or ends with a space).
#[test]
fn padded_token_attributes_validate_and_lower_to_their_collapsed_values() {
    let mut failures = Vec::new();
    for (label, source) in &padded_cases() {
        let errors: Vec<String> = common::validate(source)
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect();
        if !errors.is_empty() {
            failures.push(format!("{label}: validate reports {errors:?}"));
        }
        let document = parse_xml(SourceText::new(SOURCE, "case.scxml", source.as_str()))
            .expect("the case is well-formed");
        match lower(&document) {
            Err(error) => failures.push(format!("{label}: lowering failed with {error:?}")),
            Ok(ir) => {
                let json: serde_json::Value =
                    serde_json::from_slice(&ir.to_json().expect("the IR encodes"))
                        .expect("IR JSON decodes");
                let mut padded = Vec::new();
                padded_strings(&json, &mut padded);
                if !padded.is_empty() {
                    failures.push(format!("{label}: IR keeps padded values {padded:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// White space in XML is #x20, #x9, #xD and #xA only (XML 1.0 §2.3 `S`).
/// U+1680 OGHAM SPACE MARK is Unicode white space but an XML
/// `NameStartChar` (`[#x37F-#x1FFF]`), so an `id`, `initial`, `target`,
/// or event descriptor containing it is valid, and validate and lowering
/// both read it as part of the name; U+00A0 is neither, so an event
/// descriptor of it is an invalid descriptor, not an empty list. Text of
/// it in executable content is text, which validate and lowering judge
/// alike.
#[test]
fn only_xml_white_space_separates_or_pads_values() {
    let mut failures = Vec::new();
    let clean = [(
        "U+1680 in ids, targets, and descriptors",
        machine(
            "<state id=\"s\" initial=\"&#x1680;c\"><state id=\"&#x1680;c\"><transition event=\"&#x1680; go&#x1680;x\" target=\"a&#x1680;b\"/></state><state id=\"a&#x1680;b\"><onentry><raise event=\"&#x1680;\"/></onentry><transition event=\"&#x1680;\" target=\"f\"/></state></state>",
        ),
    )];
    for (label, source) in &clean {
        let errors: Vec<String> = common::validate(source)
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect();
        if !errors.is_empty() {
            failures.push(format!("{label}: validate reports {errors:?}"));
        }
        let document = parse_xml(SourceText::new(SOURCE, "case.scxml", source.as_str()))
            .expect("the case is well-formed");
        if let Err(error) = lower(&document) {
            failures.push(format!("{label}: lowering failed with {error:?}"));
        }
    }
    let no_break = state("<transition event=\"&#xA0;\" target=\"f\"/>");
    let report = common::validate(&no_break);
    if report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.message.contains("at least one event descriptor"))
    {
        failures.push("U+00A0 descriptor: reported as an empty list".to_owned());
    }
    for (label, source) in [
        (
            "U+1680 text in <onentry>",
            machine(
                "<state id=\"s\"><onentry>&#x1680;</onentry><transition event=\"go\" target=\"f\"/></state>",
            ),
        ),
        (
            "U+1680 text in <if>",
            machine(
                "<state id=\"s\"><onentry><if cond=\"true\">&#x1680;</if></onentry><transition event=\"go\" target=\"f\"/></state>",
            ),
        ),
    ] {
        let validated = common::validate(&source).has_errors();
        let document = parse_xml(SourceText::new(SOURCE, "case.scxml", source.as_str()))
            .expect("the case is well-formed");
        let lowered = lower(&document).is_err();
        if validated != lowered {
            failures.push(format!(
                "{label}: validate has errors {validated}, lowering fails {lowered}"
            ));
        }
    }
    // The schema engine splits list types on Unicode white space, but a
    // list item holding U+00A0 (not a name character) or a Boolean holding
    // U+1680 is still an error in validate, as in lowering.
    for (label, source) in [
        (
            "U+00A0 in <invoke autoforward>",
            state("<invoke id=\"i\" src=\"c.scxml\" autoforward=\"true&#xA0;\"/>"),
        ),
        (
            "U+1680 in <invoke autoforward>",
            state("<invoke id=\"i\" src=\"c.scxml\" autoforward=\"true&#x1680;\"/>"),
        ),
        (
            "U+00A0 in <state initial>",
            machine(
                "<state id=\"s\" initial=\"a&#xA0;b\"><state id=\"a\"/><state id=\"b\"/><transition event=\"go\" target=\"f\"/></state>",
            ),
        ),
        (
            "U+00A0 in <transition target>",
            state("<transition event=\"x\" target=\"s&#xA0;f\"/>"),
        ),
    ] {
        let document = parse_xml(SourceText::new(SOURCE, "case.scxml", source.as_str()))
            .expect("the case is well-formed");
        let validated = common::validate(&source).has_errors();
        let lowered = lower(&document).is_err();
        if !validated || !lowered {
            failures.push(format!(
                "{label}: validate has errors {validated}, lowering fails {lowered}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A value blank only in Unicode, such as U+1680 OGHAM SPACE MARK, is not
/// blank in XML (XML 1.0 §2.3 `S`): `<log expr="&#x1680;">` is an
/// expression, which lowering keeps, Machine IR loads, and the validator
/// checks instead of reporting it as an empty `expr` (`LINT009`).
#[test]
fn a_value_of_other_unicode_white_space_is_not_blank() {
    let source = xpath_action(r#"<log expr="&#x1680;"/>"#);
    let document =
        parse_xml(SourceText::new(SOURCE, "case.scxml", source.as_str())).expect("well-formed");
    let bytes = lower(&document)
        .expect("lowers")
        .to_json()
        .expect("the IR encodes");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("IR JSON decodes");
    assert!(
        json.to_string().contains('\u{1680}'),
        "lowering keeps the expression: {json}"
    );
    scxml_compiler_core::MachineIr::from_json(&bytes).expect("Machine IR loads the expression");
    let report = common::validate(&source);
    assert!(
        !report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "LINT009"),
        "validate reads an expression, not an empty `expr`: {:?}",
        report.diagnostics()
    );
}

/// `<scxml exmode>` (`lax` or `strict`) is declared by the W3C schema
/// (`scxml-module-core.xsd`) and given no semantics by the Recommendation,
/// so a document carrying it validates, and lowers exactly as the same
/// document without it.
#[test]
fn exmode_validates_and_lowers_with_no_effect() {
    let source = |exmode: &str| {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="s"{exmode}><state id="s"><transition event="go" target="f"/></state><final id="f"/></scxml>"#
        )
    };
    let lowered = |source: &str| {
        let document = parse_xml(SourceText::new(SOURCE, "case.scxml", source))
            .expect("the case is well-formed");
        lower(&document).map(|ir| ir.to_json().expect("the IR encodes"))
    };
    let plain = lowered(&source("")).expect("the plain document lowers");
    let mut failures = Vec::new();
    for exmode in [
        r#" exmode="lax""#,
        r#" exmode="strict""#,
        r#" exmode=" strict ""#,
    ] {
        let document = source(exmode);
        let errors: Vec<String> = common::validate(&document)
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect();
        if !errors.is_empty() {
            failures.push(format!("{exmode}: validate reports {errors:?}"));
        }
        match lowered(&document) {
            Ok(ir) if ir == plain => {}
            Ok(_) => failures.push(format!("{exmode}: lowers to different IR")),
            Err(error) => failures.push(format!("{exmode}: lowering failed with {error:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every string in `value` that begins or ends with XML white space.
fn padded_strings(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if text.trim_matches(is_xml_space) != text => {
            found.push(text.clone());
        }
        serde_json::Value::Array(items) => {
            for item in items {
                padded_strings(item, found);
            }
        }
        serde_json::Value::Object(members) => {
            for member in members.values() {
                padded_strings(member, found);
            }
        }
        _ => {}
    }
}
