//! `COLLAPSED_ATTRIBUTES` is exactly the set of SCXML attributes whose type
//! in the bundled W3C SCXML 1.0 schema collapses white space.
//!
//! XML Schema 1.1 Part 2 §4.3.6 fixes `whiteSpace` to `collapse` for every
//! built-in type except `xsd:string` (`preserve`) and
//! `xsd:normalizedString` (`replace`), and a type restricted from another
//! inherits its facet. This test reads every `scxml.<element>.attlist`
//! attribute group of the bundled schema, resolves each attribute's type
//! through the schema's simple types to a built-in type, and compares the
//! collapsing set with the validator's table.

use std::collections::{BTreeMap, BTreeSet};

use scxml_syntax::{Element, SourceId, SourceText, parse_xml};
use scxml_validator_core::COLLAPSED_ATTRIBUTES;

const XSD: &str = "http://www.w3.org/2001/XMLSchema";
const SCHEMAS: [&str; 4] = [
    include_str!("../../scxml-validator-xsd/schemas/w3c-scxml-1.0/scxml-datatypes.xsd"),
    include_str!("../../scxml-validator-xsd/schemas/w3c-scxml-1.0/scxml-module-core.xsd"),
    include_str!("../../scxml-validator-xsd/schemas/w3c-scxml-1.0/scxml-module-data.xsd"),
    include_str!("../../scxml-validator-xsd/schemas/w3c-scxml-1.0/scxml-module-external.xsd"),
];

fn schema_elements(local_name: &str, roots: &[Element]) -> Vec<Element> {
    let mut found = Vec::new();
    for root in roots {
        root.visit(&mut |element| {
            if element.name.is(Some(XSD), local_name) {
                found.push(element.clone());
            }
        });
    }
    found
}

fn value<'a>(element: &'a Element, name: &str) -> &'a str {
    element
        .attribute(name)
        .map_or("", |attribute| attribute.value.as_str())
}

/// Whether the schema type `name` collapses white space.
fn collapses(name: &str, bases: &BTreeMap<String, String>) -> bool {
    match name.strip_prefix("xsd:") {
        Some("string" | "normalizedString") => false,
        Some(_) => true,
        None => collapses(
            bases
                .get(name)
                .unwrap_or_else(|| panic!("the schema defines `{name}`")),
            bases,
        ),
    }
}

#[test]
fn the_table_lists_exactly_the_schemas_collapsing_attributes() {
    let roots: Vec<Element> = SCHEMAS
        .iter()
        .enumerate()
        .map(|(index, text)| {
            parse_xml(SourceText::new(
                SourceId::new(u32::try_from(index).expect("four schemas")),
                "schema.xsd",
                *text,
            ))
            .expect("the bundled schema parses")
            .root
        })
        .collect();
    let bases: BTreeMap<String, String> = schema_elements("simpleType", &roots)
        .iter()
        .filter(|simple| simple.attribute("name").is_some())
        .map(|simple| {
            let restriction = schema_elements("restriction", std::slice::from_ref(simple));
            let base = restriction
                .first()
                .map(|restriction| value(restriction, "base").to_owned())
                .expect("each named simple type is a restriction");
            (value(simple, "name").to_owned(), base)
        })
        .collect();
    let mut from_schema = BTreeSet::new();
    let mut groups = 0;
    for group in schema_elements("attributeGroup", &roots) {
        let Some(element) = value(&group, "name")
            .strip_prefix("scxml.")
            .and_then(|rest| rest.strip_suffix(".attlist"))
        else {
            continue;
        };
        groups += 1;
        for attribute in schema_elements("attribute", std::slice::from_ref(&group)) {
            if collapses(value(&attribute, "type"), &bases) {
                from_schema.insert((element.to_owned(), value(&attribute, "name").to_owned()));
            }
        }
    }
    assert!(groups >= 25, "read {groups} SCXML attribute groups");
    let table: BTreeSet<(String, String)> = COLLAPSED_ATTRIBUTES
        .iter()
        .flat_map(|(element, attributes)| {
            attributes
                .iter()
                .map(|attribute| ((*element).to_owned(), (*attribute).to_owned()))
        })
        .collect();
    assert_eq!(table, from_schema);
}
