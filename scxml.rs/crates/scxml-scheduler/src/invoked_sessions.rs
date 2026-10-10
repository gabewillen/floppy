//! A root session, the sessions it invoked (SCXML 1.0 section 6.4), at any
//! nested invoke depth, and the external event queue of each (SCXML 4.2 and
//! Appendix D).
//!
//! A session is addressed by its [`SessionKey`], assigned when it is
//! registered; its path of invoke ids from the root session
//! ([`InvokedSessions::path`]) is never stored. Invocation order is the
//! pre-order of the invoke tree: the root session, then each invoked
//! session followed by the sessions it invoked, siblings in ascending
//! invoke-id order (the lexicographic order of the paths). Each session
//! takes its place in that order when it is registered (an integer label,
//! see `pre_order`), so comparing two sessions costs one integer
//! comparison and the sessions with queued work stay in that order without
//! sorting.
//!
//! Every operation is iterative, and its work does not grow with the
//! nesting depth: registering a session costs a lookup among its siblings
//! (plus an amortized logarithmic relabeling), taking the next queued entry
//! is logarithmic in the number of sessions with queued work, and
//! cancelling or removing a session costs work proportional to its
//! subtree. A chain of any depth is handled without native stack per
//! level.
//!
//! The SCXML Event I/O Processor delivers across sessions by pushing onto
//! the target session's external queue when the event is sent: an event
//! for an invoked session (`#_<invokeid>`, autoforward) and an event for the
//! invoking session (`#_parent`, `done.invoke.<id>`). The internal event
//! queue belongs to the machine and never appears here.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::work::tick;

use crate::pre_order::{Position, PreOrder};
use crate::{Reading, Violation};

/// Invoke limits an embedder chooses for one root session. An invocation
/// beyond them cannot start; the host reports that to the invoking session
/// as the failure of that `<invoke>` (`error.execution`). The default sets
/// no limit: the host imposes none of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InvokeLimits {
    /// Deepest nested invoke depth: a session the root session invokes is at
    /// depth 1, a session that one invokes at depth 2, and so on. `None`
    /// sets no limit; `Some(0)` lets no session invoke.
    pub max_invoke_depth: Option<usize>,
    /// Most invoked sessions alive under the root session at once, at every
    /// depth (the root session itself not counted). `None` sets no limit;
    /// `Some(0)` lets no session invoke.
    pub max_invoked_sessions: Option<usize>,
}

/// One session of a run: the root session ([`SessionKey::ROOT`]) or an
/// invoked session, from its registration until its removal. Keys are
/// never reused within one [`InvokedSessions`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionKey(u64);

impl SessionKey {
    /// The root session.
    pub const ROOT: Self = Self(0);

    /// Whether this is the root session.
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }
}

/// One message an invoking session receives from a session it invoked
/// (SCXML 6.4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invoked<E, V> {
    /// An event the invoked session sent to `#_parent`.
    Event(E),
    /// The invoked session reached a top-level final state: the invoking
    /// session receives `done.invoke.<id>` carrying its return data, if any.
    Done(Option<V>),
    /// The invocation failed after it started (an invoked service whose
    /// future resolved `err`, that trapped, or that returned an invalid
    /// event): the invoking session receives `error.communication.<id>`
    /// carrying the invoke id (WIT `invocation-message.communication-error`). The
    /// caller begins the invocation's cancellation right after queueing it,
    /// so no `done.invoke` follows.
    CommunicationError,
}

/// One entry of a session's external event queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Delivery<K, E, V> {
    /// An external event (from the invoking session, autoforward, or the
    /// session's own delayed send).
    External(E),
    /// A message from the invoked session `invoke_id`.
    FromInvocation {
        /// The invoke id of the sender, as its invoking session knows it.
        invoke_id: K,
        /// The message.
        message: Invoked<E, V>,
    },
}

