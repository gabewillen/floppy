// The ECMAScript of abilities/routines_runner/routines_runner.scx (SCXML 5.8): loaded by its root
// <script src="routines_runner.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// routine_on

function routineSource(index) {
  return routines[index].source;
}

// An error.* for the routine, or one with no invokeid (a source that does
// not compile). The routine and the rest run at once, and a child that
// fails after it started raises error.communication in this session
// whichever child it is (scxml.rs names no invoke in it), so only the
// event's invokeid tells them apart.
function isRoutineError(_event) {
  return _event.invokeid === 'routine' || _event.invokeid === undefined;
}

function isRestError(_event) {
  return _event.invokeid === 'rest';
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

// running

// mosfet Routines._is_running_tick: a tick named as one of the chain's
// routines.
function isRoutineTick(_event) {
  return routines.some(function (routine) { return routine.name === _event.data.name; });
}
