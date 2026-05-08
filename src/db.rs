use crate::game::types::*;
use anyhow::Result;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
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

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA busy_timeout = 5000;
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
        "#,
        )?;
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
            CREATE TABLE IF NOT EXISTS leaderboard (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                streak INTEGER NOT NULL,
                wins INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
        "#,
        )?;
        ensure_column(&conn, "runs", "player_id", "TEXT")?;
        ensure_column(&conn, "runs", "best_streak", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_column(&conn, "runs", "mmr", "INTEGER NOT NULL DEFAULT 1000")?;
        ensure_column(&conn, "leaderboard", "player_id", "TEXT")?;
        ensure_column(&conn, "leaderboard", "updated_at", "INTEGER")?;
        ensure_column(&conn, "leaderboard", "mmr", "INTEGER NOT NULL DEFAULT 1000")?;
        conn.execute(
            "UPDATE runs SET player_id = id WHERE player_id IS NULL OR player_id = ''",
            [],
        )?;
        conn.execute(
            "UPDATE runs SET best_streak = streak WHERE best_streak < streak",
            [],
        )?;
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
        conn.execute(
            "UPDATE runs SET phase = ?1 WHERE phase = ?2",
            params![
                serde_json::to_string(&Phase::Shop)?,
                serde_json::to_string(&Phase::Battle)?,
            ],
        )?;
        let db_ver: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
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
        recompute_cost_values(&conn)?;
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
            "UPDATE runs SET name = ?2 WHERE player_id = ?1",
            params![player_id, name],
        )?;
        conn.execute(
            "UPDATE leaderboard SET name = ?2 WHERE player_id = ?1",
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

    pub fn upsert_run(&self, run: &Run, cost: i32) -> Result<()> {
        let conn = self.conn.lock();
        upsert_run_on(&conn, run, cost)?;
        Ok(())
    }

    pub fn load_run(&self, id: &str) -> Result<Option<Run>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,player_id,name,money,wins,losses,streak,best_streak,mmr,alive,phase,build_json,shop_json FROM runs WHERE id=?")?;
        let mut rows = stmt.query([id])?;
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

    pub fn load_latest_run_for_player(&self, player_id: &str) -> Result<Option<Run>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id,player_id,name,money,wins,losses,streak,best_streak,mmr,alive,phase,build_json,shop_json
             FROM runs
             WHERE player_id = ?1
             ORDER BY updated_at DESC
             LIMIT 1",
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

    /// Find another player's build whose cost is near the requested gold budget. No bot rows exist in the DB.
    /// Allowed opponent cost: equal to or up to 15% below `target_cost` (ceiled, at least 1 gold of slack).
    pub fn find_opponent(
        &self,
        current_run_id: &str,
        current_player_id: &str,
        target_cost: i32,
    ) -> Result<Option<(String, String, String, Build, i32)>> {
        let conn = self.conn.lock();
        let down = ((target_cost as f32) * 0.15).ceil().max(1.0) as i32;
        let min_cost = (target_cost - down).max(1);
        let max_cost = target_cost;
        let mut stmt = conn.prepare(
            "SELECT id,player_id,name,build_json,mmr FROM runs
             WHERE id != ?1
               AND player_id != ?2
               AND cost_value BETWEEN ?3 AND ?4",
        )?;
        let mut rows = stmt.query(params![
            current_run_id,
            current_player_id,
            min_cost,
            max_cost
        ])?;
        let mut candidates: Vec<(String, String, String, Build, i32)> = vec![];
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let player_id: String = row.get(1)?;
            let name: String = row.get(2)?;
            let bjson: String = row.get(3)?;
            let mmr: i32 = row.get(4)?;
            let build: Build = serde_json::from_str(&bjson)?;
            candidates.push((id, player_id, name, build, mmr));
        }
        let mut rng = rand::thread_rng();
        if let Some(candidate) = candidates.choose(&mut rng).cloned() {
            return Ok(Some(candidate));
        }

        Ok(None)
    }

    pub fn player_mmr(&self, player_id: &str) -> Result<Option<i32>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT mmr FROM leaderboard WHERE player_id = ?1")?;
        let mut rows = stmt.query([player_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
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
        record_score_on(&conn, player_id, name, streak, wins, mmr, true)?;
        Ok(())
    }

    pub fn record_score_and_upsert_run(
        &self,
        run: &Run,
        cost: i32,
        update_mmr: bool,
        completed_ultimate_victory: bool,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        record_profile_progress_on(
            &tx,
            &run.player_id,
            &run.name,
            run.wins,
            completed_ultimate_victory,
        )?;
        record_score_on(
            &tx,
            &run.player_id,
            &run.name,
            run.best_streak,
            run.wins,
            run.mmr,
            update_mmr,
        )?;
        upsert_run_on(&tx, run, cost)?;
        tx.commit()?;
        Ok(())
    }

    pub fn leaderboard(
        &self,
        page: usize,
        per_page: usize,
    ) -> Result<(Vec<(String, String, i32, i32, i32, String)>, usize)> {
        let conn = self.conn.lock();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM leaderboard", [], |r| r.get(0))?;
        let page_count = if total == 0 {
            1
        } else {
            ((total as usize) + per_page - 1) / per_page
        };
        let page = page.clamp(1, page_count);
        let offset = (page - 1) * per_page;
        let mut stmt = conn.prepare(
            "SELECT l.player_id,l.name,l.streak,l.wins,l.mmr,COALESCE(p.selected_avatar,'meme_man')
             FROM leaderboard l
             LEFT JOIN player_profiles p ON p.player_id = l.player_id
             ORDER BY l.mmr DESC, l.wins DESC, l.streak DESC, l.updated_at ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![per_page as i64, offset as i64], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
        Ok((rows.filter_map(|r| r.ok()).collect(), page_count))
    }

    /// 1-based rank of a player on the leaderboard, if they have an entry.
    pub fn player_rank(&self, player_id: &str) -> Result<Option<usize>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT mmr,wins,streak,updated_at FROM leaderboard WHERE player_id = ?1")?;
        let mut rows = stmt.query([player_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let mmr: i32 = row.get(0)?;
        let wins: i32 = row.get(1)?;
        let streak: i32 = row.get(2)?;
        let updated_at: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM leaderboard WHERE
               mmr > ?1
               OR (mmr = ?1 AND wins > ?2)
               OR (mmr = ?1 AND wins = ?2 AND streak > ?3)
               OR (mmr = ?1 AND wins = ?2 AND streak = ?3 AND updated_at < ?4)",
            params![mmr, wins, streak, updated_at],
            |r| r.get(0),
        )?;
        Ok(Some(count as usize + 1))
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

fn record_profile_progress_on(
    conn: &Connection,
    player_id: &str,
    name: &str,
    wins: i32,
    completed_ultimate_victory: bool,
) -> Result<()> {
    ensure_player_profile_on(conn, player_id, name)?;
    conn.execute(
        "UPDATE player_profiles
         SET name = ?2,
             best_wins = MAX(best_wins, ?3),
             ultimate_victories = ultimate_victories + ?4
         WHERE player_id = ?1",
        params![
            player_id,
            clean_profile_name(name),
            wins,
            if completed_ultimate_victory { 1 } else { 0 },
        ],
    )?;
    Ok(())
}

/// Cull empty-team rows and recompute every run's cached cost_value against current item/character defs.
/// Run once at startup so price changes in code propagate to the matchmaking index.
fn recompute_cost_values(conn: &Connection) -> Result<()> {
    let before_total: i64 = conn.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))?;
    let before_empty: i64 = conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE cost_value = 0",
        [],
        |r| r.get(0),
    )?;
    tracing::info!(
        "recompute_cost_values: starting; rows={} empty_rows={}",
        before_total,
        before_empty,
    );
    let culled = conn.execute("DELETE FROM runs WHERE cost_value = 0", [])?;

    let mut updated: usize = 0;
    let mut skipped: usize = 0;
    let mut select = conn.prepare("SELECT id, build_json, cost_value FROM runs")?;
    let mut rows = select.query([])?;
    let mut updates: Vec<(String, i32, i32)> = vec![];
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let bjson: String = row.get(1)?;
        let old_cost: i32 = row.get(2)?;
        match serde_json::from_str::<Build>(&bjson) {
            Ok(build) => {
                let new_cost = build.cost_value();
                if new_cost != old_cost {
                    updates.push((id, old_cost, new_cost));
                }
            }
            Err(e) => {
                skipped += 1;
                tracing::warn!("recompute_cost_values: skip run {} ({})", id, e);
            }
        }
    }
    drop(rows);
    drop(select);

    let mut update = conn.prepare("UPDATE runs SET cost_value = ?1 WHERE id = ?2")?;
    for (id, old_cost, new_cost) in &updates {
        update.execute(params![new_cost, id])?;
        tracing::debug!(
            "recompute_cost_values: run {} {} -> {}",
            id,
            old_cost,
            new_cost
        );
        updated += 1;
    }
    tracing::info!(
        "recompute_cost_values: done; culled_empty={} updated={} skipped={}",
        culled,
        updated,
        skipped,
    );
    Ok(())
}

