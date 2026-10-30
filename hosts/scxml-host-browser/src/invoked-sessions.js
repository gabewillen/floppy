// The sessions of one browser run and the external event queue of each
// (SCXML 1.0 section 4.2 and Appendix D): the root and the sessions it
// invokes, at any depth. It orders cross-session work exactly as the
// `scxml-scheduler` crate's `InvokedSessions`, which the Wasmtime session
// host drives.
//
// A session is addressed by an integer key, assigned when it registers
// (the root is ROOT). Invocation order is the pre-order of the invoke tree:
// the root, then each invoked session followed by the sessions it invoked,
// siblings in ascending invoke-id order by Unicode code point (the Rust
// `String` order). Each session takes its place in that order when it
// registers (an integer label, see PreOrder), so comparing two sessions
// costs one comparison and the sessions with queued work stay in that order
// without sorting: registering costs a lookup among the siblings (plus an
// amortized logarithmic relabeling), taking the next queued entry is
// logarithmic, and cancelling or removing a session costs its subtree. No
// operation's work grows with the nesting depth. The SCXML Event I/O
// Processor delivers across sessions by pushing onto the target session's
// external queue when the event is sent.
//
// Invariants: the root is always present, and every other session's
// invoking session; when the page set them, at most
// `limits.maxInvokedSessions` invoked sessions, none deeper than
// `limits.maxInvokeDepth`. The host bounds neither on its own, nor the
// length of an external queue. The queued index holds exactly the sessions
// not marked finished whose external queue is not empty.

/** An invoke limit was exceeded or the invoking session is absent; `code` is the executor's violation code. */
export class InvokeViolation {
  /** @param {string} code */
  constructor(code) {
    this.code = code;
  }
}

/** Whether `limit` is absent (no limit) or a non-negative integer. */
const isLimit = (limit) => limit === undefined || (Number.isSafeInteger(limit) && limit >= 0);

/** The keys an `invokeLimits` object may have. */
const INVOKE_LIMIT_KEYS = new Set(['maxInvokeDepth', 'maxInvokedSessions']);

/**
 * The page's `invokeLimits` as one frozen plain copy
 * `{maxInvokeDepth?, maxInvokedSessions?}`, or `undefined` when it is not a
 * valid limits object. It must be a plain object (prototype
 * `Object.prototype` or `null`) whose own keys are among those two, each a
 * data property (an accessor could answer each read differently, so it is
 * rejected without being called) holding `undefined` (no limit) or a
 * non-negative integer. Any other key, an array, or another kind of object
 * is invalid, so a misspelled limit is never silently no limit. Each own
 * property is read once; the session uses only the copy. (A Proxy cannot be
 * told from a plain object in a browser: its traps are consulted once per
 * own property, like any object's descriptors.)
 */
export function invokeLimitsCopy(limits) {
  if (limits === null || typeof limits !== 'object') return undefined;
  const prototype = Object.getPrototypeOf(limits);
  if (prototype !== Object.prototype && prototype !== null) return undefined;
  const descriptors = Object.getOwnPropertyDescriptors(limits);
  const copy = {};
  for (const key of Reflect.ownKeys(descriptors)) {
    const descriptor = descriptors[key];
    if (!INVOKE_LIMIT_KEYS.has(key) || !('value' in descriptor) || !isLimit(descriptor.value)) return undefined;
    if (descriptor.value !== undefined) copy[key] = descriptor.value;
  }
  return Object.freeze(copy);
}

/** An unbounded first-in, first-out queue with amortized O(1) `shift`. */
class Fifo {
  #items = [];
  #head = 0;

  get length() {
    return this.#items.length - this.#head;
  }

  push(item) {
    this.#items.push(item);
  }

  /** The queued items, oldest first (a copy). */
  items() {
    return this.#items.slice(this.#head);
  }

  shift() {
    if (this.length === 0) return undefined;
    const item = this.#items[this.#head];
    this.#items[this.#head] = undefined;
    this.#head += 1;
    if (this.#head > 1024 && this.#head * 2 > this.#items.length) {
      this.#items = this.#items.slice(this.#head);
      this.#head = 0;
    }
    return item;
  }
}

