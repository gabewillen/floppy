//! Property tests at the Machine IR decoder trust boundary
//! (`MachineIr::from_json`).
//!
//! Generated SCXML charts must lower to IR that round-trips through the
//! canonical encoder. Hostile input (arbitrary bytes, and structural
//! mutations of real lowered IR) must never panic the decoder, unknown
//! fields must be rejected, and anything accepted must satisfy every IR
//! invariant (it re-encodes and decodes to the same value). Runs are
//! deterministic: a fixed RNG seed, a fixed case count, and no
//! failure-persistence files.

use std::fmt::Write as _;
use std::sync::OnceLock;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use scxml_compiler_core::{MachineIr, SCXML_NAMESPACE, lower};
use scxml_syntax::{SourceId, SourceText, parse_xml};
use serde_json::Value;

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

fn config() -> Config {
    Config {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x1a70_de0c),
        ..Config::default()
    }
}

fn lower_source(source: &str) -> MachineIr {
    let document = parse_xml(SourceText::new(SOURCE, "m.scxml", source.to_owned()))
        .expect("generated SCXML is well-formed XML");
    lower(&document).expect("generated SCXML lowers")
}

/// Canonical IR bytes of a chart that exercises every state kind and most
/// executable content.
fn seed_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let source = format!(
            r##"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="start" datamodel="ecmascript">
  <datamodel><data id="x" expr="1"/></datamodel>
  <state id="start">
    <onentry>
      <raise event="raised"/><log label="l" expr="x"/><assign location="x" expr="2"/>
      <if cond="a"><raise event="a"/><elseif cond="b"/><raise event="b"/><else/><raise event="c"/></if>
      <foreach array="items" item="item" index="idx"><log expr="item"/></foreach>
      <send event="out" target="#_internal" id="send1" delay="1s"><param name="p" expr="x"/></send>
      <cancel sendid="send1"/>
    </onentry>
    <transition event="go" cond="ready" target="both"/>
  </state>
  <parallel id="both"><state id="left"/><state id="right"/><transition event="done" target="compound"/></parallel>
  <state id="compound">
    <initial><transition target="leaf"/></initial>
    <state id="leaf"><transition event="end" target="done"/></state>
    <history id="hist" type="deep"><transition target="leaf"/></history>
  </state>
  <final id="done"/>
</scxml>"##
        );
        lower_source(&source).to_json().expect("seed IR encodes")
    })
}

fn seed_value() -> Value {
    serde_json::from_slice(seed_bytes()).expect("IR bytes are JSON")
}

fn pointers(value: &Value, prefix: &str, output: &mut Vec<String>) {
    output.push(prefix.to_owned());
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                pointers(child, &format!("{prefix}/{escaped}"), output);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                pointers(child, &format!("{prefix}/{index}"), output);
            }
        }
        _ => {}
    }
}

fn replacement() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<u64>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        (0_u64..16).prop_map(Value::from),
        any::<String>().prop_map(Value::String),
        Just(Value::Array(Vec::new())),
        Just(Value::Object(serde_json::Map::new())),
    ]
}

fn assert_accepted_is_canonical(bytes: &[u8]) {
    if let Ok(ir) = MachineIr::from_json(bytes) {
        let encoded = ir.to_json().expect("accepted IR re-encodes");
        assert_eq!(
            MachineIr::from_json(&encoded).expect("re-encoded IR decodes"),
            ir
        );
    }
}

/// A flat chart: `count` atomic states, each with transitions to indexed
/// targets, and one final state.
fn chart() -> impl Strategy<Value = String> {
    (1_usize..12).prop_flat_map(|count| {
        prop::collection::vec(prop::collection::vec(0..=count, 0..4), count).prop_map(
            move |transitions| {
                let mut source = format!(
                    r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="s0" datamodel="null">"#
                );
                for (index, targets) in transitions.iter().enumerate() {
                    write!(source, r#"<state id="s{index}">"#)
                        .expect("writing to a String cannot fail");
                    for (event, target) in targets.iter().enumerate() {
                        let target = if *target == count {
                            "end".to_owned()
                        } else {
                            format!("s{target}")
                        };
                        write!(source, r#"<transition event="e{event}" target="{target}"/>"#)
                            .expect("writing to a String cannot fail");
                    }
                    source.push_str("</state>");
                }
                source.push_str(r#"<final id="end"/></scxml>"#);
                source
            },
        )
    })
}

#[test]
fn seed_round_trips_byte_for_byte() {
    let ir = MachineIr::from_json(seed_bytes()).expect("seed IR decodes");
    assert_eq!(ir.to_json().expect("seed IR re-encodes"), seed_bytes());
}

proptest! {
    #![proptest_config(config())]

    /// Generated charts lower to canonical IR that round-trips byte for byte.
    #[test]
    fn generated_charts_round_trip(source in chart()) {
        let ir = lower_source(&source);
        let bytes = ir.to_json().expect("lowered IR encodes");
        let decoded = MachineIr::from_json(&bytes).expect("lowered IR decodes");
        prop_assert_eq!(decoded.to_json().expect("decoded IR re-encodes"), bytes);
        prop_assert_eq!(decoded, ir);
    }

    /// Arbitrary bytes never panic the decoder.
    #[test]
    fn decoder_is_total_on_arbitrary_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..1_024)
    ) {
        assert_accepted_is_canonical(&bytes);
    }

    /// Replacing any node of real IR with a hostile value is either rejected
    /// or yields IR that still satisfies every invariant.
    #[test]
    fn structural_mutations_fail_closed(
        index in any::<prop::sample::Index>(),
        value in replacement(),
    ) {
        let mut document = seed_value();
        let mut all = Vec::new();
        pointers(&document, "", &mut all);
        let pointer = &all[index.index(all.len())];
        if let Some(slot) = document.pointer_mut(pointer) {
            *slot = value;
        }
        let bytes = serde_json::to_vec(&document).expect("mutated JSON serializes");
        assert_accepted_is_canonical(&bytes);
    }

    /// An unknown field in any object of the IR is rejected.
    #[test]
    fn unknown_fields_are_rejected(index in any::<prop::sample::Index>()) {
        let mut document = seed_value();
        let mut all = Vec::new();
        pointers(&document, "", &mut all);
        let objects: Vec<_> = all
            .into_iter()
            .filter(|pointer| document.pointer(pointer).is_some_and(Value::is_object))
            .collect();
        let pointer = &objects[index.index(objects.len())];
        if let Some(Value::Object(map)) = document.pointer_mut(pointer) {
            map.insert("x-unknown-property".into(), Value::Null);
        }
        let bytes = serde_json::to_vec(&document).expect("mutated JSON serializes");
        prop_assert!(MachineIr::from_json(&bytes).is_err(), "accepted unknown field at `{pointer}`");
    }
}
