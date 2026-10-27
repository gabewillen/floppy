//! Phase 4 event contracts: one canonical JSON Schema file per event
//! (docs/port-plan.md, "Phase 4 event contracts"). Body events live in `bots/floppy/schemas/`, ability
//! events in `abilities/schemas/`, each in exactly one file: an
//! `event:schema` file reference resolves against its document's base URI
//! like any relative URI, so an ability references
//! `../schemas/<event>.json`, and the body and the environments
//! `../../abilities/schemas/<event>.json` or
//! `../../bots/floppy/schemas/<event>.json`; no document carries an inline
//! copy.
//!
//! Checked here, over the schema set:
//!
//! * every schema is JSON Schema 2020-12, compiles, and is titled by its
//!   file name; its examples satisfy it;
//! * the set is exactly the phase 4 events, split body / abilities;
//! * every name is on the event name list
//!   (`abilities/schemas/floppy-event-names.json`) or is marked
//!   `[floppy name]` in its description;
//! * the mapping rules hold: correlated events require an integer `turn`,
//!   the runtime-only correlation fields are gone, and media is a reference.
//!
//! And over every `.scx` under `abilities/`, `bots/floppy/` and
//! `environments/` (the body `bots/floppy/floppy.scx`, its abilities, and
//! the environments that invoke it; the checks themselves are proven on
//! `tests/fixtures/phase4/contracts/`):
//!
//! * `scx validate` reports nothing (which includes `EVENT011`: a literal
//!   payload sent to a literal child is judged against the child's schema);
//! * every `event:schema` is a reference to an existing file, never inline;
//! * every `#_parent` or `#_<invokeid>` send has a transition taking it in
//!   the receiving document;
//! * every `bot.*` or `environment.*` name used is on the event name list
//!   or a marked floppy name;
//! * the layout rule (decision D65): the document is `<name>/<name>.scx`,
//!   its only `<script>` is the root `<script src="<name>.js"/>` naming the
//!   `<name>.js` beside it, and the document itself has no `function`
//!   token;
//! * payload checks are `event:schema` (scxml.rs checks them where an
//!   event is created), and a transition taking one contract event carries
//!   its `event:schema`;
//! * every `cond`, and every `expr` outside `<data>`, is at most 160
//!   characters [floppy rule];
//! * every `<invoke type="scxml">` has a literal `id` and either a
//!   literal relative `src` or a `srcexpr` (the candidates and routines
//!   runners invoke a builder-produced behavior by its inventory row's
//!   source, the path of its program).
//!
//! Builder output, the documents a `srcexpr` invokes, is three documents
//! beside each other in a directory (decision D121), `<name>.scx`,
//! `<name>.json` and `<name>.js`, which scxml.rs loads against the
//! document's own base URI: every `event:schema` it carries references
//! `<name>.json`, and it carries at most its one root
//! `<script src="<name>.js"/>`; it meets the rest of the structure rule.
//! `tests/fixtures/phase4/behavior_double.scx`, the stand-in for one, is
//! checked here.
//!
//! Every reference of those documents resolves to its event's one schema
//! file in `abilities/schemas/` or `bots/floppy/schemas/`, and no schema
//! file name occurs twice in the workspace's `schemas/` directories.

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
const BODY_EVENTS: [&str; 12] = [
    "bot.deactivate",
    "bot.input",
    "bot.perceived",
    "bot.focus_device",
    "bot.activated",
    "bot.activating.failed",
    "bot.deactivated",
    "bot.processing.completed",
    "bot.processing.failed",
    "bot.processing.timed_out",
    "bot.processing.cancel.timed_out",
    "bot.degraded",
];

