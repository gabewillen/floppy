//! Drafts: the text of a selection the model is still writing (mosfet.py's
//! `processing.DraftParser`).
//!
//! A model writes a JSON string left to right, so a string field of a
//! dispatch call's arguments only ever grows by appending. With `drafts`
//! (a [`Marker`] per offered event), a streamed call's arguments are read
//! as they arrive ([`DraftParser`]) and each marked field's newly written
//! text is returned as a [`Draft`], under the marker's `event`, before the
//! call ends: the invoking document can start acting on the words (saying
//! them) while the model is still writing them. The final selection, in
//! `generate.result`, is authoritative; drafts are never checked against a
//! schema.
//!
//! The arguments read are `{"events": [{"event": …, "target": …, "data":
//! {…}}]}`, the dispatch tool's shape. An item is drafted when its
//! `event` string ended before the marked field's string (or the array
//! the marker names in `within`) began, or, when the model writes the data
//! first (the tool's members are sorted, so `data` precedes `event`), when
//! the data names a key that only the marked event's data has among the
//! offered events ([`shapes`]); otherwise it is not drafted. Only the first
//! item drafted in a call is drafted. The
//! marker's `leading` sibling fields, in the same object as the field,
//! whose values close before the field's string begins are decoded and
//! ride its first draft; an element of a `within` array that closes with
//! nothing written into the field is drafted once as it closes, with empty
//! text and its leading fields, so its part still opens in order.
//! Malformed arguments stop drafting for the call; nothing here fails the
//! call.

use scxml_types::exceeds_wasm_bound;
use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;

/// Upper bound on the markers one call may carry (wasm builds only).
pub const MAX_MARKERS: usize = scxml_types::wasm_bound(64);
/// Upper bound on the bytes of a marker's names (wasm builds only).
pub const MAX_MARKER_NAME_BYTES: usize = scxml_types::wasm_bound(256);
/// Upper bound on a marker's leading fields (wasm builds only).
pub const MAX_LEADING: usize = scxml_types::wasm_bound(16);
/// Upper bound on the open JSON containers of the arguments being read
/// (wasm builds only): a deeper call is not drafted.
pub const MAX_DEPTH: usize = scxml_types::wasm_bound(128);
/// Upper bound on the bytes of one leading field's value being captured
/// (wasm builds only): a longer one is left out.
pub const MAX_CAPTURE_BYTES: usize = scxml_types::wasm_bound(16 * 1024);

/// A marker on an offered event (mosfet.py's `Draft`): its `field` may
/// reach the target as `event` while it is written. Without `within`,
/// `field` is in the event's data; with it, the drafted string is
/// `data.<within>[i].<field>` for each element `i` of that array.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Marker {
    /// The drafted string field.
    pub field: String,
    /// The event each draft is returned as.
    pub event: String,
    /// The array of the event's data the field lives in, if any.
    pub within: Option<String>,
    /// Sibling fields that ride the field's first draft.
    pub leading: Vec<String>,
}

/// The `drafts` parameter: a JSON object mapping each offered event's name
/// to its marker `{field, event, within?, leading?}`.
///
/// # Errors
///
/// A static message when the value is not such an object (a name that is
/// empty or over [`MAX_MARKER_NAME_BYTES`], `leading` naming `field`,
/// `within` that is `field`, or more than [`MAX_MARKERS`] markers or
/// [`MAX_LEADING`] leading fields).
pub fn markers(value: &Json) -> Result<BTreeMap<String, Marker>, &'static str> {
    const MALFORMED: &str =
        "drafts must be an object of {field, event, within?, leading?} markers by event";
    let declared = value.as_object().ok_or(MALFORMED)?;
    if exceeds_wasm_bound(declared.len(), MAX_MARKERS) {
        return Err("drafts names too many events");
    }
    let name = |value: Option<&Json>| -> Result<String, &'static str> {
        value
            .and_then(Json::as_str)
            .filter(|text| {
                !text.is_empty() && !exceeds_wasm_bound(text.len(), MAX_MARKER_NAME_BYTES)
            })
            .map(str::to_owned)
            .ok_or(MALFORMED)
    };
    let mut markers = BTreeMap::new();
    for (event, marker) in declared {
        let marker = marker.as_object().ok_or(MALFORMED)?;
        if event.is_empty()
            || exceeds_wasm_bound(event.len(), MAX_MARKER_NAME_BYTES)
            || marker
                .keys()
                .any(|key| !["field", "event", "within", "leading"].contains(&key.as_str()))
        {
            return Err(MALFORMED);
        }
        let field = name(marker.get("field"))?;
        let within = match marker.get("within") {
            None | Some(Json::Null) => None,
            within => Some(name(within)?),
        };
        let leading = match marker.get("leading") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(leading)) => leading
                .iter()
                .map(|key| name(Some(key)))
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(MALFORMED),
        };
        if exceeds_wasm_bound(leading.len(), MAX_LEADING) {
            return Err("a draft marker names too many leading fields");
        }
        if leading.contains(&field) || within.as_ref() == Some(&field) {
            return Err(MALFORMED);
        }
        markers.insert(
            event.clone(),
            Marker {
                field,
                event: name(marker.get("event"))?,
                within,
                leading,
            },
        );
    }
    Ok(markers)
}