/// One session: the root session or an invoked session.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Node<K, E, V> {
    /// The invoking session and the invoke id it gave this one (`None`
    /// for the root).
    parent: Option<(SessionKey, K)>,
    /// Nested invoke depth (0 for the root).
    depth: usize,
    /// The sessions this one invoked, by invoke id.
    children: BTreeMap<K, SessionKey>,
    /// The session's external event queue.
    external: VecDeque<Delivery<K, E, V>>,
    /// Cancellation of this session (or an ancestor) began: its sends to
    /// its invoking session are ignored from now on (official case 252).
    cancelled: bool,
    /// Whether its `done.invoke` was already queued on its invoking session.
    done_reported: bool,
    /// Whether the executor read it as finished (never for the root): it
    /// runs nothing more, so its queue is not indexed.
    finished: bool,
    /// Where it opens in the invocation order.
    open: Position,
    /// Where its subtree ends in the invocation order.
    close: Position,
}

impl<K, E, V> Node<K, E, V> {
    fn new(
        parent: Option<(SessionKey, K)>,
        depth: usize,
        (open, close): (Position, Position),
    ) -> Self {
        Self {
            parent,
            depth,
            children: BTreeMap::new(),
            external: VecDeque::new(),
            cancelled: false,
            done_reported: false,
            finished: false,
            open,
            close,
        }
    }
}

/// A root session and every session it invoked, keyed by [`SessionKey`],
/// their external event queues, and the executor's indexes over them.
///
/// # Ownership
///
/// The [`crate::Scheduler`] owns the sessions and lends them mutably to every
/// [`crate::Sessions`] command, so a session's effects can push onto another
/// session's queue and register or remove the sessions it invokes. Nothing
/// else holds it.
///
/// # Invariants
///
/// * The root ([`SessionKey::ROOT`]) is always present; every other
///   session's invoking session is present and lists it as its child.
/// * At most [`InvokeLimits::max_invoked_sessions`] invoked sessions, none deeper than
///   [`InvokeLimits::max_invoke_depth`], when the embedder set them.
/// * `queued` holds exactly the sessions not read as finished whose
///   external queue is not empty, each with the current label of its
///   opening; every index entry names a present session.
/// * `order` holds exactly the open and close positions of the present
///   sessions, in invocation order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvokedSessions<K, E, V> {
    nodes: BTreeMap<SessionKey, Node<K, E, V>>,
    next_key: u64,
    limits: InvokeLimits,
    /// The invocation order of the sessions.
    order: PreOrder,
    /// Sessions with queued work, by the label of their opening.
    queued: BTreeSet<(u64, SessionKey)>,
    /// Sessions a command ran on since the executor last looked
    /// ([`Self::touch`]).
    touched: BTreeSet<SessionKey>,
    /// The pending deadline of each unfinished session that has one, as
    /// the executor last read it.
    deadlines: BTreeMap<SessionKey, u64>,
    /// `deadlines` ordered by deadline.
    by_deadline: BTreeSet<(u64, SessionKey)>,
    /// Finished sessions whose `done.invoke` is not yet decided.
    finished: BTreeSet<SessionKey>,
}

impl<K: Ord + Clone, E, V> Default for InvokedSessions<K, E, V> {
    fn default() -> Self {
        Self::new(InvokeLimits::default())
    }
}

impl<K: Ord + Clone, E, V> InvokedSessions<K, E, V> {
    /// Only the root session, with an empty queue, under `limits`.
    #[must_use]
    pub fn new(limits: InvokeLimits) -> Self {
        let (order, open, close) = PreOrder::new(SessionKey::ROOT);
        Self {
            nodes: BTreeMap::from([(SessionKey::ROOT, Node::new(None, 0, (open, close)))]),
            next_key: 1,
            limits,
            order,
            queued: BTreeSet::new(),
            touched: BTreeSet::from([SessionKey::ROOT]),
            deadlines: BTreeMap::new(),
            by_deadline: BTreeSet::new(),
            finished: BTreeSet::new(),
        }
    }

    /// The invoke limits.
    #[must_use]
    pub const fn limits(&self) -> InvokeLimits {
        self.limits
    }

    /// Whether the session `key` is present.
    #[must_use]
    pub fn contains(&self, key: SessionKey) -> bool {
        self.nodes.contains_key(&key)
    }

