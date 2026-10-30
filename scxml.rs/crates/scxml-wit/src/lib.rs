//! Canonical Component Model contracts and lightweight contract metadata.
//!
//! The WIT files under [`WIT_PATH`] are the single source of truth for every
//! independently deployed SCXML component. There are six packages:
//!
//! * `wit/scxml/scxml.wit` — `stateforward:scxml` (machine, data model, I/O
//!   processor, invoked service, extension component, Wasm script component,
//!   script bundle provider, session snapshot codec, repository resolver, and
//!   host worlds);
//! * `wit/scxml/deps/clock/clock.wit` — `stateforward:scxml-clock`;
//! * `wit/scxml/deps/wasi-clocks/clocks.wit` — `wasi:clocks@0.3.0`, vendored
//!   verbatim from WASI 0.3 (as `wasmtime-wasi` 49.0.2 ships it) for the
//!   invoked-service world's `wasi:clocks/monotonic-clock` import;
//! * `wit/scxml/deps/wasi-{http,cli,random,filesystem,sockets}/` —
//!   `wasi:http@0.3.0` and the packages its worlds name, vendored verbatim
//!   from WASI 0.3 (as `wasmtime-wasi-http` 49.0.2 ships them) for the
//!   resolver world's `wasi:http/client` import;
//! * `wit/basichttp/basichttp.wit` — `stateforward:scxml-basichttp` (the
//!   `BasicHTTP` Event I/O Processor and its narrow `http-transport` import);
//! * `wit/validator/validator.wit` — `stateforward:scxml-validator`;
//! * `wit/compiler/compiler.wit` — `stateforward:scxml-compiler`;
//! * `wit/reporter/reporter.wit` — `stateforward:scxml-reporter`.
//!
//! Each package documents, next to the governed item, its caller
//! preconditions, the exact numeric bounds the bundled components enforce,
//! lifecycle and ownership rules, postconditions, and the meaning of every
//! error code. Tests in this crate and in the implementing components fail
//! when a documented bound drifts from the enforcing constant.
//!
//! ## Versioning policy
//!
//! This policy is owned by the repository maintainers; it is the single
//! authoritative statement that the README and CHANGELOG point to.
//!
//! Workspace version `0.2.0` is the release this source builds. The table
//! records, for every versioned contract, the version the `v0.1.0` tag
//! shipped and the version this release ships; every contract whose shape
//! changed since `v0.1.0` has a new version:
//!
//! | Contract | `0.1.0` | `0.2.0` | Identified by |
//! |---|---|---|---|
//! | WIT packages (the six above) | `@0.1.0` | `@0.2.0` | package id in the `.wit` files and the constants below |
//! | Machine IR | `4` | `5` | `scxml_compiler_core::MACHINE_IR_VERSION` |
//! | Canonical report envelope | `scxml.report/v2` | `scxml.report/v3` | `scxml_reporting::ARTIFACT_VERSION` |
//! | Conformance report payload | `8` | `9` | `scxml_reporting::CONFORMANCE_REPORT_VERSION` |
//! | Session snapshot document | `scxml.session-snapshot/v1` | `scxml.session-snapshot/v2` | `scxml_snapshot::SESSION_SNAPSHOT_VERSION` |
//! | Session snapshot diff document | `scxml.session-snapshot-diff/v1` | `scxml.session-snapshot-diff/v2` | `scxml_snapshot::SESSION_SNAPSHOT_DIFF_VERSION` |
//! | Delayed-send wire record | `7` | `8` | `scxml_types::DELAYED_SEND_WIRE_VERSION` |
//! | Prepared conformance corpus manifest | `5` | `6` | `scxml_conformance::MANIFEST_VERSION` |
//! | IRP errata file | `1` | `1` | `scxml_conformance::ERRATA_VERSION` |
//! | Manual-equivalent oracle table | `2` | `4` | `scxml_conformance::manual_oracle::MANUAL_ORACLE_VERSION` |
//! | Live-session log | none | `scxml.live-log/v1` | `LOG_FORMAT` in `crates/scxml-cli/src/run.rs` |
//! | Extension package manifest | none | `stateforward.scxml.extension-package/v2` | `EXTENSION_PACKAGE_FORMAT` in `crates/scxml-validator-core/src/schema.rs` |
//! | Extension cache index entry | none | `stateforward.scxml.extension-cache/v1` | `ENTRY_FORMAT` in `crates/scxml-cli/src/repository.rs` |
//! | Service package manifest | none | `stateforward.scxml.service-package/v1` | `SERVICE_PACKAGE_FORMAT` in `crates/scxml-validator-core/src/service_package.rs` |
//! | Service cache index entry | none | `stateforward.scxml.service-cache/v1` | `SERVICE_ENTRY_FORMAT` in `crates/scxml-cli/src/repository.rs` |
//! | Data-model package manifest | none | `stateforward.scxml.data-model-package/v1` | `DATA_MODEL_PACKAGE_FORMAT` in `crates/scxml-validator-core/src/data_model_package.rs` |
//! | Data-model cache index entry | none | `stateforward.scxml.data-model-cache/v1` | `DATA_MODEL_ENTRY_FORMAT` in `crates/scxml-cli/src/repository.rs` |
//! | Rust crates and the browser npm package | `0.1.0` | `0.2.0` | `Cargo.toml` workspace version; `hosts/scxml-host-browser/package.json` |
//!
//! The `versioning_table_matches_the_governing_constants` test binds every
//! row of this table to the constant it names and fails on any drift.
//! A released version is never changed in place. Every decoder accepts
//! exactly its current version and fails closed on any other, so there is no
//! multi-version decoding and no silent migration.
//!
//! Change classes:
//!
//! * **Breaking** — removing or renaming an item; changing a function
//!   signature, record field, variant or enum case; adding a function to an
//!   existing interface (implementers break) or a case to a variant or enum
//!   (exhaustive consumers break); adding, removing, or changing any field
//!   of Machine IR, a report, a snapshot, or a wire record (their decoders
//!   reject unknown fields); or tightening a documented precondition or
//!   bound so previously valid input is rejected. A breaking change bumps
//!   the affected WIT package's minor version (`0.2.0` to `0.3.0`), the
//!   affected integer format version by one (for example Machine IR `4` to
//!   `5`), or the report envelope suffix (`scxml.report/v3` to `/v4`), and
//!   the workspace and npm package minor version.
//! * **Additive** — a new interface or world in a package, or a new
//!   independently versioned package, that changes no existing item. It
//!   bumps the package's patch version (`0.2.0` to `0.2.1`). Data formats
//!   have no additive class: every schema change is breaking.
//! * **Compatible** — a documentation clarification, a bug fix that keeps
//!   every contract, or relaxing a documented bound. It bumps the patch
//!   version of what it touches and never an integer format version.
//! * **Deprecation** — none before 1.0. The project is pre-1.0, so the
//!   hard-cutover rule applies: a replaced item, format version, or path is
//!   removed in the same change that introduces its replacement and bumps
//!   the version. No deprecated alias, compatibility shim, or dual-version
//!   decoder is kept unless a named released consumer requires it and the
//!   change documents the condition that retires it.
//!
//! Every version bump updates, in the same change, the constants that carry
//! it, the implementing components, the hosts, every extension package
//! manifest that pins a WIT world and version, the checked-in fixtures and
//! baselines that embed it, and the CHANGELOG.
//!
//! # Example
//!
//! Resolve a canonical WIT package from [`WIT_PATH`] (here with
//! `wit-parser`, as the contract tests do):
//!
//! ```
//! let mut resolve = wit_parser::Resolve::default();
//! let (package, _sources) = resolve
//!     .push_dir(std::path::Path::new(scxml_wit::WIT_PATH).join("compiler"))
//!     .map_err(|error| error.to_string())?;
//! assert_eq!(
//!     format!("{}", resolve.packages[package].name),
//!     scxml_wit::COMPILER_WIT_PACKAGE
//! );
//! # Ok::<(), String>(())
//! ```

