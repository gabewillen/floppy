//! Document bytes → validated [`Document`], and the full issue list.

use std::collections::BTreeSet;

use scxml_types::{EventName, InvokeId, SendId, SessionId, StateId};

use crate::encode::{MACHINE_IR_VERSION, sha256_hex};
use crate::{
    BasicHttpMessage, Children, ComponentRole, Document, Event, EventSchema, MAX_BASICHTTP_PARAMS,
    MAX_COND_BYTES, MAX_CONFIGURATION_STATES, MAX_DOCUMENT_BYTES, MAX_EVENT_SCHEMA_BYTES,
    MAX_INVOKE_DEPTH, MAX_INVOKED_SESSIONS, MAX_PENDING_SENDS, MAX_POINTER_BYTES,
    MAX_QUEUE_ENTRIES, MAX_SESSION_ID_BYTES, MAX_TRANSITIONS, MAX_TYPEOF_BYTES, MAX_VALUE_BYTES,
    MAX_VARIABLES, SESSION_SNAPSHOT_VERSION, Section, Session, SnapshotError, SnapshotErrorCode,
    SnapshotPoint, Status, TraceContext, TraceContextPolicy, Value, VariableValue,
};

/// Largest JSON integer the document may hold (2^53 − 1).
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
/// Largest event metadata string (`origin`, `origintype`, `sendid`, …).
const MAX_METADATA_BYTES: usize = scxml_types::MAX_EVENT_METADATA_BYTES;

/// An RFC 6901 pointer built only from fixed keys and indices.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pointer(String);

impl Pointer {
    pub(crate) const fn root() -> Self {
        Self(String::new())
    }

    pub(crate) fn push_key(&mut self, key: &'static str) {
        self.0.push('/');
        self.0.push_str(key);
    }

    pub(crate) fn push_index(&mut self, index: usize) {
        use std::fmt::Write as _;
        // Discarded: `fmt::Write for String` never returns an error.
        let _ = write!(self.0, "/{index}");
    }

    /// Appends a field name the closed model matched, escaped per RFC 6901
    /// (no model field contains `~` or `/`; the escape keeps the pointer
    /// well formed regardless).
    fn push_field(&mut self, key: &str) {
        self.0.push('/');
        self.0.push_str(&key.replace('~', "~0").replace('/', "~1"));
    }

    fn with_key(&self, key: &'static str) -> Self {
        let mut next = self.clone();
        next.push_key(key);
        next
    }

    fn with_index(&self, index: usize) -> Self {
        let mut next = self.clone();
        next.push_index(index);
        next
    }

