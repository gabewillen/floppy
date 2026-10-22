use super::*;

fn resolved(text: &str, fragment: Option<&str>) -> Resolved {
    resolve(text.as_bytes(), fragment).expect("the schema resolves")
}

fn error(text: &str, fragment: Option<&str>) -> SchemaError {
    resolve(text.as_bytes(), fragment).expect_err("the schema is rejected")
}

#[test]
fn values_are_inline_json_or_references() {
    assert_eq!(
        source(" {\"type\":\"string\"}"),
        Source::Inline(" {\"type\":\"string\"}")
    );
    assert_eq!(source("true"), Source::Inline("true"));
    assert_eq!(source(" false "), Source::Inline(" false "));
    assert_eq!(
        source("schemas/order.json#/$defs/Order"),
        Source::Reference {
            resource: "schemas/order.json",
            fragment: Some("/$defs/Order"),
        }
    );
    assert_eq!(
        source(" order.json "),
        Source::Reference {
            resource: "order.json",
            fragment: None,
        }
    );
    // `[` is not a schema: it is read as a (bad) reference.
    assert!(matches!(source("[1]"), Source::Reference { .. }));
}

#[test]
fn only_relative_paths_name_files() {
    for good in [
        "order.json",
        "schemas/order.json",
        "../order.json",
        "../../schemas/order.json",
        "a/../order.json",
        "./order.json",
    ] {
        assert_eq!(resource_problem(good), None, "{good}");
    }
    for bad in [
        "",
        "/etc/order.json",
        "https://example.com/order.json",
        "https://github.com/o/r/namespaces/x@v1/order.json",
        "file:order.json",
        "a//b.json",
        "schemas/",
        "a\\b.json",
        "a.json?x=1",
    ] {
        assert!(resource_problem(bad).is_some(), "{bad}");
    }
}

#[test]
fn inline_schemas_resolve_to_canonical_json() {
    let one = resolved(
        r#"{ "type" : "object", "required": ["id"], "properties": {"id": {"type": "string"}} }"#,
        None,
    );
    assert_eq!(
        one.schema,
        r#"{"properties":{"id":{"type":"string"}},"required":["id"],"type":"object"}"#
    );
    assert_eq!(one.sha256, sha256_hex(one.schema.as_bytes()));
    assert_eq!(one.dialect, Dialect::Draft202012);
    assert!(is_canonical_json(&one.schema));
    let two = resolved(
        r#"{"required":["id"],"properties":{"id":{"type":"string"}},"type":"object"}"#,
        None,
    );
    assert_eq!(one, two, "member order does not change the schema");
    assert_eq!(resolved("true", None).schema, "true");
    assert_eq!(resolved("\u{feff}{}", None).schema, "{}");
}

#[test]
fn invalid_json_is_located() {
    let SchemaError::InvalidJson {
        offset,
        line,
        column,
        message,
    } = error("{\n  \"type\": \"object\",\n}", None)
    else {
        panic!("invalid JSON");
    };
    assert_eq!((line, column), (3, 1));
    assert_eq!(offset, "{\n  \"type\": \"object\",\n".len());
    assert!(!message.contains("line"), "{message}");
    assert!(matches!(
        resolve(&[b'{', 0xff, b'}'], None),
        Err(SchemaError::InvalidUtf8 { offset: 1 })
    ));
}

