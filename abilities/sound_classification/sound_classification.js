// The ECMAScript of abilities/sound_classification/sound_classification.scx (SCXML 5.8): loaded by its root
// <script src="sound_classification.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// applying: the sound labelled from its kind.

// The kind trimmed, or blank when there is none.
function trimmedKind() {
  return typeof kind === 'string' ? kind.trim() : '';
}

// Unlabeled for a blank kind, else the one label.
function classifiedOutput() {
  return label === '' ? ({labels: []}) : ({labels: [label], confidence: confidence});
}
