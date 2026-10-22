# Plan: episodic graph memory, embeddings and associative recall

Parity gap 9 and 10 ([parity gaps](parity-gaps.md)), workstream item M6. This
is a plan only: it needs the maintainer's decisions 3 (migrations; D126 is
proposed), 7 (rest and consolidation) and, for persons, 6 (participant
identity). Nothing here is built until they are answered.

## What the original has

Measured against the original's `main` at `0e77029`
(`src/mosfet/abilities/memory/`, `embedding/`):

- `memory.py` (1352 lines): the Memory ability grows from one statement
  transaction into a long-lived server of `bot.ability.memory.remember`,
  `.search`, `.fetch`, `.recent`, `.backfill`, `.lease`, `.consolidate`,
  `.release` and `.person`, each answered by its reply event or
  `bot.ability.memory.failed {message, kind}` (`payload.FailureKind`), and
  bounded by the operation timeout (60 s) and an embed timeout (30 s).
- `graph.py`, `index.py`, `payload.py`: episodes are nodes
  (`bot_memory_node`, with parts and links in the original's rewritten
  `0001`), indexed by an embedding vector; search lanes are nearest by
  vector, recent, involving someone present, and one link away.
- `associative.py` (1010 lines): scores candidates
  `similarity * cosine + recency * 0.5 ** (age / half_life) + present +
  linked * weight` (weights 1.0, 0.35, 0.5, 0.6) and admits them best first
  within `budget_tokens` 1024; `bot.ability.memory.recall` is the bot's
  model-callable deliberate recall; know/forget maintain the person index.
- `consolidation.py` (800 lines): consolidation while the cognition rests.
- `embedding/`: text and image embeddings through a remote embedder.

## Shape in floppy

Each step keeps the document structure rule (D39) and puts I/O in services.

1. **Embedding.** An `embed` operation on `services/generate` (POST
   `{base}/embeddings`, OpenAI's wire format, through the `generate`
   package's `env` slots), owned by the model-services workstream, wrapped
   by `abilities/embedding/embedding.scx` with the original's
   `bot.ability.embedding.*` events.
2. **Vectors in SQL.** `services/sql` registers a `vec_distance_cosine`
   scalar function (rusqlite `create_scalar_function`, deterministic) and
   accepts `{blob: hex}` parameters; `services/pgsql` maps the same
   statement to `pgvector` or a SQL expression (to decide with decision 2).
3. **Schema.** The graph tables as revision `0004` in both dialects
   (D126's non-breaking path), unless decision 3 chooses the original's
   rewrite.
4. **Memory serving.** `abilities/memory/memory.scx` keeps its one-shot
   transaction and gains a long-lived serving state for remember, search,
   fetch, recent and backfill, its SQL built by pure namespace elements
   `<memory:episode_statements>` and `<memory:search_statements>`, each
   result paged under the sql service's 88 KiB result bound. The failure
   event becomes the original's `bot.ability.memory.failed` in one hard
   cutover across every invoker (reasoning, reflection, identity,
   conversation, routines, autonomy, candidates runner, persona, stm);
   until then `bot.ability.failed` carries the original's `kind` (M1).
5. **Associative.** `abilities/associative/associative.scx` asks memory
   for candidates by message and scores them with a pure
   `<memory:associate>` element (the original's formula, weights and token
   budget), answering `bot.ability.memory.associated`.
6. **Cognition.** An `associating` step before the tiers fills the
   prompt's associations (C4's layout); reasoning's own recalling state
   and D97's episodes retire in the same change.
7. **Rest and consolidation** (decision 7) and **persons** (decision 6)
   follow as their own items.

## Tests

Each step brings its IRP suite (`abilities/<name>/irp`), namespace unit
tests for the pure elements (the scoring and statement builders against the
original's fixtures), `tests/tests/memory.rs` cases on SQLite and the
`postgres_` variants, and a live run through the environment once
cognition associates.
