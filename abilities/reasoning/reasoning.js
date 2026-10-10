// The ECMAScript of abilities/reasoning/reasoning.scx (SCXML 5.8): loaded by its root
// <script src="reasoning.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by recalling and retaining.

function hasNoDatabase() {
  return database === null;
}

function isFromInvoke(_event, invokeid) {
  return _event.invokeid === invokeid;
}

// The input of bot.ability.reasoning.recalled and .retained.
function focusInput() {
  return ({focus: focus, focus_candidates: focus_candidates});
}

// This turn's bot.ability.reasoning.recalled or .retained.
function isCurrentTurn(_event) {
  return _event.data.turn === turn;
}

// recalling

// mosfet _context_ref_from_input: the stimulus's target_device, or null.
function contextRefFromInput() {
  return typeof Object(Object(stimulus).data).target_device === 'string' ? stimulus.data.target_device : null;
}

function hostInput() {
  return ({input: stimulus});
}

// mosfet episodes.py: the prior cognitive episodes.
function episodeSelectStatements() {
  return [{sql: episode_select, parameters: ['cognitive_episode', context_ref]}];
}

// mosfet directives.py: the standing directives appended.
function directiveSelectStatements() {
  return statements.concat([{sql: episode_select, parameters: ['standing_directive', context_ref]}]);
}

// The parsed contents of the rows of one statement's result.
function recalledContents(_event, index) {
  return _event.data.results[index].rows.map(function (row) { return JSON.parse(row.values[0]); });
}

function recallFailedResult(detail) {
  return ({turn: turn, handled: false, failed: true, reason: 'Reasoning memory recall failed: ' + detail});
}

function hasRecalledWithoutChoices(_event) {
  return isCurrentTurn(_event) && choices.length === 0;
}

function noChoicesResult() {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning has no choices to select from'});
}

// applying

function choiceNamesLog() {
  return 'turn ' + turn + ': ' + choice_names;
}

function modelPrompt() {
  return JSON.stringify({stimulus: stimulus, focus: focus, focus_candidates: focus_candidates, prior_episodes: prior_episodes, standing_directives: standing_directives});
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

// The turn's episode, as JSON.
function episodeContent() {
  return JSON.stringify({focus: context_ref, focus_candidates: [], stimulus_name: stimulus.event, output: selected, behavior: null});
}

function episodeInsertStatements() {
  return [{sql: episode_insert, parameters: ['memory', context_ref, 'task', 'standard', 'retain', episode, 'application/json', 'cognitive_episode']}];
}

function retainFailedResult(detail) {
  return ({turn: turn, handled: false, failed: true, reason: 'Reasoning memory retain failed: ' + detail});
}

function selectedResult() {
  return ({turn: turn, handled: true, selected: selected});
}
