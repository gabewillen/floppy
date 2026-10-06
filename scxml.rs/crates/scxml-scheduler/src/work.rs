//! A count of the scheduler's own work steps, for tests only.
//!
//! Each loop iteration over sessions, labels, or queue entries, and each
//! ordered-collection operation (logarithmic in its size), adds one unit, so
//! a test can bound how the work of a sequence of calls grows with its
//! input instead of timing it. Outside `cfg(test)` [`tick`] is an empty
//! inline function and the counter does not exist: release and wasm builds
//! carry no cost.

#[cfg(test)]
thread_local! {
    static UNITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Adds one work step to this thread's count.
#[cfg(test)]
pub(crate) fn tick() {
    UNITS.with(|count| count.set(count.get().saturating_add(1)));
}

/// Adds nothing: the count exists only in tests.
#[cfg(not(test))]
#[inline(always)]
pub(crate) const fn tick() {}

/// The work steps this thread counted since the last call, resetting the
/// count.
#[cfg(test)]
pub(crate) fn take() -> u64 {
    UNITS.with(std::cell::Cell::take)
}