/// The abilities' events (`abilities/schemas/`).
const ABILITY_EVENTS: [&str; 125] = [
    "bot.ability.failed",
    "bot.reboot",
    "bot.ability.cognition.input",
    "bot.ability.cognition.output",
    "bot.ability.cognition.cancel",
    "bot.ability.cognition.cancelled",
    "bot.ability.cognition.ignore",
    "bot.ability.cognition.child.hop.timed_out",
    // A failed or timed-out step, handed back to be perceived.
    "bot.ability.cognition.step.failed",
    "bot.ability.cognition.step.timed_out",
    "bot.ability.processing.cancelled",
    "bot.ability.autonomy.input",
    "bot.ability.autonomy.output",
    "bot.ability.intuition.input",
    "bot.ability.intuition.output",
    "bot.ability.reasoning.input",
    "bot.ability.reasoning.output",
    "bot.ability.reasoning.recalled",
    "bot.ability.communication.input",
    "bot.ability.communication.activate",
    "bot.ability.communication.respond",
    "bot.ability.communication.draft",
    "bot.ability.communication.failed",
    "bot.ability.communication.response.hop.timed_out",
    "bot.ability.communication.stop",
    "bot.ability.conversation.input",
    "bot.ability.conversation.routed_input",
    "bot.ability.conversation.append",
    "bot.ability.conversation.output",
    "bot.ability.conversation.failed",
    "bot.ability.conversation.snapshot.request",
    "bot.ability.embedding.failed",
    "bot.ability.embedding.input",
    "bot.ability.embedding.output",
    "bot.ability.conversation.snapshot.output",
    "bot.ability.speaking.input",
    "bot.ability.speaking.output",
    "bot.ability.speaking.apply.failed",
    "bot.ability.speaking.draft",
    "bot.ability.speaking.efference",
    "bot.ability.speaking.stop",
    "bot.ability.speaking.progress",
    // The mouth's report that a commanded piece played, which speaking
    // times itself until the mouth (a device) is ported.
    "devices.audio.played",
    "bot.ability.vocal.speech.encoding.input",
    // Each chunk of a streamed encoding, before its output (decision D203).
    "bot.ability.vocal.speech.encoding.chunk",
    "bot.ability.vocal.speech.encoding.output",
    // Speaking's playout, which the body puts into the environment.
    "environment.sound",
    "bot.ability.identity.adopt",
    "bot.ability.identity.adopted",
    "bot.ability.identity.addressed",
    "bot.ability.identity.recalled",
    "bot.ability.identity.ask",
    "bot.ability.identity.named",
    // The persona: its revise, its output and its private completions.
    "bot.ability.persona.revise",
    "bot.ability.persona.output",
    "bot.ability.persona.recalled",
    "bot.ability.persona.revised",
    "bot.behavior.create",
    "bot.behavior.change",
    "bot.behavior.break",
    "bot.behavior.tick",
    "bot.ability.reflection.input",
    "bot.ability.reflection.output",
    // The revision's applied behavior (decision D148).
    "bot.ability.reflection.revision.output",
    // The learning (decision D149): the lesson a turn selects, the lesson
    // with its teaching turn, and how it went.
    "bot.ability.learning.input",
    "bot.ability.learning.lesson",
    "bot.ability.learning.output",
    "bot.ability.routines.reconcile",
    "bot.ability.ready",
    // Phase 3: a request to the bot's memory and its committed transaction
    // (decision D265), described where the memory and the abilities take them.
    "bot.ability.memory.input",
    "bot.ability.memory.output",
    // The migration ability's reached revision (Alembic's upgrade or
    // downgrade), described where the memory ability takes it.
    "bot.ability.migration.output",
    // Phase 6: listening (with its sensitivity and interpretation) and the
    // efference copy it takes from speaking.
    "bot.ability.listening.speech.output",
    "bot.ability.listening.failed",
    "bot.ability.listening.stage.failed",
    "bot.ability.listening.sensitivity.score",
    "bot.ability.listening.sensitivity.output",
    "bot.ability.listening.sensitivity.failed",
    // Phase 6: visual classification and reading.
    "bot.ability.vision.classification.input",
    "bot.ability.vision.classification.output",
    "bot.ability.vision.classification.apply.completed",
    "bot.ability.reading.input",
    "bot.ability.reading.output",
    "bot.ability.reading.failed",
    "bot.ability.reading.stage.failed",
    "bot.ability.reading.classification.completed",
    "bot.ability.reading.text.decoded",
    "bot.ability.reading.image.decoded",
    "bot.ability.reading.unreadable.output.ready",
    "bot.ability.reading.output.encoded",
    // The interpretation's long-lived stages: their requests and answers.
    "bot.ability.hearing.voice.detection.input",
    "bot.ability.hearing.voice.detection.output",
    "bot.ability.hearing.sound.classification.input",
    "bot.ability.hearing.sound.classification.output",
    "bot.ability.hearing.voice.diarization.input",
    "bot.ability.hearing.voice.diarization.output",
    "bot.ability.hearing.voice.identification.input",
    "bot.ability.hearing.voice.identification.output",
    "bot.ability.hearing.speech.decoding.input",
    "bot.ability.hearing.speech.decoding.output",
    "bot.ability.classifying.input",
    "bot.ability.classifying.output",
    // The turn detector (the conversation's turn_detector stage).
    "bot.ability.turn_detector.turn.start",
    "bot.ability.turn_detector.turn.update",
    "bot.ability.turn_detector.turn.pause",
    "bot.ability.turn_detector.turn.end",
    "bot.ability.turn_detector.turn.complete",
    "bot.ability.turn_detector.failed",
    "bot.ability.turn_detector.ready",
    // The short-term register (abilities/stm/stm.scx): its notifications,
    // its requests and their replies.
    "bot.ability.memory.stm.record",
    "bot.ability.memory.stm.reinforce",
    "bot.ability.memory.stm.recent",
    "bot.ability.memory.stm.recent.output",
    "bot.ability.memory.stm.turn",
    "bot.ability.memory.stm.recent_turns",
    "bot.ability.memory.stm.recent_turns.output",
    // Seeing, the stimulus it takes, and its stage
    // children's object detection and description (decisions D185 to
    // D189).
    "environment.visual",
    "bot.ability.seeing.sight",
    "bot.ability.seeing.look",
    "bot.ability.seeing.measure.completed",
    "bot.ability.seeing.measure.failed",
    "bot.ability.vision.detection.input",
    "bot.ability.vision.detection.output",
    "bot.ability.vision.description.input",
    "bot.ability.vision.description.output",
];