#![forbid(unsafe_code)]

/// Core runtime WIT package name owned by this crate.
pub const WIT_PACKAGE: &str = "stateforward:scxml@0.2.0";
/// Deterministic clock and scheduler WIT package name.
pub const CLOCK_WIT_PACKAGE: &str = "stateforward:scxml-clock@0.2.0";
/// `BasicHTTP` Event I/O Processor WIT package name.
pub const BASICHTTP_WIT_PACKAGE: &str = "stateforward:scxml-basichttp@0.2.0";
/// Validator WIT package name.
pub const VALIDATOR_WIT_PACKAGE: &str = "stateforward:scxml-validator@0.2.0";
/// Compiler WIT package name.
pub const COMPILER_WIT_PACKAGE: &str = "stateforward:scxml-compiler@0.2.0";
/// Generic canonical artifact reporter WIT package name.
pub const REPORTER_WIT_PACKAGE: &str = "stateforward:scxml-reporter@0.2.0";

/// Relative root containing the versioned WIT packages.
pub const WIT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wit_package_resolves() {
        for (directory, package_name, world_name) in [
            ("scxml", "scxml", "machine"),
            ("scxml", "scxml", "machine-ir-provider"),
            ("scxml", "scxml", "script"),
            ("scxml", "scxml", "script-bundle-provider"),
            ("scxml", "scxml", "session-snapshot"),
            ("scxml", "scxml", "resolver"),
            ("scxml/deps/clock", "scxml-clock", "clock"),
            ("basichttp", "scxml-basichttp", "basichttp"),
            ("validator", "scxml-validator", "validator"),
            ("validator", "scxml-validator", "package-validator"),
            ("compiler", "scxml-compiler", "compiler"),
            ("reporter", "scxml-reporter", "reporter"),
        ] {
            let mut resolve = wit_parser::Resolve::default();
            let path = std::path::Path::new(WIT_PATH).join(directory);
            let (package, _sources) = resolve
                .push_dir(&path)
                .expect("canonical WIT package must parse and resolve");
            assert_eq!(resolve.packages[package].name.namespace, "stateforward");
            assert_eq!(resolve.packages[package].name.name, package_name);
            assert!(
                resolve.packages[package].worlds.contains_key(world_name),
                "missing WIT world {world_name}"
            );
        }

        let mut resolve = wit_parser::Resolve::default();
        let path = std::path::Path::new(WIT_PATH).join("scxml");
        let (package, _sources) = resolve
            .push_dir(&path)
            .expect("canonical SCXML WIT package must parse and resolve");
        assert!(
            resolve.packages[package]
                .worlds
                .contains_key("invoked-service"),
            "missing WIT world invoked-service"
        );
    }

    /// Every interface, world, function, and named type in every canonical
    /// package carries a doc comment; `use` aliases inherit their origin's.
    #[test]
    fn every_public_wit_item_is_documented() {
        let mut undocumented = Vec::new();
        for directory in ["scxml", "basichttp", "validator", "compiler", "reporter"] {
            let mut resolve = wit_parser::Resolve::default();
            resolve
                .push_dir(std::path::Path::new(WIT_PATH).join(directory))
                .expect("canonical WIT package must parse and resolve");
            // Only this repository's packages; a vendored standard package
            // (`wasi:clocks`) keeps its upstream text.
            for (_, package) in resolve
                .packages
                .iter()
                .filter(|(_, package)| package.name.namespace == "stateforward")
            {
                let package_name = package.name.to_string();
                if package.docs.contents.is_none() {
                    undocumented.push(format!("{package_name} package"));
                }
                for (name, world) in &package.worlds {
                    if resolve.worlds[*world].docs.contents.is_none() {
                        undocumented.push(format!("{package_name} world {name}"));
                    }
                }
                for (name, interface) in &package.interfaces {
                    let interface = &resolve.interfaces[*interface];
                    if interface.docs.contents.is_none() {
                        undocumented.push(format!("{package_name} interface {name}"));
                    }
                    for (function_name, function) in &interface.functions {
                        if function.docs.contents.is_none() {
                            undocumented.push(format!("{package_name} {name}.{function_name}"));
                        }
                    }
                    for (type_name, type_id) in &interface.types {
                        let definition = &resolve.types[*type_id];
                        let is_alias = matches!(definition.kind, wit_parser::TypeDefKind::Type(_));
                        if !is_alias && definition.docs.contents.is_none() {
                            undocumented.push(format!("{package_name} {name}.{type_name}"));
                        }
                    }
                }
            }
        }
        undocumented.sort();
        undocumented.dedup();
        assert!(
            undocumented.is_empty(),
            "undocumented WIT items: {undocumented:#?}"
        );
    }

    /// Joins WIT `///` doc lines into single-spaced prose for phrase checks.
    fn wit_doc_text(wit: &str) -> String {
        wit.lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("///"))
            .flat_map(str::split_whitespace)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Reads the literal value of `const NAME: usize = <digits>;` from source
    /// text, for components whose native build is too heavy to link here. A
    /// bound declared as `scxml_types::wasm_bound(<digits>)` reads as its
    /// digits: the value the wasm component enforces and its WIT documents
    /// (natively such a bound is unbounded).
    fn literal_usize_const(source: &str, name: &str) -> usize {
        let marker = format!("const {name}: usize = ");
        let value = source
            .lines()
            .find_map(|line| line.trim().split_once(&marker).map(|(_, value)| value))
            .and_then(|value| value.strip_suffix(';'))
            .unwrap_or_else(|| panic!("{name} must be a literal usize constant"));
        let value = value
            .strip_prefix("scxml_types::wasm_bound(")
            .and_then(|bound| bound.strip_suffix(')'))
            .unwrap_or(value);
        value
            .replace('_', "")
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a decimal literal"))
    }

    fn assert_documented(docs: &str, phrases: &[String], contract: &str) {
        for phrase in phrases {
            assert!(
                docs.contains(phrase.as_str()),
                "{contract} must document the enforced bound: {phrase:?}"
            );
        }
    }

    #[test]
    fn request_bounds_match_validator_core_constants() {
        use scxml_validator_core::{
            MAX_COMPONENT_METADATA_BYTES, MAX_COMPONENT_PACKAGE_COUNT, MAX_COMPONENT_REQUEST_BYTES,
            MAX_COMPONENT_SCHEMA_BYTES, MAX_COMPONENT_SCHEMA_COUNT, MAX_COMPONENT_SOURCE_BYTES,
            MAX_COMPONENT_TOTAL_METADATA_BYTES, MAX_COMPONENT_TOTAL_SCHEMA_BYTES,
        };
        let phrases = [
            format!("`source` at most {MAX_COMPONENT_SOURCE_BYTES} bytes"),
            format!(
                "at most {MAX_COMPONENT_SCHEMA_COUNT} `schemas` and {MAX_COMPONENT_PACKAGE_COUNT} `packages`"
            ),
            format!("each schema's `bytes` at most {MAX_COMPONENT_SCHEMA_BYTES} bytes"),
            format!("all schemas at most {MAX_COMPONENT_TOTAL_SCHEMA_BYTES} bytes together"),
            format!("non-empty and at most {MAX_COMPONENT_METADATA_BYTES} bytes"),
            format!("at most {MAX_COMPONENT_TOTAL_METADATA_BYTES} metadata bytes together"),
            format!("the whole request at most {MAX_COMPONENT_REQUEST_BYTES} bytes"),
        ];
        for (contract, wit) in [
            (
                "validator.wit",
                include_str!("../wit/validator/validator.wit"),
            ),
            ("compiler.wit", include_str!("../wit/compiler/compiler.wit")),
        ] {
            assert_documented(&wit_doc_text(wit), &phrases, contract);
        }
        let compiler = wit_doc_text(include_str!("../wit/compiler/compiler.wit"));
        assert_documented(
            &compiler,
            &[
                format!(
                    "other than `{}`",
                    scxml_validator_core::SUPPORTED_NAMESPACE_WIT_WORLD
                ),
                format!(
                    "WIT version other than `{}`",
                    scxml_validator_core::SUPPORTED_NAMESPACE_WIT_VERSION
                ),
                format!(
                    "its `bytes` at most {} bytes",
                    scxml_packager::MAX_TEMPLATE_BYTES
                ),
                format!(
                    "Exact template component bytes (at most {} bytes).",
                    scxml_packager::MAX_TEMPLATE_BYTES
                ),
            ],
            "compiler.wit",
        );
    }

    #[test]
    fn reporter_bounds_match_reporting_constants() {
        let docs = wit_doc_text(include_str!("../wit/reporter/reporter.wit"));
        assert_documented(
            &docs,
            &[
                format!(
                    "The artifact exceeded {} bytes",
                    scxml_reporting::MAX_CANONICAL_BYTES
                ),
                format!(
                    "rendering would exceed {} bytes",
                    scxml_reporting::MAX_RENDERED_BYTES
                ),
                format!("is not `{}`", scxml_reporting::ARTIFACT_VERSION),
                format!(
                    "is not `{}`, `{}`, or `{}`",
                    scxml_reporting::VALIDATOR_KIND,
                    scxml_reporting::COMPILER_KIND,
                    scxml_reporting::CONFORMANCE_KIND
                ),
            ],
            "reporter.wit",
        );
    }

    #[test]
    fn provider_bounds_match_bundled_component_constants() {
        // The XPath provider's evaluation fuel `scxml.wit` documents for the
        // wasm component; natively evaluation is unbounded.
        const DOCUMENTED_XPATH_FUEL: u64 = 1_000_000;
        assert_eq!(
            scxml_data_model_xpath::WASM_EVALUATION_FUEL,
            DOCUMENTED_XPATH_FUEL
        );
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(scxml_data_model_xpath::EVALUATION_FUEL, u64::MAX);
        let docs = wit_doc_text(include_str!("../wit/scxml/scxml.wit"));
        assert_documented(
            &docs,
            &[format!(
                "`type-uri` at most {} bytes",
                scxml_types::MAX_SERVICE_TYPE_URI_BYTES
            )],
            "scxml.wit invoke-request",
        );

        assert_documented(
            &docs,
            &[
                "Bundled-provider bounds: no text/JSON/XML value or location name has a byte bound.".to_owned(),
                format!(
                    "A variable whose rendered text is longer than {} bytes is `oversized` instead, in every provider.",
                    scxml_snapshot::MAX_VALUE_BYTES,
                ),
                format!(
                    "The XPath provider gives each expression evaluation {DOCUMENTED_XPATH_FUEL} fuel units; running out of fuel, like any other evaluator limit, fails that call with `execution` (SCXML `error.execution`) and changes no data."
                ),
            ],
            "scxml.wit data-model-api",
        );
    }

    #[test]
    fn bundled_data_model_bounds_fit_the_snapshot_document_bounds() {
        // Every state a bundled provider commits must encode as a session
        // snapshot: each provider reports a variable as `oversized` exactly when its
        // text exceeds the document's per-value bound.
        let ecmascript = include_str!("../../../components/scxml-data-model-ecmascript/src/lib.rs");
        let xpath = include_str!("../../../components/scxml-data-model-xpath/src/lib.rs");
        for (provider, source) in [("ecmascript", ecmascript), ("xpath", xpath)] {
            assert_eq!(
                literal_usize_const(source, "MAX_VALUE_BYTES"),
                scxml_snapshot::MAX_VALUE_BYTES,
                "{provider} oversized threshold"
            );
        }
    }

    #[test]
    fn public_errors_are_typed_and_compiler_decision_is_canonical() {
        let validator = include_str!("../wit/validator/validator.wit");
        let compiler = include_str!("../wit/compiler/compiler.wit");
        let reporter = include_str!("../wit/reporter/reporter.wit");

        for contract in [validator, compiler, reporter] {
            assert!(contract.contains("enum error-version { v1 }"));
            assert!(contract.contains("record component-error"));
            assert!(!contract.contains("result<list<u8>, string>"));
            assert!(!contract.contains("result<report-output, string>"));
            assert!(!contract.contains("result<compile-result, string>"));
        }

        assert!(compiler.contains("report-artifact: list<u8>"));
        assert!(compiler.contains("artifact-bytes: option<list<u8>>"));
        assert!(!compiler.contains("record diagnostic"));
        assert!(!compiler.contains("diagnostics: list<diagnostic>"));
        assert!(!compiler.contains("record artifact"));
        // Artifact metadata (media type, recipe) lives only in the canonical
        // report; the result record carries bytes. Script packages declare
        // their own input media type, which is request data, not output.
        let result = compiler
            .split_once("record compile-result {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(record, _)| record)
            .expect("canonical WIT must define compile-result");
        assert!(!result.contains("media-type: string"));
        assert!(!compiler.contains("package-recipe: string"));
    }

    #[test]
    fn runtime_errors_are_versioned_closed_and_payload_safe() {
        let contract = include_str!("../wit/scxml/scxml.wit");

        assert!(contract.contains("enum error-version { v1 }"));
        assert!(contract.contains("enum error-code"));
        assert!(contract.contains("duplicate-id"));
        assert!(contract.contains("unknown-id"));
        assert!(contract.contains("capacity-exhausted"));
        assert!(contract.contains("protocol"));
        assert!(contract.contains("record component-error"));
        assert!(contract.contains("detail: option<string>"));
        assert!(!contract.contains("variant component-error"));
        assert!(!contract.contains("invalid-input(string)"));
        assert!(!contract.contains("execution(string)"));
    }

    #[test]
    fn ir_source_contract_documents_the_enforced_machine_ir_bounds() {
        // `scxml.wit` documents the wasm machine's bound; natively it is unbounded.
        const DOCUMENTED_MAX_IR_BYTES: usize = 16 * 1024 * 1024;
        let contract = include_str!("../wit/scxml/scxml.wit");
        assert!(contract.contains("machine-ir: func() -> list<u8>;"));
        assert_eq!(
            scxml_compiler_core::MAX_IR_BYTES,
            scxml_types::wasm_bound(DOCUMENTED_MAX_IR_BYTES)
        );
        let docs = wit_doc_text(contract);
        assert_documented(
            &docs,
            &[format!(
                "rejects more than {DOCUMENTED_MAX_IR_BYTES} bytes with `payload-too-large`"
            )],
            "scxml.wit ir-source",
        );
        let machine = contract
            .split_once("world machine {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(world, _)| world)
            .expect("canonical WIT must define the machine world");
        assert!(machine.contains("import ir-source;"));
        let provider = contract
            .split_once("world machine-ir-provider {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(world, _)| world)
            .expect("canonical WIT must define the provider world");
        assert_eq!(provider.trim(), "export ir-source;");
    }

    #[test]
    fn script_contracts_document_the_enforced_script_bounds() {
        let contract = include_str!("../wit/scxml/scxml.wit");
        let docs = wit_doc_text(contract);
        assert_documented(
            &docs,
            &[
                format!(
                    "(bundled Wasmtime host: at most {} bytes)",
                    scxml_script::MAX_SCRIPT_COMPONENT_BYTES
                ),
                format!(
                    "`uri` non-empty and at most {} bytes",
                    scxml_script::MAX_SCRIPT_URI_BYTES
                ),
            ],
            "scxml.wit script contracts",
        );
        let script = contract
            .split_once("world script {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(world, _)| world)
            .expect("canonical WIT must define the script world");
        let mut imports = script
            .lines()
            .filter_map(|line| line.trim().strip_prefix("import "))
            .map(|name| format!("stateforward:scxml/{}@0.2.0", name.trim_end_matches(';')))
            .collect::<Vec<_>>();
        imports.push("stateforward:scxml/types@0.2.0".into());
        imports.sort();
        assert_eq!(imports, scxml_script::SCRIPT_ALLOWED_IMPORTS);
        assert!(script.contains("export script-api;"));
        assert_eq!(
            scxml_script::SCRIPT_WORLD,
            format!("stateforward:scxml/script@{}", "0.2.0")
        );
        let compiler = wit_doc_text(include_str!("../wit/compiler/compiler.wit"));
        assert_documented(
            &compiler,
            &[format!(
                "`scripts` holds at most {} packages and {} bytes together (each package's `bytes` at most {} bytes, its `uri` at most {} bytes",
                scxml_script::MAX_SCRIPT_PACKAGES,
                scxml_script::MAX_TOTAL_SCRIPT_BYTES,
                scxml_script::MAX_SCRIPT_COMPONENT_BYTES,
                scxml_script::MAX_SCRIPT_URI_BYTES
            )],
            "compiler.wit scripts",
        );
    }

    #[test]
    fn event_contract_carries_an_optional_send_id() {
        let contract = include_str!("../wit/scxml/scxml.wit");
        let event = contract
            .split_once("record event {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(event, _)| event)
            .expect("canonical WIT must define the event record");
        assert!(event.contains("send-id: option<string>"));
        assert!(!event.contains("send-id: string"));
    }

    #[test]
    fn data_model_evaluation_takes_an_optional_current_event() {
        // SCXML 1.0 section 5.10: `_event` is unbound before the first
        // event, so `<onentry>` run by `start` evaluates with `none`.
        let contract = include_str!("../wit/scxml/scxml.wit");
        let api = contract
            .split_once("interface data-model-api {")
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map(|(api, _)| api)
            .expect("canonical WIT must define data-model-api");
        for function in [
            "evaluate-condition: func(expression: string, event: option<event>)",
            "evaluate-value: func(expression: string, event: option<event>)",
            "execute: func(script: string, event: option<event>)",
        ] {
            assert!(api.contains(function), "{function}");
        }
        assert!(!api.contains("event: event)"));
    }

    #[test]
    fn telemetry_uses_a_closed_unindexed_numeric_measurement() {
        let contract = include_str!("../wit/scxml/scxml.wit");
        let clock = include_str!("../wit/scxml/deps/clock/clock.wit");

        assert!(contract.contains(
            "enum measurement-kind { effects, input-bytes, snapshot-bytes, queue-depth, fuel }"
        ));
        assert!(contract.contains(
            "observe: func(operation: operation, outcome: outcome, measurement: option<measurement>, context: option<trace-context>);"
        ));
        assert!(contract.contains(
            "event: func(operation: operation, event: signal-event, context: option<trace-context>);"
        ));
        assert!(clock.contains("enum measurement-kind { queue-depth }"));
        assert!(clock.contains(
            "observe: func(operation: operation, outcome: outcome, measurement: option<measurement>, context: option<trace-context>);"
        ));
        assert!(clock.contains(
            "event: func(operation: operation, event: signal-event, context: option<trace-context>);"
        ));
    }

    /// One versioning-table row whose version is the constant at `path`.
    fn version_row(
        contract: &str,
        released: &str,
        version: impl std::fmt::Display,
        path: &str,
    ) -> String {
        format!("{contract} | `{released}` | `{version}` | `{path}` |")
    }

    /// The format id the CLI source declares as `const {name}: &str`.
    fn cli_format(source: &str, name: &str) -> String {
        let prefix = format!("const {name}: &str = \"");
        let start = source.find(&prefix).map_or_else(
            || panic!("the CLI declares {name}"),
            |start| start + prefix.len(),
        );
        let rest = &source[start..];
        rest[..rest.find('"').expect("the format id is a string literal")].to_owned()
    }

    /// The WIT package version every package id carries.
    fn shared_wit_version() -> &'static str {
        let wit_version = WIT_PACKAGE
            .rsplit_once('@')
            .map(|(_, version)| version)
            .expect("the scxml WIT package id carries an @version suffix");
        for package in [
            WIT_PACKAGE,
            CLOCK_WIT_PACKAGE,
            BASICHTTP_WIT_PACKAGE,
            VALIDATOR_WIT_PACKAGE,
            COMPILER_WIT_PACKAGE,
            REPORTER_WIT_PACKAGE,
        ] {
            assert!(
                package.ends_with(&format!("@{wit_version}")),
                "{package} must share the documented WIT package version"
            );
        }
        wit_version
    }

    /// Binds every row of the crate-level versioning table to the constant
    /// it names, so the published table cannot drift from the code.
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one expected row per governed contract, in table order"
    )]
    fn versioning_table_matches_the_governing_constants() {
        let table: Vec<&str> = include_str!("lib.rs")
            .lines()
            .map(str::trim_start)
            .filter_map(|line| line.strip_prefix("//! | "))
            .skip(1)
            .collect();
        let crate_version = env!("CARGO_PKG_VERSION");
        assert!(
            include_str!("../../../hosts/scxml-host-browser/package.json")
                .contains(&format!("\"version\": \"{crate_version}\"")),
            "the browser npm package version must equal the workspace version"
        );
        let live_log = cli_format(include_str!("../../scxml-cli/src/run.rs"), "LOG_FORMAT");
        let extension_manifest = cli_format(
            include_str!("../../scxml-validator-core/src/schema.rs"),
            "EXTENSION_PACKAGE_FORMAT",
        );
        let extension_cache = cli_format(
            include_str!("../../scxml-cli/src/repository.rs"),
            "ENTRY_FORMAT",
        );
        let service_manifest = cli_format(
            include_str!("../../scxml-validator-core/src/service_package.rs"),
            "SERVICE_PACKAGE_FORMAT",
        );
        let service_cache = cli_format(
            include_str!("../../scxml-cli/src/repository.rs"),
            "SERVICE_ENTRY_FORMAT",
        );
        let data_model_manifest = cli_format(
            include_str!("../../scxml-validator-core/src/data_model_package.rs"),
            "DATA_MODEL_PACKAGE_FORMAT",
        );
        let data_model_cache = cli_format(
            include_str!("../../scxml-cli/src/repository.rs"),
            "DATA_MODEL_ENTRY_FORMAT",
        );
        // The `0.1.0` column is what the `v0.1.0` tag shipped; it never
        // changes.
        let expected = [
            format!(
                "WIT packages (the six above) | `@0.1.0` | `@{}` | package id in the `.wit` files and the constants below |",
                shared_wit_version()
            ),
            version_row(
                "Machine IR",
                "4",
                scxml_compiler_core::MACHINE_IR_VERSION,
                "scxml_compiler_core::MACHINE_IR_VERSION",
            ),
            version_row(
                "Canonical report envelope",
                "scxml.report/v2",
                scxml_reporting::ARTIFACT_VERSION,
                "scxml_reporting::ARTIFACT_VERSION",
            ),
            version_row(
                "Conformance report payload",
                "8",
                scxml_reporting::CONFORMANCE_REPORT_VERSION,
                "scxml_reporting::CONFORMANCE_REPORT_VERSION",
            ),
            version_row(
                "Session snapshot document",
                "scxml.session-snapshot/v1",
                scxml_snapshot::SESSION_SNAPSHOT_VERSION,
                "scxml_snapshot::SESSION_SNAPSHOT_VERSION",
            ),
            version_row(
                "Session snapshot diff document",
                "scxml.session-snapshot-diff/v1",
                scxml_snapshot::SESSION_SNAPSHOT_DIFF_VERSION,
                "scxml_snapshot::SESSION_SNAPSHOT_DIFF_VERSION",
            ),
            version_row(
                "Delayed-send wire record",
                "7",
                scxml_types::DELAYED_SEND_WIRE_VERSION,
                "scxml_types::DELAYED_SEND_WIRE_VERSION",
            ),
            version_row(
                "Prepared conformance corpus manifest",
                "5",
                scxml_conformance::MANIFEST_VERSION,
                "scxml_conformance::MANIFEST_VERSION",
            ),
            version_row(
                "IRP errata file",
                "1",
                scxml_conformance::ERRATA_VERSION,
                "scxml_conformance::ERRATA_VERSION",
            ),
            version_row(
                "Manual-equivalent oracle table",
                "2",
                scxml_conformance::manual_oracle::MANUAL_ORACLE_VERSION,
                "scxml_conformance::manual_oracle::MANUAL_ORACLE_VERSION",
            ),
            format!(
                "Live-session log | none | `{live_log}` | `LOG_FORMAT` in `crates/scxml-cli/src/run.rs` |"
            ),
            format!(
                "Extension package manifest | none | `{extension_manifest}` | `EXTENSION_PACKAGE_FORMAT` in `crates/scxml-validator-core/src/schema.rs` |"
            ),
            format!(
                "Extension cache index entry | none | `{extension_cache}` | `ENTRY_FORMAT` in `crates/scxml-cli/src/repository.rs` |"
            ),
            format!(
                "Service package manifest | none | `{service_manifest}` | `SERVICE_PACKAGE_FORMAT` in `crates/scxml-validator-core/src/service_package.rs` |"
            ),
            format!(
                "Service cache index entry | none | `{service_cache}` | `SERVICE_ENTRY_FORMAT` in `crates/scxml-cli/src/repository.rs` |"
            ),
            format!(
                "Data-model package manifest | none | `{data_model_manifest}` | `DATA_MODEL_PACKAGE_FORMAT` in `crates/scxml-validator-core/src/data_model_package.rs` |"
            ),
            format!(
                "Data-model cache index entry | none | `{data_model_cache}` | `DATA_MODEL_ENTRY_FORMAT` in `crates/scxml-cli/src/repository.rs` |"
            ),
            format!(
                "Rust crates and the browser npm package | `0.1.0` | `{crate_version}` | `Cargo.toml` workspace version; `hosts/scxml-host-browser/package.json` |"
            ),
        ];
        assert_eq!(
            table, expected,
            "the versioning table must list exactly the current version of every governed contract"
        );
    }
}
