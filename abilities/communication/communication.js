// The ECMAScript of abilities/communication/communication.scx (SCXML 5.8): loaded by its root
// <script src="communication.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// communication

// The sequence of the product's latest message.
function productSequence(_event) {
  var messages = _event.data.stimulus.data.messages;
  return String(messages[messages.length - 1].sequence);
}

function historyLength(_event) {
  return String(_event.data.messages.length);
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

// The queue with the event appended, as {event, data}.
function deferredQueue(_event) {
  return queue.concat([{event: _event.name, data: _event.data}]);
}

// active

function hasRespond(_event) {
  return offering || /\S/.test(_event.data.text);
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