#[test]
fn pointers_select_and_reject() {
    let file = r##"{"$defs":{"Order":{"type":"object","properties":{"items":{"type":"array","items":{"$ref":"#/$defs/Item"}},"self":{"$ref":"#/$defs/Order"}}},"Item":{"type":"string"},"a/b":{"type":"integer"}}}"##;
    let order = resolved(file, Some("/$defs/Order"));
    assert_eq!(
        order.schema,
        r##"{"$defs":{"/$defs/Item":{"type":"string"}},"properties":{"items":{"items":{"$ref":"#/$defs/~1$defs~1Item"},"type":"array"},"self":{"$ref":"#"}},"type":"object"}"##
    );
    assert_eq!(
        resolved(file, Some("/$defs/a~1b")).schema,
        r#"{"type":"integer"}"#
    );
    assert_eq!(
        resolved(file, Some("/%24defs/Item")).schema,
        r#"{"type":"string"}"#
    );
    assert_eq!(
        error(file, Some("/$defs/Missing/x")),
        SchemaError::PointerNotFound {
            pointer: "/$defs/Missing".to_owned()
        }
    );
    assert!(matches!(
        error(file, Some("Order")),
        SchemaError::InvalidPointer { .. }
    ));
    assert!(matches!(
        error(file, Some("/a~2")),
        SchemaError::InvalidPointer { .. }
    ));
    assert!(matches!(
        error(file, Some("/%zz")),
        SchemaError::InvalidPointer { .. }
    ));
}

#[test]
fn a_pointer_keeps_the_files_dialect() {
    let file = r##"{"$schema":"http://json-schema.org/draft-07/schema#","definitions":{"A":{"$ref":"#/definitions/B"},"B":{"type":"number"}}}"##;
    let a = resolved(file, Some("/definitions/A"));
    assert_eq!(a.dialect, Dialect::Draft7);
    assert_eq!(
        a.schema,
        r##"{"$ref":"#/definitions/~1definitions~1B","$schema":"http://json-schema.org/draft-07/schema#","definitions":{"/definitions/B":{"type":"number"}}}"##
    );
}