/// The data keys that tell a marked event's item before its `event` is
/// written: for each marker, the key its field is reached through (its
/// `within`, else its `field`), when the event's branch of the dispatch
/// schema `parameters` (`properties.events.items.anyOf`, each branch's
/// `properties.event.enum` naming one event) has it among its data's
/// `properties` and no other branch does. A key two events share tells
/// neither.
#[must_use]
pub fn shapes(parameters: &Json, markers: &BTreeMap<String, Marker>) -> BTreeMap<String, String> {
    let branches = parameters
        .pointer("/properties/events/items/anyOf")
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let data_keys = |branch: &Json| -> Vec<String> {
        branch
            .pointer("/properties/data/properties")
            .and_then(Json::as_object)
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for branch in branches {
        let Some(event) = branch
            .pointer("/properties/event/enum/0")
            .and_then(Json::as_str)
        else {
            continue;
        };
        for key in data_keys(branch) {
            owners.entry(key).or_default().push(event.to_owned());
        }
    }
    let mut shapes = BTreeMap::new();
    for (event, marker) in markers {
        let key = marker.within.as_ref().unwrap_or(&marker.field);
        if let Some([owner]) = owners.get(key).map(Vec::as_slice)
            && owner == event
        {
            shapes.insert(key.clone(), event.clone());
        }
    }
    shapes
}

/// Text a model has newly written into one marked string field of an
/// event it is still selecting: a delta (mosfet.py's `DraftData`).
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The marker's event: the name the draft is returned as.
    pub name: String,
    /// The unescaped text newly written into the field; empty only on the
    /// one draft of an element that ended with nothing written into it.
    pub text: String,
    /// How many characters (Unicode scalar values) of the field were
    /// written before `text`, within its part when it lives in an array.
    pub offset: usize,
    /// The event whose field this drafts.
    pub event: String,
    /// The drafted event's index in the selection's `events`.
    pub item: usize,
    /// The element of the marker's `within` array, when it names one.
    pub part: Option<usize>,
    /// The draft's order among every draft of one call.
    pub seq: usize,
    /// On a field's first draft only: the leading fields written before it.
    pub leading: Map<String, Json>,
}

impl Draft {
    /// The draft's data as a JSON object.
    #[must_use]
    pub fn data(&self) -> Json {
        let mut data = Map::new();
        data.insert("text".into(), self.text.clone().into());
        data.insert("offset".into(), self.offset.into());
        data.insert("event".into(), self.event.clone().into());
        data.insert("item".into(), self.item.into());
        if let Some(part) = self.part {
            data.insert("part".into(), part.into());
        }
        data.insert("seq".into(), self.seq.into());
        data.insert("leading".into(), Json::Object(self.leading.clone()));
        Json::Object(data)
    }

