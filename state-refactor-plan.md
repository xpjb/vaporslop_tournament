# State refactor: split live state from matchmaking pool, fix multi-session races

## The cooked situation today

A single `runs` table is doing two unrelated jobs:

1. **Live player state** — one row per *run*, mutated on every shop action (`BuyCharacter`, `BuyItem`, `Sell`, `Reroll`, `Reorder`, `MoveItem`, `SetProfile` rename…). `upsert_run` runs after almost every `ClientMsg`.
2. **Matchmaking pool** — `find_opponent` reads `runs` filtered by `cost_value` band.

Consequences:

- Every mid-shop tweak instantly republishes the player as a matchmaking opponent, even though the player hasn't committed to that build. You can be matched against someone's half-built team that they were about to sell.
- The "team is empty for a moment" path needs a `pending_run` in-memory workaround in [src/main.rs:397](src/main.rs:397) so empties don't end up in the pool.
- Each socket carries its own `current_run_id` + `pending_run` ([src/main.rs:393-397](src/main.rs:393)). Two tabs/sessions for the same player both load the run from disk, both mutate, both `upsert_run` — last write wins, and shop state observed by one tab is silently overwritten by the other. The login plan ([plan-auth.md](plan-auth.md)) added session pinning but didn't address the per-socket cache, so registered users can still race themselves across tabs.

This plan splits the two roles, adds proper multi-session arbitration, and stops the shop from polluting matchmaking.

## Decisions locked in

- **Two tables**: `player_state` (live, mutable, one row per `player_id`) and `battles` (append-only, snapshot at battle commit, powers matchmaking). The current `runs` table becomes `player_state` after a rename + drop of matchmaking columns.
- **One row per `player_id`**, not per run. The current "one row per run UUID" model is overkill — we never actually surface old runs in the UI; `load_latest_run_for_player` is the only reader. The run's UUID survives as a column on `player_state` so battle records can reference it, but the primary key becomes `player_id`.
- **Battles are written exactly once**, in the same transaction as `record_score_and_upsert_run`, *only* on a real `ClientMsg::Battle` commit. Shop clicks never touch `battles`.
- **Single active socket per `player_id`**. When a second WS connects for the same `player_id`, the older one is evicted (sent `{ type: "session_replaced" }`, then closed). Eliminates the stale-cache race entirely.
- **Server-side action serialization per player**: actions for a given `player_id` are processed under a per-player async mutex on the server, so even within a single socket two in-flight messages can't interleave reads/writes. (Cheap insurance; the single-socket rule already covers most of it.)
- **Cookie-bound** — for registered users, the only way to act is via the session cookie, so `session_replaced` happens at the WS level (not the HTTP `/api/login` level).
- **Guests are still keyed by their localStorage UUID**; `session_replaced` works for them too (two tabs in the same browser share the UUID).
- Drop `runs.cost_value` and its index. Keep `cost_value` only on `battles`. The boot recompute walks `battles` only.

## Schema

Migrating in `src/db.rs::Db::open` behind `PRAGMA user_version = 4`:

```sql
-- 1. Rename + reshape: runs -> player_state, keyed on player_id
CREATE TABLE player_state (
  player_id     TEXT PRIMARY KEY,
  run_id        TEXT NOT NULL,         -- UUID for the current run; battles reference this
  name          TEXT NOT NULL,
  money         INTEGER NOT NULL,
  wins          INTEGER NOT NULL,
  losses        INTEGER NOT NULL,
  streak        INTEGER NOT NULL,
  best_streak   INTEGER NOT NULL,
  mmr           INTEGER NOT NULL DEFAULT 1000,
  alive         INTEGER NOT NULL,
  phase         TEXT NOT NULL,
  build_json    TEXT NOT NULL,
  shop_json     TEXT NOT NULL,
  updated_at    INTEGER NOT NULL
);
-- (no cost_value, no idx_runs_cost; matchmaking no longer reads this table)

-- 2. Append-only matchmaking pool
CREATE TABLE battles (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id          TEXT NOT NULL,         -- player_state.run_id at commit time
  player_id       TEXT NOT NULL,
  name            TEXT NOT NULL,         -- denormalized so opponent display avoids a join
  cost_value      INTEGER NOT NULL,
  build_json      TEXT NOT NULL,
  wins_at_battle  INTEGER NOT NULL,
  mmr_at_battle   INTEGER NOT NULL,
  result          TEXT NOT NULL,         -- "win" | "loss" | "draw" (player's perspective)
  created_at      INTEGER NOT NULL
);
CREATE INDEX idx_battles_cost   ON battles(cost_value);
CREATE INDEX idx_battles_player ON battles(player_id);
```

### Migration (one-shot at v4)

