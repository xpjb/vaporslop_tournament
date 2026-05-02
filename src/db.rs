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

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(r#"
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
        "#)?;
        ensure_column(&conn, "runs", "player_id", "TEXT")?;
        ensure_column(&conn, "runs", "best_streak", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_column(&conn, "leaderboard", "player_id", "TEXT")?;
        ensure_column(&conn, "leaderboard", "updated_at", "INTEGER")?;
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
        let db_ver: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if db_ver < 2 {
            // One-time: remove ladder rows from older versions (bots are generated on demand now).
            conn.execute(
                "DELETE FROM runs WHERE name GLOB '*_bot_[0-9][0-9][0-9]'",
                [],
            )?;
            conn.pragma_update(None, "user_version", 2)?;
        }
        Ok(Db { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn upsert_run(&self, run: &Run, cost: i32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO runs(id,player_id,name,money,wins,losses,streak,best_streak,alive,phase,build_json,shop_json,cost_value,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,strftime('%s','now'))
             ON CONFLICT(id) DO UPDATE SET
               player_id=excluded.player_id, name=excluded.name, money=excluded.money,
               wins=excluded.wins, losses=excluded.losses, streak=excluded.streak,
               best_streak=excluded.best_streak, alive=excluded.alive, phase=excluded.phase,
               build_json=excluded.build_json, shop_json=excluded.shop_json,
               cost_value=excluded.cost_value, updated_at=excluded.updated_at",
            params![
                run.id, run.player_id, run.name,
                run.money, run.wins, run.losses,
                run.streak, run.best_streak,
                if run.alive { 1 } else { 0 },
                serde_json::to_string(&run.phase)?,
                serde_json::to_string(&run.build)?,
                serde_json::to_string(&run.shop)?,
                cost,
            ],
        )?;
        Ok(())
    }

    pub fn load_run(&self, id: &str) -> Result<Option<Run>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,player_id,name,money,wins,losses,streak,best_streak,alive,phase,build_json,shop_json FROM runs WHERE id=?")?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            let phase: String = row.get(9)?;
            let build_json: String = row.get(10)?;
            let shop_json: String = row.get(11)?;
            Ok(Some(Run {
                id: row.get(0)?,
                player_id: row.get(1)?,
                name: row.get(2)?,
                money: row.get(3)?,
                wins: row.get(4)?,
                losses: row.get(5)?,
                streak: row.get(6)?,
                best_streak: row.get(7)?,
                alive: row.get::<_, i32>(8)? != 0,
                phase: serde_json::from_str(&phase)?,
                build: serde_json::from_str(&build_json)?,
                shop: serde_json::from_str(&shop_json)?,
            }))
        } else { Ok(None) }
    }

    /// Find another player's build whose cost is near the requested gold budget. No bot rows exist in the DB.
    pub fn find_opponent(&self, current_run_id: &str, target_cost: i32) -> Result<Option<(String, String, Build)>> {
        let conn = self.conn.lock();
        let band = ((target_cost as f32) * 0.05).ceil().max(1.0) as i32;
        let min_cost = (target_cost - band).max(0);
        let max_cost = target_cost + band;
        let mut stmt = conn.prepare(
            "SELECT id,name,build_json FROM runs
             WHERE id != ?1 AND cost_value BETWEEN ?2 AND ?3"
        )?;
        let mut rows = stmt.query(params![current_run_id, min_cost, max_cost])?;
        let mut candidates: Vec<(String, String, Build)> = vec![];
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let bjson: String = row.get(2)?;
            let build: Build = serde_json::from_str(&bjson)?;
            if !build.team.is_empty() { candidates.push((id, name, build)); }
        }
        let mut rng = rand::thread_rng();
        if let Some(candidate) = candidates.choose(&mut rng).cloned() {
            return Ok(Some(candidate));
        }

        let mut stmt = conn.prepare(
            "SELECT id,name,build_json FROM runs
             WHERE id != ?1
             ORDER BY ABS(cost_value - ?2) ASC
             LIMIT 10"
        )?;
        let mut rows = stmt.query(params![current_run_id, target_cost])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let bjson: String = row.get(2)?;
            let build: Build = serde_json::from_str(&bjson)?;
            if !build.team.is_empty() {
                return Ok(Some((id, name, build)));
            }
        }

        Ok(None)
    }

    pub fn record_score(&self, player_id: &str, name: &str, streak: i32, wins: i32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO leaderboard(player_id,name,streak,wins,created_at,updated_at)
             VALUES(?,?,?,?,strftime('%s','now'),strftime('%s','now'))
             ON CONFLICT(player_id) DO UPDATE SET
               name=excluded.name,
               streak=CASE
                 WHEN excluded.wins > leaderboard.wins THEN excluded.streak
                 WHEN excluded.wins = leaderboard.wins AND excluded.streak > leaderboard.streak THEN excluded.streak
                 ELSE leaderboard.streak
               END,
               wins=MAX(leaderboard.wins, excluded.wins),
               updated_at=strftime('%s','now')",
            params![player_id, name, streak, wins],
        )?;
        Ok(())
    }

    pub fn leaderboard(&self, page: usize, per_page: usize) -> Result<(Vec<(String, i32, i32)>, usize)> {
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
            "SELECT name,streak,wins FROM leaderboard
             ORDER BY wins DESC, streak DESC, updated_at ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![per_page as i64, offset as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok((rows.filter_map(|r| r.ok()).collect(), page_count))
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
    conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"), [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        Db::open(":memory:").unwrap()
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
            }],
        }
    }

    #[test]
    fn find_opponent_excludes_current_run() {
        let db = test_db();
        db.conn.lock().execute("DELETE FROM runs", []).unwrap();

        let current = run_with_build("current", "current", one_member_build("orang"));
        db.upsert_run(&current, current.build.cost_value()).unwrap();

        assert!(db.find_opponent(&current.id, current.build.cost_value()).unwrap().is_none());

        let opponent = run_with_build("opponent", "opponent", one_member_build("azul_picardia"));
        db.upsert_run(&opponent, opponent.build.cost_value()).unwrap();
        let far = run_with_build("far", "far", one_member_build("elephoont"));
        db.upsert_run(&far, far.build.cost_value()).unwrap();

        let found = db.find_opponent(&current.id, current.build.cost_value()).unwrap().unwrap();
        assert_eq!(found.0, "opponent");
    }

    #[test]
    fn leaderboard_keeps_each_players_best_score() {
        let db = test_db();
        db.conn.lock().execute("DELETE FROM leaderboard", []).unwrap();

        db.record_score("player-1", "old name", 2, 4).unwrap();
        db.record_score("player-1", "new name", 1, 3).unwrap();
        db.record_score("player-2", "winner", 3, 5).unwrap();

        let (entries, pages) = db.leaderboard(1, 1).unwrap();
        assert_eq!(pages, 2);
        assert_eq!(entries, vec![("winner".to_string(), 3, 5)]);

        let (entries, _) = db.leaderboard(2, 1).unwrap();
        assert_eq!(entries, vec![("new name".to_string(), 2, 4)]);
    }
}
