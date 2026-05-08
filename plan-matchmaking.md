# Matchmaking pool → `battles` table

Split the dual role of `runs` (live state + match pool) into two tables. `runs` keeps tracking the player's working state; `battles` becomes an append-only log of committed builds and powers matchmaking. The current "every shop transaction shows up in the pool" problem goes away, and we get a foundation for match history / stats / replays.

## Decisions locked in

- **Match against any historical battle**, not just the latest per player. Simpler query, gives the pool natural diversity (you can fight an old version of a player who's since moved on).
- **Snapshot at battle end**, in the same transaction as `record_score_and_upsert_run`, so we can store the result on the same row for free.
- **Seed migration on boot** so existing players don't fall out of matchmaking when we cut over.
- `runs.cost_value` and its index stay for now (no longer drive matchmaking but harmless). Revisit removing them once nothing reads them.

## Schema

```sql
CREATE TABLE battles (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL,
  player_id TEXT NOT NULL,
  name TEXT NOT NULL,
  cost_value INTEGER NOT NULL,
  build_json TEXT NOT NULL,
  wins_at_battle INTEGER NOT NULL,
  mmr_at_battle INTEGER NOT NULL,
  result TEXT NOT NULL,            -- "win" | "loss" | "draw"
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_battles_cost ON battles(cost_value);
CREATE INDEX idx_battles_player ON battles(player_id);
```

Notes:
- `name` denormalized so opponent display doesn't need a join against `player_profiles`.
- `result` stored from the player's perspective (the player whose row this is).
- Opponent fields (`opponent_player_id`, `opponent_run_id`, `opponent_build_json`, etc.) intentionally not added yet — easy to bolt on later when we actually surface match history in the UI.

## Implementation steps

1. **Schema** (`src/db.rs::Db::open`)
   - `CREATE TABLE IF NOT EXISTS battles ...` with the two indexes.
   - One-time migration gated on `PRAGMA user_version`: when stepping to v4, seed `battles` with one row per existing `runs` row that has `cost_value > 0`, using the run's current `build_json`/`cost_value`/`wins`/`mmr` and `result = 'win'` (best-effort placeholder; only matters for matchmaking, not stats).

2. **Insert path** (`src/db.rs::record_score_on` / `record_score_and_upsert_run`)
   - Add `insert_battle_on(conn, run, cost, result)` and call it inside the existing transaction in `record_score_and_upsert_run` so the upsert and battle insert commit atomically.
   - Determine `result` from the battle outcome already known at the call site in `src/main.rs` (the `Battle` ServerMsg already carries `winner`).

3. **Matchmaking** (`src/db.rs::find_opponent`)
   - Switch the query to `battles` instead of `runs`:
     ```sql
     SELECT id, player_id, run_id, name, build_json, mmr_at_battle
     FROM battles
     WHERE player_id != ?1
       AND cost_value BETWEEN ?2 AND ?3
     ```
     (no `current_run_id` exclusion needed once we exclude by `player_id`).
   - Keep the `[target − 15%, target]` band already in place.
   - Drop `recompute_cost_values` updates against `runs.cost_value` from being load-bearing — keep the function (still useful as a sanity sweep) but stop relying on it for matchmaking accuracy. Snapshots in `battles` are already correct at the moment they were written; if we ever bulk-edit prices, add a `recompute_battle_costs` pass with the same shape.

4. **Boot recompute** (`src/db.rs::recompute_cost_values`)
   - Extend (or duplicate) to also walk `battles` and recompute `cost_value` from `build_json` against current item/character defs. This is the one piece of B that *doesn't* go away — battle snapshots still need to track price changes so cost-banding stays accurate after a balance pass.
   - Logging stays the same shape: counts before/after, per-row debug logs for changes.

5. **Tests** (`src/db.rs::tests`)
   - Update `find_opponent_*` tests to insert into `battles` instead of (or in addition to) `runs`.
   - Add a test: a player with 3 historical `battles` rows at different costs should be matchable from any of those costs.
   - Add a test: a player with no `battles` rows is invisible to matchmaking (returns `None` → caller falls back to synthetic AI opponent in `main.rs`).

6. **Cleanup of empty-team workaround in `src/main.rs`**
   - The `pending_run` machinery and the empty-team upsert skip can come out. `runs` can accept any state including empties; matchmaking no longer reads it. The boot cleanup `DELETE FROM runs WHERE cost_value = 0` can also go (or stay as a cosmetic vacuum — low priority either way).
   - Verify the "sell your last guy" path now persists correctly: just always upsert.

## What this sets up

- **Match history**: `SELECT * FROM battles WHERE player_id = ? ORDER BY created_at DESC` — already there.
- **Stats**: win/loss counts, recent form, build evolution over time — all derivable from `battles`.
- **Replays / "the build that beat you"**: when we add opponent fields to the row, every battle has full data to reconstruct.

## Risks / things to watch

- **Pool size growth**: `battles` grows linearly with battles played. At our scale, fine for a long time. Index on `cost_value` keeps lookups fast. If it ever matters, sample / weight by recency rather than prematurely capping.
- **Power users overrepresented**: a player who battles 1000 times has 1000 entries in the pool. Roughly fair (they *are* more active), but if it skews matches noticeably, switch to "latest N per player" or weighted sampling later.
- **Build divergence after a price change**: handled by extending `recompute_cost_values` to also walk `battles`.