    /// Number of sessions: the root session and every invoked session.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Number of invoked sessions, at every depth (the root session not
    /// counted).
    #[must_use]
    pub fn invoked_len(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }

    /// Always false: the root is always present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every session in invocation order (the root first).
    #[must_use]
    pub fn keys(&self) -> Vec<SessionKey> {
        self.subtree(SessionKey::ROOT)
    }

    /// The session `key` and every session below it, in invocation order;
    /// empty when `key` is absent.
    #[must_use]
    pub fn subtree(&self, key: SessionKey) -> Vec<SessionKey> {
        let mut order = Vec::new();
        let mut stack = vec![key];
        while let Some(next) = stack.pop() {
            tick();
            let Some(node) = self.nodes.get(&next) else {
                continue;
            };
            order.push(next);
            stack.extend(node.children.values().rev().copied());
        }
        order
    }

    /// The session `parent` invoked as `invoke_id`, when present.
    #[must_use]
    pub fn child(&self, parent: SessionKey, invoke_id: &K) -> Option<SessionKey> {
        self.nodes.get(&parent)?.children.get(invoke_id).copied()
    }

    /// The sessions `key` invoked, by ascending invoke id.
    pub fn children(&self, key: SessionKey) -> impl Iterator<Item = (&K, SessionKey)> {
        self.nodes
            .get(&key)
            .into_iter()
            .flat_map(|node| node.children.iter().map(|(id, child)| (id, *child)))
    }

    /// The invoking session of `key` and the invoke id it gave `key`;
    /// `None` for the root or an absent session.
    #[must_use]
    pub fn parent(&self, key: SessionKey) -> Option<(SessionKey, &K)> {
        self.nodes
            .get(&key)?
            .parent
            .as_ref()
            .map(|(parent, id)| (*parent, id))
    }

    /// The nested invoke depth of `key` (0 for the root).
    #[must_use]
    pub fn depth(&self, key: SessionKey) -> Option<usize> {
        self.nodes.get(&key).map(|node| node.depth)
    }

    /// The invoke ids from the root to `key` (empty for the root): work and
    /// memory proportional to its depth.
    #[must_use]
    pub fn path(&self, key: SessionKey) -> Option<Vec<K>> {
        let mut path = Vec::new();
        let mut current = self.nodes.get(&key)?;
        while let Some((parent, id)) = &current.parent {
            tick();
            path.push(id.clone());
            current = self.nodes.get(parent)?;
        }
        path.reverse();
        Some(path)
    }

    /// Whether the invoke ids from the root to `key` are exactly `path`,
    /// without building them: a path of another length than the session's
    /// depth is no match at once, and the ids are compared walking up from
    /// `key`, stopping at the first that differs.
    #[must_use]
    pub fn has_path(&self, key: SessionKey, path: &[K]) -> bool {
        let Some(mut node) = self.nodes.get(&key) else {
            return false;
        };
        if node.depth != path.len() {
            return false;
        }
        let mut remaining = path;
        while let Some((parent, id)) = &node.parent {
            tick();
            let Some((last, rest)) = remaining.split_last() else {
                return false;
            };
            if last != id {
                return false;
            }
            remaining = rest;
            let Some(next) = self.nodes.get(parent) else {
                return false;
            };
            node = next;
        }
        remaining.is_empty()
    }

    /// Whether `first` comes before `second` in invocation order (both
    /// present): an invoking session before every session below it,
    /// siblings by ascending invoke id. One integer comparison.
    #[must_use]
    pub fn precedes(&self, first: SessionKey, second: SessionKey) -> bool {
        match (self.rank(first), self.rank(second)) {
            (Some(first), Some(second)) => first < second,
            _ => false,
        }
    }

    /// The current label of the opening of `key`.
    fn rank(&self, key: SessionKey) -> Option<u64> {
        self.nodes.get(&key).map(|node| self.order.label(node.open))
    }

    /// Moves the index entries of the sessions whose opening a relabeling
    /// moved to their new labels.
    fn follow(&mut self, moved: Vec<(SessionKey, u64)>) {
        for (key, old) in moved {
            tick();
            if self.queued.remove(&(old, key))
                && let Some(label) = self.rank(key)
            {
                self.queued.insert((label, key));
            }
        }
    }

