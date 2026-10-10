// The ECMAScript of abilities/reasoning/reasoning.scx (SCXML 5.8): loaded by its root
// <script src="reasoning.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// recalling

function hostInput() {
  return ({input: stimulus});
}

// The input of bot.ability.reasoning.recalled and .retained.
function focusInput() {
  return ({focus: focus, focus_candidates: focus_candidates});
}

// mosfet Reasoning._has_recalled: this turn's recall, memory not consulted.
function hasRecalled(_event) {
  return _event.data.turn === turn && _event.data.memory_consulted === false;
}

function hasRecalledWithoutChoices(_event) {
  return hasRecalled(_event) && choices.length === 0;
}

function noChoicesResult() {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning has no choices to select from'});
}

// applying

function choiceNamesLog() {
  return 'turn ' + turn + ': ' + choice_names;
}

function modelPrompt() {
  return JSON.stringify({stimulus: stimulus, focus: focus, focus_candidates: focus_candidates});
}

// The events of a generate.result object reply, else selected unchanged.
function generatedSelection(_event) {
  return _event.name === 'generate.result' && Object(_event.data).json instanceof Object && !Array.isArray(_event.data.json) ? _event.data.json.events : selected;
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return _event.name === 'generate.failed' && typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

function isGenerateDone(_event) {
  return _event.invokeid === gen_id;
}

function hasSelection(_event) {
  return isGenerateDone(_event) && selected !== null;
}

function failedResult() {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning failed (' + (gen_failure === null ? 'no selection' : gen_failure) + ')'});
}

function couldNotRunResult(_event) {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning could not run (' + _event.name + ')'});
}

// retaining

// mosfet Reasoning._has_retained: this turn's retain, memory not written.
function hasRetained(_event) {
  return _event.data.turn === turn && _event.data.memory_written === false;
}

function selectedResult() {
  return ({turn: turn, handled: true, selected: selected});
}
