//! Phase 4 event contracts: one canonical JSON Schema file per event,
//! extracted from mosfet's pydantic models (docs/port-plan.md, "Phase 4
//! event contracts"). Body events live in `bots/floppy/schemas/`, ability
//! events in `abilities/schemas/`, beside the documents that reference
//! them: an `event:schema` file reference is relative to its document and
//! cannot leave the document's directory, so documents reference these
//! files (`schemas/<event>.json`) and never carry inline copies.
//!
//! Checked here, over the schema set:
//!
//! * every schema is JSON Schema 2020-12, compiles, and is titled by its
//!   file name; its examples satisfy it;
//! * the set is exactly the phase 4 events, split body / abilities;
//! * every name is mosfet's (`abilities/schemas/mosfet-event-names.json`,
//!   the extracted list) or is marked `[floppy name]` in its description;
//! * the mapping rules hold: correlated events require an integer `turn`,
//!   mosfet's runtime-only fields are gone, and media is a reference.
//!
//! And over every `.scx` under `abilities/` and `bots/floppy/` (the body
//! `bots/floppy/floppy.scx` and its abilities; the checks themselves are
//! proven on `tests/fixtures/phase4/contracts/`):
//!
//! * `scx validate` reports nothing (which includes `EVENT011`: a literal
//!   payload sent to a literal child is judged against the child's schema);
//! * every `event:schema` is a reference to an existing file, never inline;
//! * every `#_parent` or `#_<invokeid>` send has a transition taking it in
//!   the receiving document;
//! * every `bot.*` or `environment.*` name used is mosfet's or a marked
//!   floppy name;
//! * no `<script>` element and no `function` token: payload checks are
//!   `event:schema` (scxml.rs checks them where an event is created), and
//!   a transition taking one contract event carries its `event:schema`;
//! * every `cond`, and every `expr` outside `<data>`, is at most 160
//!   characters [floppy rule];
//! * every `<invoke type="scxml">` has a literal `id` and either a
//!   literal relative `src`, a `srcexpr`, or a `<content expr>` (autonomy
//!   invokes a builder-produced behavior from its inventory row's source).
//!
//! Builder output, the documents a `srcexpr` or `<content expr>` invokes,
//! gets no schema files (scxml.rs reads them only for a literal `src`
//! child), so it carries no
//! `event:schema` reference; it meets the rest of the structure rule.
//! `tests/fixtures/phase4/behavior_double.scx`, the stand-in for one, is
//! checked here.
//!
//! The body's twins (`bots/floppy/schemas/` copies of the ability events
//! the body takes, which it cannot reference across directories) are
//! byte-identical to their `abilities/schemas/` sources.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value as Json;
use support::{output, workspace};

/// The marker a floppy-only event name carries at the start of its
/// schema's description.
const FLOPPY_NAME: &str = "[floppy name]";

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The body's events (`bots/floppy/schemas/`).
const BODY_EVENTS: [&str; 14] = [
    "bot.activate",
    "bot.deactivate",
    "bot.reboot",
    "bot.input",
    "bot.focus_device",
    "bot.clear_focus",
    "bot.activated",
    "bot.activating.failed",
    "bot.deactivated",
    "bot.processing.completed",
    "bot.processing.failed",
    "bot.processing.timed_out",
    "bot.processing.cancel.timed_out",
    "environment.visual",
];