    /// Folds `next` into this draft when it continues it (the same field
    /// of the same part, written right after it), so a call's drafts can be
    /// returned at most once per read.
    pub fn absorb(&mut self, next: &Self) -> bool {
        let continues = next.event == self.event
            && next.item == self.item
            && next.part == self.part
            && next.leading.is_empty()
            && next.offset == self.offset + self.text.chars().count();
        if continues {
            self.text.push_str(&next.text);
        }
        continues
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Object,
    Array,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Root,
    Events,
    Item,
    Data,
    Within,
    Element,
    Other,
}

#[derive(Clone, Debug)]
struct Frame {
    kind: Kind,
    role: Role,
    key: Option<String>,
    count: usize,
    item: usize,
    part: Option<usize>,
    event: Option<String>,
    drafted: bool,
    leading: Map<String, Json>,
}

impl Frame {
    fn new(kind: Kind, role: Role, item: usize, part: Option<usize>) -> Self {
        Self {
            kind,
            role,
            key: None,
            count: 0,
            item,
            part,
            event: None,
            drafted: false,
            leading: Map::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Value,
    Key,
    Colon,
    Next,
    String,
    Escape,
    Unicode,
    Literal,
    End,
}

/// The raw JSON text of one leading field's value being read, and the
/// object (by its depth in the stack) it is in.
#[derive(Debug)]
struct Capture {
    depth: usize,
    key: String,
    text: String,
}

/// Incremental reader of a streamed dispatch call's arguments that drafts
/// marked string fields (mosfet.py's `DraftParser`).
#[derive(Debug)]
pub struct DraftParser {
    markers: BTreeMap<String, Marker>,
    shapes: BTreeMap<String, String>,
    // The one item this call drafts, once one is.
    item: Option<usize>,
    stack: Vec<Frame>,
    mode: Mode,
    failed: bool,
    seq: usize,
    is_key: bool,
    // The depth of the frame whose field is being drafted, if any.
    drafting: Option<usize>,
    text: String,
    // How much of `text` was already drafted, in bytes and characters.
    emitted: usize,
    offset: usize,
    hex: String,
    high: Option<u32>,
    capture: Option<Capture>,
}

impl DraftParser {
    /// A parser for one call's arguments, drafting what `markers` mark.
    #[must_use]
    pub fn new(markers: BTreeMap<String, Marker>) -> Self {
        Self {
            markers,
            shapes: BTreeMap::new(),
            item: None,
            stack: Vec::new(),
            mode: Mode::Value,
            failed: false,
            seq: 0,
            is_key: false,
            drafting: None,
            text: String::new(),
            emitted: 0,
            offset: 0,
            hex: String::new(),
            high: None,
            capture: None,
        }
    }

    /// The parser, also drafting an item whose data is written before its
    /// `event` when the data names one of `shapes`' keys ([`shapes`]).
    #[must_use]
    pub fn with_shapes(mut self, shapes: BTreeMap<String, String>) -> Self {
        self.shapes = shapes;
        self
    }

    /// Whether the arguments read so far were malformed, which stopped
    /// drafting for the call.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.failed
    }

    /// Reads the next piece of the arguments; returns the drafts it
    /// advanced, in order.
    pub fn feed(&mut self, chunk: &str) -> Vec<Draft> {
        let mut drafts = Vec::new();
        if self.failed {
            return drafts;
        }
        for character in chunk.chars() {
            if !self.step(character, &mut drafts) {
                self.failed = true;
                self.drafting = None;
                self.capture = None;
                return drafts;
            }
        }
        self.flush(&mut drafts);
        drafts
    }

    fn flush(&mut self, drafts: &mut Vec<Draft>) {
        let Some(depth) = self.drafting else {
            return;
        };
        if self.text.len() == self.emitted {
            return;
        }
        let Some(frame) = self.stack.get_mut(depth) else {
            return;
        };
        let Some(event) = frame.event.clone() else {
            return;
        };
        let Some(marker) = self.markers.get(&event) else {
            return;
        };
        let delta = self.text.get(self.emitted..).unwrap_or_default().to_owned();
        let leading = if self.offset == 0 {
            frame.leading.clone()
        } else {
            Map::new()
        };
        let count = delta.chars().count();
        drafts.push(Draft {
            name: marker.event.clone(),
            text: delta,
            offset: self.offset,
            event,
            item: frame.item,
            part: frame.part,
            seq: self.seq,
            leading,
        });
        frame.drafted = true;
        self.item = Some(frame.item);
        self.emitted = self.text.len();
        self.offset += count;
        self.seq += 1;
    }

    fn open(&mut self, kind: Kind) -> bool {
        if exceeds_wasm_bound(self.stack.len() + 1, MAX_DEPTH) {
            return false;
        }
        let (role, item, part) = match self.stack.last() {
            None => (
                if kind == Kind::Object {
                    Role::Root
                } else {
                    Role::Other
                },
                0,
                None,
            ),
            Some(parent) => {
                let key = parent.key.as_deref();
                match (parent.role, kind) {
                    (Role::Root, Kind::Array) if key == Some("events") => (Role::Events, 0, None),
                    (Role::Events, Kind::Object) => (Role::Item, parent.count, None),
                    (Role::Item, Kind::Object) if key == Some("data") => {
                        (Role::Data, parent.item, None)
                    }
                    (Role::Data, Kind::Array) => {
                        let item = parent.item;
                        if self.within(self.stack.len() - 1) {
                            (Role::Within, item, None)
                        } else {
                            (Role::Other, 0, None)
                        }
                    }
                    (Role::Within, Kind::Object) => {
                        (Role::Element, parent.item, Some(parent.count))
                    }
                    _ => (Role::Other, 0, None),
                }
            }
        };
        self.stack.push(Frame::new(kind, role, item, part));
        self.mode = if kind == Kind::Object {
            Mode::Key
        } else {
            Mode::Value
        };
        true
    }

    fn close(&mut self, kind: Kind, drafts: &mut Vec<Draft>) -> bool {
        if self.stack.last().map(|frame| frame.kind) != Some(kind) {
            return false;
        }
        let depth = self.stack.len() - 1;
        if self
            .stack
            .get(depth)
            .is_some_and(|frame| frame.role == Role::Element && !frame.drafted)
            && let Some((name, event)) = self.holder(depth)
            && let Some(frame) = self.stack.get(depth)
        {
            // An element that ends with nothing written into its field
            // still opens its part, in order, with what it carries.
            self.item = Some(frame.item);
            drafts.push(Draft {
                name,
                text: String::new(),
                offset: 0,
                event,
                item: frame.item,
                part: frame.part,
                seq: self.seq,
                leading: frame.leading.clone(),
            });
            self.seq += 1;
        }
        self.stack.pop();
        self.value_done();
        true
    }

    fn value_done(&mut self) {
        let depth = self.stack.len().checked_sub(1);
        if let Some(capture) = self.capture.take() {
            if Some(capture.depth) == depth {
                if let (Ok(value), Some(frame)) = (
                    serde_json::from_str::<Json>(&capture.text),
                    self.stack.get_mut(capture.depth),
                ) {
                    frame.leading.insert(capture.key, value);
                }
            } else {
                self.capture = Some(capture);
            }
        }
        match self.stack.last_mut() {
            Some(frame) => {
                frame.count += 1;
                self.mode = Mode::Next;
            }
            None => self.mode = Mode::End,
        }
    }

    fn start_string(&mut self) {
        self.text.clear();
        self.emitted = 0;
        self.offset = 0;
        self.hex.clear();
        self.high = None;
        self.drafting = None;
        let Some(depth) = self.stack.len().checked_sub(1) else {
            return;
        };
        let Some(key) = self.stack.get(depth).and_then(|frame| frame.key.clone()) else {
            return;
        };
        if self.is_key {
            return;
        }
        if let Some((_, event)) = self.holder(depth)
            && self
                .markers
                .get(&event)
                .is_some_and(|marker| key == marker.field)
            && let Some(frame) = self.stack.get_mut(depth)
        {
            frame.event = Some(event);
            self.drafting = Some(depth);
        }
    }

    /// Whether the array opening at the data object at `depth`'s current
    /// key is the one its item's marker drafts within.
    fn within(&self, depth: usize) -> bool {
        let Some(data) = self.stack.get(depth) else {
            return false;
        };
        let marker = self
            .item_of(depth)
            .filter(|item| self.item.is_none_or(|drafted| drafted == item.item))
            .and_then(|item| item.event.as_ref())
            .and_then(|event| self.markers.get(event));
        marker.is_some_and(|marker| {
            marker.within.is_some() && data.key.as_ref() == marker.within.as_ref()
        })
    }

    /// The marker whose field lives directly in the frame at `depth` (a
    /// data object or an element): its draft's name and its event.
    fn holder(&self, depth: usize) -> Option<(String, String)> {
        let frame = self.stack.get(depth)?;
        if !matches!(frame.role, Role::Data | Role::Element) {
            return None;
        }
        let item = self.item_of(depth)?;
        if self.item.is_some_and(|drafted| drafted != item.item) {
            return None;
        }
        let event = item.event.clone()?;
        let marker = self.markers.get(&event)?;
        if marker.within.is_none() != (frame.role == Role::Data) {
            return None;
        }
        Some((marker.event.clone(), event))
    }

    /// After a key's colon in an item's data: an item whose `event` is not
    /// written yet is the marked event the key tells ([`shapes`]).
    fn shape(&mut self) {
        let Some(frame) = self.stack.last() else {
            return;
        };
        if frame.role != Role::Data {
            return;
        }
        let Some(event) = frame
            .key
            .as_ref()
            .and_then(|key| self.shapes.get(key))
            .cloned()
        else {
            return;
        };
        let item = frame.item;
        if let Some(frame) = self
            .stack
            .iter_mut()
            .rev()
            .find(|frame| frame.role == Role::Item && frame.item == item)
            && frame.event.is_none()
        {
            frame.event = Some(event);
        }
    }

    /// After a key's colon: start reading its value when it is a leading
    /// field of the item's marker.
    fn lead(&mut self) {
        let Some(depth) = self.stack.len().checked_sub(1) else {
            return;
        };
        let Some(key) = self.stack.get(depth).and_then(|frame| frame.key.clone()) else {
            return;
        };
        let Some((_, event)) = self.holder(depth) else {
            return;
        };
        let leads = self
            .markers
            .get(&event)
            .is_some_and(|marker| marker.leading.contains(&key));
        // A leading field written after the drafted field began is late:
        // only the final selection carries it.
        let late = self
            .stack
            .get(depth)
            .is_some_and(|frame| frame.event.is_some());
        if leads && !late {
            self.capture = Some(Capture {
                depth,
                key,
                text: String::new(),
            });
        }
    }

    fn item_of(&self, depth: usize) -> Option<&Frame> {
        let item = self.stack.get(depth)?.item;
        self.stack
            .iter()
            .rev()
            .find(|frame| frame.role == Role::Item && frame.item == item)
    }

    fn end_string(&mut self, drafts: &mut Vec<Draft>) {
        let value = std::mem::take(&mut self.text);
        let depth = self.stack.len().checked_sub(1);
        if self.is_key {
            if let Some(frame) = depth.and_then(|depth| self.stack.get_mut(depth))
                && frame.kind == Kind::Object
            {
                frame.key = Some(value);
                self.mode = Mode::Colon;
            }
            return;
        }
        if self.drafting.is_some() {
            self.text = value;
            self.flush(drafts);
            self.text.clear();
            self.drafting = None;
        } else if let Some(frame) = depth.and_then(|depth| self.stack.get_mut(depth))
            && frame.role == Role::Item
            && frame.key.as_deref() == Some("event")
        {
            frame.event = Some(value);
        }
        self.value_done();
    }

    fn append(&mut self, text: char) {
        if self.high.take().is_some() {
            self.text.push('\u{fffd}');
        }
        self.text.push(text);
    }

    /// One character inside a string: its text, an escape, or its end.
    fn string_step(&mut self, character: char, drafts: &mut Vec<Draft>) -> bool {
        match self.mode {
            Mode::String => {
                if character == '"' {
                    if self.high.take().is_some() {
                        self.text.push('\u{fffd}');
                    }
                    self.end_string(drafts);
                } else if character == '\\' {
                    self.mode = Mode::Escape;
                } else if character < ' ' {
                    return false;
                } else {
                    self.append(character);
                }
                true
            }
            Mode::Escape => {
                if character == 'u' {
                    self.hex.clear();
                    self.mode = Mode::Unicode;
                    return true;
                }
                let escaped = match character {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return false,
                };
                self.append(escaped);
                self.mode = Mode::String;
                true
            }
            Mode::Unicode => {
                if !character.is_ascii_hexdigit() {
                    return false;
                }
                self.hex.push(character);
                if self.hex.len() < 4 {
                    return true;
                }
                let Ok(code) = u32::from_str_radix(&self.hex, 16) else {
                    return false;
                };
                self.mode = Mode::String;
                match (self.high, code) {
                    (Some(high), 0xDC00..=0xDFFF) => {
                        self.high = None;
                        let scalar = 0x10000 + ((high - 0xD800) << 10) + (code - 0xDC00);
                        self.text.push(char::from_u32(scalar).unwrap_or('\u{fffd}'));
                    }
                    (_, 0xD800..=0xDBFF) => {
                        if self.high.replace(code).is_some() {
                            self.text.push('\u{fffd}');
                        }
                    }
                    _ => self.append(char::from_u32(code).unwrap_or('\u{fffd}')),
                }
                true
            }
            _ => false,
        }
    }

    fn step(&mut self, character: char, drafts: &mut Vec<Draft>) -> bool {
        if self.mode == Mode::Literal && !is_literal(character) {
            // A literal ends just before the character that follows it.
            self.value_done();
        }
        if let Some(capture) = &mut self.capture {
            if exceeds_wasm_bound(capture.text.len() + 1, MAX_CAPTURE_BYTES) {
                self.capture = None;
            } else {
                capture.text.push(character);
            }
        }
        match self.mode {
            Mode::String | Mode::Escape | Mode::Unicode => self.string_step(character, drafts),
            Mode::Literal => true,
            _ if matches!(character, ' ' | '\t' | '\r' | '\n') => true,
            Mode::Value => match character {
                '"' => {
                    self.is_key = false;
                    self.mode = Mode::String;
                    self.start_string();
                    true
                }
                '{' => self.open(Kind::Object),
                '[' => self.open(Kind::Array),
                ']' if self
                    .stack
                    .last()
                    .is_some_and(|frame| frame.kind == Kind::Array && frame.count == 0) =>
                {
                    self.close(Kind::Array, drafts)
                }
                _ if is_literal(character) => {
                    self.mode = Mode::Literal;
                    true
                }
                _ => false,
            },
            Mode::Key => {
                if character == '"' {
                    self.is_key = true;
                    self.mode = Mode::String;
                    self.start_string();
                    return true;
                }
                character == '}'
                    && self.stack.last().is_some_and(|frame| frame.count == 0)
                    && self.close(Kind::Object, drafts)
            }
            Mode::Colon => {
                if character != ':' {
                    return false;
                }
                self.mode = Mode::Value;
                self.shape();
                self.lead();
                true
            }
            Mode::Next => match (character, self.stack.last().map(|frame| frame.kind)) {
                (',', Some(Kind::Object)) => {
                    self.mode = Mode::Key;
                    true
                }
                (',', Some(Kind::Array)) => {
                    self.mode = Mode::Value;
                    true
                }
                ('}', _) => self.close(Kind::Object, drafts),
                (']', _) => self.close(Kind::Array, drafts),
                _ => false,
            },
            Mode::End => false,
        }
    }
}

const fn is_literal(character: char) -> bool {
    matches!(
        character,
        '0'..='9' | '+' | '-' | '.' | 'e' | 'E' | 't' | 'r' | 'u' | 'f' | 'a' | 'l' | 's' | 'n'
    )
}

/// The arguments of a streamed reply's first tool call (`index` 0) as
/// they arrive: each read of the body gives the fragments of
/// `delta.tool_calls[0].function.arguments` its complete `data:` lines
/// carry, in order. Lines that are not JSON chunks are skipped; the whole
/// reply is still read and checked by [`crate::stream_reply`].
#[derive(Debug, Default)]
pub struct Arguments {
    scanned: usize,
}

impl Arguments {
    /// The argument fragments of the complete lines of `body` (all of it
    /// so far) not read before.
    pub fn read(&mut self, body: &[u8]) -> String {
        let mut fragments = String::new();
        let mut start = self.scanned;
        for (offset, byte) in body.iter().enumerate().skip(self.scanned) {
            if *byte != b'\n' {
                continue;
            }
            let line = body.get(start..offset).unwrap_or_default();
            start = offset + 1;
            let Some(data) = line.strip_prefix(b"data:") else {
                continue;
            };
            let Ok(chunk) = serde_json::from_slice::<Json>(data) else {
                continue;
            };
            for choice in chunk
                .get("choices")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter(|choice| choice.get("index").and_then(Json::as_u64).unwrap_or(0) == 0)
            {
                for call in choice
                    .pointer("/delta/tool_calls")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|call| call.get("index").and_then(Json::as_u64) == Some(0))
                {
                    if let Some(arguments) =
                        call.pointer("/function/arguments").and_then(Json::as_str)
                    {
                        fragments.push_str(arguments);
                    }
                }
            }
        }
        self.scanned = start;
        fragments
    }
}

/// The least logical time between two returns of drafts, in nanoseconds
/// (0.1 s): a model writes a token every few milliseconds, and each draft
/// the invoking session takes costs it a macrostep or more, so the drafts
/// of one interval are returned together, each field's continuing text
/// joined into one draft ([`Draft::absorb`]).
pub const DRAFT_INTERVAL_NS: u64 = 100_000_000;

/// The drafts read and not yet returned, and when drafts were last
/// returned.
#[derive(Debug, Default)]
pub struct Pending {
    drafts: Vec<Draft>,
    last: Option<u64>,
}

impl Pending {
    /// Holds `drafts`, joining each to the one before it when it continues
    /// it.
    pub fn extend(&mut self, drafts: Vec<Draft>) {
        for draft in drafts {
            if !self
                .drafts
                .last_mut()
                .is_some_and(|last| last.absorb(&draft))
            {
                self.drafts.push(draft);
            }
        }
    }