    /// Indexes the queue of `key` when it holds work it can still run.
    fn index_queued(&mut self, key: SessionKey) {
        if let Some(node) = self.nodes.get(&key)
            && !node.finished
            && !node.external.is_empty()
        {
            self.queued.insert((self.order.label(node.open), key));
        }
    }

    /// Whether the session `key` may invoke one more session within the
    /// invoke limits.
    #[must_use]
    pub fn can_invoke(&self, key: SessionKey) -> bool {
        self.nodes.get(&key).is_some_and(|node| {
            self.limits
                .max_invoke_depth
                .is_none_or(|limit| node.depth < limit)
        }) && self
            .limits
            .max_invoked_sessions
            .is_none_or(|limit| self.invoked_len() < limit)
    }

    /// Registers the session `id` invoked by `parent`, with an empty queue,
    /// and returns its key. Register immediately before starting it, so
    /// events it sends while starting have a place to go.
    ///
    /// # Errors
    ///
    /// [`Violation::InvokeLimit`] beyond the invoke limits,
    /// [`Violation::SessionMissing`] when `parent` is absent, and
    /// [`Violation::DuplicateInvocation`] when `parent` already has a live
    /// session `id`.
    pub fn register(&mut self, parent: SessionKey, id: K) -> Result<SessionKey, Violation> {
        tick();
        let Some(node) = self.nodes.get(&parent) else {
            return Err(Violation::SessionMissing);
        };
        if node.children.contains_key(&id) {
            return Err(Violation::DuplicateInvocation);
        }
        if !self.can_invoke(parent) {
            return Err(Violation::InvokeLimit);
        }
        let depth = node.depth.saturating_add(1);
        // Pre-order: right before the next sibling's subtree, else at the
        // end of the invoking session's subtree.
        let anchor = node
            .children
            .range((std::ops::Bound::Excluded(&id), std::ops::Bound::Unbounded))
            .next()
            .and_then(|(_, sibling)| self.nodes.get(sibling))
            .map_or(node.close, |sibling| sibling.open);
        let key = SessionKey(self.next_key);
        self.next_key = self.next_key.saturating_add(1);
        let mut moved = Vec::new();
        let open = self.order.insert_before(anchor, Some(key), &mut moved);
        let close = self.order.insert_before(anchor, None, &mut moved);
        if let Some(node) = self.nodes.get_mut(&parent) {
            node.children.insert(id.clone(), key);
        }
        self.nodes
            .insert(key, Node::new(Some((parent, id)), depth, (open, close)));
        self.follow(moved);
        self.touched.insert(key);
        Ok(key)
    }

    /// Marks the session `key` and every session below it as being
    /// cancelled: whatever they send to their invoking sessions from now on
    /// is ignored. Events already on a queue stay there. The root is never
    /// marked.
    pub fn begin_cancel(&mut self, key: SessionKey) {
        if key.is_root() {
            return;
        }
        for below in self.subtree(key) {
            tick();
            if let Some(node) = self.nodes.get_mut(&below) {
                node.cancelled = true;
            }
        }
    }

    /// Removes the session `key` and every session below it, with their
    /// queues and index entries, and returns them deepest first (each
    /// after every session below it). The root is never removed.
    pub fn remove(&mut self, key: SessionKey) -> Vec<SessionKey> {
        if key.is_root() || !self.contains(key) {
            return Vec::new();
        }
        if let Some((parent, id)) = self.nodes.get(&key).and_then(|node| node.parent.clone())
            && let Some(parent) = self.nodes.get_mut(&parent)
        {
            parent.children.remove(&id);
        }
        let mut removed = self.subtree(key);
        removed.reverse();
        for gone in &removed {
            tick();
            if let Some(node) = self.nodes.remove(gone) {
                self.queued.remove(&(self.order.label(node.open), *gone));
                self.order.remove(node.open);
                self.order.remove(node.close);
            }
            self.touched.remove(gone);
            self.finished.remove(gone);
            if let Some(deadline) = self.deadlines.remove(gone) {
                self.by_deadline.remove(&(deadline, *gone));
            }
        }
        removed
    }

