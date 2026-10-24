//! Model-facing XML (`<cognition:model_facing_data>`, decision D201) read
//! back as JSON, so a test can name a prompt's field: each attribute a
//! string field, an element with a `count` an array of its `item`s, an
//! `<entry key>` the field its key names, an element holding only text a
//! string, and any other element an object. Scalars read back as their text,
//! so [`rendered`] puts an expected value through the same rendering first.

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde_json::{Map, Value as Json};

/// One element as it was read.
#[derive(Default)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    text: String,
}

/// The prompt's root element read back as JSON.
///
/// # Panics
///
/// When `xml` is not one well-formed element.
#[must_use]
pub fn read(xml: &str) -> Json {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = true;
    let mut open: Vec<Element> = Vec::new();
    loop {
        match reader.read_event().expect("the prompt is well-formed XML") {
            Event::Start(start) => open.push(element(&start)),
            Event::Empty(start) => {
                let element = element(&start);
                match open.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return value(&element),
                }
            }
            Event::End(_) => {
                let element = open.pop().expect("an open element");
                match open.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return value(&element),
                }
            }
            Event::Text(text) => {
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&text.decode().expect("UTF-8"));
                }
            }
            Event::CData(text) => {
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&text.decode().expect("UTF-8"));
                }
            }
            Event::GeneralRef(reference) => {
                let character = match reference.resolve_char_ref().expect("a character") {
                    Some(character) => character,
                    None => match reference.decode().expect("UTF-8").as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => panic!("an entity the prompt never holds: {other}"),
                    },
                };
                if let Some(element) = open.last_mut() {
                    element.text.push(character);
                }
            }
            Event::Eof => panic!("the prompt holds no element"),
            _ => {}
        }
    }
}

/// `value` as a prompt's field `name` reads back (see [`read`]).
///
/// # Panics
///
/// When the value is over the projection's budgets.
#[must_use]
pub fn rendered(name: &str, value: &Json) -> Json {
    let mut object = Map::new();
    object.insert(name.to_owned(), value.clone());
    let xml = floppy_cognition::model_facing_data("rendered", &Json::Object(object))
        .expect("within the budgets");
    read(&xml)[name].clone()
}

fn element(start: &BytesStart<'_>) -> Element {
    Element {
        name: String::from_utf8(start.name().as_ref().to_vec()).expect("UTF-8"),
        attributes: start
            .attributes()
            .map(|attribute| {
                let attribute = attribute.expect("a well-formed attribute");
                (
                    String::from_utf8(attribute.key.as_ref().to_vec()).expect("UTF-8"),
                    attribute
                        .normalized_value(XmlVersion::Implicit1_0)
                        .expect("a well-formed value")
                        .into_owned(),
                )
            })
            .collect(),
        ..Element::default()
    }
}

fn value(element: &Element) -> Json {
    if let Some((_, count)) = element.attributes.iter().find(|(name, _)| name == "count") {
        let items: Vec<Json> = element.children.iter().map(value).collect();
        assert_eq!(count, &items.len().to_string(), "a count is its items'");
        return Json::Array(items);
    }
    if element.attributes.is_empty() && element.children.is_empty() {
        return if element.text.is_empty() {
            Json::Null
        } else {
            Json::String(element.text.clone())
        };
    }
    let mut object = Map::new();
    for (name, text) in &element.attributes {
        object.insert(name.clone(), Json::String(text.clone()));
    }
    for child in &element.children {
        if child.name == "entry" {
            let entry = value(child);
            let key = entry["key"].as_str().expect("an entry's key").to_owned();
            object.insert(key, entry["value"].clone());
        } else {
            object.insert(child.name.clone(), value(child));
        }
    }
    Json::Object(object)
}
