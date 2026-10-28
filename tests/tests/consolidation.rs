//! The consolidation rest runs (`abilities/consolidation/consolidation.scx`,
//! decision D205) and the people statements of the conversation
//! (`abilities/conversation/conversation.scx`), run through `scx run` on
//! seeded memory.
//!
//! Each consolidation test seeds people through the memory ability (which
//! brings the database to the head revision), runs the consolidation as a
//! harness's child, and reads memory back: a person's representative takes
//! in the voices heard since (a running mean of directions), two people
//! alike enough are merged with everything keyed on the newer re-pointed,
//! and a consolidation cancelled mid-way leaves every representative whole
//! (each step one transaction). The `postgres_` tests run the same on the
//! Postgres engine (the pgsql service) against a fresh server each
//! ([`support::Pglite`]), ignored unless `FLOPPY_PGLITE_SERVER` or
//! `FLOPPY_POSTGRES_BIN` is set.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::Path;
use std::time::Duration;

use serde_json::{Value as Json, json};
use support::{Finished, Pglite, Run, logs, unique_scratch, workspace};

/// The SQLite database every SQLite case uses (in the granted directory).
const SQLITE: &str = "/memory/floppy.sqlite";
/// The longest one run may take.
const LIMIT: Duration = Duration::from_secs(300);

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A representative voice's compact text, as the conversation namespace
/// writes it: `i8:` and the base64 of the unit vector's components, each
/// rounded times 127 as a signed byte.
fn encode(voice: &[f64]) -> String {
    let norm = voice.iter().map(|x| x * x).sum::<f64>().sqrt();
    let bytes: Vec<u8> = voice
        .iter()
        .map(|x| {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "a whole number from -127 to 127"
            )]
            let quantized = (x / norm * 127.0).round().clamp(-127.0, 127.0) as i8;
            quantized.to_le_bytes()[0]
        })
        .collect();
    let mut text = String::from("i8:");
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|i| chunk.get(i).copied().unwrap_or(0));
        let sextets = [
            a >> 2,
            ((a & 3) << 4) | (b >> 4),
            ((b & 15) << 2) | (c >> 6),
            c & 63,
        ];
        for (i, sextet) in sextets.iter().enumerate() {
            text.push(if i <= chunk.len() {
                char::from(BASE64[usize::from(*sextet)])
            } else {
                '='
            });
        }
    }
    text
}

/// The unit vector of a representative's compact text.
fn decode(text: &str) -> Vec<f64> {
    let body = text.strip_prefix("i8:").expect("a representative");
    let mut bits = Vec::new();
    for byte in body.bytes().filter(|byte| *byte != b'=') {
        let value = BASE64
            .iter()
            .position(|known| *known == byte)
            .expect("base64");
        bits.push(u8::try_from(value).expect("a sextet"));
    }
    let mut bytes = Vec::new();
    for quad in bits.chunks(4) {
        let get = |i: usize| quad.get(i).copied().unwrap_or(0);
        let decoded = [
            (get(0) << 2) | (get(1) >> 4),
            (get(1) << 4) | (get(2) >> 2),
            (get(2) << 6) | get(3),
        ];
        bytes.extend_from_slice(&decoded[..quad.len() - 1]);
    }
    let components: Vec<f64> = bytes
        .iter()
        .map(|byte| f64::from(i8::from_le_bytes([*byte])) / 127.0)
        .collect();
    let norm = components.iter().map(|x| x * x).sum::<f64>().sqrt();
    components.iter().map(|x| x / norm).collect()
}

/// XML-escapes `text` for an attribute value.
fn attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One statement.
fn statement(sql: &str, parameters: &Json) -> Json {
    json!({"sql": sql, "parameters": parameters})
}

/// Statements seeding `person` with their first voice as their
/// representative (as the conversation's tag does).
fn person(person: &str, voice: &[f64]) -> Vec<Json> {
    vec![
        statement(
            "INSERT INTO bot_person (person_id) VALUES (?1)",
            &json!([person]),
        ),
        heard(person, &Json::from(voice.to_vec()).to_string()),
        statement(
            "INSERT INTO bot_person_centroid (person_id, kind, centroid, weight, first_seq, through_seq) SELECT CAST(?1 AS TEXT), 'voice', CAST(?2 AS TEXT), 1, MIN(seq), MIN(seq) FROM bot_person_embedding WHERE person_id = ?1",
            &json!([person, encode(voice)]),
        ),
    ]
}

/// A statement storing a voice (its JSON text) heard for `person`.
fn heard(person: &str, voice: &str) -> Json {
    statement(
        "INSERT INTO bot_person_embedding (person_id, kind, embedding) VALUES (?1, 'voice', ?2)",
        &json!([person, voice]),
    )
}

