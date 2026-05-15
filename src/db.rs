use crate::game::defs::{Defs, DefsTable};
use crate::game::rng::Rng;
use crate::game::types::*;
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::sync::Arc;

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug)]
pub enum AttachErr {
    AlreadyRegistered,
    UsernameTaken,
    Db(rusqlite::Error),
}

impl std::fmt::Display for AttachErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachErr::AlreadyRegistered => write!(f, "already_registered"),
            AttachErr::UsernameTaken => write!(f, "username_taken"),
            AttachErr::Db(e) => write!(f, "db_error: {e}"),
        }
    }
}

impl std::error::Error for AttachErr {}

/// Player-perspective outcome of a battle, stored on the `battles` replay-log row.
#[derive(Debug, Clone, Copy)]
pub enum BattleOutcome {
    Win,
    Loss,
    Draw,
}

impl BattleOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            BattleOutcome::Win => "win",
            BattleOutcome::Loss => "loss",
            BattleOutcome::Draw => "draw",
        }
    }

    fn from_str(raw: &str) -> Option<Self> {
        match raw {
            "win" => Some(BattleOutcome::Win),
            "loss" => Some(BattleOutcome::Loss),
            "draw" => Some(BattleOutcome::Draw),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReplayRecord {
    pub id: i64,
    pub player_id: String,
    pub enemy_player_id: String,
    pub player_build: Build,
    pub enemy_build: Build,
    pub player_mmr_before: i32,
    pub enemy_mmr_before: i32,
    pub combat_seed: u32,
    /// Which [`crate::game::defs::DefsVersion`] (numeric) produced this replay. Persists under
    /// the legacy SQLite column name `version_hash`.
    pub version_hash: u32,
    pub outcome: BattleOutcome,
    pub created_at: i64,
}

/// Lightweight replay listing row: enough to render the firehose/history list
/// without loading the full builds. Click-through fetches the full replay via
/// `replay_record`.
#[derive(Debug, Clone)]
pub struct ReplaySummary {
    pub id: i64,
    pub player_id: String,
    pub player_name: String,
    pub player_avatar: String,
    pub player_mmr_before: i32,
    pub enemy_player_id: String,
    pub enemy_name: String,
    pub enemy_avatar: String,
    pub enemy_mmr_before: i32,
    pub outcome: BattleOutcome,
    pub created_at: i64,
}

impl Db {
    pub fn open(path: &str, defs: &DefsTable) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA busy_timeout = 5000;
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
        "#,
        )?;

        let db_ver: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        // Pre-v4 schema: legacy `runs` table doubled as live state + matchmaking pool.
        // Pre-v7 schema: `leaderboard` mirrored profile rows. Both are dropped by later
        // migrations; we only need them to exist (with the right columns) for those
        // migrations to read from. Gating these on db_ver < {4,7} keeps post-migration
        // opens from re-creating empty husks of dropped tables.
        if db_ver < 4 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS runs (
                    id TEXT PRIMARY KEY,
                    player_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    money INTEGER NOT NULL,
                    wins INTEGER NOT NULL,
                    losses INTEGER NOT NULL,
                    streak INTEGER NOT NULL,
                    best_streak INTEGER NOT NULL,
                    alive INTEGER NOT NULL,
                    phase TEXT NOT NULL,
                    build_json TEXT NOT NULL,
                    shop_json TEXT NOT NULL,
                    cost_value INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_runs_cost ON runs(cost_value);
            "#,
            )?;
            ensure_column(&conn, "runs", "player_id", "TEXT")?;
            ensure_column(&conn, "runs", "best_streak", "INTEGER NOT NULL DEFAULT 0")?;
            ensure_column(&conn, "runs", "mmr", "INTEGER NOT NULL DEFAULT 1000")?;
            conn.execute(
                "UPDATE runs SET player_id = id WHERE player_id IS NULL OR player_id = ''",
                [],
            )?;
            conn.execute(
                "UPDATE runs SET best_streak = streak WHERE best_streak < streak",
                [],
            )?;
            // Older versions persisted Phase::Battle. We never want to resume into that;
            // collapse to Shop. (Same coercion lands later for player_state during the
            // v4 migration since it copies from `runs`.)
            conn.execute(
                "UPDATE runs SET phase = ?1 WHERE phase = ?2",
                params![
                    serde_json::to_string(&Phase::Shop)?,
                    serde_json::to_string(&Phase::Battle)?,
                ],
            )?;
        }
        if db_ver < 7 {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS leaderboard (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    streak INTEGER NOT NULL,
                    wins INTEGER NOT NULL,
                    created_at INTEGER NOT NULL
                )",
                [],
            )?;
            ensure_column(&conn, "leaderboard", "player_id", "TEXT")?;
            ensure_column(&conn, "leaderboard", "updated_at", "INTEGER")?;
            ensure_column(&conn, "leaderboard", "mmr", "INTEGER NOT NULL DEFAULT 1000")?;
            conn.execute(
                "UPDATE leaderboard SET player_id = id WHERE player_id IS NULL OR player_id = ''",
                [],
            )?;
            conn.execute(
                "UPDATE leaderboard SET updated_at = created_at WHERE updated_at IS NULL",
                [],
            )?;
            conn.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_leaderboard_player ON leaderboard(player_id)",
                [],
            )?;
        }
        if db_ver < 2 {
            // One-time: remove ladder rows from older versions (bots are generated on demand now).
            conn.execute(
                "DELETE FROM runs WHERE name GLOB '*_bot_[0-9][0-9][0-9]'",
                [],
            )?;
            conn.pragma_update(None, "user_version", 2)?;
        }
        if db_ver < 3 {
            conn.execute(
                r#"UPDATE runs SET build_json = REPLACE(build_json, '"frostscepter"', '"winterstaff"')"#,
                [],
            )?;
            conn.execute(
                r#"UPDATE runs SET shop_json = REPLACE(shop_json, '"frostscepter"', '"winterstaff"')"#,
                [],
            )?;
            conn.pragma_update(None, "user_version", 3)?;
        }

        // player_state schema (unchanged since v4). The legacy v4-shape `battles` table
        // and its indexes are only needed as a stepping stone for the v4 INSERT below; the
        // v6 migration further down rebuilds `battles` into `opponents` + a new `battles`
        // replay log. Gate the legacy bootstrap on db_ver < 6 so post-v6 installs don't
        // try to create indexes on columns the new schema doesn't have.
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS player_state (
                player_id   TEXT PRIMARY KEY,
                run_id      TEXT NOT NULL,
                name        TEXT NOT NULL,
                money       INTEGER NOT NULL,
                wins        INTEGER NOT NULL,
                losses      INTEGER NOT NULL,
                streak      INTEGER NOT NULL,
                best_streak INTEGER NOT NULL,
                mmr         INTEGER NOT NULL DEFAULT 1000,
                alive       INTEGER NOT NULL,
                phase       TEXT NOT NULL,
                build_json  TEXT NOT NULL,
                shop_json   TEXT NOT NULL,
                updated_at  INTEGER NOT NULL
            );
            "#,
        )?;
        if db_ver < 6 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS battles (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id          TEXT NOT NULL,
                    player_id       TEXT NOT NULL,
                    name            TEXT NOT NULL,
                    cost_value      INTEGER NOT NULL,
                    build_json      TEXT NOT NULL,
                    wins_at_battle  INTEGER NOT NULL,
                    mmr_at_battle   INTEGER NOT NULL,
                    result          TEXT NOT NULL,
                    created_at      INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_battles_cost   ON battles(cost_value);
                CREATE INDEX IF NOT EXISTS idx_battles_player ON battles(player_id);
                "#,
            )?;
        }
        if db_ver < 4 {
            // Seed player_state from the latest `runs` row per player_id.
            // Window-function pick handles ties on updated_at deterministically.
            conn.execute(
                r#"
                INSERT OR IGNORE INTO player_state
                  (player_id, run_id, name, money, wins, losses, streak, best_streak,
                   mmr, alive, phase, build_json, shop_json, updated_at)
                SELECT player_id, id, name, money, wins, losses, streak, best_streak,
                       mmr, alive, phase, build_json, shop_json, updated_at
                FROM (
                    SELECT *,
                           ROW_NUMBER() OVER (PARTITION BY player_id ORDER BY updated_at DESC, rowid DESC) AS rn
                    FROM runs
                ) WHERE rn = 1
                "#,
                [],
            )?;
            // Seed the matchmaking pool from any existing run with a non-empty team.
            // result='win' is a placeholder; only matters for matchmaking, not real history.
            conn.execute(
                r#"
                INSERT INTO battles
                  (run_id, player_id, name, cost_value, build_json,
                   wins_at_battle, mmr_at_battle, result, created_at)
                SELECT id, player_id, name, cost_value, build_json,
                       wins, mmr, 'win', updated_at
                FROM runs
                WHERE cost_value > 0
                "#,
                [],
            )?;
            // Drop the legacy table outright. Nothing in the new code reads it.
            conn.execute_batch("DROP TABLE IF EXISTS runs;")?;
            conn.pragma_update(None, "user_version", 4)?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS player_daily (
                player_id TEXT NOT NULL,
                day_id INTEGER NOT NULL,
                PRIMARY KEY (player_id, day_id)
            );",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS player_profiles (
                player_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                selected_avatar TEXT NOT NULL DEFAULT 'meme_man',
                best_wins INTEGER NOT NULL DEFAULT 0,
                ultimate_victories INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        ensure_column(&conn, "player_profiles", "username", "TEXT")?;
        ensure_column(&conn, "player_profiles", "password_hash", "TEXT")?;
        conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_profiles_username
             ON player_profiles(username COLLATE NOCASE)
             WHERE username IS NOT NULL",
            [],
        )?;
        if db_ver < 5 {
            // Cap historical best_wins values from when the run cap was higher than MAX_WINS.
            conn.execute(
                "UPDATE player_profiles SET best_wins = ?1 WHERE best_wins > ?1",
                params![MAX_WINS],
            )?;
            conn.pragma_update(None, "user_version", 5)?;
        }
        ensure_column(&conn, "player_profiles", "mmr", "INTEGER NOT NULL DEFAULT 1000")?;
        ensure_column(&conn, "player_profiles", "best_streak", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_column(&conn, "player_profiles", "last_battle_at", "INTEGER NOT NULL DEFAULT 0")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                token TEXT PRIMARY KEY,
                player_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_player ON sessions(player_id);",
        )?;
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        conn.execute(
            "DELETE FROM sessions WHERE expires_at < ?1",
            params![now_secs],
        )?;
        if db_ver < 6 {
            // v6: split the old `battles` matchmaking pool into:
            //   - `opponents` (the pool, slimmed down — drops name/run_id/wins_at_battle/result)
            //   - `battles`   (new: append-only replay log: which two opponent snapshots
            //                  fought, the combat seed, version hash, outcome)
            // Done as a rename+rebuild so old ids carry over (any future external ref keeps
            // working). Wrapped in a transaction so a partial migration leaves the DB clean.
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                r#"
                ALTER TABLE battles RENAME TO opponents_old;

                CREATE TABLE opponents (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    player_id       TEXT    NOT NULL,
                    cost_value      INTEGER NOT NULL,
                    build_json      TEXT    NOT NULL,
                    mmr_at_snapshot INTEGER NOT NULL,
                    created_at      INTEGER NOT NULL
                );

                INSERT INTO opponents (id, player_id, cost_value, build_json, mmr_at_snapshot, created_at)
                SELECT id, player_id, cost_value, build_json, mmr_at_battle, created_at
                FROM opponents_old;

                DROP TABLE opponents_old;

                CREATE INDEX idx_opponents_cost   ON opponents(cost_value);
                CREATE INDEX idx_opponents_player ON opponents(player_id);

                CREATE TABLE battles (
                    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                    player_opponent_id  INTEGER NOT NULL REFERENCES opponents(id),
                    enemy_opponent_id   INTEGER NOT NULL REFERENCES opponents(id),
                    combat_seed         INTEGER NOT NULL,
                    version_hash        INTEGER NOT NULL,
                    outcome             TEXT    NOT NULL,
                    player_mmr_before   INTEGER NOT NULL,
                    created_at          INTEGER NOT NULL
                );
                CREATE INDEX idx_battles_player_opp ON battles(player_opponent_id);
                CREATE INDEX idx_battles_enemy_opp  ON battles(enemy_opponent_id);
                "#,
            )?;
            tx.pragma_update(None, "user_version", 6)?;
            tx.commit()?;
        }
        if db_ver < 7 {
            // v7: collapse the `leaderboard` table into `player_profiles`. Previously each
            // battle commit wrote to both tables; `name` and the per-player best `wins` were
            // mirrored, which let them drift (notably: v5 capped `player_profiles.best_wins`
            // to MAX_WINS but left `leaderboard.wins` at the historical higher value, so the
            // profile page showed 30 while the leaderboard showed 36 for the same player).
            //
            // After this migration, `player_profiles` carries the ranking columns (mmr,
            // best_streak, last_battle_at) and is itself the leaderboard — the LB query is
            // just an ORDER BY over this table, filtered to players who have battled.
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO player_profiles
                   (player_id, name, selected_avatar, best_wins, ultimate_victories)
                 SELECT player_id, COALESCE(name, 'anon'), 'meme_man', 0, 0
                 FROM leaderboard
                 WHERE player_id IS NOT NULL AND player_id != ''",
                [],
            )?;
            // Cap leaderboard.wins at MAX_WINS so the historical overshoot doesn't survive.
            tx.execute(
                "UPDATE player_profiles AS p
                 SET mmr = (SELECT l.mmr FROM leaderboard l WHERE l.player_id = p.player_id),
                     best_streak = MAX(
                       p.best_streak,
                       (SELECT l.streak FROM leaderboard l WHERE l.player_id = p.player_id)
                     ),
                     best_wins = MAX(
                       p.best_wins,
                       MIN((SELECT l.wins FROM leaderboard l WHERE l.player_id = p.player_id), ?1)
                     ),
                     last_battle_at = COALESCE(
                       (SELECT l.updated_at FROM leaderboard l WHERE l.player_id = p.player_id),
                       0
                     )
                 WHERE EXISTS (SELECT 1 FROM leaderboard l WHERE l.player_id = p.player_id)",
                params![MAX_WINS],
            )?;
            tx.execute("DROP TABLE leaderboard", [])?;
            tx.pragma_update(None, "user_version", 7)?;
            tx.commit()?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_profiles_rank
             ON player_profiles(mmr DESC, best_wins DESC, best_streak DESC, last_battle_at ASC)
             WHERE last_battle_at > 0",
            [],
        )?;
        recompute_opponent_costs(&conn, defs)?;
        Ok(Db {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// UTC calendar day as days since Unix epoch (for grouping daily logins).
    pub fn utc_day_id_now() -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now / 86_400
    }

    pub fn touch_player_daily(&self, player_id: &str) -> Result<()> {
        let day_id = Self::utc_day_id_now();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO player_daily (player_id, day_id) VALUES (?1, ?2)",
            params![player_id, day_id],
        )?;
        Ok(())
    }

    pub fn count_players_logged_in_today(&self) -> Result<u32> {
        let day_id = Self::utc_day_id_now();
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM player_daily WHERE day_id = ?1",
            [day_id],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    pub fn ensure_player_profile(&self, player_id: &str, name: &str) -> Result<PlayerProfile> {
        let conn = self.conn.lock();
        ensure_player_profile_on(&conn, player_id, name)
    }

    pub fn update_player_profile(
        &self,
        player_id: &str,
        name: &str,
        selected_avatar: &str,
    ) -> Result<PlayerProfile> {
        let name = clean_profile_name(name);
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO player_profiles(player_id,name,selected_avatar)
             VALUES(?1,?2,?3)
             ON CONFLICT(player_id) DO UPDATE SET
               name=excluded.name,
               selected_avatar=excluded.selected_avatar",
            params![player_id, name, selected_avatar],
        )?;
        conn.execute(
            "UPDATE player_state SET name = ?2 WHERE player_id = ?1",
            params![player_id, name],
        )?;
        load_player_profile_on(&conn, player_id)
    }

    pub fn backfill_profile_name(&self, player_id: &str, name: &str) -> Result<PlayerProfile> {
        let name = clean_profile_name(name);
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE player_profiles
             SET name = ?2
             WHERE player_id = ?1
               AND name = 'anon'
               AND ?2 != 'anon'",
            params![player_id, name],
        )?;
        load_player_profile_on(&conn, player_id)
    }

    pub fn username_for_player(&self, player_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT username FROM player_profiles WHERE player_id = ?1")?;
        let mut rows = stmt.query([player_id])?;
        if let Some(row) = rows.next()? {
            let username: Option<String> = row.get(0)?;
            Ok(username.filter(|s| !s.is_empty()))
        } else {
            Ok(None)
        }
    }

    pub fn attach_credentials(
        &self,
        player_id: &str,
        username: &str,
        password_hash: &str,
    ) -> std::result::Result<(), AttachErr> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO player_profiles(player_id,name,selected_avatar,best_wins,ultimate_victories)
             VALUES(?1,'anon','meme_man',0,0)",
            params![player_id],
        )
        .map_err(AttachErr::Db)?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT username FROM player_profiles WHERE player_id = ?1",
                [player_id],
                |row| row.get(0),
            )
            .map_err(AttachErr::Db)?;
        if existing.as_deref().filter(|s| !s.is_empty()).is_some() {
            return Err(AttachErr::AlreadyRegistered);
        }
        let res = conn.execute(
            "UPDATE player_profiles SET username = ?2, password_hash = ?3 WHERE player_id = ?1",
            params![player_id, username, password_hash],
        );
        match res {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(AttachErr::UsernameTaken)
            }
            Err(e) => Err(AttachErr::Db(e)),
        }
    }

    /// Find a registered account by username (case-insensitive). Returns (player_id, password_hash).
    pub fn find_account(&self, username: &str) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT player_id, password_hash FROM player_profiles
             WHERE username = ?1 COLLATE NOCASE
             LIMIT 1",
        )?;
        let mut rows = stmt.query([username])?;
        if let Some(row) = rows.next()? {
            let pid: String = row.get(0)?;
            let hash: Option<String> = row.get(1)?;
            if let Some(hash) = hash.filter(|s| !s.is_empty()) {
                return Ok(Some((pid, hash)));
            }
        }
        Ok(None)
    }

    pub fn create_session(&self, player_id: &str, ttl_secs: i64) -> Result<String> {
        let token = crate::auth::gen_session_token();
        let now = crate::auth::now_unix();
        let expires_at = now + ttl_secs;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO sessions(token, player_id, created_at, expires_at, last_seen_at)
             VALUES(?1, ?2, ?3, ?4, ?3)",
            params![token, player_id, now, expires_at],
        )?;
        Ok(token)
    }

    /// Look up a session by token. Returns the bound `player_id` if it's still valid, otherwise
    /// `None` (and removes the row if expired). On a successful hit, slides `expires_at` forward
    /// by `slide_ttl_secs` so active sessions don't expire underfoot.
    pub fn lookup_session(&self, token: &str, slide_ttl_secs: i64) -> Result<Option<String>> {
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock();
        let now = crate::auth::now_unix();
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT player_id, expires_at FROM sessions WHERE token = ?1",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((player_id, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at <= now {
            let _ = conn.execute("DELETE FROM sessions WHERE token = ?1", [token]);
            return Ok(None);
        }
        let new_expires = now + slide_ttl_secs;
        conn.execute(
            "UPDATE sessions SET last_seen_at = ?2, expires_at = MAX(expires_at, ?3) WHERE token = ?1",
            params![token, now, new_expires],
        )?;
        Ok(Some(player_id))
    }

    pub fn delete_session(&self, token: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM sessions WHERE token = ?1", [token])?;
        Ok(())
    }

    /// Read-only profile load; returns `None` if no row exists (does not create one).
    pub fn load_profile(&self, player_id: &str) -> Result<Option<PlayerProfile>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT player_id,name,selected_avatar,best_wins,ultimate_victories
             FROM player_profiles
             WHERE player_id = ?1",
        )?;
        let mut rows = stmt.query([player_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(PlayerProfile {
                player_id: row.get(0)?,
                name: row.get(1)?,
                selected_avatar: row.get(2)?,
                best_wins: row.get(3)?,
                ultimate_victories: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn profile_avatar(&self, player_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT selected_avatar FROM player_profiles WHERE player_id = ?1")?;
        let mut rows = stmt.query([player_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Current display name for `player_id`, or `None` if no profile row exists.
    /// Used to label an opponent at battle time (we don't pin their name onto the
    /// opponents snapshot, so a rename is always reflected).
    pub fn profile_name(&self, player_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT name FROM player_profiles WHERE player_id = ?1")?;
        let mut rows = stmt.query([player_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Persist the player's working state (one row per player_id). Called by every shop action.
    /// Never writes to `battles` — that only happens at battle commit.
    pub fn upsert_player_state(&self, run: &Run) -> Result<()> {
        let conn = self.conn.lock();
        upsert_player_state_on(&conn, run)?;
        Ok(())
    }

    /// Load the live `Run` for a player_id. Returns None if the player has never started a run.
    pub fn load_player_state(&self, player_id: &str) -> Result<Option<Run>> {
        let conn = self.conn.lock();
        load_player_state_on(&conn, player_id)
    }

    /// Atomic battle commit: publish the player's post-battle snapshot to `opponents`,
    /// append a replay-log row to `battles` (only when there's a real enemy snapshot),
    /// update leaderboard + profile progress, and persist post-battle player_state — all
    /// in one transaction.
    ///
    /// `enemy_opponent_id` is `None` for synthetic AI battles; in that case no replay row
    /// is written (we don't snapshot bot builds into `opponents`).
    /// `combat_seed` is the seed that drove combat `resolve_v1`, needed for byte-for-byte replay.
    /// `player_mmr_before` is the player's MMR going into this battle (pre-update).
    pub fn record_battle_and_save_state(
        &self,
        run: &Run,
        result: BattleOutcome,
        update_mmr: bool,
        completed_ultimate_victory: bool,
        enemy_opponent_id: Option<i64>,
        combat_seed: u32,
        player_mmr_before: i32,
        defs: &DefsTable,
    ) -> Result<Option<i64>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        record_profile_progress_on(
            &tx,
            &run.player_id,
            &run.name,
            run.wins,
            run.best_streak,
            run.mmr,
            update_mmr,
            completed_ultimate_victory,
        )?;
        upsert_player_state_on(&tx, run)?;
        let player_opponent_id = insert_opponent_on(&tx, run, defs)?;
        let mut replay_id = None;
        if let (Some(player_id), Some(enemy_id)) = (player_opponent_id, enemy_opponent_id) {
            replay_id = Some(insert_battle_on(
                &tx,
                player_id,
                enemy_id,
                combat_seed,
                defs,
                result,
                player_mmr_before,
            )?);
        }
        tx.commit()?;
        Ok(replay_id)
    }

    /// Pick another player's historical snapshot whose cost is near `target_cost`.
    /// Allowed band: equal to or up to 15% below `target_cost` (ceiled, at least 1 gold of slack).
    /// Returns (opponent_id, player_id, build, mmr_at_snapshot). The caller looks up the
    /// opponent's current display name via `player_profiles` — we don't pin a stale name.
    pub fn find_opponent(
        &self,
        current_player_id: &str,
        target_cost: i32,
        rng: &mut Rng,
    ) -> Result<Option<(i64, String, Build, i32)>> {
        let conn = self.conn.lock();
        let down = ((target_cost as f32) * 0.15).ceil().max(1.0) as i32;
        let min_cost = (target_cost - down).max(1);
        let max_cost = target_cost;
        // Stable ORDER BY so candidate ordering is independent of SQLite's
        // execution plan — the rng pick must be reproducible from `(state, seed)`.
        let mut stmt = conn.prepare(
            "SELECT id, player_id, build_json, mmr_at_snapshot
             FROM opponents
             WHERE player_id != ?1
               AND cost_value BETWEEN ?2 AND ?3
             ORDER BY id",
        )?;
        let mut rows = stmt.query(params![current_player_id, min_cost, max_cost])?;
        let mut candidates: Vec<(i64, String, Build, i32)> = vec![];
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let player_id: String = row.get(1)?;
            let bjson: String = row.get(2)?;
            let mmr: i32 = row.get(3)?;
            let build: Build = serde_json::from_str(&bjson)?;
            candidates.push((id, player_id, build, mmr));
        }
        Ok(rng.choice(&candidates).cloned())
    }

    pub fn player_mmr(&self, player_id: &str) -> Result<Option<i32>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT mmr FROM player_profiles WHERE player_id = ?1 AND last_battle_at > 0",
        )?;
        let mut rows = stmt.query([player_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn replay_record(&self, battle_id: i64) -> Result<Option<ReplayRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT b.id,
                    p.player_id,
                    e.player_id,
                    p.build_json,
                    e.build_json,
                    b.player_mmr_before,
                    e.mmr_at_snapshot,
                    b.combat_seed,
                    b.version_hash,
                    b.outcome,
                    b.created_at
             FROM battles b
             JOIN opponents p ON p.id = b.player_opponent_id
             JOIN opponents e ON e.id = b.enemy_opponent_id
             WHERE b.id = ?1",
        )?;
        let mut rows = stmt.query([battle_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let player_build_json: String = row.get(3)?;
        let enemy_build_json: String = row.get(4)?;
        let outcome_raw: String = row.get(9)?;
        let Some(outcome) = BattleOutcome::from_str(&outcome_raw) else {
            anyhow::bail!("unknown battle outcome: {outcome_raw}");
        };
        Ok(Some(ReplayRecord {
            id: row.get(0)?,
            player_id: row.get(1)?,
            enemy_player_id: row.get(2)?,
            player_build: serde_json::from_str(&player_build_json)?,
            enemy_build: serde_json::from_str(&enemy_build_json)?,
            player_mmr_before: row.get(5)?,
            enemy_mmr_before: row.get(6)?,
            combat_seed: row.get(7)?,
            version_hash: row.get(8)?,
            outcome,
            created_at: row.get(10)?,
        }))
    }

    /// Paginated replay listing. When `filter_player_id` is `Some`, only returns
    /// battles that player appeared in (on either side); otherwise returns every
    /// battle (the firehose). For filtered queries we normalize each row so the
    /// filtered player is always the "player" side and the outcome is from their
    /// perspective — the caller can render `mine` rows uniformly without
    /// reasoning about which slot they were in.
    pub fn replays_list(
        &self,
        filter_player_id: Option<&str>,
        page: usize,
        per_page: usize,
    ) -> Result<(Vec<ReplaySummary>, usize)> {
        let conn = self.conn.lock();
        let filter = filter_player_id.unwrap_or("");
        let total: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM battles b
             JOIN opponents p ON p.id = b.player_opponent_id
             JOIN opponents e ON e.id = b.enemy_opponent_id
             WHERE ?1 = '' OR p.player_id = ?1 OR e.player_id = ?1",
            params![filter],
            |r| r.get(0),
        )?;
        let page_count = if total == 0 {
            1
        } else {
            ((total as usize) + per_page - 1) / per_page
        };
        let page = page.clamp(1, page_count);
        let offset = (page - 1) * per_page;
        let mut stmt = conn.prepare(
            "SELECT b.id,
                    p.player_id, COALESCE(pp.name, 'anon'), COALESCE(pp.selected_avatar, 'meme_man'),
                    e.player_id, COALESCE(ep.name, 'anon'), COALESCE(ep.selected_avatar, 'meme_man'),
                    b.player_mmr_before, e.mmr_at_snapshot,
                    b.outcome, b.created_at
             FROM battles b
             JOIN opponents p ON p.id = b.player_opponent_id
             JOIN opponents e ON e.id = b.enemy_opponent_id
             LEFT JOIN player_profiles pp ON pp.player_id = p.player_id
             LEFT JOIN player_profiles ep ON ep.player_id = e.player_id
             WHERE ?1 = '' OR p.player_id = ?1 OR e.player_id = ?1
             ORDER BY b.id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![filter, per_page as i64, offset as i64], |r| {
            let id: i64 = r.get(0)?;
            let p_id: String = r.get(1)?;
            let p_name: String = r.get(2)?;
            let p_avatar: String = r.get(3)?;
            let e_id: String = r.get(4)?;
            let e_name: String = r.get(5)?;
            let e_avatar: String = r.get(6)?;
            let p_mmr: i32 = r.get(7)?;
            let e_mmr: i32 = r.get(8)?;
            let outcome_raw: String = r.get(9)?;
            let created_at: i64 = r.get(10)?;
            Ok((
                id, p_id, p_name, p_avatar, e_id, e_name, e_avatar, p_mmr, e_mmr, outcome_raw,
                created_at,
            ))
        })?;
        let mut out: Vec<ReplaySummary> = Vec::new();
        for row in rows {
            let (id, p_id, p_name, p_avatar, e_id, e_name, e_avatar, p_mmr, e_mmr, raw, created_at) =
                match row {
                    Ok(r) => r,
                    Err(_) => continue,
                };
            let outcome = match BattleOutcome::from_str(&raw) {
                Some(o) => o,
                None => continue,
            };
            // Normalize perspective: if we're filtering for a player and they're
            // on the enemy slot of this row, swap sides and flip the outcome so
            // the player is always shown on the "player" side.
            let flip = !filter.is_empty() && e_id == filter && p_id != filter;
            let summary = if flip {
                ReplaySummary {
                    id,
                    player_id: e_id,
                    player_name: e_name,
                    player_avatar: e_avatar,
                    player_mmr_before: e_mmr,
                    enemy_player_id: p_id,
                    enemy_name: p_name,
                    enemy_avatar: p_avatar,
                    enemy_mmr_before: p_mmr,
                    outcome: match outcome {
                        BattleOutcome::Win => BattleOutcome::Loss,
                        BattleOutcome::Loss => BattleOutcome::Win,
                        BattleOutcome::Draw => BattleOutcome::Draw,
                    },
                    created_at,
                }
            } else {
                ReplaySummary {
                    id,
                    player_id: p_id,
                    player_name: p_name,
                    player_avatar: p_avatar,
                    player_mmr_before: p_mmr,
                    enemy_player_id: e_id,
                    enemy_name: e_name,
                    enemy_avatar: e_avatar,
                    enemy_mmr_before: e_mmr,
                    outcome,
                    created_at,
                }
            };
            out.push(summary);
        }
        Ok((out, page_count))
    }

    #[cfg(test)]
    pub fn record_score(
        &self,
        player_id: &str,
        name: &str,
        streak: i32,
        wins: i32,
        mmr: i32,
    ) -> Result<()> {
        let conn = self.conn.lock();
        record_profile_progress_on(&conn, player_id, name, wins, streak, mmr, true, false)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn insert_opponent(&self, run: &Run, defs: &DefsTable) -> Result<Option<i64>> {
        let conn = self.conn.lock();
        insert_opponent_on(&conn, run, defs)
    }

    /// Paginated leaderboard view. Only includes players who have battled
    /// (`last_battle_at > 0`); a fresh profile row with default MMR doesn't appear
    /// until its first `record_battle_and_save_state` call.
    pub fn leaderboard(
        &self,
        page: usize,
        per_page: usize,
    ) -> Result<(Vec<(String, String, i32, i32, String)>, usize)> {
        let conn = self.conn.lock();
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM player_profiles WHERE last_battle_at > 0",
            [],
            |r| r.get(0),
        )?;
        let page_count = if total == 0 {
            1
        } else {
            ((total as usize) + per_page - 1) / per_page
        };
        let page = page.clamp(1, page_count);
        let offset = (page - 1) * per_page;
        let mut stmt = conn.prepare(
            "SELECT player_id, name, best_wins, mmr, selected_avatar
             FROM player_profiles
             WHERE last_battle_at > 0
             ORDER BY mmr DESC, best_wins DESC, best_streak DESC, last_battle_at ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![per_page as i64, offset as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        Ok((rows.filter_map(|r| r.ok()).collect(), page_count))
    }

    /// 1-based rank and MMR of a player on the leaderboard, if they have an entry.
    /// A profile with `last_battle_at = 0` (never battled) is treated as not ranked.
    pub fn player_rank(&self, player_id: &str) -> Result<Option<(usize, i32)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT mmr, best_wins, best_streak, last_battle_at
             FROM player_profiles
             WHERE player_id = ?1 AND last_battle_at > 0",
        )?;
        let mut rows = stmt.query([player_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let mmr: i32 = row.get(0)?;
        let wins: i32 = row.get(1)?;
        let streak: i32 = row.get(2)?;
        let updated_at: i64 = row.get(3)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM player_profiles
             WHERE last_battle_at > 0 AND (
                 mmr > ?1
                 OR (mmr = ?1 AND best_wins > ?2)
                 OR (mmr = ?1 AND best_wins = ?2 AND best_streak > ?3)
                 OR (mmr = ?1 AND best_wins = ?2 AND best_streak = ?3 AND last_battle_at < ?4)
             )",
            params![mmr, wins, streak, updated_at],
            |r| r.get(0),
        )?;
        Ok(Some((count as usize + 1, mmr)))
    }
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing? == column {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn clean_profile_name(name: &str) -> String {
    let name: String = name.trim().chars().take(24).collect();
    if name.is_empty() {
        "anon".to_string()
    } else {
        name
    }
}

fn load_player_profile_on(conn: &Connection, player_id: &str) -> Result<PlayerProfile> {
    conn.query_row(
        "SELECT player_id,name,selected_avatar,best_wins,ultimate_victories
         FROM player_profiles
         WHERE player_id = ?1",
        [player_id],
        |row| {
            Ok(PlayerProfile {
                player_id: row.get(0)?,
                name: row.get(1)?,
                selected_avatar: row.get(2)?,
                best_wins: row.get(3)?,
                ultimate_victories: row.get(4)?,
            })
        },
    )
    .map_err(Into::into)
}

fn ensure_player_profile_on(
    conn: &Connection,
    player_id: &str,
    name: &str,
) -> Result<PlayerProfile> {
    let name = clean_profile_name(name);
    conn.execute(
        "INSERT OR IGNORE INTO player_profiles(player_id,name,selected_avatar,best_wins,ultimate_victories)
         VALUES(?1,?2,'meme_man',0,0)",
        params![player_id, name],
    )?;
    load_player_profile_on(conn, player_id)
}

/// Post-battle update of the player's profile/ranking row. Folds in best-wins / best-streak
/// growth, optionally swaps in the new MMR (skipped for synthetic AI battles), bumps
/// `last_battle_at` so the player is included in the leaderboard, and increments the
/// ultimate-victory counter on a tournament win. This is the sole writer to the ranking
/// columns — the leaderboard query is just an ORDER BY over what we set here.
fn record_profile_progress_on(
    conn: &Connection,
    player_id: &str,
    name: &str,
    wins: i32,
    best_streak: i32,
    mmr: i32,
    update_mmr: bool,
    completed_ultimate_victory: bool,
) -> Result<()> {
    ensure_player_profile_on(conn, player_id, name)?;
    conn.execute(
        "UPDATE player_profiles
         SET name = ?2,
             best_wins = MAX(best_wins, ?3),
             best_streak = MAX(best_streak, ?4),
             mmr = CASE WHEN ?6 THEN ?5 ELSE mmr END,
             last_battle_at = strftime('%s','now'),
             ultimate_victories = ultimate_victories + ?7
         WHERE player_id = ?1",
        params![
            player_id,
            clean_profile_name(name),
            wins,
            best_streak,
            mmr,
            update_mmr,
            if completed_ultimate_victory { 1 } else { 0 },
        ],
    )?;
    Ok(())
}

/// Walk every `opponents` row and refresh `cost_value` against current item/character defs.
/// Run once at startup so price changes in code propagate to the matchmaking index.
fn recompute_opponent_costs(conn: &Connection, defs: &DefsTable) -> Result<()> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM opponents", [], |r| r.get(0))?;
    tracing::info!("recompute_opponent_costs: starting; opponents={}", total);

    let mut updates: Vec<(i64, i32, i32)> = vec![];
    let mut skipped: usize = 0;
    {
        let mut select = conn.prepare("SELECT id, build_json, cost_value FROM opponents")?;
        let mut rows = select.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let bjson: String = row.get(1)?;
            let old_cost: i32 = row.get(2)?;
            match serde_json::from_str::<Build>(&bjson) {
                Ok(build) => {
                    let new_cost = build.cost_value(defs);
                    if new_cost != old_cost {
                        updates.push((id, old_cost, new_cost));
                    }
                }
                Err(e) => {
                    skipped += 1;
                    tracing::warn!("recompute_opponent_costs: skip opponent {} ({})", id, e);
                }
            }
        }
    }

    let mut update = conn.prepare("UPDATE opponents SET cost_value = ?1 WHERE id = ?2")?;
    for (id, old_cost, new_cost) in &updates {
        update.execute(params![new_cost, id])?;
        tracing::debug!(
            "recompute_opponent_costs: opponent {} {} -> {}",
            id,
            old_cost,
            new_cost
        );
    }
    tracing::info!(
        "recompute_opponent_costs: done; updated={} skipped={}",
        updates.len(),
        skipped,
    );
    // Cost-0 rows are naturally excluded from matchmaking by the band filter (min_cost >= 1);
    // we leave them in place because `battles` may FK-reference them for replay history.
    Ok(())
}

fn upsert_player_state_on(conn: &Connection, run: &Run) -> Result<()> {
    conn.execute(
        "INSERT INTO player_state(player_id, run_id, name, money, wins, losses, streak, best_streak,
                                  mmr, alive, phase, build_json, shop_json, updated_at)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,strftime('%s','now'))
         ON CONFLICT(player_id) DO UPDATE SET
           run_id=excluded.run_id, name=excluded.name, money=excluded.money,
           wins=excluded.wins, losses=excluded.losses, streak=excluded.streak,
           best_streak=excluded.best_streak, mmr=excluded.mmr, alive=excluded.alive,
           phase=excluded.phase, build_json=excluded.build_json, shop_json=excluded.shop_json,
           updated_at=excluded.updated_at",
        params![
            run.player_id, run.id, run.name,
            run.money, run.wins, run.losses,
            run.streak, run.best_streak, run.mmr,
            if run.alive { 1 } else { 0 },
            serde_json::to_string(&run.phase)?,
            serde_json::to_string(&run.build)?,
            serde_json::to_string(&run.shop)?,
        ],
    )?;
    Ok(())
}

fn load_player_state_on(conn: &Connection, player_id: &str) -> Result<Option<Run>> {
    let mut stmt = conn.prepare(
        "SELECT run_id, player_id, name, money, wins, losses, streak, best_streak, mmr, alive,
                phase, build_json, shop_json
         FROM player_state
         WHERE player_id = ?1",
    )?;
    let mut rows = stmt.query([player_id])?;
    if let Some(row) = rows.next()? {
        let phase: String = row.get(10)?;
        let build_json: String = row.get(11)?;
        let shop_json: String = row.get(12)?;
        Ok(Some(Run {
            id: row.get(0)?,
            player_id: row.get(1)?,
            name: row.get(2)?,
            money: row.get(3)?,
            wins: row.get(4)?,
            losses: row.get(5)?,
            streak: row.get(6)?,
            best_streak: row.get(7)?,
            mmr: row.get(8)?,
            alive: row.get::<_, i32>(9)? != 0,
            phase: serde_json::from_str(&phase)?,
            build: serde_json::from_str(&build_json)?,
            shop: serde_json::from_str(&shop_json)?,
        }))
    } else {
        Ok(None)
    }
}

/// Publish the player's post-battle build snapshot into the matchmaking pool.
/// Returns the new opponent row id, or `None` if the build was empty (cost == 0) — empty
/// teams must never be matchable and never participate in replay history.
fn insert_opponent_on(conn: &Connection, run: &Run, defs: &DefsTable) -> Result<Option<i64>> {
    let cost = run.build.cost_value(defs);
    if cost <= 0 {
        return Ok(None);
    }
    conn.execute(
        "INSERT INTO opponents(player_id, cost_value, build_json, mmr_at_snapshot, created_at)
         VALUES(?,?,?,?,strftime('%s','now'))",
        params![
            run.player_id,
            cost,
            serde_json::to_string(&run.build)?,
            run.mmr,
        ],
    )?;
    Ok(Some(conn.last_insert_rowid()))
}

/// Append one row to the replay log. Both opponent ids must reference live `opponents` rows
/// (FK-enforced). Skip for synthetic AI battles — there's no enemy opponent row to point at.
fn insert_battle_on(
    conn: &Connection,
    player_opponent_id: i64,
    enemy_opponent_id: i64,
    combat_seed: u32,
    defs: &DefsTable,
    outcome: BattleOutcome,
    player_mmr_before: i32,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO battles(player_opponent_id, enemy_opponent_id, combat_seed, version_hash,
                             outcome, player_mmr_before, created_at)
         VALUES(?,?,?,?,?,?,strftime('%s','now'))",
        params![
            player_opponent_id,
            enemy_opponent_id,
            combat_seed,
            defs.version().0,
            outcome.as_str(),
            player_mmr_before,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_defs() -> DefsTable {
        Defs::load().current_table()
    }

    fn test_db() -> Db {
        let defs = test_defs();
        Db::open(":memory:", &defs).expect("open memory db")
    }

    fn test_db_path(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("vaporslop-{name}-{}.sqlite", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    fn cleanup_db_path(path: &str) {
        fs::remove_file(path).ok();
        fs::remove_file(format!("{path}-wal")).ok();
        fs::remove_file(format!("{path}-shm")).ok();
    }

    fn run_with_build(player_id: &str, name: &str, build: Build) -> Run {
        Run {
            id: uuid::Uuid::new_v4().to_string(),
            player_id: player_id.to_string(),
            name: name.to_string(),
            money: 0,
            wins: 0,
            losses: 0,
            streak: 0,
            best_streak: 0,
            mmr: STARTING_MMR,
            alive: true,
            build,
            shop: Shop::default(),
            phase: Phase::Shop,
        }
    }

    fn one_member_build(def_id: &str) -> Build {
        Build {
            team: vec![TeamMember {
                def_id: def_id.to_string(),
                hat: None,
                left_hand: None,
                right_hand: None,
                hand_3: None,
                hand_4: None,
            }],
        }
    }

    fn three_member_build() -> Build {
        Build {
            team: vec![
                TeamMember {
                    def_id: "orang".to_string(),
                    hat: None,
                    left_hand: None,
                    right_hand: None,
                    hand_3: None,
                    hand_4: None,
                },
                TeamMember {
                    def_id: "dark_vegetal".to_string(),
                    hat: None,
                    left_hand: None,
                    right_hand: None,
                    hand_3: None,
                    hand_4: None,
                },
                TeamMember {
                    def_id: "meme_man".to_string(),
                    hat: None,
                    left_hand: None,
                    right_hand: None,
                    hand_3: None,
                    hand_4: None,
                },
            ],
        }
    }

    #[test]
    fn find_opponent_excludes_caller_and_their_history() {
        let db = test_db();

        // Caller publishes a snapshot, but searching as them must skip it.
        let mine = run_with_build("player-1", "me", one_member_build("orang"));
        db.insert_opponent(&mine, &test_defs()).unwrap();

        let mut rng = Rng::new(1);
        assert!(db
            .find_opponent("player-1", mine.build.cost_value(&test_defs()), &mut rng)
            .unwrap()
            .is_none());

        let other = run_with_build("player-2", "rival", one_member_build("meme_man"));
        db.insert_opponent(&other, &test_defs()).unwrap();

        let found = db
            .find_opponent("player-1", mine.build.cost_value(&test_defs()), &mut rng)
            .unwrap()
            .unwrap();
        assert_eq!(found.1, "player-2"); // player_id
        assert_eq!(found.3, STARTING_MMR); // mmr_at_snapshot
    }

    #[test]
    fn find_opponent_returns_none_when_pool_is_empty() {
        let db = test_db();
        let mine = run_with_build("player-1", "me", one_member_build("orang"));
        let mut rng = Rng::new(1);
        assert!(db
            .find_opponent("player-1", mine.build.cost_value(&test_defs()), &mut rng)
            .unwrap()
            .is_none());
    }

    #[test]
    fn find_opponent_band_filters_far_costs() {
        let db = test_db();
        let mine = run_with_build("player-1", "me", one_member_build("orang"));
        let far = run_with_build("player-2", "far", three_member_build());
        db.insert_opponent(&far, &test_defs()).unwrap();
        // far's cost is much higher than mine's — out of band.
        let mut rng = Rng::new(1);
        assert!(db
            .find_opponent("player-1", mine.build.cost_value(&test_defs()), &mut rng)
            .unwrap()
            .is_none());
    }

    #[test]
    fn find_opponent_uses_historical_snapshots_not_current_state() {
        // Player rewrites their team in player_state, but old snapshots remain matchable
        // at their original cost.
        let db = test_db();
        let mut hero = run_with_build("hero", "hero", one_member_build("orang"));
        db.insert_opponent(&hero, &test_defs()).unwrap();
        // Player's *current* state changes drastically; this should not affect matchmaking.
        hero.build = three_member_build();
        db.upsert_player_state(&hero).unwrap();

        let target = one_member_build("orang").cost_value(&test_defs());
        let mut rng = Rng::new(1);
        let found = db.find_opponent("seeker", target, &mut rng).unwrap();
        assert!(found.is_some(), "expected to match the historical snapshot");
    }

    #[test]
    fn shop_action_does_not_publish_to_opponents() {
        // Saving player_state must never write to `opponents`.
        let db = test_db();
        let run = run_with_build("p1", "p1", three_member_build());
        db.upsert_player_state(&run).unwrap();
        let n: i64 = db
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM opponents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn battle_commit_publishes_opponent_snapshot() {
        // Without a real enemy_opponent_id (synthetic AI path), the commit publishes the
        // player's post-battle snapshot to the pool but writes no replay row.
        let db = test_db();
        let mut run = run_with_build("p1", "p1", one_member_build("orang"));
        run.wins = 1;
        run.best_streak = 1;
        let replay_id = db
            .record_battle_and_save_state(&run, BattleOutcome::Win, true, false, None, 0, run.mmr, &test_defs())
            .unwrap();
        assert_eq!(replay_id, None);
        let conn = db.conn.lock();
        let opponents: i64 = conn
            .query_row("SELECT COUNT(*) FROM opponents", [], |r| r.get(0))
            .unwrap();
        let battles: i64 = conn
            .query_row("SELECT COUNT(*) FROM battles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(opponents, 1);
        assert_eq!(battles, 0, "synthetic battles must not create replay rows");
    }

    #[test]
    fn battle_commit_writes_replay_row_with_enemy() {
        // Real-opponent path: opponents gets the player's snapshot, battles gets a replay
        // row that references both opponent ids.
        let db = test_db();
        let enemy = run_with_build("enemy", "enemy", one_member_build("meme_man"));
        let enemy_opponent_id = db.insert_opponent(&enemy, &test_defs()).unwrap().unwrap();

        let mut me = run_with_build("me", "me", one_member_build("orang"));
        me.wins = 1;
        me.best_streak = 1;
        let combat_seed: u32 = 0xCAFE_BABE;
        let player_mmr_before = me.mmr - 16; // simulate post-battle delta
        let replay_id = db
            .record_battle_and_save_state(
                &me,
                BattleOutcome::Win,
                true,
                false,
                Some(enemy_opponent_id),
                combat_seed,
                player_mmr_before,
                &test_defs(),
            )
            .unwrap()
            .expect("real opponent battle should create replay row");

        let (seed, outcome, mmr_before, version, p_op, e_op): (u32, String, i32, u32, i64, i64) = {
            let conn = db.conn.lock();
            conn.query_row(
                "SELECT combat_seed, outcome, player_mmr_before, version_hash,
                        player_opponent_id, enemy_opponent_id
                 FROM battles",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(seed, combat_seed);
        assert_eq!(outcome, "win");
        assert_eq!(mmr_before, player_mmr_before);
        let expect_v = Defs::load().current_version().0;
        assert_eq!(version, expect_v);
        assert_eq!(e_op, enemy_opponent_id);
        assert_ne!(p_op, enemy_opponent_id); // player's snapshot is a fresh row

        let replay = db.replay_record(replay_id).unwrap().expect("replay must load");
        assert_eq!(replay.id, replay_id);
        assert_eq!(replay.player_id, "me");
        assert_eq!(replay.enemy_player_id, "enemy");
        assert_eq!(replay.player_mmr_before, player_mmr_before);
        assert_eq!(replay.enemy_mmr_before, enemy.mmr);
        assert_eq!(replay.combat_seed, combat_seed);
        assert_eq!(replay.version_hash, Defs::load().current_version().0);
        assert_eq!(replay.outcome.as_str(), "win");
        assert_eq!(replay.player_build.team.len(), 1);
        assert_eq!(replay.enemy_build.team.len(), 1);
    }

    #[test]
    fn leaderboard_keeps_each_players_best_score() {
        let db = test_db();

        // Same player records two runs; best_wins takes the max, name updates to latest.
        db.record_score("player-1", "old name", 2, 4, 1000).unwrap();
        db.record_score("player-1", "new name", 1, 3, 1000).unwrap();
        db.record_score("player-2", "winner", 3, 5, 1100).unwrap();

        let (entries, pages) = db.leaderboard(1, 1).unwrap();
        assert_eq!(pages, 2);
        assert_eq!(
            entries,
            vec![(
                "player-2".to_string(),
                "winner".to_string(),
                5,
                1100,
                "meme_man".to_string()
            )]
        );

        let (entries, _) = db.leaderboard(2, 1).unwrap();
        assert_eq!(
            entries,
            vec![(
                "player-1".to_string(),
                "new name".to_string(),
                4,
                1000,
                "meme_man".to_string()
            )]
        );
    }

    #[test]
    fn leaderboard_orders_by_mmr_before_score() {
        let db = test_db();

        db.record_score("player-1", "low mmr", 5, 10, 900).unwrap();
        db.record_score("player-2", "high mmr", 1, 1, 1200).unwrap();

        let (entries, _) = db.leaderboard(1, 10).unwrap();
        assert_eq!(
            entries,
            vec![
                (
                    "player-2".to_string(),
                    "high mmr".to_string(),
                    1,
                    1200,
                    "meme_man".to_string()
                ),
                (
                    "player-1".to_string(),
                    "low mmr".to_string(),
                    10,
                    900,
                    "meme_man".to_string()
                ),
            ]
        );
    }

    #[test]
    fn player_profile_defaults_and_updates() {
        let db = test_db();
        let profile = db.ensure_player_profile("player-1", "  vapor  ").unwrap();
        assert_eq!(profile.player_id, "player-1");
        assert_eq!(profile.name, "vapor");
        assert_eq!(profile.selected_avatar, "meme_man");
        assert_eq!(profile.best_wins, 0);
        assert_eq!(profile.ultimate_victories, 0);

        let profile = db
            .update_player_profile("player-1", "new vapor", "orang")
            .unwrap();
        assert_eq!(profile.name, "new vapor");
        assert_eq!(profile.selected_avatar, "orang");
    }

    #[test]
    fn profile_name_backfill_only_replaces_placeholder_name() {
        let db = test_db();
        db.ensure_player_profile("player-1", "anon").unwrap();

        let profile = db.backfill_profile_name("player-1", "old handle").unwrap();
        assert_eq!(profile.name, "old handle");

        let profile = db.backfill_profile_name("player-1", "new handle").unwrap();
        assert_eq!(profile.name, "old handle");
    }

    #[test]
    fn profile_progress_tracks_best_wins_and_ultimate_victories() {
        let db = test_db();
        let mut run = run_with_build("player-1", "winner", one_member_build("orang"));
        run.wins = 12;

        db.record_battle_and_save_state(&run, BattleOutcome::Win, true, false, None, 0, run.mmr, &test_defs())
            .unwrap();
        let profile = db.ensure_player_profile("player-1", "winner").unwrap();
        assert_eq!(profile.best_wins, 12);
        assert_eq!(profile.ultimate_victories, 0);

        run.wins = MAX_WINS;
        db.record_battle_and_save_state(&run, BattleOutcome::Win, true, true, None, 0, run.mmr, &test_defs())
            .unwrap();
        let profile = db.ensure_player_profile("player-1", "winner").unwrap();
        assert_eq!(profile.best_wins, MAX_WINS);
        assert_eq!(profile.ultimate_victories, 1);
    }

    #[test]
    fn player_state_round_trips_through_file_backed_db() {
        let path = test_db_path("round-trip");
        let mut run = run_with_build("player-1", "player", three_member_build());
        run.money = 250;
        run.wins = 2;
        run.mmr = 1234;
        run.shop = Shop {
            characters: vec![Some("orang".to_string()), None],
            items: vec![Some("bread".to_string())],
        };

        {
            let table = test_defs();
            let db = Db::open(&path, &table).unwrap();
            db.upsert_player_state(&run).unwrap();
        }

        let table = test_defs();
        let db = Db::open(&path, &table).unwrap();
        let loaded = db.load_player_state("player-1").unwrap().unwrap();
        assert_eq!(loaded.id, run.id);
        assert_eq!(loaded.player_id, "player-1");
        assert_eq!(loaded.money, 250);
        assert_eq!(loaded.wins, 2);
        assert_eq!(loaded.mmr, 1234);
        assert_eq!(loaded.phase, Phase::Shop);
        assert_eq!(loaded.build.team.len(), 3);
        drop(db);
        cleanup_db_path(&path);
    }

    #[test]
    #[ignore = "run on demand against a copy of the production DB"]
    fn v6_migration_against_production_copy() {
        // Set MIGRATION_DB to a copy of vaporslop.sqlite. The test asserts the post-open
        // state is what v6 expects: user_version=6, opponents has rows preserved, battles
        // (replay log) exists and is empty.
        let path = std::env::var("MIGRATION_DB").expect("set MIGRATION_DB");
        let table = Defs::load().current_table();
        let db = Db::open(&path, &table).unwrap();
        let conn = db.conn.lock();

        let ver: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, 6, "expected migration to reach v6");

        // New battles (replay log) starts empty regardless of pre-migration version.
        let battles_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM battles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(battles_count, 0, "new battles (replay log) starts empty");

        // Opponents pool is populated either from a pre-existing v4-shape `battles` (paths
        // v4/v5 → v6) or from `runs` via the v4 step itself (path v0..=v3 → v6). Just
        // sanity-check that something matchable ended up there if the source had non-empty
        // builds.
        let opponents_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM opponents", [], |r| r.get(0))
            .unwrap();
        eprintln!("post-migration: opponents={}", opponents_count);

        // Confirm the new opponents schema — failure here means a column survived that shouldn't.
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(opponents)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(
            cols,
            vec![
                "id".to_string(),
                "player_id".to_string(),
                "cost_value".to_string(),
                "build_json".to_string(),
                "mmr_at_snapshot".to_string(),
                "created_at".to_string(),
            ]
        );

        // And the new battles schema.
        let battle_cols: Vec<String> = conn
            .prepare("PRAGMA table_info(battles)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(
            battle_cols,
            vec![
                "id".to_string(),
                "player_opponent_id".to_string(),
                "enemy_opponent_id".to_string(),
                "combat_seed".to_string(),
                "version_hash".to_string(),
                "outcome".to_string(),
                "player_mmr_before".to_string(),
                "created_at".to_string(),
            ]
        );
    }

    #[test]
    fn second_run_overwrites_first_for_same_player() {
        let db = test_db();
        let first = run_with_build("p1", "p1", one_member_build("orang"));
        db.upsert_player_state(&first).unwrap();
        let mut second = run_with_build("p1", "p1", three_member_build());
        second.id = uuid::Uuid::new_v4().to_string();
        db.upsert_player_state(&second).unwrap();

        let loaded = db.load_player_state("p1").unwrap().unwrap();
        assert_eq!(loaded.id, second.id);
        assert_eq!(loaded.build.team.len(), 3);
    }

    #[test]
    fn battle_commit_persists_state_and_writes_leaderboard_and_opponents() {
        let db = test_db();
        let mut run = run_with_build("player-1", "winner", one_member_build("orang"));
        run.wins = 1;
        run.streak = 1;
        run.best_streak = 1;
        run.mmr = 1016;
        run.money = 200;
        run.phase = Phase::Shop;

        db.record_battle_and_save_state(&run, BattleOutcome::Win, true, false, None, 0, run.mmr, &test_defs())
            .unwrap();

        let loaded = db.load_player_state("player-1").unwrap().unwrap();
        assert_eq!(loaded.phase, Phase::Shop);
        assert_eq!(loaded.wins, 1);
        assert_eq!(loaded.mmr, 1016);

        let (entries, _) = db.leaderboard(1, 10).unwrap();
        assert_eq!(
            entries,
            vec![(
                "player-1".to_string(),
                "winner".to_string(),
                1,
                1016,
                "meme_man".to_string()
            )]
        );

        let opponents_count: i64 = db
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM opponents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(opponents_count, 1);
    }

    #[test]
    fn battle_commit_can_skip_leaderboard_mmr_update() {
        let db = test_db();
        let mut run = run_with_build("player-1", "winner", one_member_build("orang"));
        run.wins = 1;
        run.best_streak = 1;
        run.mmr = 1100;

        db.record_battle_and_save_state(&run, BattleOutcome::Win, true, false, None, 0, run.mmr, &test_defs())
            .unwrap();
        run.wins = 2;
        run.best_streak = 2;
        run.mmr = 1300;
        db.record_battle_and_save_state(&run, BattleOutcome::Win, false, false, None, 0, run.mmr, &test_defs())
            .unwrap();

        let loaded = db.load_player_state("player-1").unwrap().unwrap();
        assert_eq!(loaded.mmr, 1300);

        let (entries, _) = db.leaderboard(1, 10).unwrap();
        assert_eq!(
            entries,
            vec![(
                "player-1".to_string(),
                "winner".to_string(),
                2,
                1100,
                "meme_man".to_string()
            )]
        );
    }

    #[test]
    fn opponent_pool_reads_at_current_version() {
        let path = test_db_path("opp-cost-bump");
        let base = test_defs();
        let (mut units, items) = base.clone_maps();

        let orang_cheap = units.get("orang").expect("orang fixture").clone();
        let mut orang_pricier = orang_cheap.clone();
        orang_pricier.cost += 801;

        units.insert("orang".into(), orang_cheap.clone());
        let cheap_table = DefsTable::from_maps(base.version(), units.clone(), items.clone());
        let run = run_with_build("seller", "seller", one_member_build("orang"));
        let opp_id = {
            let db = Db::open(&path, &cheap_table).unwrap();
            db.insert_opponent(&run, &cheap_table).unwrap().unwrap()
        };
        let cost_at_write = run.build.cost_value(&cheap_table);

        units.insert("orang".into(), orang_pricier);
        let pricy_table = DefsTable::from_maps(base.version(), units, items);

        drop(Db::open(&path, &pricy_table).unwrap());

        let db = Db::open(&path, &pricy_table).unwrap();
        let refreshed: i32 = db
            .conn
            .lock()
            .query_row(
                "SELECT cost_value FROM opponents WHERE id=?1",
                [opp_id],
                |row| row.get(0),
            )
            .unwrap();
        drop(db);

        assert_eq!(refreshed, run.build.cost_value(&pricy_table));
        assert_ne!(refreshed, cost_at_write);
        cleanup_db_path(&path);
    }
}