/// The correlated events of one processing turn: each requires `turn`.
/// Tool events (the ones a model chooses, such as
/// `bot.ability.cognition.ignore`) never do: their data is the model's.
const CORRELATED: [&str; 19] = [
    "bot.processing.completed",
    "bot.processing.failed",
    "bot.processing.timed_out",
    "bot.processing.cancel.timed_out",
    "bot.ability.cognition.output",
    "bot.ability.cognition.cancel",
    "bot.ability.cognition.cancelled",
    "bot.ability.cognition.child.hop.timed_out",
    "bot.ability.cognition.step.failed",
    "bot.ability.cognition.step.timed_out",
    "bot.ability.processing.cancelled",
    "bot.ability.autonomy.input",
    "bot.ability.autonomy.output",
    "bot.ability.intuition.input",
    "bot.ability.intuition.output",
    "bot.ability.reflection.input",
    "bot.ability.learning.lesson",
    "bot.ability.reasoning.output",
    "bot.ability.reasoning.recalled",
];

/// Runtime-only correlation and cancel-token fields, which SCXML invoke
/// cancellation and `turn` replace, so no schema carries them.
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

const NAMES_FILE: &str = "floppy-event-names.json";

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
            assert!(
                all.insert(event.clone(), read_json(&path)).is_none(),
                "{event} has one schema file"
            );
        }
    }
    all
}

