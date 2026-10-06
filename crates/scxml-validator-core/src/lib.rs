//! Native SCXML validation orchestration without runtime or Wasmtime coupling.
//!
//! Parsing and semantic validation are lightweight. XSD support is supplied by
//! an implementation of [`SchemaValidator`], keeping backend parser types out
//! of this public contract and out of engine/runtime dependency graphs.
//!
//! # Example
//!
//! Validate SCXML source with schema validation switched off (a real caller
//! supplies an XSD backend such as `scxml-validator-xsd`):
//!
//! ```
//! use scxml_syntax::{Document, SourceId};
//! use scxml_validator_core::{
//!     MemorySchemaResolver, Report, SchemaBackendError, SchemaMode, SchemaSource,
//!     SchemaValidator, Validator,
//! };
//!
//! #[derive(Debug)]
//! struct NoSchemaBackend;
//!
//! impl SchemaValidator for NoSchemaBackend {
//!     fn validate(
//!         &self,
//!         _document: &Document,
//!         _schemas: &[SchemaSource],
//!     ) -> Result<Report, SchemaBackendError> {
//!         Ok(Report::new())
//!     }
//! }
//!
//! let validator = Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
//!     .with_schema_mode(SchemaMode::Off);
//! let valid = validator.validate_source(
//!     SourceId::new(1),
//!     "ok.scxml",
//!     r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">
//!   <state id="a"><transition event="go" target="b"/></state>
//!   <final id="b"/>
//! </scxml>"#,
//! );
//! assert!(!valid.report.has_errors());
//!
//! // An unknown transition target is a located, coded error diagnostic.
//! let invalid = validator.validate_source(
//!     SourceId::new(2),
//!     "bad.scxml",
//!     r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="a">
//!   <state id="a"><transition event="go" target="missing"/></state>
//! </scxml>"#,
//! );
//! assert!(invalid.report.has_errors());
//! assert!(invalid.report.diagnostics().iter().all(|diagnostic| !diagnostic.code.is_empty()));
//! ```

#![forbid(unsafe_code)]

mod datamodel_check;
mod diagnostic;
mod env_attributes;
mod event_attributes;
mod graph;
mod invoke_types;
mod lowering;
mod refine;
mod request;
mod schema;
mod schema_values;
mod script_check;
mod semantic;
mod service_package;
pub mod spans;
mod text_direction;
mod xml_error;

/// Drift tests binding the canonical explain table in `scxml-reporting` to
/// the codes this crate's producers emit and to `docs/validator-rules.md`.
#[cfg(test)]
mod explain {
    pub(crate) use scxml_reporting::explain::*;
    mod tests;
}

