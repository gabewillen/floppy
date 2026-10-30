//! `scx snapshot show|check|diff|run` against real documents: output
//! forms and the documented exit codes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use scxml_snapshot::{
    Capture, ComponentIdentity, ComponentRole, DataModelCapture, EventType, MachineCapture,
    Options, SessionCapture, Status, System, TransitionCapture, TransitionType, Value, Variable,
    VariableValue, encode,
};

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/cli-snapshot")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory must be creatable");
    directory
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scx"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("scx must run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn capture(state: &str, x: &str, now_ms: u64) -> Capture {
    let digest = |fill: char| std::iter::repeat_n(fill, 64).collect::<String>();
    Capture {
        components: [
            ComponentRole::Clock,
            ComponentRole::DataModelEcmascript,
            ComponentRole::IoProcessorScxml,
            ComponentRole::Machine,
        ]
        .into_iter()
        .map(|role| ComponentIdentity {
            role,
            sha256: digest('b'),
        })
        .collect(),
        now_ms,
        sessions: vec![SessionCapture {
            parent: None,
            session_id: "session-1".into(),
            ir_sha256: digest('a'),
            data_model_name: Some("ecmascript".into()),
            machine: MachineCapture {
                status: Status::Running,
                name: Some("main".into()),
                atomic: vec![state.into()],
                configuration: vec![state.into()],
                internal: vec![],
                history: vec![],
                bound_states: vec![],
                invocations: vec![],
                invoked_by: None,
                send_counter: 0,
                invoke_counter: 0,
                transitions: vec![TransitionCapture {
                    source: state.into(),
                    index: 0,
                    events: vec!["go".into()],
                    targets: vec!["s1".into()],
                    transition_type: TransitionType::External,
                    cond: None,
                    domain: None,
                    exit: vec![state.into()],
                    entry: vec!["s1".into()],
                    event_schema: None,
                }],
            },
            data_model: Some(DataModelCapture {
                system: System {
                    sessionid: "session-1".into(),
                    name: Some("main".into()),
                    ioprocessors: Value::JsonObject("{}".into()),
                },
                variables: vec![Variable {
                    name: "x".into(),
                    value: VariableValue::Value(Value::Number(x.into())),
                }],
            }),
            pending_sends: vec![],
            external: vec![scxml_snapshot::Event {
                name: "go".into(),
                event_type: EventType::External,
                origin: None,
                origintype: None,
                sendid: None,
                invokeid: None,
                data: Value::Absent,
                raw: None,
                trace_context: None,
            }],
            finished: false,
        }],
    }
}

fn write_documents(directory: &Path) -> (String, String) {
    let before = directory.join("0000.snapshot.json");
    let after = directory.join("0001.snapshot.json");
    let options = Options::default();
    fs::write(
        &before,
        encode(&capture("s0", "1", 0), &options).expect("encodes"),
    )
    .expect("writable");
    fs::write(
        &after,
        encode(&capture("s1", "2", 100), &options).expect("encodes"),
    )
    .expect("writable");
    (before.display().to_string(), after.display().to_string())
}

