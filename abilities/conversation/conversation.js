// The ECMAScript of abilities/conversation/conversation.scx (SCXML 5.8): loaded by its root
// <script src="conversation.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// running

// The parent envelope {event, data} of the input in hand.
function inputEnvelope() {
  return inputEnvelopeOf(input);
}

function inputEnvelopeOf(data) {
  return {event: 'bot.ability.conversation.input', data: data};
}

// Identities (mosfet identity/value.py)

// Python's repr of a finite float: the shortest round-trip digits,
// positional with a fraction when the decimal exponent is from -4 to 15,
// else d[.ddd]e±XX.
function pythonFloat(value) {
  if (value === 0) {
    return 1 / value < 0 ? '-0.0' : '0.0';
  }
  var parts = value.toExponential().split('e');
  var exponent = Number(parts[1]);
  if (exponent < -4 || exponent >= 16) {
    var size = Math.abs(exponent);
    return parts[0] + 'e' + (exponent < 0 ? '-' : '+') + (size < 10 ? '0' : '') + size;
  }
  var text = String(value);
  return text.indexOf('.') < 0 ? text + '.0' : text;
}

// An embedding as Python's compact json.dumps writes a list of floats.
function pythonVector(id) {
  return '[' + id.map(pythonFloat).join(',') + ']';
}

// mosfet identity_sort_key: names first, by their text; embeddings by
// their compact Python JSON.
function identityKey(id) {
  return typeof id === 'string' ? '0' + id : '1' + pythonVector(id);
}

// Python's str order: by code point.
function codePointOrder(left, right) {
  var a = Array.from(left);
  var b = Array.from(right);
  for (var i = 0; i < a.length && i < b.length; i++) {
    var difference = a[i].codePointAt(0) - b[i].codePointAt(0);
    if (difference !== 0) {
      return difference;
    }
  }
  return a.length - b.length;
}

// mosfet's identity order (value.sorted_identities).
function sortedIdentities(ids) {
  return ids.slice().sort(function (left, right) {
    return codePointOrder(identityKey(left), identityKey(right));
  });
}

function sameIdentity(left, right) {
  return identityKey(left) === identityKey(right);
}

function unitVector(id) {
  var norm = Math.sqrt(id.reduce(function (sum, component) { return sum + component * component; }, 0));
  return norm > 0 ? id.map(function (component) { return component / norm; }) : null;
}

// mosfet value.cosine_similarity: null for vectors of other sizes.
function cosineSimilarity(left, right) {
  var a = unitVector(left);
  var b = unitVector(right);
  if (a === null || b === null || a.length !== b.length) {
    return null;
  }
  return a.reduce(function (sum, component, i) { return sum + component * b[i]; }, 0);
}

// A participant's track (mosfet _new_track_ref): a name is its own track,
// an embedding "track-" and its unit vector as JSON.
function trackRef(id) {
  if (typeof id === 'string') {
    return id;
  }
  var norm = Math.sqrt(id.reduce(function (sum, component) { return sum + component * component; }, 0));
  return 'track-' + JSON.stringify(id.map(function (component) { return component / norm; }));
}

// Relationships (mosfet Conversation._relationship_for_input)

// The relationship in hand.
function relationship() {
  return relationships[relationship_index];
}

function relationshipRef() {
  return relationship().session_ref;
}

// The index of the relationship the input belongs to, else -1: targetless
// input shares the first ambient relationship; other input takes the best
// scoring one, in either direction.
function relationshipIndexFor(data) {
  if (data.target_ids.length === 0) {
    return relationships.findIndex(function (each) { return each.target_ids.length === 0; });
  }
  var best = -1;
  var bestScore = null;
  relationships.forEach(function (each, i) {
    var score = relationshipScore(each, data.source_ids, data.target_ids);
    if (score !== null && (best < 0 || score > bestScore || (score === bestScore && each.session_ref < relationships[best].session_ref))) {
      best = i;
      bestScore = score;
    }
  });
  return best;
}