pub use datamodel_check::{
    DEFAULT_DATA_MODEL, EffectCause, EffectCheck, EffectFinding, EffectFragment, EffectKind,
    EffectRole, EffectSpan, ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker,
    ExternalScripts, MAX_CHECKED_EXPRESSION_BYTES, MAX_CHECKED_EXPRESSIONS,
    MAX_ISSUES_PER_EXPRESSION, SyntaxIssue, report_unchecked_expressions, validate_expressions,
    validate_expressions_with,
};
pub use diagnostic::{Diagnostic, Fix, FixEdit, Label, Report, Severity, line_column_span};
pub use env_attributes::{MAX_ENV_NAME_BYTES, env_attribute_diagnostics, env_slot, is_env_name};
pub use event_attributes::event_attribute_diagnostics;
pub use graph::{
    GRAPH_WORK_PER_ITEM, MAX_GRAPH_NODES, MAX_GRAPH_TARGETS, MAX_GRAPH_TRANSITIONS, MAX_GRAPH_WORK,
    graph_work_budget,
};
pub use lowering::bounds as lowering_bounds;
pub use request::{
    ComponentRequestBoundary, MAX_COMPONENT_METADATA_BYTES, MAX_COMPONENT_PACKAGE_COUNT,
    MAX_COMPONENT_REQUEST_BYTES, MAX_COMPONENT_SCHEMA_BYTES, MAX_COMPONENT_SCHEMA_COUNT,
    MAX_COMPONENT_SOURCE_BYTES, MAX_COMPONENT_TOTAL_METADATA_BYTES,
    MAX_COMPONENT_TOTAL_SCHEMA_BYTES, PackageIdentity, RequestValidationError,
};
pub use schema::{
    CachingSchemaResolver, EXTENSION_PACKAGE_FORMAT, ExtensionPackageManifest, FileSchemaResolver,
    MAX_ENV_SLOT_BYTES, MAX_EXTENSION_PACKAGE_FIELD_BYTES, MAX_EXTENSION_PACKAGE_LIST_ITEMS,
    MAX_HTTP_ORIGIN_BYTES, MemorySchemaResolver, MissingSchemaPolicy,
    SUPPORTED_NAMESPACE_WIT_VERSION, SUPPORTED_NAMESPACE_WIT_WORLD, SchemaBackendError,
    SchemaResolveError, SchemaResolver, SchemaSource, SchemaValidator, is_env_slot, is_http_origin,
};
pub use schema_values::{COLLAPSED_ATTRIBUTES, collapses_whitespace};
pub use scxml_event_schema::Resources as EventSchemaResources;
pub use scxml_reporting::{Applicability, ArtifactEnvelope, DiagnosticCategory};
use scxml_syntax::telemetry::{
    ErrorCategory, FunctionSpan, Module, Operation, SignalEvent, observe,
};
use scxml_syntax::{
    Document, SourceId, SourceText, XmlLimits, parse_xml_with_limits, validate_document_limits,
};
pub use scxml_types::{ENV_NAMESPACE_URI, EVENT_NAMESPACE_URI};
pub use semantic::{MAX_NESTED_SCXML_DEPTH, Rule, SCXML_NAMESPACE, validate_semantics};
pub use service_package::{
    SERVICE_PACKAGE_FIELDS, SERVICE_PACKAGE_FORMAT, SUPPORTED_SERVICE_WIT_VERSION,
    SUPPORTED_SERVICE_WIT_WORLD, ServicePackageManifest,
};
pub use xml_error::{xml_error_code, xml_error_diagnostic};

/// Schema execution mode for one validation run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SchemaMode {
    /// Run XML and semantic validation only.
    Off,
    /// Run XSD for registered namespaces and skip missing schemas.
    Lax,
    /// Require an XSD for every used element/attribute namespace.
    #[default]
    Strict,
}

/// First-class validation pipeline with independently testable phases.
#[derive(Debug)]
pub struct Validator<R, B> {
    resolver: R,
    backend: B,
    schema_mode: SchemaMode,
    xml_limits: XmlLimits,
    /// Linked parse-only checkers, at most one per data model.
    expression_checkers: Vec<std::sync::Arc<dyn ExpressionSyntaxChecker>>,
    /// The `env` slots each bound extension namespace's package declares
    /// (`SCXML083`); `None` judges no slot.
    extension_slots: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// The `env` slots of each invoked service the caller's service packages
    /// bind, by exact type URI (`SCXML094`, `SCXML083` on `<invoke>`);
    /// `None` judges no binding.
    services: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// The bytes of each schema file an `event:schema` references, keyed by
    /// the reference's resource part as authored.
    event_schema_resources: EventSchemaResources,
}

impl<R, B> Validator<R, B> {
    /// Creates a strict schema-enabled validator.
    #[must_use]
    pub const fn new(resolver: R, backend: B) -> Self {
        Self {
            resolver,
            backend,
            schema_mode: SchemaMode::Strict,
            xml_limits: XmlLimits::DEFAULT,
            expression_checkers: Vec::new(),
            extension_slots: None,
            services: None,
            event_schema_resources: std::collections::BTreeMap::new(),
        }
    }

    /// The bytes of each schema file the document's `event:schema`
    /// attributes reference, keyed by the reference's resource part exactly
    /// as authored (the value before `#`, trimmed of XML white space). The
    /// caller reads them, relative to the document; a reference with no
    /// entry is `SCXML088`. The validator itself performs no I/O.
    #[must_use]
    pub fn with_event_schema_resources(mut self, resources: EventSchemaResources) -> Self {
        self.event_schema_resources = resources;
        self
    }