/// Runs the consolidation once on `database` after `seed`, then `inspect`;
/// with `stop`, the harness leaves the consolidation (cancelling it) that
/// long after it starts. `grant` grants the engine.
fn consolidate(
    database: &str,
    grant: &[String],
    seed: &[Json],
    inspect: &[Json],
    stop: Option<&str>,
) -> Finished {
    let dir = unique_scratch("consolidation");
    let abilities = dir.join("abilities");
    support::copy_ability("consolidation", &abilities);
    let stopping = stop.map_or(String::new(), |delay| {
        format!(
            r#"<onentry><send event="stop" delay="{delay}"/></onentry>
    <transition event="stop" target="inspecting"><log label="stopped" expr="'stopped'"/></transition>"#
        )
    });
    let document = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="harness">
  <datamodel>
    <data id="seeds" expr="{seed}"/>
    <data id="inspects" expr="{inspect}"/>
  </datamodel>
  <state id="harness" initial="seeding">
    <invoke id="memory" type="scxml" src="abilities/memory/memory.scx">
      <param name="database" expr="'{database}'"/>
    </invoke>
    <state id="seeding">
      <onentry><send event="started"/></onentry>
      <transition event="started">
        <send event="bot.ability.memory.input" target="#_memory">
          <param name="id" expr="'seed'"/>
          <param name="statements" expr="seeds"/>
        </send>
      </transition>
      <transition event="bot.ability.memory.output" target="consolidating"/>
      <transition event="bot.ability.failed" target="done"><log label="failed" expr="_event.data.message"/></transition>
    </state>
    <state id="consolidating">
      {stopping}
      <invoke id="consolidation" type="scxml" src="abilities/consolidation/consolidation.scx">
        <param name="memory" expr="'#_scxml_' + _sessionid + '.memory'"/>
        <param name="bounds" expr="({{consolidation_pause_ms: 100}})"/>
      </invoke>
      <transition event="error.*" target="done"><log label="failed" expr="_event.name"/></transition>
      <transition event="done.invoke.consolidation" target="inspecting">
        <log label="consolidated" expr="JSON.stringify(_event.data)"/>
      </transition>
    </state>
    <state id="inspecting">
      <onentry>
        <send event="bot.ability.memory.input" target="#_memory">
          <param name="id" expr="'inspect'"/>
          <param name="statements" expr="inspects"/>
        </send>
      </onentry>
      <transition event="bot.ability.memory.output" target="done">
        <log label="inspected" expr="JSON.stringify(_event.data.results)"/>
      </transition>
      <transition event="bot.ability.failed" target="done"><log label="failed" expr="_event.data.message"/></transition>
    </state>
  </state>
  <final id="done"/>
</scxml>"##,
        seed = attribute(&Json::from(seed.to_vec()).to_string()),
        inspect = attribute(&Json::from(inspect.to_vec()).to_string()),
    );
    let path = support::write(&dir, "harness.scx", &document);
    let mut args = vec!["--fuel".to_owned(), support::FUEL.to_owned()];
    args.extend_from_slice(grant);
    args.push(path.display().to_string());
    // The run ends when the harness reaches its final state.
    Run::start(&args, &[]).finish(LIMIT)
}

/// `--dir` granting the sql service a fresh memory directory.
fn sqlite_grant() -> Vec<String> {
    support::memory_grant()
}

/// The text of the one log labelled `label`.
fn logged(finished: &Finished, label: &str) -> Option<String> {
    logs(&finished.records)
        .into_iter()
        .find(|(logged, _)| logged == label)
        .map(|(_, text)| text)
}

/// The rows of each inspected statement, each its values.
fn inspected(finished: &Finished) -> Vec<Vec<Vec<Json>>> {
    let text = logged(finished, "inspected")
        .unwrap_or_else(|| panic!("memory was read back: {:?}", finished.records));
    let results: Json = serde_json::from_str(&text).expect("the results are JSON");
    results
        .as_array()
        .expect("an array of results")
        .iter()
        .map(|result| {
            result["rows"]
                .as_array()
                .expect("rows")
                .iter()
                .map(|row| row["values"].as_array().expect("values").clone())
                .collect()
        })
        .collect()
}

fn consolidated(finished: &Finished) -> Json {
    let text = logged(finished, "consolidated").unwrap_or_else(|| {
        panic!(
            "the consolidation ended: {:?} {}",
            finished.records, finished.stderr
        )
    });
    serde_json::from_str(&text).expect("the donedata is JSON")
}