fn upsert_run_on(conn: &Connection, run: &Run, cost: i32) -> Result<()> {
    conn.execute(
        "INSERT INTO runs(id,player_id,name,money,wins,losses,streak,best_streak,mmr,alive,phase,build_json,shop_json,cost_value,updated_at)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,strftime('%s','now'))
         ON CONFLICT(id) DO UPDATE SET
           player_id=excluded.player_id, name=excluded.name, money=excluded.money,
           wins=excluded.wins, losses=excluded.losses, streak=excluded.streak,
           best_streak=excluded.best_streak, mmr=excluded.mmr, alive=excluded.alive, phase=excluded.phase,
           build_json=excluded.build_json, shop_json=excluded.shop_json,
           cost_value=excluded.cost_value, updated_at=excluded.updated_at",
        params![
            run.id, run.player_id, run.name,
            run.money, run.wins, run.losses,
            run.streak, run.best_streak, run.mmr,
            if run.alive { 1 } else { 0 },
            serde_json::to_string(&run.phase)?,
            serde_json::to_string(&run.build)?,
            serde_json::to_string(&run.shop)?,
            cost,
        ],
    )?;
    Ok(())
}

fn record_score_on(
    conn: &Connection,
    player_id: &str,
    name: &str,
    streak: i32,
    wins: i32,
    mmr: i32,
    update_mmr: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO leaderboard(player_id,name,streak,wins,mmr,created_at,updated_at)
         VALUES(?,?,?,?,?,strftime('%s','now'),strftime('%s','now'))
         ON CONFLICT(player_id) DO UPDATE SET
           name=excluded.name,
           streak=CASE
             WHEN excluded.wins > leaderboard.wins THEN excluded.streak
             WHEN excluded.wins = leaderboard.wins AND excluded.streak > leaderboard.streak THEN excluded.streak
             ELSE leaderboard.streak
           END,
           wins=MAX(leaderboard.wins, excluded.wins),
           mmr=CASE
             WHEN ?6 THEN excluded.mmr
             ELSE leaderboard.mmr
           END,
           updated_at=strftime('%s','now')",
        params![player_id, name, streak, wins, mmr, update_mmr],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_db() -> Db {
        Db::open(":memory:").unwrap()
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

    fn run_with_build(id: &str, name: &str, build: Build) -> Run {
        Run {
            id: id.to_string(),
            player_id: id.to_string(),
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

    fn set_updated_at(db: &Db, id: &str, updated_at: i64) {
        db.conn
            .lock()
            .execute(
                "UPDATE runs SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![updated_at, id],
            )
            .unwrap();
    }

    fn one_member_build(def_id: &str) -> Build {
        Build {
            team: vec![TeamMember {
                def_id: def_id.to_string(),
                hat: None,
                left_hand: None,
                right_hand: None,
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
                },
                TeamMember {
                    def_id: "dark_vegetal".to_string(),
                    hat: None,
                    left_hand: None,
                    right_hand: None,
                },
                TeamMember {
                    def_id: "meme_man".to_string(),
                    hat: None,
                    left_hand: None,
                    right_hand: None,
                },
            ],
        }
    }

    #[test]
    fn find_opponent_excludes_current_run() {
        let db = test_db();
        db.conn.lock().execute("DELETE FROM runs", []).unwrap();

        let current = run_with_build("current", "current", one_member_build("orang"));
        db.upsert_run(&current, current.build.cost_value()).unwrap();

        assert!(db
            .find_opponent(&current.id, &current.player_id, current.build.cost_value())
            .unwrap()
            .is_none());

        let opponent = run_with_build("opponent", "opponent", one_member_build("meme_man"));
        db.upsert_run(&opponent, opponent.build.cost_value())
            .unwrap();
        let far = run_with_build("far", "far", one_member_build("elephoont"));
        db.upsert_run(&far, far.build.cost_value()).unwrap();

        let found = db
            .find_opponent(&current.id, &current.player_id, current.build.cost_value())
            .unwrap()
            .unwrap();
        assert_eq!(found.0, "opponent");
        assert_eq!(found.4, STARTING_MMR);
    }

    #[test]
    fn find_opponent_ignores_own_prior_runs_and_far_builds() {
        let db = test_db();
        db.conn.lock().execute("DELETE FROM runs", []).unwrap();

        let mut current = run_with_build("current", "current", one_member_build("orang"));
        current.player_id = "player-1".to_string();
        db.upsert_run(&current, current.build.cost_value()).unwrap();

        let mut prior = run_with_build("prior", "prior", three_member_build());
        prior.player_id = current.player_id.clone();
        db.upsert_run(&prior, prior.build.cost_value()).unwrap();

        let far = run_with_build("far", "far", three_member_build());
        db.upsert_run(&far, far.build.cost_value()).unwrap();

        assert!(db
            .find_opponent(&current.id, &current.player_id, current.build.cost_value())
            .unwrap()
            .is_none());
    }

    #[test]
    fn leaderboard_keeps_each_players_best_score() {
        let db = test_db();
        db.conn
            .lock()
            .execute("DELETE FROM leaderboard", [])
            .unwrap();

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
                3,
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
                2,
                4,
                1000,
                "meme_man".to_string()
            )]
        );
    }

    #[test]
    fn leaderboard_orders_by_mmr_before_score() {
        let db = test_db();
        db.conn
            .lock()
            .execute("DELETE FROM leaderboard", [])
            .unwrap();

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
                    1,
                    1200,
                    "meme_man".to_string()
                ),
                (
                    "player-1".to_string(),
                    "low mmr".to_string(),
                    5,
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
        let mut run = run_with_build("winner", "winner", one_member_build("orang"));
        run.player_id = "player-1".to_string();
        run.wins = 12;

        db.record_score_and_upsert_run(&run, run.build.cost_value(), true, false)
            .unwrap();
        let profile = db.ensure_player_profile("player-1", "winner").unwrap();
        assert_eq!(profile.best_wins, 12);
        assert_eq!(profile.ultimate_victories, 0);

        run.wins = MAX_WINS;
        db.record_score_and_upsert_run(&run, run.build.cost_value(), true, true)
            .unwrap();
        let profile = db.ensure_player_profile("player-1", "winner").unwrap();
        assert_eq!(profile.best_wins, MAX_WINS);
        assert_eq!(profile.ultimate_victories, 1);
    }

    #[test]
    fn run_round_trips_through_file_backed_db() {
        let path = test_db_path("round-trip");
        let mut run = run_with_build("run-1", "player", three_member_build());
        run.player_id = "player-1".to_string();
        run.money = 250;
        run.wins = 2;
        run.mmr = 1234;
        run.shop = Shop {
            characters: vec![Some("orang".to_string()), None],
            items: vec![Some("sword".to_string())],
        };

        {
            let db = Db::open(&path).unwrap();
            db.upsert_run(&run, run.build.cost_value()).unwrap();
        }

        let db = Db::open(&path).unwrap();
        let loaded = db.load_run("run-1").unwrap().unwrap();
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
    fn load_latest_run_for_player_returns_latest_game_over_run() {
        let db = test_db();
        let mut active = run_with_build("active", "player", one_member_build("meme_man"));
        active.player_id = "player-1".to_string();
        db.upsert_run(&active, active.build.cost_value()).unwrap();
        set_updated_at(&db, "active", 1);

        let mut game_over = run_with_build("old", "player", one_member_build("orang"));
        game_over.player_id = "player-1".to_string();
        game_over.alive = false;
        game_over.phase = Phase::GameOver;
        db.upsert_run(&game_over, game_over.build.cost_value())
            .unwrap();
        set_updated_at(&db, "old", 2);

        let loaded = db.load_latest_run_for_player("player-1").unwrap().unwrap();
        assert_eq!(loaded.id, "old");
    }

    #[test]
    fn load_latest_run_for_player_returns_latest_active_run() {
        let db = test_db();
        let mut game_over = run_with_build("old", "player", one_member_build("orang"));
        game_over.player_id = "player-1".to_string();
        game_over.alive = false;
        game_over.phase = Phase::GameOver;
        db.upsert_run(&game_over, game_over.build.cost_value())
            .unwrap();
        set_updated_at(&db, "old", 1);

        let mut active = run_with_build("active", "player", one_member_build("meme_man"));
        active.player_id = "player-1".to_string();
        db.upsert_run(&active, active.build.cost_value()).unwrap();
        set_updated_at(&db, "active", 2);

        let loaded = db.load_latest_run_for_player("player-1").unwrap().unwrap();
        assert_eq!(loaded.id, "active");
    }

    #[test]
    fn transactional_battle_save_records_score_and_completed_shop_phase() {
        let db = test_db();
        let mut run = run_with_build("winner", "winner", one_member_build("orang"));
        run.player_id = "player-1".to_string();
        run.wins = 1;
        run.streak = 1;
        run.best_streak = 1;
        run.mmr = 1016;
        run.money = 200;
        run.phase = Phase::Shop;

        db.record_score_and_upsert_run(&run, run.build.cost_value(), true, false)
            .unwrap();

        let loaded = db.load_run("winner").unwrap().unwrap();
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
                1,
                1016,
                "meme_man".to_string()
            )]
        );
    }

    #[test]
    fn transactional_battle_save_can_skip_leaderboard_mmr_update() {
        let db = test_db();
        let mut run = run_with_build("winner", "winner", one_member_build("orang"));
        run.player_id = "player-1".to_string();
        run.wins = 1;
        run.best_streak = 1;
        run.mmr = 1100;

        db.record_score_and_upsert_run(&run, run.build.cost_value(), true, false)
            .unwrap();
        run.wins = 2;
        run.best_streak = 2;
        run.mmr = 1300;
        db.record_score_and_upsert_run(&run, run.build.cost_value(), false, false)
            .unwrap();

        let loaded = db.load_run("winner").unwrap().unwrap();
        assert_eq!(loaded.mmr, 1300);

        let (entries, _) = db.leaderboard(1, 10).unwrap();
        assert_eq!(
            entries,
            vec![(
                "player-1".to_string(),
                "winner".to_string(),
                2,
                2,
                1100,
                "meme_man".to_string()
            )]
        );
    }

    #[test]
    fn legacy_battle_phase_reopens_as_shop() {
        let path = test_db_path("legacy-battle");
        {
            let db = Db::open(&path).unwrap();
            let mut run = run_with_build("legacy", "player", one_member_build("orang"));
            run.phase = Phase::Battle;
            db.upsert_run(&run, run.build.cost_value()).unwrap();
        }

        let db = Db::open(&path).unwrap();
        let loaded = db.load_run("legacy").unwrap().unwrap();
        assert_eq!(loaded.phase, Phase::Shop);
        drop(db);
        cleanup_db_path(&path);
    }
}
