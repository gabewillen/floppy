// The ECMAScript of abilities/listening/listening.scx (SCXML 5.8): loaded by its root
// <script src="listening.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: the sender of an event (its invoke id; none from the body) and
// send delays.

function isFromOutside(_event) {
  return !_event.invokeid;
}

function isFromSensitivity(_event) {
  return _event.invokeid === 'sensitivity';
}

function isFromInterpretation(_event) {
  return _event.invokeid === 'interpretation';
}

function delay(ms) {
  return ms + 'ms';
}

// live

function isChild(_event, children) {
  return children.indexOf(_event.invokeid) >= 0;
}

// A child's offers, sent under its own invoke id.
function isChildOffers(_event, children) {
  return isChild(_event, children) && _event.data.ability === _event.invokeid;
}

function childCouldNotRunLog(_event) {
  return 'the ' + _event.invokeid + ' child could not run (' + _event.name + ')';
}

function childEndedLog(_event) {
  return 'the ' + _event.invokeid + ' child ended';
}

// initializing

function everyChildReady(ready) {
  return ready.sensitivity === true && ready.interpretation === true;
}

function childCouldNotStartLog(_event) {
  return 'a child could not start (' + _event.name + ')';
}

// The queue with the sound of _event last.
function queueWithSound(_event, queue) {
  return queue.concat([_event.data]);
}

// Perceiving

// Not from a child: the body's efference report.
function isNotFromChild(_event, children) {
  return children.indexOf(_event.invokeid) < 0;
}

// Listening

function hasQueuedSound(queue) {
  return queue.length > 0;
}

// The oldest waiting sound.
function queuedSound(queue) {
  return queue[0];
}

// The queue without its oldest sound.
function queueRest(queue) {
  return queue.slice(1);
}

// Sensing

function scoreRequest(sound) {
  return ({stimulus: {event: 'environment.sound', data: sound}});
}

function sensitivityTimedOut() {
  return ({stage: 'sensitivity', message: 'Listening sensitivity timed out.'});
}

function isSensingTimer(_event) {
  return _event.sendid === 'sensing-timer';
}