    /// The drafts to return at the logical time `now` (nanoseconds): all
    /// that are held once [`DRAFT_INTERVAL_NS`] passed since the last
    /// return (the first return is due at once), or when the reply `ended`;
    /// none otherwise.
    pub fn due(&mut self, now: u64, ended: bool) -> Vec<Draft> {
        if self.drafts.is_empty() {
            return Vec::new();
        }
        let due = ended
            || self
                .last
                .is_none_or(|last| now.saturating_sub(last) >= DRAFT_INTERVAL_NS);
        if !due {
            return Vec::new();
        }
        self.last = Some(now);
        std::mem::take(&mut self.drafts)
    }
}

#[cfg(test)]
mod tests {
    use super::{Arguments, Draft, DraftParser, Marker, Pending, markers, shapes};
    use serde_json::{Map, Value as Json, json};
    use std::collections::BTreeMap;

    const SAY: &str = "tests.processing.say";
    const LINES: &str = "tests.processing.lines";
    const DRAFTED: &str = "tests.processing.say.draft";

    fn marked(event: &str, marker: &Json) -> BTreeMap<String, Marker> {
        markers(&json!({ event: marker })).expect("the marker is well formed")
    }

    fn said() -> BTreeMap<String, Marker> {
        marked(
            SAY,
            &json!({"field": "text", "event": DRAFTED, "leading": ["tone", "level"]}),
        )
    }