    /// Pushes an external event onto the queue of the session `key`.
    /// Returns whether it is present; an absent session receives nothing.
    pub fn push_external(&mut self, key: SessionKey, event: E) -> bool {
        let Some(node) = self.nodes.get_mut(&key) else {
            return false;
        };
        node.external.push_back(Delivery::External(event));
        self.index_queued(key);
        true
    }

    /// Pushes a message of the session `key` onto its invoking session's
    /// queue, stamped with the sender's invoke id. A message from the
    /// root, an absent session, or a session being cancelled is ignored.
    /// Returns whether it was queued.
    pub fn push_to_parent(&mut self, key: SessionKey, message: Invoked<E, V>) -> bool {
        tick();
        let Some(node) = self.nodes.get(&key) else {
            return false;
        };
        let Some((parent, invoke_id)) = node.parent.clone() else {
            return false;
        };
        if node.cancelled {
            return false;
        }
        let Some(parent_node) = self.nodes.get_mut(&parent) else {
            return false;
        };
        parent_node
            .external
            .push_back(Delivery::FromInvocation { invoke_id, message });
        self.index_queued(parent);
        true
    }

    /// Records that a command ran on the session `key`, so the executor
    /// reads its status and deadline again. A host calls it for every
    /// session it runs a command on outside the executor's own calls.
    pub fn touch(&mut self, key: SessionKey) {
        if self.contains(key) {
            self.touched.insert(key);
        }
    }

    /// Takes the sessions touched since the last call.
    pub(crate) fn take_touched(&mut self) -> BTreeSet<SessionKey> {
        std::mem::take(&mut self.touched)
    }

    /// Records the executor's reading of the session `key`.
    pub(crate) fn record(&mut self, key: SessionKey, reading: Reading) {
        if !self.contains(key) {
            return;
        }
        if let Some(old) = self.deadlines.remove(&key) {
            self.by_deadline.remove(&(old, key));
        }
        match reading {
            Reading::Finished => {
                if !key.is_root()
                    && let Some(node) = self.nodes.get_mut(&key)
                {
                    node.finished = true;
                    let open = node.open;
                    self.finished.insert(key);
                    self.queued.remove(&(self.order.label(open), key));
                }
            }
            Reading::Running(Some(deadline)) => {
                self.deadlines.insert(key, deadline);
                self.by_deadline.insert((deadline, key));
            }
            Reading::Running(None) => {}
        }
    }

    /// Marks the invoked sessions whose `done.invoke` is due: finished
    /// since the last call, not cancelled. Returns them in invocation order.
    pub(crate) fn take_done_due(&mut self) -> Vec<SessionKey> {
        let mut due = Vec::new();
        for key in std::mem::take(&mut self.finished) {
            let Some(node) = self.nodes.get_mut(&key) else {
                continue;
            };
            if !node.done_reported && !node.cancelled {
                node.done_reported = true;
                due.push(key);
            }
        }
        self.sort_by_invocation_order(&mut due);
        due
    }

    /// The earliest deadline the executor read, over unfinished sessions.
    pub(crate) fn earliest_deadline(&self) -> Option<u64> {
        self.by_deadline.first().map(|(deadline, _)| *deadline)
    }

    /// The invoked sessions whose deadline the executor read as `deadline`,
    /// in invocation order.
    pub(crate) fn invoked_due_at(&self, deadline: u64) -> Vec<SessionKey> {
        let mut due: Vec<SessionKey> = self
            .by_deadline
            .range((deadline, SessionKey::ROOT)..=(deadline, SessionKey(u64::MAX)))
            .map(|(_, key)| *key)
            .filter(|key| !key.is_root())
            .collect();
        self.sort_by_invocation_order(&mut due);
        due
    }

    /// The deadline the executor last read for `key`.
    pub(crate) fn deadline(&self, key: SessionKey) -> Option<u64> {
        self.deadlines.get(&key).copied()
    }

    fn sort_by_invocation_order(&self, keys: &mut [SessionKey]) {
        keys.sort_by_cached_key(|key| self.rank(*key));
    }

