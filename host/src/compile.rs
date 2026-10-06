//! Validates and compiles one bot document to Machine IR, exactly as
//! `scxml-cli run` compiles its document: scxml.rs's validator (strict XSD
//! with the bound extension namespaces' schemas, their `env` slots, and the
//! bound services' slots and event contracts) and compiler, with the
//! `event:schema` files the document references read relative to it.
//!
//! No expression syntax checker is linked yet (the CLI's are private to it),
//! so data-model expressions are judged when the session evaluates them.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use scxml_compiler_core::{CompileInput, Compiler, MachineIr, ScriptResolution};
use scxml_event_schema::{
    EVENT_NAMESPACE_URI, Resources, SCHEMA_ATTRIBUTE, Source, resource_problem,
};
use scxml_validator_core::{
    Diagnostic, FileSchemaResolver, MemorySchemaResolver, SchemaResolveError, SchemaResolver,
    SchemaSource, Severity, Validator,
};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

use crate::packages::Packages;
use crate::telemetry::{Operation, observe};
use crate::{FailureKind, HostFailure};

/// The stack of the compilation thread (deep documents recurse in the
/// validator), as `scxml-cli`.
pub const COMPILE_STACK_BYTES: usize = 64 * 1024 * 1024;
/// The most diagnostics a [`HostFailure`] message lists.
const LISTED_DIAGNOSTICS: usize = 20;

/// One compiled document.
#[derive(Clone, Debug)]
pub struct Compiled {
    /// Its canonical Machine IR bytes.
    pub ir: Vec<u8>,
    /// Its `datamodel` attribute.
    pub data_model: Option<String>,
    /// The warnings and notes validation reported.
    pub diagnostics: Vec<Diagnostic>,
}

/// Answers bound extension namespaces with their packages' verified schemas
/// and every other namespace with the bundled schemas.
struct PackageSchemas {
    inner: BundledSchemaResolver<FileSchemaResolver>,
    packages: MemorySchemaResolver,
    bound: BTreeSet<String>,
}

impl SchemaResolver for PackageSchemas {
    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
        if self.bound.contains(namespace_uri) {
            self.packages.resolve(namespace_uri)
        } else {
            self.inner.resolve(namespace_uri)
        }
    }

    fn resolve_documents(
        &self,
        namespace_uri: &str,
    ) -> Result<Vec<SchemaSource>, SchemaResolveError> {
        if self.bound.contains(namespace_uri) {
            self.packages.resolve_documents(namespace_uri)
        } else {
            self.inner.resolve_documents(namespace_uri)
        }
    }
}

fn compile_error(message: impl Into<String>) -> HostFailure {
    HostFailure::new(FailureKind::Compile, message)
}

/// Reads and compiles the document at `path`.
///
/// # Errors
///
/// [`FailureKind::Compile`] for an unreadable or non-UTF-8 document, error
/// diagnostics (listed by code and message), or unusable Machine IR.
pub fn compile_file(path: &Path, packages: &Packages) -> Result<Compiled, HostFailure> {
    let bytes = fs::read(path)
        .map_err(|error| compile_error(format!("cannot read `{}`: {error}", path.display())))?;
    let schemas = load_event_schemas(path, &bytes);
    let source = String::from_utf8(bytes)
        .map_err(|_| compile_error(format!("`{}` is not UTF-8", path.display())))?;
    compile_source(&path.display().to_string(), source, schemas, packages)
}

/// Compiles `source` (named `name` in diagnostics) with the `event:schema`
/// files in `event_schemas`.
///
/// # Errors
///
/// As [`compile_file`], without the read.
pub fn compile_source(
    name: &str,
    source: String,
    event_schemas: Resources,
    packages: &Packages,
) -> Result<Compiled, HostFailure> {
    let span = observe(Operation::Compile);
    let result = compile_inner(name, source, event_schemas, packages);
    span.finish(&result);
    result
}