    fn lines() -> BTreeMap<String, Marker> {
        marked(
            LINES,
            &json!({"field": "words", "event": DRAFTED, "within": "parts", "leading": ["how"]}),
        )
    }

    fn drafted(markers: &BTreeMap<String, Marker>, pieces: &[&str]) -> Vec<Draft> {
        let mut parser = DraftParser::new(markers.clone());
        pieces.iter().flat_map(|piece| parser.feed(piece)).collect()
    }

    fn by_character(markers: &BTreeMap<String, Marker>, arguments: &str) -> Vec<Draft> {
        let pieces: Vec<String> = arguments.chars().map(String::from).collect();
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        drafted(markers, &pieces)
    }

    /// Each part's words as its deltas wrote them, each delta starting
    /// where its part's last one ended.
    fn parts_of(drafts: &[Draft]) -> BTreeMap<Option<usize>, String> {
        let mut written: BTreeMap<Option<usize>, String> = BTreeMap::new();
        for draft in drafts {
            let part = written.entry(draft.part).or_default();
            assert_eq!(
                draft.offset,
                part.chars().count(),
                "a draft continues its part"
            );
            part.push_str(&draft.text);
        }
        written
    }

    fn every_cut(markers: &BTreeMap<String, Marker>, arguments: &str) -> Vec<Vec<Draft>> {
        let bounds: Vec<usize> = arguments
            .char_indices()
            .map(|(at, _)| at)
            .chain([arguments.len()])
            .collect();
        bounds
            .into_iter()
            .map(|cut| drafted(markers, &[&arguments[..cut], &arguments[cut..]]))
            .collect()
    }

