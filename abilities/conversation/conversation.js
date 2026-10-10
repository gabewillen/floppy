// The ECMAScript of abilities/conversation/conversation.scx (SCXML 5.8): loaded by its root
// <script src="conversation.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// recalling

// The parent envelope {event, data} of the input in hand.
function inputEnvelope() {
  return {event: 'bot.ability.conversation.input', data: input};
}

function historyWithMessage() {
  return history.concat([message]);
}

// The participants' transcript with the message's sequence appended.
function transcriptWithMessage() {
  return (transcripts[participants] || []).concat([message.sequence]);
}

function inboundCommitLog() {
  return 'inbound ' + (history.length - 1) + ' from ' + participants + ' (' + transcripts[participants].length + ' in its transcript)';
}

// The last max_messages messages of the history.
function outputMessages() {
  return history.slice(Math.max(0, history.length - max_messages));
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
