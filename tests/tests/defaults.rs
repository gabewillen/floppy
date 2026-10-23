//! The defaults floppy ships are stated in one place: docs/defaults.md,
//! and `.env.example` for the variables.
//!
//! Checked here, over every `.scx` under `bots/` and `abilities/`:
//!
//! * every `model` `<data>` is [`support::DEFAULT_MODEL`], and the model
//!   name appears nowhere else in a document, so a request's or an
//!   activation's fallback is the `<data>` itself;
//! * the variables the documents' `env` attributes name are exactly
//!   `.env.example`'s names (bare, without values) and
//!   [`support::GRANTED`];
//! * every row of docs/defaults.md's "Timeouts and bounds" table names a document's
//!   top-level `<data>` and its value, and every `*_timeout_ms` `<data>` has
//!   a row.
//!
//! Nothing here runs a bot, so it needs no components.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use support::workspace;

/// Every `.scx` document under `bots/` and `abilities/`, relative to the
/// workspace root (builder examples are builder output, not shipped
/// defaults, so `bots/builder/examples/` is skipped).
fn documents() -> Vec<PathBuf> {
    fn visit(dir: &Path, found: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("the directory is readable")
            .map(|entry| entry.expect("the entry is readable").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "examples") {
                    visit(&path, found);
                }
            } else if path.extension().is_some_and(|extension| extension == "scx") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    for dir in ["bots", "abilities"] {
        visit(&workspace().join(dir), &mut found);
    }
    found
        .into_iter()
        .map(|path| {
            path.strip_prefix(workspace())
                .expect("under the workspace")
                .to_path_buf()
        })
        .collect()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(workspace().join(path)).expect("the file is readable")
}

/// The value of `name="..."` in `text`, from its first occurrence.
fn attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("{name}=\""))? + name.len() + 2;
    let end = text[start..].find('"')?;
    Some(&text[start..start + end])
}

/// A document's one-line `<data id expr>` elements, as `id` to `expr`.
fn data(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("<data "))
        .filter_map(|rest| Some((attribute(rest, "id")?, attribute(rest, "expr")?)))
        .map(|(id, expr)| (id.to_owned(), expr.to_owned()))
        .collect()
}

/// The variable names a document's `env:<slot>="NAME"` attributes give.
fn env_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(at) = rest.find("env:") {
        let preceded = rest[..at].chars().next_back();
        rest = &rest[at + 4..];
        if !preceded.is_some_and(char::is_whitespace) {
            continue;
        }
        let slot_end = rest
            .find(|c: char| !c.is_ascii_lowercase())
            .unwrap_or(rest.len());
        if slot_end > 0
            && let Some(value) = rest[slot_end..].strip_prefix("=\"")
            && let Some(end) = value.find('"')
        {
            names.insert(value[..end].to_owned());
        }
    }
    names
}

/// The documents whose `model` is the speech service's (an audio or voice
/// model, the original's speech decoder, speech encoder and voice classifiers'
/// own), not the generation model: each `model` `<data>` is `null` (the
/// service's request names none of floppy's) or a model docs/defaults.md's
/// "Speech and voice models" names.
const SPEECH_DOCUMENTS: [&str; 6] = [
    "abilities/speaking/speaking.scx",
    "abilities/vocal/vocal.scx",
    "abilities/speech_decoding/speech_decoding.scx",
    "abilities/voice_detection/voice_detection.scx",
    "abilities/voice_diarization/voice_diarization.scx",
    "abilities/voice_identification/voice_identification.scx",
];

#[test]
fn every_model_data_is_the_default_model_and_the_name_appears_nowhere_else() {
    let quoted = format!("'{}'", support::DEFAULT_MODEL);
    let page = read(Path::new("docs/defaults.md"));
    let mut models = 0;
    for path in documents() {
        let text = read(&path);
        let data = data(&text);
        if SPEECH_DOCUMENTS
            .iter()
            .any(|document| path == Path::new(document))
        {
            let model = data
                .get("model")
                .unwrap_or_else(|| panic!("{}: a model <data>", path.display()));
            assert!(
                model == "null" || page.contains(&format!("`{}`", model.trim_matches('\''))),
                "{}: the speech model {model} is docs/defaults.md's",
                path.display()
            );
            continue;
        }
        if let Some(model) = data.get("model") {
            assert_eq!(model, &quoted, "{}: the model <data>", path.display());
            models += 1;
        }
        assert_eq!(
            text.matches(support::DEFAULT_MODEL).count(),
            usize::from(data.contains_key("model")),
            "{}: the model is named only by its <data>",
            path.display()
        );
    }
    assert_eq!(
        models, 9,
        "builder, floppy, cognition, reasoning, reflection, reading, seeing, object detection and description"
    );
    assert!(
        read(Path::new("docs/defaults.md")).contains(&format!("`{}`", support::DEFAULT_MODEL)),
        "docs/defaults.md names the model"
    );
}

