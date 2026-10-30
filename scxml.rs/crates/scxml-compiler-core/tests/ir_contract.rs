//! Contract tests for deterministic, fail-closed Machine IR lowering and decoding.

use scxml_compiler_core::{
    ArtifactKind, ExecutableContentIr, LowerError, MACHINE_IR_VERSION, MachineIr, SCXML_NAMESPACE,
    StateKind, lower,
};
use scxml_syntax::{Document, SourceId, SourceText, parse_xml};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

fn parse(source: &str) -> Document {
    parse_xml(SourceText::new(SOURCE, "machine.scxml", source.to_owned()))
        .expect("test SCXML must be well-formed XML")
}

#[test]
fn complete_lowering_is_deterministic_and_round_trips() {
    let source = format!(
        r##"<scxml xmlns="{SCXML_NAMESPACE}" xmlns:x="urn:test:custom" version="1.0" initial="start" name="machine" datamodel="ecmascript" binding="late">
  <datamodel><data id="rootData" expr="1"/></datamodel>
  <script>boot()</script>
  <state id="start">
    <onentry>
      <raise event="raised"/><log label="label" expr="value"/><assign location="x" expr="2"/>
      <if cond="a"><raise event="a"/><elseif cond="b"/><raise event="b"/><else/><raise event="c"/></if>
      <foreach array="items" item="item" index="idx"><log expr="item"/></foreach>
      <send event="out" target="#_internal" type="scxml" id="send1" delay="1s" namelist="x y"><param name="p" expr="x"/></send>
      <send eventexpr="eventName"><content><x:payload key="v">body</x:payload></content></send>
      <cancel sendid="send1"/><script src="logic.js"/><x:action x:mode="safe"><x:nested/></x:action>
    </onentry>
    <invoke type="scxml" src="child.scxml" id="child" namelist="x" autoforward="true"><param name="p" location="x"/><content expr="payload"/><finalize><log expr="done"/></finalize></invoke>
    <transition event="go next.*" cond="ready" target="left right"/>
  </state>
  <parallel id="both"><state id="left"/><state id="right"/></parallel>
  <state id="compound">
    <initial><transition target="leaf"><raise event="initial"/></transition></initial>
    <state id="leaf"/>
    <history id="hist" type="deep"><transition target="leaf"/></history>
    <transition type="internal" target="leaf"/>
  </state>
  <final id="done"><onentry><log label="Outcome" expr="'pass'"/></onentry><donedata><param name="result" expr="1"/></donedata></final>
</scxml>"##
    );
    let first = lower(&parse(&source)).expect("complete supported SCXML must lower");
    let second = lower(&parse(&source)).expect("same source must lower deterministically");
    assert_eq!(first, second);
    assert_eq!(first.version, MACHINE_IR_VERSION);
    assert!(
        first
            .states
            .iter()
            .any(|state| state.kind == StateKind::Parallel)
    );
    assert!(
        first
            .states
            .iter()
            .any(|state| state.kind == StateKind::Compound)
    );
    assert!(
        first
            .states
            .iter()
            .any(|state| state.kind == StateKind::DeepHistory)
    );
    assert_eq!(
        first
            .transitions
            .first()
            .expect("complex fixture must lower its first transition")
            .targets
            .len(),
        2
    );
    let bytes = first.to_json().expect("valid IR must serialize");
    assert_eq!(
        bytes,
        second
            .to_json()
            .expect("same IR must serialize identically")
    );
    assert_eq!(
        MachineIr::from_json(&bytes).expect("canonical IR must decode"),
        first
    );
}

