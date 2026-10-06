//! Deterministic logical clock and bounded delayed-send scheduler.
//!
//! [`ManualClock`] is the single writer for logical time and timer ordering.
//! Callers own timer identifiers and may not reuse a live identifier. All
//! mutation is synchronous and transactional: errors leave clock state
//! unchanged. Time and durations are unsigned milliseconds.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use std::fmt;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};

/// Maximum number of live timers owned by one clock component
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_SCHEDULES: usize = scxml_types::wasm_bound(1_024);
/// Maximum UTF-8 byte length of a caller-owned timer identifier
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_ID_BYTES: usize = scxml_types::wasm_bound(256);
/// Maximum opaque payload bytes retained for one timer (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_PAYLOAD_BYTES: usize = scxml_types::wasm_bound(65_536);
/// Maximum aggregate payload bytes retained by one component instance
/// (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_TOTAL_PAYLOAD_BYTES: usize = scxml_types::wasm_bound(4 * 1_024 * 1_024);
/// Maximum timers returned by one due-drain operation.
pub const MAX_DRAIN_ITEMS: u32 = 1_024;

/// One successfully scheduled timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scheduled {
    /// Absolute logical deadline in milliseconds.
    pub deadline_ms: u64,
    /// Monotonic insertion order used to break equal-deadline ties.
    pub sequence: u64,
}

/// One timer removed by [`ManualClock::drain_due`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DueItem {
    /// Caller-owned timer identifier.
    pub id: String,
    /// Opaque caller-owned bytes returned unchanged when the timer becomes due.
    pub payload: Vec<u8>,
    /// Absolute logical deadline in milliseconds.
    pub deadline_ms: u64,
    /// Stable insertion order.
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    id: String,
    payload: Vec<u8>,
    deadline_ms: u64,
    sequence: u64,
}

/// Deterministic clock-domain failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockError {
    /// A timer identifier was empty or exceeded [`MAX_ID_BYTES`].
    InvalidId,
    /// A zero duration was supplied where forward progress is required.
    InvalidDuration,
    /// One payload or the aggregate retained payload bytes exceeded its bound.
    PayloadTooLarge,
    /// A live timer already uses the caller-owned identifier.
    DuplicateId,
    /// The bounded timer queue is full.
    CapacityExhausted,
    /// Logical time, deadline, or insertion sequence overflowed.
    Overflow,
    /// A due-drain batch limit was zero or exceeded [`MAX_DRAIN_ITEMS`].
    InvalidLimit,
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "timer id must be non-empty and within the byte bound",
            Self::InvalidDuration => "duration must be greater than zero",
            Self::PayloadTooLarge => "timer payload exceeds its byte bound",
            Self::DuplicateId => "timer id is already scheduled",
            Self::CapacityExhausted => "timer queue capacity is exhausted",
            Self::Overflow => "logical clock arithmetic overflowed",
            Self::InvalidLimit => "due-drain limit is outside the supported bound",
        })
    }
}

impl std::error::Error for ClockError {}

/// Single-owner deterministic logical clock and timer queue.
///
/// The queue is always sorted by `(deadline_ms, sequence)`. Equal deadlines
/// therefore drain in schedule-call order. This type performs no I/O and reads
/// no ambient clock, environment, filesystem, network, or random source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualClock {
    now_ms: u64,
    next_sequence: u64,
    entries: Vec<Entry>,
}

impl ManualClock {
    /// Constructs an empty clock at the supplied logical millisecond.
    #[must_use]
    pub fn new(initial_ms: u64) -> Self {
        Self {
            now_ms: initial_ms,
            next_sequence: 0,
            entries: Vec::new(),
        }
    }

