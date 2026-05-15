# Defs versioning — replay determinism through rebalancing

Replays today re-run combat against whatever `character_defs()` / `item_defs()` return *now*. The moment any numeric rebalance lands, every prior replay silently plays out under the new rules; the only thing preserved is the recorded winner ([main.rs:1410-1424](src/main.rs:1410)).

This plan introduces a versioned def registry. Replays bind to the version they were recorded under. Numeric rebalances become append-only: bump the registry, old replays stay accurate, the matchmaking pool keeps working because opponent snapshots are reference-only and read against the current version when matched.

## Decisions locked in

- **One version per replay snapshot.** Stored on the `battles` row, not per slot.
- **Storage is per-id history**, not per-snapshot table. Only the def that actually changed gets a new entry.
- **Opponent pool is version-blind.** `opponents.build_json` stays just string refs; matchmaking interprets it at `defs.current_version()`. Same row, different lens.
- **No tombstones for now.** Deletion is out of scope; can only retire ids by removing them from shop pools, never from `Defs`. Add tombstones the day a real deletion is needed.
- **Combat rule changes are deferred.** This plan only solves numeric rebalances. Plan-versioning-followup covers the boundary cleanup; combat-rule versioning is a third pass when the first rule change actually appears.

## Type signatures (load-bearing)

These are the contracts the rest of the code rests on. Notice what is *not* expressible:

- You cannot construct a `Team` except via `DefsTable::resolve` (private fields). Combat therefore cannot run against a `Build` that hasn't been pinned to a version.
- `DefsTable` cannot be constructed except via `Defs::table_at`. There is no way to assemble a def table at a "blended" version.
- `resolve_v1` takes `&DefsTable` and `&Team`. It cannot reach the registry to pull a different version mid-sim. The version is fully captured by its inputs.

```rust
// src/game/defs.rs (new)

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefsVersion(pub u32);

pub struct Defs {
    current: DefsVersion,
    // Per-id history, sorted by version descending. First entry whose version
    // is <= requested is the answer.
    units: HashMap<String, Vec<(DefsVersion, UnitDef)>>,
    items: HashMap<String, Vec<(DefsVersion, ItemDef)>>,
}

impl Defs {
    pub fn load() -> Self { /* hard-coded today; data file later */ }
    pub fn current_version(&self) -> DefsVersion { self.current }
    pub fn current_table(&self) -> DefsTable { self.table_at(self.current).expect("current always resolves") }
    pub fn table_at(&self, version: DefsVersion) -> Option<DefsTable>;
}

pub struct DefsTable {
    version: DefsVersion,
    units: HashMap<String, UnitDef>,
    items: HashMap<String, ItemDef>,
}

impl DefsTable {
    pub fn version(&self) -> DefsVersion { self.version }
    pub fn unit(&self, id: &str) -> Option<&UnitDef> { self.units.get(id) }
    pub fn item(&self, id: &str) -> Option<&ItemDef> { self.items.get(id) }

    /// The only way to produce a `Team`. Returns `None` if any id in the build
    /// is unknown at this version (corrupt replay, deleted def, typo, etc).
    pub fn resolve(&self, build: &Build) -> Option<Team>;
}

/// Resolved snapshot of a Build pinned to a specific DefsTable. Combat input.
/// Fields are private; constructed only by `DefsTable::resolve`.
pub struct Team {
    version: DefsVersion,           // for debug/asserts only
    members: Vec<ResolvedMember>,
}

pub struct ResolvedMember {
    pub unit: UnitDef,              // owned clone, small
    pub hat: Option<ItemDef>,
    pub left_hand: Option<ItemDef>,
    pub right_hand: Option<ItemDef>,
    pub hand_3: Option<ItemDef>,
    pub hand_4: Option<ItemDef>,
}

// src/game/combat.rs

/// Pure function. Output is determined entirely by inputs.
/// `defs` must be the same table `a` and `b` were resolved against;
/// debug-assert that `a.version() == b.version() == defs.version()`.
pub fn resolve_v1(
    defs: &DefsTable,
    a: &Team,
    b: &Team,
    seed: u32,
) -> Vec<Event>;
```

`Build` keeps its current shape ([types.rs:235-237](src/game/types.rs:235)) — string ids, no version. Storage form, version-blind, durable.

## DB migration

The `battles` table already has a `version_hash INTEGER` column ([db.rs](src/db.rs)). Reinterpret it as `defs_version`:

- No schema change required.
- One-time migration in `Db::open`: existing rows already have `version_hash = 1` from the `VERSION_HASH = 0x0000_0001` constant ([mod.rs:10](src/game/mod.rs:10)). They map to `DefsVersion(1)` cleanly.
- `opponents.build_json` rows: unchanged. Reference-only, version-agnostic.
- New battles record `defs.current_version().0` into the column.