#[test]
fn hostile_decode_rejects_unknown_fields_versions_references_and_size() {
    let source =
        format!(r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="ready"/></scxml>"#);
    let ir = lower(&parse(&source)).expect("minimal machine must lower");
    let mut value = serde_json::to_value(&ir).expect("test IR must encode as a value");
    value
        .as_object_mut()
        .expect("Machine IR JSON must be an object")
        .insert("unknown".into(), serde_json::Value::Bool(true));
    let bytes = serde_json::to_vec(&value).expect("test JSON value must encode");
    assert_eq!(
        MachineIr::from_json(&bytes),
        Err(LowerError::InvalidIrSchema)
    );

    let mut wrong_version = ir.clone();
    wrong_version.version = MACHINE_IR_VERSION + 1;
    let bytes = serde_json::to_vec(&wrong_version).expect("test IR must encode");
    assert_eq!(
        MachineIr::from_json(&bytes),
        Err(LowerError::UnsupportedIrVersion(MACHINE_IR_VERSION + 1))
    );

    // A future version with a different closed schema is still reported as
    // an unsupported version, not as malformed IR.
    let future = serde_json::json!({"version": MACHINE_IR_VERSION + 1, "future_field": []});
    let bytes = serde_json::to_vec(&future).expect("future-version JSON must encode");
    assert_eq!(
        MachineIr::from_json(&bytes),
        Err(LowerError::UnsupportedIrVersion(MACHINE_IR_VERSION + 1))
    );
    assert_eq!(
        MachineIr::from_json(br#"{"states": []}"#),
        Err(LowerError::InvalidIrSchema),
        "IR without a version is malformed"
    );

    let mut bad_reference = ir;
    *bad_reference
        .initial
        .targets
        .first_mut()
        .expect("minimal fixture must have a root initial target") = u32::MAX;
    let bytes = serde_json::to_vec(&bad_reference).expect("test IR must encode");
    assert_eq!(
        MachineIr::from_json(&bytes),
        Err(LowerError::InvalidReference("initial targets"))
    );

    // The encoded-size bound applies to wasm builds only.
    #[cfg(not(target_arch = "wasm32"))]
    assert_eq!(scxml_compiler_core::MAX_IR_BYTES, usize::MAX);
}

#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_only_decode_rejects_ir_over_the_byte_bound() {
    let oversized = vec![b' '; scxml_compiler_core::MAX_IR_BYTES + 1];
    assert_eq!(
        MachineIr::from_json(&oversized),
        Err(LowerError::LimitExceeded {
            field: "IR bytes",
            limit: scxml_compiler_core::MAX_IR_BYTES,
        })
    );
}

#[test]
fn lowering_rejects_unsupported_source_instead_of_dropping_it() {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="ready"><unsupported/></state></scxml>"#
    );
    assert_eq!(
        lower(&parse(&source)),
        Err(LowerError::UnsupportedElement("unsupported".into()))
    );
}

#[test]
fn targetless_transition_is_preserved() {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="ready"><transition event="tick"><raise event="again"/></transition></state></scxml>"#
    );
    let ir = lower(&parse(&source)).expect("targetless SCXML transition must lower");
    let transition = ir
        .transitions
        .first()
        .expect("targetless fixture must lower one transition");
    assert!(transition.targets.is_empty());
    assert_eq!(transition.events, ["tick"]);
}