function relationshipScore(candidate, sources, targets) {
  if (candidate.target_ids.length === 0) {
    var addressed = targets.some(function (target) {
      return candidate.participants.some(function (participant) { return isSameParticipant(target, participant); });
    });
    if (addressed) {
      return similarity_threshold;
    }
  }
  var direct = lowerScore(collectionScore(sources, candidate.source_ids), collectionScore(targets, candidate.target_ids));
  var reverse = lowerScore(collectionScore(sources, candidate.target_ids), collectionScore(targets, candidate.source_ids));
  return direct === null ? reverse : reverse === null ? direct : Math.max(direct, reverse);
}

function isSameParticipant(target, participant) {
  if (typeof target === 'string' || typeof participant === 'string') {
    return target === participant;
  }
  var score = cosineSimilarity(target, participant);
  return score !== null && score >= similarity_threshold;
}

function lowerScore(left, right) {
  return left === null || right === null ? null : Math.min(left, right);
}

// mosfet _identity_collection_score: the best one-to-one match of two
// identity groups (names equal, embeddings at least similarity_threshold
// alike), refused when another match comes within similarity_margin.
function collectionScore(observed, expected) {
  if (observed.length !== expected.length || observed.length === 0) {
    return null;
  }
  var scores = observed.map(function (left) {
    return expected.map(function (right) { return pairScore(left, right); });
  });
  var best = bestAssignment(scores, -1, -1);
  if (best === null) {
    return null;
  }
  var second = null;
  best.columns.forEach(function (column, row) {
    var alternative = bestAssignment(scores, row, column);
    if (alternative !== null && (second === null || alternative.total > second)) {
      second = alternative.total;
    }
  });
  if (second !== null && best.total - second <= similarity_margin) {
    return null;
  }
  return best.total / observed.length;
}

function pairScore(left, right) {
  if (typeof left === 'string' || typeof right === 'string') {
    return left === right ? 1 : null;
  }
  var score = cosineSimilarity(left, right);
  return score !== null && score >= similarity_threshold ? score : null;
}

// The highest total one-to-one assignment ({total, columns}) without the
// forbidden row and column pair, else null (groups hold at most a few).
function bestAssignment(scores, forbiddenRow, forbiddenColumn) {
  var best = null;
  var visit = function (row, used, total, columns) {
    if (row === scores.length) {
      if (best === null || total > best.total) {
        best = {total: total, columns: columns};
      }
      return;
    }
    scores[row].forEach(function (score, column) {
      if (score !== null && used.indexOf(column) < 0 && !(row === forbiddenRow && column === forbiddenColumn)) {
        visit(row + 1, used.concat([column]), total + score, columns.concat([column]));
      }
    });
  };
  visit(0, [], 0, []);
  return best;
}

function isNewRelationship() {
  return relationship_index < 0;
}

// The relationships with the input's own (its context_ref the input's).
function relationshipsWithInput() {
  return relationships.concat([{session_ref: 'relationship-' + context_ref, context_ref: context_ref, source_ids: sortedIdentities(input.source_ids), target_ids: sortedIdentities(input.target_ids), participants: [], sequences: []}]);
}

function lastRelationshipIndex() {
  return relationships.length - 1;
}

// The relationships with the one in hand changed by change(copy).
function relationshipsChanged(change) {
  return relationships.map(function (each, i) {
    if (i !== relationship_index) {
      return each;
    }
    var copy = JSON.parse(JSON.stringify(each));
    change(copy);
    return copy;
  });
}

// The source in hand becomes a participant once its detector is ready
// (mosfet keeps a participant while it has a detector).
function isNewParticipant() {
  return !relationship().participants.some(function (participant) { return sameIdentity(participant, turnSource()); });
}

function addedWithSource() {
  return added.concat([turnSource()]);
}

function relationshipsWithParticipant() {
  return relationshipsChanged(function (copy) { copy.participants.push(turnSource()); });
}

// Without the source in hand, whose detector failed (mosfet stops it).
function relationshipsWithoutSource() {
  var source = turnSource();
  return relationshipsChanged(function (copy) {
    copy.participants = copy.participants.filter(function (participant) { return !sameIdentity(participant, source); });
  });
}

// Without this input's new participants (mosfet stops the detectors it
// started for a turn whose memory failed).
function relationshipsWithoutAdded() {
  return relationshipsChanged(function (copy) {
    copy.participants = copy.participants.filter(function (participant) {
      return !added.some(function (each) { return sameIdentity(each, participant); });
    });
  });
}

