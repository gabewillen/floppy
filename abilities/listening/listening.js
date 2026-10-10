// The ECMAScript of abilities/listening/listening.scx (SCXML 5.8): loaded by its root
// <script src="listening.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: send delays.

function delay(ms) {
  return ms + 'ms';
}

// live

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