/// A blank `<log expr>` carries no value and lowers exactly as if `expr`
/// were absent (upstream W3C IRP case 307 authors `<log expr="">`); decoded
/// IR that still carries one is not canonical and is rejected. A blank
/// `<transition cond>` is an invalid expression, not an absent one (SCXML
/// 3.13, 5.9): it is preserved so evaluation raises `error.execution` and
/// the transition is not taken.
#[test]
fn blank_log_expr_lowers_as_absent_but_blank_cond_is_preserved() {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript"><state id="ready"><transition event="go" cond=" " target="ready"><log label="l" expr=""/></transition><transition event="stay" cond="x" target="ready"><log label="m" expr="v"/></transition></state></scxml>"#
    );
    let ir = lower(&parse(&source)).expect("blank expressions must lower");
    let conditions = ir
        .transitions
        .iter()
        .map(|transition| transition.cond.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(conditions, [Some(" "), Some("x")]);
    let log_exprs = ir
        .transitions
        .iter()
        .flat_map(|transition| &transition.actions)
        .map(|action| match action {
            ExecutableContentIr::Log { expr, .. } => expr.as_deref(),
            other => panic!("fixture transitions only log: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(log_exprs, [None, Some("v")]);

    let mut blank_condition = ir.clone();
    blank_condition.transitions[0].cond = Some(String::new());
    assert_eq!(
        blank_condition.validate(),
        Ok(()),
        "a blank condition is canonical: evaluation decides it"
    );
    let mut blank_log = ir;
    blank_log.transitions[0].actions = vec![ExecutableContentIr::Log {
        label: None,
        expr: Some(String::new()),
    }];
    assert!(matches!(
        blank_log.validate(),
        Err(LowerError::InvalidStructure(_))
    ));
}

#[test]
fn send_without_event_lowers_only_when_it_names_its_processor() {
    // SCXML 6.2 / C.2: a BasicHTTP send may omit the event (the receiver
    // uses `HTTP.POST`); a send with neither an event nor a processor type
    // addresses the SCXML processor, which requires one.
    let lowered = |send: &str| {
        lower(&parse(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0"><state id="s"><onentry>{send}</onentry></state></scxml>"#
        )))
    };
    for send in [
        r#"<send type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor"/>"#,
        r#"<send typeexpr="kind"/>"#,
    ] {
        assert!(lowered(send).is_ok(), "{send} must lower");
    }
    for send in [r"<send/>", r#"<send event="e" eventexpr="name"/>"#] {
        assert_eq!(
            lowered(send),
            Err(LowerError::InvalidStructure("send event/eventexpr")),
            "{send} must fail closed"
        );
    }
}

#[test]
fn artifact_kind_does_not_confuse_ir_with_wasm() {
    assert_ne!(ArtifactKind::LoweredIr, ArtifactKind::ComponentWasm);
}

#[test]
fn assign_lowens_inline_markup_body_and_rejects_combined_forms() {
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript">
  <datamodel><data id="Var1" expr="1"/></datamodel>
  <state id="s0">
    <onentry>
      <assign location="Var1"><child key="v">text</child></assign>
    </onentry>
  </state>
</scxml>"#
    );
    let ir = lower(&parse(&source)).expect("assign with inline markup must lower");
    let Some(scxml_compiler_core::ActionBlockIr { actions, .. }) =
        ir.states.first().and_then(|state| state.onentry.first())
    else {
        panic!("fixture must lower one onentry block");
    };
    let Some(scxml_compiler_core::ExecutableContentIr::Assign {
        location,
        expr,
        body,
        assign_type,
        attr,
    }) = actions.first().cloned()
    else {
        panic!("fixture must lower one assign action");
    };
    assert_eq!(location, "Var1");
    assert_eq!(expr, None);
    assert_eq!(body.len(), 1);
    assert_eq!(
        assign_type, None,
        "absent type is the default replacechildren"
    );
    assert_eq!(attr, None);

    let combined = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript">
  <state id="ready"><onentry><assign location="Var1" expr="2"><x/></assign></onentry></state>
</scxml>"#
    );
    assert_eq!(
        lower(&parse(&combined)),
        Err(LowerError::InvalidStructure(
            "assign cannot combine expr and inline body"
        ))
    );

    let empty = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript">
  <state id="ready"><onentry><assign location="Var1"/></onentry></state>
</scxml>"#
    );
    assert_eq!(
        lower(&parse(&empty)),
        Err(LowerError::InvalidStructure(
            "assign requires expr or inline body"
        ))
    );
}