#[test]
fn rest_folds_each_persons_voices_into_their_representative() {
    folds_voices(SQLITE, &sqlite_grant());
}

#[test]
#[ignore = "needs a Postgres server: FLOPPY_PGLITE_SERVER or FLOPPY_POSTGRES_BIN"]
fn postgres_rest_folds_each_persons_voices_into_their_representative() {
    let server = Pglite::start().expect("a Postgres server is configured");
    folds_voices(&server.database(), &support::pgsql_grant());
}

fn folds_voices(database: &str, grant: &[String]) {
    let mut seed = person("person-1", &[1.0, 0.0]);
    seed.extend(person("person-2", &[-1.0, 0.0]));
    seed.push(heard("person-1", "[0,1]"));
    seed.push(heard("person-2", "damaged"));
    seed.push(heard("person-1", "[0,3]"));
    let inspect = [
        json!({"sql": "SELECT person_id, centroid, weight, through_seq, (SELECT MAX(seq) FROM bot_person_embedding AS e WHERE e.person_id = c.person_id) AS last FROM bot_person_centroid AS c ORDER BY person_id"}),
    ];
    let finished = consolidate(database, grant, &seed, &inspect, None);
    assert_eq!(
        consolidated(&finished),
        json!({"folded": 2, "merged": 0, "steps": 2, "failed": false, "reason": null}),
        "{}",
        finished.stderr
    );
    let rows = &inspected(&finished)[0];
    assert_eq!(rows.len(), 2, "{rows:?}");
    // person-1: [1,0] weighing one, then two voices along [0,1]: the unit
    // vector of [1,2].
    assert_eq!(rows[0][0], json!("person-1"));
    let centroid = decode(rows[0][1].as_str().expect("a representative"));
    let expected = [1.0 / 5.0_f64.sqrt(), 2.0 / 5.0_f64.sqrt()];
    assert!(
        (centroid[0] - expected[0]).abs() < 0.01 && (centroid[1] - expected[1]).abs() < 0.01,
        "{centroid:?}"
    );
    assert_eq!(rows[0][2], json!(3));
    assert_eq!(rows[0][3], rows[0][4], "every voice of person-1 is folded");
    // person-2: the damaged voice is passed over and not counted.
    assert_eq!(rows[1][0], json!("person-2"));
    assert_eq!(rows[1][1], json!(encode(&[-1.0, 0.0])));
    assert_eq!(rows[1][2], json!(1));
    assert_eq!(rows[1][3], rows[1][4]);
}

#[test]
fn rest_merges_duplicate_people_and_repoints_everything_keyed_on_them() {
    merges_people(SQLITE, &sqlite_grant());
}

#[test]
#[ignore = "needs a Postgres server: FLOPPY_PGLITE_SERVER or FLOPPY_POSTGRES_BIN"]
fn postgres_rest_merges_duplicate_people_and_repoints_everything_keyed_on_them() {
    let server = Pglite::start().expect("a Postgres server is configured");
    merges_people(&server.database(), &support::pgsql_grant());
}

/// A `bot_memory` row.
fn memory(id: &str, context_ref: Option<&str>, subject_ref: Option<&str>, content: &str) -> Json {
    statement(
        "INSERT INTO bot_memory (memory_id, context_ref, subject_ref, content, query_tags) VALUES (?1, ?2, ?3, ?4, 'conversation_memory')",
        &json!([id, context_ref, subject_ref, content]),
    )
}

