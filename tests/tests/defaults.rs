//! The defaults floppy ships are stated in one place: README.md's
//! "Defaults" section, and `.env.example` for the variables.
//!
//! Checked here, over every `.scx` under `bots/` and `abilities/`:
//!
//! * every `model` `<data>` is [`support::DEFAULT_MODEL`], and the model
//!   name appears nowhere else in a document, so a request's or an
//!   activation's fallback is the `<data>` itself;
//! * the variables the documents' `env` attributes name are exactly
//!   `.env.example`'s names (bare, without values) and
//!   [`support::GRANTED`];
//! * every row of README.md's "Timeouts and bounds" table names a document's
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
/// model, mosfet's speech decoder, speech encoder and voice classifiers'
/// own), not the generation model: each `model` `<data>` is `null` (the
/// service's request names none of floppy's) or a model README.md's
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
    let readme = read(Path::new("README.md"));
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
                model == "null" || readme.contains(&format!("`{}`", model.trim_matches('\''))),
                "{}: the speech model {model} is README.md's",
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
        models, 6,
        "builder, floppy, cognition, reasoning, reflection and reading"
    );
    assert!(
        read(Path::new("README.md")).contains(&format!("`{}`", support::DEFAULT_MODEL)),
        "README.md's Defaults names the model"
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
fn the_readme_states_every_timeout_and_bound_the_documents_ship() {
    let readme = read(Path::new("README.md"));
    let section = readme
        .split_once("### Timeouts and bounds")
        .expect("README.md has the Timeouts and bounds table")
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
            "README.md's row for {document} {id}"
        );
    }
    for path in documents() {
        for id in data(&read(&path)).into_keys() {
            if id.ends_with("_timeout_ms") {
                let document = path.to_str().expect("UTF-8 path");
                assert!(
                    rows.iter()
                        .any(|(row_document, row_id, _)| row_document == document && row_id == &id),
                    "README.md's table has no row for {document} {id}"
                );
            }
        }
    }
}