/// The event names `abilities/schemas/floppy-event-names.json` lists.
fn listed_event_names() -> BTreeSet<String> {
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
        // An event with no data (a bare `<send>`, bot.ability.ready).
        Json::Null => return schema.admits(&scxml_types::DataValue::Null),
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

/// Whether `node` is a participant schema: an object schema whose
/// properties include `ref` and `role`.
fn is_participant(node: &Json) -> bool {
    node.get("properties")
        .and_then(Json::as_object)
        .is_some_and(|properties| properties.contains_key("ref") && properties.contains_key("role"))
}

#[test]
fn every_schema_inlines_the_same_one_participant() {
    // scxml.rs bundles no schema from another file (a `$ref` that is not a
    // fragment is refused, `ReferenceReason::External`), so each schema
    // that carries participants inlines the Participant schema, and every
    // one is the same: its ref, role, own and resembles.
    let mut participants: Vec<(String, Json)> = Vec::new();
    for (event, schema) in all_schemas() {
        walk(&schema, &mut |node| {
            if is_participant(node) {
                participants.push((event.clone(), node.clone()));
            }
        });
    }
    let (_, first) = participants
        .iter()
        .find(|(event, _)| event == "bot.input")
        .expect("bot.input carries participants");
    let properties: BTreeSet<&str> = first["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        properties,
        BTreeSet::from(["ref", "role", "own", "resembles"])
    );
    for (event, participant) in &participants {
        assert_eq!(participant, first, "{event} inlines the one Participant");
    }
    assert!(participants.len() > 1, "{participants:?}");
}

#[test]
fn every_episode_inlines_the_same_outcome_and_failure() {
    // A recalled episode's `outcome` and `failure` (decision D139) are
    // inlined in each schema that carries episodes, as the Participant is,
    // and every copy is the same.
    let mut episodes: Vec<(String, Json, Json)> = Vec::new();
    for (event, schema) in all_schemas() {
        walk(&schema, &mut |node| {
            let Some(properties) = node.get("properties").and_then(Json::as_object) else {
                return;
            };
            if properties.contains_key("output")
                && let (Some(outcome), Some(failure)) =
                    (properties.get("outcome"), properties.get("failure"))
            {
                episodes.push((event.clone(), outcome.clone(), failure.clone()));
            }
        });
    }
    let events: BTreeSet<&str> = episodes
        .iter()
        .map(|(event, _, _)| event.as_str())
        .collect();
    assert_eq!(
        events,
        BTreeSet::from(["bot.ability.failed", "bot.ability.reasoning.recalled"])
    );
    let (_, outcome, failure) = &episodes[0];
    for (event, other_outcome, other_failure) in &episodes {
        assert_eq!(
            other_outcome, outcome,
            "{event} inlines the one episode outcome"
        );
        assert_eq!(
            other_failure, failure,
            "{event} inlines the one episode failure"
        );
    }
}

#[test]
fn the_schema_set_is_exactly_the_phase4_events_split_body_and_abilities() {
    let body: BTreeSet<String> = schema_files(&body_schemas()).into_keys().collect();
    let abilities: BTreeSet<String> = schema_files(&ability_schemas()).into_keys().collect();
    assert_eq!(
        body,
        BODY_EVENTS.iter().map(|&name| name.to_owned()).collect()
    );
    assert_eq!(
        abilities,
        ABILITY_EVENTS.iter().map(|&name| name.to_owned()).collect()
    );
}

/// Every `schemas/` directory's JSON files under `dir`, recursively,
/// skipping build output (`target/`).
fn schema_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "target") {
                    pending.push(path);
                }
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && dir.file_name().is_some_and(|name| name == "schemas")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn every_schema_file_has_one_copy() {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for top in ["abilities", "bots", "environments", "devices", "services"] {
        for path in schema_files_under(&workspace().join(top)) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a UTF-8 file name")
                .to_owned();
            if let Some(first) = seen.insert(name.clone(), path.clone()) {
                panic!(
                    "{name} has two copies: {} and {}",
                    first.display(),
                    path.display()
                );
            }
        }
    }
}

