# Defs versioning — followup: thin events, client-resolved sprites

After [plan-versioning.md](plan-versioning.md) lands, replays are deterministic but the wire format is fat. Every `Combatant` carries inline sprite strings for the unit and each gear slot ([combat.rs:7-65](src/game/combat.rs:7)), and `Combatant` is cloned into events. A long battle's JSON is mostly duplicated sprite paths.

This followup shifts the def table to a client-fetched resource keyed by `DefsVersion`. The replay payload becomes ids-and-numbers; the client resolves sprites from a cached def table per version.

## What changes

- **New endpoint:** `GET /api/defs?version=N` returns the resolved `DefsTable` as JSON. Cacheable forever (each version is immutable). Missing version → 404.
- **Replay payload slims:** events reference units and items by `def_id` / `item_id` and *position* (existing `uid`, `side`, slot index). Sprite, name, stats fields drop off `Combatant` for the wire.
- **Client cache:** keyed by `DefsVersion`. Replay loader fetches `/api/defs?version={payload.version}` first, then renders events against it. Live battles use `?version=current` (or skip the cache entirely on the live path — the client already has the relevant defs from `/api/defs/initial` or equivalent).

## Type signatures

The split is between the *runtime* `Combatant` (used inside `resolve_v1`, holds inlined stats so the inner loop never hashes) and the *wire* `CombatantSnapshot` (sent to the client, holds only ids and dynamic state).

```rust
// src/game/combat.rs

/// Server-side runtime state. Stays fat — inner combat loop reads these fields.
/// NOT serialized to the client.
pub struct Combatant {
    pub uid: u32,
    pub unit: UnitDef,            // owned, from ResolvedMember
    pub hat: Option<ItemDef>,
    pub left_hand: Option<ItemDef>,
    pub right_hand: Option<ItemDef>,
    pub hand_3: Option<ItemDef>,
    pub hand_4: Option<ItemDef>,
    pub hp: i32,
    pub max_hp: i32,
    pub might: i32,
    pub reflexes: i32,
    pub wisdom: i32,
    pub mana: i32,
    pub max_mana: i32,
    pub frozen_turns: i32,
    pub side: u8,
    pub revive_charges: u8,
    pub revive_at_back_charges: u8,
    // ...applied_* bookkeeping fields stay here
}

/// Wire shape. References defs by id; client resolves names/sprites/properties
/// from the cached DefsTable for `replay.version`.
#[derive(Serialize, Deserialize)]
pub struct CombatantSnapshot {
    pub uid: u32,
    pub side: u8,
    pub def_id: String,
    pub hat_id: Option<String>,
    pub left_hand_id: Option<String>,
    pub right_hand_id: Option<String>,
    pub hand_3_id: Option<String>,
    pub hand_4_id: Option<String>,
    // Only the dynamic state the client needs to animate:
    pub hp: i32,
    pub max_hp: i32,
    pub mana: i32,
    pub frozen_turns: i32,
}

impl From<&Combatant> for CombatantSnapshot { /* trivial */ }

/// Events serialize via `CombatantSnapshot`, never `Combatant`.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Spawn { combatant: CombatantSnapshot },
    Hit { attacker: u32, target: u32, damage: i32, /* ... */ },
    Death { uid: u32 },
    // ...
}
```

```rust
// src/main.rs (new handler)

#[derive(Deserialize)]
struct DefsQuery { version: u32 }

async fn defs_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DefsQuery>,
) -> impl IntoResponse {
    let table = state.defs.table_at(DefsVersion(q.version))
        .ok_or(StatusCode::NOT_FOUND)?;
    // Long Cache-Control: each version's table is immutable.
    ([(header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
     Json(table)).into_response()
}
```

`DefsTable` becomes `Serialize`, which is the only public API change visible to the client.

## Client side

```js
// static/main.js — sketch

const defsCache = new Map(); // version → DefsTable

async function getDefs(version) {
  let table = defsCache.get(version);
  if (!table) {
    table = await fetch(`/api/defs?version=${version}`).then(r => r.json());
    defsCache.set(version, table);
  }
  return table;
}

async function playReplay(replayPayload) {
  const defs = await getDefs(replayPayload.version);
  for (const ev of replayPayload.events) {
    render(ev, defs); // resolves sprites/names/properties by id
  }
}
```

Sprites and display names are no longer transmitted per-combatant per-event. The def table is fetched once per version and cached for the session (and by the browser HTTP cache across sessions, since each version URL is immutable).

## Migration

This is a pure wire-format change; nothing in the DB moves. But it is breaking for any in-flight client connected to an older bundle. Options:

1. **Hard cut.** Bump a wire-format integer; old client refuses to render new replays with a "reload" prompt. Simplest.
2. **Dual-emit.** Server emits both shapes behind a query flag for one release; clients migrate. More moving parts, only worth it if there's a real stickiness problem.

For this codebase: hard cut. There are no long-lived websockets that hold a replay payload across deploys.

## Implementation steps

1. **`DefsTable: Serialize`** in `src/game/defs.rs`. Add a test that round-trips through JSON.
2. **Add `/api/defs` handler** in [main.rs](src/main.rs). Returns `DefsTable` JSON; 404 on unknown version; immutable Cache-Control.
3. **Split `Combatant` into runtime + snapshot.**
   - Strip name/sprite/properties from the wire shape; keep them in the runtime shape.
   - `Event` variants serialize `CombatantSnapshot` where they currently serialize `Combatant`.
   - Internal references inside combat stay as `Combatant`.
4. **Client def cache** in [main.js](static/main.js). Fetch on first replay open per version, cache in-memory plus rely on HTTP cache.
5. **Client renderer** consumes `CombatantSnapshot + DefsTable` instead of `Combatant`. The sprite-lookup logic moves from "read `combatant.sprite`" to "look up `defs.units[def_id].sprite`."
6. **Drop the `version_mismatch` UI hint.** With per-version def fetch, there's no mismatch — old replays render correctly under old defs. Keep the badge if you want to flag "this is from an old patch" cosmetically; remove it if the goal was only "warn about wrong stats."

## What this buys

- Replay JSON shrinks by a large factor in long battles (sprites are the heaviest repeated field).
- Browser caches each `DefsTable` once per version, indefinitely.
- Adding a sprite-only change (rename an asset, tweak a name) becomes a `defs` bump without re-encoding any historical replay payloads.
- Server stops cloning sprite strings into every event.

## What's still deferred

- **Combat-rule versioning.** Still a third pass. The right time is when the first non-numeric rule change actually ships; the dispatch is a match arm at the `resolve_v*` call site. Don't introduce the trait abstraction speculatively.
- **Def deletion / tombstones.** Same as plan-versioning.md: only when needed.
