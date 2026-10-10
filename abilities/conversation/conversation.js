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

// waiting and running: the defer queue.

function hasQueuedInput() {
  return queue.length > 0;
}

function remainingQueue() {
  return queue.slice(1);
}

// The queue with an input and its parent envelope.
function queuedWith(data, envelope) {
  return queue.concat([{input: data, parent: envelope}]);
}

// turn_exchange (mosfet Conversation's turn_detector stage)

// mosfet's identity order (value.sorted_identities): names sorted, then
// embeddings by their JSON text.
function sortedIdentities(ids) {
  return ids.slice().sort(function (left, right) {
    var a = identitySortKey(left);
    var b = identitySortKey(right);
    return a < b ? -1 : a > b ? 1 : 0;
  });
}

function identitySortKey(id) {
  return typeof id === 'string' ? '0' + id : '1' + JSON.stringify(id);
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

function turnDelay() {
  return turn_timeout_ms + 'ms';
}

function isTimer(_event) {
  return _event.sendid === timer;
}

function isFromTurnDetector(_event) {
  return _event.invokeid === 'turn_detector';
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
  return {conversation_ref: message.provenance.session_ref, turn_ref: turnRef(), self_participant_ref: trackRef(turnSource()), source_participant_ref: turnSource(), content: turnContent()};
}

function turnEnd() {
  return {conversation_ref: message.provenance.session_ref, turn_ref: turnRef(), source_participant_ref: turnSource()};
}

// The detector's completed turn for the source in hand.
function isExchangedTurn(_event) {
  var turn = _event.data;
  return isFromTurnDetector(_event) && turn.turn_ref === turnRef() && turn.conversation_ref === message.provenance.session_ref && turn.participant_ref === trackRef(turnSource()) && JSON.stringify(turn.source_participant_ref) === JSON.stringify(turnSource());
}

function turnsWith(_event) {
  return turns.concat([_event.data]);
}

function hasAllTurns() {
  return turn_index >= turn_sources.length;
}

// recalling_from

function isMemoryAbsent() {
  return database === null;
}

function memoryTimeoutDelay() {
  return memory_timeout_ms + 'ms';
}

// recalling_memory

// The input as remembered: {source_ids, target_ids, content, content_type} as JSON.
function rememberedInput() {
  return JSON.stringify({source_ids: input.source_ids, target_ids: input.target_ids, content: input.content, content_type: input.content_type});
}

// The recall of the relationship's memories (mosfet conversation_memory_recall_input).
function memoryRecallStatements() {
  return [{sql: memory_select, parameters: ['conversation_memory', message.provenance.session_ref]}];
}

// The statements with the input's insert (mosfet conversation_memory_insert_input).
function statementsWithMemoryInsert() {
  return statements.concat([{sql: memory_insert, parameters: ['memory', message.provenance.session_ref, 'conversation']}]);
}

// The insert's parameters with the remembered input.
function memoryInsertParameters() {
  return statements[1].parameters.concat(['standard', 'retain', remembered, 'application/json', 'conversation_memory']);
}

// From the memory ability invoked as "remember".
function isFromMemory(_event) {
  return _event.invokeid === 'remember';
}

function memoriesLog(_event) {
  return 'recalled ' + _event.data.results[0].rowcount + ', remembered ' + _event.data.results[1].rowcount;
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

// The committed provenance's turn: the first turn's.
function turnProvenance() {
  return {turn_ref: turns[0].turn_ref};
}

function historyWithMessage() {
  return history.concat([message]);
}

// The participants' relationship with the message's sequence appended.
function relationshipWithMessage() {
  return (relationships[participants] || []).concat([message.sequence]);
}

// The committed message is text: string content of a text/* type.
function isTextMessage() {
  return typeof message.content === 'string' && /^text\//i.test(message.content_type);
}

function inboundCommitLog() {
  return 'inbound ' + (history.length - 1) + ' from ' + participants + ' (' + relationships[participants].length + ' in its relationship)';
}

// conversation (append)

function isOutboundAppend(_event) {
  return _event.data.message.direction === 'outbound';
}

// An outbound append whose provenance id is not yet committed.
function isNewOutboundAppend(_event) {
  return isOutboundAppend(_event) && appended.indexOf(_event.data.message.provenance.id) < 0;
}

function hasProvenanceId() {
  return typeof message.provenance.id === 'string';
}

function appendedWithProvenance() {
  return appended.concat([message.provenance.id]);
}

function outboundCommitLog() {
  return 'outbound ' + (history.length - 1);
}

function alreadyCommittedLog(_event) {
  return 'outbound ' + _event.data.message.provenance.id + ' already committed';
}

function rejectedLog(_event) {
  return _event.name + ' does not satisfy its schema';
}
