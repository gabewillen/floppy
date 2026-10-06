//! Package-declared event contracts: the `events` field of extension and
//! invoked-service package manifests.
//!
//! A package may declare the events its component emits (`emits`: a service
//! sends them to the invoking session) and the events it accepts (`accepts`:
//! an invoking session sends them to `#_<invokeid>`), each with a JSON
//! Schema for the event's data:
//!
//! ```json
//! "events": {
//!   "emits": [
//!     {"name": "echo.reply", "schema-artifact": "events.json#/$defs/Reply", "schema-sha256": "<64 hex>"}
//!   ],
//!   "accepts": []
//! }
//! ```
//!
//! * `name` is an exact event name (an SCXML event name of at most
//!   [`MAX_DECLARED_EVENT_NAME_BYTES`] bytes with no `*` and no empty
//!   `.`-separated token); each list is strictly sorted by name.
//! * `schema-artifact` names a JSON file relative to the manifest (the same
//!   reference syntax as `event:schema`: a relative path at or below the
//!   manifest's directory, optionally with a JSON Pointer fragment), and
//!   `schema-sha256` is the SHA-256 of that file's bytes.
//! * Each schema is resolved exactly like an `event:schema` reference
//!   ([`scxml_event_schema::resolve`]: dialects, references, bundling,
//!   canonical JSON), so a declared contract and a document's own schema
//!   compare by their canonical text.
//!
//! The field is optional: a manifest without it declares no event contract,
//! and its canonical form omits it. A manifest with it declares a closed
//! contract: the session host checks every event the service emits, and
//! every `<send>` to `#_<invokeid>`, against it (an undeclared name fails
//! like a schema mismatch); the validator cross-checks a document's
//! `event:schema` against it (`SCXML095`, `SCXML096`).
//!
//! Every function is pure, deterministic, and bounded; none records
//! telemetry (callers run them inside their own boundary span).

use std::collections::BTreeMap;

use crate::schema::MAX_EXTENSION_PACKAGE_FIELD_BYTES;

/// Maximum entries of each of `emits` and `accepts`.
pub const MAX_DECLARED_EVENTS: usize = 64;
/// Maximum bytes of one declared event name.
pub const MAX_DECLARED_EVENT_NAME_BYTES: usize = 256;
/// The members of one declared event, in canonical order.
const EVENT_FIELDS: [&str; 3] = ["name", "schema-artifact", "schema-sha256"];
/// The members of the `events` object, in canonical order.
const EVENTS_FIELDS: [&str; 2] = ["emits", "accepts"];

/// One declared event: its exact name and its data schema's artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageEvent {
    /// Exact event name.
    pub name: String,
    /// The schema file, relative to the manifest, with an optional JSON
    /// Pointer fragment (`events.json#/$defs/Reply`).
    pub schema_artifact: String,
    /// Lowercase hex SHA-256 of the schema file's bytes.
    pub schema_sha256: String,
}

impl PackageEvent {
    /// The schema file part of `schema-artifact` (before `#`).
    #[must_use]
    pub fn schema_file(&self) -> &str {
        self.schema_artifact
            .split_once('#')
            .map_or(self.schema_artifact.as_str(), |(file, _)| file)
    }

    /// The JSON Pointer fragment of `schema-artifact`, when it has one.
    #[must_use]
    pub fn schema_fragment(&self) -> Option<&str> {
        self.schema_artifact
            .split_once('#')
            .map(|(_, fragment)| fragment)
    }
}

/// A package's declared event contract (the manifest's `events` field).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageEvents {
    /// Events the component emits, sorted by name.
    pub emits: Vec<PackageEvent>,
    /// Events the component accepts, sorted by name.
    pub accepts: Vec<PackageEvent>,
}

/// One declared event with its resolved schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredEvent {
    /// Exact event name.
    pub name: String,
    /// The resolved schema as canonical JSON
    /// ([`scxml_event_schema::Resolved::schema`]).
    pub schema: String,
    /// Lowercase hex SHA-256 of `schema`.
    pub sha256: String,
}

