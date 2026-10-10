// The ECMAScript of abilities/conversation/conversation.scx (SCXML 5.8): loaded by its root
// <script src="conversation.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// running

// The parent envelope {event, data} of the input in hand.
function inputEnvelope() {
  return {event: 'bot.ability.conversation.input', data: input};
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