    /// The schema-file bytes supplied with
    /// [`Self::with_event_schema_resources`].
    #[must_use]
    pub const fn event_schema_resources(&self) -> &EventSchemaResources {
        &self.event_schema_resources
    }

    /// The `env` slots each bound extension namespace's package declares
    /// (exact namespace URI to `env-slots`): an `env` attribute on an element
    /// of one of these namespaces must name a declared slot (`SCXML083`).
    /// Elements of other namespaces are not judged.
    #[must_use]
    pub fn with_extension_slots(
        mut self,
        slots: std::collections::BTreeMap<String, Vec<String>>,
    ) -> Self {
        self.extension_slots = Some(slots);
        self
    }

    /// The `env` slots of each invoked service the caller's service packages
    /// bind, by exact type URI: a literal `<invoke type>` naming a service
    /// none of them binds is `SCXML094`, and an `env` attribute on an
    /// `<invoke>` of a bound service must name a slot its package declares
    /// (`SCXML083`). Without this call no binding is judged (`SCXML093`, a
    /// type that is not a URI, is judged either way).
    #[must_use]
    pub fn with_services(
        mut self,
        services: std::collections::BTreeMap<String, Vec<String>>,
    ) -> Self {
        self.services = Some(services);
        self
    }

    /// Links one optional parse-only expression syntax capability; each
    /// machine is checked by the checker for its exact `datamodel`. A later
    /// checker for the same data model replaces the earlier one.
    ///
    /// Without a checker, only data-model-neutral expression rules run (for
    /// example `datamodel="null"` misuse); no expression syntax is guessed.
    #[must_use]
    pub fn with_expression_checker(
        mut self,
        checker: std::sync::Arc<dyn ExpressionSyntaxChecker>,
    ) -> Self {
        self.expression_checkers
            .retain(|linked| linked.data_model() != checker.data_model());
        self.expression_checkers.push(checker);
        self
    }

    /// Selects schema behavior.
    #[must_use]
    pub const fn with_schema_mode(mut self, schema_mode: SchemaMode) -> Self {
        self.schema_mode = schema_mode;
        self
    }

    /// Selects deterministic XML source and tree budgets.
    #[must_use]
    pub const fn with_xml_limits(mut self, xml_limits: XmlLimits) -> Self {
        self.xml_limits = xml_limits;
        self
    }
}

impl<R: SchemaResolver, B: SchemaValidator> Validator<R, B> {
    /// Parses and validates UTF-8 XML source.
    ///
    /// XML well-formedness failures are converted into the common diagnostic
    /// model. Schema resolution is exact-URI and performs no ambient network
    /// access. Semantic rules run even when structural XSD errors are present.
    #[must_use]
    pub fn validate_source(
        &self,
        source_id: SourceId,
        name: impl Into<std::sync::Arc<str>>,
        text: impl Into<std::sync::Arc<str>>,
    ) -> ValidationOutcome {
        self.validate_source_with_event_schemas(source_id, name, text, &self.event_schema_resources)
    }

    /// [`Self::validate_source`] with the schema-file bytes of this one
    /// document's `event:schema` references (in place of those given with
    /// [`Self::with_event_schema_resources`]); the compiler validates each
    /// input with its own.
    #[must_use]
    pub fn validate_source_with_event_schemas(
        &self,
        source_id: SourceId,
        name: impl Into<std::sync::Arc<str>>,
        text: impl Into<std::sync::Arc<str>>,
        event_schemas: &EventSchemaResources,
    ) -> ValidationOutcome {
        let span = observe(Module::Validator, Operation::ValidatorValidateSource);
        let source = SourceText::new(source_id, name, text);
        let outcome = match parse_xml_with_limits(source.clone(), self.xml_limits) {
            Ok(document) => {
                let report = self.validate_parsed(&document, event_schemas);
                ValidationOutcome {
                    source,
                    document: Some(document),
                    report,
                }
            }
            Err(error) => {
                let mut report = Report::new();
                report.push(xml_error_diagnostic(&source, &error));
                refine::refine_report(None, &mut report);
                ValidationOutcome {
                    source,
                    document: None,
                    report,
                }
            }
        };
        record_outcome(&span, &outcome.report);
        outcome
    }

