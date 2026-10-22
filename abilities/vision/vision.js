// The ECMAScript of abilities/vision/vision.scx (SCXML 5.8): loaded by its root
// <script src="vision.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Classifying

// Text whose text is not blank.
function classifiesAsText() {
  return kind === 'text' && content.trim().length > 0;
}

// An image (its schema refuses an empty reference).
function classifiesAsImage() {
  return kind === 'image';
}

// The classifier's confidence for the classification.
function classificationConfidence() {
  return {text: text_confidence, image: image_confidence, unreadable: unreadable_confidence}[classification];
}