/// The abilities' events (`abilities/schemas/`).
const ABILITY_EVENTS: [&str; 50] = [
    "bot.ability.failed",
    "bot.ability.cognition.input",
    "bot.ability.cognition.output",
    "bot.ability.cognition.cancel",
    "bot.ability.cognition.cancelled",
    "bot.ability.cognition.ignore",
    "bot.ability.cognition.child.hop.timed_out",
    "bot.ability.processing.cancelled",
    "bot.ability.autonomy.input",
    "bot.ability.autonomy.output",
    "bot.ability.intuition.input",
    "bot.ability.intuition.output",
    "bot.ability.reasoning.input",
    "bot.ability.reasoning.output",
    "bot.ability.reasoning.recalled",
    "bot.ability.reasoning.retained",
    "bot.ability.communication.input",
    "bot.ability.communication.activate",
    "bot.ability.communication.respond",
    "bot.ability.communication.failed",
    "bot.ability.communication.response.hop.timed_out",
    "bot.ability.conversation.input",
    "bot.ability.conversation.append",
    "bot.ability.conversation.output",
    "bot.ability.conversation.failed",
    "bot.ability.speaking.input",
    "bot.ability.speaking.output",
    "bot.ability.speaking.apply.failed",
    "bot.ability.identity.adopt",
    "bot.ability.identity.adopted",
    "bot.ability.identity.adopt.failed",
    "bot.ability.identity.addressed",
    "bot.ability.identity.recalled",
    "bot.behavior.create",
    "bot.behavior.change",
    "bot.behavior.break",
    "bot.behavior.tick",
    "bot.ability.reflection.input",
    "bot.ability.reflection.output",
    "bot.ability.routines.reconcile",
    "bot.ability.offers",
    // Phase 3: the memory service's committed transaction (mosfet's
    // Memory.output_event), described where the abilities take it.
    "bot.ability.memory.output",
    // Phase 6: listening (mosfet's Listening, Sensitivity and
    // Interpretation) and the efference copy it takes from speaking.
    "environment.sound",
    "bot.ability.listening.speech.output",
    "bot.ability.listening.failed",
    "bot.ability.listening.stage.failed",
    "bot.ability.listening.sensitivity.score",
    "bot.ability.listening.sensitivity.output",
    "bot.ability.listening.sensitivity.failed",
    "bot.ability.speaking.efference",
];

/// The correlated events of one processing turn: each requires `turn`.
/// Tool events (the ones a model chooses, such as
/// `bot.ability.cognition.ignore`) never do: their data is the model's.
const CORRELATED: [&str; 18] = [
    "bot.processing.completed",
    "bot.processing.failed",
    "bot.processing.timed_out",
    "bot.processing.cancel.timed_out",
    "bot.ability.cognition.input",
    "bot.ability.cognition.output",
    "bot.ability.cognition.cancel",
    "bot.ability.cognition.cancelled",
    "bot.ability.cognition.child.hop.timed_out",
    "bot.ability.processing.cancelled",
    "bot.ability.autonomy.input",
    "bot.ability.autonomy.output",
    "bot.ability.intuition.input",
    "bot.ability.intuition.output",
    "bot.ability.reflection.input",
    "bot.ability.reasoning.output",
    "bot.ability.reasoning.recalled",
    "bot.ability.reasoning.retained",
];

/// mosfet's runtime-only correlation and cancel-token fields, which SCXML
/// invoke cancellation and `turn` replace.
const RUNTIME_ONLY: [&str; 8] = [
    "operation_id",
    "generation",
    "parent_operation_id",
    "parent_generation",
    "token",
    "request_id",
    "actor_id",
    "capability",
];

const NAMES_FILE: &str = "mosfet-event-names.json";

/// The ability events the body takes, twinned into `bots/floppy/schemas/`.
const BODY_TWINS: [&str; 14] = [
    "bot.ability.offers",
    "bot.ability.communication.input",
    "bot.ability.communication.failed",
    "bot.ability.speaking.output",
    "bot.ability.cognition.output",
    "bot.ability.cognition.cancelled",
    "bot.ability.processing.cancelled",
    "bot.ability.failed",
    "bot.ability.conversation.output",
    "bot.ability.identity.addressed",
    "bot.behavior.tick",
    "environment.sound",
    "bot.ability.listening.speech.output",
    "bot.ability.listening.failed",
];

/// The longest `cond` or (outside `<data>`) `expr` a phase 4 document may
/// carry, in characters [floppy rule].
const MAX_EXPRESSION_CHARS: usize = 160;

fn body_schemas() -> PathBuf {
    workspace().join("bots/floppy/schemas")
}

fn ability_schemas() -> PathBuf {
    workspace().join("abilities/schemas")
}

fn fixture(path: &str) -> PathBuf {
    workspace()
        .join("tests/fixtures/phase4/contracts")
        .join(path)
}

fn read_json(path: &Path) -> Json {
    serde_json::from_slice(&std::fs::read(path).expect("the file is readable"))
        .expect("the file is JSON")
}

/// The schema files of one directory: event name to path.
fn schema_files(dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("the schema directory is listable") {
        let path = entry.expect("a directory entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a UTF-8 file name")
            .to_owned();
        if name == NAMES_FILE {
            continue;
        }
        let event = name
            .strip_suffix(".json")
            .expect("every schema file is <event>.json")
            .to_owned();
        files.insert(event, path);
    }
    files
}