    pub(crate) fn into_string(self) -> String {
        let mut text = self.0;
        if text.len() > MAX_POINTER_BYTES {
            let mut end = MAX_POINTER_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        text
    }
}

/// Decodes and fully validates document bytes.
///
/// Checks run in this order and the first failure is returned: size, JSON,
/// `version`, shape (unknown fields rejected), re-encode byte equality,
/// `sha256`, bounds, then names, canonical values, and consistency.
///
/// # Errors
///
/// The first failed check.
pub fn decode(bytes: &[u8]) -> Result<Document, SnapshotError> {
    let (document, issues) = decode_with_issues(bytes)?;
    match issues.into_iter().next() {
        Some(first) => Err(first),
        None => Ok(document),
    }
}

/// Validates document bytes and returns every issue found (empty when the
/// document is valid). Structural failures (version, shape, canonical form,
/// digest) end the check with that single issue.
///
/// # Errors
///
/// [`SnapshotErrorCode::TooLarge`] or [`SnapshotErrorCode::MalformedJson`]
/// when no issue list can be built.
pub fn check(bytes: &[u8]) -> Result<Vec<SnapshotError>, SnapshotError> {
    match decode_with_issues(bytes) {
        Ok((_, issues)) => Ok(issues),
        Err(error)
            if matches!(
                error.code,
                SnapshotErrorCode::TooLarge | SnapshotErrorCode::MalformedJson
            ) =>
        {
            Err(error)
        }
        Err(error) => Ok(vec![error]),
    }
}

fn decode_with_issues(bytes: &[u8]) -> Result<(Document, Vec<SnapshotError>), SnapshotError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SnapshotError::new(SnapshotErrorCode::TooLarge, None));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| SnapshotError::new(SnapshotErrorCode::MalformedJson, None))?;
    if !value.is_object() {
        return Err(SnapshotError::new(
            SnapshotErrorCode::SchemaViolation,
            Some(Pointer::root().into_string()),
        ));
    }
    match value.get("version").and_then(serde_json::Value::as_str) {
        Some(SESSION_SNAPSHOT_VERSION) => {}
        Some(_) => {
            return Err(SnapshotError::new(
                SnapshotErrorCode::UnsupportedVersion,
                Some("/version".into()),
            ));
        }
        None => {
            return Err(SnapshotError::new(
                SnapshotErrorCode::SchemaViolation,
                Some("/version".into()),
            ));
        }
    }
    let document: Document = serde_path_to_error::deserialize(value).map_err(|error| {
        SnapshotError::new(
            SnapshotErrorCode::SchemaViolation,
            Some(shape_pointer(&error).into_string()),
        )
    })?;
    let reencoded = serde_json::to_vec(&document)
        .map_err(|_| SnapshotError::new(SnapshotErrorCode::SchemaViolation, None))?;
    if reencoded != bytes {
        return Err(SnapshotError::new(SnapshotErrorCode::NonCanonical, None));
    }
    let snapshot_bytes = serde_json::to_vec(&document.snapshot)
        .map_err(|_| SnapshotError::new(SnapshotErrorCode::SchemaViolation, None))?;
    if sha256_hex(&snapshot_bytes) != document.sha256 {
        return Err(SnapshotError::new(
            SnapshotErrorCode::IntegrityMismatch,
            Some("/sha256".into()),
        ));
    }
    let mut issues = Vec::new();
    validate_document(&document, &mut issues);
    Ok((document, issues))
}

/// The RFC 6901 pointer of a shape failure: the value that failed to
/// deserialize, or, for an unknown key, the object that holds it (the key
/// itself is document text and never reaches a pointer). Every other map
/// segment is a field name the closed model matched, and every sequence
/// segment an index, so the pointer carries fixed keys and indices only.
/// A failure inside a buffered (tagged or untagged) enum is located at the
/// enum value.
fn shape_pointer(error: &serde_path_to_error::Error<serde_json::Error>) -> Pointer {
    let segments: Vec<&serde_path_to_error::Segment> = error.path().iter().collect();
    let unknown_key = match segments.last() {
        Some(serde_path_to_error::Segment::Map { key }) => error
            .inner()
            .to_string()
            .starts_with(&format!("unknown field `{key}`")),
        _ => false,
    };
    let kept = if unknown_key {
        &segments[..segments.len() - 1]
    } else {
        &segments[..]
    };
    let mut pointer = Pointer::root();
    for segment in kept {
        match segment {
            serde_path_to_error::Segment::Seq { index } => pointer.push_index(*index),
            serde_path_to_error::Segment::Map { key } => pointer.push_field(key),
            serde_path_to_error::Segment::Enum { .. } | serde_path_to_error::Segment::Unknown => {}
        }
    }
    pointer
}

/// Runs the bounds, name, canonical-value, and consistency passes over a
/// document, appending issues in check order.
pub(crate) fn validate_document(document: &Document, issues: &mut Vec<SnapshotError>) {
    let mut passes = Passes::default();
    passes.document(document);
    issues.extend(passes.bounds);
    issues.extend(passes.names);
    issues.extend(passes.consistency);
}

#[derive(Default)]
struct Passes {
    bounds: Vec<SnapshotError>,
    names: Vec<SnapshotError>,
    consistency: Vec<SnapshotError>,
}

fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A number string is canonical when it matches the schema pattern and is
/// exactly Rust's `f64` `Display` of its value.
fn is_canonical_number(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (integer, fraction) = match digits.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (digits, None),
    };
    let integer_ok = integer == "0"
        || (!integer.is_empty()
            && !integer.starts_with('0')
            && integer.bytes().all(|byte| byte.is_ascii_digit()));
    let fraction_ok = fraction.is_none_or(|fraction| {
        !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit())
    });
    if !integer_ok || !fraction_ok {
        return false;
    }
    text.parse::<f64>()
        .is_ok_and(|number| number.is_finite() && number.to_string() == text)
}

