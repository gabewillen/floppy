// The ECMAScript of abilities/behaviors/speech_heard/speech_heard.scx (SCXML 5.8): loaded by its root
// <script src="speech_heard.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle

// Speech with at least one source_id is admissible.
function hasAdmissibleSpeech() {
  return stimulus.data.source_ids.length > 0;
}

function turnData() {
  return ({source_ids: stimulus.data.source_ids, target_ids: [], content: stimulus.data.content});
}

// handled

function handledOutput() {
  return ({turn: turn, handled: true, selected: [{event: 'bot.ability.communication.input', data: turn_data, reason: 'seeded speech admit via communication'}]});
}

// unhandled

function unhandledOutput() {
  return ({turn: turn, handled: false});
}