fn all_schemas() -> BTreeMap<String, Json> {
    let mut all = BTreeMap::new();
    for dir in [body_schemas(), ability_schemas()] {
        for (event, path) in schema_files(&dir) {
            if dir == body_schemas() && BODY_TWINS.contains(&event.as_str()) {
                continue;
            }
            assert!(
                all.insert(event.clone(), read_json(&path)).is_none(),
                "{event} has one schema file"
            );
        }
    }
    all
}

fn mosfet_names() -> BTreeSet<String> {
    let names: Vec<String> = serde_json::from_value(read_json(&ability_schemas().join(NAMES_FILE)))
        .expect("the name list is an array of strings");
    names.into_iter().collect()
}

/// The event names floppy declares itself: schemas marked `[floppy name]`.
fn floppy_names(schemas: &BTreeMap<String, Json>) -> BTreeSet<String> {
    schemas
        .iter()
        .filter(|(_, schema)| {
            schema
                .get("description")
                .and_then(Json::as_str)
                .is_some_and(|description| description.starts_with(FLOPPY_NAME))
        })
        .map(|(event, _)| event.clone())
        .collect()
}

fn compiled(schema: &Json) -> scxml_event_schema::CompiledSchema {
    let text = serde_json::to_vec(schema).expect("a JSON value serializes");
    let resolved = scxml_event_schema::resolve(&text, None)
        .expect("the schema resolves: 2020-12 metaschema-valid, references bundled");
    assert_eq!(resolved.dialect, scxml_event_schema::Dialect::Draft202012);
    scxml_event_schema::CompiledSchema::new(&resolved.schema).expect("the schema compiles")
}

fn admits(schema: &scxml_event_schema::CompiledSchema, value: &Json) -> bool {
    let text = scxml_event_schema::canonical_json(value);
    let data = match value {
        Json::Object(_) => scxml_types::DataValue::canonical_json_object(&text),
        Json::Array(_) => scxml_types::DataValue::canonical_json_array(&text),
        _ => return false,
    }
    .expect("canonical JSON is data");
    schema.admits(&data)
}