#[test]
fn show_check_and_diff_follow_the_documented_exit_codes() {
    let directory = scratch("exit-codes");
    let (before, after) = write_documents(&directory);

    let show = cli(&["snapshot", "show", &before]);
    assert_eq!(show.status.code(), Some(0), "{}", text(&show.stderr));
    let shown = text(&show.stdout);
    assert!(shown.contains("session root `session-1` name `main` datamodel ecmascript"));
    assert!(shown.contains("       x = 1"));
    assert!(shown.contains("       s0[0] on go -> [s1] (external)"));
    let json = cli(&["snapshot", "show", &before, "--format", "json"]);
    assert_eq!(json.stdout, fs::read(&before).expect("readable"));

    let check = cli(&["snapshot", "check", &before]);
    assert_eq!(check.status.code(), Some(0));
    assert!(text(&check.stdout).starts_with("ok: "));
    let check_json = cli(&["snapshot", "check", &before, "--format", "json"]);
    assert_eq!(text(&check_json.stdout), r#"{"issues":[],"valid":true}"#);

    let same = cli(&["snapshot", "diff", &before, &before]);
    assert_eq!(same.status.code(), Some(0));
    assert!(text(&same.stdout).contains("= identical: no difference"));

    let diff = cli(&["snapshot", "diff", &before, &after]);
    assert_eq!(diff.status.code(), Some(1), "{}", text(&diff.stderr));
    assert!(diff.stderr.is_empty(), "a difference is not an error");
    let rendered = text(&diff.stdout);
    assert!(rendered.starts_with(&format!("snapshot diff: {before} → {after}\n")));
    assert!(rendered.contains("= clock: 0ms -> 100ms"));
    assert!(rendered.contains("       - s0\n       + s1"));
    assert!(rendered.contains("       ~ x: 1 -> 2"));
    let diff_json = cli(&["snapshot", "diff", &before, &after, "--format", "json"]);
    assert_eq!(diff_json.status.code(), Some(1));
    assert!(
        text(&diff_json.stdout)
            .starts_with(r#"{"version":"scxml.session-snapshot-diff/v2","before":{"sha256":""#)
    );
    fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn damaged_documents_are_located_snap_diagnostics() {
    let directory = scratch("damaged");
    let (before, _) = write_documents(&directory);
    let damaged = directory.join("damaged.json");
    let bytes = fs::read(&before).expect("readable");
    let edited = text(&bytes).replacen("\"go\"", "\"gp\"", 1);
    fs::write(&damaged, edited).expect("writable");
    let damaged = damaged.display().to_string();

    let check = cli(&["snapshot", "check", &damaged]);
    assert_eq!(check.status.code(), Some(1));
    let stderr = text(&check.stderr);
    assert!(stderr.starts_with(&format!(
        "error[SNAP-integrity-mismatch]: `sha256` does not match the canonical bytes of `snapshot`\n  --> {damaged}\n   = pointer: /sha256\n   = help: "
    )), "{stderr}");
    assert!(stderr.contains("try `scx --explain SNAP-integrity-mismatch`"));
    let check_json = cli(&["snapshot", "check", &damaged, "--format", "json"]);
    assert_eq!(
        text(&check_json.stdout),
        r#"{"issues":[{"code":"SNAP-integrity-mismatch","pointer":"/sha256"}],"valid":false}"#
    );

    let show = cli(&["snapshot", "show", &damaged]);
    assert_eq!(show.status.code(), Some(2));
    assert!(text(&show.stderr).starts_with("error[SNAP-integrity-mismatch]"));
    let diff = cli(&["snapshot", "diff", &before, &damaged]);
    assert_eq!(diff.status.code(), Some(2));

    let explain = cli(&["--explain", "SNAP-not-stable"]);
    assert_eq!(explain.status.code(), Some(0));
    assert!(text(&explain.stdout).starts_with("SNAP-not-stable (error · snapshot)"));

    let missing = cli(&["snapshot", "check", "missing.snapshot.json"]);
    assert_eq!(missing.status.code(), Some(2));
    fs::remove_dir_all(&directory).expect("cleanup");
}

#[cfg(not(feature = "wasmtime"))]
#[test]
fn run_without_wasmtime_fails_closed() {
    let run = cli(&[
        "snapshot",
        "run",
        "machine.scxml",
        "--components",
        "dist",
        "--components-manifest-sha256",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ]);
    assert_eq!(run.status.code(), Some(2));
    assert!(text(&run.stderr).starts_with("error: [wasmtime-disabled]"));
}

/// `show` and `diff` never write a document's control or bidi characters to
/// the terminal raw (see crates/scxml-snapshot/tests/terminal.rs).
#[test]
fn show_and_diff_escape_terminal_controls() {
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scxml-snapshot/tests/fixtures/terminal");
    let xml = fixtures.join("ansi_xml.json").display().to_string();
    let sysname = fixtures.join("ansi_sysname.json").display().to_string();
    let raw = |bytes: &[u8]| {
        text(bytes).chars().any(|character| {
            (character.is_control() && character != '\n')
                || matches!(character, '\u{0080}'..='\u{009f}' | '\u{202a}'..='\u{202e}')
        })
    };
    for args in [
        vec!["snapshot", "show", xml.as_str()],
        vec!["snapshot", "show", sysname.as_str()],
        vec!["snapshot", "show", xml.as_str(), "--color", "never"],
        vec!["snapshot", "diff", sysname.as_str(), xml.as_str()],
    ] {
        let output = cli(&args);
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "{args:?}: {}",
            text(&output.stderr)
        );
        assert!(!raw(&output.stdout), "{args:?}: {}", text(&output.stdout));
        assert!(text(&output.stdout).contains(r"\u{001B}"), "{args:?}");
    }
}

/// Every shape failure names where it is: the offending value, or for an
/// unknown key the object holding it; malformed JSON names line and column.
#[test]
fn schema_violations_carry_a_pointer_and_malformed_json_a_position() {
    let directory = scratch("pointers");
    let (before, _) = write_documents(&directory);
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(&before).expect("readable")).expect("json");
    let edit = |name: &str, change: &dyn Fn(&mut serde_json::Value)| {
        let mut document = valid.clone();
        change(&mut document);
        let path = directory.join(name);
        fs::write(&path, serde_json::to_vec(&document).expect("json")).expect("writable");
        path.display().to_string()
    };
    let cases = [
        (
            edit("unknown.json", &|d| {
                d["snapshot"]["sessions"][0]["bogus\u{1b}"] = 1.into();
            }),
            "/snapshot/sessions/0",
        ),
        (
            edit("float.json", &|d| {
                d["snapshot"]["clock"]["now_ms"] = 1.5.into();
            }),
            "/snapshot/clock/now_ms",
        ),
        (
            edit("negative.json", &|d| {
                d["snapshot"]["sessions"][0]["counters"]["send"] = (-1).into();
            }),
            "/snapshot/sessions/0/counters/send",
        ),
        (
            edit("huge.json", &|d| {
                d["snapshot"]["clock"]["now_ms"] = serde_json::Value::from(1e20);
            }),
            "/snapshot/clock/now_ms",
        ),
        (
            edit("status.json", &|d| {
                d["snapshot"]["sessions"][0]["status"] = "paused".into();
            }),
            "/snapshot/sessions/0/status",
        ),
        (
            edit("variable.json", &|d| {
                d["snapshot"]["sessions"][0]["data_model"][0]["name"] = 7.into();
            }),
            "/snapshot/sessions/0/data_model/0/name",
        ),
    ];
    for (path, pointer) in &cases {
        let json = cli(&["snapshot", "check", path, "--format", "json"]);
        assert_eq!(json.status.code(), Some(1), "{path}");
        assert_eq!(
            text(&json.stdout),
            format!(
                r#"{{"issues":[{{"code":"SNAP-schema-violation","pointer":"{pointer}"}}],"valid":false}}"#
            ),
            "{path}"
        );
        let human = cli(&["snapshot", "check", path]);
        assert!(
            text(&human.stderr).contains(&format!("\n   = pointer: {pointer}\n")),
            "{path}: {}",
            text(&human.stderr)
        );
    }

    let array = directory.join("array.json");
    fs::write(&array, "[]").expect("writable");
    let array = array.display().to_string();
    let json = cli(&["snapshot", "check", &array, "--format", "json"]);
    assert_eq!(
        text(&json.stdout),
        r#"{"issues":[{"code":"SNAP-schema-violation","pointer":""}],"valid":false}"#
    );
    let human = cli(&["snapshot", "check", &array]);
    assert!(
        text(&human.stderr).contains("\n   = pointer: \"\" (the document root)\n"),
        "{}",
        text(&human.stderr)
    );
    fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn malformed_json_names_its_line_and_column() {
    let directory = scratch("malformed");
    let malformed = directory.join("malformed.json");
    fs::write(&malformed, "{\"version\":\n  tru}").expect("writable");
    let malformed = malformed.display().to_string();
    let human = cli(&["snapshot", "check", &malformed]);
    assert_eq!(human.status.code(), Some(1));
    assert!(
        text(&human.stderr).starts_with(&format!(
            "error[SNAP-malformed-json]: the snapshot document is not JSON\n  --> {malformed}:2:6\n"
        )),
        "{}",
        text(&human.stderr)
    );
    let json = cli(&["snapshot", "check", &malformed, "--format", "json"]);
    assert_eq!(
        text(&json.stdout),
        r#"{"issues":[{"code":"SNAP-malformed-json","column":6,"line":2}],"valid":false}"#
    );
    let show = cli(&["snapshot", "show", &malformed]);
    assert_eq!(show.status.code(), Some(2));
    assert!(text(&show.stderr).contains(&format!("  --> {malformed}:2:6\n")));
    fs::remove_dir_all(&directory).expect("cleanup");
}
