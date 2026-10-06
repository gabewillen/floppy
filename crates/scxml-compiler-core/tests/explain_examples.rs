//! The `--explain` examples of the compile-only `COMP` codes, compiled as
//! `scx compile` compiles them: a document example is shown as XML
//! only because this test checks it, so the erroneous one must emit its
//! code and the corrected one must compile with no diagnostic at all.

#[allow(
    dead_code,
    reason = "the shared helpers also serve validate_covers_lowering.rs"
)]
mod common;

use scxml_reporting::explain::{self, CODES, is_document_example};

#[test]
fn compile_only_examples_are_checked_or_prose() {
    let mut checked = 0_usize;
    for entry in CODES.iter().filter(|entry| entry.code.starts_with("COMP")) {
        assert!(
            explain::examples_are_checked(entry),
            "{}: compile-only examples are checked here",
            entry.code
        );
        let rendered = explain::explanation(entry);
        for example in [entry.bad, entry.fixed] {
            let fenced_xml = rendered.contains(&format!("```xml\n{example}\n```"));
            assert_eq!(
                fenced_xml,
                is_document_example(example),
                "{}: exactly the document examples are XML",
                entry.code
            );
        }
        if is_document_example(entry.bad) {
            checked += 1;
            let output = common::compile(entry.bad);
            let codes: Vec<&str> = output
                .validation
                .report
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect();
            assert!(
                codes.contains(&entry.code),
                "{}: the erroneous example must emit its code, got {codes:?}",
                entry.code
            );
            assert!(output.artifact.is_none(), "{}", entry.code);
        }
        if is_document_example(entry.fixed) {
            checked += 1;
            let output = common::compile(entry.fixed);
            let remaining: Vec<(&str, &str)> = output
                .validation
                .report
                .diagnostics()
                .iter()
                .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.message.as_str()))
                .collect();
            assert!(
                remaining.is_empty() && output.artifact.is_some(),
                "{}: the corrected example must compile cleanly, got {remaining:?}",
                entry.code
            );
        }
    }
    // COMP002's two documents and the erroneous one of COMP003.
    assert_eq!(checked, 3, "compile-checked document examples");
}

#[test]
fn comp001_example_is_the_encoded_ir_bound_that_validation_cannot_see() {
    let entry = explain::lookup("COMP001").expect("COMP001 is documented");
    assert!(!is_document_example(entry.bad) && !is_document_example(entry.fixed));
    assert!(entry.bad.contains("16 MiB"), "{}", entry.bad);
}