/// A package's event contract with every schema resolved.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeclaredEvents {
    /// Whether the manifest declares a contract at all (an `events` field).
    pub declared: bool,
    /// Emitted events, sorted by name.
    pub emits: Vec<DeclaredEvent>,
    /// Accepted events, sorted by name.
    pub accepts: Vec<DeclaredEvent>,
}

impl DeclaredEvents {
    /// The declared emitted event named exactly `name`.
    #[must_use]
    pub fn emitted(&self, name: &str) -> Option<&DeclaredEvent> {
        find(&self.emits, name)
    }

    /// The declared accepted event named exactly `name`.
    #[must_use]
    pub fn accepted(&self, name: &str) -> Option<&DeclaredEvent> {
        find(&self.accepts, name)
    }
}

fn find<'a>(events: &'a [DeclaredEvent], name: &str) -> Option<&'a DeclaredEvent> {
    events
        .binary_search_by(|event| event.name.as_str().cmp(name))
        .ok()
        .and_then(|index| events.get(index))
}

/// Whether `name` is a declarable event name: a valid SCXML event name of
/// at most [`MAX_DECLARED_EVENT_NAME_BYTES`] bytes with no `*` and no empty
/// `.`-separated token.
#[must_use]
pub fn is_declared_event_name(name: &str) -> bool {
    name.len() <= MAX_DECLARED_EVENT_NAME_BYTES
        && scxml_types::EventName::new(name).is_ok()
        && !name.contains('*')
        && name.split('.').all(|token| !token.is_empty())
}

