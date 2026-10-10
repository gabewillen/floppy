// The ECMAScript of abilities/vision/vision.scx (SCXML 5.8): loaded by its root
// <script src="vision.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Classifying

// Text whose text is not blank.
function classifiesAsText() {
  return kind === 'text' && content.trim().length > 0;
}

// An image with a reference.
function classifiesAsImage() {
  return kind === 'image' && content.length > 0;
}
