// The ECMAScript of abilities/stm/stm.scx (SCXML 5.8): loaded by its root
// <script src="stm.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// The clock (the original's _utc_text)

// Microsecond UTC ISO text, as Python's isoformat writes it (no fraction
// when it is zero), so lexicographic order is chronological order.
function utcText(ms) {
  const iso = new Date(ms).toISOString();
  const millis = iso.slice(20, 23);
  return iso.slice(0, 19) + (millis === '000' ? '' : '.' + millis + '000') + '+00:00';
}

// A stored time as pydantic serializes a UTC datetime (Turn.at).
function serializedTime(text) {
  return text.replace(/\+00:00$/, 'Z');
}

function delay(ms) {
  return ms + 'ms';
}

// opening

function hasNoDatabase() {
  return database === null;
}

function openStatements() {
  return [{sql: open_select}];
}

function openFailedMessage(_event) {
  return 'short-term memory could not open its database: ' + failureText(_event);
}

// record (the original's _record)

// The projection as the original stores it: sorted keys, compact.
function payloadJson(payload) {
  return JSON.stringify(payload, Object.keys(payload).sort());
}

function optionalText(value) {
  return value === undefined ? null : value;
}

function recordStatements() {
  const data = request;
  const parameters = [data.stimulus_name, payloadJson(data.payload || {}), optionalText(data.event_id),
    optionalText(data.source), optionalText(data.target), utcText(Date.now())];
  return [{sql: record_insert, parameters: parameters}, {sql: record_evict, parameters: [capacity]}];
}

// reinforce (the original's _reinforce): at most 128 ids a statement, so
// each stays within the services' 256 parameters.

function reinforceStatements() {
  const at = utcText(Date.now());
  const statements = [];
  for (let start = 0; start < request.stm_ids.length; start += 128) {
    const ids = request.stm_ids.slice(start, start + 128);
    const marks = ids.map((_, index) => '?' + (index + 2)).join(', ');
    statements.push({sql: reinforce_update + ' (' + marks + ')', parameters: [at].concat(ids)});
  }
  return statements;
}

// recent (the original's _recent)

function recentStatements() {
  const cutoff = utcText(Date.now() - recency_window_ms);
  const limit = request.limit === undefined ? recent_limit : request.limit;
  const named = typeof request.stimulus_name === 'string';
  const sql = named ? recent_named_select : recent_select;
  const parameters = named ? [cutoff, request.stimulus_name, limit] : [cutoff, limit];
  return [{sql: sql, parameters: parameters}];
}

// The original's _projection_payload_from_json: only bounded scalar fields.
function storedPayload(text) {
  let payload = {};
  try {
    payload = JSON.parse(text);
  } catch (error) {
    return {};
  }
  const fields = {};
  for (const key of Object.keys(payload)) {
    const value = payload[key];
    if (key !== '' && ['string', 'number', 'boolean'].indexOf(typeof value) >= 0) {
      fields[key] = value;
    }
  }
  return fields;
}

// The original's _observed_from_row, newest effective clock first.
function recentEntries(_event) {
  return _event.data.results[0].rows.map((row) => {
    const v = row.values;
    return {stm_id: v[0], stimulus_name: v[1], payload: storedPayload(v[2]), event_id: v[3], source: v[4],
      target: v[5], created_at: v[6], last_accessed_at: v[7]};
  });
}

// turn (the original's _record_turn)

function turnCut() {
  return bounded.cut || request.cut === true;
}

function isEmptyTurn() {
  return bounded.text.length === 0;
}

function emptyTurnMessage() {
  return 'short-term memory bot.ability.memory.stm.turn failed: the words cut to nothing at the turn bound';
}

function turnStatements() {
  const kept = turn_capacity - Math.floor(turn_capacity / 2);
  const parameters = [request.id, utcText(Date.now()), request.direction, JSON.stringify(request.speakers || []),
    bounded.text, turnCut() ? 1 : 0, request.conversation];
  return [{sql: turn_insert, parameters: parameters}, {sql: turn_shed, parameters: [turn_capacity, kept]}];
}

function turnRecordedLog() {
  return request.direction + ' cut=' + turnCut();
}

// recent_turns (the original's _recent_turns)

// within (ms) narrows the register's window, never widens it.
function turnWindowMs() {
  const within = request.within;
  return typeof within === 'number' && within < turn_window_ms ? within : turn_window_ms;
}

// The floor advances in whole half-window steps since the Unix epoch, so
// every turn of one step leaves together.
function recentTurnsStatements() {
  const window = turnWindowMs();
  const step = window / 2;
  const floor = Math.floor((Date.now() - window) / step) * step;
  const limit = request.limit === undefined ? recent_turns_limit : request.limit;
  return [{sql: recent_turns_select, parameters: [utcText(floor), limit]}];
}

function storedSpeakers(text) {
  try {
    return JSON.parse(text);
  } catch (error) {
    return [];
  }
}

// A non-empty string.
function isText(value) {
  return typeof value === 'string' && value.length > 0;
}

// One stored speaker as the original's Participant would take it: a ref of
// 1 to 200 characters and a known role (own and resembles as stored).
function isSpeaker(speaker) {
  return speaker !== null && typeof speaker === 'object' && isText(speaker.ref) &&
    Array.from(speaker.ref).length <= 200 && (speaker.role === 'source' || speaker.role === 'target');
}

// One stored row as the original's Turn, or null when it does not decode as
// one (a damaged row is skipped, so it never costs the read the turns
// around it).
function storedTurn(v) {
  const speakers = storedSpeakers(v[3]);
  if (!isText(v[0]) || !isText(v[1]) || (v[2] !== 'heard' && v[2] !== 'said') || !isText(v[4]) || !isText(v[6]) ||
      !Array.isArray(speakers) || speakers.length > 8 || !speakers.every(isSpeaker)) {
    return null;
  }
  return {id: v[0], at: serializedTime(v[1]), direction: v[2], speakers: speakers, text: v[4],
    cut: v[5] === 1 || v[5] === true, conversation: v[6]};
}

// The newest turns that decode, read back oldest first.
function recentTurns(_event) {
  return _event.data.results[0].rows.map((row) => storedTurn(row.values)).filter((turn) => turn !== null).reverse();
}

// Failures

function failureText(_event) {
  return _event.data && typeof _event.data.message === 'string' ? _event.data.message : _event.name;
}

// The original's failure message for a request it could not serve.
function operationFailedMessage(name, _event) {
  return 'short-term memory ' + name + ' failed: ' + failureText(_event);
}
