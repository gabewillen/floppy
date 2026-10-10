// The ECMAScript of abilities/routines_runner/routines_runner.scx (SCXML 5.8): loaded by its root
// <script src="routines_runner.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// routine_on

function routineSource(index) {
  return routines[index].source;
}

// mosfet Routines._is_running_tick: a tick from the routine, named as it.
function isRunningTick(_event) {
  return _event.invokeid === 'routine' && _event.data.name === routines[0].name;
}

// An error.* for the routine, or one with no invokeid (a source that does not compile).
function isRoutineError(_event) {
  return _event.invokeid === 'routine' || _event.invokeid === undefined;
}

// routine_on and rest_on

function routineFailure(index, _event) {
  return routines[index].name + ' (' + _event.name + ')';
}

// rest_deciding

function hasRest() {
  return routines.length > 1;
}

// rest_on

function restRoutines() {
  return routines.slice(1);
}

function isRest(_event) {
  return _event.invokeid === 'rest';
}