// The relationship's committed messages with this one.
function relationshipsWithMessage() {
  return relationshipsChanged(function (copy) { copy.sequences.push(message.sequence); });
}

// mosfet Conversation.detector_refs: (relationship, source) of every
// participant, ordered by relationship and then identity.
function detectorRefs() {
  var refs = [];
  relationships.forEach(function (each) {
    each.participants.forEach(function (participant) { refs.push([each.session_ref, participant]); });
  });
  return refs.sort(function (left, right) {
    return left[0] < right[0] ? -1 : left[0] > right[0] ? 1 : codePointOrder(identityKey(left[1]), identityKey(right[1]));
  });
}

// turn_exchange (mosfet Conversation's turn_detector stage)

function turnDelay() {
  return turn_timeout_ms + 'ms';
}

function isTimer(_event) {
  return _event.sendid === timer;
}

function timedOutMessage(what) {
  return what + ' timed out after ' + turn_timeout_ms / 1000 + ' seconds.';
}

function turnErrorMessage(_event) {
  return 'The turn detector could not take the turn (' + _event.name + ')';
}

// The source in hand, its track, and its turn.
function turnSource() {
  return turn_sources[turn_index];
}

function turnRef() {
  return 'input-' + inputs + ':turn:' + trackRef(turnSource());
}

// The input as the source's stimulus (mosfet _content_for_detector).
function turnContent() {
  var type = input.content_type.toLowerCase();
  if (type === 'text/plain' && typeof input.content === 'string') {
    return {kind: 'text', source_participant_ref: turnSource(), content: input.content};
  }
  if (type.indexOf('audio/') === 0 && typeof input.content === 'string') {
    return {kind: 'audio', source_participant_ref: turnSource(), content: input.content, sample_rate_hz: orNull(input.sample_rate_hz), channels: orNull(input.channels)};
  }
  return {kind: 'content', source_participant_ref: turnSource(), content: input.content, content_type: input.content_type};
}

function orNull(value) {
  return value === undefined ? null : value;
}

function turnStart() {
  return {conversation_ref: relationshipRef(), turn_ref: turnRef(), self_participant_ref: trackRef(turnSource()), source_participant_ref: turnSource(), content: turnContent()};
}

function turnEnd() {
  return {conversation_ref: relationshipRef(), turn_ref: turnRef(), source_participant_ref: turnSource()};
}

// The detector's completed turn for the source in hand.
function isExchangedTurn(_event) {
  var turn = _event.data;
  return turn.turn_ref === turnRef() && turn.conversation_ref === relationshipRef() && turn.participant_ref === trackRef(turnSource()) && JSON.stringify(turn.source_participant_ref) === JSON.stringify(turnSource());
}

function turnsWith(_event) {
  return turns.concat([_event.data]);
}

function hasAllTurns() {
  return turn_index >= turn_sources.length;
}

// recalling_from and remembering_from

function isMemoryAbsent() {
  return database === null;
}

function memoryTimeoutDelay() {
  return memory_timeout_ms + 'ms';
}

// recalling_memory: the relationship's memories (mosfet
// conversation_memory_recall_input), kept under its context_ref.
function memoryRecallStatements() {
  return [{sql: memory_select, parameters: ['conversation_memory', relationship().context_ref]}];
}

function recalledCount(_event) {
  return _event.data.results[0].rowcount;
}

// remembering_memory: the input (mosfet conversation_memory_insert_input).
function memoryInsertStatements() {
  return [{sql: memory_insert, parameters: ['memory', relationship().context_ref, 'conversation', 'standard', 'retain', memoryPayload(), 'application/json', 'conversation_memory']}];
}

// mosfet Memory.to_json: {content, content_type, source_ids, target_ids}
// with sorted keys and compact separators, the identities in mosfet's
// order (embeddings as floats) and the content encoded collision-safe.
function memoryPayload() {
  return '{"content":' + pythonJson(memoryContent(input.content)) + ',"content_type":' + JSON.stringify(input.content_type) + ',"source_ids":' + identitiesJson(input.source_ids) + ',"target_ids":' + identitiesJson(input.target_ids) + '}';
}