/**
 * UTF-16 code unit `unit` remapped so that code unit order equals code point
 * order: surrogates (D800-DFFF, the supplementary planes) move above
 * E000-FFFF, which move down to make room. Order-preserving within each
 * range and the identity below D800.
 */
const codePointRank = (unit) => (unit < 0xd800 ? unit : unit < 0xe000 ? unit + 0x2000 : unit - 0x800);

/**
 * Compares two strings by Unicode code point, as Rust orders `String`
 * (UTF-8 bytes). JavaScript's `<` compares UTF-16 code units, which puts
 * U+10000..U+10FFFF before U+E000..U+FFFF.
 */
export function byCodePoint(a, b) {
  const length = Math.min(a.length, b.length);
  for (let i = 0; i < length; i += 1) {
    const x = a.charCodeAt(i);
    const y = b.charCodeAt(i);
    if (x !== y) return codePointRank(x) - codePointRank(y);
  }
  return a.length - b.length;
}

/** Labels lie in `0..LABEL_SPACE` (safe integers). */
const LABEL_BITS = 50;
const LABEL_SPACE = 2 ** LABEL_BITS;
const NONE = -1;

/**
 * The open and close positions of every session as one ordered list with
 * increasing integer labels: an order-maintenance list (Bender, Cole,
 * Demaine, Farach-Colton, and Zito, "Two Simplified Algorithms for
 * Maintaining Order in a List", ESA 2002), as in the Rust scheduler. A
 * position inserted where its neighbours leave no gap relabels the smallest
 * aligned range of `2^j` labels around it that holds at most `2^(j/2)`
 * positions; relabeling keeps the relative order of every position.
 */
class PreOrder {
  #label = [0, LABEL_SPACE - 1];
  #prev = [NONE, 0];
  #next = [1, NONE];
  #free = [];

  /** The root's open (first) and close (last) positions. */
  static ROOT_OPEN = 0;
  static ROOT_CLOSE = 1;

  label(position) {
    return this.#label[position];
  }

  /** Inserts a position right before `anchor` (never the first) and returns it. */
  insertBefore(anchor) {
    const prev = this.#prev[anchor];
    const low = this.#label[prev];
    const high = this.#label[anchor];
    const position = this.#free.length > 0 ? this.#free.pop() : this.#label.length;
    this.#label[position] = low + Math.floor((high - low) / 2);
    this.#prev[position] = prev;
    this.#next[position] = anchor;
    this.#next[prev] = position;
    this.#prev[anchor] = position;
    if (high - low < 2) this.#relabel(position);
    return position;
  }

  /** Unlinks `position`; it is reused. */
  remove(position) {
    const prev = this.#prev[position];
    const next = this.#next[position];
    if (prev !== NONE) this.#next[prev] = next;
    if (next !== NONE) this.#prev[next] = prev;
    this.#free.push(position);
  }

  /** The positions in list order, starting at `from`. */
  *from(from) {
    for (let position = from; position !== NONE; position = this.#next[position]) yield position;
  }

  #relabel(position) {
    for (let bits = 1; bits <= LABEL_BITS; bits += 1) {
      const size = 2 ** bits;
      const base = Math.floor(this.#label[position] / size) * size;
      const end = base + size;
      let first = position;
      let last = position;
      let count = 1;
      while (this.#prev[first] !== NONE && this.#label[this.#prev[first]] >= base) {
        first = this.#prev[first];
        count += 1;
      }
      while (this.#next[last] !== NONE && this.#label[this.#next[last]] < end) {
        last = this.#next[last];
        count += 1;
      }
      if (bits < LABEL_BITS && count > 2 ** Math.floor(bits / 2)) continue;
      const spacing = Math.floor(size / count);
      let label = base;
      for (let current = first; ; current = this.#next[current]) {
        this.#label[current] = label;
        if (current === last) return;
        label += spacing;
      }
    }
  }
}

/**
 * A binary min-heap of session keys with removal by key, ordered by
 * `less`. Relabeling keeps the relative order of every session, so the heap
 * stays ordered when labels move.
 */
class KeyHeap {
  #keys = [];
  #index = new Map();
  #less;

  constructor(less) {
    this.#less = less;
  }

  has(key) {
    return this.#index.has(key);
  }

  peek() {
    return this.#keys[0];
  }