#[test]
fn every_reference_resolves_to_its_events_one_schema_file() {
    let body = std::fs::canonicalize(body_schemas()).expect("the body's schemas exist");
    let abilities = std::fs::canonicalize(ability_schemas()).expect("the abilities' schemas exist");
    for document in phase4_documents() {
        let dir = document.parent().expect("a document has a directory");
        Parsed::read(&document).with(|root| {
            root.visit(&mut |element| {
                for attribute in element.attributes.iter().filter(|a| is_schema_attribute(a)) {
                    let scxml_event_schema::Source::Reference { resource, .. } =
                        scxml_event_schema::source(&attribute.value)
                    else {
                        continue;
                    };
                    let resolved = std::fs::canonicalize(dir.join(resource)).unwrap_or_else(|_| {
                        panic!("{}: {resource} does not resolve", document.display())
                    });
                    let file = resolved.file_name().expect("a file name");
                    let canonical = if abilities.join(file).is_file() {
                        abilities.join(file)
                    } else {
                        body.join(file)
                    };
                    assert_eq!(
                        resolved,
                        canonical,
                        "{}: {resource} is not its event's schema file",
                        document.display()
                    );
                }
            });
        });
    }
}

#[test]
fn every_event_name_is_a_listed_name_or_a_marked_floppy_name() {
    let listed = listed_event_names();
    let schemas = all_schemas();
    let floppy = floppy_names(&schemas);
    assert_eq!(
        floppy,
        BTreeSet::from([
            "bot.ability.learning.lesson".to_owned(),
            "bot.ability.migration.output".to_owned(),
            "bot.ability.ready".to_owned()
        ]),
        "the floppy-only events: an ability's readiness, the migration ability's output and the learning's lesson"
    );
    for event in schemas.keys() {
        assert!(
            listed.contains(event) != floppy.contains(event),
            "{event} is on the event name list, or floppy's and marked, never both"
        );
    }
}