/// Three people, the second alike enough to the first to be merged into
/// them, and every kind of row keyed on the second.
fn duplicate_people() -> Vec<Json> {
    let mut seed = person("person-1", &[1.0, 0.0]);
    seed.extend(person("person-2", &[0.98, 0.2]));
    seed.extend(person("person-3", &[0.0, 1.0]));
    seed.push(heard("person-1", "[0.99,0.14]"));
    seed.extend([
        memory(
            "m1",
            Some(r#"{"identity_groups":[[],["person-2"]]}"#),
            None,
            r#"{"source_ids":["person-2"],"text":"hi"}"#,
        ),
        memory(
            "m2",
            Some(r#"{"identity_groups":[["person-1"],["person-2"]]}"#),
            None,
            r#"{"text":"person-2 is here"}"#,
        ),
        memory(
            "m3",
            Some("lamp"),
            None,
            r#"{"stimulus":{"participants":[{"ref":"person-2","role":"source"}]}}"#,
        ),
        memory("m4", None, Some("person-2"), "a note"),
        memory(
            "m5",
            Some(r#"{"identity_groups":[[],["person-3"]]}"#),
            None,
            r#"{"source_ids":["person-3"]}"#,
        ),
        statement(
            "INSERT INTO bot_stm_turn (turn_id, at, direction, speakers_json, text, conversation) VALUES ('t1', '2026-10-24T00:00:00Z', 'heard', ?1, 'hi', 'c1')",
            &json!([r#"["person-2","alice"]"#]),
        ),
        statement(
            "INSERT INTO bot_stm_memory (stm_id, stimulus_name, payload_json, source, target, created_at) VALUES ('1', 'bot.input', ?1, 'person-2', 'floppy', '2026-10-24T00:00:00Z')",
            &json!([r#"{"source":"person-2"}"#]),
        ),
    ]);
    seed
}

fn merges_people(database: &str, grant: &[String]) {
    let seed = duplicate_people();
    let inspect = [
        json!({"sql": "SELECT person_id FROM bot_person ORDER BY person_id"}),
        json!({"sql": "SELECT person_id, weight FROM bot_person_centroid ORDER BY person_id"}),
        json!({"sql": "SELECT person_id, COUNT(*) FROM bot_person_embedding GROUP BY person_id ORDER BY person_id"}),
        json!({"sql": "SELECT memory_id, context_ref, subject_ref, content FROM bot_memory ORDER BY memory_id"}),
        json!({"sql": "SELECT speakers_json FROM bot_stm_turn"}),
        json!({"sql": "SELECT payload_json, source, target FROM bot_stm_memory"}),
    ];
    let finished = consolidate(database, grant, &seed, &inspect, None);
    assert_eq!(
        consolidated(&finished),
        json!({"folded": 1, "merged": 1, "steps": 2, "failed": false, "reason": null}),
        "{}",
        finished.stderr
    );
    let read = inspected(&finished);
    assert_eq!(read[0], [vec![json!("person-1")], vec![json!("person-3")]]);
    assert_eq!(
        read[1],
        [
            vec![json!("person-1"), json!(3)],
            vec![json!("person-3"), json!(1)]
        ]
    );
    assert_eq!(
        read[2],
        [
            vec![json!("person-1"), json!(3)],
            vec![json!("person-3"), json!(1)]
        ]
    );
    assert_eq!(
        read[3],
        [
            vec![
                json!("m1"),
                json!(r#"{"identity_groups":[[],["person-1"]]}"#),
                Json::Null,
                json!(r#"{"source_ids":["person-1"],"text":"hi"}"#)
            ],
            // The relationship between the two is the person's with
            // themselves, as the conversation computes its bytes; a ref
            // inside another string is left as it was.
            vec![
                json!("m2"),
                json!(r#"{"identity_groups":[["person-1"],["person-1"]]}"#),
                Json::Null,
                json!(r#"{"text":"person-2 is here"}"#)
            ],
            vec![
                json!("m3"),
                json!("lamp"),
                Json::Null,
                json!(r#"{"stimulus":{"participants":[{"ref":"person-1","role":"source"}]}}"#)
            ],
            vec![json!("m4"), Json::Null, json!("person-1"), json!("a note")],
            vec![
                json!("m5"),
                json!(r#"{"identity_groups":[[],["person-3"]]}"#),
                Json::Null,
                json!(r#"{"source_ids":["person-3"]}"#)
            ],
        ]
    );
    assert_eq!(read[4], [vec![json!(r#"["person-1","alice"]"#)]]);
    assert_eq!(
        read[5],
        [vec![
            json!(r#"{"source":"person-1"}"#),
            json!("person-1"),
            json!("floppy")
        ]]
    );
}

#[test]
fn a_consolidation_cancelled_mid_way_leaves_every_representative_whole() {
    // Twelve people with forty voices each to fold, eight a step: sixty
    // steps, far more than the harness waits before it leaves.
    let axis = |n: u32, lean: f64| -> Vec<f64> {
        (0..12)
            .map(|d| {
                if d == n {
                    1.0
                } else if d == (n + 1) % 12 {
                    lean
                } else {
                    0.0
                }
            })
            .collect()
    };
    let mut seed = Vec::new();
    for n in 0..12_u32 {
        seed.extend(person(&format!("person-{}", n + 1), &axis(n, 0.0)));
    }
    // Each person's forty voices in one statement (the seed's statements
    // stay well within the memory ability's bounds).
    for n in 0..12_u32 {
        seed.push(statement(
            "WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r WHERE i < 40) INSERT INTO bot_person_embedding (person_id, kind, embedding) SELECT ?1, 'voice', ?2 FROM r",
            &json!([format!("person-{}", n + 1), Json::from(axis(n, 0.05)).to_string()]),
        ));
    }
    let inspect = [
        json!({"sql": "SELECT c.person_id, c.weight, (SELECT COUNT(*) FROM bot_person_embedding AS e WHERE e.person_id = c.person_id AND e.seq <= c.through_seq) AS folded, (SELECT COUNT(*) FROM bot_person_embedding AS e WHERE e.person_id = c.person_id) AS heard FROM bot_person_centroid AS c ORDER BY c.person_id"}),
    ];
    // Each step follows a pause of a tenth of a second (the harness's
    // bounds; with no input to take, steps then follow each other in one
    // call, and the harness's timer fires among them in deadline order):
    // by three and a half seconds some steps have committed.
    let finished = consolidate(SQLITE, &sqlite_grant(), &seed, &inspect, Some("3500ms"));
    assert_eq!(logged(&finished, "stopped").as_deref(), Some("stopped"));
    assert_eq!(logged(&finished, "consolidated"), None);
    let rows = &inspected(&finished)[0];
    assert_eq!(rows.len(), 12, "{rows:?}");
    // Every representative is as a committed step left it: its weight is
    // the voices up to its through_seq, never a fold half-written.
    for row in rows {
        assert_eq!(row[1], row[2], "{row:?}");
    }
    // Some steps committed, and the cancel came before the end: not every
    // voice is folded.
    assert!(rows.iter().any(|row| row[1] != json!(1)), "{rows:?}");
    assert!(rows.iter().any(|row| row[2] != row[3]), "{rows:?}");
}

/// The SQL of `<data id>` in a document (a quoted ECMAScript string).
fn data_sql(document: &Path, id: &str) -> String {
    let text = std::fs::read_to_string(document).expect("the document is readable");
    let start = text
        .find(&format!(r#"<data id="{id}" expr="'"#))
        .unwrap_or_else(|| panic!("{} declares {id}", document.display()))
        + format!(r#"<data id="{id}" expr="'"#).len();
    let end = start + text[start..].find(r#"'"/>"#).expect("the string closes");
    text[start..end]
        .replace("\\'", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[test]
fn the_conversations_people_statements_run_on_sqlite() {
    people_statements(SQLITE, &sqlite_grant());
}

#[test]
#[ignore = "needs a Postgres server: FLOPPY_PGLITE_SERVER or FLOPPY_POSTGRES_BIN"]
fn postgres_the_conversations_people_statements_run() {
    let server = Pglite::start().expect("a Postgres server is configured");
    people_statements(&server.database(), &support::pgsql_grant());
}

/// The conversation's own people statements (decision D205): numbers come
/// from the people sequence, never a count, so they are never taken twice;
/// a voice for a person no longer held is not stored; a new person's first
/// voice is their representative; identify reads one row per person.
fn people_statements(database: &str, grant: &[String]) {
    let conversation = workspace().join("abilities/conversation/conversation.scx");
    let sql = |id: &str| data_sql(&conversation, id);
    let seed = [
        statement(&sql("sequence_next"), &json!(["person", 2])),
        statement(&sql("sequence_next"), &json!(["person", 3])),
        statement(&sql("person_insert"), &json!(["person-1"])),
        statement(&sql("voice_insert"), &json!(["person-1", "voice", "[1,0]"])),
        statement(
            &sql("voice_insert"),
            &json!(["person-1", "voice", "[0.9,0.1]"]),
        ),
        statement(&sql("voice_insert"), &json!(["person-9", "voice", "[0,1]"])),
        statement(
            &sql("representative_insert"),
            &json!(["person-1", "voice", encode(&[1.0, 0.0])]),
        ),
    ];
    let inspect = [
        statement(&sql("sequence_next"), &json!(["person", 1])),
        statement(&sql("voice_select"), &json!(["voice"])),
        json!({"sql": "SELECT person_id, weight FROM bot_person_centroid"}),
        json!({"sql": "SELECT COUNT(*) FROM bot_person_embedding WHERE person_id = 'person-9'"}),
    ];
    // The consolidation in between finds person-1's second voice to fold.
    let finished = consolidate(database, grant, &seed, &inspect, None);
    assert_eq!(
        consolidated(&finished)["folded"],
        json!(1),
        "{}",
        finished.stderr
    );
    let read = inspected(&finished);
    // 2, then 5, then 6: each call takes its own numbers.
    assert_eq!(read[0], [vec![json!(6)]]);
    assert_eq!(read[1].len(), 1);
    assert_eq!(read[1][0][0], json!("person-1"));
    assert_eq!(read[2][0][0], json!("person-1"));
    assert_eq!(read[2][0][1], json!(2));
    assert_eq!(read[3], [vec![json!(0)]]);
}
