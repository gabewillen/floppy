//! The package validators `validate`, `compile`, `snapshot run`, and `run`
//! link: each bound extension or service package whose manifest names a
//! validator component (`validator-artifact`, `validator-sha256`, read and
//! verified with the package, [`crate::extensions::read_validator`] or the
//! repository's [`crate::repository`] resolution) is authenticated by that
//! digest and bound to the package's exact URI in the Wasmtime host's
//! [`scxml_host_wasmtime::PackageValidators`] (no imports, bounded fuel,
//! fresh store per call). The validator calls it once per document after the
//! XSD and built-in checks (`scxml_validator_core::package_validator`).
//! Without the `wasmtime` feature a package that names a validator fails
//! closed: nothing would run it.

use std::sync::Arc;

use scxml_validator_core::PackageValidator;

use crate::extensions::Package;
use crate::services::ServicePackage;

/// Every bound package's `(URI, validator bytes, manifest validator-sha256)`.
fn named<'a>(
    extensions: &'a [Package],
    services: &'a [ServicePackage],
) -> Vec<(&'a str, &'a [u8], &'a str)> {
    extensions
        .iter()
        .filter_map(|package| {
            Some((
                package.manifest.namespace_uri.as_str(),
                package.validator.as_deref()?,
                package.manifest.validator_sha256.as_deref()?,
            ))
        })
        .chain(services.iter().filter_map(|package| {
            Some((
                package.manifest.service_uri.as_str(),
                package.validator.as_deref()?,
                package.manifest.validator_sha256.as_deref()?,
            ))
        }))
        .collect()
}

/// The package validators of `extensions` and `services`; `None` when no
/// package names one.
///
/// # Errors
///
/// A validator that does not authenticate, compile, or import nothing; any
/// validator in a build without the `wasmtime` feature.
#[cfg(feature = "wasmtime")]
pub(crate) fn linked(
    extensions: &[Package],
    services: &[ServicePackage],
) -> Result<Option<Arc<dyn PackageValidator>>, String> {
    use scxml_host_wasmtime::{
        PackageValidators, TelemetryProviders, TrustedComponentDigest, WasmtimeHostConfig,
        authenticate_package_validator_component,
    };
    let named = named(extensions, services);
    if named.is_empty() {
        return Ok(None);
    }
    let mut validators =
        PackageValidators::new(&WasmtimeHostConfig::default(), &TelemetryProviders::new())
            .map_err(|error| format!("cannot start the package validator host: {error}"))?;
    for (uri, bytes, digest) in named {
        // The manifest (verified, or pinned for a repository package) is the
        // trust anchor of its validator's digest.
        TrustedComponentDigest::from_hex(digest)
            .map_err(|error| error.to_string())
            .and_then(|trusted| {
                authenticate_package_validator_component(bytes, trusted)
                    .map_err(|error| error.to_string())
            })
            .and_then(|component| {
                validators
                    .bind(uri, &component)
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| {
                format!("the package validator of `{uri}` cannot be bound: {error}")
            })?;
    }
    Ok(Some(Arc::new(Linked(validators))))
}

/// Without Wasmtime no package validator can run, so a package that names
/// one fails closed.
#[cfg(not(feature = "wasmtime"))]
pub(crate) fn linked(
    extensions: &[Package],
    services: &[ServicePackage],
) -> Result<Option<Arc<dyn PackageValidator>>, String> {
    match named(extensions, services).first() {
        None => Ok(None),
        Some((uri, ..)) => Err(format!(
            "the package `{uri}` names a package validator, which this scx cannot run: it was built without `--features wasmtime`\n  = help: rebuild with `--features wasmtime`"
        )),
    }
}

/// The host's package validators in the validator's terms.
#[cfg(feature = "wasmtime")]
#[derive(Debug)]
struct Linked(scxml_host_wasmtime::PackageValidators);

#[cfg(feature = "wasmtime")]
impl PackageValidator for Linked {
    fn validate(
        &self,
        package: &str,
        document: &scxml_validator_core::package_validator::document::Context,
        targets: &[u32],
    ) -> Result<
        Vec<scxml_validator_core::package_validator::types::Diagnostic>,
        scxml_validator_core::PackageValidatorFailure,
    > {
        use scxml_host_wasmtime::validator_types as wit;
        use scxml_validator_core::package_validator::types;
        let span = |span: types::Span| wit::Span {
            start: span.start,
            end: span.end,
        };
        let name = |name: &scxml_validator_core::package_validator::document::QualifiedName| {
            wit::QualifiedName {
                namespace_uri: name.namespace_uri.clone(),
                local_name: name.local_name.clone(),
            }
        };
        let context = wit::Context {
            elements: document
                .elements
                .iter()
                .map(|element| wit::Element {
                    name: name(&element.name),
                    span: span(element.span),
                    attributes: element
                        .attributes
                        .iter()
                        .map(|attribute| wit::Attribute {
                            name: name(&attribute.name),
                            value: attribute.value.clone(),
                            span: span(attribute.span),
                        })
                        .collect(),
                    children: element.children.clone(),
                    parent: element.parent,
                })
                .collect(),
            data_ids: document.data_ids.clone(),
            state_ids: document.state_ids.clone(),
            transitions: document
                .transitions
                .iter()
                .map(|transition| wit::TransitionInfo {
                    state: transition.state.clone(),
                    event: transition.event.clone(),
                    event_schema: transition.event_schema.clone(),
                })
                .collect(),
            datamodel: document.datamodel.clone(),
        };
        let returned = self
            .0
            .validate(package, &context, targets)
            .map_err(|_| scxml_validator_core::PackageValidatorFailure)?;
        let span = |span: wit::Span| types::Span {
            start: span.start,
            end: span.end,
        };
        let label = |label: wit::Label| types::Label {
            span: span(label.span),
            message: label.message,
        };
        Ok(returned
            .into_iter()
            .map(|diagnostic| types::Diagnostic {
                code: diagnostic.code,
                severity: match diagnostic.severity {
                    wit::Severity::Note => types::Severity::Note,
                    wit::Severity::Warning => types::Severity::Warning,
                    wit::Severity::Error => types::Severity::Error,
                },
                message: diagnostic.message,
                primary: label(diagnostic.primary),
                secondary: diagnostic.secondary.into_iter().map(label).collect(),
                help: diagnostic.help,
                fixes: diagnostic
                    .fixes
                    .into_iter()
                    .map(|fix| types::Fix {
                        message: fix.message,
                        applicability: match fix.applicability {
                            wit::Applicability::MachineApplicable => {
                                types::Applicability::MachineApplicable
                            }
                            wit::Applicability::MaybeIncorrect => {
                                types::Applicability::MaybeIncorrect
                            }
                            wit::Applicability::HasPlaceholders => {
                                types::Applicability::HasPlaceholders
                            }
                        },
                        edits: fix
                            .edits
                            .into_iter()
                            .map(|edit| types::Edit {
                                span: span(edit.span),
                                replacement: edit.replacement,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect())
    }
}