// mosfet value.identities_json of an identity set.
function identitiesJson(ids) {
  var unique = sortedIdentities(ids).filter(function (id, i, all) { return i === 0 || !sameIdentity(all[i - 1], id); });
  return '[' + unique.map(function (id) { return typeof id === 'string' ? JSON.stringify(id) : pythonVector(id); }).join(',') + ']';
}

// mosfet _encode_content: arrays and objects tagged, an object's entries
// ordered by their keys' JSON.
function memoryContent(value) {
  if (Array.isArray(value)) {
    return {__stateforward_memory_content__: 'list', value: value.map(memoryContent)};
  }
  if (value !== null && typeof value === 'object') {
    var entries = Object.keys(value).map(function (key) { return [key, memoryContent(value[key])]; });
    entries.sort(function (left, right) { return codePointOrder(asciiJson(left[0]), asciiJson(right[0])); });
    return {__stateforward_memory_content__: 'mapping', value: entries};
  }
  return value;
}

// Python's json.dumps(text) (ensure_ascii): every character outside
// printable ASCII escaped.
function asciiJson(text) {
  return JSON.stringify(text).replace(/[\u007f-\uffff]/g, function (character) {
    return '\\u' + ('000' + character.charCodeAt(0).toString(16)).slice(-4);
  });
}

// json.dumps(sort_keys, compact separators, ensure_ascii off): integers as
// integers, other numbers as Python floats.
function pythonJson(value) {
  if (Array.isArray(value)) {
    return '[' + value.map(pythonJson).join(',') + ']';
  }
  if (value !== null && typeof value === 'object') {
    return '{' + Object.keys(value).sort(codePointOrder).map(function (key) { return JSON.stringify(key) + ':' + pythonJson(value[key]); }).join(',') + '}';
  }
  if (typeof value === 'number' && !Number.isSafeInteger(value)) {
    return pythonFloat(value);
  }
  return JSON.stringify(value);
}

function memoriesLog(_event) {
  return 'recalled ' + recalled + ', remembered ' + _event.data.results[0].rowcount;
}

function memoryFailedMessage(_event) {
  return _event.data.message;
}

// committing

// The turns' product in place of the input (mosfet _complete_turn): their
// texts joined as text/plain when any has text, each text once (D79), else
// no content for audio, else the input's content.
function turnProduct() {
  var texts = turns.map(function (turn) { return turn.text; }).filter(function (text, i, all) { return text !== '' && all.indexOf(text) === i; });
  if (texts.length > 0) {
    return {source_ids: input.source_ids, target_ids: input.target_ids, content: texts.join(' '), content_type: 'text/plain'};
  }
  var audio = input.content_type.toLowerCase().indexOf('audio/') === 0;
  return {source_ids: input.source_ids, target_ids: input.target_ids, content: audio ? null : input.content, content_type: input.content_type};
}

// The committed provenance: the first turn's, in the relationship.
function turnProvenance() {
  return {turn_ref: turns[0].turn_ref, session_ref: relationshipRef()};
}

function historyWithMessage() {
  return history.concat([message]);
}

// The committed message is text: string content of a text/* type.
function isTextMessage() {
  return typeof message.content === 'string' && /^text\//i.test(message.content_type);
}

function inboundCommitLog() {
  return 'inbound ' + message.sequence + ' from ' + JSON.stringify(sortedIdentities(message.source_ids)) + ' (' + relationship().sequences.length + ' in its relationship)';
}

// conversation (append)

function isOutboundAppend(_event) {
  return _event.data.message.direction === 'outbound';
}

// An append already committed: mosfet matches its event's id, source and
// target against each committed provenance (an append without an id is
// never one).
function isCommittedAppend(_event) {
  var given = _event.data.message.provenance;
  var id = stringOrNull(given.id);
  return id !== null && id !== '' && history.some(function (committed) {
    return committed.provenance.id === id && committed.provenance.source === stringOrNull(given.source) && committed.provenance.target === stringOrNull(given.target);
  });
}

function stringOrNull(value) {
  return typeof value === 'string' ? value : null;
}

function outboundCommitLog() {
  return 'outbound ' + message.sequence;
}

function alreadyCommittedLog(_event) {
  return 'outbound ' + _event.data.message.provenance.id + ' already committed';
}

function rejectedLog(_event) {
  return _event.name + ' does not satisfy its schema';
}