1. Create `player_state` and `battles` per above.
2. For each player_id in `runs`, copy the **latest** row (by `updated_at`) into `player_state`, mapping `runs.id` → `player_state.run_id`. Discard older rows for the same player — they were never user-visible anyway (`load_latest_run_for_player` already only returned the newest).
3. Seed `battles` from every `runs` row with `cost_value > 0`: one row per `runs` row, with `result = 'win'` placeholder (only matters for matchmaking, not stats; no real history exists pre-cutover). This preserves the matchmaking pool across the cutover so the lobby doesn't go cold.
4. Drop `runs` (or rename to `runs_legacy_v3` and DROP after one boot if paranoid).
5. `PRAGMA user_version = 4`.

`recompute_cost_values` is repurposed to walk `battles.build_json` only.

## Code changes

### `src/db.rs`

Replace run-centric helpers with state-centric ones:

- `upsert_player_state(state: &PlayerState)` — used by every shop action.
- `load_player_state(player_id) -> Option<PlayerState>` — replaces `load_run` + `load_latest_run_for_player`.
- `insert_battle_on(conn, run, cost, result)` — private, called inside the txn.
- `record_battle_and_upsert_state(state, cost, result, update_mmr, completed_ultimate_victory)` — replaces `record_score_and_upsert_run`. One transaction: `upsert_player_state` + `insert_battle_on` + `record_score_on` + profile progress.
- `find_opponent(current_player_id, target_cost)` — query becomes:
  ```sql
  SELECT id, run_id, player_id, name, build_json, mmr_at_battle
  FROM battles
  WHERE player_id != ?1
    AND cost_value BETWEEN ?2 AND ?3
  ```
  No `current_run_id` exclusion needed (we filter on `player_id`). Drop the `current_run_id` parameter from callers.
- Drop `recompute_cost_values` against `runs.cost_value`; rewrite to walk `battles` instead. Same logging shape.
- Drop the legacy `Phase::Battle` -> `Phase::Shop` rewrite (no longer relevant; `phase` lives on `player_state` and is small).
- Test refactor: every test that calls `db.upsert_run` becomes a `db.upsert_player_state` call; tests that exercise `find_opponent` insert into `battles`.

### `src/main.rs`

**Per-player serialization + single-active-socket arbitration** (replaces the per-socket `current_run_id` / `pending_run` machinery):

```rust
struct PlayerSlot {
    /// Wakes the current owner so it can shut down before the new one starts.
    evict: tokio::sync::oneshot::Sender<()>,
    /// Held by the current owner for the life of its socket.
    /// New owners await this lock to take over.
    lock: Arc<tokio::sync::Mutex<()>>,
}

struct AppState {
    // ...
    sessions: parking_lot::Mutex<HashMap<String, PlayerSlot>>,
}
```

Flow on a new WS message that resolves to `player_id = P`:

1. If another `PlayerSlot` exists for `P`, send `{ type: "session_replaced" }` to it via the `evict` channel and remove it from the map. The old socket task observes the eviction in its `tokio::select!` loop, sends the message, and breaks.
2. Insert a fresh `PlayerSlot` keyed on `P`, take the lock for the duration of this socket.
3. Every `ClientMsg` for `P` runs under that lock — load fresh state from DB, mutate, write back, all atomic from the socket's perspective.
4. On disconnect, release the lock and remove the slot if it's still ours.

Drop `pending_run` and `current_run_id` from `handle_socket` — we always reload `player_state` from the DB at the start of each action and write it back at the end, under the per-player lock. The "empty team between Sell and next action" case stops being a problem because the empty state is just a normal `player_state` row; matchmaking doesn't care.

**Battle commit path** ([src/main.rs:1024](src/main.rs:1024) area): pass the resolved `result` (win/loss/draw from `res.winner`) into `record_battle_and_upsert_state`. The battle row is inserted in the same txn as the leaderboard / state update.

**Shop path**: every other `ClientMsg` ends with `state.db.upsert_player_state(&run)`. No `cost_value` argument. No `pending_run`.

**`NewRun` semantics**: instead of inventing a new run-row UUID and treating it as primary, just `upsert_player_state` with a fresh `run_id` (UUID) on the existing `player_state` row. The previous run's data is overwritten — same as today's `load_latest_run_for_player` behavior, made explicit.

**`Resume`**: `load_player_state(player_id)` and send `State { run, profile }`. No more "find the latest of N runs."

### `static/main.js`

- Handle `{ type: "session_replaced" }`: show a "this run is being played in another tab" banner with a **Reload** button. Don't auto-reconnect — that would just kick the other tab back. Stops the WS.
- Already the case that the client uses `localStorage.playerUuid`; nothing to change there.

## Concrete answers to the user's three points

> **It stores everything in the run table including the players current active runs.**