impl Passes {
    fn bound(&mut self, pointer: &Pointer) {
        self.bounds.push(SnapshotError::new(
            SnapshotErrorCode::BoundsExceeded,
            Some(pointer.clone().into_string()),
        ));
    }

    fn name(&mut self, pointer: &Pointer) {
        self.names.push(SnapshotError::new(
            SnapshotErrorCode::InvalidName,
            Some(pointer.clone().into_string()),
        ));
    }

    fn fail(&mut self, code: SnapshotErrorCode, pointer: &Pointer) {
        self.consistency.push(SnapshotError::new(
            code,
            Some(pointer.clone().into_string()),
        ));
    }

    fn integer(&mut self, value: u64, pointer: &Pointer) {
        if value > MAX_SAFE_INTEGER {
            self.bound(pointer);
        }
    }

    fn count(&mut self, len: usize, max: usize, pointer: &Pointer) {
        if len > max {
            self.bound(pointer);
        }
    }

    fn text(&mut self, text: &str, max: usize, pointer: &Pointer) {
        if text.len() > max {
            self.bound(pointer);
        }
    }

    fn state(&mut self, id: &str, pointer: &Pointer) {
        if StateId::new(id).is_err() {
            self.name(pointer);
        }
    }

    fn states(&mut self, ids: &[String], pointer: &Pointer) {
        self.count(ids.len(), MAX_CONFIGURATION_STATES, pointer);
        for (index, id) in ids.iter().enumerate() {
            self.state(id, &pointer.with_index(index));
        }
    }

    fn unique_states(&mut self, ids: &[String], pointer: &Pointer) {
        let mut seen = BTreeSet::new();
        if !ids.iter().all(|id| seen.insert(id.as_str())) {
            self.fail(SnapshotErrorCode::Inconsistent, pointer);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear pass over the document envelope and the invoked sessions"
    )]
    fn document(&mut self, document: &Document) {
        let options = Pointer::root().with_key("options");
        let exclude = &document.options.exclude;
        if !exclude.windows(2).all(|pair| pair[0] < pair[1]) {
            self.fail(
                SnapshotErrorCode::NonCanonical,
                &options.with_key("exclude"),
            );
        }
        let snapshot = Pointer::root().with_key("snapshot");
        let components_pointer = snapshot.with_key("components");
        let components = &document.snapshot.components;
        for (index, component) in components.iter().enumerate() {
            if !is_digest(&component.sha256) {
                self.fail(
                    SnapshotErrorCode::SchemaViolation,
                    &components_pointer.with_index(index).with_key("sha256"),
                );
            }
        }
        if !components
            .windows(2)
            .all(|pair| pair[0].role < pair[1].role)
        {
            self.fail(SnapshotErrorCode::Inconsistent, &components_pointer);
        }
        let roles: BTreeSet<ComponentRole> = components.iter().map(|c| c.role).collect();
        for required in [
            ComponentRole::Clock,
            ComponentRole::IoProcessorScxml,
            ComponentRole::Machine,
        ] {
            if !roles.contains(&required) {
                self.fail(SnapshotErrorCode::Inconsistent, &components_pointer);
            }
        }
        self.integer(
            document.snapshot.clock.now_ms,
            &snapshot.with_key("clock").with_key("now_ms"),
        );
        // An executable-content document holds one session, possibly an invoked
        // one, and never invocations or previews.
        let executable_content = document.options.point == SnapshotPoint::ExecutableContent;
        if executable_content
            && !(exclude.contains(&Section::Invocations) && exclude.contains(&Section::Previews))
        {
            self.fail(SnapshotErrorCode::Inconsistent, &options.with_key("point"));
        }
        let context = Context {
            exclude,
            strip: document.options.trace_context == TraceContextPolicy::Strip,
            roles: &roles,
            executable_content,
        };
        let root = &document.snapshot.root;
        let root_pointer = snapshot.with_key("root");
        self.session(root, &root_pointer, &context);
        // A document of a whole run starts at its root session, which no
        // session invoked; one session of the run alone (`invocations`
        // excluded) may be an invoked one.
        if root.invoked_by.is_some() && !exclude.contains(&Section::Invocations) {
            self.fail(
                SnapshotErrorCode::Inconsistent,
                &root_pointer.with_key("invoked_by"),
            );
        }
        let children = Children::under(exclude);
        let mut invoked = InvokedSeen {
            session_ids: BTreeSet::from([root.session_id.clone()]),
            invoked_sessions: 0,
        };
        self.invocations(root, &root_pointer, &context, children, 0, &mut invoked);
    }

