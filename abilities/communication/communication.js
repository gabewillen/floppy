// The ECMAScript of abilities/communication/communication.scx (SCXML 5.8): loaded by its root
// <script src="communication.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// attached

function isFromChild(_event) {
  return children.indexOf(_event.invokeid) >= 0;
}

function isNotFromChild(_event) {
  return children.indexOf(_event.invokeid) < 0;
}

function isFromConversation(_event) {
  return _event.invokeid === 'conversation';
}

function isFromSpeaking(_event) {
  return _event.invokeid === 'speaking';
}

// A child's own bot.ability.offers: it is ready.
function isChildOffers(_event) {
  return isFromChild(_event) && _event.data.ability === _event.invokeid;
}

// A conversation output whose last message is inbound.
function hasConversationProduct(_event) {
  return isFromConversation(_event) && _event.data.messages.length > 0 && _event.data.messages.slice(-1)[0].direction === 'inbound';
}

function productSequence(_event) {
  return String(_event.data.messages[_event.data.messages.length - 1].sequence);
}

function historyLength(_event) {
  return String(_event.data.messages.length);
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

// The queue with the event appended, as {event, data}.
function deferredQueue(_event) {
  return queue.concat([{event: _event.name, data: _event.data}]);
}

// active

function isQueuedNext(eventName) {
  return queue.length > 0 && queue[0].event === eventName;
}

// A queued bot.ability.communication.respond with non-blank text.
function hasQueuedRespond() {
  return isQueuedNext('bot.ability.communication.respond') && /\S/.test(queue[0].data.text);
}

function remainingQueue() {
  return queue.slice(1);
}

function hasRespond(_event) {
  return offering || (isNotFromChild(_event) && /\S/.test(_event.data.text));
}

function hasActivate(_event) {
  return offering || isNotFromChild(_event);
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

function hopTimedOutMessage() {
  return 'response ' + operation + ' after ' + response_timeout_ms + ' ms';
}