#[test]
fn the_env_example_names_exactly_the_variables_the_documents_read() {
    let read_by_documents: BTreeSet<String> = documents()
        .iter()
        .flat_map(|path| env_names(&read(path)))
        .collect();
    let example: Vec<String> = read(Path::new(".env.example"))
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    for line in &example {
        assert!(
            !line.contains('='),
            ".env.example holds names only, never values: {line}"
        );
    }
    let example: BTreeSet<String> = example.into_iter().collect();
    let granted: BTreeSet<String> = support::GRANTED
        .iter()
        .map(|&name| name.to_owned())
        .collect();
    assert_eq!(example, read_by_documents);
    assert_eq!(example, granted);
}

#[test]
fn the_defaults_page_states_every_timeout_and_bound_the_documents_ship() {
    let page = read(Path::new("docs/defaults.md"));
    let section = page
        .split_once("## Timeouts and bounds")
        .expect("docs/defaults.md has the Timeouts and bounds table")
        .1;
    let rows: Vec<(String, String, String)> = section
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2)
        .map(|line| {
            let cells: Vec<String> = line
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_owned())
                .collect();
            (cells[1].clone(), cells[2].clone(), cells[3].clone())
        })
        .collect();
    assert!(!rows.is_empty(), "the table has rows");
    for (document, id, value) in &rows {
        let data = data(&read(Path::new(document)));
        assert_eq!(
            data.get(id),
            Some(value),
            "docs/defaults.md's row for {document} {id}"
        );
    }
    for path in documents() {
        for id in data(&read(&path)).into_keys() {
            if id.ends_with("_timeout_ms") {
                let document = path.to_str().expect("UTF-8 path");
                assert!(
                    rows.iter()
                        .any(|(row_document, row_id, _)| row_document == document && row_id == &id),
                    "docs/defaults.md's table has no row for {document} {id}"
                );
            }
        }
    }
}

/// One `<data>` of a document, as milliseconds.
fn millis(path: &str, id: &str) -> u64 {
    data(&read(Path::new(path)))
        .get(id)
        .unwrap_or_else(|| panic!("{path}: a {id} <data>"))
        .parse()
        .unwrap_or_else(|_| panic!("{path}: {id} is a whole number of milliseconds"))
}

/// The time bounds the bot's `bounds` param may set with a `<data>` default
/// (decision D101): the cognition's stage bounds and reflection's revising
/// bounds, each a `<data>` of its document that the document's script reads
/// from `bounds` first.
const ACTIVATION_BOUNDS: [(&str, &str); 4] = [
    (
        "abilities/cognition/cognition.scx",
        "child_operation_timeout_ms",
    ),
    (
        "abilities/cognition/cognition.scx",
        "child_operation_ceiling_ms",
    ),
    ("abilities/reflection/reflection.scx", "revising_timeout_ms"),
    ("abilities/reflection/reflection.scx", "revising_ceiling_ms"),
];

/// The members of the bot's `bounds` param with no `<data>` default: a
/// cognition tier's own step bound (decision D140, the reference's
/// `_tier_timeouts`, read as `bounds[tier() + '_timeout_ms']`) and
/// reasoning's context fit (decision D143); absent, a tier takes the stage
/// bounds above and reasoning fits nothing. Each is one the body documents
/// and the cognition's script reads.
const TIER_BOUNDS: [&str; 3] = [
    "reflex_timeout_ms",
    "intuition_timeout_ms",
    "reasoning_timeout_ms",
];
const CONTEXT_BOUNDS: [&str; 2] = ["max_context_tokens", "completion_reserve_tokens"];

