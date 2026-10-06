//! Property tests at the canonical report decoder trust boundary
//! (`decode_canonical`).
//!
//! Hostile input starts from arbitrary bytes and from structural mutations of
//! a real validator artifact (the adversarial fixture). The decoder must
//! never panic, must reject unknown object fields, and anything it accepts
//! must satisfy the envelope invariants (it re-encodes, and the re-encoding
//! decodes to the same value). Runs are deterministic: a fixed RNG seed, a
//! fixed case count, and no failure-persistence files.

use std::sync::OnceLock;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use scxml_reporting::{decode_canonical, encode_canonical};
use scxml_syntax::SourceId;
use scxml_validator_core::{MemorySchemaResolver, SchemaMode, Validator};
use scxml_validator_xsd::XsdSchemaBackend;
use serde_json::Value;

/// The source id of the documents these tests parse.
const SOURCE_ID: SourceId = SourceId::new(1);

const SOURCE: &str = include_str!("../../../tests/fixtures/validator-adversarial.scxml");

fn config() -> Config {
    Config {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x2e90_12d5),
        ..Config::default()
    }
}

/// Canonical bytes of the adversarial fixture's validator artifact.
fn canonical_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let validator = Validator::new(MemorySchemaResolver::new(), XsdSchemaBackend)
            .with_schema_mode(SchemaMode::Off);
        let outcome = validator.validate_source(SOURCE_ID, "property.scxml", SOURCE);
        let artifact = outcome
            .report
            .canonical_artifact(&outcome.source)
            .expect("adversarial fixture produces a canonical artifact");
        encode_canonical(&artifact).expect("adversarial artifact encodes")
    })
}

fn canonical_value() -> Value {
    serde_json::from_slice(canonical_bytes()).expect("canonical bytes are JSON")
}

/// JSON pointers of every node in document order.
fn pointers(value: &Value, prefix: &str, output: &mut Vec<String>) {
    output.push(prefix.to_owned());
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                pointers(child, &format!("{prefix}/{escaped}"), output);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                pointers(child, &format!("{prefix}/{index}"), output);
            }
        }
        _ => {}
    }
}

fn object_pointers(value: &Value) -> Vec<String> {
    let mut all = Vec::new();
    pointers(value, "", &mut all);
    all.into_iter()
        .filter(|pointer| value.pointer(pointer).is_some_and(Value::is_object))
        .collect()
}

fn replacement() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<u64>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        (0_u64..4_096).prop_map(Value::from),
        any::<String>().prop_map(Value::String),
        Just(Value::Array(Vec::new())),
        Just(Value::Object(serde_json::Map::new())),
    ]
}

/// Anything the decoder accepts must satisfy the envelope invariants.
fn assert_accepted_is_canonical(bytes: &[u8]) {
    if let Ok(artifact) = decode_canonical(bytes) {
        let encoded = encode_canonical(&artifact).expect("accepted artifacts re-encode");
        assert_eq!(
            decode_canonical(&encoded).expect("re-encoded artifacts decode"),
            artifact
        );
    }
}

#[test]
fn fixture_round_trips_byte_for_byte() {
    let artifact = decode_canonical(canonical_bytes()).expect("fixture decodes");
    assert_eq!(
        encode_canonical(&artifact).expect("fixture re-encodes"),
        canonical_bytes()
    );
}

proptest! {
    #![proptest_config(config())]

    /// Arbitrary bytes never panic the decoder.
    #[test]
    fn decoder_is_total_on_arbitrary_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..1_024)
    ) {
        assert_accepted_is_canonical(&bytes);
    }

    /// Replacing any node of a real artifact with a hostile value is either
    /// rejected or yields an artifact that still satisfies every invariant.
    #[test]
    fn structural_mutations_fail_closed(
        index in any::<prop::sample::Index>(),
        value in replacement(),
    ) {
        let mut document = canonical_value();
        let mut all = Vec::new();
        pointers(&document, "", &mut all);
        let pointer = &all[index.index(all.len())];
        if let Some(slot) = document.pointer_mut(pointer) {
            *slot = value;
        }
        let bytes = serde_json::to_vec(&document).expect("mutated JSON serializes");
        assert_accepted_is_canonical(&bytes);
    }

    /// An unknown field in any object of the artifact is rejected.
    #[test]
    fn unknown_fields_are_rejected(index in any::<prop::sample::Index>()) {
        let mut document = canonical_value();
        let objects = object_pointers(&document);
        let pointer = &objects[index.index(objects.len())];
        if let Some(Value::Object(map)) = document.pointer_mut(pointer) {
            map.insert("x-unknown-property".into(), Value::Null);
        }
        let bytes = serde_json::to_vec(&document).expect("mutated JSON serializes");
        prop_assert!(decode_canonical(&bytes).is_err(), "accepted unknown field at `{pointer}`");
    }

    /// Byte-level corruption and truncation of real canonical bytes never
    /// panic the decoder.
    #[test]
    fn byte_mutations_fail_closed(
        index in any::<prop::sample::Index>(),
        replacement in any::<u8>(),
        truncate in any::<bool>(),
    ) {
        let mut bytes = canonical_bytes().to_vec();
        let position = index.index(bytes.len());
        if truncate {
            bytes.truncate(position);
        } else {
            bytes[position] = replacement;
        }
        assert_accepted_is_canonical(&bytes);
    }
}