  push(key) {
    if (this.#index.has(key)) return;
    this.#keys.push(key);
    this.#index.set(key, this.#keys.length - 1);
    this.#up(this.#keys.length - 1);
  }

  delete(key) {
    const at = this.#index.get(key);
    if (at === undefined) return;
    const last = this.#keys.pop();
    this.#index.delete(key);
    if (at === this.#keys.length) return;
    this.#keys[at] = last;
    this.#index.set(last, at);
    this.#down(at);
    this.#up(at);
  }

  #swap(a, b) {
    const keys = this.#keys;
    [keys[a], keys[b]] = [keys[b], keys[a]];
    this.#index.set(keys[a], a);
    this.#index.set(keys[b], b);
  }

  #up(at) {
    while (at > 0) {
      const parent = (at - 1) >> 1;
      if (!this.#less(this.#keys[at], this.#keys[parent])) return;
      this.#swap(at, parent);
      at = parent;
    }
  }

  #down(at) {
    for (;;) {
      const left = 2 * at + 1;
      const right = left + 1;
      let least = at;
      if (left < this.#keys.length && this.#less(this.#keys[left], this.#keys[least])) least = left;
      if (right < this.#keys.length && this.#less(this.#keys[right], this.#keys[least])) least = right;
      if (least === at) return;
      this.#swap(at, least);
      at = least;
    }
  }
}

/** The root session's key. */
export const ROOT = 0;

/** The index of the first of the sorted `ids` after `id` by code point. */
function firstAfter(ids, id) {
  let low = 0;
  let high = ids.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (byCodePoint(ids[middle], id) <= 0) low = middle + 1;
    else high = middle;
  }
  return low;
}

export class InvokedSessions {
  /**
   * key -> {parent, invokeId, depth, children: Map<id, key>, childIds (the
   * ids of `children` sorted by code point), external, cancelled,
   * doneReported, finished, open, close}
   */
  #nodes = new Map();
  #nextKey = ROOT + 1;
  #limits;
  #order = new PreOrder();
  /** Sessions not marked finished with queued work, in invocation order. */
  #queued = new KeyHeap((a, b) => this.#rank(a) < this.#rank(b));
  /** Sessions marked finished whose `done.invoke` is not yet decided. */
  #finished = new Set();
  /** Sessions a command ran on since the host last read their deadlines (see `touch`). */
  #touched = new Set();
  /** The pending deadline of each session that has one, as the host last read it. */
  #deadlines = new Map();
  /** The sessions of each deadline in `#deadlines`. */
  #byDeadline = new Map();
  /** The deadlines of `#byDeadline`, earliest first. */
  #earliest = new KeyHeap((a, b) => a < b);

  /** @param {{maxInvokeDepth?: number, maxInvokedSessions?: number}} limits absent fields set no limit */
  constructor(limits = {}) {
    this.#limits = { maxInvokeDepth: limits.maxInvokeDepth, maxInvokedSessions: limits.maxInvokedSessions };
    this.#nodes.set(ROOT, InvokedSessions.#node(undefined, undefined, 0, PreOrder.ROOT_OPEN, PreOrder.ROOT_CLOSE));
  }

  static #node(parent, invokeId, depth, open, close) {
    return {
      parent,
      invokeId,
      depth,
      children: new Map(),
      childIds: [],
      external: new Fifo(),
      receive: undefined,
      cancelled: false,
      doneReported: false,
      finished: false,
      open,
      close,
    };
  }

  #rank(key) {
    return this.#order.label(this.#nodes.get(key).open);
  }

  /** Whether the session `key` is present. */
  contains(key) {
    return this.#nodes.has(key);
  }

  /** Number of sessions: the root session and every invoked session. */
  get size() {
    return this.#nodes.size;
  }

  /** Every session key in invocation order (the root first). */
  keys() {
    const keys = [];
    for (const position of this.#order.from(PreOrder.ROOT_OPEN)) {
      const key = this.#opens.get(position);
      if (key !== undefined) keys.push(key);
    }
    return keys;
  }

  /** Session key by open position. */
  #opens = new Map([[PreOrder.ROOT_OPEN, ROOT]]);

  /** The session `parent` invoked as `invokeId`, if present. */
  child(parent, invokeId) {
    return this.#nodes.get(parent)?.children.get(invokeId);
  }