    const SAY_ARGS: &str = r#"{"events": [{"event": "tests.processing.other", "data": {"text": "no"}}, {"event": "tests.processing.say", "target": "voice", "data": {"text": "Hi \"there\", café 😀!"}}]}"#;

    #[test]
    fn a_marked_field_is_drafted_as_append_only_deltas_however_the_arguments_are_split() {
        let markers = said();
        let whole = drafted(&markers, &[SAY_ARGS]);
        let text = "Hi \"there\", café 😀!";
        assert_eq!(whole.len(), 1);
        let first = whole.first().expect("one draft");
        assert_eq!(
            (
                first.text.as_str(),
                first.offset,
                first.item,
                first.part,
                first.seq
            ),
            (text, 0, 1, None, 0)
        );
        assert_eq!((first.name.as_str(), first.event.as_str()), (DRAFTED, SAY));
        for drafts in every_cut(&markers, SAY_ARGS) {
            assert_eq!(parts_of(&drafts), BTreeMap::from([(None, text.to_owned())]));
            let seqs: Vec<usize> = drafts.iter().map(|draft| draft.seq).collect();
            assert_eq!(seqs, (0..drafts.len()).collect::<Vec<_>>());
        }
        assert_eq!(
            parts_of(&by_character(&markers, SAY_ARGS)),
            BTreeMap::from([(None, text.to_owned())])
        );
    }