    /// Removes the oldest queued entry of the first session, in invocation order,
    /// that `runnable` accepts and that has one.
    pub(crate) fn pop_next(
        &mut self,
        runnable: impl Fn(SessionKey) -> bool,
    ) -> Option<(SessionKey, Delivery<K, E, V>)> {
        let (label, key) = self.queued.iter().copied().find(|(_, key)| {
            tick();
            runnable(*key)
        })?;
        let node = self.nodes.get_mut(&key)?;
        let delivery = node.external.pop_front()?;
        if node.external.is_empty() {
            self.queued.remove(&(label, key));
        }
        Some((key, delivery))
    }

    /// The queued entries of the session `key`, oldest first (read-only);
    /// `None` when absent.
    #[must_use]
    pub fn external(&self, key: SessionKey) -> Option<impl Iterator<Item = &Delivery<K, E, V>>> {
        self.nodes.get(&key).map(|node| node.external.iter())
    }

    /// Number of queued entries of the session `key`; `None` when absent.
    #[must_use]
    pub fn external_len(&self, key: SessionKey) -> Option<usize> {
        self.nodes.get(&key).map(|node| node.external.len())
    }
}

#[cfg(test)]
mod order_tests {
    use std::collections::BTreeSet;

    use super::{InvokedSessions, SessionKey};

    /// A small deterministic generator (xorshift64*), so the run is the same
    /// every time without a test dependency.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound).unwrap_or(u64::MAX)).unwrap_or(0)
        }
    }

    /// Random registrations (half of them before every child of the root,
    /// which exhausts one label gap and forces relabeling), removals, and queued
    /// events: after every operation, `precedes` is the lexicographic order
    /// of the invoke-id paths, and `queued` holds exactly the sessions with
    /// queued work under their opening's current label.
    #[test]
    fn invocation_order_matches_path_order_under_random_inserts_and_removals() {
        let mut sessions: InvokedSessions<String, u64, ()> = InvokedSessions::default();
        let mut rng = Rng(0x5c3a_1d07_9e37_79b9);
        let mut descending = u64::MAX / 2;
        let mut relabeled = 0_usize;
        for step in 0..4_000_u64 {
            let keys = sessions.keys();
            let labels: Vec<(SessionKey, Option<u64>)> =
                keys.iter().map(|key| (*key, sessions.rank(*key))).collect();
            let key = keys[rng.below(keys.len())];
            match rng.below(10) {
                0..=4 if keys.len() < 64 => {
                    // Before every child of the root, the same label gap
                    // halves until it must be relabeled.
                    let (parent, id) = if rng.below(2) == 0 {
                        descending -= 1;
                        (SessionKey::ROOT, descending)
                    } else {
                        (key, rng.next() % 1_000_000)
                    };
                    let _ = sessions.register(parent, format!("{id:020}"));
                }
                0..=6 => {
                    sessions.remove(key);
                }
                _ => {
                    sessions.push_external(key, step);
                }
            }
            if labels
                .iter()
                .any(|(key, label)| sessions.contains(*key) && sessions.rank(*key) != *label)
            {
                relabeled += 1;
            }
            let keys = sessions.keys();
            let paths: Vec<(SessionKey, Vec<String>)> = keys
                .iter()
                .map(|key| (*key, sessions.path(*key).expect("present")))
                .collect();
            for (first, first_path) in &paths {
                for (second, second_path) in &paths {
                    assert_eq!(
                        sessions.precedes(*first, *second),
                        first_path < second_path,
                        "step {step}: {first_path:?} vs {second_path:?}"
                    );
                }
            }
            let expected: BTreeSet<(u64, SessionKey)> = keys
                .iter()
                .filter(|key| {
                    sessions
                        .nodes
                        .get(key)
                        .is_some_and(|node| !node.finished && !node.external.is_empty())
                })
                .map(|key| (sessions.rank(*key).expect("present"), *key))
                .collect();
            assert_eq!(sessions.queued, expected, "step {step}");
        }
        assert!(relabeled > 0, "the run exercises relabeling");
    }
}