    /// Returns current logical time in milliseconds without mutation.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Returns the number of live timers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no timers are live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Earliest live deadline in milliseconds, or `None` when no timer is
    /// live. The queue is sorted, so this is the next timer to become due.
    #[must_use]
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.entries.first().map(|entry| entry.deadline_ms)
    }

    /// Every live timer as `(id, payload, deadline_ms)` in queue order
    /// (deadline, then schedule order), without mutation.
    pub fn timers(&self) -> impl Iterator<Item = (&str, &[u8], u64)> {
        self.entries.iter().map(|entry| {
            (
                entry.id.as_str(),
                entry.payload.as_slice(),
                entry.deadline_ms,
            )
        })
    }

    /// Schedules one caller-identified timer relative to current logical time.
    ///
    /// # Errors
    ///
    /// Rejects invalid or duplicate identifiers, zero durations, full capacity,
    /// and arithmetic overflow. Failure does not mutate clock state.
    pub fn schedule(
        &mut self,
        id: String,
        payload: Vec<u8>,
        delay_ms: u64,
    ) -> Result<Scheduled, ClockError> {
        let span = observe(Module::ClockComponent, Operation::ClockSchedule);
        let result = (|| {
            validate_id(&id)?;
            if delay_ms == 0 {
                return Err(ClockError::InvalidDuration);
            }
            if scxml_types::exceeds_wasm_bound(self.entries.len().saturating_add(1), MAX_SCHEDULES)
            {
                return Err(ClockError::CapacityExhausted);
            }
            if scxml_types::exceeds_wasm_bound(payload.len(), MAX_PAYLOAD_BYTES) {
                return Err(ClockError::PayloadTooLarge);
            }
            let retained_bytes = self.entries.iter().try_fold(0_usize, |total, entry| {
                total.checked_add(entry.payload.len())
            });
            if retained_bytes
                .and_then(|total| total.checked_add(payload.len()))
                .is_none_or(|total| scxml_types::exceeds_wasm_bound(total, MAX_TOTAL_PAYLOAD_BYTES))
            {
                return Err(ClockError::PayloadTooLarge);
            }
            if self.entries.iter().any(|entry| entry.id == id) {
                return Err(ClockError::DuplicateId);
            }
            let deadline_ms = self
                .now_ms
                .checked_add(delay_ms)
                .ok_or(ClockError::Overflow)?;
            let sequence = self.next_sequence;
            let next_sequence = sequence.checked_add(1).ok_or(ClockError::Overflow)?;
            let insertion = self.entries.partition_point(|entry| {
                (entry.deadline_ms, entry.sequence) < (deadline_ms, sequence)
            });
            self.entries.insert(
                insertion,
                Entry {
                    id,
                    payload,
                    deadline_ms,
                    sequence,
                },
            );
            self.next_sequence = next_sequence;
            Ok(Scheduled {
                deadline_ms,
                sequence,
            })
        })();
        if result.is_err() {
            span.event(SignalEvent::ClockRejected);
            span.fail(ErrorCategory::InvalidInput);
        } else {
            span.event(SignalEvent::ClockScheduled);
        }
        result
    }

    /// Cancels a live timer by caller-owned identifier.
    ///
    /// Returns `false` without mutation when the identifier is valid but absent.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identifier without mutating state.
    pub fn cancel(&mut self, id: &str) -> Result<bool, ClockError> {
        let span = observe(Module::ClockComponent, Operation::ClockCancel);
        let result = validate_id(id).map(|()| {
            self.entries
                .iter()
                .position(|entry| entry.id == id)
                .is_some_and(|position| {
                    self.entries.remove(position);
                    true
                })
        });
        match result {
            Ok(true) => span.event(SignalEvent::ClockCancelled),
            Ok(false) => {}
            Err(_) => {
                span.event(SignalEvent::ClockRejected);
                span.fail(ErrorCategory::InvalidInput);
            }
        }
        result
    }

    /// Removes at most `max_items` timers whose deadlines are not after `now`.
    ///
    /// Returned timers are ordered by deadline and then stable insertion order.
    ///
    /// # Errors
    ///
    /// Rejects zero or over-bound batch limits without mutating state.
    pub fn drain_due(&mut self, max_items: u32) -> Result<Vec<DueItem>, ClockError> {
        let span = observe(Module::ClockComponent, Operation::ClockDrainDue);
        if max_items == 0 || max_items > MAX_DRAIN_ITEMS {
            span.event(SignalEvent::ClockRejected);
            span.fail(ErrorCategory::InvalidInput);
            return Err(ClockError::InvalidLimit);
        }
        let limit = usize::try_from(max_items).map_err(|_| ClockError::InvalidLimit)?;
        let due_count = self
            .entries
            .iter()
            .take_while(|entry| entry.deadline_ms <= self.now_ms)
            .take(limit)
            .count();
        let due = self
            .entries
            .drain(..due_count)
            .map(|entry| DueItem {
                id: entry.id,
                payload: entry.payload,
                deadline_ms: entry.deadline_ms,
                sequence: entry.sequence,
            })
            .collect();
        span.event(SignalEvent::ClockDueDrained);
        Ok(due)
    }

    /// Advances logical time by a positive millisecond duration.
    ///
    /// # Errors
    ///
    /// Rejects zero and arithmetic overflow without mutating state.
    pub fn advance(&mut self, delta_ms: u64) -> Result<u64, ClockError> {
        let span = observe(Module::ClockComponent, Operation::ClockAdvance);
        let result = if delta_ms == 0 {
            Err(ClockError::InvalidDuration)
        } else {
            self.now_ms
                .checked_add(delta_ms)
                .ok_or(ClockError::Overflow)
        };
        match result {
            Ok(now_ms) => {
                self.now_ms = now_ms;
                span.event(SignalEvent::ClockAdvanced);
                Ok(now_ms)
            }
            Err(error) => {
                span.event(SignalEvent::ClockRejected);
                span.fail(ErrorCategory::InvalidInput);
                Err(error)
            }
        }
    }
}

