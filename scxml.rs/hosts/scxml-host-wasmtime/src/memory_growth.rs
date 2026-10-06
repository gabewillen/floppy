//! Linear memory growth, bounded by the host call's copy allowance.
//!
//! Every linear memory is one read-write mapping of exactly its size that
//! moves when it grows (`component_engine`: no reservation to grow into, so
//! a live session costs few memory mappings). Wasmtime's move maps the new
//! size and copies the whole old memory into it, and the copy runs in the
//! host, outside fuel: a guest that grows its memory a page at a time up to
//! the store's 64 MiB copies about 32 GiB. So every growth is taken from the
//! call's copy allowance ([`CallBudget`]): the call's fuel times
//! [`MEMORY_GROWTH_COPY_BYTES_PER_FUEL`] bytes, kept apart from its Wasm
//! fuel and started and shared as the fuel is, on every build. A growth
//! whose copy (the memory's size before it grows) the allowance cannot hold
//! traps as fuel exhaustion (`wasmtime::Trap::OutOfFuel`), so one call does
//! at most its fuel of Wasm and its fuel's worth of copies, and a copy never
//! takes fuel a document's Wasm would otherwise have had.

use std::sync::Arc;

use crate::call_budget::CallBudget;

/// Bytes of memory growth copies one host call may make per fuel unit of
/// its budget. At this rate the copies take no more host time than the
/// call's Wasm can. Measured by
/// `memory_growth_copies_are_bounded_by_the_calls_copy_allowance` (release, Apple M3
/// Max), which prints each call's Wasm fuel, copied bytes and time: its
/// ECMAScript loop runs 319 million fuel of Wasm, and no growth, in 30 to
/// 39 ms (95 to 121 ms per billion fuel), and its growing expression copies
/// 1.98 GB in 133 to 183 ms (67 to 93 ms per billion bytes).
pub const MEMORY_GROWTH_COPY_BYTES_PER_FUEL: u64 = 1;

/// A store's resource limits, with every linear memory growth taken from
/// the call's copy allowance.
pub(crate) struct MeteredLimits {
    limits: wasmtime::StoreLimits,
    budget: Arc<CallBudget>,
    /// Whether `budget` is the store's own (it shares no call budget).
    own: bool,
    copied_bytes: u64,
}

impl MeteredLimits {
    /// `limits`, taking growth from the copy allowance of `budget` (the call budget the store
    /// shares, or the store's own).
    pub(crate) const fn new(limits: wasmtime::StoreLimits, budget: Arc<CallBudget>) -> Self {
        Self {
            limits,
            budget,
            own: false,
            copied_bytes: 0,
        }
    }

    /// `limits` taking growth from the copy allowance of `shared`, the call budget the store
    /// shares, or, when it shares none, to a budget of its own of `fuel`
    /// per outermost call ([`Self::set_call_fuel`] follows the store's
    /// per-call fuel). Share [`Self::budget`] with the store once it exists.
    pub(crate) fn metered(
        limits: wasmtime::StoreLimits,
        shared: Option<&Arc<CallBudget>>,
        fuel: u64,
    ) -> Self {
        match shared {
            Some(shared) => Self::new(limits, Arc::clone(shared)),
            None => Self {
                own: true,
                ..Self::new(limits, CallBudget::new(fuel))
            },
        }
    }

    /// Makes the store's own budget start each outermost call with `fuel`,
    /// the store's new per-call fuel; a shared call budget is the session
    /// host's and stays as it is.
    pub(crate) fn set_call_fuel(&self, fuel: u64) {
        if self.own {
            self.budget.set_fuel(fuel);
        }
    }

    /// Restarts the store's own budget with its full copy allowance (an
    /// invoked service's store, at each input); a shared call budget is the
    /// session host's and stays as it is.
    pub(crate) fn restart(&self) {
        if self.own {
            self.budget.start();
        }
    }

    /// The budget whose copy allowance growth is taken from.
    pub(crate) const fn budget(&self) -> &Arc<CallBudget> {
        &self.budget
    }

    /// The bytes every growth of this store's memories copied so far.
    pub(crate) const fn copied_bytes(&self) -> u64 {
        self.copied_bytes
    }
}

impl wasmtime::ResourceLimiter for MeteredLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if !self.limits.memory_growing(current, desired, maximum)? {
            return Ok(false);
        }
        if !self
            .budget
            .try_copy(u64::try_from(current).unwrap_or(u64::MAX))
        {
            return Err(wasmtime::Trap::OutOfFuel.into());
        }
        self.copied_bytes = self
            .copied_bytes
            .saturating_add(u64::try_from(current).unwrap_or(u64::MAX));
        Ok(true)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.limits.memory_grow_failed(error)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }

    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.limits.table_grow_failed(error)
    }

    fn instances(&self) -> usize {
        self.limits.instances()
    }

    fn tables(&self) -> usize {
        self.limits.tables()
    }

    fn memories(&self) -> usize {
        self.limits.memories()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Every Rust source of this crate, with its path relative to `src`.
    fn sources(directory: &Path, root: &Path, found: &mut Vec<(PathBuf, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .expect("the source directory is readable")
            .map(|entry| entry.expect("the directory entry is readable").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                sources(&path, root, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).expect("the source is readable");
                let relative = path
                    .strip_prefix(root)
                    .expect("the source is under src")
                    .to_path_buf();
                found.push((relative, text));
            }
        }
    }

    /// The source a build without tests compiles: none of a test-only
    /// file, and none of a file's trailing `#[cfg(test)]` module.
    fn non_test_source<'a>(path: &Path, text: &'a str) -> &'a str {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("_tests.rs"))
        {
            return "";
        }
        text.find("#[cfg(test)]\nmod tests")
            .map_or(text, |start| &text[..start])
    }

    /// Statically, no store can miss the growth charge: only
    /// `MeteredLimits` names a bare `wasmtime::StoreLimits` or implements
    /// `ResourceLimiter`, and every store the crate creates installs a
    /// limiter, whose state (the only limiter there is) is `MeteredLimits`.
    #[test]
    fn every_store_takes_its_growth_from_a_copy_allowance() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        sources(&root, &root, &mut found);
        assert!(found.len() > 10, "the crate's sources are found");
        let this_module = Path::new("memory_growth.rs");
        for (path, text) in &found {
            if path == this_module {
                continue;
            }
            let bare_limits = text
                .match_indices("StoreLimits")
                .filter(|(at, _)| !text[at + "StoreLimits".len()..].starts_with("Builder"))
                .count();
            assert_eq!(
                bare_limits,
                0,
                "{} names a bare StoreLimits; wrap it in MeteredLimits",
                path.display()
            );
            assert!(
                !text.contains("ResourceLimiter for"),
                "{} implements ResourceLimiter; only MeteredLimits may",
                path.display()
            );
            let built = non_test_source(path, text);
            let stores = built.matches("Store::new(").count();
            let limiters = built.matches(".limiter(").count();
            assert!(
                limiters >= stores,
                "{} creates {stores} stores but installs {limiters} limiters",
                path.display()
            );
        }
    }
}
