//! The one bounded guest span table every Wasmtime store uses.
//!
//! Guests (machine, data-model, script, clock, I/O processor, and
//! invoked-service components) open operation spans through the WIT
//! telemetry facade's `start` and close them with `end`. Each store owns one
//! [`GuestSpanTable`]: at most [`MAX_ACTIVE_GUEST_SPANS`] open spans, handles
//! allocated with checked arithmetic and never `0` (the facade's "no span"
//! value), each span holding a provider lifecycle lease, and every span still
//! open when the table drops ended exactly once. Span attributes are the
//! closed `scxml.module`, `scxml.operation`, and `scxml.outcome` labels.

use std::collections::BTreeMap;

use opentelemetry::Context;
use opentelemetry::trace::{TraceContextExt as _, Tracer as _, TracerProvider as _};
use scxml_telemetry::TraceContext;

use crate::{HostError, LifecycleLease, TelemetryProviders};

/// The parent a host-created span starts under: `context` unless its
/// active span is invalid, in which case a fresh root context.
///
/// The ambient context can hold an invalid span, for example the one the
/// `scxml-telemetry` facade opens through a no-op process-global provider
/// while the embedder passed real [`TelemetryProviders`]. The SDK would make
/// such a span the parent of an all-zero trace and drop every child, so an
/// invalid or absent ambient parent starts a root span instead: it never
/// fails and never discards the span.
pub(crate) fn span_parent(context: &Context) -> Context {
    if context.has_active_span() && !context.span().span_context().is_valid() {
        Context::new()
    } else {
        context.clone()
    }
}

/// Largest number of guest spans one store keeps open at once.
pub(crate) const MAX_ACTIVE_GUEST_SPANS: usize = 1_024;

/// Why one guest span operation was refused; the table is unchanged.
#[derive(Debug)]
pub(crate) enum GuestSpanError {
    /// [`MAX_ACTIVE_GUEST_SPANS`] spans are already open.
    Capacity,
    /// The telemetry providers refused a span lease.
    Provider(HostError),
    /// The new span has no propagatable trace context.
    Propagation,
    /// Handle allocation would overflow.
    HandleOverflow,
    /// No open span has this handle (never opened, or already ended).
    UnknownHandle,
}

impl GuestSpanError {
    /// The host error a failing capability boundary reports.
    pub(crate) fn into_host_error(self) -> HostError {
        match self {
            Self::Capacity => HostError::Component("guest span capacity exhausted".into()),
            Self::Provider(error) => error,
            Self::Propagation => HostError::Component("telemetry propagation unavailable".into()),
            Self::HandleOverflow => HostError::UnboundedExecution,
            Self::UnknownHandle => HostError::Component("unknown guest span handle".into()),
        }
    }
}

struct GuestSpan {
    context: Option<Context>,
    _lease: LifecycleLease,
}

impl Drop for GuestSpan {
    fn drop(&mut self) {
        if let Some(context) = self.context.take() {
            context.span().end();
        }
    }
}

/// Open guest spans of one store, keyed by handle.
pub(crate) struct GuestSpanTable {
    spans: BTreeMap<u64, GuestSpan>,
    next: u64,
}

impl GuestSpanTable {
    /// An empty table.
    pub(crate) fn new() -> Self {
        Self {
            spans: BTreeMap::new(),
            next: 0,
        }
    }

    /// Whether no span is open.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Opens one span named `operation` under `parent` with tracer `tracer`
    /// and returns its non-zero handle and the context to propagate.
    ///
    /// # Errors
    ///
    /// [`GuestSpanError`] without opening anything.
    pub(crate) fn start(
        &mut self,
        providers: &TelemetryProviders,
        tracer: &'static str,
        module: &'static str,
        operation: &'static str,
        parent: &Context,
    ) -> Result<(u64, TraceContext), GuestSpanError> {
        if self.spans.len() >= MAX_ACTIVE_GUEST_SPANS {
            return Err(GuestSpanError::Capacity);
        }
        let handle = self
            .next
            .checked_add(1)
            .ok_or(GuestSpanError::HandleOverflow)?;
        let lease = providers.acquire_span().map_err(GuestSpanError::Provider)?;
        let parent = span_parent(parent);
        let span = providers
            .tracer
            .tracer(tracer)
            .start_with_context(operation, &parent);
        let context = parent.with_span(span);
        context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.module", module));
        context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.operation", operation));
        let propagated = scxml_telemetry::encode_trace_context(&context);
        if propagated.traceparent.is_none() {
            context.span().end();
            return Err(GuestSpanError::Propagation);
        }
        self.next = handle;
        self.spans.insert(
            handle,
            GuestSpan {
                context: Some(context),
                _lease: lease,
            },
        );
        Ok((handle, propagated))
    }