/// `XPath` `<assign type attr>` (SCXML 1.0 CR 2014-03-13 §C.3.6) lowers to typed IR: the
/// default `replacechildren` stays absent, `delete` takes no value, and
/// `attr` accompanies exactly `addattribute`.
#[test]
fn assign_type_and_attr_lower_to_typed_ir() {
    let lowered = |body: &str| {
        lower(&parse(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="xpath"><state id="s"><onentry>{body}</onentry></state></scxml>"#
        )))
    };
    let first_assign = |body: &str| {
        let ir = lowered(body).expect("typed assign must lower");
        let action = ir
            .states
            .first()
            .and_then(|state| state.onentry.first())
            .and_then(|block| block.actions.first())
            .cloned();
        let Some(scxml_compiler_core::ExecutableContentIr::Assign {
            expr,
            body,
            assign_type,
            attr,
            ..
        }) = action
        else {
            panic!("fixture must lower one assign action");
        };
        (expr, body.len(), assign_type, attr)
    };
    assert_eq!(
        first_assign(r#"<assign location="$a" type="lastchild" expr="$b"/>"#),
        (
            Some("$b".to_owned()),
            0,
            Some(scxml_compiler_core::AssignTypeIr::Lastchild),
            None
        )
    );
    assert_eq!(
        first_assign(r#"<assign location="$a" type="replacechildren" expr="1"/>"#),
        (Some("1".to_owned()), 0, None, None),
        "the default type is represented by absence"
    );
    assert_eq!(
        first_assign(r#"<assign location="$a/b" type="delete"/>"#),
        (
            None,
            0,
            Some(scxml_compiler_core::AssignTypeIr::Delete),
            None
        )
    );
    assert_eq!(
        first_assign(r#"<assign location="$a" type="addattribute" attr="k" expr="'v'"/>"#),
        (
            Some("'v'".to_owned()),
            0,
            Some(scxml_compiler_core::AssignTypeIr::Addattribute),
            Some("k".to_owned())
        )
    );
    for (body, reason) in [
        (
            r#"<assign location="$a" type="sideways" expr="1"/>"#,
            "assign type is not an SCXML assign type",
        ),
        (
            r#"<assign location="$a" type="addattribute" expr="1"/>"#,
            "assign attr is required exactly for type addattribute",
        ),
        (
            r#"<assign location="$a" attr="k" expr="1"/>"#,
            "assign attr is required exactly for type addattribute",
        ),
    ] {
        assert_eq!(
            lowered(body),
            Err(LowerError::InvalidStructure(reason)),
            "{body}"
        );
    }
}

/// Machine IR v3 encoded `<script src>` as a plain string; v4 and later encode
/// it as a `{uri, pinned}` record. Pre-v4 bytes must fail as an unsupported
/// version, never as malformed IR and never decode under the current schema.
#[test]
fn machine_ir_v3_with_string_script_src_is_an_unsupported_version() {
    assert_eq!(MACHINE_IR_VERSION, 5, "script.src is a record since IR v4");
    let source = format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="s" datamodel="ecmascript"><state id="s"><onentry><script src="logic.js"/></onentry></state></scxml>"#
    );
    let ir = lower(&parse(&source)).expect("an external script reference lowers");
    let mut value = serde_json::to_value(&ir).expect("IR encodes");
    let src = script_src(&mut value).expect("the lowered IR holds the script reference");
    assert_eq!(src["uri"], "logic.js", "v4 src is a record: {src:?}");
    *src = serde_json::json!("logic.js");
    value["version"] = serde_json::json!(3);
    let bytes = serde_json::to_vec(&value).expect("v3-shaped IR encodes");
    assert_eq!(
        MachineIr::from_json(&bytes),
        Err(LowerError::UnsupportedIrVersion(3))
    );
}

/// The first `src` record carrying a `uri` anywhere in the IR JSON.
fn script_src(value: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("src").is_some_and(|src| src.get("uri").is_some()) {
                return map.get_mut("src");
            }
            map.values_mut().find_map(script_src)
        }
        serde_json::Value::Array(items) => items.iter_mut().find_map(script_src),
        _ => None,
    }
}

/// Inline markup serializes as a standalone, namespace-correct fragment:
/// non-ASCII text round-trips byte-for-byte, markup-significant characters
/// are escaped, default namespaces are restated or undeclared, and
/// namespaced attributes get deterministic declared prefixes.
#[test]
fn xml_markup_is_namespace_correct_and_escaped() {
    use scxml_compiler_core::{ExpandedNameIr, XmlAttributeIr, XmlNodeIr, xml_markup};
    let name = |namespace: Option<&str>, local: &str| ExpandedNameIr {
        namespace_uri: namespace.map(str::to_owned),
        local_name: local.to_owned(),
    };
    let child = XmlNodeIr::Element {
        name: name(None, "child"),
        attributes: vec![
            XmlAttributeIr {
                name: name(None, "label"),
                value: "héllo → 🎉 \"q\" & <\n".into(),
            },
            XmlAttributeIr {
                name: name(Some("urn:x"), "k"),
                value: "v".into(),
            },
        ],
        children: vec![XmlNodeIr::Text {
            value: "tëxt €42 & <b>".into(),
        }],
    };
    let root = XmlNodeIr::Element {
        name: name(Some(SCXML_NAMESPACE), "scxml"),
        attributes: Vec::new(),
        children: vec![
            child,
            XmlNodeIr::Element {
                name: name(Some(SCXML_NAMESPACE), "final"),
                attributes: Vec::new(),
                children: Vec::new(),
            },
        ],
    };
    assert_eq!(
        xml_markup(&[root]).expect("bounded markup serializes"),
        format!(
            "<scxml xmlns=\"{SCXML_NAMESPACE}\"><child xmlns=\"\" label=\"héllo → 🎉 &quot;q&quot; &amp; &lt;&#10;\" xmlns:ns0=\"urn:x\" ns0:k=\"v\">tëxt €42 &amp; &lt;b&gt;</child><final/></scxml>"
        )
    );
    assert_eq!(
        xml_markup(&[XmlNodeIr::Text {
            value: "plain".into()
        }])
        .expect("text serializes"),
        "plain"
    );
}