#[test]
fn the_bounds_param_sets_exactly_the_bounds_the_documents_declare() {
    for (document, id) in ACTIVATION_BOUNDS {
        assert!(
            data(&read(Path::new(document))).contains_key(id),
            "{document} declares <data id=\"{id}\">"
        );
        let script = document.replace(".scx", ".js");
        assert!(
            read(Path::new(&script)).contains(&format!("bounds.{id}")),
            "{script} reads {id} from bounds"
        );
    }
    let body = read(Path::new("bots/floppy/floppy.scx"));
    let cognition = read(Path::new("abilities/cognition/cognition.js"));
    assert!(
        cognition.contains("bounds[tier() + '_timeout_ms']"),
        "the cognition reads its tier's own bound from bounds"
    );
    for id in TIER_BOUNDS.into_iter().chain(CONTEXT_BOUNDS) {
        assert!(body.contains(id), "the body documents the bounds' {id}");
        assert!(
            !data(&read(Path::new("abilities/cognition/cognition.scx"))).contains_key(id),
            "{id} has no <data> default"
        );
    }
    for id in CONTEXT_BOUNDS {
        assert!(
            cognition.contains(&format!("bounds.{id}")),
            "the cognition reads {id} from bounds"
        );
    }
    assert!(
        body.contains(r#"<param name="bounds" expr="bounds"/>"#),
        "the body passes its bounds on to the cognition"
    );
}

/// Each inactivity bound has a backstop above it, and a turn's stages fit
/// inside the body's turn bound, so the turn's bound never cuts off a stage
/// that is still within its own (decision D101).
#[test]
fn every_inactivity_bound_has_a_backstop_and_the_stages_nest_in_the_turn() {
    for (document, inactivity, ceiling) in [
        (
            "abilities/cognition/cognition.scx",
            "child_operation_timeout_ms",
            "child_operation_ceiling_ms",
        ),
        (
            "abilities/reflection/reflection.scx",
            "select_timeout_ms",
            "select_ceiling_ms",
        ),
        (
            "abilities/reflection/reflection.scx",
            "revising_timeout_ms",
            "revising_ceiling_ms",
        ),
        (
            "bots/builder/builder.scx",
            "generate_timeout_ms",
            "generate_ceiling_ms",
        ),
    ] {
        assert!(
            millis(document, inactivity) < millis(document, ceiling),
            "{document}: {inactivity} is below its backstop {ceiling}"
        );
    }
    // The stages before reasoning answer inside the stage's inactivity bound.
    let stage = millis(
        "abilities/cognition/cognition.scx",
        "child_operation_timeout_ms",
    );
    assert!(millis("abilities/autonomy/autonomy.scx", "memory_timeout_ms") < stage);
    assert!(millis("abilities/cognition/cognition.scx", "decision_timeout_ms") < stage);
    // Autonomy's memory, the decision layer's decision and reasoning's stage at
    // its backstop, one after another, end before the body times the turn
    // out.
    let turn = millis("bots/floppy/floppy.scx", "processing_timeout_ms");
    let cascade = millis("abilities/autonomy/autonomy.scx", "memory_timeout_ms")
        + millis("abilities/cognition/cognition.scx", "decision_timeout_ms")
        + millis(
            "abilities/cognition/cognition.scx",
            "child_operation_ceiling_ms",
        );
    assert!(
        cascade < turn,
        "the cascade ({cascade} ms) ends inside the turn ({turn} ms)"
    );
}

/// Streaming is per backend (decision D101): every model call that is bounded
/// by inactivity lets the run's `FLOPPY_STREAM` decide (the generate
/// package's `stream` slot), and no document asks for a stream itself, so a
/// run that does not set it streams nothing (the default docs/defaults.md states).
#[test]
fn the_runs_stream_setting_alone_decides_whether_a_call_streams() {
    let manifest = read(Path::new("services/generate/package/generate.service.json"));
    assert!(
        manifest.contains(r#""env-slots": ["apikey", "baseurl", "stream"]"#),
        "{manifest}"
    );
    for document in [
        "abilities/reasoning/reasoning.scx",
        "abilities/reflection/reflection.scx",
        "bots/builder/builder.scx",
    ] {
        let text = read(Path::new(document));
        let invokes = text
            .matches(r#"type="https://github.com/stateforward/floppy/services/generate""#)
            .count();
        assert!(invokes > 0, "{document} calls the model");
        assert_eq!(
            text.matches(r#"env:stream="FLOPPY_STREAM""#).count(),
            invokes,
            "{document}: every model call lets the run decide"
        );
        assert!(
            !text.contains(r#"<param name="stream""#),
            "{document} never asks for a stream itself"
        );
    }
    let page = read(Path::new("docs/defaults.md"));
    assert!(
        page.contains("| `FLOPPY_STREAM` |")
            && page.contains("unset (the default), no call is streamed"),
        "docs/defaults.md states the default"
    );
}