    /// Validates raw document bytes. Valid UTF-8 is validated exactly as by
    /// [`Self::validate_source`]; otherwise the report carries one `XML001`
    /// error at the first invalid byte, over the document decoded with each
    /// invalid sequence replaced by U+FFFD (the report's source text), so
    /// every output format locates it by line and column. Nothing else is
    /// validated: no rule can trust text the parser cannot read.
    #[must_use]
    pub fn validate_bytes(
        &self,
        source_id: SourceId,
        name: impl Into<std::sync::Arc<str>>,
        bytes: &[u8],
    ) -> ValidationOutcome {
        let error = match std::str::from_utf8(bytes) {
            Ok(text) => return self.validate_source(source_id, name, text),
            Err(error) => error,
        };
        let span = observe(Module::Validator, Operation::ValidatorValidateSource);
        let source = SourceText::new(source_id, name, String::from_utf8_lossy(bytes).into_owned());
        let mut report = Report::new();
        report.push(xml_error::invalid_utf8_diagnostic(&source, bytes, &error));
        let outcome = ValidationOutcome {
            source,
            document: None,
            report,
        };
        record_outcome(&span, &outcome.report);
        outcome
    }

    /// Runs schema and semantic validation for an already parsed document.
    #[must_use]
    pub fn validate_document(&self, document: &Document) -> Report {
        let span = observe(Module::Validator, Operation::ValidatorValidateDocument);
        let report = self.validate_parsed(document, &self.event_schema_resources);
        record_outcome(&span, &report);
        report
    }

    /// Schema and semantic validation shared by both public entry points;
    /// it runs inside their span and records no telemetry of its own.
    fn validate_parsed(&self, document: &Document, event_schemas: &EventSchemaResources) -> Report {
        let mut report = Report::new();
        if let Err(error) = validate_document_limits(document, self.xml_limits) {
            report.push(xml_error_diagnostic(&document.source, &error));
            refine::refine_report(Some(document), &mut report);
            return report;
        }
        if self.schema_mode != SchemaMode::Off {
            let mut schemas = Vec::new();
            for namespace_uri in document.used_namespaces() {
                // The XML namespace and the XML Schema instance namespace
                // (`xsi:schemaLocation`, `xsi:type`, ...) are built into
                // XML and XML Schema; neither has a schema to register.
                // The `env` attribute vocabulary is the host's, checked by
                // `SCXML083`/`SCXML084`; extension schemas admit it laxly.
                // The `event` attribute vocabulary is checked by
                // `SCXML085`-`SCXML092`; the W3C schema admits it laxly.
                if namespace_uri == "http://www.w3.org/XML/1998/namespace"
                    || namespace_uri == "http://www.w3.org/2001/XMLSchema-instance"
                    || namespace_uri == scxml_types::ENV_NAMESPACE_URI
                    || namespace_uri == scxml_types::EVENT_NAMESPACE_URI
                {
                    continue;
                }
                // SCXML001 owns a near-miss namespace on the root (with the
                // exact-URI fix); a missing-schema error would restate it.
                if semantic::is_near_scxml_namespace(&namespace_uri)
                    && document.root.name.namespace_uri.as_deref() == Some(namespace_uri.as_str())
                {
                    continue;
                }
                match self.resolver.resolve_documents(&namespace_uri) {
                    Ok(documents) => schemas.extend(documents),
                    Err(SchemaResolveError::NotFound) if self.schema_mode == SchemaMode::Lax => {}
                    Err(error) => report.push(unresolved_schema_diagnostic(
                        document,
                        &namespace_uri,
                        &error,
                    )),
                }
            }
            if !schemas.is_empty() {
                match self.backend.validate(document, &schemas) {
                    Ok(schema_report) => report.extend(schema_report),
                    Err(error) => report.push(error.into_diagnostic(document)),
                }
            }
        }
        let checkers = self
            .expression_checkers
            .iter()
            .map(|checker| checker.as_ref() as &dyn ExpressionSyntaxChecker)
            .collect::<Vec<_>>();
        let rules = |document: &Document| {
            let mut report = validate_semantics(document);
            if checkers.is_empty() {
                report.extend(report_unchecked_expressions(document));
            } else {
                report.extend(validate_expressions_with(document, &checkers));
            }
            report
        };
        report.extend(rules(document));
        for diagnostic in env_attribute_diagnostics(
            document,
            self.extension_slots.as_ref(),
            self.services.as_ref(),
        ) {
            report.push(diagnostic);
        }
        for diagnostic in event_attribute_diagnostics(document, event_schemas) {
            report.push(diagnostic);
        }
        for diagnostic in invoke_types::invoke_type_diagnostics(document, self.services.as_ref()) {
            report.push(diagnostic);
        }
        refine::refine_report(Some(document), &mut report);
        report.sort();
        report
    }
}

