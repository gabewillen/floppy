# Document ECMAScript (`<name>/<name>.js` beside a `.scx`)

This file is the ECMAScript of the SCXML document `<name>.scx` beside it,
loaded only by that document's root `<script src="<name>.js"/>` (SCXML 5.8,
D65); its functions are globals of the document's data model, which runs on
Boa. Review it against these rules and cite the rule.

- **Functions read and return.** Each function reads the data model (or the
  `_event` passed to it as a parameter) and returns a value. It never assigns
  data-model variables or other globals and has no side effects; state changes
  stay `<assign>` in the `.scx` (D65). Flag mutation of arguments or globals.
- **Every non-trivial `cond`/`expr` of the document calls a function here**,
  and each call site stays at most 160 characters. A function the document
  calls must exist here; a function must not throw on every evaluation (D123).
- **No payload validators.** Do not add `isObject`/`validX` shape checks of
  contract events: `event:schema` does that (D42). Shape checks of
  `done.invoke` donedata, service results, or stored rows read back from a
  database are allowed.
- **Numbers are plain JS numbers.** Do not imitate Python or pydantic float
  formatting (`1.0` vs `1`), `repr`-style number-to-string shims, or Python
  rounding helpers; emit numbers as JSON numbers and let JSON do the rest. The
  data model holds IEEE doubles and canonical JSON sorts object keys, so do not
  rely on key insertion order or big-integer precision (D88, D141).
- **Time.** Boa has no `Intl` or `Temporal`: time-zone logic uses POSIX TZ
  rules as `next_after` in `abilities/reflection/reflection.js` does (D83). Use
  `Date.now()` and UTC (`toISOString`, `Date.UTC`). Database timestamps come
  from SQL in UTC (D63, D69); do not hand-format a timestamp to look like a
  Python serializer's output when the schema only needs a valid date-time.
  Delays are `*_ms` integers, turned into a delay with the shared
  `delay(ms)` helper.
- **Bounds.** On wasm every value crossing the data-model boundary is limited
  to 64 KiB (D117), and a send over the event bound raises `error.execution`
  in the sender. Code that builds prompts or payloads must not assume more.
- **Reuse namespace elements.** Logic that exists as a namespace element is
  called through the element, not reimplemented here (D113 to D116).
- **Idiomatic code.** Write the behavior in ordinary ECMAScript; flag helper
  names, structure or data shapes carried over from a Python implementation
  rather than required by the behavior or the schema.
- **Style** (flag only gross drift): a header comment naming the document,
  functions grouped by state with `// <state>` comments, a one-line comment on
  what each returns.
- Builder output's `<name>.js` may be empty but always exists (D121).