    /// Checks the `invocations` of `parent` (at `depth`) and, recursively,
    /// every session they hold: presence per the `invocations` exclusion,
    /// ascending unique invoke ids, `invoked_by` links, unique session ids,
    /// and the invoke bounds [`MAX_INVOKE_DEPTH`] and [`MAX_INVOKED_SESSIONS`].
    fn invocations(
        &mut self,
        parent: &Session,
        parent_pointer: &Pointer,
        context: &Context<'_>,
        children: Children,
        depth: usize,
        invoked: &mut InvokedSeen,
    ) {
        let invocations_pointer = parent_pointer.with_key("invocations");
        let invocations = match (&parent.invocations, children) {
            (Some(_), Children::Excluded) | (None, Children::Included) => {
                self.fail(SnapshotErrorCode::Inconsistent, &invocations_pointer);
                return;
            }
            (None, Children::Excluded) => return,
            (Some(invocations), Children::Included) => invocations,
        };
        if !invocations.is_empty() && depth >= MAX_INVOKE_DEPTH {
            self.fail(SnapshotErrorCode::DepthExceeded, &invocations_pointer);
            return;
        }
        invoked.invoked_sessions = invoked.invoked_sessions.saturating_add(invocations.len());
        if invoked.invoked_sessions > MAX_INVOKED_SESSIONS {
            self.fail(SnapshotErrorCode::BoundsExceeded, &invocations_pointer);
            return;
        }
        if !invocations
            .windows(2)
            .all(|pair| pair[0].invoke_id < pair[1].invoke_id)
        {
            self.fail(SnapshotErrorCode::Inconsistent, &invocations_pointer);
        }
        for (index, invocation) in invocations.iter().enumerate() {
            let pointer = invocations_pointer.with_index(index);
            if InvokeId::new(invocation.invoke_id.as_str()).is_err() {
                self.name(&pointer.with_key("invoke_id"));
            }
            self.state(&invocation.state, &pointer.with_key("state"));
            if invocation.invoke_type.is_empty() {
                self.fail(
                    SnapshotErrorCode::SchemaViolation,
                    &pointer.with_key("type"),
                );
            }
            self.text(
                &invocation.invoke_type,
                MAX_METADATA_BYTES,
                &pointer.with_key("type"),
            );
            let child = &invocation.session;
            let child_pointer = pointer.with_key("session");
            self.session(child, &child_pointer, context);
            match &child.invoked_by {
                Some(by)
                    if by.invoke_id == invocation.invoke_id
                        && by.parent_session_id == parent.session_id => {}
                _ => self.fail(
                    SnapshotErrorCode::Inconsistent,
                    &child_pointer.with_key("invoked_by"),
                ),
            }
            if !invoked.session_ids.insert(child.session_id.clone()) {
                self.fail(
                    SnapshotErrorCode::Inconsistent,
                    &child_pointer.with_key("session_id"),
                );
            }
            self.invocations(child, &child_pointer, context, children, depth + 1, invoked);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear pass over the session keys in document order"
    )]
    fn session(&mut self, session: &Session, pointer: &Pointer, context: &Context<'_>) {
        let excluded = |section| context.exclude.contains(&section);
        let session_id = pointer.with_key("session_id");
        self.text(&session.session_id, MAX_SESSION_ID_BYTES, &session_id);
        if SessionId::new(session.session_id.as_str()).is_err() {
            self.name(&session_id);
        }
        if let Some(name) = &session.name {
            if name.is_empty() {
                self.name(&pointer.with_key("name"));
            }
            self.text(name, MAX_SESSION_ID_BYTES, &pointer.with_key("name"));
        }
        if let Some(name) = &session.datamodel_name {
            let role = match name.as_str() {
                "ecmascript" => Some(ComponentRole::DataModelEcmascript),
                "null" => Some(ComponentRole::DataModelNull),
                "xpath" => Some(ComponentRole::DataModelXpath),
                _ => None,
            };
            if !role.is_some_and(|role| context.roles.contains(&role)) {
                self.fail(
                    SnapshotErrorCode::Inconsistent,
                    &pointer.with_key("datamodel_name"),
                );
            }
        }
        let ir = pointer.with_key("ir");
        if session.ir.version != MACHINE_IR_VERSION {
            self.fail(
                SnapshotErrorCode::UnsupportedVersion,
                &ir.with_key("version"),
            );
        }
        if !is_digest(&session.ir.sha256) {
            self.fail(SnapshotErrorCode::SchemaViolation, &ir.with_key("sha256"));
        }
        if let Some(by) = &session.invoked_by {
            let by_pointer = pointer.with_key("invoked_by");
            if SessionId::new(by.parent_session_id.as_str()).is_err() {
                self.name(&by_pointer.with_key("parent_session_id"));
            }
            if InvokeId::new(by.invoke_id.as_str()).is_err() {
                self.name(&by_pointer.with_key("invoke_id"));
            }
        }
        // Configuration.
        let configuration = pointer.with_key("configuration");
        let atomic = &session.configuration.atomic;
        let full = &session.configuration.full;
        self.states(atomic, &configuration.with_key("atomic"));
        self.states(full, &configuration.with_key("full"));
        self.unique_states(atomic, &configuration.with_key("atomic"));
        self.unique_states(full, &configuration.with_key("full"));
        let full_set: BTreeSet<&str> = full.iter().map(String::as_str).collect();
        let atomic_in_full = atomic.iter().all(|id| full_set.contains(id.as_str()));
        // At a checkpoint a started session is in at least one atomic
        // state; executable content may run before one is entered (the
        // entry content of a compound state, whose children are not yet
        // in the configuration it observes).
        let shape_ok = if session.status == Status::Ready {
            atomic.is_empty() && full.is_empty()
        } else {
            !atomic.is_empty() || context.executable_content
        };
        if !atomic_in_full || !shape_ok {
            self.fail(SnapshotErrorCode::Inconsistent, &configuration);
        }
        // History and bindings.
        let history = pointer.with_key("history");
        self.count(session.history.len(), MAX_CONFIGURATION_STATES, &history);
        let mut history_states = BTreeSet::new();
        for (index, value) in session.history.iter().enumerate() {
            let entry = history.with_index(index);
            self.state(&value.state, &entry.with_key("state"));
            self.states(&value.value, &entry.with_key("value"));
            if !history_states.insert(value.state.as_str()) {
                self.fail(SnapshotErrorCode::Inconsistent, &entry.with_key("state"));
            }
        }
        let bound = pointer.with_key("bound_states");
        self.states(&session.bound_states, &bound);
        self.unique_states(&session.bound_states, &bound);
        let counters = pointer.with_key("counters");
        self.integer(session.counters.send, &counters.with_key("send"));
        self.integer(session.counters.invoke, &counters.with_key("invoke"));
        // Data-model sections: absent when excluded; otherwise absent only
        // when the host gathered no data-model image (`ready`, or no model).
        let may_lack_model = session.status == Status::Ready || session.datamodel_name.is_none();
        let system_pointer = pointer.with_key("system");
        match (&session.system, excluded(Section::System)) {
            (Some(_), true) => self.fail(SnapshotErrorCode::Inconsistent, &system_pointer),
            (None, false) if !may_lack_model => {
                self.fail(SnapshotErrorCode::Inconsistent, &system_pointer);
            }
            (Some(system), false) => {
                if system.sessionid != session.session_id || system.name != session.name {
                    self.fail(SnapshotErrorCode::Inconsistent, &system_pointer);
                }
                self.value(
                    &system.ioprocessors,
                    &system_pointer.with_key("ioprocessors"),
                );
            }
            _ => {}
        }
        let data_model_pointer = pointer.with_key("data_model");
        match (&session.data_model, excluded(Section::DataModel)) {
            (Some(_), true) => self.fail(SnapshotErrorCode::Inconsistent, &data_model_pointer),
            (None, false) if !may_lack_model => {
                self.fail(SnapshotErrorCode::Inconsistent, &data_model_pointer);
            }
            (Some(variables), false) => {
                self.count(variables.len(), MAX_VARIABLES, &data_model_pointer);
                let mut names = BTreeSet::new();
                for (index, variable) in variables.iter().enumerate() {
                    let entry = data_model_pointer.with_index(index);
                    self.text(
                        &variable.name,
                        MAX_SESSION_ID_BYTES,
                        &entry.with_key("name"),
                    );
                    if variable.name.is_empty() || variable.name.starts_with('_') {
                        self.name(&entry.with_key("name"));
                    }
                    if !names.insert(variable.name.as_str()) {
                        self.fail(SnapshotErrorCode::Inconsistent, &entry.with_key("name"));
                    }
                    match &variable.value {
                        VariableValue::Value(value) => self.value(value, &entry.with_key("value")),
                        VariableValue::Text(text) => {
                            let value = entry.with_key("value");
                            self.text(&text.kind, MAX_TYPEOF_BYTES, &value.with_key("typeof"));
                            if text.kind.is_empty() {
                                self.fail(
                                    SnapshotErrorCode::Inconsistent,
                                    &value.with_key("typeof"),
                                );
                            }
                            self.text(&text.text, MAX_VALUE_BYTES, &value.with_key("text"));
                        }
                        VariableValue::Oversized(oversized) => {
                            let bytes = entry.with_key("value").with_key("bytes");
                            self.integer(oversized.bytes, &bytes);
                            // A value that fits is written inline.
                            if oversized.bytes <= MAX_VALUE_BYTES as u64 {
                                self.fail(SnapshotErrorCode::Inconsistent, &bytes);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        // Queues.
        let queues_pointer = pointer.with_key("queues");
        match (&session.queues, excluded(Section::Queues)) {
            (Some(_), true) | (None, false) => {
                self.fail(SnapshotErrorCode::Inconsistent, &queues_pointer);
            }
            (None, true) => {}
            (Some(queues), false) => {
                let internal = queues_pointer.with_key("internal");
                self.events(&queues.internal, &internal, context);
                if !queues.internal.is_empty() && !context.executable_content {
                    self.fail(SnapshotErrorCode::NotStable, &internal);
                }
                self.events(
                    &queues.external,
                    &queues_pointer.with_key("external"),
                    context,
                );
            }
        }
        // Pending sends.
        let pending_pointer = pointer.with_key("pending_sends");
        match (&session.pending_sends, excluded(Section::PendingSends)) {
            (Some(_), true) | (None, false) => {
                self.fail(SnapshotErrorCode::Inconsistent, &pending_pointer);
            }
            (None, true) => {}
            (Some(pending), false) => {
                self.count(pending.len(), MAX_PENDING_SENDS, &pending_pointer);
                let mut ids = BTreeSet::new();
                for (index, send) in pending.iter().enumerate() {
                    let entry = pending_pointer.with_index(index);
                    if SendId::new(send.sendid.as_str()).is_err() {
                        self.name(&entry.with_key("sendid"));
                    }
                    if !ids.insert(send.sendid.as_str()) {
                        self.fail(SnapshotErrorCode::Inconsistent, &entry.with_key("sendid"));
                    }
                    match (&send.event, &send.basichttp) {
                        (Some(event), None) => {
                            self.event(event, &entry.with_key("event"), context);
                        }
                        (None, Some(message)) => {
                            self.basichttp(message, &entry.with_key("basichttp"), context);
                        }
                        _ => self.fail(SnapshotErrorCode::Inconsistent, &entry),
                    }
                    for (text, key) in [(&send.target, "target"), (&send.send_type, "type")] {
                        if let Some(text) = text {
                            self.text(text, MAX_METADATA_BYTES, &entry.with_key(key));
                        }
                    }
                    self.integer(send.delay_ms, &entry.with_key("delay_ms"));
                    self.integer(send.deadline_ms, &entry.with_key("deadline_ms"));
                }
                if !pending
                    .windows(2)
                    .all(|pair| pair[0].deadline_ms <= pair[1].deadline_ms)
                {
                    self.fail(SnapshotErrorCode::Inconsistent, &pending_pointer);
                }
            }
        }
        // Transitions.
        let transitions = pointer.with_key("transitions");
        match (&session.transitions, excluded(Section::Transitions)) {
            (Some(_), true) | (None, false) => {
                self.fail(SnapshotErrorCode::Inconsistent, &transitions);
            }
            (None, true) => {}
            (Some(listed), false) => {
                self.count(listed.len(), MAX_TRANSITIONS, &transitions);
                let previews = !excluded(Section::Previews);
                let mut keys = BTreeSet::new();
                for (index, transition) in listed.iter().enumerate() {
                    let entry = transitions.with_index(index);
                    self.state(&transition.source, &entry.with_key("source"));
                    if !keys.insert((transition.source.as_str(), transition.index)) {
                        self.fail(SnapshotErrorCode::Inconsistent, &entry.with_key("index"));
                    }
                    let events = entry.with_key("events");
                    self.count(transition.events.len(), MAX_TRANSITIONS, &events);
                    if transition.events.is_empty() {
                        self.fail(SnapshotErrorCode::Inconsistent, &events);
                    }
                    for (event_index, descriptor) in transition.events.iter().enumerate() {
                        if EventName::new(descriptor.as_str()).is_err() {
                            self.name(&events.with_index(event_index));
                        }
                    }
                    self.states(&transition.targets, &entry.with_key("targets"));
                    if let Some(cond) = &transition.cond {
                        self.text(cond, MAX_COND_BYTES, &entry.with_key("cond"));
                    }
                    if transition.has_cond != transition.cond.is_some() {
                        self.fail(SnapshotErrorCode::Inconsistent, &entry.with_key("has_cond"));
                    }
                    match &transition.event_schema {
                        None => {}
                        Some(EventSchema::Schemaexpr { expr }) => self.text(
                            expr,
                            MAX_EVENT_SCHEMA_BYTES,
                            &entry.with_key("event_schema").with_key("expr"),
                        ),
                        Some(EventSchema::Schema { schema, sha256 }) => {
                            let pointer = entry.with_key("event_schema");
                            self.text(schema, MAX_EVENT_SCHEMA_BYTES, &pointer.with_key("schema"));
                            if !is_digest(sha256) || *sha256 != sha256_hex(schema.as_bytes()) {
                                self.fail(
                                    SnapshotErrorCode::Inconsistent,
                                    &pointer.with_key("sha256"),
                                );
                            }
                        }
                    }
                    if let Some(domain) = &transition.domain {
                        self.state(domain, &entry.with_key("domain"));
                    }
                    let targetless = transition.targets.is_empty();
                    if targetless && transition.domain.is_some() {
                        self.fail(SnapshotErrorCode::Inconsistent, &entry.with_key("domain"));
                    }
                    for (list, key) in [(&transition.exit, "exit"), (&transition.entry, "entry")] {
                        let list_pointer = entry.with_key(key);
                        match list {
                            Some(list) => {
                                self.states(list, &list_pointer);
                                if !previews || (targetless && !list.is_empty()) {
                                    self.fail(SnapshotErrorCode::Inconsistent, &list_pointer);
                                }
                            }
                            None if previews => {
                                self.fail(SnapshotErrorCode::Inconsistent, &list_pointer);
                            }
                            None => {}
                        }
                    }
                }
            }
        }
    }

    fn events(&mut self, events: &[Event], pointer: &Pointer, context: &Context<'_>) {
        self.count(events.len(), MAX_QUEUE_ENTRIES, pointer);
        for (index, event) in events.iter().enumerate() {
            self.event(event, &pointer.with_index(index), context);
        }
    }

    fn event(&mut self, event: &Event, pointer: &Pointer, context: &Context<'_>) {
        let name = pointer.with_key("name");
        self.text(&event.name, scxml_types::MAX_NAME_BYTES, &name);
        if EventName::new(event.name.as_str()).is_err() {
            self.name(&name);
        }
        for (text, key) in [
            (&event.origin, "origin"),
            (&event.origintype, "origintype"),
            (&event.sendid, "sendid"),
            (&event.invokeid, "invokeid"),
        ] {
            if let Some(text) = text {
                self.text(text, MAX_METADATA_BYTES, &pointer.with_key(key));
            }
        }
        if let Some(raw) = &event.raw {
            self.text(raw, MAX_VALUE_BYTES, &pointer.with_key("raw"));
        }
        self.value(&event.data, &pointer.with_key("data"));
        if let Some(carrier) = &event.trace_context {
            self.carrier(carrier, &pointer.with_key("trace_context"), context);
        }
    }

    /// One held trace carrier: present only under the preserve policy.
    fn carrier(&mut self, carrier: &TraceContext, pointer: &Pointer, context: &Context<'_>) {
        if context.strip {
            self.fail(SnapshotErrorCode::Inconsistent, pointer);
        }
        self.text(
            &carrier.traceparent,
            scxml_types::TRACEPARENT_BYTES,
            &pointer.with_key("traceparent"),
        );
        if let Some(state) = &carrier.tracestate {
            self.text(
                state,
                scxml_types::MAX_TRACESTATE_BYTES,
                &pointer.with_key("tracestate"),
            );
        }
        if let Some(baggage) = &carrier.baggage {
            self.text(
                baggage,
                scxml_types::MAX_BAGGAGE_BYTES,
                &pointer.with_key("baggage"),
            );
        }
    }

    /// One pending `BasicHTTP` message: the WIT `basichttp-send` bounds.
    fn basichttp(&mut self, message: &BasicHttpMessage, pointer: &Pointer, context: &Context<'_>) {
        if let Some(name) = &message.event {
            self.text(
                name,
                scxml_types::MAX_BASICHTTP_EVENT_NAME_BYTES,
                &pointer.with_key("event"),
            );
        }
        let params = pointer.with_key("params");
        self.count(message.params.len(), MAX_BASICHTTP_PARAMS, &params);
        for (index, param) in message.params.iter().enumerate() {
            let entry = params.with_index(index);
            let name = entry.with_key("name");
            self.text(&param.name, scxml_types::MAX_NAME_BYTES, &name);
            if param.name.is_empty() {
                self.fail(SnapshotErrorCode::SchemaViolation, &name);
            }
            self.value(&param.value, &entry.with_key("value"));
        }
        if let Some(content) = &message.content {
            self.value(content, &pointer.with_key("content"));
        }
        if let Some(carrier) = &message.trace_context {
            self.carrier(carrier, &pointer.with_key("trace_context"), context);
        }
    }

    fn value(&mut self, value: &Value, pointer: &Pointer) {
        let inner = pointer.with_key("value");
        match value {
            Value::Absent | Value::Undefined | Value::Null | Value::Boolean(_) => {}
            Value::Number(text) => {
                if !is_canonical_number(text) {
                    self.fail(SnapshotErrorCode::NonCanonical, &inner);
                }
            }
            Value::Text(text) | Value::Xml(text) => self.text(text, MAX_VALUE_BYTES, &inner),
            Value::JsonArray(text) => {
                self.text(text, MAX_VALUE_BYTES, &inner);
                if text.len() <= MAX_VALUE_BYTES
                    && scxml_types::DataValue::canonical_json_array(text)
                        != Ok(scxml_types::DataValue::JsonArray(text.clone()))
                {
                    self.fail(SnapshotErrorCode::NonCanonical, &inner);
                }
            }
            Value::JsonObject(text) => {
                self.text(text, MAX_VALUE_BYTES, &inner);
                if text.len() <= MAX_VALUE_BYTES
                    && scxml_types::DataValue::canonical_json_object(text)
                        != Ok(scxml_types::DataValue::JsonObject(text.clone()))
                {
                    self.fail(SnapshotErrorCode::NonCanonical, &inner);
                }
            }
        }
    }
}

struct Context<'a> {
    exclude: &'a [Section],
    strip: bool,
    roles: &'a BTreeSet<ComponentRole>,
    /// An executable-content document (`options.point`): its internal queue may
    /// hold events and its one session may be an invoked one.
    executable_content: bool,
}

/// What the recursive invocation check has seen so far: every session id
/// and the number of invoked sessions under the root session.
struct InvokedSeen {
    session_ids: BTreeSet<String>,
    invoked_sessions: usize,
}

#[cfg(test)]
mod tests {
    use super::is_canonical_number;

    #[test]
    fn numbers_are_rust_display_text_without_exponent() {
        for good in [
            "0",
            "-0",
            "1",
            "-1.5",
            "0.1",
            "1000000000000000000000",
            "123.456",
        ] {
            assert!(is_canonical_number(good), "{good}");
        }
        for bad in [
            "", "-", "01", "1.", ".5", "1e3", "1.50", "+1", "NaN", "inf", "0.10",
        ] {
            assert!(!is_canonical_number(bad), "{bad}");
        }
    }
}