Two tables: `player_state` for "what the player is currently doing", `battles` for the matchmaking pool. The shop only writes `player_state`.

> **It needs to not push up 'runs' from when you simply adjust your loadout during the shop phase, only from the build you submit to fight (it can submit after including win or loss etc).**

The only writer of `battles` is `record_battle_and_upsert_state`, which is only called from the `ClientMsg::Battle` arm in `main.rs`. Shop actions never touch `battles`. The battle row is written *after* the fight resolves, so it carries `wins_at_battle`, `mmr_at_battle`, and `result`.

> **It needs to play nicely with the login system.**

`player_state.player_id` is the same UUID identity used by `player_profiles` and `sessions`. The existing `resolve_player_id` ([src/main.rs:377](src/main.rs:377)) still applies: session cookie wins over body. Nothing in this refactor changes the auth model — it just consolidates the per-player run row to match the per-player profile/session shape.

> **The login system does weird stuff if you take actions from two different sessions at the same time.**

That's the per-socket cache + last-write-wins on the shared `runs` row. Fixed by:
1. **Single active socket per `player_id`** — the new socket evicts the old one with `session_replaced`. Two tabs in the same browser, two browsers signed into the same account, mobile + desktop — all funnel to one live socket. (No silent data loss; the evicted tab knows it lost.)
2. **Per-player async mutex** — even within one socket, messages process serially against fresh DB state. No stale `pending_run` snapshots driving writes.
3. **No more `current_run_id` cached on the socket** — every action loads `player_state` fresh, mutates, writes back, releases.

## Implementation steps (order matters)

1. **Schema migration** in `Db::open`: create new tables, copy data, bump `user_version` to 4. Verify on a copy of `vaporslop.sqlite` first — back up via the existing `vaporslop_backup.sh` before running.
2. **DB layer rewrite**: introduce `PlayerState` struct (a thin alias / move from `Run`), `upsert_player_state`, `load_player_state`, `insert_battle_on`, `record_battle_and_upsert_state`, `find_opponent` against `battles`. Keep the old fns as `#[deprecated]` shims for one commit if it makes the diff readable, then delete.
3. **Boot recompute**: rewrite `recompute_cost_values` to walk `battles`.
4. **Per-player session arbitration in `AppState`**: add `sessions: HashMap<String, PlayerSlot>`, the eviction `oneshot`, and the per-player `Mutex`. Wire `handle_socket` to acquire/release.
5. **`handle_socket` rewrite**: drop `current_run_id` and `pending_run`. Every action: load `player_state` under lock → mutate → write back → reply. Battle path goes through `record_battle_and_upsert_state` with the resolved `result`.
6. **Tests**: update `find_opponent_*` to seed `battles` directly. Add tests for:
   - Shop actions don't insert into `battles`.
   - Battle commits insert exactly one `battles` row with the right `result`.
   - A second WS for the same `player_id` evicts the first.
   - Two messages on the same socket can't interleave (lock test — spawn two `tokio::spawn` futures racing on the same player_id; assert serialized writes).
7. **Frontend**: handle `session_replaced` in [static/main.js](static/main.js). Banner + Reload button.
8. **Cleanup**: delete the empty-team workaround in `handle_socket`. Delete `DELETE FROM runs WHERE cost_value = 0` boot path. Drop `cost_value` from `runs` (now `player_state`) — already happens via the migration recreating the table.

## Risks / things to watch

- **Migration over a populated DB**: the prod sqlite file is the source of truth for live players. Take a backup; consider gating the v4 migration behind an env var the first time so it can be tested on a copy. Wrap in a transaction.
- **Battle pool seeding**: every existing player gets exactly one `battles` row (their last `runs` snapshot, `result='win'`). After cutover, the pool is "what was in `runs`"; over the next few real battles the pool grows organically.
- **Session eviction UX**: must be obvious. A silent close looks like a network error. The `session_replaced` message + visible banner is the contract.
- **Lock holding across `await` points**: `record_battle_and_upsert_state` runs synchronous SQLite under a parking_lot mutex; the per-player tokio mutex wraps the whole "load/mutate/write" sequence. Don't call `argon2` or any other slow work inside it. Battle resolution itself is CPU-bound and quick — fine to run inside.
- **Multiple registered tabs is a real workflow** for some users (one tab on phone, one on desktop). The eviction model says "you can't play simultaneously" — that's a deliberate design call, matches the user's complaint, and is consistent with "one player, one run."
- **Guest UUID drift**: nothing changes here. Two browsers = two guest UUIDs = two independent player_states. The eviction rule only fires when `player_id` actually collides.
- **Pool growth**: `battles` grows linearly with battles played. Index on `cost_value` keeps lookups O(log n). Revisit sampling/pruning if it ever matters.