fn validate_id(id: &str) -> Result<(), ClockError> {
    if id.is_empty() || scxml_types::exceeds_wasm_bound(id.len(), MAX_ID_BYTES) {
        Err(ClockError::InvalidId)
    } else {
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    use std::cell::RefCell;

    use super::{ClockError, ManualClock};

    wit_bindgen::generate!({
        world: "clock",
        path: "../../crates/scxml-wit/wit/scxml/deps/clock",
    });

    use exports::stateforward::scxml_clock::clock_api::{ClockSnapshot, Guest, Timer};
    use stateforward::scxml_clock::telemetry;
    use stateforward::scxml_clock::types::{
        BaggagePolicy, ClockError as WitClockError, DueItem, Scheduled, TraceContext,
    };

    struct Component;

    thread_local! {
        // The one virtual clock and schedule of this component instance.
        // The Component Model instance is its single owner: a guest runs on
        // one thread, and no state crosses threads or instances. Each export
        // borrows it for that call only. The telemetry imports it calls
        // cannot re-enter the component, so no borrow ever overlaps.
        static CLOCK: RefCell<ManualClock> = RefCell::new(ManualClock::new(0));
    }

    fn to_wit_error(error: ClockError) -> WitClockError {
        match error {
            ClockError::InvalidId => WitClockError::InvalidId,
            ClockError::InvalidDuration => WitClockError::InvalidDuration,
            ClockError::PayloadTooLarge => WitClockError::PayloadTooLarge,
            ClockError::DuplicateId => WitClockError::DuplicateId,
            ClockError::CapacityExhausted => WitClockError::CapacityExhausted,
            ClockError::Overflow => WitClockError::Overflow,
            ClockError::InvalidLimit => WitClockError::InvalidLimit,
        }
    }

    impl Guest for Component {
        fn now() -> u64 {
            let (now_ms, depth) = CLOCK.with(|clock| {
                let clock = clock.borrow();
                (
                    clock.now_ms(),
                    u32::try_from(clock.len()).unwrap_or(u32::MAX),
                )
            });
            telemetry::observe(
                telemetry::Operation::Now,
                telemetry::Outcome::Ok,
                Some(queue_depth(depth)),
                None,
            );
            now_ms
        }

        fn next_deadline() -> Option<u64> {
            // Pure: no telemetry (WIT contract).
            CLOCK.with(|clock| clock.borrow().next_deadline_ms())
        }

        fn schedule(
            id: String,
            payload: Vec<u8>,
            delay_ms: u64,
            context: Option<TraceContext>,
        ) -> Result<Scheduled, WitClockError> {
            let context = match validate_context(context) {
                Ok(context) => context,
                Err(error) => {
                    telemetry::observe(
                        telemetry::Operation::Schedule,
                        telemetry::Outcome::Rejected,
                        None,
                        None,
                    );
                    telemetry::event(
                        telemetry::Operation::Schedule,
                        telemetry::SignalEvent::Rejected,
                        None,
                    );
                    return Err(error);
                }
            };
            let (result, depth) = CLOCK.with(|clock| {
                let mut clock = clock.borrow_mut();
                let result = clock
                    .schedule(id, payload, delay_ms)
                    .map(|scheduled| Scheduled {
                        deadline_ms: scheduled.deadline_ms,
                        sequence: scheduled.sequence,
                    });
                (result, u32::try_from(clock.len()).unwrap_or(u32::MAX))
            });
            let (outcome, event) = if result.is_ok() {
                (telemetry::Outcome::Ok, telemetry::SignalEvent::Scheduled)
            } else {
                (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::Rejected,
                )
            };
            telemetry::observe(
                telemetry::Operation::Schedule,
                outcome,
                Some(queue_depth(depth)),
                context.as_ref(),
            );
            telemetry::event(telemetry::Operation::Schedule, event, context.as_ref());
            result.map_err(to_wit_error)
        }

        fn cancel(id: String, context: Option<TraceContext>) -> Result<bool, WitClockError> {
            let context = match validate_context(context) {
                Ok(context) => context,
                Err(error) => {
                    telemetry::observe(
                        telemetry::Operation::Cancel,
                        telemetry::Outcome::Rejected,
                        None,
                        None,
                    );
                    telemetry::event(
                        telemetry::Operation::Cancel,
                        telemetry::SignalEvent::Rejected,
                        None,
                    );
                    return Err(error);
                }
            };
            let (result, depth) = CLOCK.with(|clock| {
                let mut clock = clock.borrow_mut();
                let result = clock.cancel(&id);
                (result, u32::try_from(clock.len()).unwrap_or(u32::MAX))
            });
            let (outcome, event) = match result {
                Ok(true) => (
                    telemetry::Outcome::Ok,
                    Some(telemetry::SignalEvent::Cancelled),
                ),
                Ok(false) => (telemetry::Outcome::Ok, None),
                Err(_) => (
                    telemetry::Outcome::Rejected,
                    Some(telemetry::SignalEvent::Rejected),
                ),
            };
            telemetry::observe(
                telemetry::Operation::Cancel,
                outcome,
                Some(queue_depth(depth)),
                context.as_ref(),
            );
            if let Some(event) = event {
                telemetry::event(telemetry::Operation::Cancel, event, context.as_ref());
            }
            result.map_err(to_wit_error)
        }

        fn drain_due(max_items: u32) -> Result<Vec<DueItem>, WitClockError> {
            let (result, depth) = CLOCK.with(|clock| {
                let mut clock = clock.borrow_mut();
                let result = clock.drain_due(max_items).map(|items| {
                    items
                        .into_iter()
                        .map(|item| DueItem {
                            id: item.id,
                            payload: item.payload,
                            deadline_ms: item.deadline_ms,
                            sequence: item.sequence,
                        })
                        .collect()
                });
                (result, u32::try_from(clock.len()).unwrap_or(u32::MAX))
            });
            let (outcome, event) = if result.is_ok() {
                (telemetry::Outcome::Ok, telemetry::SignalEvent::DueDrained)
            } else {
                (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::Rejected,
                )
            };
            telemetry::observe(
                telemetry::Operation::DrainDue,
                outcome,
                Some(queue_depth(depth)),
                None,
            );
            telemetry::event(telemetry::Operation::DrainDue, event, None);
            result.map_err(to_wit_error)
        }

        // Read-only: copies logical time and the live timers in queue order
        // and records no telemetry of its own (the host's session-snapshot
        // span covers the call).
        fn snapshot() -> ClockSnapshot {
            CLOCK.with_borrow(|clock| ClockSnapshot {
                now_ms: clock.now_ms(),
                timers: clock
                    .timers()
                    .map(|(id, payload, deadline_ms)| Timer {
                        id: id.to_owned(),
                        payload: payload.to_vec(),
                        deadline_ms,
                    })
                    .collect(),
            })
        }

        fn advance(delta_ms: u64) -> Result<u64, WitClockError> {
            let (result, depth) = CLOCK.with(|clock| {
                let mut clock = clock.borrow_mut();
                let result = clock.advance(delta_ms);
                (result, u32::try_from(clock.len()).unwrap_or(u32::MAX))
            });
            let (outcome, event) = if result.is_ok() {
                (telemetry::Outcome::Ok, telemetry::SignalEvent::Advanced)
            } else {
                (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::Rejected,
                )
            };
            telemetry::observe(
                telemetry::Operation::Advance,
                outcome,
                Some(queue_depth(depth)),
                None,
            );
            telemetry::event(telemetry::Operation::Advance, event, None);
            result.map_err(to_wit_error)
        }
    }

    fn validate_context(
        context: Option<TraceContext>,
    ) -> Result<Option<TraceContext>, WitClockError> {
        context
            .map(|context| {
                scxml_types::TraceContext::new(
                    context.traceparent,
                    context.tracestate,
                    context.baggage,
                    match context.baggage_policy {
                        BaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                        BaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                    },
                )
                .map_err(|_| WitClockError::InvalidContext)
                .map(|validated| TraceContext {
                    traceparent: validated.traceparent,
                    tracestate: validated.tracestate,
                    baggage: validated.baggage,
                    baggage_policy: match validated.baggage_policy {
                        scxml_types::BaggagePolicy::Drop => BaggagePolicy::Drop,
                        scxml_types::BaggagePolicy::Propagate => BaggagePolicy::Propagate,
                    },
                })
            })
            .transpose()
    }

    fn queue_depth(value: u32) -> telemetry::Measurement {
        telemetry::Measurement {
            kind: telemetry::MeasurementKind::QueueDepth,
            value: u64::from(value),
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical WIT prose (doc comments joined into single-spaced text); its
    /// documented bounds must equal the constants this component enforces.
    fn clock_wit_docs() -> String {
        include_str!("../../../crates/scxml-wit/wit/scxml/deps/clock/clock.wit")
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("///"))
            .flat_map(str::split_whitespace)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The bounds `clock.wit` documents for the wasm component; natively
    /// each is unbounded ([`scxml_types::wasm_bound`]).
    const DOCUMENTED_ID_BYTES: usize = 256;
    const DOCUMENTED_PAYLOAD_BYTES: usize = 65_536;
    const DOCUMENTED_TOTAL_PAYLOAD_BYTES: usize = 4 * 1_024 * 1_024;
    const DOCUMENTED_SCHEDULES: usize = 1_024;

    #[test]
    fn documented_wit_bounds_match_enforced_constants() {
        use scxml_types::wasm_bound;
        assert_eq!(MAX_ID_BYTES, wasm_bound(DOCUMENTED_ID_BYTES));
        assert_eq!(MAX_PAYLOAD_BYTES, wasm_bound(DOCUMENTED_PAYLOAD_BYTES));
        assert_eq!(
            MAX_TOTAL_PAYLOAD_BYTES,
            wasm_bound(DOCUMENTED_TOTAL_PAYLOAD_BYTES)
        );
        assert_eq!(MAX_SCHEDULES, wasm_bound(DOCUMENTED_SCHEDULES));
        let docs = clock_wit_docs();
        for phrase in [
            format!("longer than {DOCUMENTED_ID_BYTES} bytes"),
            format!("`id` is 1..={DOCUMENTED_ID_BYTES} bytes"),
            format!("One payload exceeded {DOCUMENTED_PAYLOAD_BYTES} bytes"),
            format!("`payload` is at most {DOCUMENTED_PAYLOAD_BYTES} bytes"),
            format!("would exceed {DOCUMENTED_TOTAL_PAYLOAD_BYTES} bytes in aggregate"),
            format!("stay within {DOCUMENTED_TOTAL_PAYLOAD_BYTES} bytes"),
            format!("already holds {DOCUMENTED_SCHEDULES} live timers"),
            format!("fewer than {DOCUMENTED_SCHEDULES} timers are live"),
            format!("greater than {MAX_DRAIN_ITEMS}"),
            format!("1 <= `max-items` <= {MAX_DRAIN_ITEMS}"),
        ] {
            assert!(
                docs.contains(&phrase),
                "clock.wit must document the enforced bound: {phrase:?}"
            );
        }
    }

    #[test]
    fn equal_deadlines_are_stable_and_cancellation_is_exact() {
        let mut clock = ManualClock::new(10);
        let first = clock
            .schedule("first".into(), b"first-payload".to_vec(), 5)
            .expect("first timer fits");
        let second = clock
            .schedule("second".into(), b"second-payload".to_vec(), 5)
            .expect("second timer fits");
        clock
            .schedule("cancelled".into(), b"cancelled-payload".to_vec(), 3)
            .expect("third timer fits");
        assert!(clock.cancel("cancelled").expect("valid id can cancel"));
        assert_eq!(clock.advance(5), Ok(15));
        let due = clock.drain_due(8).expect("bounded drain succeeds");
        assert_eq!(
            due.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(due[0].payload, b"first-payload");
        assert_eq!(due[1].payload, b"second-payload");
        assert!(first.sequence < second.sequence);
    }

    #[test]
    fn timers_list_live_timers_in_drain_order_without_mutation() {
        let mut clock = ManualClock::new(100);
        for (id, delay) in [("late", 50), ("tie-a", 10), ("tie-b", 10), ("gone", 5)] {
            clock
                .schedule(id.into(), id.as_bytes().to_vec(), delay)
                .expect("timer fits");
        }
        assert!(clock.cancel("gone").expect("valid id can cancel"));
        let before = clock.clone();
        let timers: Vec<(&str, &[u8], u64)> = clock.timers().collect();
        assert_eq!(
            timers,
            [
                ("tie-a", b"tie-a".as_slice(), 110),
                ("tie-b", b"tie-b".as_slice(), 110),
                ("late", b"late".as_slice(), 150),
            ]
        );
        assert_eq!(clock, before, "listing timers changes nothing");
        assert_eq!(ManualClock::new(0).timers().count(), 0);
    }

    #[test]
    fn failures_are_transactional_and_bounds_fail_closed() {
        let mut clock = ManualClock::new(u64::MAX - 1);
        let before = clock.clone();
        assert_eq!(
            clock.schedule("overflow".into(), Vec::new(), 2),
            Err(ClockError::Overflow)
        );
        assert_eq!(clock, before);
        assert_eq!(
            clock.schedule(String::new(), Vec::new(), 1),
            Err(ClockError::InvalidId)
        );
        assert_eq!(clock.advance(0), Err(ClockError::InvalidDuration));
        assert_eq!(clock.drain_due(0), Err(ClockError::InvalidLimit));
        assert_eq!(
            clock.drain_due(MAX_DRAIN_ITEMS + 1),
            Err(ClockError::InvalidLimit)
        );
        // The identifier and payload bounds apply to wasm builds only; the
        // host's component tests check them through the shipped clock
        // (`a_delayed_send_over_the_clock_id_or_payload_bound_raises_error_execution`).
    }

    /// Natively the clock has no capacity, identifier, or payload bound:
    /// one past each wasm bound schedules, so a native clock never raises
    /// `error.execution` for capacity.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_clocks_have_no_capacity_or_size_bound() {
        let mut clock = ManualClock::new(0);
        for index in 0..=DOCUMENTED_SCHEDULES {
            clock
                .schedule(format!("timer-{index}"), Vec::new(), 1)
                .expect("a native clock has no capacity bound");
        }
        clock
            .schedule("x".repeat(DOCUMENTED_ID_BYTES + 1), Vec::new(), 1)
            .expect("a native clock has no identifier bound");
        for index in 0..=DOCUMENTED_TOTAL_PAYLOAD_BYTES / DOCUMENTED_PAYLOAD_BYTES {
            clock
                .schedule(
                    format!("payload-{index}"),
                    vec![0; DOCUMENTED_PAYLOAD_BYTES + 1],
                    1,
                )
                .expect("a native clock has no payload bound");
        }
    }

    #[test]
    fn capacity_duplicates_and_sequence_overflow_fail_without_mutation() {
        // The capacity bound applies to wasm builds only; the host's
        // component tests check it through the shipped clock
        // (`a_delayed_send_beyond_the_clock_capacity_raises_error_execution`).
        let mut duplicate = ManualClock::new(0);
        duplicate
            .schedule("same".into(), Vec::new(), 1)
            .expect("timer fits");
        let before = duplicate.clone();
        assert_eq!(
            duplicate.schedule("same".into(), Vec::new(), 2),
            Err(ClockError::DuplicateId)
        );
        assert_eq!(duplicate, before);

        let mut exhausted_sequence = ManualClock {
            next_sequence: u64::MAX,
            ..ManualClock::new(1)
        };
        let before = exhausted_sequence.clone();
        assert_eq!(
            exhausted_sequence.schedule("cannot-sequence".into(), Vec::new(), 1),
            Err(ClockError::Overflow)
        );
        assert_eq!(exhausted_sequence, before);
    }
}
