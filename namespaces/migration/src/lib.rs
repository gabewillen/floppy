//! The `migration` extension namespace: `<migration:plan>` as a
//! `stateforward:scxml/executable-content-extension` Wasm component.
//!
//! Alembic's revision graph walk, agnostic of the database (the user's
//! direction: "make it like alembic that's agnostic of the database"): it
//! knows revisions, never an engine, never SQL. The migration ability
//! (`abilities/migration/migration.scx`) reads the current revision through the sql
//! service, plans with this element, and applies the planned statements in
//! one transaction. Documents bind [`NAMESPACE_URI`] (conventionally with
//! the prefix `migration`):
//!
//! ```xml
//! <migration:plan location="plan" revisions="revisions" current="current" target="'head'"/>
//! ```
//!
//! # `<migration:plan location="L" revisions="R" current="C" target="T"/>`
//!
//! `revisions` (required) evaluates to an array of revisions, each an
//! object `{revision, down_revision, upgrade, downgrade}` (Alembic's
//! script: `revision` a non-empty string, `down_revision` the revision it
//! follows or `null` for the first, `upgrade` and `downgrade` arrays of
//! statements, each an object the element passes through untouched; other
//! members are ignored). The history must be linear: one first revision,
//! every `down_revision` a known revision, no revision followed by two
//! (Alembic's "multiple heads"), no cycle, and no revision named twice.
//!
//! `current` (required) evaluates to the database's current revision, a
//! string, or `null` (Alembic's `base`: no revision applied).
//!
//! `target` (optional, default `head`) evaluates to Alembic's target
//! syntax: `head` (the last revision), `base` (none), a revision, or a
//! relative step `+N` or `-N` from `current`.
//!
//! `location` (required) receives `{from, to, steps}`: `from` is `current`,
//! `to` the target revision (`null` for `base`), and `steps` the ordered
//! steps `{revision, direction, statements}`: upgrading, each revision
//! after `current` up to `to`, oldest first, with its `upgrade` statements
//! (`direction` `upgrade`); downgrading, each revision from `current` down
//! to the one after `to`, newest first, with its `downgrade` statements
//! (`direction` `downgrade`). At the target already, `steps` is empty.
//!
//! # Failures
//!
//! A malformed element (a missing, empty, unknown or repeated attribute, or
//! any content but white space), an expression that fails or evaluates to a
//! value of the wrong kind, a history that is not linear, a `current` or
//! `target` revision the history does not hold, a relative step past
//! `base` or `head`, or a failed assignment is an `execution` error with a
//! static detail (SCXML `error.execution`). A host failure that is not
//! recoverable is passed through unchanged.
//!
//! # Telemetry
//!
//! Each entry point (`initialize`, `execute`, `shutdown`) records one
//! observation through scxml.rs's `telemetry` WIT facade: its closed
//! operation, its closed outcome (`ok`, `rejected`, `failed`), and one closed
//! signal event. No attribute carries a revision, a statement, or a
//! location, so the cardinality is the product of the three closed enums.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; no authored unsafe code"
    )
)]

use std::collections::BTreeMap;

use scxml_types::DataValue as Data;
use serde_json::{Map, Value as Json};

/// The namespace's name: the last path segment of its repository namespace
/// URI before the tag.
pub const NAMESPACE_NAME: &str = "migration";

/// The exact namespace URI documents bind: tagless, so it names the
/// repository's latest release (decision D19); the manifest and schema name
/// it too.
pub const NAMESPACE_URI: &str = "https://github.com/stateforward/floppy/namespaces/migration";

pub use floppy_guest::element::{Attribute, Content, Executed, Failure, Host};
use floppy_guest::element::{assign_to, attribute_values, evaluate, json_of, no_content};

/// Executes one element with the given local name, attributes, and
/// flattened content.
///
/// # Errors
///
/// [`Failure::Execution`] for every malformed element, value, history, or
/// assignment; [`Failure::Host`] for a non-recoverable host failure.
pub fn execute<H: Host>(
    local_name: &str,
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    match local_name {
        "plan" => plan_element(attributes, content, host),
        _ => Ok(Executed::NotHandled),
    }
}

/// Executes `<migration:plan>`.
fn plan_element<H: Host>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, revisions, current, target] = attribute_values(
        attributes,
        ["location", "revisions", "current", "target"],
        [
            "migration:plan takes only the location, revisions, current and target attributes",
            "migration:plan repeats an attribute",
            "migration:plan has an empty attribute",
        ],
    )?;
    let location = location.ok_or(Failure::Execution("migration:plan requires a location"))?;
    let revisions = revisions.ok_or(Failure::Execution("migration:plan requires revisions"))?;
    let current = current.ok_or(Failure::Execution("migration:plan requires a current"))?;
    no_content(content, "migration:plan holds no content")?;
    let Some(Json::Array(revisions)) = json_of(&evaluate(
        host,
        revisions,
        "migration:plan revisions did not evaluate",
    )?) else {
        return Err(Failure::Execution(
            "migration:plan revisions is not an array",
        ));
    };
    let current = match evaluate(host, current, "migration:plan current did not evaluate")? {
        Data::Null => None,
        Data::String(current) if !current.is_empty() => Some(current),
        _ => {
            return Err(Failure::Execution(
                "migration:plan current is not a revision or null",
            ));
        }
    };
    let target = match target {
        None => "head".to_owned(),
        Some(target) => match evaluate(host, target, "migration:plan target did not evaluate")? {
            Data::String(target) if !target.is_empty() => target,
            _ => {
                return Err(Failure::Execution(
                    "migration:plan target is not a non-empty string",
                ));
            }
        },
    };
    let planned = plan(&revisions, current.as_deref(), &target).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(Json::Object(planned).to_string()),
        "migration:plan could not assign the plan",
    )?;
    Ok(Executed::Handled)
}

