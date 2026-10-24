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

// Relationships (<conversation:relationship_for_input> keeps them)

// The relationship in hand.
function relationship() {
  return relationships[relationship_index];
}

function relationshipRef() {
  return relationship().session_ref;
}

// turn_exchange

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
  return 'input-' + inputs + ':turn:' + track_ref;
}

function turnStart() {
  return {conversation_ref: relationshipRef(), turn_ref: turnRef(), self_participant_ref: track_ref, source_participant_ref: turnSource(), content: turn_content};
}

function turnEnd() {
  return {conversation_ref: relationshipRef(), turn_ref: turnRef(), source_participant_ref: turnSource()};
}

// The detector's completed turn for the source in hand.
function isExchangedTurn(_event) {
  var turn = _event.data;
  return turn.turn_ref === turnRef() && turn.conversation_ref === relationshipRef() && turn.participant_ref === track_ref && JSON.stringify(turn.source_participant_ref) === JSON.stringify(turnSource());
}

function turnsWith(_event) {
  return turns.concat([_event.data]);
}

function hasAllTurns() {
  return turn_index >= turn_sources.length;
}

// identifying_from, identifying_memory, numbering_from, numbering_memory and
// tagging_memory (decisions D194 and D205)

// Nothing to identify: no memory, or no embedding among the input's
// identities.
function isUnidentified() {
  return database === null || !input.source_ids.concat(input.target_ids).some(Array.isArray);
}

// Every person's representative voice (decision D205).
function identifyStatements() {
  return [{sql: voice_select, parameters: ['voice']}];
}

// The representatives memory holds, [person_ref, voice] each.
function heardVoices(_event) {
  return _event.data.results[0].rows.map(function (row) { return row.values; });
}

function hasNewPeople() {
  return people !== null && people.people.length > 0;
}

// The new people's numbers, taken from the people sequence.
function numberStatements() {
  return [{sql: sequence_next, parameters: ['person', people.people.length]}];
}

// The last number taken before this input's.
function numbersBefore(_event) {
  return Number(_event.data.results[0].rows[0].values[0]) - people.people.length;
}

// The new people, then every voice heard, each for its person, then the
// new people's representatives.
function tagStatements() {
  return people.people.map(personStatement).concat(people.voices.map(voiceStatement), people.representatives.map(representativeStatement));
}

function personStatement(person) {
  return {sql: person_insert, parameters: [person]};
}

function voiceStatement(voice) {
  return {sql: voice_insert, parameters: [voice[0], 'voice', voice[1]]};
}

function representativeStatement(representative) {
  return {sql: representative_insert, parameters: [representative[0], 'voice', representative[1]]};
}

function identifyErrorMessage(_event) {
  return 'Conversation could not identify the input\'s voices (' + _event.name + ')';
}

// recalling_from and remembering_from

function isMemoryAbsent() {
  return database === null;
}

function memoryTimeoutDelay() {
  return memory_timeout_ms + 'ms';
}

// recalling_memory: the relationship's memories, kept under its
// context_ref.
function memoryRecallStatements() {
  return [{sql: memory_select, parameters: ['conversation_memory', relationship().context_ref]}];
}

function recalledCount(_event) {
  return _event.data.results[0].rowcount;
}

// remembering_memory: the input, a conversation memory of the relationship.
function memoryInsertStatements() {
  return [{sql: memory_insert, parameters: ['memory', relationship().context_ref, 'conversation', 'standard', 'retain', memory_payload, 'application/json', 'conversation_memory']}];
}

function memoriesLog(_event) {
  return 'recalled ' + recalled + ', remembered ' + _event.data.results[0].rowcount;
}

function memoryFailedMessage(_event) {
  return _event.data.message;
}

// committing

// The exchange with the outbound message in hand.
function exchangeWithMessage() {
  return exchange.concat([message]);
}

// The committed message is text: string content of a text/* type.
function isTextMessage() {
  return typeof message.content === 'string' && /^text\//i.test(message.content_type);
}

// A voice embedding is biometric and never logged: it is written as
// <biometric:VoiceEmbedding>.
function inboundCommitLog() {
  var sources = turn_sources.map(function (id) { return Array.isArray(id) ? '<biometric:VoiceEmbedding>' : id; });
  return 'inbound from ' + JSON.stringify(sources) + ' in relationship ' + relationship_index;
}

// conversation (append)

function isOutboundAppend(_event) {
  return _event.data.message.direction === 'outbound';
}

// An append already committed to the exchange: its event's id, source and
// target match a provenance in the exchange (an append without an id is
// never one; decision D176).
function isCommittedAppend(_event) {
  var given = _event.data.message.provenance;
  var id = stringOrNull(given.id);
  return id !== null && id !== '' && exchange.some(function (committed) {
    return committed.provenance.id === id && committed.provenance.source === stringOrNull(given.source) && committed.provenance.target === stringOrNull(given.target);
  });
}

function stringOrNull(value) {
  return typeof value === 'string' ? value : null;
}

// The outbound message's place in the exchange.
function outboundCommitLog() {
  return 'outbound ' + (exchange.length - 1);
}

function alreadyCommittedLog(_event) {
  return 'outbound ' + _event.data.message.provenance.id + ' already committed';
}

function rejectedLog(_event) {
  return _event.name + ' does not satisfy its schema';
}
