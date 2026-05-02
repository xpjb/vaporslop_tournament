use crate::game::shop::ai_ladder_build;
use crate::game::types::*;
use anyhow::Result;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rusqlite::{params, Connection};
use std::sync::Arc;

const AI_LADDER_STEPS: usize = 100;
const AI_LADDER_GROWTH: f32 = 1.05;

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
                name TEXT NOT NULL,
                money INTEGER NOT NULL,
                wins INTEGER NOT NULL,
                losses INTEGER NOT NULL,
                streak INTEGER NOT NULL,
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
        let db = Db { conn: Arc::new(Mutex::new(conn)) };
        db.maybe_seed()?;
        Ok(db)
    }

    fn maybe_seed(&self) -> Result<()> {
        let count: i64 = self.conn.lock().query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))?;
        if count > 0 { return Ok(()); }
        let names = ["aesthet1c", "vapor", "moonbeam", "y2k", "memehead", "cybr", "lofi", "pix3l", "tokr", "dolphin", "neon", "glitch"];
        let mut target = STARTING_MONEY as f32;
        for i in 0..AI_LADDER_STEPS {
            let name = format!("{}_bot_{:03}", names[i % names.len()], i + 1);
            let build = ai_ladder_build(target.round() as i32);
            let cost = build.cost_value();
            let run = Run {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                money: 0,
                wins: 0,
                losses: 0,
                streak: 0,
                alive: true,
                build,
                shop: Shop::default(),
                phase: Phase::Shop,
            };
            self.upsert_run(&run, cost)?;
            target *= AI_LADDER_GROWTH;
        }
        Ok(())
    }

    pub fn upsert_run(&self, run: &Run, cost: i32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO runs(id,name,money,wins,losses,streak,alive,phase,build_json,shop_json,cost_value,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,strftime('%s','now'))
             ON CONFLICT(id) DO UPDATE SET
               name=excluded.name, money=excluded.money, wins=excluded.wins, losses=excluded.losses,
               streak=excluded.streak, alive=excluded.alive, phase=excluded.phase,
               build_json=excluded.build_json, shop_json=excluded.shop_json,
               cost_value=excluded.cost_value, updated_at=excluded.updated_at",
            params![
                run.id, run.name, run.money, run.wins, run.losses, run.streak,
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
        let mut stmt = conn.prepare("SELECT id,name,money,wins,losses,streak,alive,phase,build_json,shop_json FROM runs WHERE id=?")?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            let phase: String = row.get(7)?;
            let build_json: String = row.get(8)?;
            let shop_json: String = row.get(9)?;
            Ok(Some(Run {
                id: row.get(0)?,
                name: row.get(1)?,
                money: row.get(2)?,
                wins: row.get(3)?,
                losses: row.get(4)?,
                streak: row.get(5)?,
                alive: row.get::<_, i32>(6)? != 0,
                phase: serde_json::from_str(&phase)?,
                build: serde_json::from_str(&build_json)?,
                shop: serde_json::from_str(&shop_json)?,
            }))
        } else { Ok(None) }
    }

    /// Find a matchup whose actual build cost is near the requested gold budget.
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

    pub fn record_streak(&self, name: &str, streak: i32, wins: i32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO leaderboard(name,streak,wins,created_at) VALUES(?,?,?,strftime('%s','now'))",
            params![name, streak, wins],
        )?;
        Ok(())
    }

    pub fn leaderboard(&self) -> Result<Vec<(String, i32, i32)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT name,streak,wins FROM leaderboard ORDER BY streak DESC, wins DESC LIMIT 20")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
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
            name: name.to_string(),
            money: 0,
            wins: 0,
            losses: 0,
            streak: 0,
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
    fn seeds_one_hundred_ladder_runs() {
        let db = test_db();
        let count: i64 = db.conn.lock()
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, AI_LADDER_STEPS as i64);
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
}
