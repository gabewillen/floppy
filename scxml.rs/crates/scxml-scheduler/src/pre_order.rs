//! The invocation order of a run's sessions as integer labels.
//!
//! Every session holds two positions in one ordered list: where it opens
//! and where its subtree closes. A session invoked as `id` opens right
//! before the opening of its invoking session's next child (the first one
//! with a greater invoke id), or right before its invoking session's close
//! when it has none, and closes right after it opens; so the openings, read
//! in list order, are the pre-order of the invoke tree, and finding a new
//! session's place costs one lookup among its siblings.
//!
//! The list is an order-maintenance list (Bender, Cole, Demaine,
//! Farach-Colton, and Zito, "Two Simplified Algorithms for Maintaining
//! Order in a List", ESA 2002): each position carries an integer label that
//! increases along the list, so comparing two positions compares two
//! integers. A position inserted where its neighbours' labels leave no gap
//! relabels the smallest aligned label range around it whose positions are
//! sparse enough, at amortized logarithmic cost; relabeling keeps the
//! relative order of every position, and the caller learns which session
//! openings moved, so indexes keyed by labels follow.

use crate::invoked_sessions::SessionKey;
use crate::work::tick;

/// Labels lie in `0..LABEL_SPACE`.
const LABEL_BITS: u32 = 62;
const LABEL_SPACE: u64 = 1 << LABEL_BITS;

/// No position (the end of the list either way).
const NONE: usize = usize::MAX;

/// One position of the list.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Slot {
    label: u64,
    prev: usize,
    next: usize,
    /// The session this position opens, or `None` for a close.
    opens: Option<SessionKey>,
}

/// A position in a [`PreOrder`], valid until it is removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Position(usize);

/// The ordered list of every session's open and close positions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreOrder {
    slots: Vec<Slot>,
    /// Removed slots, reused before the arena grows.
    free: Vec<usize>,
}

impl PreOrder {
    /// The list of the root session alone: its open first, its close last.
    /// Every other position is inserted between them.
    pub(crate) fn new(root: SessionKey) -> (Self, Position, Position) {
        let order = Self {
            slots: vec![
                Slot {
                    label: 0,
                    prev: NONE,
                    next: 1,
                    opens: Some(root),
                },
                Slot {
                    label: LABEL_SPACE - 1,
                    prev: 0,
                    next: NONE,
                    opens: None,
                },
            ],
            free: Vec::new(),
        };
        (order, Position(0), Position(1))
    }

    /// The label of `position`: positions earlier in the list have smaller
    /// labels.
    pub(crate) fn label(&self, position: Position) -> u64 {
        self.slots[position.0].label
    }

    /// Inserts a position right before `anchor` (never the first position)
    /// that opens `opens`, or closes when `None`. Every session opening
    /// whose label moved is appended to `moved` with its previous label.
    pub(crate) fn insert_before(
        &mut self,
        anchor: Position,
        opens: Option<SessionKey>,
        moved: &mut Vec<(SessionKey, u64)>,
    ) -> Position {
        tick();
        let next = anchor.0;
        let prev = self.slots[next].prev;
        let (low, high) = (self.slots[prev].label, self.slots[next].label);
        let slot = Slot {
            label: low + (high - low) / 2,
            prev,
            next,
            opens,
        };
        let index = if let Some(index) = self.free.pop() {
            self.slots[index] = slot;
            index
        } else {
            self.slots.push(slot);
            self.slots.len() - 1
        };
        self.slots[prev].next = index;
        self.slots[next].prev = index;
        if high - low < 2 {
            self.relabel(index, moved);
        }
        Position(index)
    }

    /// Unlinks `position`; its slot is reused.
    pub(crate) fn remove(&mut self, position: Position) {
        let Slot { prev, next, .. } = self.slots[position.0];
        if prev != NONE {
            self.slots[prev].next = next;
        }
        if next != NONE {
            self.slots[next].prev = prev;
        }
        self.free.push(position.0);
    }

    /// Spreads the labels of the smallest aligned range of `2^j` labels
    /// around `index` that holds at most `2^(j/2)` positions (the whole
    /// label space at the latest) evenly over that range. `index` carries
    /// its predecessor's label, so the list is ordered, not strictly.
    fn relabel(&mut self, index: usize, moved: &mut Vec<(SessionKey, u64)>) {
        for bits in 1..=LABEL_BITS {
            let size = 1u64 << bits;
            let base = self.slots[index].label & !(size - 1);
            let end = base + size;
            let (mut first, mut last, mut count) = (index, index, 1u64);
            while let Some(prev) = self.step(first, |slot| slot.prev)
                && self.slots[prev].label >= base
            {
                tick();
                first = prev;
                count += 1;
            }
            while let Some(next) = self.step(last, |slot| slot.next)
                && self.slots[next].label < end
            {
                tick();
                last = next;
                count += 1;
            }
            if bits < LABEL_BITS && count > size >> (bits / 2) {
                continue;
            }
            let spacing = size / count;
            let mut label = base;
            let mut current = first;
            loop {
                tick();
                let slot = &mut self.slots[current];
                if slot.label != label
                    && let Some(session) = slot.opens
                {
                    moved.push((session, slot.label));
                }
                slot.label = label;
                if current == last {
                    return;
                }
                label += spacing;
                current = slot.next;
            }
        }
    }

    fn step(&self, index: usize, link: impl Fn(&Slot) -> usize) -> Option<usize> {
        let next = link(&self.slots[index]);
        (next != NONE).then_some(next)
    }
}