/// One revision of a linear history (Alembic's `Script`).
struct Script<'a> {
    revision: &'a str,
    upgrade: &'a Json,
    downgrade: &'a Json,
}

/// The history oldest first, when it is linear (see the crate
/// documentation).
fn history(revisions: &[Json]) -> Result<Vec<Script<'_>>, &'static str> {
    let mut by_down: BTreeMap<Option<&str>, &Map<String, Json>> = BTreeMap::new();
    let mut names = std::collections::BTreeSet::new();
    for revision in revisions {
        let Json::Object(fields) = revision else {
            return Err("migration:plan revision is not an object");
        };
        let name = match fields.get("revision") {
            Some(Json::String(name)) if !name.is_empty() => name.as_str(),
            _ => return Err("migration:plan revision has no revision id"),
        };
        let down = match fields.get("down_revision") {
            None | Some(Json::Null) => None,
            Some(Json::String(down)) if !down.is_empty() => Some(down.as_str()),
            _ => return Err("migration:plan down_revision is not a revision or null"),
        };
        for direction in ["upgrade", "downgrade"] {
            match fields.get(direction) {
                Some(Json::Array(statements)) if statements.iter().all(Json::is_object) => {}
                _ => {
                    return Err(
                        "migration:plan upgrade and downgrade must be arrays of statements",
                    );
                }
            }
        }
        if !names.insert(name) {
            return Err("migration:plan names a revision twice");
        }
        if by_down.insert(down, fields).is_some() {
            return Err("migration:plan history has multiple heads");
        }
    }
    let mut ordered = Vec::with_capacity(revisions.len());
    let mut previous: Option<&str> = None;
    while let Some(fields) = by_down.get(&previous) {
        let revision = fields
            .get("revision")
            .and_then(Json::as_str)
            .unwrap_or_default();
        ordered.push(Script {
            revision,
            upgrade: &fields["upgrade"],
            downgrade: &fields["downgrade"],
        });
        if ordered.len() > revisions.len() {
            return Err("migration:plan history has a cycle");
        }
        previous = Some(revision);
    }
    if ordered.len() != revisions.len() {
        return Err("migration:plan history is not one line from its first revision");
    }
    Ok(ordered)
}

/// The plan `{from, to, steps}` from `current` to `target` (see the crate
/// documentation).
///
/// # Errors
///
/// A static detail for a history that is not linear, or a `current` or
/// `target` it does not hold.
pub fn plan(
    revisions: &[Json],
    current: Option<&str>,
    target: &str,
) -> Result<Map<String, Json>, &'static str> {
    let history = history(revisions)?;
    // Positions: 0 is base, n is the n-th revision.
    let position = |revision: &str| {
        history
            .iter()
            .position(|known| known.revision == revision)
            .map(|index| index + 1)
    };
    let from = match current {
        None => 0,
        Some(current) => {
            position(current).ok_or("migration:plan current is not a revision of the history")?
        }
    };
    let to = match target {
        "head" => history.len(),
        "base" => 0,
        relative if relative.starts_with('+') || relative.starts_with('-') => {
            let steps: usize = relative[1..]
                .parse()
                .map_err(|_| "migration:plan target is not a revision or a relative step")?;
            if relative.starts_with('+') {
                from.checked_add(steps)
                    .filter(|to| *to <= history.len())
                    .ok_or("migration:plan target steps past head")?
            } else {
                from.checked_sub(steps)
                    .ok_or("migration:plan target steps past base")?
            }
        }
        revision => {
            position(revision).ok_or("migration:plan target is not a revision of the history")?
        }
    };
    let step = |revision: &Script<'_>, direction: &str, statements: &Json| {
        serde_json::json!({
            "revision": revision.revision,
            "direction": direction,
            "statements": statements,
        })
    };
    let steps: Vec<Json> = if to >= from {
        history[from..to]
            .iter()
            .map(|revision| step(revision, "upgrade", revision.upgrade))
            .collect()
    } else {
        history[to..from]
            .iter()
            .rev()
            .map(|revision| step(revision, "downgrade", revision.downgrade))
            .collect()
    };
    let name = |at: usize| match at {
        0 => Json::Null,
        at => Json::String(history[at - 1].revision.to_owned()),
    };
    let mut planned = Map::new();
    planned.insert("from".into(), name(from));
    planned.insert("to".into(), name(to));
    planned.insert("steps".into(), Json::Array(steps));
    Ok(planned)
}

floppy_guest::extension_guest!();

#[cfg(test)]
mod tests;
