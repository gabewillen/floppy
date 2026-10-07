//! The validator component (`stateforward:scxml-validator/validator`) with
//! its `expression-syntax` import linked to one data-model component.
//!
//! The validator world imports one parse-only `expression-syntax`
//! capability. This module is the host adapter the WIT names: it answers
//! that import by forwarding each fragment to an authenticated data-model
//! component's `data-model-api.check-syntax`, in the data model's own
//! bounded store, so the component validator checks expressions exactly as
//! the data model will parse them at run time. The host stays a thin
//! linker: it never parses SCXML or expressions itself and never chooses a
//! data model for a document.
//!
//! Contract:
//!
//! * `supported-data-model` answers the linked data model's `name()`, or
//!   `none` when no data model is linked or the linked one has no expression
//!   syntax (`null`, whose provider answers every check `unavailable`). Only
//!   documents whose `datamodel` equals that name exactly are checked; the
//!   validator reports every other recognized data model's expressions with
//!   the `SCXML045` note.
//! * `check-syntax` forwards the fragment unchanged with a fresh fuel budget
//!   ([`DEFAULT_DATA_MODEL_CALL_FUEL`](crate::DEFAULT_DATA_MODEL_CALL_FUEL)).
//!   A data-model `payload-too-large` becomes `declined::too-large`,
//!   `invalid-input` (a parser bound) `declined::too-complex`, and any other
//!   failure or a trap `declined::unavailable`; the validator reports every
//!   decline as `SCXML040`. A trap poisons the data-model instance, so later
//!   checks decline as well.
//! * `check-effects` forwards the request unchanged the same way and maps
//!   the findings one to one; a failure declines as for `check-syntax`, and
//!   the validator checks every returned span and bound itself.
//! * The validator runs in its own bounded store (memory, table, instance
//!   limits and fuel from [`WasmtimeHostConfig`]); the canonical report
//!   bytes it returns are passed through unchanged.
//!
//! Telemetry: one `scxml.host-wasmtime.validator-new` span per compile and
//! link and one `scxml.host-wasmtime.validator-validate` span per request,
//! with closed outcome labels; the guests' closed WIT spans nest under
//! it. Source text, expressions, and report bytes never become attributes.

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::stateforward::scxml::types::{ComponentError, ErrorCode};
use crate::data_model::{
    DataModelEffectCause, DataModelEffectFinding, DataModelEffectFragment, DataModelEffectKind,
    DataModelEffectRole, DataModelEffectSpan, DataModelExternalScripts, DataModelStoreData,
    DataModelSyntaxIssue, DataModelSyntaxKind, data_model_linker,
};
use crate::guest_spans::GuestSpanTable;
use crate::memory_growth::MeteredLimits;
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ComponentDataModel, HostError,
    LifecycleLease, MachineDataModel as _, PackageValidators, TelemetryProviders,
    WasmtimeHostConfig, component_engine, record_store_limits,
};

#[allow(
    missing_docs,
    reason = "generated WIT bindings re-exported as `validator_types`; the canonical documentation is validator.wit"
)]
pub(crate) mod validator_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/validator",
        world: "validator",
    });
}

use validator_bindings::stateforward::scxml_validator::expression_syntax::{
    Declined, EffectCause, EffectFinding, EffectFragment, EffectKind, EffectRole, EffectSpan,
    ExternalScripts, SyntaxIssue, SyntaxKind,
};
use validator_bindings::stateforward::scxml_validator::telemetry as validator_telemetry;
use validator_bindings::stateforward::scxml_validator::types::ErrorCode as ValidatorErrorCode;

/// The validator component's request types (`stateforward:scxml-validator`
/// `types`), as the generated bindings define them.
pub mod validator_types {
    pub use super::validator_bindings::stateforward::scxml_validator::document::{
        Attribute, Context, Element, QualifiedName, TransitionInfo,
    };
    pub use super::validator_bindings::stateforward::scxml_validator::types::{
        Applicability, Diagnostic, Edit, EventSchemaResource, ExtensionPackage, Fix, Label,
        Profile, SchemaPolicy, SchemaResource, Severity, Span, TraceContext, ValidationRequest,
    };
}

/// Store state of one validator component instance.
struct ValidatorStoreData {
    limits: MeteredLimits,
    providers: TelemetryProviders,
    execution_context: Context,
    guest_spans: GuestSpanTable,
    /// The linked data model and its name, answering `expression-syntax`.
    syntax: Option<(ComponentDataModel, String)>,
    /// The bound package validators, answering `package-validator-api`.
    packages: Option<std::sync::Arc<PackageValidators>>,
    _store_lease: LifecycleLease,
}

