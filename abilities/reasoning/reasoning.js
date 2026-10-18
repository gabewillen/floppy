// The ECMAScript of abilities/reasoning/reasoning.scx (SCXML 5.8): loaded by its root
// <script src="reasoning.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared by the steps.

function hasNoDatabase() {
  return database === null;
}

// The input of bot.ability.reasoning.recalled.
function focusInput() {
  return ({focus: focus, focus_candidates: focus_candidates});
}

// This turn's bot.ability.reasoning.recalled.
function isCurrentTurn(_event) {
  return _event.data.turn === turn;
}

// recalling

// The original _context_ref_from_input: the stimulus's target_device, or null.
function contextRefFromInput() {
  return typeof Object(Object(stimulus).data).target_device === 'string' ? stimulus.data.target_device : null;
}

function hostInput() {
  return ({input: stimulus});
}

// The reference episodes.py: the prior cognitive episodes.
function episodeSelectStatements() {
  return [{sql: episode_select, parameters: ['cognitive_episode', context_ref]}];
}

// The reference directives.py: the standing directives appended.
function directiveSelectStatements() {
  return statements.concat([{sql: episode_select, parameters: ['standing_directive', context_ref]}]);
}

// The parsed contents of the rows of one statement's result; a row that is
// not a JSON object is skipped (the reference episodes_from_contents and
// directives_from_contents skip rows that do not validate).
function recalledContents(_event, index) {
  return _event.data.results[index].rows.map(function (row) { return parsedContent(row.values[0]); }).filter(function (content) { return content !== null; });
}

function parsedContent(text) {
  try {
    var content = JSON.parse(text);
    return content !== null && typeof content === 'object' && !Array.isArray(content) ? content : null;
  } catch (error) {
    return null;
  }
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

// The prompt over a budget: <cognition:model_facing_content> wrote the
// original's message.
function hasNoPrompt() {
  return prompt_failure !== null;
}

function projectionFailedResult() {
  return ({turn: turn, handled: false, failed: true, reason: prompt_failure});
}

// The code of a generate.failed, else gen_failure unchanged.
function generateFailure(_event) {
  return typeof Object(_event.data).code === 'string' ? _event.data.code : gen_failure;
}

function hasSelection() {
  return selected !== null;
}

function failedResult() {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning failed (' + (gen_failure === null ? 'no selection' : gen_failure) + ')'});
}

function couldNotRunResult(_event) {
  return ({turn: turn, handled: false, failed: true, reason: 'the reasoning could not run (' + _event.name + ')'});
}

// done

function selectedResult() {
  return ({turn: turn, handled: true, selected: selected});
}

// The data a selection is sent with, as the original's dispatch_selected_events
// takes it: its data object, else {} (no data, null, or a non-object).
function selectionData(s) {
  return s.data !== null && typeof s.data === 'object' && !Array.isArray(s.data) ? s.data : ({});
}