impl PackageEvents {
    /// Whether no event is declared (the manifest omits `events`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.emits.is_empty() && self.accepts.is_empty()
    }

    /// Parses the optional `events` field: absent is empty; present, an
    /// object with exactly `emits` and `accepts`, each an array of at most
    /// [`MAX_DECLARED_EVENTS`] objects with exactly the string members
    /// `name`, `schema-artifact`, and `schema-sha256`. A present field
    /// declaring no event is not canonical and is rejected. The entries are
    /// judged by [`Self::check`].
    ///
    /// # Errors
    ///
    /// A message naming the offending member, never a value.
    pub fn from_json(value: Option<&serde_json::Value>) -> Result<Self, String> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let serde_json::Value::Object(object) = value else {
            return Err("field `events` must be an object".into());
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| !EVENTS_FIELDS.contains(&key.as_str()))
        {
            return Err(format!(
                "field `events` has an unknown member `{}` (the members are emits, accepts)",
                unknown.escape_debug()
            ));
        }
        let list = |member: &str| -> Result<Vec<PackageEvent>, String> {
            let Some(serde_json::Value::Array(items)) = object.get(member) else {
                return Err(format!("field `events` needs the array `{member}`"));
            };
            if items.len() > MAX_DECLARED_EVENTS {
                return Err(format!(
                    "field `events.{member}` lists more than {MAX_DECLARED_EVENTS} events"
                ));
            }
            items
                .iter()
                .map(|item| {
                    let serde_json::Value::Object(entry) = item else {
                        return Err(format!("field `events.{member}` must list objects"));
                    };
                    if entry.len() != EVENT_FIELDS.len()
                        || !entry.keys().all(|key| EVENT_FIELDS.contains(&key.as_str()))
                    {
                        return Err(format!(
                            "an entry of `events.{member}` must have exactly the members name, schema-artifact, schema-sha256"
                        ));
                    }
                    let text = |name: &str| match entry.get(name) {
                        Some(serde_json::Value::String(text)) => Ok(text.clone()),
                        _ => Err(format!(
                            "member `{name}` of an `events.{member}` entry must be a string"
                        )),
                    };
                    Ok(PackageEvent {
                        name: text("name")?,
                        schema_artifact: text("schema-artifact")?,
                        schema_sha256: text("schema-sha256")?,
                    })
                })
                .collect()
        };
        let events = Self {
            emits: list("emits")?,
            accepts: list("accepts")?,
        };
        if events.is_empty() {
            return Err(
                "field `events` declares no event (omit it to declare no event contract)".into(),
            );
        }
        Ok(events)
    }

    /// The canonical manifest field: empty when no event is declared, else
    /// `,"events":{"emits":[...],"accepts":[...]}` with each entry's members
    /// in canonical order. Parsing it yields `self`.
    #[must_use]
    pub fn canonical_field(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let text = |value: &str| serde_json::Value::String(value.to_owned()).to_string();
        let list = |events: &[PackageEvent]| {
            events
                .iter()
                .map(|event| {
                    format!(
                        "{{\"name\":{},\"schema-artifact\":{},\"schema-sha256\":{}}}",
                        text(&event.name),
                        text(&event.schema_artifact),
                        text(&event.schema_sha256)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            ",\"events\":{{\"emits\":[{}],\"accepts\":[{}]}}",
            list(&self.emits),
            list(&self.accepts)
        )
    }

    /// Checks each list's bounds, order, names, artifacts, and digests.
    ///
    /// # Errors
    ///
    /// A message naming the offending list (`package` prefixes it), never a
    /// value.
    pub fn check(&self, package: &str) -> Result<(), String> {
        for (member, events) in [("emits", &self.emits), ("accepts", &self.accepts)] {
            if events.len() > MAX_DECLARED_EVENTS {
                return Err(format!(
                    "{package} field `events.{member}` lists more than {MAX_DECLARED_EVENTS} events"
                ));
            }
            if !events.windows(2).all(|pair| {
                pair.first().map(|event| event.name.as_str())
                    < pair.get(1).map(|event| event.name.as_str())
            }) {
                return Err(format!(
                    "{package} field `events.{member}` must be sorted by name with no name twice"
                ));
            }
            for event in events {
                if !is_declared_event_name(&event.name) {
                    return Err(format!(
                        "{package} field `events.{member}` names an invalid event (an SCXML event name of at most {MAX_DECLARED_EVENT_NAME_BYTES} bytes with no `*` and no empty token)"
                    ));
                }
                if event.schema_artifact.len() > MAX_EXTENSION_PACKAGE_FIELD_BYTES
                    || scxml_event_schema::resource_problem(event.schema_file()).is_some()
                {
                    return Err(format!(
                        "{package} field `events.{member}` has a `schema-artifact` that is not a relative path inside the manifest's directory (with an optional JSON Pointer fragment)"
                    ));
                }
                if event.schema_sha256.len() != 64
                    || !event
                        .schema_sha256
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(format!(
                        "{package} field `events.{member}` has a `schema-sha256` that is not 64 lowercase hex digits"
                    ));
                }
            }
        }
        Ok(())
    }

    /// The distinct schema files the declared events name, sorted.
    #[must_use]
    pub fn schema_files(&self) -> Vec<&str> {
        let mut files: Vec<&str> = self
            .emits
            .iter()
            .chain(&self.accepts)
            .map(PackageEvent::schema_file)
            .collect();
        files.sort_unstable();
        files.dedup();
        files
    }

    /// Verifies every schema file against its declared digest and resolves
    /// every declared schema ([`scxml_event_schema::resolve`]). `files` holds
    /// the bytes of each [`Self::schema_files`] entry, read relative to the
    /// manifest by the caller.
    ///
    /// # Errors
    ///
    /// A message for a check failure ([`Self::check`]), a missing file, a
    /// digest mismatch, or a schema that does not resolve.
    pub fn resolve(
        &self,
        package: &str,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<DeclaredEvents, String> {
        self.check(package)?;
        let resolve_list = |member: &str, events: &[PackageEvent]| {
            events
                .iter()
                .map(|event| {
                    let bytes = files.get(event.schema_file()).ok_or_else(|| {
                        format!(
                            "{package} field `events.{member}`: the bytes of a schema file are missing"
                        )
                    })?;
                    if scxml_event_schema::sha256_hex(bytes) != event.schema_sha256 {
                        return Err(format!(
                            "{package} field `events.{member}`: a schema file does not match its `schema-sha256`"
                        ));
                    }
                    let resolved = scxml_event_schema::resolve(bytes, event.schema_fragment())
                        .map_err(|error| {
                            format!(
                                "{package} field `events.{member}`: the schema of `{}` does not resolve: {error}",
                                event.name
                            )
                        })?;
                    Ok(DeclaredEvent {
                        name: event.name.clone(),
                        schema: resolved.schema,
                        sha256: resolved.sha256,
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        };
        Ok(DeclaredEvents {
            declared: !self.is_empty(),
            emits: resolve_list("emits", &self.emits)?,
            accepts: resolve_list("accepts", &self.accepts)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &[u8] = br#"{"$defs":{"Reply":{"type":"object","required":["text"]}}}"#;

    fn digest() -> String {
        scxml_event_schema::sha256_hex(SCHEMA)
    }

    fn events() -> PackageEvents {
        PackageEvents {
            emits: vec![PackageEvent {
                name: "echo.reply".into(),
                schema_artifact: "events.json#/$defs/Reply".into(),
                schema_sha256: digest(),
            }],
            accepts: vec![PackageEvent {
                name: "echo.say".into(),
                schema_artifact: "events.json".into(),
                schema_sha256: digest(),
            }],
        }
    }

    #[test]
    fn the_field_round_trips_canonically() {
        let field = events().canonical_field();
        let object: serde_json::Value =
            serde_json::from_str(&format!("{{{}}}", field.trim_start_matches(','))).expect("JSON");
        assert_eq!(
            PackageEvents::from_json(object.get("events")).expect("parses"),
            events()
        );
        assert_eq!(PackageEvents::from_json(None), Ok(PackageEvents::default()));
        assert_eq!(PackageEvents::default().canonical_field(), "");
    }

    #[test]
    fn malformed_fields_fail_closed() {
        for bad in [
            "[]",
            r#"{"emits":[]}"#,
            r#"{"emits":[],"accepts":[]}"#,
            r#"{"emits":[],"accepts":[],"other":[]}"#,
            r#"{"emits":[{"name":"a"}],"accepts":[]}"#,
            r#"{"emits":[{"name":"a","schema-artifact":"s.json","schema-sha256":1}],"accepts":[]}"#,
            r#"{"emits":[{"name":"a","schema-artifact":"s.json","schema-sha256":"x","extra":"y"}],"accepts":[]}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(bad).expect("JSON");
            assert!(PackageEvents::from_json(Some(&value)).is_err(), "{bad}");
        }
    }

    #[test]
    fn checks_bind_names_order_artifacts_and_digests() {
        assert_eq!(events().check("service package"), Ok(()));
        let mut unsorted = events();
        unsorted.emits.push(PackageEvent {
            name: "a".into(),
            ..unsorted.emits[0].clone()
        });
        assert!(unsorted.check("p").is_err());
        let mut twice = events();
        twice.emits.push(twice.emits[0].clone());
        assert!(twice.check("p").is_err());
        for name in ["", "a b", "a.*", "a..b", ".a", "a."] {
            let mut bad = events();
            bad.emits[0].name = name.into();
            assert!(bad.check("p").is_err(), "{name:?}");
        }
        for artifact in ["../x.json", "/x.json", "https://e/x.json", "a//b.json", ""] {
            let mut bad = events();
            bad.emits[0].schema_artifact = artifact.into();
            assert!(bad.check("p").is_err(), "{artifact}");
        }
        let mut digest = events();
        digest.emits[0].schema_sha256 = "A".repeat(64);
        assert!(digest.check("p").is_err());
    }

    #[test]
    fn resolution_verifies_digests_and_canonicalizes_schemas() {
        let files = BTreeMap::from([("events.json".to_owned(), SCHEMA.to_vec())]);
        let declared = events().resolve("p", &files).expect("resolves");
        assert!(declared.declared);
        let reply = declared.emitted("echo.reply").expect("declared");
        assert_eq!(reply.schema, r#"{"required":["text"],"type":"object"}"#);
        assert_eq!(
            reply.sha256,
            scxml_event_schema::sha256_hex(reply.schema.as_bytes())
        );
        assert!(declared.accepted("echo.say").is_some());
        assert!(declared.emitted("echo.say").is_none());
        let tampered = BTreeMap::from([("events.json".to_owned(), b"{}".to_vec())]);
        assert!(events().resolve("p", &tampered).is_err());
        assert!(events().resolve("p", &BTreeMap::new()).is_err());
        let mut pointer = events();
        pointer.emits[0].schema_artifact = "events.json#/$defs/Missing".into();
        assert!(pointer.resolve("p", &files).is_err());
    }
}
