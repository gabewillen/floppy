# Plan: episodic graph memory, embeddings and associative recall

Gaps 9 and 10 ([open gaps](parity-gaps.md)), workstream item M6. This
is a plan only: it needs the maintainer's decisions 3 (migrations; D165 is
proposed), 7 (rest and consolidation) and, for persons, 6 (participant
identity). Nothing here is built until they are answered.

## Target behavior

- The memory ability grows from one statement transaction into a
  long-lived server of `bot.ability.memory.remember`, `.search`, `.fetch`,
  `.recent`, `.backfill`, `.lease`, `.consolidate`, `.release` and
  `.person`, each answered by its reply event or
  `bot.ability.memory.failed {message, kind}`, and bounded by an
  operation timeout (60 s) and an embed timeout (30 s).
- Episodes are nodes (`bot_memory_node`, with parts and links), indexed
  by an embedding vector; search lanes are nearest by vector, recent,
  involving someone present, and one link away.
- Associative recall scores candidates
  `similarity * cosine + recency * 0.5 ** (age / half_life) + present +
  linked * weight` (weights 1.0, 0.35, 0.5, 0.6) and admits them best first
  within `budget_tokens` 1024; `bot.ability.memory.recall` is the bot's
  model-callable deliberate recall; know and forget maintain the person
  index.
- Consolidation runs while the cognition rests.
- Text and image embeddings come from a remote embedder.

## Shape in floppy

Each step keeps the document structure rule (D39) and puts I/O in services.

1. **Embedding.** An `embed` operation on `services/generate` (POST
   `{base}/embeddings`, OpenAI's wire format, through the `generate`
   package's `env` slots), owned by the model-services workstream, wrapped
   by `abilities/embedding/embedding.scx` with `bot.ability.embedding.*`
   events.
2. **Vectors in SQL.** `services/sql` registers a `vec_distance_cosine`
   scalar function (rusqlite `create_scalar_function`, deterministic) and
   accepts `{blob: hex}` parameters; `services/pgsql` maps the same
   statement to `pgvector` or a SQL expression (to decide with decision 2).
3. **Schema.** The graph tables as revision `0004` in both dialects
   (D165's non-breaking path), unless decision 3 chooses rewriting `0001`
   instead.
4. **Memory serving.** `abilities/memory/memory.scx` keeps its one-shot
   transaction and gains a long-lived serving state for remember, search,
   fetch, recent and backfill, its SQL built by pure namespace elements
   `<memory:episode_statements>` and `<memory:search_statements>`, each
   result paged under the sql service's 88 KiB result bound. The failure
   event becomes `bot.ability.memory.failed {message, kind}` in one hard
   cutover across every invoker (reasoning, reflection, identity,
   conversation, routines, autonomy, candidates runner, persona, stm);
   until then `bot.ability.failed` carries `kind` (M1).
5. **Associative.** `abilities/associative/associative.scx` asks memory
   for candidates by message and scores them with a pure
   `<memory:associate>` element (the formula, weights and token budget
   above), answering `bot.ability.memory.associated`.
6. **Cognition.** An `associating` step before the tiers fills the
   prompt's associations (C4's layout); reasoning's own recalling state
   and D97's episodes retire in the same change.
7. **Rest and consolidation** (decision 7) and **persons** (decision 6)
   follow as their own items.

## Tests

Each step brings its IRP suite (`abilities/<name>/irp`), namespace unit
tests for the pure elements (the scoring and statement builders against
fixtures), `tests/tests/memory.rs` cases on SQLite and the
`postgres_` variants, and a live run through the environment once
cognition associates.