/// Lowering splits event descriptor and name lists on XML white space, so
/// each item of untrusted IR must hold none: a padded descriptor (`" e "`)
/// could never match an event, and a padded name is no location.
#[test]
fn untrusted_ir_list_items_hold_no_white_space() {
    let decode = |ir: &MachineIr| {
        MachineIr::from_json(&serde_json::to_vec(ir).expect("test IR must encode"))
    };
    let machine = lower(&parse(&format!(
        r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="s"><state id="s"><onentry><send event="e" namelist="x"/></onentry><transition event="go" target="s"/></state></scxml>"#
    )))
    .expect("fixture lowers");
    for padded in [" e ", "e\t", "\ne"] {
        let mut ir = machine.clone();
        ir.transitions[0].events = vec![padded.to_owned()];
        assert_eq!(
            decode(&ir),
            Err(LowerError::InvalidStructure(
                "event descriptors must be individually normalized"
            )),
            "{padded:?}"
        );
        let mut ir = machine.clone();
        if let ExecutableContentIr::Send { value } = &mut ir.states[0].onentry[0].actions[0] {
            value.namelist = vec![padded.to_owned()];
        }
        assert_eq!(
            decode(&ir),
            Err(LowerError::InvalidStructure(
                "name list items must be individually normalized"
            )),
            "{padded:?}"
        );
    }
}

/// Every name the engine turns into an event name, send ID, or invoke ID is
/// a valid name (non-empty, no white space, of any length): untrusted IR
/// that breaks one is refused when it is decoded, on every build, instead of
/// failing the session when the machine reaches it.
#[test]
fn untrusted_ir_names_are_checked_when_decoded() {
    let decode = |ir: &MachineIr| {
        MachineIr::from_json(&serde_json::to_vec(ir).expect("test IR must encode"))
    };
    let lowered = |body: &str| {
        lower(&parse(&format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="s">{body}</scxml>"#
        )))
        .expect("fixture lowers")
    };
    let long = "x".repeat(4_096);
    let spaced = "a b".to_owned();

    let atomic = lowered(r#"<state id="s"/>"#);
    let mut ir = atomic.clone();
    ir.states[0].id = Some(spaced.clone());
    assert_eq!(decode(&ir), Err(LowerError::InvalidName("state id")));
    // A state that can complete raises `done.state.<ID>` of any length.
    let compound = lowered(r#"<state id="s"><final id="f"/></state>"#);
    let mut ir = compound.clone();
    ir.states[0].id = Some(long.clone());
    ir.initial.targets.clone_from(&compound.initial.targets);
    assert!(
        decode(&ir).is_ok(),
        "a long ID and its done event are valid"
    );

    let actions = lowered(
        r#"<state id="s"><onentry><raise event="r"/><send event="e" id="i"/></onentry><invoke id="k" src="c.scxml"/></state>"#,
    );
    for name in [&spaced, &long] {
        let valid = name == &long;
        let mut ir = actions.clone();
        if let ExecutableContentIr::Raise { event, .. } = &mut ir.states[0].onentry[0].actions[0] {
            event.clone_from(name);
        }
        assert_eq!(decode(&ir).is_ok(), valid, "raise event {}", name.len());
        for field in ["send event", "send id"] {
            let mut ir = actions.clone();
            if let ExecutableContentIr::Send { value } = &mut ir.states[0].onentry[0].actions[1] {
                if field == "send event" {
                    value.event = Some(name.clone());
                } else {
                    value.id = Some(name.clone());
                }
            }
            if valid {
                assert!(decode(&ir).is_ok(), "{field}");
            } else {
                assert_eq!(decode(&ir), Err(LowerError::InvalidName(field)));
            }
        }
        let mut ir = actions.clone();
        ir.states[0].invokes[0].id = Some(name.clone());
        if valid {
            assert!(decode(&ir).is_ok(), "a long invoke id and its done event");
        } else {
            assert_eq!(decode(&ir), Err(LowerError::InvalidName("invoke id")));
        }
    }
}
