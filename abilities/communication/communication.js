// The ECMAScript of abilities/communication/communication.scx (SCXML 5.8): loaded by its root
// <script src="communication.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// communication

// The product's latest message's provenance turn reference.
function productTurn(_event) {
  var messages = _event.data.stimulus.data.messages;
  return String(messages[messages.length - 1].provenance.turn_ref);
}

function exchangeLength(_event) {
  return String(_event.data.messages.length);
}

// A product: the exchange's latest message is inbound.
function isInboundProduct(_event) {
  var messages = _event.data.messages;
  return messages.length > 0 && messages[messages.length - 1].direction === 'inbound';
}

// A failed conversation input, as stage: message.
function conversationFailedLog(_event) {
  return _event.data.stage + ': ' + _event.data.message;
}

function childRunFailure(_event) {
  return 'the ' + _event.invokeid + ' child could not run (' + _event.name + ')';
}

function childEndFailure(_event) {
  return 'the ' + _event.invokeid + ' child ended';
}

// inactive

function childrenReady() {
  return ready.conversation === true && ready.speaking === true;
}

function childStartFailure(_event) {
  return 'a child could not start (' + _event.name + ')';
}

// The input as the routed input's parent envelope {event, data}.
function routedParent(_event) {
  return {event: _event.name, data: _event.data};
}

// active

// A reply that says something (decision D178): every part has words (not blank), a sound or a pause, every
// expression directs a cue (sound_at only with a sound), and the words,
// joined exactly as written, are not blank.
function isSpokenReply(parts) {
  return parts.every(isSpokenPart) && /\S/.test(said(parts));
}

function isSpokenPart(part) {
  var expression = isSet(part.expression) ? part.expression : null;
  if (isSet(part.words) && !/\S/.test(part.words)) {
    return false;
  }
  if (expression === null) {
    return typeof part.words === 'string';
  }
  var cues = ['instructions', 'emotion', 'style', 'intensity', 'pace', 'pitch', 'volume', 'pause', 'sound'];
  return (cues.some(function (cue) { return isSet(expression[cue]); }) || expression.emphasis === true)
    && (expression.sound_at === undefined || isSet(expression.sound))
    && (typeof part.words === 'string' || isSet(expression.pause) || isSet(expression.sound));
}

function isSet(value) {
  return value !== undefined && value !== null;
}

// The text of record of a reply: its parts' words joined verbatim
// (decision D178).
function said(parts) {
  return parts.map(function (part) { return typeof part.words === 'string' ? part.words : ''; }).join('');
}

// A respond for a response that already finished (decision D175): only a
// respond whose sender named its id can be one.
function isClosedResponse(_event) {
  return !!_event.sendid && closed_responses.indexOf(_event.sendid) >= 0;
}

// The response's id: its respond's sendid, or null when none was named.
function responseId(_event) {
  return _event.sendid ? _event.sendid : null;
}

// admitting_response (decision D242: active's one respond transition has
// no cond, so its parent reads it as an offer)

// The respond being admitted is for a response that already finished, or
// is the whole of a drafted response that ended without it (decision
// D262).
function isClosedAdmission() {
  return (response_id !== null && closed_responses.indexOf(response_id) >= 0)
    || (abandoned !== null && response_origin === abandoned);
}

// The respond being admitted says something.
function isSpokenAdmission() {
  return isSpokenReply(response);
}

// Drafts (decision D262)

// A draft of a respond's part that opens a drafted response: not from the
// sender of one that ended without its respond.
function isOpeningDraft(_event) {
  return _event.data.event === 'bot.ability.communication.respond' && isSpeakableDraft(_event)
    && !(abandoned !== null && _event.origin === abandoned);
}

// A draft of the drafted response in hand, before its respond.
function isDraftOfResponse(_event) {
  return isDraftedResponse(_event) && isSpeakableDraft(_event);
}

// A draft speaking's contract takes: one of a reply's 64 parts, its new
// words at most a part's 2000 characters.
function isSpeakableDraft(_event) {
  return typeof _event.data.part === 'number' && _event.data.part < 64 && _event.data.text.length <= 2000;
}

// From the sender of the drafted response in hand, before its respond.
function isDraftedResponse(_event) {
  return drafting !== null && response === null && _event.origin === drafting;
}

// The drafted response in hand ended without its respond.
function isUnsettledDraft() {
  return drafting !== null && response === null;
}

// A respond's draft as speaking's: the part, its new words and where they
// start, and the part's expression on its first draft.
function speakingDraft(_event) {
  var draft = ({part: _event.data.part, words: _event.data.text, offset: _event.data.offset});
  var expression = Object(_event.data.leading).expression;
  if (expression !== null && typeof expression === 'object' && !Array.isArray(expression)) {
    draft.expression = expression;
  }
  return draft;
}

// preparing_response

function nextOperation() {
  return operation + 1;
}

// responding

function responseTimeoutDelay() {
  return response_timeout_ms + 'ms';
}

// The hop timer of the response in hand.
function isResponseHopTimeout(_event) {
  return _event.sendid === hop_timer && _event.data.operation === operation;
}

// A respond for a finished response, or another copy of the one in hand
// or of one held.
function isLateResponse(_event) {
  return isClosedResponse(_event) || isCurrentResponse(_event) || isHeldResponse(_event);
}

// Another copy of the response in hand.
function isCurrentResponse(_event) {
  return !!_event.sendid && _event.sendid === response_id;
}

// Another copy of a held response: the first copy is kept, as
// <event:defer/> cannot replace an event it holds (decision D175).
function isHeldResponse(_event) {
  return !!_event.sendid && held_ids.indexOf(_event.sendid) >= 0;
}

// The held ids with this respond's, when its sender named one.
function heldIds(_event) {
  return _event.sendid ? held_ids.concat([_event.sendid]) : held_ids;
}

function canHold() {
  return held < held_responses_bound;
}

function heldOverMessage() {
  return 'holding ' + held + ' responses, the most it holds';
}

// The finished ids with the one in hand, the oldest dropped past the
// bound (decision D175).
function closedResponses() {
  var ids = response_id === null ? closed_responses : closed_responses.concat([response_id]);
  return ids.slice(Math.max(0, ids.length - closed_responses_bound));
}

function hopTimedOutMessage() {
  return 'response ' + operation + ' after ' + response_timeout_ms + ' ms';
}

// Speaking's playout: the environment's session, where its sound goes
// (decision D103, amended 2026-10-15).
function environmentTarget() {
  return '#_scxml_' + environment;
}