  /** The invoke ids from the root to `key` (`[]` for the root): work proportional to its depth. */
  path(key) {
    const path = [];
    for (let node = this.#nodes.get(key); node?.parent !== undefined; node = this.#nodes.get(node.parent)) {
      path.push(node.invokeId);
    }
    return path.reverse();
  }

  /** Whether the session `key` may invoke one more session within the page's limits. */
  canInvoke(key) {
    const { maxInvokeDepth, maxInvokedSessions } = this.#limits;
    const node = this.#nodes.get(key);
    return (
      node !== undefined &&
      node.receive === undefined &&
      (maxInvokeDepth === undefined || node.depth < maxInvokeDepth) &&
      (maxInvokedSessions === undefined || this.#nodes.size - 1 < maxInvokedSessions)
    );
  }

  /** Registers the session `id` invoked by `parent`, with an empty queue, and returns its key. */
  register(parent, id) {
    const node = this.#nodes.get(parent);
    if (node === undefined) throw new InvokeViolation('session-missing');
    if (node.children.has(id)) throw new InvokeViolation('duplicate-invocation');
    if (!this.canInvoke(parent)) throw new InvokeViolation('invoke-limit-exceeded');
    // Pre-order: right before the next sibling's subtree, else at the end
    // of the invoking session's subtree. The next sibling is found by
    // binary search, as the Rust host's `BTreeMap` range finds it.
    const at = firstAfter(node.childIds, id);
    const next = node.childIds[at];
    const anchor = next === undefined ? node.close : this.#nodes.get(node.children.get(next)).open;
    const open = this.#order.insertBefore(anchor);
    const close = this.#order.insertBefore(anchor);
    const key = this.#nextKey;
    this.#nextKey += 1;
    node.children.set(id, key);
    node.childIds.splice(at, 0, id);
    this.#nodes.set(key, InvokedSessions.#node(parent, id, node.depth + 1, open, close));
    this.#opens.set(open, key);
    this.#touched.add(key);
    return key;
  }

  /**
   * Registers the invoked service `id` of `parent` and returns its key: it
   * has no external queue, and every event pushed to it goes to `receive`
   * (the service's `events` stream); it invokes nothing.
   */
  registerService(parent, id, receive) {
    const key = this.register(parent, id);
    const node = this.#nodes.get(key);
    node.receive = receive;
    this.#touched.delete(key);
    return key;
  }

  /** The session `key` and every session below it, in invocation order. */
  #subtree(key) {
    const order = [];
    const pending = [key];
    while (pending.length > 0) {
      const next = pending.pop();
      const node = this.#nodes.get(next);
      if (node === undefined) continue;
      order.push(next);
      // Pushed last first, so they are taken by ascending invoke id.
      for (let index = node.childIds.length - 1; index >= 0; index -= 1) {
        pending.push(node.children.get(node.childIds[index]));
      }
    }
    return order;
  }

  /**
   * Marks the session `key` and every session below it as being
   * cancelled: whatever they send to their invoking sessions from now on is
   * ignored. Events already on a queue stay there.
   */
  beginCancel(key) {
    if (key === ROOT) return;
    for (const below of this.#subtree(key)) this.#nodes.get(below).cancelled = true;
  }

  /**
   * Removes the session `key` and every session below it (never the root)
   * and returns their keys, deepest first (each after every session below
   * it); `[]` when absent.
   */
  remove(key) {
    const node = this.#nodes.get(key);
    if (key === ROOT || node === undefined) return [];
    const parent = this.#nodes.get(node.parent);
    if (parent !== undefined) {
      parent.children.delete(node.invokeId);
      parent.childIds.splice(firstAfter(parent.childIds, node.invokeId) - 1, 1);
    }
    const removed = this.#subtree(key).reverse();
    for (const gone of removed) {
      const removed = this.#nodes.get(gone);
      this.#queued.delete(gone);
      this.#finished.delete(gone);
      this.#touched.delete(gone);
      this.record(gone, undefined);
      this.#opens.delete(removed.open);
      this.#order.remove(removed.open);
      this.#order.remove(removed.close);
      this.#nodes.delete(gone);
    }
    return removed;
  }

  #index(key, node) {
    if (!node.finished && node.external.length > 0) this.#queued.push(key);
  }