/// Records a validation boundary's closed outcome: rejected with the
/// `validation` category when any error is reported, accepted otherwise.
fn record_outcome(span: &FunctionSpan, report: &Report) {
    if report.has_errors() {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Validation);
    } else {
        span.event(SignalEvent::InputAccepted);
    }
}

/// Source, optional parsed tree, and diagnostics from one run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationOutcome {
    /// Source text suitable for terminal rendering.
    pub source: SourceText,
    /// Parsed tree when XML was well formed.
    pub document: Option<Document>,
    /// Deterministically ordered diagnostics.
    pub report: Report,
}

/// Builds the span-exact XSD001 diagnostic for one unresolved namespace.
///
/// The primary label is the namespace URI in the declaration that binds it
/// (the bytes an author would correct), and the first use of the namespace is
/// a secondary label. A near-miss of the SCXML namespace gets a
/// machine-applicable correction, because the exact URI is normative.
fn unresolved_schema_diagnostic(
    document: &Document,
    namespace_uri: &str,
    error: &SchemaResolveError,
) -> Diagnostic {
    let declaration = spans::namespace_declaration_value(document, namespace_uri);
    let usage = spans::first_namespace_use(document, namespace_uri);
    let primary = declaration
        .or(usage)
        .unwrap_or(document.root.start_tag_span);
    let reason = match error {
        SchemaResolveError::NotFound => "no schema is registered for it".to_owned(),
        SchemaResolveError::Load(message) => {
            format!("its registered schema could not be loaded: {message}")
        }
    };
    let mut diagnostic = Diagnostic::error(
        "XSD001",
        format!("cannot resolve an XML Schema for namespace `{namespace_uri}`"),
        primary,
    )
    .with_primary_message(reason)
    .with_note("schema lookup uses the exact namespace URI; the SCXML namespace uses the bundled W3C SCXML 1.0 schema by default, and every other namespace used under the strict policy needs an explicitly registered schema")
    .with_spec_reference("SCXML 1.0 §4.10 Extensibility of Executable Content");
    if let (Some(usage), Some(declaration)) = (usage, declaration)
        && usage != declaration
    {
        diagnostic = diagnostic
            .with_secondary(Label::new(usage).with_message("first use of this namespace"));
    }
    let near_scxml = semantic::is_near_scxml_namespace(namespace_uri);
    diagnostic = if near_scxml && let Some(declaration) = declaration {
        diagnostic
            .with_help(format!(
                "the SCXML namespace is exactly `{SCXML_NAMESPACE}`"
            ))
            .with_fix(Fix::replace(
                "use the exact SCXML namespace URI",
                Applicability::MachineApplicable,
                declaration,
                SCXML_NAMESPACE,
            ))
    } else if matches!(error, SchemaResolveError::Load(_)) {
        diagnostic.with_help("fix the registered schema path, or register a different schema with `--schema URI=PATH`")
    } else {
        diagnostic.with_help(format!(
            "register a schema with `--schema {namespace_uri}=PATH.xsd`, or validate with `--policy lax` to skip namespaces without a schema"
        ))
    };
    diagnostic
}

