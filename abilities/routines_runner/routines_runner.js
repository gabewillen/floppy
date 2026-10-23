// The ECMAScript of abilities/routines_runner/routines_runner.scx (SCXML 5.8): loaded by its root
// <script src="routines_runner.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// routine_on

// The path of the routine's program (its source).
function routineSource(index) {
  return routines[index].source;
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

// A running tick: a tick named as one of the chain's
// routines.
function isRoutineTick(_event) {
  return routines.some(function (routine) { return routine.name === _event.data.name; });
}