## Implementation steps

1. **Add `defs.rs` and types** (`src/game/defs.rs`)
   - `DefsVersion`, `Defs`, `DefsTable`, `Team`, `ResolvedMember` as above.
   - `Team` and `DefsTable` fields are private; constructors are the only entry.
   - `Defs::load` builds v1 from the existing literals in [data.rs](src/game/data.rs). The two `build_*_defs()` functions move into `Defs::load` and become `Vec<(DefsVersion(1), ...)>` per id.

2. **Plumb `&Defs` through `AppState`**
   - `AppState` gains `pub defs: Defs` (built once at startup, no `OnceLock`).
   - Delete `CHARACTER_DEFS` / `ITEM_DEFS` `OnceLock` ([data.rs:427-441](src/game/data.rs:427)).
   - Delete free functions `character_def` / `item_def` from the public API. The 25 call sites become `state.defs.current_table().unit(id)` (or carry a `&DefsTable` through).

3. **Convert call sites** (mechanical)
   - Handlers in [main.rs](src/main.rs) (~10 sites): take `defs: &DefsTable` from `state.defs.current_table()` at the top of the handler, pass down.
   - [shop.rs](src/game/shop.rs) helpers (~5 sites): take `&DefsTable` parameter.
   - [combat.rs](src/game/combat.rs) `materialize_team` (~4 sites): becomes `DefsTable::resolve`.
   - [types.rs:240-256](src/game/types.rs:240) `Build::cost_value`: becomes `fn cost_value(&self, defs: &DefsTable) -> i32`.

4. **Rewrite `resolve_battle` as `resolve_v1`**
   - Move from [combat.rs:185+](src/game/combat.rs:185).
   - New signature: `fn resolve_v1(defs: &DefsTable, a: &Team, b: &Team, seed: u32) -> Vec<Event>`.
   - Team materialization is gone — the `Team` arrives pre-resolved. `Combatant` construction reads from `ResolvedMember`, not the global table.
   - `SummonOnEnemyDeath` / `SummonOnAllyDeath` at [combat.rs:679](src/game/combat.rs:679) and [combat.rs:758](src/game/combat.rs:758) call `defs.unit(species)` against the same table that resolved the original team. No hidden version drift.

5. **Replay handler version-dispatch** ([main.rs:1357+](src/main.rs:1357))
   ```rust
   let table = state.defs.table_at(DefsVersion(replay.version_hash))
       .ok_or(/* unknown version → fall back to recorded_winner only */)?;
   let player_team = table.resolve(&replay.player_build).ok_or(/* corrupt */)?;
   let enemy_team  = table.resolve(&replay.enemy_build).ok_or(/* corrupt */)?;
   let events = resolve_v1(&table, &player_team, &enemy_team, replay.combat_seed);
   ```
   - The `version_mismatch` field stays on the response purely as a hint to the UI for "this replay is under an older balance" — but events are now actually correct for that version. Winner is no longer overridden.

6. **Live-battle write path** ([main.rs](src/main.rs) battle commit)
   - Resolve via `state.defs.current_table()`.
   - Record `defs.current_version().0` into `battles.version_hash`.

7. **Delete `pub const VERSION_HASH`** ([mod.rs:10](src/game/mod.rs:10))
   - Single source of truth becomes `Defs::current_version()`. Bumping a version means adding `(DefsVersion(2), ...)` entries to `Defs::load` and incrementing `Defs::current`.

## What this does and does not buy

**Buys:**
- Numeric rebalances are append-only. Old replays are bit-for-bit correct.
- Opponent pool keeps working without per-row migration — same JSON, read against current.
- The static-global pattern is gone; combat is a pure function of explicit inputs.
- Adding a new version is "edit `Defs::load`, bump current." No call-site changes.

**Does not buy:**
- Combat-rule changes still bump determinism. When the first one lands, route by version at the `resolve_battle` call site: `match replay_version { ..=N => resolve_v1, N+1.. => resolve_v2 }`. The trait abstraction can wait until there's a second implementation to compare against.
- Replay JSON still carries inlined sprites on every `Combatant` ([combat.rs:7-65](src/game/combat.rs:7)). That's plan-versioning-followup's problem.

## Test coverage

- `defs_table_at_picks_latest_le_version` — v1 entry + v3 entry, request v2 → v1.
- `defs_table_at_returns_none_for_unknown_version` — request v99 against a registry whose max is v3.
- `resolve_returns_none_on_unknown_id` — build references a def that doesn't exist at that version.
- `replay_against_old_version_matches_recorded_outcome` — record a battle at v1, bump to v2 with a stat change, replay → events use v1 stats, winner matches recorded.
- `opponent_pool_reads_at_current_version` — pool row written at v1, matchmaking pulls it after bump to v2 → cost_value evaluated at v2.
