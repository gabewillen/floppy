# `consolidation` implementation report plan

The implementation report plan of [`consolidation.scx`](../consolidation.scx): the W3C SCXML IRP's
manifest format and `Outcome` oracle over the document's numbered
statements below, one assertion each in [`manifest.xml`](manifest.xml),
tested by the harness documents in [`sources/`](sources). Each harness
invokes the document as its child, drives it, and ends in `pass` (logging
`Outcome` `pass`) or `fail`. `tests/tests/document_irp.rs` runs every case
through `scx run` with the real invoked services (a case's
`backend.json` scripts the fake HTTP backend they reach), and
`tests/irp/harness.py` writes the cases from [`cases.py`](cases.py).

## Statements

- <a id="consolidation-1"></a>**consolidation-1**. With no `memory` `<param>` (null) it logs `memory` `absent` and ends at once, its donedata `{folded: 0, merged: 0, steps: 0, failed: false, reason: "absent"}`.
- <a id="consolidation-2"></a>**consolidation-2**. With a database it reads the people memory holds through the Memory ability (which brings the database to the head revision), folds the voices heard for each person since into their representative and merges two people whose representatives are at the conversation's threshold, one guarded transaction per step; with nothing to do it ends with `{folded: 0, merged: 0, steps: 0, failed: false, reason: null}`.
- <a id="consolidation-3"></a>**consolidation-3**. A memory step that fails ends it at once with `failed: true` and the failure as its `reason`.
