//! Phase 7 spike "Spike `PGlite` In A Component": an invoked service
//! (`tests/spikes/pglite`) reaches `PGlite` over the Postgres wire protocol
//! on `wasi:sockets/tcp`, granted with `--wasi URI=inherit-network`; the
//! document is `tests/fixtures/phase7/spikes/pglite/root.scx`. The verdict
//! is recorded in `docs/plans/phase7-pglite-defaults.mdscript.md`.
//!
//! * `ungranted_network_is_refused`: without the grant the run is refused
//!   before the session starts.
//! * `pglite_answers_over_the_socket` (ignored): against a running `PGlite`
//!   server, `SELECT 1` returns its row, an error comes back as
//!   `spike.pglite.failed`, and the next invocation still answers. The test
//!   starts the server ([`support::Pglite`]): `FLOPPY_PGLITE_SERVER=<path
//!   to pglite-socket's dist/scripts/server.js> cargo test -p floppy-tests
//!   --test phase7_spikes -- --ignored`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use support::{Components, Run, labelled, logs, unique_scratch, workspace, write};

/// The spike service's type URI.
const URI: &str = "https://github.com/stateforward/floppy/services/pglite-spike";

/// The spike component: built for `wasm32-wasip2` and composed with the
/// sql service's deny-all `wasi:cli` provider, so it imports only
/// `wasi:sockets/tcp` and what it uses.
fn component() -> &'static PathBuf {
    static COMPONENT: OnceLock<PathBuf> = OnceLock::new();
    COMPONENT.get_or_init(|| {
        let target = workspace().join("target/spike-pglite");
        let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "build",
                "--locked",
                "--release",
                "--target",
                "wasm32-wasip2",
                "-p",
                "floppy-spike-pglite",
            ])
            .arg("--target-dir")
            .arg(&target)
            .current_dir(workspace())
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .status()
            .expect("cargo runs");
        assert!(status.success(), "the spike component must build");
        let deny = target.join("wasi-cli-deny.wasm");
        let status = Command::new("wasm-tools")
            .arg("parse")
            .arg(workspace().join("services/sql/wasi-cli-deny.wat"))
            .arg("-o")
            .arg(&deny)
            .status()
            .expect("wasm-tools runs");
        assert!(status.success(), "the deny provider must parse");
        let composed = target.join("floppy_spike_pglite.wasm");
        let status = Command::new("wasm-tools")
            .arg("compose")
            .arg(target.join("wasm32-wasip2/release/floppy_spike_pglite.wasm"))
            .arg("-d")
            .arg(&deny)
            .arg("-o")
            .arg(&composed)
            .stderr(Stdio::null())
            .status()
            .expect("wasm-tools runs");
        assert!(status.success(), "the spike component must compose");
        composed
    })
}

/// The flags binding the spike service.
fn package() -> Vec<String> {
    Components::package_args(
        "service",
        &workspace().join("tests/spikes/pglite/package/pglite.service.json"),
        URI,
        component(),
    )
}

/// The fixture document with `address` and `sql` substituted.
fn document(address: &str, sql: &str) -> PathBuf {
    let text =
        std::fs::read_to_string(workspace().join("tests/fixtures/phase7/spikes/pglite/root.scx"))
            .expect("the fixture is readable")
            .replace("'127.0.0.1:5432'", &format!("'{address}'"))
            .replace("'SELECT 1'", &format!("'{sql}'"));
    write(&unique_scratch("phase7-pglite"), "root.scx", &text)
}

fn run(address: &str, sql: &str, grant: bool) -> support::Finished {
    let mut args = package();
    if grant {
        args.extend(["--wasi".to_owned(), format!("{URI}=inherit-network")]);
    }
    args.push(document(address, sql).display().to_string());
    let mut run = Run::start(&args, &[]);
    run.end_input();
    run.finish(Duration::from_secs(120))
}

#[test]
fn ungranted_network_is_refused() {
    let finished = run("127.0.0.1:9", "SELECT 1", false);
    assert!(!finished.status.success(), "{}", finished.stderr);
    assert!(
        finished
            .stderr
            .contains("imports a WASI interface whose resource the run does not grant"),
        "{}",
        finished.stderr
    );
}

#[test]
#[ignore = "needs a PGlite server: FLOPPY_PGLITE_SERVER"]
fn pglite_answers_over_the_socket() {
    let server = support::Pglite::start().expect("FLOPPY_PGLITE_SERVER is set");
    let address = server.address.clone();
    let finished = run(&address, "SELECT 1", true);
    assert_eq!(
        logs(&finished.records),
        labelled(&[("result", r#"{"columns":["?column?"],"rows":[["1"]]}"#)]),
        "{}",
        finished.stderr
    );
    let finished = run(&address, "SELECT 1/0", true);
    assert_eq!(
        logs(&finished.records),
        labelled(&[("failed", "division by zero")]),
        "{}",
        finished.stderr
    );
    let finished = run(&address, "SELECT 2 AS two", true);
    assert_eq!(
        logs(&finished.records),
        labelled(&[("result", r#"{"columns":["two"],"rows":[["2"]]}"#)]),
        "{}",
        finished.stderr
    );
}