fn compile_inner(
    name: &str,
    source: String,
    event_schemas: Resources,
    packages: &Packages,
) -> Result<Compiled, HostFailure> {
    let mut memory = MemorySchemaResolver::new();
    let mut bound = BTreeSet::new();
    for extension in &packages.extensions {
        memory.insert(
            extension.manifest.namespace_uri.clone(),
            extension.schema.clone(),
        );
        bound.insert(extension.manifest.namespace_uri.clone());
    }
    let compiler = Compiler::new(
        Validator::new(
            PackageSchemas {
                inner: BundledSchemaResolver::new(FileSchemaResolver::new()),
                packages: memory,
                bound,
            },
            XsdSchemaBackend,
        )
        .with_extension_slots(packages.extension_slots())
        .with_services(packages.bound_services()),
    );
    let (manifests, package_schemas) = packages.compile_inputs();
    let input = CompileInput {
        source_name: name.to_owned(),
        source,
        packages: manifests,
        package_schemas,
        scripts: Vec::new(),
        script_resolution: ScriptResolution::Compile,
        service_types: Some(packages.bound_services().into_keys().collect()),
        event_schemas,
    };
    let output = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(COMPILE_STACK_BYTES)
            .spawn_scoped(scope, || compiler.compile(input))
            .map_err(|error| {
                compile_error(format!("cannot start the compilation thread: {error}"))
            })?
            .join()
            .map_err(|_| compile_error("the compilation thread failed"))
    })?;
    let diagnostics = output.validation.report.diagnostics().to_vec();
    let Some(artifact) = output.artifact else {
        let errors: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .collect();
        let mut message = format!("`{name}` does not compile: {} error(s)", errors.len());
        message.extend(
            errors
                .iter()
                .take(LISTED_DIAGNOSTICS)
                .map(|diagnostic| format!("\n  {}: {}", diagnostic.code, diagnostic.message)),
        );
        return Err(compile_error(message));
    };
    let ir = MachineIr::from_json(&artifact.bytes).map_err(|error| {
        compile_error(format!(
            "the compiler produced unusable Machine IR: {error}"
        ))
    })?;
    Ok(Compiled {
        ir: artifact.bytes,
        data_model: ir.data_model,
        diagnostics,
    })
}

/// Reads every schema file the document at `document` references with
/// `event:schema`, relative to its directory. A reference that is not a
/// checked relative path, resolves outside the directory, or is no readable
/// regular file is left out (the validator reports it, `SCXML088`).
#[must_use]
pub fn load_event_schemas(document: &Path, source: &[u8]) -> Resources {
    let mut resources = Resources::new();
    let references = references(source);
    if references.is_empty() {
        return resources;
    }
    let directory = document
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let Ok(root) = fs::canonicalize(&directory) else {
        return resources;
    };
    for reference in references {
        if let Some(bytes) = read_below(&root, &reference) {
            resources.insert(reference, bytes);
        }
    }
    resources
}

fn references(source: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(source) else {
        return Vec::new();
    };
    let Ok(document) = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        scxml_syntax::SourceId::new(1),
        "document",
        text,
    )) else {
        return Vec::new();
    };
    let mut references = Vec::new();
    document.root.visit(&mut |element| {
        for attribute in &element.attributes {
            if attribute.name.namespace_uri.as_deref() != Some(EVENT_NAMESPACE_URI)
                || attribute.name.local_name != SCHEMA_ATTRIBUTE
            {
                continue;
            }
            if let Source::Reference { resource, .. } = scxml_event_schema::source(&attribute.value)
                && resource_problem(resource).is_none()
            {
                references.push(resource.to_owned());
            }
        }
    });
    references.sort();
    references.dedup();
    references
}

fn read_below(root: &Path, reference: &str) -> Option<Vec<u8>> {
    let mut path = root.to_path_buf();
    for segment in reference.split('/') {
        path.push(percent_decode(segment)?);
    }
    let resolved = fs::canonicalize(&path).ok()?;
    if !resolved.starts_with(root) || !fs::metadata(&resolved).ok()?.is_file() {
        return None;
    }
    let mut file = fs::File::open(&resolved).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// One percent-decoded path segment; `None` when the encoding is invalid,
/// not UTF-8, or decodes to a separator, NUL, or a `.`/`..` segment.
#[must_use]
pub fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let hex = segment.get(index.checked_add(1)?..index.checked_add(3)?)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index = index.checked_add(3)?;
        } else {
            out.push(byte);
            index = index.checked_add(1)?;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    (!decoded.is_empty()
        && decoded != "."
        && decoded != ".."
        && !decoded.contains(['/', '\\', '\0']))
    .then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="idle"><state id="idle"><transition event="go" target="done"/></state><final id="done"/></scxml>"#;

    #[test]
    fn a_valid_document_compiles_to_machine_ir() {
        let compiled = compile_source(
            "trivial.scxml",
            DOCUMENT.to_owned(),
            Resources::new(),
            &Packages::default(),
        )
        .expect("the document compiles");
        assert_eq!(compiled.data_model.as_deref(), Some("null"));
        assert!(!compiled.ir.is_empty());
    }

    #[test]
    fn an_invalid_document_reports_its_diagnostic_codes() {
        let error = compile_source(
            "broken.scxml",
            DOCUMENT.replace("target=\"done\"", "target=\"missing\""),
            Resources::new(),
            &Packages::default(),
        )
        .expect_err("an unknown target does not compile");
        assert_eq!(error.kind(), FailureKind::Compile);
        assert!(error.message().contains("SCXML"), "{}", error.message());
    }

    #[test]
    fn percent_decoding_refuses_separators_and_dot_segments() {
        assert_eq!(percent_decode("a%20b").as_deref(), Some("a b"));
        for bad in ["", ".", "..", "%2e%2E", "a%2Fb", "%zz", "%2"] {
            assert_eq!(percent_decode(bad), None, "{bad}");
        }
    }
}