    /// Ends the open span `handle` with the closed `outcome` label; a
    /// non-`ok` outcome also sets the error status.
    ///
    /// # Errors
    ///
    /// [`GuestSpanError::UnknownHandle`] when no open span has `handle`.
    pub(crate) fn end(&mut self, handle: u64, outcome: &'static str) -> Result<(), GuestSpanError> {
        self.close(handle, outcome, None)
    }

    /// Ends the open span `handle`: outcome `ok` when `error` is `None`,
    /// otherwise outcome `failed` with the closed `error` category as the
    /// error status and the `error.type` attribute.
    ///
    /// # Errors
    ///
    /// [`GuestSpanError::UnknownHandle`] when no open span has `handle`.
    pub(crate) fn end_with_error(
        &mut self,
        handle: u64,
        error: Option<&'static str>,
    ) -> Result<(), GuestSpanError> {
        match error {
            Some(category) => self.close(handle, "failed", Some(category)),
            None => self.close(handle, "ok", None),
        }
    }

    /// Adds the closed `event` to the open span `handle`.
    ///
    /// # Errors
    ///
    /// [`GuestSpanError::UnknownHandle`] when no open span has `handle`.
    pub(crate) fn event(&self, handle: u64, event: &'static str) -> Result<(), GuestSpanError> {
        let span = self
            .spans
            .get(&handle)
            .ok_or(GuestSpanError::UnknownHandle)?;
        if let Some(context) = span.context.as_ref() {
            context.span().add_event(event, Vec::new());
        }
        Ok(())
    }

    fn close(
        &mut self,
        handle: u64,
        outcome: &'static str,
        error_type: Option<&'static str>,
    ) -> Result<(), GuestSpanError> {
        let Some(mut span) = self.spans.remove(&handle) else {
            return Err(GuestSpanError::UnknownHandle);
        };
        if let Some(context) = span.context.take() {
            if outcome != "ok" {
                context
                    .span()
                    .set_status(opentelemetry::trace::Status::error(
                        error_type.unwrap_or(outcome),
                    ));
            }
            if let Some(category) = error_type {
                context
                    .span()
                    .set_attribute(opentelemetry::KeyValue::new("error.type", category));
            }
            context
                .span()
                .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", outcome));
            context.span().end();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};

    fn exporting_providers() -> (TelemetryProviders, InMemorySpanExporter) {
        let exporter = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (
            TelemetryProviders::from_providers(tracer, SdkMeterProvider::builder().build()),
            exporter,
        )
    }

    fn attribute(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| attribute.value.as_str().into_owned())
    }

    fn event_names(span: &SpanData) -> Vec<&str> {
        span.events
            .iter()
            .map(|event| event.name.as_ref())
            .collect()
    }

    #[test]
    fn events_and_error_categories_land_on_the_open_span() {
        let (providers, exporter) = exporting_providers();
        let mut table = GuestSpanTable::new();
        let parent = Context::new();
        let mut open = || {
            table
                .start(
                    &providers,
                    "scxml-component-guest",
                    "compiler-component",
                    "compile",
                    &parent,
                )
                .expect("a guest span opens")
                .0
        };
        let failed = open();
        let succeeded = open();
        table
            .event(failed, "input-rejected")
            .expect("an open span accepts an event");
        table
            .event(succeeded, "artifact-produced")
            .expect("an open span accepts an event");
        table
            .end_with_error(failed, Some("lowering"))
            .expect("the failed span ends once");
        table
            .end_with_error(succeeded, None)
            .expect("the successful span ends once");
        assert!(table.is_empty());
        assert!(matches!(
            table.event(failed, "input-rejected"),
            Err(GuestSpanError::UnknownHandle)
        ));
        assert!(matches!(
            table.end_with_error(failed, None),
            Err(GuestSpanError::UnknownHandle)
        ));

        let spans = exporter
            .get_finished_spans()
            .expect("in-memory spans are readable");
        assert_eq!(spans.len(), 2);
        let failed = &spans[0];
        assert_eq!(
            attribute(failed, "scxml.outcome").as_deref(),
            Some("failed")
        );
        assert_eq!(attribute(failed, "error.type").as_deref(), Some("lowering"));
        assert_eq!(
            failed.status,
            opentelemetry::trace::Status::error("lowering")
        );
        assert_eq!(event_names(failed), ["input-rejected"]);
        let succeeded = &spans[1];
        assert_eq!(attribute(succeeded, "scxml.outcome").as_deref(), Some("ok"));
        assert_eq!(attribute(succeeded, "error.type"), None);
        assert_eq!(event_names(succeeded), ["artifact-produced"]);
    }
}
