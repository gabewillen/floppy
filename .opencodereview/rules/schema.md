# Event schemas (`abilities/schemas/`, `bots/*/schemas/`, builder `<name>.json`)

Each file is the one JSON Schema of one event, referenced from documents with
`event:schema`. scxml.rs checks it where the event is created, on sends between
sessions and on host input (D41). Review against these rules and cite them.
Rules marked *(contracts.rs)* are enforced by `tests/tests/contracts.rs`.

- **One file per event** (D125): ability events in `abilities/schemas/`, body
  events in `bots/floppy/schemas/`, builder contracts in
  `bots/builder/schemas/`. No copies or twins; no file name twice under
  `abilities/`, `bots/`, `environments/`, `devices/`, `services/`
  *(contracts.rs)*.
- `"$schema": "https://json-schema.org/draft/2020-12/schema"`; `title` equals
  the file name without `.json`; every `examples` entry satisfies the schema
  *(contracts.rs)*.
- Adding or removing a schema file updates `BODY_EVENTS` / `ABILITY_EVENTS`
  (and `CORRELATED` when relevant) in contracts.rs and the name list
  `abilities/schemas/floppy-event-names.json` (sorted, unique) in the same
  change. A name floppy adds is either in that list or marked `[floppy name]`
  at the start of `description`, never both *(contracts.rs)*.
- Correlated, non-tool events of a turn require `turn` (`integer`,
  `minimum: 1`); tool events, whose data a model fills, must not (D36).
- Never carry runtime-only fields: `operation_id`, `generation`,
  `parent_operation_id`, `parent_generation`, `token`, `request_id`,
  `actor_id`, `capability` (D36) *(contracts.rs)*.
- Media is a reference (`format: uri-reference`); no `format: base64url`
  (D36) *(contracts.rs)*.
- No cross-file `$ref` (scxml.rs refuses it): shared shapes are inlined, and
  every participant item is the one `{ref, role, own, resembles}` Participant,
  identical everywhere (D157) *(contracts.rs)*. A recursive "any JSON" is `{}`.
- Fields floppy adds are marked `[floppy field]` in their `description`;
  floppy-only enum values and checks `[floppy value]` / `[floppy check]`.
- Tool events keep model-facing `description` prose, titles and examples.
  Prefer `additionalProperties: false` on objects; generate drops only
  undeclared top-level fields, nested objects stay strict (D112).
- **Describe the contract, not a serializer.** Constrain values the way the
  behavior needs (`type: number`, `format: date-time`); do not encode one
  language's formatting quirks (forcing floats to carry `.0`, Python datetime
  string patterns) into a schema.
- Schema descriptions are new text like any other: they do not cite another
  implementation ("as the original", "the reference's model"); state what the
  field means.
- Builder output `<name>.json` is an object whose `$defs` is keyed by input
  event and referenced as `<name>.json#/$defs/<event>` (D121).
- The body's choices and offers are held to 32 KiB (`bound="32768"`, D35);
  flag changes that inflate tool schemas noticeably.