    #[test]
    fn nothing_is_drafted_when_the_data_comes_before_the_event_name() {
        let arguments =
            r#"{"events": [{"data": {"text": "Too early."}, "event": "tests.processing.say"}]}"#;
        assert!(drafted(&said(), &[arguments]).is_empty());
    }

    #[test]
    fn malformed_arguments_stop_drafting_and_never_fail() {
        let arguments = r#"{"events": [{"event": "tests.processing.say", "data": {"text": "Fine so far"}}]] }} {"text": "x"}"#;
        let mut parser = DraftParser::new(said());
        let drafts = parser.feed(arguments);
        assert_eq!(
            parts_of(&drafts),
            BTreeMap::from([(None, "Fine so far".to_owned())])
        );
        assert!(parser.failed());
        assert!(
            parser
                .feed(
                    r#"{"events": [{"event": "tests.processing.say", "data": {"text": "more"}}]}"#
                )
                .is_empty()
        );
    }

    #[test]
    fn leading_fields_that_close_before_the_text_ride_its_first_draft_only() {
        let tone = json!({"mood": "dry \"wry\"", "tags": ["a", {"b": null}], "on": true});
        let arguments = format!(
            r#"{{"events": [{{"event": "tests.processing.say", "target": "voice", "data": {{"tone": {tone}, "level": -0.5e1, "note": "not listed", "text": "Hi there, friend"}}}}]}}"#
        );
        let leading: Map<String, Json> = json!({"tone": tone, "level": -5.0})
            .as_object()
            .cloned()
            .expect("an object");
        let markers = said();
        let whole = drafted(&markers, &[&arguments]);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole.first().map(|draft| &draft.leading), Some(&leading));
        for drafts in every_cut(&markers, &arguments)
            .into_iter()
            .chain([by_character(&markers, &arguments)])
        {
            assert_eq!(drafts.first().map(|draft| &draft.leading), Some(&leading));
            assert!(drafts.iter().skip(1).all(|draft| draft.leading.is_empty()));
        }
    }

    #[test]
    fn leading_fields_that_are_late_or_do_not_decode_are_left_out() {
        let arguments = r#"{"events": [{"event": "tests.processing.say", "data": {"level": tru, "text": "Said", "tone": {"mood": "x"}}}]}"#;
        let drafts = drafted(&said(), &[arguments]);
        assert_eq!(drafts.len(), 1);
        assert!(
            drafts
                .iter()
                .all(|draft| draft.leading.is_empty() && draft.text == "Said")
        );
    }

    #[test]
    fn a_marker_that_names_its_field_as_leading_or_within_is_refused() {
        assert!(
            markers(
                &json!({SAY: {"field": "text", "event": DRAFTED, "leading": ["tone", "text"]}})
            )
            .is_err()
        );
        assert!(
            markers(&json!({SAY: {"field": "words", "event": DRAFTED, "within": "words"}}))
                .is_err()
        );
        assert!(markers(&json!({SAY: {"field": "words"}})).is_err());
        assert!(markers(&json!({SAY: {"field": "words", "event": DRAFTED, "extra": 1}})).is_err());
        assert!(markers(&json!([])).is_err());
    }

    #[test]
    fn each_part_of_a_within_array_is_drafted_with_its_own_leading_however_split() {
        let how = json!({"mood": "dry", "nested": {"words": "not drafted", "list": [1, {"words": "no"}]}});
        let arguments = format!(
            r#"{{"events": [{{"event": "tests.processing.lines", "target": "voice", "data": {{"words": "not a part", "parts": [{{"how": {how}, "words": "Oh, "}}, {{"words": "you \"got\" me."}}, {{"how": {{"pace": 1.2}}, "words": "Ha"}}]}}}}]}}"#
        );
        let markers = lines();
        let expected = BTreeMap::from([
            (Some(0), "Oh, ".to_owned()),
            (Some(1), "you \"got\" me.".to_owned()),
            (Some(2), "Ha".to_owned()),
        ]);
        let whole = drafted(&markers, &[&arguments]);
        assert_eq!(whole.len(), 3);
        assert_eq!(parts_of(&whole), expected);
        for drafts in every_cut(&markers, &arguments)
            .into_iter()
            .chain([by_character(&markers, &arguments)])
        {
            assert_eq!(parts_of(&drafts), expected);
            let openings: Vec<&Map<String, Json>> = drafts
                .iter()
                .filter(|draft| draft.offset == 0)
                .map(|draft| &draft.leading)
                .collect();
            assert_eq!(
                openings,
                [
                    json!({"how": how}).as_object().expect("an object"),
                    &Map::new(),
                    json!({"how": {"pace": 1.2}})
                        .as_object()
                        .expect("an object"),
                ]
            );
            assert!(
                drafts
                    .iter()
                    .filter(|draft| draft.offset > 0)
                    .all(|draft| draft.leading.is_empty())
            );
        }
    }

