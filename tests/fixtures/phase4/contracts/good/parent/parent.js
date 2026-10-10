// The ECMAScript of the contract fixture parent (SCXML 5.8), loaded by its
// root <script src="parent.js">.

// Whether the event came from the literal child.
function fromChild(event) {
  return event.invokeid === 'child';
}