  /** Pushes an external event onto a session's queue; false when it is absent. */
  pushExternal(key, event) {
    const node = this.#nodes.get(key);
    if (node === undefined) return false;
    if (node.receive !== undefined) {
      node.receive(event);
      return true;
    }
    node.external.push({ tag: 'external', val: event });
    this.#index(key, node);
    return true;
  }

  /**
   * Pushes a message (`{tag: 'event' | 'done', val}`) of the session `key`
   * onto its invoking session's queue, stamped with the sender's invoke id;
   * false (ignored) from the root, an absent session, or one being
   * cancelled.
   */
  pushToParent(key, message) {
    const node = this.#nodes.get(key);
    if (node === undefined || node.parent === undefined || node.cancelled) return false;
    const parent = this.#nodes.get(node.parent);
    if (parent === undefined) return false;
    parent.external.push({ tag: 'from-invocation', invokeId: node.invokeId, message });
    this.#index(node.parent, parent);
    return true;
  }

  /**
   * Records that the invoked session `key` reached a top-level final
   * state: it runs nothing more, so its queue leaves the index, and its
   * `done.invoke` becomes due.
   */
  markFinished(key) {
    const node = this.#nodes.get(key);
    if (key === ROOT || node === undefined || node.finished) return;
    node.finished = true;
    this.#queued.delete(key);
    this.#finished.add(key);
  }

  /**
   * Records that a command ran on the session `key` (or drained its due
   * sends), so its pending deadline may have changed: the host reads it
   * again before it next needs the earliest deadline.
   */
  touch(key) {
    if (this.#nodes.has(key)) this.#touched.add(key);
  }

  /** The sessions touched since the last call. */
  takeTouched() {
    const touched = [...this.#touched];
    this.#touched.clear();
    return touched;
  }

  /** Records the pending deadline the host read for the session `key` (`undefined`: none, or finished). */
  record(key, deadline) {
    const previous = this.#deadlines.get(key);
    if (previous === deadline) return;
    if (previous !== undefined) {
      const keys = this.#byDeadline.get(previous);
      keys.delete(key);
      if (keys.size === 0) {
        this.#byDeadline.delete(previous);
        this.#earliest.delete(previous);
      }
      this.#deadlines.delete(key);
    }
    if (deadline === undefined || !this.#nodes.has(key)) return;
    this.#deadlines.set(key, deadline);
    let keys = this.#byDeadline.get(deadline);
    if (keys === undefined) {
      keys = new Set();
      this.#byDeadline.set(deadline, keys);
      this.#earliest.push(deadline);
    }
    keys.add(key);
  }

  /** The earliest recorded deadline, if any. */
  earliestDeadline() {
    return this.#earliest.peek();
  }

  /** The sessions whose recorded deadline is `deadline`, in invocation order. */
  dueAt(deadline) {
    return [...(this.#byDeadline.get(deadline) ?? [])].sort((a, b) => this.#rank(a) - this.#rank(b));
  }

  /**
   * Marks the invoked sessions whose `done.invoke` is due: marked finished
   * since the last call, not cancelled. Their keys, in invocation order.
   */
  takeDoneDue() {
    const due = [];
    for (const key of this.#finished) {
      const node = this.#nodes.get(key);
      if (!node.doneReported && !node.cancelled) {
        node.doneReported = true;
        due.push(key);
      }
    }
    this.#finished.clear();
    return due.sort((a, b) => this.#rank(a) - this.#rank(b));
  }

  /**
   * Removes the oldest entry of the first session, in invocation order, that
   * `runnable(key)` accepts and that has one: `[key, entry]`.
   */
  popNext(runnable) {
    const skipped = [];
    let next;
    for (let key = this.#queued.peek(); key !== undefined; key = this.#queued.peek()) {
      if (runnable(key)) {
        next = key;
        break;
      }
      this.#queued.delete(key);
      skipped.push(key);
    }
    for (const key of skipped) this.#queued.push(key);
    if (next === undefined) return undefined;
    const node = this.#nodes.get(next);
    const entry = node.external.shift();
    if (node.external.length === 0) this.#queued.delete(next);
    return [next, entry];
  }

  /** The queued entries of the session `key`, oldest first (a copy); `[]` when absent. */
  external(key) {
    return this.#nodes.get(key)?.external.items() ?? [];
  }
}