#[test]
fn the_extracted_name_list_is_sorted_domain_names_only() {
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
        let schema = &all_schemas()[event];
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
    documents.extend(documents_under(&workspace().join("environments")));
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
/// descriptors, `<send>` and `<raise>` events) that are neither on the event
/// name list nor a marked floppy name.
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
                // A descriptor may name a prefix of the listed names (`bot.ability`).
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

/// The structure rule's problems in one document: a `function` token in
/// the document itself; with `own_script`, anything but exactly one root
/// `<script src="<name>.js"/>` naming the existing `<name>/<name>.js` beside
/// `<name>/<name>.scx`, else any `<script>`; a `cond` or (outside `<data>`) `expr` over
/// [`MAX_EXPRESSION_CHARS`], a transition taking one contract event without
/// an `event:schema`, or an SCXML `<invoke>` without a literal `id` and
/// either a literal relative `src` or a `srcexpr` (builder output, a stored
/// behavior's path; see [`builder_output_problems`]).
fn structure_problems(
    document: &Parsed,
    contract: &BTreeSet<String>,
    own_script: bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    if document.source.contains("function") {
        problems.push("a `function` token".to_owned());
    }
    if own_script {
        problems.extend(script_problems(document));
    }
    document.with(|root| {
        root.visit(&mut |element| {
            let name = element.name.local_name.as_str();
            if name == "script" && !own_script {
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
                // A stored behavior's source (decision D121): its path.
                let srcexpr =
                    element.attribute("src").is_none() && element.attribute("srcexpr").is_some();
                if element.attribute("id").is_none() || !(literal_src || srcexpr) {
                    problems.push(
                        "an SCXML <invoke> without a literal id and a relative src or a srcexpr"
                            .to_owned(),
                    );
                }
            }
        });
    });
    problems
}

/// The layout rule's problems in one document: it is `<name>/<name>.scx`,
/// and its only `<script>` is the root `<script src="<name>.js"/>`, with no
/// body, naming the `<name>/<name>.js` beside it.
fn script_problems(document: &Parsed) -> Vec<String> {
    let mut problems = Vec::new();
    let stem = document
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("a UTF-8 document name");
    let dir = document.path.parent().expect("a document has a directory");
    if dir.file_name().and_then(|name| name.to_str()) != Some(stem) {
        problems.push(format!("the document is not {stem}/{stem}.scx"));
    }
    let script = format!("{stem}.js");
    if !dir.join(&script).is_file() {
        problems.push(format!("no {script} beside the document"));
    }
    document.with(|root| {
        let mut roots = 0;
        for child in &root.children {
            if let scxml_syntax::Node::Element(element) = child
                && element.name.local_name == "script"
            {
                roots += 1;
                let src_ok = element
                    .attribute("src")
                    .is_some_and(|src| src.value == script);
                if !src_ok || !element.children.is_empty() {
                    problems.push(format!(
                        "a root <script> other than <script src=\"{script}\"/>"
                    ));
                }
            }
        }
        if roots != 1 {
            problems.push(format!(
                "{roots} root <script src=\"{script}\"/> elements, not one"
            ));
        }
        let mut all = 0;
        root.visit(&mut |element| {
            if element.name.local_name == "script" {
                all += 1;
            }
        });
        if all != roots {
            problems.push("a <script> outside the root".to_owned());
        }
    });
    problems
}

/// The builder output rule's problems in one document a `srcexpr`
/// invokes, `<name>.scx` (decision D121): an `event:schema` that is not a
/// reference into `<name>.json` (floppy documents carry no inline schema),
/// a `<script>` other than one root `<script src="<name>.js"/>`, and the
/// structure rule's.
fn builder_output_problems(document: &Parsed) -> Vec<String> {
    let stem = document
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("a UTF-8 document name");
    let schemas = format!("{stem}.json#");
    let script = format!("{stem}.js");
    let mut problems = Vec::new();
    document.with(|root| {
        let mut roots = 0;
        for child in &root.children {
            if let scxml_syntax::Node::Element(element) = child
                && element.name.local_name == "script"
                && element
                    .attribute("src")
                    .is_some_and(|src| src.value == script)
                && element.children.is_empty()
            {
                roots += 1;
            }
        }
        let mut all = 0;
        root.visit(&mut |element| {
            if element.name.local_name == "script" {
                all += 1;
            }
            for attribute in element.attributes.iter().filter(|a| is_schema_attribute(a)) {
                if !attribute.value.starts_with(&schemas) {
                    problems.push(format!(
                        "<{}> carries an event:schema that is not {schemas}...",
                        element.name.local_name
                    ));
                }
            }
        });
        if all != roots || roots > 1 {
            problems.push(format!(
                "a <script> other than one root <script src=\"{script}\"/>"
            ));
        }
    });
    problems.extend(
        structure_problems(document, &BTreeSet::new(), false)
            .into_iter()
            .filter(|problem| problem != "a <script> element"),
    );
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
    let mut known = listed_event_names();
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
    problems.extend(structure_problems(&document, &contract_events(), true));
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
    for name in ["good/parent/parent.scx", "good/child/child.scx"] {
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
fn a_document_needs_its_own_root_script_and_nothing_else() {
    assert_eq!(
        script_problems(&Parsed::read(&fixture("bad/parent.scx"))),
        [
            "the document is not parent/parent.scx",
            "no parent.js beside the document",
            "0 root <script src=\"parent.js\"/> elements, not one",
        ]
    );
    let dir = support::unique_scratch("contracts-script").join("doc");
    std::fs::create_dir_all(&dir).expect("creatable");
    support::write(&dir, "doc.js", "");
    let problems = |body: &str| {
        let text = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript">{body}<state id="s"/></scxml>"#
        );
        script_problems(&Parsed::read(&support::write(&dir, "doc.scx", &text)))
    };
    assert!(problems(r#"<script src="doc.js"/>"#).is_empty());
    assert_eq!(
        problems(r#"<script src="other.js"/>"#),
        [r#"a root <script> other than <script src="doc.js"/>"#]
    );
    assert_eq!(
        problems(r#"<script src="doc.js">var x = 1;</script>"#),
        [r#"a root <script> other than <script src="doc.js"/>"#]
    );
}

#[test]
fn validate_judges_a_literal_payload_against_the_childs_schema_file() {
    let dir = support::unique_scratch("contracts-event011");
    for name in [
        "child/child.scx",
        "child/child.js",
        "child/schemas/contract.ping.json",
        "parent/parent.js",
        "parent/schemas/contract.ready.json",
    ] {
        let target = dir.join(name);
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("creatable");
        std::fs::copy(fixture(&format!("good/{name}")), &target).expect("copyable");
    }
    let parent = std::fs::read_to_string(fixture("good/parent/parent.scx"))
        .expect("readable")
        .replace(r#"{"turn": 1}"#, r#"{"turn": 0}"#);
    let path = support::write(&dir.join("parent"), "parent.scx", &parent);
    let stderr = validates_clean(&path).expect_err("a ping with turn 0 breaks the child's schema");
    assert!(stderr.contains("EVENT011"), "{stderr}");
}

#[test]
fn builder_output_references_its_own_documents_and_meets_the_structure_rule() {
    let double = workspace().join("tests/fixtures/phase4/behavior_double.scx");
    let problems = builder_output_problems(&Parsed::read(&double));
    assert!(problems.is_empty(), "{problems:#?}");
    validates_clean(&double).expect("the behavior double validates clean");
    // Three documents beside each other: the chart, its schemas and its
    // script.
    let dir = support::unique_scratch("contracts-builder-output");
    std::fs::write(
        dir.join("greet.json"),
        r#"{"$defs": {"knock": {"type": "object"}}}"#,
    )
    .expect("writable");
    std::fs::write(
        dir.join("greet.js"),
        "function greeting() {\n  return 'hello';\n}\n",
    )
    .expect("writable");
    let greet = support::write(
        &dir,
        "greet.scx",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" initial="waiting"><datamodel><data id="said" expr="null"/></datamodel><script src="greet.js"/><state id="waiting"><transition event="knock" event:schema="greet.json#/$defs/knock" target="done"><assign location="said" expr="greeting()"/></transition></state><final id="done"/></scxml>"#,
    );
    let problems = builder_output_problems(&Parsed::read(&greet));
    assert!(problems.is_empty(), "{problems:#?}");
    validates_clean(&greet).expect("the three documents validate clean");
    // A schema file of another name, valid in a literal child, breaks
    // builder output.
    let referencing = Parsed::read(&fixture("good/child/child.scx"));
    assert!(
        builder_output_problems(&referencing)
            .iter()
            .any(|problem| problem.contains("carries an event:schema that is not child.json#")),
        "the good child references its schemas/ files"
    );
    // So does a script of another name.
    let other = support::write(
        &dir,
        "other.scx",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><script src="greet.js"/><final id="done"/></scxml>"#,
    );
    assert_eq!(
        builder_output_problems(&Parsed::read(&other)),
        [r#"a <script> other than one root <script src="other.js"/>"#]
    );
}

#[test]
fn an_scxml_invoke_needs_a_literal_id_and_a_relative_src_or_a_srcexpr() {
    let dir = support::unique_scratch("contracts-invoke");
    let problems = |invoke: &str| {
        let text = format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="s">{invoke}</state></scxml>"#
        );
        structure_problems(
            &Parsed::read(&support::write(&dir, "invoke.scx", &text)),
            &BTreeSet::new(),
            false,
        )
    };
    for allowed in [
        r#"<invoke id="child" type="scxml" src="child.scx"/>"#,
        r#"<invoke id="behavior" type="scxml" srcexpr="behavior"/>"#,
    ] {
        assert!(problems(allowed).is_empty(), "{allowed}");
    }
    for refused in [
        r#"<invoke type="scxml" srcexpr="behavior"/>"#,
        r#"<invoke id="candidate" type="scxml"><content expr="source"/></invoke>"#,
        r#"<invoke id="candidate" type="scxml"><content>text</content></invoke>"#,
        r#"<invoke id="child" type="scxml" src="/abs/child.scx"/>"#,
        r#"<invoke id="child" type="scxml" src="https://example.com/child.scx"/>"#,
    ] {
        assert_eq!(
            problems(refused),
            ["an SCXML <invoke> without a literal id and a relative src or a srcexpr"],
            "{refused}"
        );
    }
}
