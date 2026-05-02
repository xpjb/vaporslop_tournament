use crate::game::shop::random_build;
use crate::game::types::*;
use anyhow::Result;
use parking_lot::Mutex;
use rand::Rng;
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
        let mut rng = rand::thread_rng();
        let names = ["aesthet1c", "vapor", "moonbeam", "y2k", "memehead", "cybr", "lofi", "pix3l", "tokr", "dolphin", "neon", "glitch"];
        for _ in 0..30 {
            let name = format!("{}_bot", names[rng.gen_range(0..names.len())]);
            let target = rng.gen_range(50..=400);
            let build = random_build(target);
            let cost = build.cost_value();
            let run = Run {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                money: 0,
                wins: rng.gen_range(0..3),
                losses: 0,
                streak: 0,
                alive: true,
                build,
                shop: Shop::default(),
                phase: Phase::Shop,
            };
            self.upsert_run(&run, cost)?;
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

    /// Find a matchup near the run's lifetime gold, including the run itself.
    pub fn find_opponent(&self, target_lifetime_gold: i32) -> Result<Option<(String, String, Build)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id,name,build_json FROM runs
             ORDER BY ABS((?2 + wins * ?3 + losses * ?4) - ?1) ASC
             LIMIT 10"
        )?;
        let mut rows = stmt.query(params![target_lifetime_gold, STARTING_MONEY, WIN_REWARD, LOSE_REWARD])?;
        // Pick a random one of the top 10 to avoid always playing the same opponent.
        let mut candidates: Vec<(String, String, Build)> = vec![];
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let bjson: String = row.get(2)?;
            let build: Build = serde_json::from_str(&bjson)?;
            if !build.team.is_empty() { candidates.push((id, name, build)); }
        }
        if candidates.is_empty() { return Ok(None); }
        let mut rng = rand::thread_rng();
        let idx = rng.gen_range(0..candidates.len().min(5));
        Ok(Some(candidates.into_iter().nth(idx).unwrap()))
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
