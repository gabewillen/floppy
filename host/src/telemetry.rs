//! OpenTelemetry spans of the embedder's boundaries.
//!
//! nemobot-host reports through the OpenTelemetry API only; the process that
//! embeds it installs the SDK, provider, and exporter (none installed means
//! every span is a no-op). The scxml.rs host's own boundaries report through
//! `scxml-telemetry` underneath these spans.
//!
//! # Cardinality
//!
//! | Signal | Values | Bound |
//! | --- | --- | --- |
//! | span name | [`Operation::name`] | 7 (closed enum) |
//! | `nemobot.error.kind` (failed spans only) | [`crate::FailureKind::name`] | 7 (closed enum) |
//! | `nemobot.session.status` (`nemobot.session.close` only) | `running`, `finished`, `failed` | 3 |
//!
//! No path, URI, event or state name, variable name, value, digest, or
//! payload is ever an attribute.

use opentelemetry::KeyValue;
use opentelemetry::global::BoxedSpan;
use opentelemetry::trace::{Span as _, Status, Tracer as _};

use crate::FailureKind;

/// The instrumentation scope of every span.
const SCOPE: &str = "nemobot-host";

/// One embedder boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    /// Reading and checking one nemobot config file.
    LoadConfig,
    /// Reading the run's env-file values.
    ReadEnvFile,
    /// Reading, verifying, and authenticating the bound packages.
    BindPackages,
    /// Validating and compiling one document to Machine IR.
    Compile,
    /// Linking the session host and opening the live root session.
    SessionOpen,
    /// One ingress input applied to the live session.
    SessionStep,
    /// Closing the live session.
    SessionClose,
}

impl Operation {
    /// The span name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::LoadConfig => "nemobot.config.load",
            Self::ReadEnvFile => "nemobot.env_file.read",
            Self::BindPackages => "nemobot.packages.bind",
            Self::Compile => "nemobot.document.compile",
            Self::SessionOpen => "nemobot.session.open",
            Self::SessionStep => "nemobot.session.step",
            Self::SessionClose => "nemobot.session.close",
        }
    }
}

/// One open boundary span; ended when dropped.
pub(crate) struct Observed {
    span: BoxedSpan,
}

/// Starts the span of `operation` under the current context.
pub(crate) fn observe(operation: Operation) -> Observed {
    Observed {
        span: opentelemetry::global::tracer(SCOPE).start(operation.name()),
    }
}

impl Observed {
    /// Marks the span failed with a closed failure kind.
    pub(crate) fn fail(&mut self, kind: FailureKind) {
        self.span
            .set_attribute(KeyValue::new("nemobot.error.kind", kind.name()));
        self.span.set_status(Status::error(kind.name()));
    }

    /// Records the closed session status (`nemobot.session.close`).
    pub(crate) fn status(&mut self, status: &'static str) {
        self.span
            .set_attribute(KeyValue::new("nemobot.session.status", status));
    }

    /// Records the outcome of `result`: failed with its kind, or ok.
    pub(crate) fn finish<T>(mut self, result: &Result<T, crate::HostFailure>) {
        match result {
            Ok(_) => self.span.set_status(Status::Ok),
            Err(failure) => self.fail(failure.kind()),
        }
    }
}

impl Drop for Observed {
    fn drop(&mut self) {
        self.span.end();
    }
}
