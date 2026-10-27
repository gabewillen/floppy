// The ECMAScript of abilities/sound_classification/sound_classification.scx (SCXML 5.8): loaded by its root
// <script src="sound_classification.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// idle: each sound labelled from its kind.

// The kind trimmed, or blank when there is none.
function trimmedKind(kind) {
  return typeof kind === 'string' ? kind.trim() : '';
}

// Unlabeled for a blank kind, else the one label.
function classifiedOutput(_event) {
  var label = trimmedKind(_event.data.kind);
  return label === '' ? ({id: _event.data.id, labels: []}) : ({id: _event.data.id, labels: [label], confidence: confidence});
}