/// Every object or array node of `value`, depth first.
fn walk<'a>(value: &'a Json, visit: &mut impl FnMut(&'a Json)) {
    visit(value);
    match value {
        Json::Object(map) => map.values().for_each(|child| walk(child, visit)),
        Json::Array(items) => items.iter().for_each(|child| walk(child, visit)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// The schema set.

#[test]
fn every_schema_is_2020_12_compiles_and_is_titled_by_its_file_name() {
    for (event, schema) in all_schemas() {
        assert_eq!(
            schema.get("$schema").and_then(Json::as_str),
            Some(DIALECT),
            "{event}"
        );
        assert_eq!(
            schema.get("title").and_then(Json::as_str),
            Some(event.as_str()),
            "{event}"
        );
        let compiled = compiled(&schema);
        for example in schema
            .get("examples")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            assert!(
                admits(&compiled, example),
                "{event}: its example {example} satisfies it"
            );
        }
    }
}

#[test]
fn the_schema_set_is_exactly_the_phase4_events_split_body_and_abilities() {
    let body: BTreeSet<String> = schema_files(&body_schemas()).into_keys().collect();
    let abilities: BTreeSet<String> = schema_files(&ability_schemas()).into_keys().collect();
    assert_eq!(
        body,
        BODY_EVENTS
            .iter()
            .chain(BODY_TWINS.iter())
            .map(|&name| name.to_owned())
            .collect()
    );
    assert_eq!(
        abilities,
        ABILITY_EVENTS.iter().map(|&name| name.to_owned()).collect()
    );
}

#[test]
fn the_bodys_twins_are_byte_identical_to_their_ability_schemas() {
    for event in BODY_TWINS {
        let file = format!("{event}.json");
        let twin = std::fs::read(body_schemas().join(&file)).expect("the twin is readable");
        let source = std::fs::read(ability_schemas().join(&file)).expect("the source is readable");
        assert!(
            twin == source,
            "bots/floppy/schemas/{file} differs from abilities/schemas/{file}"
        );
    }
}

#[test]
fn every_event_name_is_mosfets_or_a_marked_floppy_name() {
    let mosfet = mosfet_names();
    let schemas = all_schemas();
    let floppy = floppy_names(&schemas);
    assert_eq!(
        floppy,
        BTreeSet::from(["bot.ability.offers".to_owned()]),
        "the one floppy-only phase 4 event"
    );
    for event in schemas.keys() {
        assert!(
            mosfet.contains(event) != floppy.contains(event),
            "{event} is mosfet's, or floppy's and marked, never both"
        );
    }
}

#[test]
fn the_extracted_mosfet_name_list_is_sorted_domain_names_only() {
    let names: Vec<String> = serde_json::from_value(read_json(&ability_schemas().join(NAMES_FILE)))
        .expect("the name list is an array of strings");
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names, sorted, "sorted and unique");
    for name in &names {
        assert!(
            name.starts_with(|first: char| first.is_ascii_lowercase()) && !name.contains('/'),
            "{name} is a domain event name, not an hsm-internal one"
        );
    }
    for prefix in ["bot.", "environment.", "phone."] {
        assert!(
            names.iter().any(|name| name.starts_with(prefix)),
            "{prefix}"
        );
    }
}

#[test]
fn correlated_events_require_an_integer_turn_and_tool_events_do_not() {
    let schemas = all_schemas();
    for (event, schema) in &schemas {
        let required = schema
            .get("required")
            .and_then(Json::as_array)
            .is_some_and(|required| required.iter().any(|name| name == "turn"));
        assert_eq!(
            required,
            CORRELATED.contains(&event.as_str()),
            "{event}: `turn` is required exactly on correlated events"
        );
        if let Some(turn) = schema.pointer("/properties/turn") {
            assert_eq!(turn.get("type"), Some(&Json::from("integer")), "{event}");
            assert_eq!(turn.get("minimum"), Some(&Json::from(1)), "{event}");
        }
    }
}

#[test]
fn runtime_only_fields_are_gone_and_media_is_a_reference() {
    for (event, schema) in all_schemas() {
        walk(&schema, &mut |node| {
            if let Some(properties) = node.get("properties").and_then(Json::as_object) {
                for field in RUNTIME_ONLY {
                    assert!(
                        !properties.contains_key(field),
                        "{event}: no runtime-only `{field}`"
                    );
                }
            }
            assert_ne!(
                node.get("format").and_then(Json::as_str),
                Some("base64url"),
                "{event}: media by reference, not bytes"
            );
        });
    }
    for (event, field) in [
        ("environment.sound", "audio"),
        ("environment.visual", "image"),
    ] {
        let schema = read_json(&body_schemas().join(format!("{event}.json")));
        assert_eq!(
            schema
                .pointer(&format!("/properties/{field}/format"))
                .and_then(Json::as_str),
            Some("uri-reference"),
            "{event}.{field}"
        );
    }
}

// ---------------------------------------------------------------------------
// The documents.

/// Every `.scx` under `dir`, recursively (`read_dir`, never `find`).
fn documents_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "scx") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn phase4_documents() -> Vec<PathBuf> {
    let mut documents = documents_under(&workspace().join("abilities"));
    documents.extend(documents_under(&workspace().join("bots/floppy")));
    documents
}

struct Parsed {
    path: PathBuf,
    source: String,
}

impl Parsed {
    fn read(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            source: std::fs::read_to_string(path).expect("the document is readable"),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&scxml_syntax::Element) -> T) -> T {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            scxml_syntax::SourceId::new(1),
            self.path.display().to_string(),
            self.source.as_str(),
        ))
        .expect("the document is XML");
        f(&document.root)
    }
}

fn is_schema_attribute(attribute: &scxml_syntax::Attribute) -> bool {
    attribute.name.namespace_uri.as_deref() == Some(scxml_event_schema::EVENT_NAMESPACE_URI)
        && attribute.name.local_name == scxml_event_schema::SCHEMA_ATTRIBUTE
}

