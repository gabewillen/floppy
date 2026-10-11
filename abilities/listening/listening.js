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

// Sensing

function scoreRequest(sound) {
  return ({stimulus: {event: 'environment.sound', data: sound}});
}

function sensitivityTimedOut() {
  return ({stage: 'sensitivity', message: 'Listening sensitivity timed out.'});
}