#[test]
fn dialects_are_2020_12_2019_09_and_draft_07() {
    for (uri, dialect) in [
        (
            "https://json-schema.org/draft/2020-12/schema",
            Dialect::Draft202012,
        ),
        (
            "https://json-schema.org/draft/2020-12/schema#",
            Dialect::Draft202012,
        ),
        (
            "https://json-schema.org/draft/2019-09/schema",
            Dialect::Draft201909,
        ),
        ("http://json-schema.org/draft-07/schema#", Dialect::Draft7),
        ("http://json-schema.org/draft-07/schema", Dialect::Draft7),
    ] {
        assert_eq!(
            resolved(&format!(r#"{{"$schema":"{uri}"}}"#), None).dialect,
            dialect
        );
    }
    for unsupported in [
        r#"{"$schema":"http://json-schema.org/draft-04/schema#"}"#,
        r#"{"$schema":"https://example.com/my-dialect"}"#,
        r#"{"$schema":7}"#,
    ] {
        assert!(
            matches!(
                error(unsupported, None),
                SchemaError::UnsupportedDialect { .. }
            ),
            "{unsupported}"
        );
    }
}

#[test]
fn metaschema_violations_are_invalid_schemas() {
    let SchemaError::InvalidSchema { location, .. } =
        error(r#"{"properties":{"id":{"type":"strin"}}}"#, None)
    else {
        panic!("invalid schema");
    };
    assert_eq!(location, "/properties/id/type");
    assert!(matches!(
        error(r#"{"type":"object","minLength":-1}"#, None),
        SchemaError::InvalidSchema { .. }
    ));
    // Draft-07 has no `prefixItems` keyword constraints, but `items` must be
    // a schema or an array of schemas.
    assert!(matches!(
        error(
            r#"{"$schema":"http://json-schema.org/draft-07/schema#","items":3}"#,
            None
        ),
        SchemaError::InvalidSchema { .. }
    ));
    assert!(matches!(
        error(r#"{"type":"string","pattern":"("}"#, None),
        SchemaError::InvalidSchema { .. }
    ));
}

#[test]
fn references_are_bundled_or_rejected() {
    let SchemaError::Reference {
        location,
        reference,
        reason,
    } = error(r#"{"properties":{"a":{"$ref":"item.json#/x"}}}"#, None)
    else {
        panic!("external reference");
    };
    assert_eq!(location, "/properties/a/$ref");
    assert_eq!(reference, "item.json#/x");
    assert_eq!(reason, ReferenceReason::External);
    assert!(matches!(
        error(r##"{"$ref":"#/$defs/missing"}"##, None),
        SchemaError::Reference {
            reason: ReferenceReason::Unresolved,
            ..
        }
    ));
    // `const` and `enum` hold data, not schemas: a `$ref` there is a value.
    resolved(
        r#"{"const":{"$ref":"elsewhere.json"},"enum":[{"$ref":"x"}]}"#,
        None,
    );
    // A property named `$ref` is a schema, not a reference.
    resolved(r#"{"properties":{"$ref":{"type":"string"}}}"#, None);
    // Identifiers cannot move with a pointer.
    let file = r##"{"$defs":{"A":{"$id":"https://example.com/a","type":"string"},"B":{"$ref":"#c"},"C":{"$anchor":"c"}}}"##;
    assert!(matches!(
        error(file, Some("/$defs/A")),
        SchemaError::Reference {
            reason: ReferenceReason::NotRelocatable("$id"),
            ..
        }
    ));
    assert!(matches!(
        error(file, Some("/$defs/B")),
        SchemaError::Reference {
            reason: ReferenceReason::NotRelocatable("plain-name fragment"),
            ..
        }
    ));
    // Without a pointer the file is embedded whole, anchors included.
    resolved(file, None);
}

/// The bundled-reference count is a resource bound: 256 on wasm builds
/// (the shipped components), none natively (AGENTS.md). The wasm value is
/// pinned here; the components apply it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_bundled_reference_count_is_a_wasm_bound() {
    assert_eq!(MAX_BUNDLED_TARGETS, wasm_bound(256));
    let mut defs = String::new();
    let mut refs = String::new();
    for index in 0..=256 {
        write!(defs, r#","D{index}":{{"type":"string"}}"#).expect("formats");
        write!(refs, r##"{{"$ref":"#/$defs/D{index}"}},"##).expect("formats");
    }
    refs.pop();
    let file = format!(r#"{{"$defs":{{"Main":{{"anyOf":[{refs}]}}{defs}}}}}"#);
    // Native builds impose no count of their own; the amplification guard
    // still applies.
    resolve(file.as_bytes(), Some("/$defs/Main")).expect("257 bundled values resolve natively");
}

/// References to a value and to each of its nested subschemas copy the
/// same bytes again and again: the result grows quadratically in a file
/// that grows linearly. The amplification guard stops it on every build.
#[test]
fn bundling_amplification_is_guarded_on_every_build() {
    let levels = 64;
    let padding = "x".repeat(256);
    let mut chain = String::from("{}");
    for _ in 0..levels {
        chain = format!(r#"{{"description":"{padding}","not":{chain}}}"#);
    }
    let refs = (0..levels)
        .map(|depth| format!(r##"{{"$ref":"#/$defs/X{}"}}"##, "/not".repeat(depth)))
        .collect::<Vec<_>>()
        .join(",");
    let file = format!(r#"{{"$defs":{{"Main":{{"anyOf":[{refs}]}},"X":{chain}}}}}"#);
    assert!(matches!(
        error(&file, Some("/$defs/Main")),
        SchemaError::Reference {
            reason: ReferenceReason::TooLarge,
            ..
        }
    ));
}

/// A fragment whose percent-encoding does not decode names no value; it is
/// never read as the root.
#[test]
fn an_undecodable_reference_fragment_is_unresolved() {
    assert!(matches!(
        error(
            r##"{"$defs":{"A":{"$ref":"#%ZZ"},"B":{"type":"string"}}}"##,
            Some("/$defs/A")
        ),
        SchemaError::Reference {
            reason: ReferenceReason::Unresolved,
            ..
        }
    ));
}

/// Bundling can nest a value one level deeper than its file did; a result
/// the Machine IR decoder (`serde_json`'s 128-level limit) cannot read back
/// is rejected, never embedded.
#[test]
fn a_bundled_result_too_deep_to_decode_is_rejected() {
    let mut rejected = false;
    for depth in 100..=130 {
        let target = format!("{}{{}}{}", r#"{"not":"#.repeat(depth), "}".repeat(depth));
        let file = format!(r##"{{"T":{target},"S":{{"$ref":"#/T"}}}}"##);
        if serde_json::from_str::<Value>(&file).is_err() {
            continue;
        }
        match resolve(file.as_bytes(), Some("/S")) {
            Ok(resolved) => assert!(
                serde_json::from_str::<Value>(&resolved.schema).is_ok(),
                "depth {depth}: an embedded schema decodes"
            ),
            Err(SchemaError::Reference {
                reason: ReferenceReason::TooLarge,
                ..
            }) => rejected = true,
            Err(_) => {}
        }
    }
    assert!(rejected, "some file deep enough is rejected after bundling");
}

#[test]
fn canonical_json_sorts_keys_by_bytes() {
    let value: Value =
        serde_json::from_str(r#"{"b":1,"a":[true,null,1.5,"x\n"],"é":{},"Z":0}"#).expect("json");
    assert_eq!(
        canonical_json(&value),
        r#"{"Z":0,"a":[true,null,1.5,"x\n"],"b":1,"é":{}}"#
    );
    assert!(!is_canonical_json(r#"{"b":1,"a":2}"#));
    assert!(!is_canonical_json("{ }"));
    assert!(is_canonical_json("{}"));
}

#[test]
fn locate_finds_the_value_a_pointer_names() {
    let text = r#"{ "a" : [1, {"b c": "x\"y"}], "type": "strng", "z/w": {"~": true} }"#;
    let at = |pointer: &str| locate(text, pointer).map(|(start, end)| &text[start..end]);
    assert_eq!(at(""), Some(text));
    assert_eq!(at("/a"), Some(r#"[1, {"b c": "x\"y"}]"#));
    assert_eq!(at("/a/0"), Some("1"));
    assert_eq!(at("/a/1/b c"), Some(r#""x\"y""#));
    assert_eq!(at("/type"), Some(r#""strng""#));
    assert_eq!(at("/z~1w/~0"), Some("true"));
    assert_eq!(at("/a/2"), None);
    assert_eq!(at("/missing"), None);
    assert_eq!(locate("{", ""), None);
}

#[test]
fn compiled_schemas_check_event_data_at_a_boundary() {
    use scxml_types::{DataValue, FiniteNumber};
    let object = CompiledSchema::new(
        r#"{"properties":{"n":{"type":"integer"}},"required":["n"],"type":"object"}"#,
    )
    .expect("compiles");
    let data = |text: &str| DataValue::canonical_json_object(text).expect("object");
    assert!(object.admits(&data(r#"{"n":1}"#)));
    assert!(!object.admits(&data(r#"{"n":"1"}"#)));
    assert!(!object.admits(&data("{}")));
    assert!(!object.admits(&DataValue::Absent));
    assert!(!object.admits(&DataValue::Xml("<n>1</n>".into())));
    let integer = CompiledSchema::new(r#"{"type":"integer"}"#).expect("compiles");
    assert!(integer.admits(&DataValue::Number(FiniteNumber::new(3.0).expect("finite"))));
    assert!(!integer.admits(&DataValue::Number(FiniteNumber::new(3.5).expect("finite"))));
    let null = CompiledSchema::new(r#"{"type":"null"}"#).expect("compiles");
    assert!(null.admits(&DataValue::Absent));
    assert!(null.admits(&DataValue::Undefined));
    assert!(null.admits(&DataValue::Null));
    let anything = CompiledSchema::new("true").expect("compiles");
    assert!(anything.admits(&DataValue::Xml("<x/>".into())));
    let nothing = CompiledSchema::new("false").expect("compiles");
    assert!(!nothing.admits(&DataValue::Null));
    // Only canonical resolved text compiles; nothing is ever loaded.
    assert!(CompiledSchema::new(r#"{ "type": "null" }"#).is_err());
    assert!(CompiledSchema::new(r#"{"$ref":"https://example.com/s.json"}"#).is_err());
    assert!(CompiledSchema::new(r#"{"$schema":"https://example.com/s"}"#).is_err());
}