/// Every `event:schema` problem of one document: an inline schema, a
/// reference whose file is missing, or a file titled by another event.
fn schema_reference_problems(document: &Parsed) -> Vec<String> {
    let dir = document.path.parent().expect("a document has a directory");
    let mut problems = Vec::new();
    document.with(|root| {
        root.visit(&mut |element| {
            for attribute in element.attributes.iter().filter(|a| is_schema_attribute(a)) {
                match scxml_event_schema::source(&attribute.value) {
                    scxml_event_schema::Source::Inline(_) => problems.push(format!(
                        "<{}> carries an inline event:schema",
                        element.name.local_name
                    )),
                    scxml_event_schema::Source::Reference { resource, .. } => {
                        let file = dir.join(resource);
                        if !file.is_file() {
                            problems.push(format!("{resource} does not exist"));
                            continue;
                        }
                        let title = read_json(&file)
                            .get("title")
                            .and_then(Json::as_str)
                            .map(str::to_owned);
                        let event = element.attribute("event").map(|event| event.value.clone());
                        if let (Some(event), Some(title)) = (event, title)
                            && !event.split_whitespace().any(|token| token == title)
                        {
                            problems.push(format!("{resource} is titled {title}, not {event}"));
                        }
                    }
                }
            }
        });
    });
    problems
}