    #[test]
    fn an_element_written_without_its_field_opens_once_in_order() {
        let arguments = r#"{"events": [{"event": "tests.processing.lines", "data": {"parts": [{"words": "Oh, "}, {"how": {"sound": "chuckle"}}, {"how": {"pause": "short"}, "words": "So."}, {"how": {"pause": "long"}}]}}]}"#;
        let markers = lines();
        for drafts in every_cut(&markers, arguments) {
            let openings: Vec<Option<usize>> = drafts
                .iter()
                .filter(|draft| draft.offset == 0)
                .map(|draft| draft.part)
                .collect();
            assert_eq!(openings, [Some(0), Some(1), Some(2), Some(3)]);
            assert_eq!(
                parts_of(&drafts),
                BTreeMap::from([
                    (Some(0), "Oh, ".to_owned()),
                    (Some(1), String::new()),
                    (Some(2), "So.".to_owned()),
                    (Some(3), String::new()),
                ])
            );
        }
    }

    #[test]
    fn no_part_written_before_the_event_name_is_drafted() {
        let arguments = r#"{"events": [{"data": {"parts": [{"words": "Too early."}]}, "event": "tests.processing.lines"}]}"#;
        assert!(drafted(&lines(), &[arguments]).is_empty());
    }

    #[test]
    fn drafts_held_in_one_interval_are_returned_together_each_field_joined() {
        let markers = lines();
        let arguments = r#"{"events": [{"event": "tests.processing.lines", "data": {"parts": [{"words": "One. Two."}, {"words": "Three."}]}}]}"#;
        let mut pending = Pending::default();
        let mut returned = Vec::new();
        let mut parser = DraftParser::new(markers);
        for (at, character) in arguments.chars().enumerate() {
            pending.extend(parser.feed(&character.to_string()));
            // A read every millisecond of logical time.
            returned.extend(pending.due(u64::try_from(at).expect("small") * 1_000_000, false));
        }
        returned.extend(pending.due(u64::MAX, true));
        assert!(returned.len() < 10, "{} drafts", returned.len());
        assert_eq!(
            parts_of(&returned),
            BTreeMap::from([
                (Some(0), "One. Two.".to_owned()),
                (Some(1), "Three.".to_owned())
            ])
        );
        assert_eq!(
            returned.first().map(Draft::data),
            Some(
                json!({"text": "O", "offset": 0, "event": LINES, "item": 0, "part": 0, "seq": 0, "leading": {}})
            )
        );
    }
    fn dispatch(branches: &[(&str, &[&str])]) -> Json {
        let branches: Vec<Json> = branches
            .iter()
            .map(|(event, keys)| {
                let properties: Map<String, Json> =
                    keys.iter().map(|key| ((*key).to_owned(), json!({}))).collect();
                json!({"properties": {"event": {"enum": [event]}, "data": {"properties": properties}}})
            })
            .collect();
        json!({"properties": {"events": {"items": {"anyOf": branches}}}})
    }

    #[test]
    fn a_part_written_before_the_event_name_is_drafted_when_only_its_event_has_that_data() {
        let arguments = r#"{"events": [{"data": {"parts": [{"words": "Not too early."}]}, "event": "tests.processing.lines"}]}"#;
        let markers = lines();
        let told = shapes(
            &dispatch(&[
                (LINES, &["parts", "words"]),
                ("tests.processing.other", &["words"]),
            ]),
            &markers,
        );
        assert_eq!(
            told,
            BTreeMap::from([("parts".to_owned(), LINES.to_owned())])
        );
        let mut parser = DraftParser::new(markers.clone()).with_shapes(told);
        assert_eq!(
            parts_of(&parser.feed(arguments)),
            BTreeMap::from([(Some(0), "Not too early.".to_owned())])
        );
        // A key two offered events share tells neither.
        let shared = shapes(
            &dispatch(&[(LINES, &["parts"]), ("tests.processing.other", &["parts"])]),
            &markers,
        );
        assert!(shared.is_empty());
        assert!(
            DraftParser::new(markers)
                .with_shapes(shared)
                .feed(arguments)
                .is_empty()
        );
    }

    #[test]
    fn only_the_first_item_drafted_in_a_call_is_drafted() {
        let arguments = r#"{"events": [{"event": "tests.processing.lines", "data": {"parts": [{"words": "First."}]}}, {"event": "tests.processing.lines", "data": {"parts": [{"words": "Second."}]}}]}"#;
        let drafts = drafted(&lines(), &[arguments]);
        assert!(drafts.iter().all(|draft| draft.item == 0));
        assert_eq!(
            parts_of(&drafts),
            BTreeMap::from([(Some(0), "First.".to_owned())])
        );
    }

    #[test]
    fn arguments_are_read_from_complete_stream_lines_only() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"ev\"}}]}}]}\n\n",
            ": keep-alive\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"other\"}}]}}]}\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ents\\\"\"}}]}}]}\n",
            "data: [DONE]\n",
        )
        .as_bytes();
        let cut = body.len() - 30;
        let mut arguments = Arguments::default();
        let mut read = arguments.read(&body[..cut]);
        read.push_str(&arguments.read(body));
        assert_eq!(read, "{\"events\"");
    }
}