#[cfg(test)]
mod tests {
    use scxml_syntax::telemetry::{Recorder, install_recorder};
    use scxml_syntax::{SourceId, SourceText, XmlLimits, parse_xml_with_limits};

    use super::*;

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);

    #[derive(Debug)]
    struct NoSchema;

    impl SchemaResolver for NoSchema {
        fn resolve(&self, _namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
            Err(SchemaResolveError::NotFound)
        }
    }

    #[derive(Debug)]
    struct NoBackend;

    impl SchemaValidator for NoBackend {
        fn validate(
            &self,
            _document: &Document,
            _schemas: &[SchemaSource],
        ) -> Result<Report, SchemaBackendError> {
            Ok(Report::new())
        }
    }

    #[test]
    fn without_a_linked_checker_expressions_are_reported_unchecked() {
        // A validator composed with no expression checker (for example the
        // Wasm component whose host links `supported-data-model = none`) must
        // still say that ECMAScript expressions went unchecked.
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" datamodel="ecmascript"><state id="s"><transition cond="n = = 1" target="s"/></state></scxml>"#
        );
        let outcome = Validator::new(NoSchema, NoBackend)
            .with_schema_mode(SchemaMode::Off)
            .validate_source(SOURCE, "unchecked.scxml", xml);
        let note = outcome
            .report
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == "SCXML045")
            .expect("unchecked expressions must be reported");
        assert_eq!(note.severity, Severity::Note);
        assert!(
            note.notes
                .iter()
                .any(|text| text.contains("no expression syntax checker")),
            "{note:?}"
        );
    }

    #[test]
    fn validator_rechecks_tree_limits_and_records_rejection() {
        let xml = format!("{}{}{}", "<a>".repeat(257), "x", "</a>".repeat(257));
        let document = parse_xml_with_limits(
            SourceText::new(SOURCE, "manual.xml", xml),
            XmlLimits {
                max_depth: 300,
                ..XmlLimits::default()
            },
        )
        .expect("fixture parses under wider caller limits");
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let report = Validator::new(NoSchema, NoBackend)
            .with_schema_mode(SchemaMode::Off)
            .validate_document(&document);
        assert_eq!(report.diagnostics()[0].code, "XML002");
        assert!(recorder.observations().iter().any(|item| {
            item.module == Module::Validator
                && item.operation == Operation::ValidatorValidateDocument
                && item.error == Some(ErrorCategory::Validation)
                && item.events.contains(&SignalEvent::InputRejected)
        }));
    }

    #[test]
    fn validation_records_only_its_boundary_spans() {
        // Boundary-only telemetry: rule helpers, graph queries, span
        // arithmetic, and diagnostic builders run inside the validate span
        // and must not open spans of their own, however many rules fire.
        let xml = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" version="1.0" initial="missing"><state id="a"><transition event="go" target="nowhere"/></state><state id="a"/></scxml>"#
        );
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let outcome = Validator::new(NoSchema, NoBackend)
            .with_schema_mode(SchemaMode::Off)
            .validate_source(SOURCE, "boundary.scxml", xml);
        assert!(outcome.report.has_errors());
        let operations = recorder
            .observations()
            .iter()
            .map(|item| item.operation)
            .collect::<Vec<_>>();
        assert_eq!(
            operations,
            [
                Operation::SyntaxParseXml,
                Operation::ValidatorValidateSource
            ],
            "only the parse and validate boundaries may record spans"
        );
        let validate = recorder
            .observations()
            .into_iter()
            .find(|item| item.operation == Operation::ValidatorValidateSource)
            .expect("the validate boundary span was recorded");
        assert_eq!(validate.module, Module::Validator);
        assert_eq!(validate.error, Some(ErrorCategory::Validation));
        assert_eq!(validate.events, [SignalEvent::InputRejected]);
    }
}
