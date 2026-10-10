// The ECMAScript of abilities/sensitivity/sensitivity.scx (SCXML 5.8): loaded by its root
// <script src="sensitivity.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Scoring: shared by quiet and producing.

// mosfet _score_unproduced: the sound's received level, null when nothing
// was measured.
function receivedLevel(sound) {
  return sound.received_level_db === undefined ? null : sound.received_level_db;
}

function scoreOutput(sound, stimulus, perceived) {
  return ({sound: sound, parent: stimulus, perceived_level_db: perceived});
}

// producing

function playoutDelay(sensitivity_playout_seconds) {
  return Math.round(sensitivity_playout_seconds * 1000) + 'ms';
}

function isOffProducingMouth(stimulus, sensitivity_producing_mouth) {
  return stimulus.source !== sensitivity_producing_mouth;
}

// mosfet _score_against_prediction: the observed path gain, null with no
// levels to compare.
function observedGain(sound) {
  return sound.amplitude_db == null || sound.received_level_db == null ? null : sound.received_level_db - sound.amplitude_db;
}

function hasNoObservedGain(observed) {
  return observed === null;
}

function hasNoLearnedGain(sensitivity_path_gain_db) {
  return sensitivity_path_gain_db === null;
}

function perceivedLevel(observed, sensitivity_path_gain_db) {
  return Math.abs(observed - sensitivity_path_gain_db);
}

function isPlayoutTimer(_event) {
  return _event.sendid === 'playout';
}