/// The validator component compiled once, with its `expression-syntax`
/// import linked to one data-model component (or to `none`). Every
/// [`Self::validate`] runs in fresh bounded stores, so requests share no
/// guest state. Single owner; calls are synchronous.
pub struct ComponentValidator {
    engine: wasmtime::Engine,
    validator: wasmtime::component::Component,
    linker: Linker<ValidatorStoreData>,
    data_model: Option<(wasmtime::component::Component, Linker<DataModelStoreData>)>,
    config: WasmtimeHostConfig,
    providers: TelemetryProviders,
    packages: Option<std::sync::Arc<PackageValidators>>,
}

impl std::fmt::Debug for ComponentValidator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComponentValidator")
            .field("data_model", &self.data_model.is_some())
            .finish_non_exhaustive()
    }
}

impl ComponentValidator {
    /// Compiles and links `validator` (the authenticated validator
    /// artifact) with its `expression-syntax` import answered by
    /// `data_model` (an authenticated `scxml_data_model_*` artifact), or by
    /// `none` when `data_model` is `None`.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::ArtifactKindMismatch`] for any other artifact kinds;
    /// [`ArtifactError::ArtifactTooLarge`] over the configured component
    /// bound; invalid host limits; and compile or link failures. Nothing is
    /// returned on failure.
    pub fn new(
        validator: &AuthenticatedComponent,
        data_model: Option<&AuthenticatedComponent>,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeValidatorNew);
        let result = Self::new_inner(validator, data_model, config, providers);
        match &result {
            Ok(_) => span.event(SignalEvent::ArtifactAccepted),
            Err(HostError::Artifact(_) | HostError::InvalidLimit(_)) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::InvalidInput);
            }
            Err(_) => span.fail(ErrorCategory::Platform),
        }
        result
    }

    fn new_inner(
        validator: &AuthenticatedComponent,
        data_model: Option<&AuthenticatedComponent>,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        config.validate()?;
        if validator.kind() != ArtifactKind::Validator
            || data_model.is_some_and(|component| {
                !matches!(
                    component.kind(),
                    ArtifactKind::DataModelEcmascript
                        | ArtifactKind::DataModelNull
                        | ArtifactKind::DataModelXpath
                )
            })
        {
            return Err(ArtifactError::ArtifactKindMismatch.into());
        }
        if validator.bytes().len() > config.component_bytes
            || data_model.is_some_and(|component| component.bytes().len() > config.component_bytes)
        {
            return Err(ArtifactError::ArtifactTooLarge.into());
        }
        let engine = component_engine(config)?;
        let data_model = match data_model {
            Some(component) => Some((
                wasmtime::component::Component::from_binary(&engine, component.bytes())?,
                data_model_linker(&engine)?,
            )),
            None => None,
        };
        let compiled = wasmtime::component::Component::from_binary(&engine, validator.bytes())?;
        let mut linker = Linker::<ValidatorStoreData>::new(&engine);
        validator_bindings::Validator::add_to_linker::<_, HasSelf<ValidatorStoreData>>(
            &mut linker,
            |state| state,
        )?;
        Ok(Self {
            engine,
            validator: compiled,
            linker,
            data_model,
            config: config.clone(),
            providers: providers.clone(),
            packages: None,
        })
    }

    /// Answers the validator's `package-validator-api` import with
    /// `packages`: each call goes to the validator bound to its targets'
    /// package ([`PackageValidators::route`]), and a package with none
    /// answers an empty list. Without this call every package answers an
    /// empty list.
    #[must_use]
    pub fn with_package_validators(mut self, packages: std::sync::Arc<PackageValidators>) -> Self {
        self.packages = Some(packages);
        self
    }

    /// Validates one request in fresh stores and returns the component's
    /// canonical `scxml.report/v3` bytes unchanged.
    ///
    /// # Errors
    ///
    /// Instantiation failures; a trap (including fuel exhaustion) in the
    /// validator; and [`HostError::Component`] with the closed WIT code
    /// (`invalid-request`, `canonical-report-failed`) when the validator
    /// rejects the request. Nothing is returned on failure.
    pub fn validate(
        &self,
        request: &validator_types::ValidationRequest,
    ) -> Result<Vec<u8>, HostError> {
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeValidatorValidate,
        );
        let result = self.validate_inner(request);
        match &result {
            Ok(_) => span.event(SignalEvent::ArtifactProduced),
            Err(HostError::Component(_)) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::InvalidInput);
            }
            Err(_) => span.fail(ErrorCategory::Platform),
        }
        result
    }

    fn validate_inner(
        &self,
        request: &validator_types::ValidationRequest,
    ) -> Result<Vec<u8>, HostError> {
        let config = &self.config;
        let syntax = match &self.data_model {
            Some((compiled, linker)) => {
                let model = ComponentDataModel::from_compiled(
                    &self.engine,
                    compiled,
                    linker,
                    config,
                    &self.providers,
                    Context::current(),
                    None,
                )?;
                let name = model.name().unwrap_or_default().to_owned();
                // The null data model has no expression syntax to check.
                (!name.is_empty() && name != "null").then_some((model, name))
            }
            None => None,
        };
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(config.store.memory_bytes)
            .table_elements(config.store.table_elements)
            .instances(config.store.instances)
            .tables(config.store.tables)
            .memories(config.store.memories)
            .trap_on_grow_failure(true)
            .build();
        let limits = MeteredLimits::metered(limits, None, config.store.fuel);
        let budget = std::sync::Arc::clone(limits.budget());
        let mut store = wasmtime::Store::new(
            &self.engine,
            ValidatorStoreData {
                limits,
                providers: self.providers.clone(),
                execution_context: Context::current(),
                guest_spans: GuestSpanTable::new(),
                syntax,
                packages: self.packages.clone(),
                _store_lease: self.providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        budget.share(&mut store);
        store.set_fuel(config.store.fuel)?;
        record_store_limits(&config.store);
        let bindings =
            validator_bindings::Validator::instantiate(&mut store, &self.validator, &self.linker)?;
        bindings
            .stateforward_scxml_validator_validator_api()
            .call_validate(&mut store, request)?
            .map_err(|error| {
                HostError::Component(
                    match error.code {
                        ValidatorErrorCode::InvalidRequest => "invalid-request",
                        ValidatorErrorCode::CanonicalReportFailed => "canonical-report-failed",
                    }
                    .into(),
                )
            })
    }
}

/// Maps a data-model check failure to the validator's closed decline.
const fn declined(error: &ComponentError) -> Declined {
    match error.code {
        ErrorCode::PayloadTooLarge => Declined::TooLarge,
        ErrorCode::InvalidInput => Declined::TooComplex,
        _ => Declined::Unavailable,
    }
}

impl validator_bindings::stateforward::scxml_validator::expression_syntax::Host
    for ValidatorStoreData
{
    fn supported_data_model(&mut self) -> Option<String> {
        self.syntax.as_ref().map(|(_, name)| name.clone())
    }

    fn check_syntax(
        &mut self,
        kind: SyntaxKind,
        source: String,
    ) -> Result<Vec<SyntaxIssue>, Declined> {
        let Some((model, _)) = self.syntax.as_mut() else {
            return Err(Declined::Unavailable);
        };
        let kind = match kind {
            SyntaxKind::Condition => DataModelSyntaxKind::Condition,
            SyntaxKind::Value => DataModelSyntaxKind::Value,
            SyntaxKind::Location => DataModelSyntaxKind::Location,
            SyntaxKind::Script => DataModelSyntaxKind::Script,
        };
        model
            .forward(|api, store| api.call_check_syntax(store, kind, &source))
            .map(|issues| {
                issues
                    .into_iter()
                    .map(
                        |DataModelSyntaxIssue {
                             start,
                             end,
                             message,
                         }| SyntaxIssue {
                            start,
                            end,
                            message,
                        },
                    )
                    .collect()
            })
            .map_err(|error| declined(&error))
    }

    fn check_effects(
        &mut self,
        fragments: Vec<EffectFragment>,
        scripts: ExternalScripts,
    ) -> Result<Vec<EffectFinding>, Declined> {
        let Some((model, _)) = self.syntax.as_mut() else {
            return Err(Declined::Unavailable);
        };
        let request: Vec<DataModelEffectFragment> = fragments
            .into_iter()
            .map(|fragment| DataModelEffectFragment {
                role: match fragment.role {
                    EffectRole::Condition => DataModelEffectRole::Condition,
                    EffectRole::Log => DataModelEffectRole::Log,
                    EffectRole::Script => DataModelEffectRole::Script,
                    EffectRole::Data => DataModelEffectRole::Data,
                    EffectRole::Value => DataModelEffectRole::Value,
                    EffectRole::Location => DataModelEffectRole::Location,
                },
                name: fragment.name,
                source: fragment.source,
            })
            .collect();
        model
            .forward(|api, store| {
                let scripts = match scripts {
                    ExternalScripts::Absent => DataModelExternalScripts::Absent,
                    ExternalScripts::Present => DataModelExternalScripts::Present,
                };
                api.call_check_effects(store, &request, scripts)
            })
            .map(|findings| {
                findings
                    .into_iter()
                    .map(|finding: DataModelEffectFinding| EffectFinding {
                        certain: finding.certain,
                        kind: effect_kind(finding.kind),
                        span: effect_span(finding.span),
                        subject: finding.subject,
                        cause: finding
                            .cause
                            .map(|cause: DataModelEffectCause| EffectCause {
                                kind: effect_kind(cause.kind),
                                span: effect_span(cause.span),
                                subject: cause.subject,
                                function: cause.function,
                            }),
                        operator: finding.operator.map(effect_span),
                    })
                    .collect()
            })
            .map_err(|error| declined(&error))
    }
}

/// The data model's effect kind as the validator's (the same closed set).
const fn effect_kind(kind: DataModelEffectKind) -> EffectKind {
    match kind {
        DataModelEffectKind::Assignment => EffectKind::Assignment,
        DataModelEffectKind::Update => EffectKind::Update,
        DataModelEffectKind::Deletion => EffectKind::Deletion,
        DataModelEffectKind::MutatingCall => EffectKind::MutatingCall,
        DataModelEffectKind::Random => EffectKind::Random,
        DataModelEffectKind::FunctionCall => EffectKind::FunctionCall,
        DataModelEffectKind::UnknownCall => EffectKind::UnknownCall,
        DataModelEffectKind::DynamicCode => EffectKind::DynamicCode,
        DataModelEffectKind::ImplicitCall => EffectKind::ImplicitCall,
        DataModelEffectKind::UnknownWrite => EffectKind::UnknownWrite,
        DataModelEffectKind::DynamicScope => EffectKind::DynamicScope,
        DataModelEffectKind::UnseenScript => EffectKind::UnseenScript,
        DataModelEffectKind::TooComplex => EffectKind::TooComplex,
        DataModelEffectKind::SystemVariableWrite => EffectKind::SystemVariableWrite,
    }
}

const fn effect_span(span: DataModelEffectSpan) -> EffectSpan {
    EffectSpan {
        fragment: span.fragment,
        start: span.start,
        end: span.end,
    }
}

impl validator_bindings::stateforward::scxml_validator::types::Host for ValidatorStoreData {}

impl validator_bindings::stateforward::scxml_validator::document::Host for ValidatorStoreData {}

impl validator_bindings::stateforward::scxml_validator::package_validator_api::Host
    for ValidatorStoreData
{
    fn validate(
        &mut self,
        document: validator_types::Context,
        targets: Vec<u32>,
    ) -> Result<
        Vec<validator_types::Diagnostic>,
        validator_bindings::stateforward::scxml_validator::types::ComponentError,
    > {
        let (Some(packages), Some(package)) = (
            self.packages.as_ref(),
            PackageValidators::route(&document, &targets),
        ) else {
            return Ok(Vec::new());
        };
        packages
            .validate(package, &document, &targets)
            .map_err(
                |_| validator_bindings::stateforward::scxml_validator::types::ComponentError {
                    version:
                        validator_bindings::stateforward::scxml_validator::types::ErrorVersion::V1,
                    code: ValidatorErrorCode::InvalidRequest,
                },
            )
    }
}

impl validator_telemetry::Host for ValidatorStoreData {
    fn start(
        &mut self,
        _module: validator_telemetry::Module,
        _operation: validator_telemetry::Operation,
        parent: Option<validator_telemetry::TraceContext>,
    ) -> (u64, Option<validator_telemetry::TraceContext>) {
        let parent = if let Some(carrier) = parent {
            let extracted =
                scxml_telemetry::extract_trace_context(&scxml_telemetry::TraceContext {
                    traceparent: carrier.traceparent,
                    tracestate: carrier.tracestate,
                });
            if let Ok(parent) = extracted {
                parent
            } else {
                crate::component::guest_telemetry_refused(&self.execution_context);
                return (0, None);
            }
        } else {
            self.execution_context.clone()
        };
        let started = self.guest_spans.start(
            &self.providers,
            "scxml-component-guest",
            "validator-component",
            "validate",
            &parent,
        );
        if let Ok((handle, carrier)) = started {
            (
                handle,
                Some(validator_telemetry::TraceContext {
                    traceparent: carrier.traceparent,
                    tracestate: carrier.tracestate,
                }),
            )
        } else {
            crate::component::guest_telemetry_refused(&self.execution_context);
            (0, None)
        }
    }

    fn event(&mut self, span: u64, event: validator_telemetry::SignalEvent) {
        let event = match event {
            validator_telemetry::SignalEvent::InputRejected => "input-rejected",
            validator_telemetry::SignalEvent::ArtifactProduced => "artifact-produced",
        };
        if self.guest_spans.event(span, event).is_err() {
            crate::component::guest_telemetry_refused(&self.execution_context);
        }
    }

    fn end(&mut self, span: u64, error: Option<validator_telemetry::ErrorCategory>) {
        let error = error.map(|category| match category {
            validator_telemetry::ErrorCategory::InvalidInput => "invalid-input",
            validator_telemetry::ErrorCategory::Validation => "validation",
            validator_telemetry::ErrorCategory::Report => "report",
        });
        if self.guest_spans.end_with_error(span, error).is_err() {
            crate::component::guest_telemetry_refused(&self.execution_context);
        }
    }
}