/// Whether one transition `event` attribute takes `name` (SCXML 3.12.1:
/// token prefix match, `*`, trailing `.*`).
fn descriptor_matches(descriptors: &str, name: &str) -> bool {
    descriptors.split_whitespace().any(|descriptor| {
        let descriptor = descriptor.strip_suffix(".*").unwrap_or(descriptor);
        descriptor == "*"
            || name == descriptor
            || name
                .strip_prefix(descriptor)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// The transitions' `event` attributes of one document.
fn taken(document: &Parsed) -> Vec<String> {
    document.with(|root| {
        let mut events = Vec::new();
        root.visit(&mut |element| {
            if element.name.local_name == "transition"
                && let Some(event) = element.attribute("event")
            {
                events.push(event.value.clone());
            }
        });
        events
    })
}

/// The literal `<send event target>` pairs of one document.
fn sends(document: &Parsed) -> Vec<(String, String)> {
    document.with(|root| {
        let mut sends = Vec::new();
        root.visit(&mut |element| {
            if element.name.local_name == "send"
                && let (Some(event), Some(target)) =
                    (element.attribute("event"), element.attribute("target"))
            {
                sends.push((event.value.clone(), target.value.clone()));
            }
        });
        sends
    })
}

/// The literal `<invoke id src>` children of one document: id to path.
fn children(document: &Parsed) -> BTreeMap<String, PathBuf> {
    let dir = document
        .path
        .parent()
        .expect("a document has a directory")
        .to_path_buf();
    document.with(|root| {
        let mut children = BTreeMap::new();
        root.visit(&mut |element| {
            if element.name.local_name == "invoke"
                && let (Some(id), Some(src)) = (element.attribute("id"), element.attribute("src"))
                && Path::new(&src.value)
                    .extension()
                    .is_some_and(|extension| extension == "scx")
            {
                children.insert(id.value.clone(), dir.join(&src.value));
            }
        });
        children
    })
}

/// Every send across one invoke boundary of `document` that the receiving
/// document has no transition for: to a literal child (`#_<id>`) and, from
/// each literal child, to its parent (`#_parent`).
fn unreceived_sends(document: &Parsed) -> Vec<String> {
    let mut problems = Vec::new();
    let own_sends = sends(document);
    let own_taken = taken(document);
    for (id, path) in children(document) {
        let child = Parsed::read(&path);
        let child_taken = taken(&child);
        for (event, target) in &own_sends {
            if *target == format!("#_{id}")
                && !child_taken.iter().any(|t| descriptor_matches(t, event))
            {
                problems.push(format!("{event} to #_{id}"));
            }
        }
        for (event, target) in sends(&child) {
            if target == "#_parent" && !own_taken.iter().any(|t| descriptor_matches(t, &event)) {
                problems.push(format!("{event} from {id} to #_parent"));
            }
        }
    }
    problems
}

/// The `bot.*` and `environment.*` names one document uses (transition
/// descriptors, `<send>` and `<raise>` events) that are neither mosfet's
/// nor a marked floppy name.
fn unknown_domain_names(document: &Parsed, known: &BTreeSet<String>) -> Vec<String> {
    document.with(|root| {
        let mut unknown = Vec::new();
        root.visit(&mut |element| {
            if !matches!(
                element.name.local_name.as_str(),
                "transition" | "send" | "raise"
            ) {
                return;
            }
            let Some(event) = element.attribute("event") else {
                return;
            };
            for token in event.value.split_whitespace() {
                let name = token.strip_suffix(".*").unwrap_or(token);
                let domain = name.starts_with("bot.") || name.starts_with("environment.");
                // A descriptor may name a prefix of mosfet names (`bot.ability`).
                let known_name = known
                    .iter()
                    .any(|known| known == name || known.starts_with(&format!("{name}.")));
                if domain && !known_name {
                    unknown.push(name.to_owned());
                }
            }
        });
        unknown
    })
}

/// The structure rule's problems in one document: a `<script>` element, a
/// `function` token, a `cond` or (outside `<data>`) `expr` over
/// [`MAX_EXPRESSION_CHARS`], a transition taking one contract event without
/// an `event:schema`, or an SCXML `<invoke>` without a literal `id` and
/// either a literal relative `src`, a `srcexpr` (builder output; see
/// [`builder_output_problems`]) or a `<content expr>` (a stored behavior).
fn structure_problems(document: &Parsed, contract: &BTreeSet<String>) -> Vec<String> {
    let mut problems = Vec::new();
    if document.source.contains("function") {
        problems.push("a `function` token".to_owned());
    }
    document.with(|root| {
        root.visit(&mut |element| {
            let name = element.name.local_name.as_str();
            if name == "script" {
                problems.push("a <script> element".to_owned());
            }
            for attribute in ["cond", "expr"] {
                if attribute == "expr" && name == "data" {
                    continue;
                }
                if let Some(value) = element.attribute(attribute)
                    && value.value.chars().count() > MAX_EXPRESSION_CHARS
                {
                    problems.push(format!(
                        "<{name} {attribute}> has {} characters",
                        value.value.chars().count()
                    ));
                }
            }
            if name == "transition"
                && let Some(event) = element.attribute("event")
                && contract.contains(event.value.trim())
                && !element.attributes.iter().any(is_schema_attribute)
            {
                problems.push(format!(
                    "the {} transition has no event:schema",
                    event.value
                ));
            }
            if name == "invoke"
                && element
                    .attribute("type")
                    .is_some_and(|kind| kind.value == "scxml")
            {
                let literal_src = element
                    .attribute("src")
                    .is_some_and(|src| !src.value.contains(':') && !src.value.starts_with('/'));
                let srcexpr =
                    element.attribute("src").is_none() && element.attribute("srcexpr").is_some();
                // A stored behavior's source (phase 5): `<content expr>`.
                let content = element.attribute("src").is_none()
                    && element.attribute("srcexpr").is_none()
                    && element.children.iter().any(|child| {
                        matches!(child, scxml_syntax::Node::Element(content)
                            if content.name.local_name == "content"
                                && content.attribute("expr").is_some())
                    });
                if element.attribute("id").is_none() || !(literal_src || srcexpr || content) {
                    problems.push(
                        "an SCXML <invoke> without a literal id and a relative src, a srcexpr or a <content expr>"
                            .to_owned(),
                    );
                }
            }
        });
    });
    problems
}

/// The builder output rule's problems in one document a `srcexpr`
/// invokes: any `event:schema` (a `srcexpr` child gets no schema files, and
/// floppy documents carry no inline schema), and the structure rule's.
fn builder_output_problems(document: &Parsed) -> Vec<String> {
    let mut problems = Vec::new();
    document.with(|root| {
        root.visit(&mut |element| {
            if element.attributes.iter().any(is_schema_attribute) {
                problems.push(format!(
                    "<{}> carries an event:schema",
                    element.name.local_name
                ));
            }
        });
    });
    problems.extend(structure_problems(document, &BTreeSet::new()));
    problems
}

fn contract_events() -> BTreeSet<String> {
    BODY_EVENTS
        .iter()
        .chain(ABILITY_EVENTS.iter())
        .map(|&name| name.to_owned())
        .collect()
}

fn known_names() -> BTreeSet<String> {
    let mut known = mosfet_names();
    known.extend(floppy_names(&all_schemas()));
    known
}

fn validates_clean(path: &Path) -> Result<(), String> {
    let validated = output(&["validate".to_owned(), path.display().to_string()]);
    if validated.status.success()
        && validated
            .stderr
            .contains("diagnostic summary: 0 error(s), 0 warning(s), 0 note(s)")
    {
        Ok(())
    } else {
        Err(validated.stderr)
    }
}

/// Every contract check over one document: its problems.
fn document_problems(path: &Path, known: &BTreeSet<String>) -> Vec<String> {
    let document = Parsed::read(path);
    let mut problems = schema_reference_problems(&document);
    problems.extend(unreceived_sends(&document));
    problems.extend(unknown_domain_names(&document, known));
    problems.extend(structure_problems(&document, &contract_events()));
    if let Err(stderr) = validates_clean(path) {
        problems.push(format!("scx validate: {stderr}"));
    }
    problems
}

#[test]
fn every_phase4_document_meets_the_contracts() {
    let known = known_names();
    for path in phase4_documents() {
        let problems = document_problems(&path, &known);
        assert!(problems.is_empty(), "{}: {problems:#?}", path.display());
    }
}

#[test]
fn the_contract_checks_pass_on_a_clean_parent_and_child() {
    let known = known_names();
    for name in ["good/parent.scx", "good/child.scx"] {
        let problems = document_problems(&fixture(name), &known);
        assert!(problems.is_empty(), "{name}: {problems:#?}");
    }
}

#[test]
fn the_contract_checks_catch_each_broken_contract() {
    let parent = Parsed::read(&fixture("bad/parent.scx"));
    assert_eq!(
        schema_reference_problems(&parent),
        ["<transition> carries an inline event:schema"]
    );
    assert_eq!(
        unreceived_sends(&parent),
        [
            "contract.unheard to #_child",
            "contract.unanswered from child to #_parent"
        ]
    );
    assert_eq!(
        unknown_domain_names(&parent, &known_names()),
        ["bot.ability.nonexistent"]
    );
}

#[test]
fn validate_judges_a_literal_payload_against_the_childs_schema_file() {
    let dir = support::unique_scratch("contracts-event011");
    for name in [
        "child.scx",
        "schemas/contract.ping.json",
        "schemas/contract.ready.json",
    ] {
        let target = dir.join(name);
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("creatable");
        std::fs::copy(fixture(&format!("good/{name}")), &target).expect("copyable");
    }
    let parent = std::fs::read_to_string(fixture("good/parent.scx"))
        .expect("readable")
        .replace(r#"{"turn": 1}"#, r#"{"turn": 0}"#);
    let path = support::write(&dir, "parent.scx", &parent);
    let stderr = validates_clean(&path).expect_err("a ping with turn 0 breaks the child's schema");
    assert!(stderr.contains("EVENT011"), "{stderr}");
}

#[test]
fn builder_output_carries_no_event_schema_and_meets_the_structure_rule() {
    let double = workspace().join("tests/fixtures/phase4/behavior_double.scx");
    let problems = builder_output_problems(&Parsed::read(&double));
    assert!(problems.is_empty(), "{problems:#?}");
    validates_clean(&double).expect("the behavior double validates clean");
    // A schema reference, valid in a literal child, breaks builder output.
    let referencing = Parsed::read(&fixture("good/child.scx"));
    assert!(
        builder_output_problems(&referencing)
            .iter()
            .any(|problem| problem.ends_with("carries an event:schema")),
        "the good child references its schema files"
    );
}

#[test]
fn an_scxml_invoke_needs_a_literal_id_and_a_relative_src_a_srcexpr_or_content() {
    let dir = support::unique_scratch("contracts-invoke");
    let problems = |invoke: &str| {
        let text = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="s">{invoke}</state></scxml>"#
        );
        structure_problems(
            &Parsed::read(&support::write(&dir, "invoke.scx", &text)),
            &BTreeSet::new(),
        )
    };
    for allowed in [
        r#"<invoke id="child" type="scxml" src="child.scx"/>"#,
        r#"<invoke id="behavior" type="scxml" srcexpr="behavior"/>"#,
        r#"<invoke id="candidate" type="scxml"><content expr="source"/></invoke>"#,
    ] {
        assert!(problems(allowed).is_empty(), "{allowed}");
    }
    for refused in [
        r#"<invoke type="scxml" srcexpr="behavior"/>"#,
        r#"<invoke id="candidate" type="scxml"><content>text</content></invoke>"#,
        r#"<invoke id="child" type="scxml" src="/abs/child.scx"/>"#,
        r#"<invoke id="child" type="scxml" src="https://example.com/child.scx"/>"#,
    ] {
        assert_eq!(
            problems(refused),
            [
                "an SCXML <invoke> without a literal id and a relative src, a srcexpr or a <content expr>"
            ],
            "{refused}"
        );
    }
}
